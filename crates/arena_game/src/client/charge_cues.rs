//! CHARGE-HOLD presentation driver — the client half of `CastTimeline::charge_cues` (the
//! designer's charge tiers). While a player holds the cast button, the tier whose threshold is
//! the highest at or below the LIVE charge fraction is active: its effect loops on the caster
//! (bone-anchored via `CueAttach::Bone`), its `anim` overlays the rig's casting layer
//! ([`CueAnimOverlay`]), and its `Charge`-sourced params stream the live fraction into the
//! running effect every frame (new particles pick the value up — the glow grows as you charge).
//! Crossing a tier drains the old effect (emission stop, particles age out) and starts the new
//! one; release/cancel drains everything and hands off to the ordinary `on_cast` cue.
//!
//! Runs for BOTH players with ZERO wire traffic: the LOCAL player from its own
//! [`ChargeState`], the REMOTE from its predicted `ActionState.charging` (the input-stream
//! telegraph — you watch the enemy's hand ignite while they hold). Windowed-only (pure
//! presentation; the headless client has no VfxLibrary).

use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use obelisk_bevy::assets::{CastTimeline, CastTimelineHandles, ChargeCue, CueAttach, CueParam, ParamSource};
use bevy_vfx::{VfxLibrary, VfxSystem};
use std::collections::HashMap;

use crate::net::input::ArenaInput;
use crate::net::protocol::{EquippedWeapon, NetworkedPlayer};
use crate::net::MAX_CHARGE_SECS;

use super::cosmetics::{apply_modulated_param, ParticleLifetime};
use super::net::{ChargeState, LocalNetPlayer, MaterializedBody, SelectedSkill};
use super::rig::CueAnimOverlay;
use super::sockets::{resolve_socket, RigSockets};

/// Height of the default (non-Bone) charge anchor above the player origin — chest/muzzle-ish,
/// same read as the cast muzzle offset.
const WORLD_TIER_ANCHOR: Vec3 = Vec3::new(0.0, 1.0, 0.0);

/// Live charge params re-apply only when the quantized fraction moves a step — the vfx runtime
/// reacts to `Changed<VfxSystem>` (mesh-emitter handle invalidation), so per-frame writes are
/// avoided.
const CHARGE_PARAM_STEPS: f32 = 32.0;

/// One player's active charge-tier visual.
#[derive(Component)]
pub struct ActiveChargeVisual {
    skill: String,
    tier: usize,
    effect: Option<Entity>,
    has_anim: bool,
    /// Last quantized charge fraction applied to the live effect's params.
    applied_step: i32,
}

/// The driver. Update-schedule presentation: reads the same predicted state every frame the rest
/// of the presentation layer does; no rollback interaction (nothing here feeds the sim).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn drive_charge_cues(
    time: Res<Time>,
    charge: Res<ChargeState>,
    selected: Res<SelectedSkill>,
    handles: Res<CastTimelineHandles>,
    timelines: Res<Assets<CastTimeline>>,
    vfx: Option<Res<VfxLibrary>>,
    mut players: Query<
        (
            Entity,
            Has<LocalNetPlayer>,
            &ActionState<ArenaInput>,
            Option<&EquippedWeapon>,
            Option<&mut ActiveChargeVisual>,
        ),
        (With<NetworkedPlayer>, With<MaterializedBody>),
    >,
    children: Query<&Children>,
    bodies: Query<(Entity, Option<&RigSockets>), With<super::rig::ArenaBody>>,
    mut live_systems: Query<&mut VfxSystem>,
    mut lifetimes: Query<&mut ParticleLifetime>,
    mut remote_hold: Local<HashMap<Entity, f32>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (player, is_local, action, weapon, active) in &mut players {
        // --- Who's charging what, and how far along. ---
        let (charging, frac, skill) = if is_local {
            (charge.charging, charge.frac(), selected.0.clone())
        } else {
            let charging = action.0.charging;
            let held = remote_hold.entry(player).or_insert(0.0);
            if charging {
                *held += dt;
            } else {
                *held = 0.0;
            }
            let frac = (*held / MAX_CHARGE_SECS).clamp(0.0, 1.0);
            let skill = weapon
                .and_then(|w| w.skills.get(action.0.skill_slot as usize))
                .cloned()
                .unwrap_or_default();
            (charging, frac, skill)
        };

        let tiers: Option<&[ChargeCue]> = handles
            .0
            .get(&skill)
            .and_then(|h| timelines.get(h))
            .map(|tl| tl.charge_cues.as_slice())
            .filter(|t| !t.is_empty());

        let target_tier = if charging {
            tiers.and_then(|t| t.iter().rposition(|tier| tier.threshold <= frac))
        } else {
            None
        };

        match (active, target_tier) {
            // Nothing active, nothing wanted.
            (None, None) => {}
            // Start (or nothing-yet-crossed): spawn the tier.
            (None, Some(t)) => {
                let tier = &tiers.unwrap()[t];
                let state = start_tier(
                    player, is_local, &skill, t, tier, frac, &vfx, &children, &bodies,
                    &mut commands,
                );
                commands.entity(player).insert(state);
            }
            // Release / cancel: drain + clear.
            (Some(state), None) => {
                end_tier(player, &state, &mut lifetimes, &mut commands);
                commands.entity(player).remove::<ActiveChargeVisual>();
            }
            (Some(mut state), Some(t)) => {
                if state.skill != skill || state.tier != t {
                    // Tier crossed (or the selection changed mid-hold): drain the old, start
                    // the new.
                    end_tier(player, &state, &mut lifetimes, &mut commands);
                    let tier = &tiers.unwrap()[t];
                    let new = start_tier(
                        player, is_local, &skill, t, tier, frac, &vfx, &children, &bodies,
                        &mut commands,
                    );
                    commands.entity(player).insert(new);
                } else {
                    // Same tier: stream the live charge into the running effect (quantized).
                    let step = (frac * CHARGE_PARAM_STEPS) as i32;
                    if step != state.applied_step {
                        state.applied_step = step;
                        if let Some(effect) = state.effect {
                            if let Ok(mut system) = live_systems.get_mut(effect) {
                                let tier = &tiers.unwrap()[t];
                                apply_charge_params(&mut system, &tier.cue.params, frac);
                            }
                        }
                    }
                }
            }
        }
    }
    remote_hold.retain(|player, _| players.contains(*player));
}

