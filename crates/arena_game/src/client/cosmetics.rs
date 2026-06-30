//! Cue cosmetics — turn `CueMessage`s (dispatched by `arena_skills`' cue-binding layer) into
//! visible, **non-authoritative** effects: an emissive particle burst per particle lane and a
//! flying emissive sphere per projectile lane.
//!
//! This is the guide §5 "simple emissive-billboard stand-in" — pure Bevy, no GPU compute, works
//! under `DefaultPlugins`. The `ParticleSpec` fields map 1:1 onto a future `bevy_vfx::SpawnModule`
//! upgrade with no change to the `LaneEvent` contract. The authoritative hit stays in obelisk's
//! `Hitbox`/`Projectile`; these cosmetics only react to the cues it fires.

use crate::trace;
use arena_skills::{resolve_cue, CueKind, CueMessage, SkillFxRegistry};
use bevy::prelude::*;
use serde_json::json;
use std::collections::HashMap;

/// `arena_game`-local bevy `Message` wrapping the engine-neutral serde [`CueMessage`].
///
/// `arena_skills::CueMessage` is a plain serde wire type (no bevy `Message` derive) so `arena_skills`
/// stays lightyear-free. `LocalCue` is the in-process Bevy channel that carries cues into the
/// cosmetics consumer ([`spawn_cue_cosmetics`], which needs `Res` access). It is fed from two sides:
/// `skills::consume_replicated_cues` forwards each survivor of the replicated `CueWireMessage` drain,
/// and `skills::predicted_local_cast` emits the local player's own on-cast cue immediately (so the
/// caster sees zero-latency cosmetics without waiting for the server round-trip).
#[derive(Message, Clone, Debug)]
pub struct LocalCue(pub CueMessage);

/// Chest/wand-height lift applied to the **OnCast** lane only. The OnCast cue's `position` is the
/// caster's origin (= the body CENTER, world y≈0.59), so the muzzle particle + projectile would spawn
/// inside the torso. Raising them ~1.2m reads the muzzle at hand height and flies the projectile from
/// chest height. The OnHit/impact lane keeps the target's actual hit position (no lift).
///
/// This fixed offset is a stand-in; a real `wand_tip` socket on the rig is later polish.
const MUZZLE_HEIGHT_OFFSET: Vec3 = Vec3::new(0.0, 1.2, 0.0);

/// Per-caster aim direction (normalized), recorded when a cast is issued, keyed by the caster's
/// stable `ObeliskId` string.
///
/// This is the FALLBACK aim source: a `CueMessage` now carries its own `aim_dir` (the wire cue is the
/// single source of truth — see [`spawn_cue_cosmetics`]), but for the local predicted cast,
/// `skills::predicted_local_cast` also stashes the camera-forward `aim_dir` here keyed by the caster's
/// `ObeliskId`, so `spawn_cue_cosmetics` has a direction even before the wire cue arrives. Looked up
/// by `msg.source_id` (the caster for an `OnCast` lane); absent ⇒ default `Vec3::Z`.
///
/// Keyed by the stable `ObeliskId` (not `Entity`) because replicated entity ids differ per process;
/// `ObeliskId` is the only key both ends agree on, matching the serde `CueMessage.source_id`.
#[derive(Resource, Default)]
pub struct AimDirs(pub HashMap<String, Vec3>);

