//! Preview cosmetics: the `On<CueEvent>` observer that turns the real sim's fired cues into the
//! authored cosmetic reactions. For each `LaneEvent` the `EditedSkillFx` binds to the cue's id it
//!   - drives the bound anim clip's weight on the caster rig's `AnimationPlayer`,
//!   - spawns the `bevy_vfx` particle effect (a clone of the named `VfxLibrary` preset, or a
//!     tagged placeholder), CPU-baking its `VfxParamBinding`s from the live `PreviewCharge`
//!     (stat sources fall back to `0.0` here — the math is proven in `arena_skills`). `OnCast`/
//!     `OnWindow` cues anchor to the resolved rig socket + offset; `OnHit` cues spawn UNPARENTED
//!     at the cue's authoritative world position (the hit point — the caster socket would render
//!     the impact on the caster, matching the game's world-positioned cue cosmetics), and
//!   - spawns the cosmetic projectile the same way (socket-anchored — a launch-point cue).
//!
//! Every spawned cosmetic is `GameEntity`-tagged so the editor despawns it on Reset, and marked
//! `PreviewCosmetic` for tracing/tests. This is the presentation half of "Play the real skill":
//! obelisk fires the authoritative `CueEvent`, and the authored lanes render off it.

use crate::model::EditedSkillFx;
use crate::preview_rig::{drive_anim_clip, PreviewAnimGraph};
use crate::socket::{resolve_socket, RigSockets};
use crate::vfx_bind::bake_bindings;
use arena_sim::preview::PreviewCaster;
use arena_skills::{VfxBindSource, VfxParamBinding};
use bevy::prelude::*;
use bevy_editor_game::GameEntity;
use bevy_vfx::data::VfxLibrary;
use obelisk_bevy::events::{CueEvent, CueKind as ObeliskCueKind};

/// The caster's charge fraction (0..1) used to bake `VfxBindSource::Charge` bindings in the
/// preview. Defaults to fully charged (`1.0`) so muzzle/impact bursts render at full strength.
#[derive(Resource)]
pub struct PreviewCharge(pub f32);

