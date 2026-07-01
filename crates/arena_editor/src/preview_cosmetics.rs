//! Preview cosmetics: the `On<CueEvent>` observer that turns the real sim's fired cues into the
//! authored cosmetic reactions. For each `LaneEvent` the `EditedSkillFx` binds to the cue's id it
//!   - drives the bound anim clip's weight on the caster rig's `AnimationPlayer`,
//!   - spawns the `bevy_vfx` particle effect (a clone of the named `VfxLibrary` preset, or a
//!     tagged placeholder) at the resolved rig socket + offset, CPU-baking its `VfxParamBinding`s
//!     from the live `PreviewCharge` (stat sources fall back to `0.0` here — the math is proven in
//!     `arena_skills`), and
//!   - spawns the cosmetic projectile the same way.
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
use obelisk_bevy::events::CueEvent;

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
    let caster = caster_q.single().unwrap_or(ev.source);
    for lane in lanes {
        if let Some(anim) = &lane.anim {
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
            let socket = resolve_socket(&sockets, p.socket.as_deref(), caster);
            spawn_effect(
                &mut commands,
                &library,
                p.effect.as_deref(),
                socket,
                p.offset,
                &p.param_bindings,
                charge.0,
            );
        }
        if let Some(pr) = &lane.projectile {
            let socket = resolve_socket(&sockets, pr.socket.as_deref(), caster);
            spawn_effect(
                &mut commands,
                &library,
                pr.effect.as_deref(),
                socket,
                Vec3::ZERO,
                &[],
                charge.0,
            );
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

/// Spawn one cosmetic under `socket`: clone the named `VfxLibrary` effect (CPU-baking its bindings
/// from the live `charge`) when present, else a tagged placeholder. Always `PreviewCosmetic` +
/// `GameEntity` tagged and parented to the socket.
#[allow(clippy::too_many_arguments)]
fn spawn_effect(
    commands: &mut Commands,
    library: &VfxLibrary,
    effect: Option<&str>,
    socket: Entity,
    offset: Vec3,
    bindings: &[VfxParamBinding],
    charge: f32,
) {
    let child = if let Some(mut system) = effect.and_then(|n| library.effects.get(n).cloned()) {
        bake_bindings(&mut system, bindings, |b| match &b.source {
            VfxBindSource::Charge => charge,
            VfxBindSource::Stat { .. } => 0.0,
        });
        commands
            .spawn((
                system,
                Transform::from_translation(offset),
                PreviewCosmetic,
                GameEntity,
            ))
            .id()
    } else {
        commands
            .spawn((
                Transform::from_translation(offset),
                Visibility::default(),
                PreviewCosmetic,
                GameEntity,
            ))
            .id()
    };
    commands.entity(child).insert(ChildOf(socket));
}