/// Cached cosmetic mesh + material handles, so each firebolt cast reuses handles instead of growing
/// `Assets<Mesh>`/`Assets<StandardMaterial>` every cast.
///
/// The meshes are UNIT primitives (a 0.25×0.25 particle quad — fixed size — and a radius-1 sphere
/// scaled to the projectile radius via `Transform`), built once in [`init_cosmetic_assets`] at
/// startup. The material maps are keyed by quantized color BITS (the cue colors are a tiny fixed
/// palette) and lane kind, since particle vs projectile materials differ (emissive multiplier +
/// alpha/blend), so repeated casts of the same spell hit the cache instead of allocating.
#[derive(Resource)]
pub struct CosmeticAssets {
    /// Unit particle billboard quad (`Rectangle::new(0.25, 0.25)`).
    quad: Handle<Mesh>,
    /// Unit sphere (radius 1.0); scaled to the projectile radius per-spawn via `Transform`.
    sphere: Handle<Mesh>,
    /// Particle (billboard) materials, keyed by quantized color bits.
    particle_mats: HashMap<[u32; 3], Handle<StandardMaterial>>,
    /// Projectile (sphere) materials, keyed by quantized color bits.
    projectile_mats: HashMap<[u32; 3], Handle<StandardMaterial>>,
}

/// Quantize an `[f32; 3]` color to its raw bit pattern so it can key a `HashMap` (floats aren't
/// `Eq`/`Hash`). The cue colors come from a fixed `.skillfx.ron` palette, so equal colors share bits.
fn color_key(c: [f32; 3]) -> [u32; 3] {
    [c[0].to_bits(), c[1].to_bits(), c[2].to_bits()]
}

/// Startup: build the unit cosmetic meshes once and insert the [`CosmeticAssets`] cache (with empty
/// material maps, filled lazily on first cast of each color).
pub fn init_cosmetic_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(CosmeticAssets {
        quad: meshes.add(Rectangle::new(0.25, 0.25)),
        sphere: meshes.add(Sphere::new(1.0)),
        particle_mats: HashMap::new(),
        projectile_mats: HashMap::new(),
    });
}

/// A short-lived cosmetic entity (particle billboard or cosmetic projectile). Despawned by
/// `age_lifetimes` once `elapsed >= duration`.
#[derive(Component)]
pub struct ParticleLifetime {
    pub elapsed: f32,
    pub duration: f32,
}

/// A cosmetic, non-authoritative projectile flown by the game. `velocity` is `aim_dir * speed`
/// (the `.cast.ron` window motion speed, here 20.0 for firebolt) so the visual mesh tracks the
/// authoritative obelisk hitbox.
#[derive(Component)]
pub struct CosmeticProjectile {
    pub velocity: Vec3,
}

