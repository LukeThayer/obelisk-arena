//! Cue cosmetics — turn `CueMessage`s (dispatched by `arena_skills`' cue-binding layer) into
//! visible, **non-authoritative** effects: an emissive particle burst per particle lane and a
//! flying emissive sphere per projectile lane.
//!
//! This is the guide §5 "simple emissive-billboard stand-in" — pure Bevy, no GPU compute, works
//! under `DefaultPlugins`. The `ParticleSpec` fields map 1:1 onto a future `bevy_vfx::SpawnModule`
//! upgrade with no change to the `LaneEvent` contract. The authoritative hit stays in obelisk's
//! `Hitbox`/`Projectile`; these cosmetics only react to the cues it fires.

use crate::client::vfx_bind::bake_bindings;
use crate::trace;
use arena_skills::{
    resolve_cue, CueKind, CueMessage, SkillFxRegistry, VfxBindSource, VfxParamBinding,
};
use bevy::prelude::*;
use bevy_vfx::VfxLibrary;
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

/// PreStartup (windowed client only): seed the `bevy_vfx` [`VfxLibrary`] with the built-in presets
/// (`fire`/`explosion`/`sparks`/… — the same set the skill designer offers) + any authored overrides
/// in `assets/vfx/` or `assets/skills/*.vfx.ron`. A `.skillfx.ron` particle/projectile lane whose
/// `effect` names one of these renders the real GPU effect in-game; lanes with no `effect` keep the
/// emissive-billboard stand-in. Mirrors the editor's `init_vfx_library` so authored → in-game is 1:1.
pub fn init_vfx_library(mut library: ResMut<VfxLibrary>) {
    for (name, system) in bevy_vfx::presets::default_presets() {
        library.effects.entry(name.to_string()).or_insert(system);
    }
    for dir in ["assets/vfx", "assets/skills"] {
        load_vfx_presets_from_dir(&mut library, dir);
    }
}

/// Load every `<name>.vfx.ron` in `dir` (a `bevy_vfx::VfxSystem`) into the library keyed by `<name>`,
/// overriding built-ins. Missing dir / unparseable file = skip with a warn (never crash on content).
fn load_vfx_presets_from_dir(library: &mut VfxLibrary, dir: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(name) = fname.strip_suffix(".vfx.ron").filter(|n| !n.is_empty()) else {
            continue;
        };
        match std::fs::read_to_string(&path).map(|s| ron::from_str::<bevy_vfx::VfxSystem>(&s)) {
            Ok(Ok(system)) => {
                library.effects.insert(name.to_string(), system);
            }
            other => warn!("skipping vfx preset {path:?}: {other:?}"),
        }
    }
}