impl Default for PreviewCharge {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Marks a spawned preview cosmetic (particle/projectile stand-in) — `GameEntity`-tagged so Reset
/// despawns it; queried by tests to prove a cue rendered its lanes.
#[derive(Component)]
pub struct PreviewCosmetic;

/// Bounds a spawned cosmetic's life: `bevy_vfx` presets default to looping-forever, so without
/// this each fired cue would leave a permanently-burning effect behind (the game bounds its lane
/// vfx the same way with `ParticleLifetime`). Ticked by [`age_preview_cosmetics`].
#[derive(Component)]
pub struct CosmeticLifetime {
    pub elapsed: f32,
    pub duration: f32,
    /// Post-expiry countdown (frames). The effect is STOPPED (VfxSystem removed) at expiry but
    /// the entity survives two grace frames: `bevy_vfx`'s `cpu_mesh_particle_cleanup` reacts to
    /// the `VfxSystem` removal with a queued component-remove, which would warn against an
    /// already-despawned entity.
    pub grace: u8,
}

/// Flies a preview cosmetic projectile: gravity into velocity, then velocity into position —
/// the same semi-implicit Euler obelisk's `move_projectiles` runs on the authoritative hitbox,
/// so the visible bolt traces the arc the sim actually resolves hits along.
#[derive(Component)]
pub struct PreviewFlight {
    pub velocity: Vec3,
    pub gravity: f32,
}

/// Integrate every [`PreviewFlight`] each frame (render-time cosmetic motion). A bolt that
/// reaches the floor plane has hit the ground: pin it there and expire its [`CosmeticLifetime`]
/// (mirrors the sim, which despawns grounded projectile hitboxes).
pub fn fly_preview_cosmetics(
    time: Res<Time>,
    mut q: Query<(&mut PreviewFlight, &mut Transform, &mut CosmeticLifetime)>,
) {
    let dt = time.delta_secs();
    for (mut flight, mut tf, mut life) in &mut q {
        flight.velocity.y -= flight.gravity * dt;
        let velocity = flight.velocity;
        tf.translation += velocity * dt;
        if tf.translation.y < 0.0 {
            tf.translation.y = 0.0;
            life.elapsed = life.duration;
        }
    }
}

/// Tick every [`CosmeticLifetime`]; on expiry stop the effect (remove its `VfxSystem`), then
/// despawn the entity after the grace frames (see [`CosmeticLifetime::grace`]).
pub fn age_preview_cosmetics(
    time: Res<Time>,
    mut q: Query<(Entity, &mut CosmeticLifetime)>,
    mut commands: Commands,
) {
    for (e, mut life) in &mut q {
        life.elapsed += time.delta_secs();
        if life.elapsed < life.duration {
            continue;
        }
        match life.grace {
            0 => {
                commands.entity(e).remove::<bevy_vfx::VfxSystem>();
                life.grace = 1;
            }
            1 => life.grace = 2,
            _ => commands.entity(e).try_despawn(),
        }
    }
}

/// Observer: on a fired `CueEvent`, play every `EditedSkillFx` lane bound to its `cue_id`.
#[allow(clippy::too_many_arguments)]
pub fn on_preview_cue(
    cue: On<CueEvent>,
    edited: Res<EditedSkillFx>,
    sockets: Res<RigSockets>,
    graph: Res<PreviewAnimGraph>,
    library: Res<VfxLibrary>,
    charge: Res<PreviewCharge>,
    caster_q: Query<Entity, With<PreviewCaster>>,
    mut players: Query<&mut AnimationPlayer>,
    children: Query<&Children>,
    mut commands: Commands,
) {
    let ev = cue.event();
    let Some(lanes) = edited.fx.lanes.get(&ev.cue_id).map(std::slice::from_ref) else {
        return;
    };
    // No caster is a legal state: the timeline scrubber fires synthetic cues in EDIT mode, before
    // any duel exists. Socket-anchored lanes then fall back to world-space at the cue position.
    let caster = caster_q.single().ok();
    for lane in lanes {
        if let (Some(anim), Some(caster)) = (&lane.anim, caster) {
            if let Some(clip) = &anim.clip {
                if let Some(node) = graph.nodes.get(clip) {
                    if let Some(pe) = find_anim_player(caster, &children, &players) {
                        if let Ok(mut player) = players.get_mut(pe) {
                            drive_anim_clip(&mut player, *node, anim.weight);
                        }
                    }
                }
            }
        }
        if let Some(p) = &lane.particle {
            // OnHit cues carry the authoritative hit position — spawn the impact THERE, in world
            // space (a socket anchor would render the explosion on the caster). Cast/window cues
            // stay socket-anchored (the muzzle burst tracks the rig) when a caster exists.
            let anchor = if ev.kind == ObeliskCueKind::OnHit {
                None
            } else {
                caster.map(|c| resolve_socket(&sockets, p.socket.as_deref(), c))
            };
            spawn_effect(
                &mut commands,
                &library,
                p.effect.as_deref(),
                if anchor.is_some() { p.offset } else { ev.position + p.offset },
                &p.param_bindings,
                charge.0,
                anchor,
                p.lifetime.max(0.5),
            );
        }
        if let Some(pr) = &lane.projectile {
            // The cosmetic projectile FLIES (world-space, never socket-parented) at the authored
            // speed/gravity, tracing the same arc as the authoritative hitbox: launched from the
            // cue position toward the dummy marker with the same lofted ballistic aim the
            // preview cast uses (level for gravity 0).
            let dir = arena_sim::ballistics::ballistic_launch_dir(
                ev.position,
                arena_sim::spawn::SPAWN_MARKERS[1],
                pr.speed,
                pr.gravity,
            );
            let bolt = spawn_effect(
                &mut commands,
                &library,
                pr.effect.as_deref(),
                ev.position,
                &[],
                charge.0,
                None,
                2.0,
            );
            commands.entity(bolt).insert(PreviewFlight {
                velocity: dir * pr.speed,
                gravity: pr.gravity,
            });
        }
    }
}

/// Depth-first search the caster rig for the first `AnimationPlayer` (the GLTF scene loader spawns
/// one inside the rig tree). Returns `None` if the rig has no player yet.
fn find_anim_player(
    root: Entity,
    children: &Query<&Children>,
    players: &Query<&mut AnimationPlayer>,
) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if players.contains(e) {
            return Some(e);
        }
        if let Ok(cs) = children.get(e) {
            stack.extend(cs.iter());
        }
    }
    None
}

/// Spawn one cosmetic: clone the named `VfxLibrary` effect (CPU-baking its bindings from the live
/// `charge`) when present, else a tagged placeholder. Always `PreviewCosmetic` + `GameEntity`
/// tagged, with a [`CosmeticLifetime`] so looping presets don't burn forever. `anchor:
/// Some(socket)` parents it to the socket with `translation` as the LOCAL offset; `anchor: None`
/// (impact cues, or no live caster) leaves it a world-space root with `translation` as the WORLD
/// position. Returns the spawned entity so callers can attach extras (e.g. [`PreviewFlight`]).
#[allow(clippy::too_many_arguments)]
fn spawn_effect(
    commands: &mut Commands,
    library: &VfxLibrary,
    effect: Option<&str>,
    translation: Vec3,
    bindings: &[VfxParamBinding],
    charge: f32,
    anchor: Option<Entity>,
    lifetime: f32,
) -> Entity {
    let mut base = commands.spawn((
        Transform::from_translation(translation),
        Visibility::default(),
        PreviewCosmetic,
        GameEntity,
        CosmeticLifetime {
            elapsed: 0.0,
            duration: lifetime,
            grace: 0,
        },
    ));
    if let Some(mut system) = effect.and_then(|n| library.effects.get(n).cloned()) {
        bake_bindings(&mut system, bindings, |b| match &b.source {
            VfxBindSource::Charge => charge,
            VfxBindSource::Stat { .. } => 0.0,
        });
        base.insert(system);
    }
    let child = base.id();
    if let Some(socket) = anchor {
        commands.entity(child).insert(ChildOf(socket));
    }
    child
}
