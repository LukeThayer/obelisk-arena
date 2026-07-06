//! Cue cosmetics — the client-side consumer of dispatched [`LocalCue`]s.
//!
//! Cue **rendering** (C3): each fired cue resolves its `obelisk_bevy::assets::CueBinding` from the
//! caster's skill's loaded `CastTimeline` — keyed by the wire's OWN `skill_id`
//! ([`cue_binding_for`]), NOT a hardcoded skill, since `cue_id` alone isn't unique across skills
//! (every skill's cast cue is named `"on_cast"`). A resolved binding's `effect` name is looked up
//! in `bevy_effect`'s [`EffectLibrary`] FIRST, falling back to `bevy_vfx`'s [`VfxLibrary`]
//! ([`spawn_cue_effect`]) — mirrors `bevy_modal_editor`'s skill-preview cue renderer
//! (`src/skill/preview/cosmetics.rs`) exactly; the ONE structural difference is that the editor
//! renders a single previewing skill while the arena resolves the timeline the wire itself names,
//! since a real duel is multi-skill. A name in neither library warns ONCE (never panics — a
//! `CueBinding` naming an effect that doesn't exist anywhere is inert by construction, same as an
//! unbound cue). The windowed client wires both libraries; the headless client (the net-test path)
//! wires NEITHER, so every cue there resolves its binding exactly the same way but never finds a
//! library to spawn from — a clean, silent no-op, not a special case.
//!
//! The authoritative hit stays in obelisk's `Hitbox`/`Projectile`; these cosmetics only ever react
//! to the cues it fires, never resolve them.

use crate::trace;
use bevy::color::LinearRgba;
use bevy::prelude::*;
use bevy_effect::{cleanup_effect, EffectLibrary, EffectPlayback, PlaybackState};
use bevy_vfx::{ColorSource, InitModule, ScalarRange, SpawnModule, VfxLibrary, VfxSystem};
use obelisk_bevy::assets::{CastTimeline, CueAttach, CueBinding, CueParam, ParamSource, VolumeMotion};
use obelisk_bevy::prelude::{charge_mult, CastTimelineHandles};
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// `arena_game`-local bevy `Message` wrapping the engine-neutral serde
/// [`crate::net::cue::CueMessage`].
///
/// `crate::net::cue::CueMessage` is a plain serde wire type (no bevy `Message` derive), so it stays
/// lightyear-free. `LocalCue` is the in-process Bevy channel that carries cues into the cosmetics
/// consumer ([`spawn_cue_cosmetics`]). It is fed from two sides: `skills::consume_replicated_cues`
/// forwards each survivor of the replicated `CueWireMessage` drain, and `skills::predicted_local_cast`
/// emits the local player's own on-cast cue immediately.
#[derive(Message, Clone, Debug)]
pub struct LocalCue(pub crate::net::cue::CueMessage);

/// Chest/wand-height lift applied to the **OnCast** lane only. The OnCast cue's `position` is the
/// caster's origin (= the body CENTER, world y≈0.59), so the muzzle particle + projectile would spawn
/// inside the torso. Raising them ~1.2m reads the muzzle at hand height and flies the projectile from
/// chest height. The OnHit/impact lane keeps the target's actual hit position (no lift).
///
/// This fixed offset is a stand-in; a real `wand_tip` socket on the rig is later polish.
const MUZZLE_HEIGHT_OFFSET: Vec3 = Vec3::new(0.0, 1.2, 0.0);

/// A cosmetic's presentation lifetime — no `CueBinding` authors one (the v2 schema has no
/// per-binding lifetime field, unlike the deleted v1 lane format), so every spawned cue cosmetic
/// gets this single default (mirrors the editor's `DEFAULT_COSMETIC_LIFETIME`).
const DEFAULT_CUE_EFFECT_LIFETIME: f32 = 1.5;

/// Per-caster aim direction (normalized), recorded when a cast is issued, keyed by the caster's
/// stable `ObeliskId` string.
///
/// This was the FALLBACK aim source for the pre-migration renderer; a `CueMessage` now always
/// carries its OWN `aim_dir` (populated for both the replicated cue — from the caster's live
/// `ActiveCast` — and the locally-predicted cue — from the cast's resolved aim), which
/// [`spawn_cue_cosmetics`] reads directly as the single source of truth. `predicted_local_cast`
/// (`skills.rs`) still populates this lookup; nothing currently reads it back.
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
///
/// v2's `CueBinding` resolves every cue through `EffectLibrary`/`VfxLibrary` (see
/// [`spawn_cue_effect`]) with no billboard fallback tier, so this emissive-billboard cache is
/// currently unread — kept (not recreated) as the pre-`bevy_vfx` stand-in infra, in case a future
/// consumer wants a last-resort tier when a name resolves in neither library.
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

