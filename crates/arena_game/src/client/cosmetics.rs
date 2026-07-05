//! Cue cosmetics — the client-side consumer of dispatched [`LocalCue`]s.
//!
//! Cue **rendering** is stubbed here: the reform replaced the old cosmetic-binding crate's
//! lane-binding format with obelisk's own `CueBinding`, which needs `bevy_effect` (not yet
//! available) to resolve into GPU particles — that lands in a later task (C3). Until then, this
//! module only (a) despawns a flying [`CosmeticProjectile`] when its bound `OnEnd` cue arrives (the
//! sim's authoritative "the bolt stopped HERE" signal) and (b) traces every dispatched cue
//! (`cue_dispatch`) so the wire keeps flowing observably. The authoritative hit stays in obelisk's
//! `Hitbox`/`Projectile`; these cosmetics only ever react to the cues it fires, never resolve them.

use crate::trace;
use bevy::prelude::*;
use bevy_vfx::VfxLibrary;
use serde_json::json;
use std::collections::HashMap;

/// `arena_game`-local bevy `Message` wrapping the engine-neutral serde
/// [`crate::net::cue::CueMessage`].
///
/// `crate::net::cue::CueMessage` is a plain serde wire type (no bevy `Message` derive), so it stays
/// lightyear-free. `LocalCue` is the in-process Bevy channel that carries cues into the cosmetics
/// consumer ([`spawn_cue_cosmetics`]). It is fed from two sides: `skills::consume_replicated_cues`
/// forwards each survivor of the replicated `CueWireMessage` drain, and `skills::predicted_local_cast`
/// emits the local player's own on-cast cue immediately — though until C3 restores rendering, the
/// only observable effect of either path is the `cue_dispatch` trace + the `OnEnd` despawn below.
#[derive(Message, Clone, Debug)]
pub struct LocalCue(pub crate::net::cue::CueMessage);

/// Chest/wand-height lift applied to the **OnCast** lane only. The OnCast cue's `position` is the
/// caster's origin (= the body CENTER, world y≈0.59), so the muzzle particle + projectile would spawn
/// inside the torso. Raising them ~1.2m reads the muzzle at hand height and flies the projectile from
/// chest height. The OnHit/impact lane keeps the target's actual hit position (no lift).
///
/// This fixed offset is a stand-in; a real `wand_tip` socket on the rig is later polish. Currently
/// unread by the stubbed [`spawn_cue_cosmetics`] — C3 reuses it when it restores rendering.
const MUZZLE_HEIGHT_OFFSET: Vec3 = Vec3::new(0.0, 1.2, 0.0);

/// Per-caster aim direction (normalized), recorded when a cast is issued, keyed by the caster's
/// stable `ObeliskId` string.
///
/// This is the FALLBACK aim source: a `CueMessage` carries its own `aim_dir` (the wire cue is meant
/// to be the single source of truth once rendering resumes), but for the local predicted cast,
/// `skills::predicted_local_cast` also stashes the camera-forward `aim_dir` here keyed by the caster's
/// `ObeliskId`. Currently unread by the stubbed [`spawn_cue_cosmetics`] — C3 reuses this lookup when
/// it restores the flying cosmetic projectile.
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
/// alpha/blend), so repeated casts of the same spell hit the cache instead of allocating. Currently
/// unpopulated by the stubbed [`spawn_cue_cosmetics`] — C3 resumes writing into these caches when it
/// restores rendering.
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
/// `Eq`/`Hash`). The cue colors come from a small fixed palette, so equal colors share bits.
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
/// in `assets/vfx/` or `assets/skills/*.vfx.ron`, so a later cue-binding consumer (C3) can spawn a
/// named effect by key. Mirrors the editor's `init_vfx_library` so authored → in-game stays 1:1.
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

/// Read every [`LocalCue`] dispatched this frame. **Stubbed** (C3 restores real rendering via
/// obelisk's `CueBinding` + `bevy_effect`): the only behavior kept here is (a) an `OnEnd` cue
/// despawning any [`CosmeticProjectile`] bound to it via `end_cue` (the sim's authoritative
/// "the bolt stopped HERE" signal — the visual can't outfly or undershoot the real hitbox even
/// with no lane rendering) and (b) a `cue_dispatch` trace line per cue so the wire stays observable
/// headlessly. No particles, beams, or projectiles are spawned by this stub.
pub fn spawn_cue_cosmetics(
    mut msgs: MessageReader<LocalCue>,
    mut commands: Commands,
    flying: Query<(Entity, &CosmeticProjectile)>,
) {
    for LocalCue(m) in msgs.read() {
        // An END cue is the sim saying "the bolt stopped HERE" — terminate every cosmetic
        // projectile bound to it. This closes the visual/sim loop even without lane rendering: the
        // cosmetic can't outfly or undershoot the authoritative hitbox.
        if m.kind == crate::net::cue::CueKind::OnEnd {
            for (e, proj) in &flying {
                if proj.end_cue.as_deref() == Some(m.cue_id.as_str()) {
                    commands.entity(e).try_despawn();
                }
            }
        }

        // Observability: one trace line per dispatched cue, even though no cosmetics spawn yet.
        // `cue_kind` not `kind` — avoid clobbering the trace harness's own top-level event kind.
        trace::event(
            "cue_dispatch",
            json!({ "cue_id": m.cue_id, "skill_id": m.skill_id,
                "cue_kind": format!("{:?}", m.kind), "source_id": m.source_id }),
        );
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