/// Try to spawn an authored `bevy_vfx` effect for a cosmetic lane. Returns `true` if it spawned the
/// real GPU effect (caller then skips the billboard stand-in), `false` to fall back. Clones the named
/// `VfxLibrary` preset, CPU-bakes its `VfxParamBinding`s from the live `charge` (stat sources → 0.0,
/// the math is proven in `arena_skills`), and spawns it at `pos` + a `ParticleLifetime` so it despawns.
/// `extra` lets the caller attach a `CosmeticProjectile` so a projectile effect flies.
#[allow(clippy::too_many_arguments)]
fn spawn_lane_vfx(
    commands: &mut Commands,
    library: Option<&VfxLibrary>,
    effect: Option<&str>,
    pos: Vec3,
    offset: Vec3,
    bindings: &[VfxParamBinding],
    charge: f32,
    duration: f32,
    extra: Option<CosmeticProjectile>,
) -> bool {
    let Some(mut system) = effect
        .zip(library)
        .and_then(|(name, lib)| lib.effects.get(name).cloned())
    else {
        return false;
    };
    bake_bindings(&mut system, bindings, |b| match &b.source {
        VfxBindSource::Charge => charge,
        VfxBindSource::Stat { .. } => 0.0,
    });
    let mut e = commands.spawn((
        system,
        Transform::from_translation(pos + offset),
        Visibility::default(),
        ParticleLifetime {
            elapsed: 0.0,
            duration: duration.max(0.5),
        },
    ));
    if let Some(proj) = extra {
        e.insert(proj);
    }
    true
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
    /// Downward acceleration (units/s²), mirroring the authoritative `Projectile.gravity` so a
    /// ballistic bolt's cosmetic flies the same arc as the invisible hitbox. `0.0` = straight.
    pub gravity: f32,
    /// The end-cue id whose arrival terminates this cosmetic (the sim's authoritative
    /// where/when the bolt stopped). `None` = legacy fixed-lifetime flight.
    pub end_cue: Option<String>,
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
#[allow(clippy::too_many_arguments)]
pub fn spawn_cue_cosmetics(
    mut msgs: MessageReader<LocalCue>,
    registry: Res<SkillFxRegistry>,
    mut commands: Commands,
    mut assets: ResMut<CosmeticAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    aim: Res<AimDirs>,
    // Present only in the windowed client (the headless path never adds `bevy_vfx::VfxPlugin`, so
    // this is `None` there and every lane falls back to the billboard — keeping the headless net-test
    // path free of any GPU-particle dependency).
    vfx_library: Option<Res<VfxLibrary>>,
    flying: Query<(Entity, &CosmeticProjectile)>,
) {
    // Charge fraction used to bake `VfxBindSource::Charge` params. The `LocalCue` doesn't carry the
    // cast's charge, so bake at full strength for now (stat-driven params fall back to 0.0). Threading
    // the real per-cast charge is a later refinement.
    let charge = 1.0;
    let library = vfx_library.as_deref();
    for LocalCue(m) in msgs.read() {
        // An END cue is the sim saying "the bolt stopped HERE" — terminate every cosmetic
        // projectile bound to it (its lanes below then render the ending, e.g. the explosion,
        // at the cue position). This closes the visual/sim loop: the cosmetic can't outfly or
        // undershoot the authoritative hitbox.
        if m.kind == CueKind::OnEnd {
            for (e, proj) in &flying {
                if proj.end_cue.as_deref() == Some(m.cue_id.as_str()) {
                    commands.entity(e).try_despawn();
                }
            }
        }

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

            // 1) Particle burst. Prefer the authored `bevy_vfx` effect (real GPU particles, as in the
            // designer preview); fall back to the emissive-billboard stand-in when the lane names no
            // effect / it's missing from the library / the headless path has no VfxLibrary.
            if let Some(p) = &lane.particle {
                let spawned_vfx = spawn_lane_vfx(
                    &mut commands,
                    library,
                    p.effect.as_deref(),
                    spawn_pos,
                    p.offset,
                    &p.param_bindings,
                    charge,
                    p.lifetime,
                    None,
                );
                if !spawned_vfx {
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
            }

            // 1b) Two-anchor beam arc: `segments` bursts sampled along the cue's
            // position_from→position segment (a beam window's open cue carries both anchors).
            // No second anchor = nothing to draw (an authoring mismatch, not a crash).
            if let (Some(b), Some(from)) = (&lane.beam, m.position_from) {
                let to = m.position;
                let n = b.segments.max(2) as usize;
                for i in 0..n {
                    let t = i as f32 / (n - 1) as f32;
                    let p = from.lerp(to, t);
                    let spawned_vfx = spawn_lane_vfx(
                        &mut commands,
                        library,
                        b.effect.as_deref(),
                        p,
                        Vec3::ZERO,
                        &[],
                        charge,
                        b.lifetime,
                        None,
                    );
                    if !spawned_vfx {
                        let c = LinearRgba::rgb(b.color[0], b.color[1], b.color[2]);
                        let material = assets
                            .particle_mats
                            .entry(color_key(b.color))
                            .or_insert_with(|| {
                                materials.add(StandardMaterial {
                                    emissive: c * 3.0,
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
                            Transform::from_translation(p).with_scale(Vec3::splat(0.35)),
                            ParticleLifetime {
                                elapsed: 0.0,
                                duration: b.lifetime,
                            },
                        ));
                    }
                }
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
                let velocity = dir * proj.speed;
                let gravity = proj.gravity;
                let end_cue = proj.end_cue.clone();
                // Prefer an authored `bevy_vfx` trail effect flown along `velocity`; else the emissive
                // sphere stand-in. Both carry `CosmeticProjectile` so they track the obelisk hitbox.
                let spawned_vfx = spawn_lane_vfx(
                    &mut commands,
                    library,
                    proj.effect.as_deref(),
                    spawn_pos,
                    Vec3::ZERO,
                    &[],
                    charge,
                    2.0,
                    Some(CosmeticProjectile {
                        velocity,
                        gravity,
                        end_cue: end_cue.clone(),
                    }),
                );
                if !spawned_vfx {
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
                            velocity,
                            gravity,
                            end_cue,
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
}

/// Advance each cosmetic projectile: gravity into velocity, then velocity into position
/// (semi-implicit Euler, the same integration obelisk's `move_projectiles` runs on the
/// authoritative hitbox). Render-time motion (uses `Time`, the per-frame clock) — purely visual,
/// never touches the obelisk sim.
pub fn fly_cosmetic_projectiles(
    time: Res<Time>,
    mut q: Query<(&mut CosmeticProjectile, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (mut proj, mut tf) in &mut q {
        proj.velocity.y -= proj.gravity * dt;
        let velocity = proj.velocity;
        tf.translation += velocity * dt;
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