/// Startup (windowed client only): seed the `bevy_vfx` [`VfxLibrary`] with the built-in presets
/// (`fire`/`explosion`/`sparks`/… — the same set the skill designer offers) + any authored overrides
/// in `assets/vfx/` or `assets/skills/*.vfx.ron`, so [`spawn_cue_effect`] can spawn a named effect by
/// key. Mirrors the editor's `init_vfx_library` so authored → in-game stays 1:1.
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
        match std::fs::read_to_string(&path).map(|s| ron::from_str::<VfxSystem>(&s)) {
            Ok(Ok(system)) => {
                library.effects.insert(name.to_string(), system);
            }
            other => warn!("skipping vfx preset {path:?}: {other:?}"),
        }
    }
}

/// Startup (windowed client only): seed the `bevy_effect` [`EffectLibrary`] with any authored
/// presets in `assets/effects/` (`.fx.ron`) — mirrors [`init_vfx_library`] so an authored effect
/// resolves in-game exactly as authored. `bevy_effect::EffectPlugin` already inserts an empty
/// `EffectLibrary` (via `init_resource`); this only adds to it. `assets/effects/` is distinct from
/// `assets/vfx/`/`assets/skills/*.vfx.ron` (those are `bevy_vfx` presets, loaded above).
pub fn init_effect_library(mut library: ResMut<EffectLibrary>) {
    bevy_effect::load_effects_from_dir(&mut library, &crate::arena_root().join("assets/effects"));
}

/// A short-lived cosmetic entity (particle billboard, `bevy_vfx`/`bevy_effect` cue effect, or
/// cosmetic projectile). Despawned by `age_lifetimes` once `elapsed >= duration`.
#[derive(Component)]
pub struct ParticleLifetime {
    pub elapsed: f32,
    pub duration: f32,
}

/// A cosmetic, non-authoritative projectile flown by the game. `velocity` is `aim_dir * speed`
/// (the `.cast.ron` window motion speed, scaled by the cast's `charge_mult` — see
/// `spawn_cue_cosmetics`) so the visual mesh tracks the authoritative obelisk hitbox.
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

/// The `CueBinding` a fired cue resolves to: `timelines[handles[skill_id]].cues[cue_id]`. The
/// wire's `skill_id` is what makes this work across multiple skills — `cue_id` alone isn't unique
/// (every skill's cast cue is named `"on_cast"`). `None` = the timeline isn't loaded yet, or no
/// binding is authored for this slot — either way the cue renders nothing, never a panic.
fn cue_binding_for<'a>(
    skill_id: &str,
    cue_id: &str,
    handles: &CastTimelineHandles,
    timelines: &'a Assets<CastTimeline>,
) -> Option<&'a CueBinding> {
    timeline_for(skill_id, handles, timelines)?.cues.get(cue_id)
}

/// Shared timeline resolution ([`cue_binding_for`] and [`window_motion_for`] both need it): the
/// loaded `CastTimeline` for `skill_id`, or `None` if it isn't registered/loaded yet.
fn timeline_for<'a>(
    skill_id: &str,
    handles: &CastTimelineHandles,
    timelines: &'a Assets<CastTimeline>,
) -> Option<&'a CastTimeline> {
    timelines.get(handles.0.get(skill_id)?)
}

/// Strip a fired cue id's `on_window_`/`emit_` prefix down to its window id. `None` for every other
/// cue-id shape (`on_cast`/`on_hit`/`on_end_*`) — `CueAttach::Follow` is only ever legal on a
/// window-scoped cue (mirrors the editor's `window_motion_for_cue` prefix check).
fn window_id_for_cue(cue_id: &str) -> Option<&str> {
    cue_id
        .strip_prefix("on_window_")
        .or_else(|| cue_id.strip_prefix("emit_"))
}

