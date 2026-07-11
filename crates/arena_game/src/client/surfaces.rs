//! Client-side surfaces (spec §6-§7). This module carries the HEADLESS-SAFE trace system
//! (both client roots register it — the net-test asserts replication reached every observer) and
//! the windowed-only visuals plugin (`SurfaceVisualsPlugin`: decals + optional looping vfx).
use avian3d::prelude::{RigidBody, SpatialQuery, SpatialQueryFilter};
use bevy::pbr::decal::{ForwardDecal, ForwardDecalMaterial, ForwardDecalMaterialExt};
use bevy::prelude::*;
use obelisk_bevy::surfaces::SurfaceRegistry;
use serde_json::json;

use crate::net::protocol::NetworkedSurfacePatch;
use crate::trace;

/// Trace every replicated patch as it materializes (headless + windowed — the harness signal).
pub(crate) fn trace_replicated_patches(
    q: Query<&NetworkedSurfacePatch, Added<NetworkedSurfacePatch>>,
) {
    for p in &q {
        trace::event("replicated_surface_patch", json!({ "surface": p.surface }));
    }
}

/// Windowed-only visuals for replicated surface patches (spec §6/D10): one tinted
/// [`ForwardDecal`] (the projected `decal_splat.png`) per [`NetworkedSurfacePatch`], plus the
/// surface's optional looping vfx preset. Everything spawns as a CHILD of the replicated entity —
/// lightyear's despawn replication (decay/consume/evict/round reset) recursively removes the
/// visuals with the patch, so neither child carries its own lifetime.
///
/// Registered in `app_windowed.rs` ONLY: `ForwardDecal` needs `PbrPlugin`'s render infra — the
/// `Assets<ForwardDecalMaterial<StandardMaterial>>` store + the camera depth prepass — which the
/// headless client / observer never add.
pub struct SurfaceVisualsPlugin;

impl Plugin for SurfaceVisualsPlugin {
    fn build(&self, app: &mut App) {
        // `PbrPlugin` already registers `MaterialPlugin::<ForwardDecalMaterial<StandardMaterial>>`
        // (via its `ForwardDecalPlugin`, bevy_pbr 0.18.1), which also inserts the unit-quad
        // `ForwardDecalMesh` the `ForwardDecal` on-add hook requires. This guarded add is therefore
        // a defensive no-op under the current bevy — kept idempotent so a future PbrPlugin that
        // drops the auto-registration doesn't silently break decals.
        if !app.is_plugin_added::<MaterialPlugin<ForwardDecalMaterial<StandardMaterial>>>() {
            app.add_plugins(MaterialPlugin::<ForwardDecalMaterial<StandardMaterial>>::default());
        }
        app.add_systems(Update, attach_surface_visuals);
    }
}

