//! Client-side surfaces (spec §6-§7). This module carries the HEADLESS-SAFE trace system
//! (both client roots register it — the net-test asserts replication reached every observer);
//! Task 3 adds the windowed visuals plugin alongside.
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
    vfx: Option<Res<bevy_vfx::VfxLibrary>>,
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

        let decal = commands
            .spawn((
                Name::new(format!("SurfaceDecal({})", p.surface)),
                ForwardDecal,
                MeshMaterial3d(decal_materials.add(ForwardDecalMaterial {
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
                })),
                // ForwardDecal's unit quad projects within its scaled box: XZ = diameter,
                // Y = projection depth (enough to catch gentle slopes / spire bases).
                Transform::from_scale(Vec3::new(p.radius * 2.0, 1.0, p.radius * 2.0)),
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
                        Transform::default(),
                        Visibility::default(),
                        system,
                    ))
                    .id();
                commands.entity(e).add_child(fx);
            }
        }
    }
}