/// Look up the `VolumeMotion` of `window_id` in `skill_id`'s loaded timeline (mirrors the editor's
/// `window_motion_for_cue`, adapted to resolve the timeline via the wire's `skill_id` instead of a
/// single previewing skill).
fn window_motion_for(
    skill_id: &str,
    window_id: &str,
    handles: &CastTimelineHandles,
    timelines: &Assets<CastTimeline>,
) -> Option<VolumeMotion> {
    timeline_for(skill_id, handles, timelines)?
        .collision_windows
        .iter()
        .find(|w| w.id == window_id)
        .map(|w| w.motion.clone())
}

fn vfx_set_size(em: &mut bevy_vfx::EmitterDef, v: f32) {
    for m in em.init.iter_mut() {
        if let InitModule::SetSize(r) = m {
            *r = ScalarRange::Constant(v);
            return;
        }
    }
    em.init.push(InitModule::SetSize(ScalarRange::Constant(v)));
}

fn vfx_scale_color(em: &mut bevy_vfx::EmitterDef, mult: f32) {
    for m in em.init.iter_mut() {
        if let InitModule::SetColor(ColorSource::Constant(c)) = m {
            *c = LinearRgba::rgb(c.red * mult, c.green * mult, c.blue * mult);
            return;
        }
    }
    em.init.push(InitModule::SetColor(ColorSource::Constant(LinearRgba::rgb(
        mult, mult, mult,
    ))));
}

/// Bake a cue's `ParamSource::Charge` fraction into the first emitter of a cloned `VfxSystem`
/// preset (ported from the editor's `skill::preview::vfx_bake::apply_modulated_param` — v2's
/// `CueParam` has no min/max to lerp, so the charge fraction bakes in directly). Maps a named
/// authoring param onto the `bevy_vfx` module stack: `"scale"` → `SetSize`, `"emission"` →
/// `SpawnModule::Rate`, `"color"` → scaled `SetColor` RGB. An unrecognized param name, or an
/// emitter-less system, is a no-op — never a panic.
fn apply_modulated_param(system: &mut VfxSystem, param: &str, value: f32) {
    let Some(em) = system.emitters.first_mut() else {
        return;
    };
    match param {
        "scale" => vfx_set_size(em, value),
        "emission" => em.spawn = SpawnModule::Rate(value),
        "color" => vfx_scale_color(em, value),
        _ => {}
    }
}

/// Spawn one cue cosmetic at `translation` (always a world-space root, mirroring the editor's
/// `CueAttach::World` handling — see its own doc comment: `CueBinding`'s v2 schema carries no
/// per-binding socket to parent a `Follow` attachment to either, so the caller re-homes a `Follow`
/// cosmetic by attaching a [`CosmeticProjectile`] to the SAME entity this returns).
///
/// **Cue effect-name resolution order** (canonical — mirrors the editor's `spawn_cue_effect`
/// exactly): tries [`EffectLibrary`] FIRST, then [`VfxLibrary`]. A name in neither warns once
/// (never panics — mirrors `CueBinding`'s own "inert by construction" doc); a headless app that
/// wires NEITHER library (the net-test path) stays silent rather than warning about content it
/// structurally has no way to render. Returns the spawned entity so the caller can attach a
/// `CosmeticProjectile` for `CueAttach::Follow`.
#[allow(clippy::too_many_arguments)]
fn spawn_cue_effect(
    commands: &mut Commands,
    effects: Option<&EffectLibrary>,
    vfx: Option<&VfxLibrary>,
    name: &str,
    params: &[CueParam],
    charge: f32,
    translation: Vec3,
    warned: &mut HashSet<String>,
) -> Entity {
    let mut base = commands.spawn((
        Transform::from_translation(translation),
        Visibility::default(),
        ParticleLifetime {
            elapsed: 0.0,
            duration: DEFAULT_CUE_EFFECT_LIFETIME,
        },
    ));

    if let Some(marker) = effects.and_then(|lib| lib.effects.get(name)).cloned() {
        base.insert((
            marker,
            EffectPlayback {
                state: PlaybackState::Playing,
                ..default()
            },
        ));
    } else if let Some(mut system) = vfx.and_then(|lib| lib.effects.get(name)).cloned() {
        for p in params {
            match p.source {
                ParamSource::Charge => apply_modulated_param(&mut system, &p.param, charge),
            }
        }
        base.insert(system);
    } else if (effects.is_some() || vfx.is_some()) && warned.insert(name.to_string()) {
        warn!(
            "cue effect '{name}' not found in EffectLibrary or VfxLibrary — this cue renders \
             nothing (checked EffectLibrary first, then VfxLibrary — the resolution order every \
             CueBinding consumer must mirror)"
        );
    }

    base.id()
}