/// Attach the decal (+ optional looping vfx) to every freshly-replicated patch. `Added<..>` fires
/// the frame the patch materializes; `Position` rides the SAME replication group so it is present
/// that frame (atomic insert). The patch is STATIC — stamp its render `Transform` once from
/// `Position` and hang the visuals as children.
///
/// `registry`/`vfx` are `Option<Res<_>>` (headless-safe even though this plugin is windowed-only):
/// a missing registry falls back to a neutral splat, a missing library skips the vfx.
fn attach_surface_visuals(
    q: Query<
        (Entity, &NetworkedSurfacePatch, &avian3d::prelude::Position),
        Added<NetworkedSurfacePatch>,
    >,
    registry: Option<Res<SurfaceRegistry>>,
    asset_server: Res<AssetServer>,
    mut decal_materials: ResMut<Assets<ForwardDecalMaterial<StandardMaterial>>>,
    // Per-surface-type decal material cache (see the attach loop for the static-registry caveat).
    mut material_cache: Local<
        std::collections::HashMap<String, Handle<ForwardDecalMaterial<StandardMaterial>>>,
    >,
    vfx: Option<Res<bevy_vfx::VfxLibrary>>,
    // Ground-snap the decals (see the attach block): a downward ray onto STATIC level geometry.
    spatial: SpatialQuery,
    static_bodies: Query<&RigidBody>,
    mut commands: Commands,
) {
    for (e, p, pos) in &q {
        let visuals = registry
            .as_ref()
            .and_then(|r| r.0.get(&p.surface))
            .and_then(|s| s.visuals.clone())
            .unwrap_or_default();
        let color = visuals
            .color
            .map(|c| Color::srgba(c[0], c[1], c[2], c[3]))
            .unwrap_or(Color::srgba(1.0, 1.0, 1.0, 0.8));
        let texture = visuals
            .decal
            .as_deref()
            .unwrap_or("textures/decal_splat.png")
            .to_string();
        // The replicated patch entity carries `Position` (replicated) but no render `Transform` —
        // patches are STATIC, so stamp the Transform once and hang the children under it.
        commands
            .entity(e)
            .insert((Transform::from_translation(pos.0), Visibility::default()));

        // One ForwardDecal material per surface TYPE, not per patch: a surface's registry visuals
        // are STATIC at runtime, so every patch of a given surface shares one handle. NOTE: a future
        // hot-reload of the surface TOMLs would mutate a type's visuals and MUST invalidate this
        // cache (drop the changed surface's entry) — there is no reload path today.
        let material = material_cache
            .entry(p.surface.clone())
            .or_insert_with(|| {
                decal_materials.add(ForwardDecalMaterial {
                    base: StandardMaterial {
                        base_color: color,
                        base_color_texture: Some(asset_server.load(&texture)),
                        alpha_mode: AlphaMode::Blend,
                        perceptual_roughness: 1.0,
                        ..default()
                    },
                    extension: ForwardDecalMaterialExt {
                        depth_fade_factor: 1.0,
                    },
                })
            })
            .clone();

        // Ground-snap the decal (+ vfx) to the floor. bevy 0.18 `ForwardDecal` is a FLAT +Y quad:
        // scale.y is INERT (there is no Y extent to grow — the old `y_span` scale did nothing),
        // depth_compare is `Always` (never occluded), and `depth_fade_factor` bounds the projection
        // (1.0 => ~1 m). An ELEVATED quad therefore floats, parallax-smears at grazing view angles,
        // and draws OVER characters standing in it. So snap the VISUAL flush to the ground — then
        // only sub-1m receivers (the floor, feet) catch it. The patch entity keeps its authored
        // `Position` Y (gameplay is server-side `SURFACE_Y_TOLERANCE`-based); this offset lives on
        // the render child alone.
        let patch_pos = pos.0;
        let origin = patch_pos + Vec3::Y * 2.0;
        let ground_y = spatial
            .cast_ray_predicate(
                origin,
                Dir3::NEG_Y,
                50.0,
                true,
                &SpatialQueryFilter::default(),
                // STATIC bodies only: a Dynamic combatant standing on the paint point is rejected
                // by the predicate (`false` = skip, keep travelling). Skill objects (including
                // settled frost_spires, which ARE Static server-side) and patches are safe for a
                // DIFFERENT reason — RigidBody/Collider never replicate, so client-side they have
                // no collider to catch the ray at all. This marker-less heuristic trusts "nearest
                // static below" (the editor uses a precise floor marker instead): on a multi-tier
                // level the decal grounds on the platform top — where a ground decal belongs —
                // but revisit if skill objects ever gain client colliders.
                &|entity| static_bodies.get(entity).is_ok_and(|rb| rb.is_static()),
            )
            .map(|hit| origin.y - hit.distance)
            // Flat-stage fallback: the arena_flat floor's top face is world Y = 0 (level data).
            .unwrap_or(0.0);
        // Child-LOCAL Y that lands the child's WORLD Y on the ground + a 1 cm bias off the floor.
        let visual_y = ground_y - patch_pos.y + 0.01;

        let decal = commands
            .spawn((
                Name::new(format!("SurfaceDecal({})", p.surface)),
                ForwardDecal,
                MeshMaterial3d(material),
                // Ground-snapped (child-local `visual_y`), XZ = diameter. scale.y is 1.0: the quad
                // has NO Y extent to scale (see the ground-snap note above — `y_span` is gone for
                // good; a raw scale never reached the floor, it only stretched a flat quad).
                Transform::from_xyz(0.0, visual_y, 0.0)
                    .with_scale(Vec3::new(p.radius * 2.0, 1.0, p.radius * 2.0)),
            ))
            .id();
        commands.entity(e).add_child(decal);

        // Optional looping vfx (e.g. burning's "Embers"): reuse cosmetics.rs's VfxLibrary spawn
        // tier (`resolve_vfx_effect`) to turn the authored name into a live `VfxSystem`, then
        // parent it under the patch with NO `ParticleLifetime` — it loops for the patch's life and
        // despawns-with-parent. A surface authors no `CueParam`s, so pass `&[]`/`0.0`.
        if let (Some(vfx_name), Some(vfx_lib)) = (visuals.vfx.as_deref(), vfx.as_ref()) {
            if let Some(system) = super::cosmetics::resolve_vfx_effect(vfx_lib, vfx_name, &[], 0.0) {
                let fx = commands
                    .spawn((
                        Name::new(format!("SurfaceVfx({})", p.surface)),
                        // Same ground-snap as the decal so embers sit on the floor, not at the
                        // patch's authored (elevated) Y.
                        Transform::from_xyz(0.0, visual_y, 0.0),
                        Visibility::default(),
                        system,
                    ))
                    .id();
                commands.entity(e).add_child(fx);
            }
        }
    }
}