/// Read every [`LocalCue`] dispatched this frame, resolve its lanes from the [`SkillFxRegistry`]
/// (`resolve_cue` re-looks-up by `cue_id` — the serde `CueMessage` carries no embedded lane), and
/// spawn each lane's cosmetics:
/// - if the lane has a `particle`, an emissive `Rectangle` billboard at `position`;
/// - if the lane has a `projectile`, an emissive `Sphere` at `position` flown along the caster's
///   stashed aim direction (`AimDirs`, keyed by `source_id`, defaulting to `Vec3::Z`).
///
/// Emits one `lane_event` trace line per resolved lane (guide §6) so the cue dispatch is observable
/// headlessly — the M1 regression gate greps the trace for these.
pub fn spawn_cue_cosmetics(
    mut msgs: MessageReader<LocalCue>,
    registry: Res<SkillFxRegistry>,
    mut commands: Commands,
    mut assets: ResMut<CosmeticAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    aim: Res<AimDirs>,
) {
    for LocalCue(m) in msgs.read() {
        // Re-look-up the lanes bound to this cue id (an unbound cue resolves to an empty slice and
        // no-ops — spec §12: never crash on missing content).
        let lanes = resolve_cue(&registry, m);

        // Spawn position: raise the OnCast lane (muzzle + projectile) to wand/chest height so it
        // clears the robe; leave OnHit (impact) and any other lane at the cue's reported position.
        let spawn_pos = if m.kind == CueKind::OnCast {
            m.position + MUZZLE_HEIGHT_OFFSET
        } else {
            m.position
        };

        for lane in lanes {
            // Observability: one trace line per dispatched lane (guide §6 sample). The cue kind is
            // emitted as `cue_kind` rather than `kind` so it doesn't clobber the trace harness's own
            // top-level `"kind":"lane_event"` (the harness merges `extra` over the base object).
            trace::event(
                "lane_event",
                json!({
                    "lane_id": lane.lane_id,
                    "cue_id": m.cue_id,
                    "cue_kind": format!("{:?}", m.kind),
                    "source_id": m.source_id,
                    "pos_x": m.position.x,
                    "pos_y": m.position.y,
                    "pos_z": m.position.z,
                    "has_particle": lane.particle.is_some(),
                    "has_projectile": lane.projectile.is_some(),
                }),
            );

            // 1) Particle burst (emissive billboard stand-in). Reuse the cached unit quad + a
            // color-keyed material so repeated casts don't grow the asset stores.
            if let Some(p) = &lane.particle {
                let c = LinearRgba::rgb(p.color[0], p.color[1], p.color[2]);
                let material = assets
                    .particle_mats
                    .entry(color_key(p.color))
                    .or_insert_with(|| {
                        materials.add(StandardMaterial {
                            emissive: c * 2.0,
                            base_color: Color::from(c),
                            alpha_mode: AlphaMode::Blend,
                            unlit: true,
                            ..default()
                        })
                    })
                    .clone();
                commands.spawn((
                    Mesh3d(assets.quad.clone()),
                    MeshMaterial3d(material),
                    Transform::from_translation(spawn_pos),
                    ParticleLifetime {
                        elapsed: 0.0,
                        duration: p.lifetime,
                    },
                ));
            }

            // 2) Cosmetic flying projectile (OnCast lane only, for firebolt).
            if let Some(proj) = &lane.projectile {
                // Direction (Bug 1b): prefer the cue's OWN aim_dir (carried over the wire from the
                // server, so an OBSERVER flies the bolt the right way). Fall back to the local
                // `AimDirs` lookup (the local-prediction path stashes it keyed by `ObeliskId` — the
                // OnCast cue's `source_id` IS the caster), then to +Z.
                let dir = if m.aim_dir != Vec3::ZERO {
                    m.aim_dir
                } else {
                    aim.0.get(&m.source_id).copied().unwrap_or(Vec3::Z)
                };
                let c = LinearRgba::rgb(proj.color[0], proj.color[1], proj.color[2]);
                // Reuse the cached unit sphere (scaled to `proj.radius` via Transform) + a
                // color-keyed material so each cast reuses handles instead of growing the stores.
                let material = assets
                    .projectile_mats
                    .entry(color_key(proj.color))
                    .or_insert_with(|| {
                        materials.add(StandardMaterial {
                            emissive: c * 3.0,
                            base_color: Color::from(c),
                            unlit: true,
                            ..default()
                        })
                    })
                    .clone();
                commands.spawn((
                    Mesh3d(assets.sphere.clone()),
                    MeshMaterial3d(material),
                    Transform::from_translation(spawn_pos).with_scale(Vec3::splat(proj.radius)),
                    CosmeticProjectile {
                        velocity: dir * proj.speed,
                    },
                    ParticleLifetime {
                        elapsed: 0.0,
                        duration: 2.0, // matches the .cast.ron window active_duration
                    },
                ));
            }
        }
    }
}

/// Advance each cosmetic projectile by `velocity * delta`. Render-time motion (uses `Time`, the
/// per-frame clock) — purely visual, never touches the obelisk sim.
pub fn fly_cosmetic_projectiles(
    time: Res<Time>,
    mut q: Query<(&CosmeticProjectile, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (proj, mut tf) in &mut q {
        tf.translation += proj.velocity * dt;
    }
}

/// Age every `ParticleLifetime` and despawn it once it has outlived its `duration`.
pub fn age_lifetimes(
    time: Res<Time>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut ParticleLifetime)>,
) {
    let dt = time.delta_secs();
    for (e, mut life) in &mut q {
        life.elapsed += dt;
        if life.elapsed >= life.duration {
            commands.entity(e).despawn();
        }
    }
}