/// Read every [`LocalCue`] dispatched this frame, resolve its `CueBinding` (keyed by the cue's OWN
/// `skill_id` — [`cue_binding_for`]), and render it: spawn the bound effect
/// ([`spawn_cue_effect`], `EffectLibrary` first then `VfxLibrary`), and — for a `CueAttach::Follow`
/// binding on a window-scoped cue — attach a [`CosmeticProjectile`] flown along that window's
/// authored `VolumeMotion`, using the wire's own `aim_dir` (the single source of truth for both the
/// replicated and the locally-predicted cue) scaled by the sim's own `charge_mult` (the
/// authoritative hitbox scales its launch speed identically — see obelisk's
/// `timeline::advance::move_projectiles` — so the cosmetic must too, to track it).
///
/// `effects`/`vfx` are `Option` so the headless client (which adds neither `EffectPlugin` nor
/// `VfxPlugin`) resolves every binding exactly the same way but simply never finds a library to
/// spawn from — a clean no-op, not a special case.
#[allow(clippy::too_many_arguments)]
pub fn spawn_cue_cosmetics(
    mut msgs: MessageReader<LocalCue>,
    handles: Res<CastTimelineHandles>,
    timelines: Res<Assets<CastTimeline>>,
    effects: Option<Res<EffectLibrary>>,
    vfx: Option<Res<VfxLibrary>>,
    mut commands: Commands,
    mut flying: Query<(Entity, &CosmeticProjectile, Option<&mut EffectPlayback>)>,
    mut warned: Local<HashSet<String>>,
) {
    for LocalCue(m) in msgs.read() {
        // An END cue is the sim saying "the bolt stopped HERE" — terminate every cosmetic
        // projectile bound to it via `end_cue` (unconditional: even an unbound end cue must still
        // stop the flight it's ending). An `EffectLibrary`-resolved cosmetic tracks its OWN spawned
        // children in `EffectPlayback::spawned` rather than as true Bevy hierarchy children, so
        // `cleanup_effect` must despawn them before the root itself despawns, or they'd leak.
        if m.kind == crate::net::cue::CueKind::OnEnd {
            for (e, proj, playback) in &mut flying {
                if proj.end_cue.as_deref() == Some(m.cue_id.as_str()) {
                    if let Some(mut playback) = playback {
                        cleanup_effect(&mut commands, &mut playback);
                    }
                    commands.entity(e).try_despawn();
                }
            }
        }

        // Observability: one trace line per dispatched cue, whether or not it resolves to a
        // binding. `cue_kind` not `kind` — avoid clobbering the trace harness's own top-level
        // event kind (the harness merges `extra` over the base object).
        trace::event(
            "cue_dispatch",
            json!({ "cue_id": m.cue_id, "skill_id": m.skill_id,
                "cue_kind": format!("{:?}", m.kind), "source_id": m.source_id }),
        );

        let Some(binding) = cue_binding_for(&m.skill_id, &m.cue_id, &handles, &timelines) else {
            trace::event(
                "cue_unbound",
                json!({ "cue_id": m.cue_id, "skill_id": m.skill_id }),
            );
            continue;
        };
        let Some(effect_name) = &binding.effect else {
            continue;
        };

        // Spawn position: raise the OnCast lane (muzzle + projectile) to wand/chest height so it
        // clears the robe; every other slot renders at the cue's own reported position.
        let spawn_pos = if m.kind == crate::net::cue::CueKind::OnCast {
            m.position + MUZZLE_HEIGHT_OFFSET
        } else {
            m.position
        };
        // Charge fraction (0..1) driving any `ParamSource::Charge` param on this binding. `None`
        // (e.g. the predicted local on_cast cue — `PredictedCast` carries no charge byte yet)
        // bakes to 0.0 — an unspecified arena charge reads as "uncharged", not "full strength".
        let charge = m.charge.unwrap_or(0) as f32 / 255.0;

        let spawned = spawn_cue_effect(
            &mut commands,
            effects.as_deref(),
            vfx.as_deref(),
            effect_name,
            &binding.params,
            charge,
            spawn_pos,
            &mut warned,
        );

        // `CueAttach::Follow` is only ever authored on `on_window_*`/`emit_*` slots (world-anchored
        // slots — on_cast/on_hit/on_end_* — have no motion to follow).
        if matches!(binding.attach, CueAttach::Follow) {
            if let Some(window_id) = window_id_for_cue(&m.cue_id) {
                let flight = match window_motion_for(&m.skill_id, window_id, &handles, &timelines) {
                    Some(VolumeMotion::Linear { speed }) => {
                        Some((m.aim_dir * speed * charge_mult(m.charge), 0.0))
                    }
                    Some(VolumeMotion::Ballistic { speed, gravity }) => {
                        Some((m.aim_dir * speed * charge_mult(m.charge), gravity))
                    }
                    // Static/Beam (or no window found): nothing flies.
                    _ => None,
                };
                if let Some((velocity, gravity)) = flight {
                    commands.entity(spawned).insert(CosmeticProjectile {
                        velocity,
                        gravity,
                        end_cue: Some(format!("on_end_{window_id}")),
                    });
                }
            }
        }

        // Two-anchor beam arc: a beam window's open cue carries BOTH anchors (`position_from` ->
        // `position`); sample a short burst arc between them off the single resolved effect name
        // (v2 has no dedicated beam lane — see `CastTimeline::cues` doc).
        if let Some(from) = m.position_from {
            const BEAM_SEGMENTS: usize = 6;
            for i in 0..BEAM_SEGMENTS {
                let t = i as f32 / (BEAM_SEGMENTS - 1) as f32;
                spawn_cue_effect(
                    &mut commands,
                    effects.as_deref(),
                    vfx.as_deref(),
                    effect_name,
                    &binding.params,
                    charge,
                    from.lerp(m.position, t),
                    &mut warned,
                );
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

/// Age every `ParticleLifetime` and despawn it once it has outlived its `duration`. An
/// `EffectLibrary`-resolved cosmetic also carries `EffectPlayback`, whose spawned children live in
/// `EffectPlayback::spawned` rather than as true Bevy hierarchy children — `cleanup_effect` stops it
/// and despawns those children before the root itself despawns, or they'd leak. (No render-frame
/// grace ladder like the editor's `reap_preview_cosmetics`: nothing here synchronously mass-despawns
/// entities mid-frame the way the editor's scrub/Reset does, so the same-frame despawn race that
/// ladder guards against doesn't arise.)
pub fn age_lifetimes(
    time: Res<Time>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut ParticleLifetime, Option<&mut EffectPlayback>)>,
) {
    let dt = time.delta_secs();
    for (e, mut life, playback) in &mut q {
        life.elapsed += dt;
        if life.elapsed >= life.duration {
            if let Some(mut playback) = playback {
                cleanup_effect(&mut commands, &mut playback);
            }
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod cue_binding_resolution_tests {
    use super::*;
    use obelisk_bevy::assets::{
        CollisionShape, CollisionWindow, HitFilter, HitMode, PhaseDurations, WindowPhase,
        WindowSpawn,
    };

    fn minimal_timeline(cues: HashMap<String, CueBinding>) -> CastTimeline {
        CastTimeline {
            skill_id: "firebolt".into(),
            phase_durations: PhaseDurations {
                windup: 0.1,
                active: 0.1,
                recovery: 0.1,
            },
            collision_windows: Vec::new(),
            acquisition: Default::default(),
            vfx_cues: Default::default(),
            chain_radius: 6.0,
            chargeable: false,
            max_hold: 1.0,
            cues,
        }
    }

    fn handles_with(skill_id: &str, tl: CastTimeline) -> (CastTimelineHandles, Assets<CastTimeline>) {
        let mut timelines: Assets<CastTimeline> = Assets::default();
        let handle = timelines.add(tl);
        let mut handles = CastTimelineHandles::default();
        handles.0.insert(skill_id.to_string(), handle);
        (handles, timelines)
    }

    #[test]
    fn resolves_cue_binding_by_skill_and_slot() {
        let mut cues = HashMap::new();
        cues.insert(
            "on_cast".to_string(),
            CueBinding {
                effect: Some("Fire".to_string()),
                ..Default::default()
            },
        );
        let (handles, timelines) = handles_with("firebolt", minimal_timeline(cues));

        let bound = cue_binding_for("firebolt", "on_cast", &handles, &timelines);
        assert_eq!(bound.and_then(|b| b.effect.as_deref()), Some("Fire"));

        assert!(
            cue_binding_for("firebolt", "on_hit", &handles, &timelines).is_none(),
            "on_hit has no authored binding"
        );
        assert!(
            cue_binding_for("unknown", "on_cast", &handles, &timelines).is_none(),
            "no timeline is registered under an unknown skill_id"
        );
    }

    #[test]
    fn window_id_for_cue_strips_known_prefixes_only() {
        assert_eq!(window_id_for_cue("on_window_bolt"), Some("bolt"));
        assert_eq!(window_id_for_cue("emit_bolt"), Some("bolt"));
        assert_eq!(window_id_for_cue("on_hit"), None);
        assert_eq!(window_id_for_cue("on_cast"), None);
        assert_eq!(window_id_for_cue("on_end_bolt"), None);
    }

    #[test]
    fn window_motion_for_resolves_by_skill_and_window_id() {
        let window = CollisionWindow {
            id: "bolt".into(),
            spawn: WindowSpawn::Scheduled {
                phase: WindowPhase::Active,
                offset: 0.0,
            },
            anchor: Default::default(),
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration: 1.0,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Linear { speed: 20.0 },
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::FirstOnly,
            rehit_interval: None,
            emitter: None,
        };
        let mut tl = minimal_timeline(HashMap::new());
        tl.collision_windows.push(window);
        let (handles, timelines) = handles_with("firebolt", tl);

        assert!(matches!(
            window_motion_for("firebolt", "bolt", &handles, &timelines),
            Some(VolumeMotion::Linear { speed }) if speed == 20.0
        ));
        assert!(window_motion_for("firebolt", "ghost", &handles, &timelines).is_none());
        assert!(window_motion_for("unknown", "bolt", &handles, &timelines).is_none());
    }

    #[test]
    fn apply_scale_inserts_or_replaces_set_size() {
        let mut system = VfxSystem::default();
        apply_modulated_param(&mut system, "scale", 0.7);
        let em = system.emitters.first().unwrap();
        let sizes: Vec<f32> = em
            .init
            .iter()
            .filter_map(|m| match m {
                InitModule::SetSize(ScalarRange::Constant(v)) => Some(*v),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![0.7]);
    }

    #[test]
    fn apply_emission_sets_spawn_rate() {
        let mut system = VfxSystem::default();
        apply_modulated_param(&mut system, "emission", 120.0);
        match system.emitters.first().unwrap().spawn {
            SpawnModule::Rate(r) => assert_eq!(r, 120.0),
            _ => panic!("expected SpawnModule::Rate"),
        }
    }

    #[test]
    fn apply_color_scales_existing_constant() {
        let mut system = VfxSystem::default();
        system.emitters[0]
            .init
            .push(InitModule::SetColor(ColorSource::Constant(LinearRgba::rgb(
                1.0, 1.0, 1.0,
            ))));
        apply_modulated_param(&mut system, "color", 0.5);
        let found = system.emitters[0].init.iter().find_map(|m| match m {
            InitModule::SetColor(ColorSource::Constant(c)) => Some(*c),
            _ => None,
        });
        assert_eq!(found, Some(LinearRgba::rgb(0.5, 0.5, 0.5)));
    }

    #[test]
    fn unknown_param_is_a_no_op() {
        let mut system = VfxSystem::default();
        let before = system.clone();
        apply_modulated_param(&mut system, "nonsense", 1.0);
        assert_eq!(system, before);
    }

    /// Every authored `assets/skills/*.vfx.ron` must parse as a `VfxSystem`. At runtime a parse
    /// failure only warns+skips (`load_vfx_presets_from_dir`), silently leaving the cue that
    /// references the preset invisible — exactly the failure mode that made blizzard's storm render
    /// nothing. This pins authoring typos at test time instead. (Covers firebolt_trail +
    /// blizzard_frost.)
    #[test]
    fn authored_vfx_presets_all_parse() {
        let dir = crate::arena_root().join("assets/skills");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("assets/skills readable").flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".vfx.ron") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("vfx file readable");
            let parsed = ron::from_str::<VfxSystem>(&src);
            assert!(
                parsed.is_ok(),
                "{name} failed to parse as VfxSystem: {:?}",
                parsed.err()
            );
            checked += 1;
        }
        assert!(
            checked >= 2,
            "expected at least firebolt_trail + blizzard_frost, found {checked}"
        );
    }
}
