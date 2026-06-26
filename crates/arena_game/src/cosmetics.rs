//! Cue cosmetics — turn `CueMessage`s (dispatched by `arena_skills`' cue-binding layer) into
//! visible, **non-authoritative** effects: an emissive particle burst per particle lane and a
//! flying emissive sphere per projectile lane.
//!
//! This is the guide §5 "simple emissive-billboard stand-in" — pure Bevy, no GPU compute, works
//! under `DefaultPlugins`. The `ParticleSpec` fields map 1:1 onto a future `bevy_vfx::SpawnModule`
//! upgrade with no change to the `LaneEvent` contract. The authoritative hit stays in obelisk's
//! `Hitbox`/`Projectile`; these cosmetics only react to the cues it fires.

use crate::trace;
use arena_skills::{CueKind, CueMessage};
use bevy::prelude::*;
use serde_json::json;
use std::collections::HashMap;

/// Chest/wand-height lift applied to the **OnCast** lane only. The OnCast cue's `position` is the
/// caster's origin (y≈0, at the feet), so the muzzle particle + projectile would spawn inside the
/// robe and be occluded. Raising them ~1.2m reads the muzzle at hand height and flies the projectile
/// from chest height. The OnHit/impact lane keeps the target's actual hit position (no lift).
///
/// This fixed offset is the M1-appropriate stand-in; a real `wand_tip` socket on the rig is later
/// polish.
const MUZZLE_HEIGHT_OFFSET: Vec3 = Vec3::new(0.0, 1.2, 0.0);

/// Per-caster aim direction (normalized), recorded when a cast is issued.
///
/// The `OnCast` `CueEvent` carries only the caster's position (no direction), so the cosmetic
/// projectile can't know which way to fly from the cue alone. The cast system stashes
/// `(target_pos - caster_pos).normalize()` here keyed by the caster `Entity`; `spawn_cue_cosmetics`
/// looks it up by `msg.source` (the caster for an `OnCast` lane). Absent ⇒ default `Vec3::Z`.
#[derive(Resource, Default)]
pub struct AimDirs(pub HashMap<Entity, Vec3>);

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
    #[allow(dead_code)]
    pub speed: f32,
}

/// Read every `CueMessage` dispatched this frame and spawn its cosmetics:
/// - if the lane has a `particle`, an emissive `Rectangle` billboard at `position`;
/// - if the lane has a `projectile`, an emissive `Sphere` at `position` flown along the caster's
///   stashed aim direction (`AimDirs`, defaulting to `Vec3::Z`).
///
/// Emits a `lane_event` trace line per message (guide §6) so the cue dispatch is observable
/// headlessly.
pub fn spawn_cue_cosmetics(
    mut msgs: MessageReader<CueMessage>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    aim: Res<AimDirs>,
) {
    for m in msgs.read() {
        // Observability: one trace line per dispatched cue (guide §6 sample). The cue kind is
        // emitted as `cue_kind` rather than `kind` so it doesn't clobber the trace harness's own
        // top-level `"kind":"lane_event"` (the harness merges `extra` over the base object).
        trace::event(
            "lane_event",
            json!({
                "lane_id": m.lane_id,
                "cue_kind": format!("{:?}", m.kind),
                "source": format!("{:?}", m.source),
                "pos_x": m.position.x,
                "pos_y": m.position.y,
                "pos_z": m.position.z,
                "has_particle": m.event.particle.is_some(),
                "has_projectile": m.event.projectile.is_some(),
            }),
        );

        // Spawn position: raise the OnCast lane (muzzle + projectile) to wand/chest height so it
        // clears the robe; leave OnHit (impact) and any other lane at the cue's reported position.
        let spawn_pos = if m.kind == CueKind::OnCast {
            m.position + MUZZLE_HEIGHT_OFFSET
        } else {
            m.position
        };

        // 1) Particle burst (emissive billboard stand-in).
        if let Some(p) = &m.event.particle {
            let c = LinearRgba::rgb(p.color[0], p.color[1], p.color[2]);
            let material = materials.add(StandardMaterial {
                emissive: c * 2.0,
                base_color: Color::from(c),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            });
            commands.spawn((
                Mesh3d(meshes.add(Rectangle::new(0.25, 0.25))),
                MeshMaterial3d(material),
                Transform::from_translation(spawn_pos),
                ParticleLifetime {
                    elapsed: 0.0,
                    duration: p.lifetime,
                },
            ));
        }

        // 2) Cosmetic flying projectile (OnCast lane only, for firebolt).
        if let Some(proj) = &m.event.projectile {
            // Direction: the caster's stashed aim (set when the cast was issued). The OnCast cue's
            // `source` IS the caster, so look it up by `m.source`. Default to +Z if unknown.
            let dir = aim.0.get(&m.source).copied().unwrap_or(Vec3::Z);
            let c = LinearRgba::rgb(proj.color[0], proj.color[1], proj.color[2]);
            commands.spawn((
                Mesh3d(meshes.add(Sphere::new(proj.radius))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    emissive: c * 3.0,
                    base_color: Color::from(c),
                    unlit: true,
                    ..default()
                })),
                Transform::from_translation(spawn_pos),
                CosmeticProjectile {
                    velocity: dir * proj.speed,
                    speed: proj.speed,
                },
                ParticleLifetime {
                    elapsed: 0.0,
                    duration: 2.0, // matches the .cast.ron window active_duration
                },
            ));
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