/// Spawn one tier's visuals: the effect (bone- or body-anchored, params seeded at the current
/// fraction, infinite play window — ended explicitly by [`end_tier`]) + the anim overlay.
#[allow(clippy::too_many_arguments)]
fn start_tier(
    player: Entity,
    is_local: bool,
    skill: &str,
    tier_index: usize,
    tier: &ChargeCue,
    frac: f32,
    vfx: &Option<Res<VfxLibrary>>,
    children: &Query<&Children>,
    bodies: &Query<(Entity, Option<&RigSockets>), With<super::rig::ArenaBody>>,
    commands: &mut Commands,
) -> ActiveChargeVisual {
    let mut effect_entity = None;
    if let Some(effect_name) = tier.cue.effect.as_deref() {
        if let Some(mut system) = vfx
            .as_deref()
            .and_then(|lib| lib.effects.get(effect_name))
            .cloned()
        {
            apply_charge_params(&mut system, &tier.cue.params, frac);
            let (parent, offset) = match &tier.cue.attach {
                CueAttach::Bone { socket, offset } => {
                    (resolve_socket(player, socket, children, bodies), *offset)
                }
                // World (and the normatively-illegal Follow) anchor to the body root — a charge
                // glow follows its caster.
                _ => (player, WORLD_TIER_ANCHOR),
            };
            let mut ec = commands.spawn((
                Name::new(format!("charge-tier-{tier_index}[{skill}]")),
                system,
                // Infinite play window: `end_tier` closes it (drain) on tier exit/release.
                ParticleLifetime {
                    elapsed: 0.0,
                    duration: f32::INFINITY,
                    drain: None,
                },
                Transform::from_translation(offset),
                Visibility::default(),
                ChildOf(parent),
            ));
            // The LOCAL caster's charge glow sits centimetres from the first-person camera —
            // rendered there it's screen-filling fog. Same treatment as the body itself:
            // SELF_BODY_LAYER (visible through portals + to the customizer preview, never to
            // the layer-0 main camera). Your own feedback is the HUD charge bar; the OPPONENT
            // sees the full hand-glow telegraph on your rig.
            if is_local {
                ec.insert(bevy::camera::visibility::RenderLayers::layer(
                    super::present::SELF_BODY_LAYER,
                ));
            }
            effect_entity = Some(ec.id());
        }
    }
    let has_anim = if let Some(clip) = tier.cue.anim.clone() {
        commands.entity(player).insert(CueAnimOverlay {
            clip,
            until: None,
            looping: true,
        });
        true
    } else {
        false
    };
    ActiveChargeVisual {
        skill: skill.to_string(),
        tier: tier_index,
        effect: effect_entity,
        has_anim,
        applied_step: (frac * CHARGE_PARAM_STEPS) as i32,
    }
}

/// End a tier's visuals: close the effect's play window NOW (emission stops next `age_lifetimes`
/// tick; live particles drain on their authored curves) + drop the anim overlay.
fn end_tier(
    player: Entity,
    state: &ActiveChargeVisual,
    lifetimes: &mut Query<&mut ParticleLifetime>,
    commands: &mut Commands,
) {
    if let Some(effect) = state.effect {
        if let Ok(mut life) = lifetimes.get_mut(effect) {
            life.duration = 0.0;
        }
    }
    if state.has_anim {
        if let Ok(mut ec) = commands.get_entity(player) {
            ec.remove::<CueAnimOverlay>();
        }
    }
}

/// Apply every `Charge`-sourced param row at the given live fraction. `scale` is remapped to a
/// readable size band (`vfx_set_size` takes ABSOLUTE particle size in world units — the raw
/// fraction would start invisible); other params get the raw fraction.
fn apply_charge_params(system: &mut VfxSystem, params: &[CueParam], frac: f32) {
    for p in params {
        if matches!(p.source, ParamSource::Charge) {
            let value = if p.param == "scale" {
                0.12 + 0.28 * frac
            } else {
                frac.max(0.05)
            };
            apply_modulated_param(system, &p.param, value);
        }
    }
}
