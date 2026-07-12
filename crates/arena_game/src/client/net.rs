//! Client-side net-driven player layer.
//!
//! Two jobs, shared by the windowed and headless clients:
//!   1. **Attach a body** to each lightyear-materialized `NetworkedPlayer`. BOTH players arrive as
//!      `Predicted` entities (`PredictionTarget::All` — design WS1): each gets a Dynamic avian
//!      body driven by the shared force controller. The `Controlled` one is the local player
//!      ([`InputMarker`] + [`LocalNetPlayer`]); the remote one's `ActionState<ArenaInput>` is
//!      filled by lightyear from the server-rebroadcast inputs, so the opponent is SIMULATED at
//!      the local predicted tick (no interpolation delay), corrected by rollback on mispredicts.
//!   2. **Stage local input**: copy the local player's WASD/yaw/pitch/jump/charging onto its
//!      `ActionState<ArenaInput>` in `FixedPreUpdate` (lightyear's `WriteClientInputs`), where
//!      lightyear samples and ships it. The server runs its authoritative controller against the
//!      same input (server/mod.rs).
//!
//! Input is sourced from a [`LocalInput`] resource so the windowed controller (real keyboard +
//! mouse-yaw) and the headless `ARENA_AUTOMOVE` hook can both feed the same path.

use avian3d::prelude::*;
use bevy::prelude::*;
use lightyear::prelude::client::input::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{Controlled, MessageSender, Predicted};

use crate::client::parts::PartSelection;
use crate::net::input::ArenaInput;
use crate::net::protocol::{
    CustomizeMessage, NetworkOwner, NetworkedCastState, NetworkedId, NetworkedPlayer,
    NetworkedSkillObject, PlayerCustomization, RequestChannel,
};
use crate::shared_controller::{apply_arena_movement, apply_arena_yaw};
use arena_sim::tuning::{PLAYER_CAPSULE_LENGTH, PLAYER_CAPSULE_RADIUS};

/// The local player's current input, written each frame by whichever input source is active (the
/// windowed controller's `bridge_windowed_input_to_local_input`, or the headless `ARENA_AUTOMOVE`
/// hook) and read by [`buffer_arena_input`], which copies it onto the predicted entity's
/// `ActionState<ArenaInput>` for lightyear to ship. This is a PURE-LOCAL staging resource now (no
/// wire) — native input owns the wire. Camera-relative: `movement.x` strafes +right, `movement.y`
/// is forward; `yaw` is the camera/body yaw.
#[derive(Resource, Default, Clone, Copy)]
pub struct LocalInput {
    pub movement: Vec2,
    pub yaw: f32,
    pub pitch: f32,
    pub jump: bool,
    /// Set by the input bridge on a jump PRESS; OR-ed into the next sampled tick then cleared, so
    /// a sub-tick tap (press+release between FixedPreUpdates) still lands in exactly one tick.
    pub jump_latch: bool,
}

/// The skill the local player will cast next — `buffer_arena_input` maps it to the input's
/// `skill_slot` each tick. Windowed number keys 1/2/3 select it; the headless autocast sets it
/// from `ARENA_AUTOCAST_SKILL`. Default firebolt.
#[derive(Resource)]
pub struct SelectedSkill(pub String);
impl Default for SelectedSkill {
    fn default() -> Self {
        Self("firebolt".into())
    }
}

/// Charge tuning + helpers now live in `crate::net` (the shared-tuning home — the SERVER derives
/// the charge byte from held input ticks too, design WS2); re-exported here so existing client
/// call sites + tests resolve unchanged.
pub use crate::net::{charge_byte_from_frac, charge_mult, MAX_CHARGE_SECS, TAP_CHARGE_BYTE};

/// Per-frame charge-hold state for the local player's cast. Written by `bridge_windowed_cast_hold`
/// (real keyboard/mouse) or the headless autocast latch, and read by `buffer_arena_input` + the
/// charge-bar HUD.
///
/// Lifecycle per cast (design WS2 — the cast IS the input stream's charging falling edge):
///   1. Cast button pressed → `charging = true` + `tap_latch = true`; `secs` accumulates.
///   2. Button released → `charging = false`. `buffer_arena_input` samples `charging || tap_latch`
///      per tick, so even a sub-tick tap yields ≥1 charging tick followed by a released tick — the
///      falling EDGE both peers' detectors fire on. The charge byte derives from the held tick
///      count on BOTH peers (`net::charge_byte_from_hold_ticks`); nothing rides a message.
#[derive(Resource, Default)]
pub struct ChargeState {
    /// Accumulated hold time this charge (clamped to [`MAX_CHARGE_SECS`]).
    pub secs: f32,
    /// True while the cast button is held but not yet released.
    pub charging: bool,
    /// Set on cast-button PRESS; guarantees the input stream carries ≥1 `charging=true` tick even
    /// for a sub-tick tap. Cleared by `buffer_arena_input` after it samples a released tick.
    pub tap_latch: bool,
}

impl ChargeState {
    /// Normalized hold fraction `[0, 1]` for the charge-bar HUD.
    pub fn frac(&self) -> f32 {
        (self.secs / MAX_CHARGE_SECS).clamp(0.0, 1.0)
    }
}

/// Emitted by [`local_cast_edge`] the tick the local cast edge fires, so the client can play the
/// PREDICTED own-cast cosmetics immediately (zero latency) instead of waiting for the server's
/// replicated cue (design WS3). Carries the local caster's stable `ObeliskId`, its world position,
/// the aim direction, and the locally-derived charge byte. Consumed by
/// `skills::predicted_local_cast`, which is the presentation-only predicted half — it spawns the
/// on_cast muzzle + cosmetic projectile, NEVER an obelisk `Hitbox` and NEVER touching `CombatRng`
/// (the server resolves damage authoritatively).
#[derive(Message, Clone, Debug)]
pub struct PredictedCast {
    pub skill_id: String,
    pub source_id: String,
    pub position: Vec3,
    pub aim_dir: Vec3,
    /// Charge byte derived from the local hold (same `charge_byte_from_hold_ticks` the server
    /// uses), threaded into the predicted cues' `ParamSource::Charge` bindings.
    pub charge: u8,
}

/// Set true when the local player finishes editing their costume (customizer panel close), so
/// [`send_customization`] ships the current local [`PartSelection`] to the server once. Debounced
/// this way (one send per edit session) rather than on every `<`/`>` click.
#[derive(Resource, Default)]
pub struct CustomizeDirty(pub bool);

/// Marker on the local player's predicted entity — the `Predicted` `NetworkedPlayer` lightyear
/// created for the entity this client `Controlled`. The windowed client attaches the follow camera +
/// (hidden) rig to this; the shared controller predicts its movement.
#[derive(Component)]
pub struct LocalNetPlayer;

/// Marker on a `NetworkedPlayer` (Predicted or Interpolated) that this client has already attached a
/// body/rig for, so the materialize systems are idempotent (they poll for new replicas + late joins).
#[derive(Component)]
pub struct MaterializedBody;

/// Plugin: the net-driven player layer. Registers [`LocalInput`] + the materialize/send systems.
/// Added by BOTH client modes (windowed + headless). The visual half (mesh/rig) is the caller's
/// responsibility via the `spawn_visual` hook the windowed client passes; the headless client
/// spawns no visual.
pub struct ClientNetPlayerPlugin;

impl Plugin for ClientNetPlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalInput>()
            .init_resource::<ChargeState>()
            // The skill the input's `skill_slot` points at. The windowed radial wheel sets it;
            // headless autocast sets it from ARENA_AUTOCAST_SKILL. `init_resource` is idempotent
            // with the windowed PartsPlugin init.
            .init_resource::<SelectedSkill>()
            // Keep the selection valid against the EQUIPPED weapon (protocol v4): on a weapon
            // change (or a selection the weapon doesn't grant), snap to the weapon's first skill.
            .add_systems(Update, sync_selected_to_weapon)
            .init_resource::<CustomizeDirty>()
            // The LOCAL selection the customizer edits + `send_customization` reads. `PartsPlugin`
            // also inits it (windowed); init here too so the headless client + the send path have
            // it even without `PartsPlugin`. `init_resource` is idempotent.
            .init_resource::<PartSelection>()
            .add_message::<PredictedCast>()
            // Buffer native input onto the predicted entity's ActionState (FixedPreUpdate, the
            // WriteClientInputs set lightyear samples from).
            .add_systems(
                FixedPreUpdate,
                buffer_arena_input.in_set(InputSystems::WriteClientInputs),
            )
            // The shared force controller, predicted on ALL `Predicted` entities (lightyear
            // re-runs it during rollback; the remote's ActionState comes from rebroadcast inputs).
            // Chained so the body yaws before the movement force; `local_cast_edge` runs after so
            // the predicted cast fires the same tick the (already-applied) input released.
            .add_systems(
                FixedUpdate,
                (client_apply_yaw, client_apply_movement, local_cast_edge).chain(),
            )
            // Portal traversal, predicted half (client/portals.rs): floor pass-through BEFORE
            // the physics step, the predicted teleport AFTER it — mirrors the server's own
            // ordering so both peers compute identical crossings. Rollback-guarded inside.
            .init_resource::<super::portals::PredictedPortalTravelers>()
            .add_systems(
                FixedUpdate,
                super::portals::predicted_floor_passthrough
                    .before(avian3d::prelude::PhysicsSystems::Prepare),
            )
            .add_systems(
                FixedUpdate,
                super::portals::predicted_portal_teleport
                    .after(avian3d::prelude::PhysicsSystems::StepSimulation),
            )
            .add_systems(
                Update,
                (
                    materialize_predicted_players,
                    // THE AUTHORITY FLIP: mirror the server's Dynamic glacier boulder as a LOCAL
                    // Kinematic collider on EVERY client (headless included) so predicted players
                    // collide with it in lockstep and the shove doesn't rubber-band. Runs here (not
                    // in the windowed-only visuals plugin) precisely so the headless observer gets
                    // the collider too — gameplay parity, not a visual.
                    attach_glacier_ball_collider,
                    trace_remote_input_once,
                    send_customization,
                    trace_customize_updates,
                    trace_received_remote_pose,
                    trace_remote_cast_phase,
                ),
            );
    }
}

/// Marker: the local Kinematic collider mirror is attached to this replicated glacier ball.
#[derive(Component)]
struct GlacierBallMirror;

/// Attach the LOCAL Kinematic collider mirror to each replicated glacier boulder. The server's ball
/// is a Dynamic body whose shove is authoritative; giving the replicated copy the IDENTICAL collider
/// + layers (the shared `server/glacier_ball.rs` recipe — a mismatch desyncs the shove) makes
/// predicted Dynamic players collide with it LOCALLY, so the knockback is predicted in lockstep
/// rather than arriving only as a rubber-banding server correction. Driven by the replicated
/// `Position` (AvianReplicationMode::Position). Windowed visuals are separate (client/skill_objects.rs).
fn attach_glacier_ball_collider(
    new: Query<(Entity, &NetworkedSkillObject), Without<GlacierBallMirror>>,
    mut commands: Commands,
) {
    for (e, obj) in &new {
        if obj.kind != crate::server::skill_objects::KIND_GLACIER_BALL {
            continue;
        }
        commands.entity(e).insert((
            GlacierBallMirror,
            crate::server::glacier_ball::client_ball_mirror_bundle(),
        ));
    }
}

/// Buffer the staged [`LocalInput`] (+ charge-hold) onto the predicted entity's
/// `ActionState<ArenaInput>` each `FixedPreUpdate`. lightyear ships it to the server (which applies
/// it to that client's authoritative entity) and re-applies it during rollback. Mirrors
/// `simple_box::buffer_input`. No-op until the `InputMarker<ArenaInput>` entity exists (post-spawn).
fn buffer_arena_input(
    mut input: ResMut<LocalInput>,
    mut charge: ResMut<ChargeState>,
    selected: Res<SelectedSkill>,
    mut query: Query<
        (
            &mut ActionState<ArenaInput>,
            Option<&crate::net::protocol::EquippedWeapon>,
        ),
        With<InputMarker<ArenaInput>>,
    >,
) {
    let Ok((mut action_state, weapon)) = query.single_mut() else {
        return;
    };
    let charging = charge.charging || charge.tap_latch;
    action_state.0 = ArenaInput {
        movement: input.movement,
        yaw: input.yaw,
        pitch: input.pitch,
        jump: input.jump || input.jump_latch,
        charging,
        // The slot indexes the EQUIPPED WEAPON's skill list (protocol v4). A selection the
        // weapon doesn't grant falls back to slot 0 (the weapon's first skill) — the same
        // fallback `sync_selected_to_weapon` converges the selection itself toward.
        skill_slot: weapon
            .and_then(|w| crate::net::weapon_skill_slot_for(&w.skills, &selected.0))
            .unwrap_or(0),
    };
    input.jump_latch = false;
    if !charge.charging {
        // We just sampled the release (or the latch-extended tap tick); the NEXT tick samples
        // false and produces the falling edge on both peers.
        charge.tap_latch = false;
    }
}

/// Snap [`SelectedSkill`] to the equipped weapon's FIRST skill whenever the current selection
/// isn't one the weapon grants (weapon swap, or a stale ARENA_AUTOCAST_SKILL naming a skill the
/// starter weapon lacks). `Changed<EquippedWeapon>`-shaped polling isn't enough — the selection
/// can also change out from under the weapon — so this checks membership every frame (two string
/// compares; the local player is a single entity).
fn sync_selected_to_weapon(
    mut selected: ResMut<SelectedSkill>,
    weapon: Query<
        &crate::net::protocol::EquippedWeapon,
        (With<NetworkedPlayer>, With<LocalNetPlayer>),
    >,
) {
    let Ok(weapon) = weapon.single() else {
        return;
    };
    if weapon.skills.iter().any(|s| *s == selected.0) {
        return;
    }
    if let Some(first) = weapon.skills.first() {
        selected.0 = first.clone();
    }
}

/// Predict the local player's body yaw from its input (avian `Rotation`), `With<Predicted>`.
fn client_apply_yaw(mut q: Query<(&ActionState<ArenaInput>, &mut Rotation), With<Predicted>>) {
    for (action, mut rot) in &mut q {
        apply_arena_yaw(&action.0, &mut rot);
    }
}

/// Predict the local player's movement force + jump, `With<Predicted>`. lightyear keeps
/// `ActionState<ArenaInput>` correct for the (re)simulated tick during rollback. `grounded` is
/// the same raycast check the server runs (self excluded; the predicted client body has no
/// hurtbox child, but excluding children is harmless) — rollback re-runs it against the
/// re-simulated pipeline state, so the check stays deterministic.
fn client_apply_movement(
    time: Res<Time>,
    spatial: SpatialQuery,
    children: Query<&Children>,
    mut q: Query<
        (Entity, &Position, &ComputedMass, &ActionState<ArenaInput>, Forces),
        With<Predicted>,
    >,
) {
    let dt = time.delta_secs();
    for (entity, position, mass, action, forces) in &mut q {
        let mut exclude = vec![entity];
        if let Ok(kids) = children.get(entity) {
            exclude.extend(kids.iter());
        }
        let grounded = crate::shared_controller::grounded_by_ray(&spatial, position.0, &exclude);
        apply_arena_movement(mass, dt, &action.0, forces, grounded);
    }
}

/// Attach a physics body to EVERY newly-`Predicted` `NetworkedPlayer` (the avian_3d_character
/// `handle_new_character` pattern — with `PredictionTarget::All`, BOTH players arrive `Predicted`
/// on both clients). The Dynamic body lets the client predict + roll back physics. The `Controlled`
/// one is the local player: it additionally gets `InputMarker` (native-input authority) +
/// [`LocalNetPlayer`] (camera + hidden rig hang off it); the remote one gets a bare `ActionState`
/// that lightyear fills from the server-rebroadcast inputs. Both tag [`MaterializedBody`].
#[allow(clippy::type_complexity)]
fn materialize_predicted_players(
    new_players: Query<
        (Entity, &NetworkOwner, Has<Controlled>),
        (
            With<NetworkedPlayer>,
            With<Predicted>,
            Without<MaterializedBody>,
        ),
    >,
    mut commands: Commands,
) {
    for (entity, owner, is_controlled) in &new_players {
        commands.entity(entity).insert((
            MaterializedBody,
            Visibility::default(),
            // Predicted physics body — Dynamic so the shared controller + rollback drive it. Mirrors
            // the server spawn (same shared capsule consts, rotation locked, zero friction). No
            // hurtbox: the client never resolves combat (Stage A) — that stays server-authoritative.
            RigidBody::Dynamic,
            Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH),
            // Same layers as the server body (arena_sim::make_arena_combatant) — the portal
            // floor pass-through toggles the Ground bit on BOTH peers and must agree.
            avian3d::prelude::CollisionLayers::new(
                arena_sim::GameLayer::Player,
                avian3d::prelude::LayerMask::ALL,
            ),
            LockedAxes::default()
                .lock_rotation_x()
                .lock_rotation_y()
                .lock_rotation_z(),
            Friction::new(0.0).with_combine_rule(CoefficientCombine::Min),
        ));
        if is_controlled {
            commands.entity(entity).insert((
                LocalNetPlayer,
                InputMarker::<ArenaInput>::default(),
                ActionState::<ArenaInput>::default(),
            ));
        } else {
            // Remote predicted player: no InputMarker (lightyear #1431 — the marker belongs only
            // on the authority entity), but it needs an ActionState for the rebroadcast
            // InputBuffer to drive (ActionState is not a replicated component; insert it locally).
            commands
                .entity(entity)
                .insert(ActionState::<ArenaInput>::default());
        }
        info!(
            "materialized (predicted) NetworkedPlayer owner={} controlled={is_controlled}",
            owner.0
        );
        crate::trace::event(
            "materialized_player",
            serde_json::json!({ "owner": owner.0, "local": is_controlled }),
        );
    }
}

/// One-shot spike gate (design WS1): trace the first time a REMOTE predicted player's
/// `ActionState<ArenaInput>` carries non-zero movement — proof that native-input rebroadcast
/// (server → this client) reaches the remote entity's input buffer. Cheap after latching.
fn trace_remote_input_once(
    remotes: Query<
        (&NetworkOwner, &ActionState<ArenaInput>),
        (With<Predicted>, Without<LocalNetPlayer>),
    >,
    mut seen: Local<bool>,
) {
    if *seen {
        return;
    }
    for (owner, action) in &remotes {
        if action.0.movement != Vec2::ZERO {
            *seen = true;
            crate::trace::event(
                "remote_input_seen",
                serde_json::json!({ "owner": owner.0,
                    "movement": [action.0.movement.x, action.0.movement.y] }),
            );
        }
    }
}

/// FixedUpdate: mirror the server's cast-edge detection on the LOCAL predicted entity's own
/// per-tick `ActionState<ArenaInput>` (the identical stream lightyear ships), emitting a
/// [`PredictedCast`] on the falling edge for zero-latency own-cast presentation (design WS3).
/// The server detects the same edge in the same stream and runs the authoritative cast — the two
/// stay in lockstep by construction (same data, same state machine).
#[allow(clippy::type_complexity)]
fn local_cast_edge(
    mut prev: Local<(bool, u32, u8)>, // (charging, hold_ticks, latched slot)
    rollback: Query<(), With<lightyear::prelude::Rollback>>,
    local: Query<
        (
            &Position,
            &crate::net::protocol::ObeliskNetId,
            &ActionState<ArenaInput>,
            Option<&crate::net::protocol::EquippedWeapon>,
        ),
        (With<NetworkedPlayer>, With<LocalNetPlayer>),
    >,
    mut predicted: MessageWriter<PredictedCast>,
) {
    // Rollback re-simulation REPLAYS FixedUpdate over already-simulated ticks — without this
    // guard the edge (a Local state machine, not rollback-managed) re-fires and duplicates the
    // predicted cosmetics every time a rollback spans a cast (observed under the conditioner:
    // 31 client edges for 8 real casts). The presentation edge only ever fires on FIRST
    // simulation; the server's own detector is rollback-free by construction.
    if !rollback.is_empty() {
        return;
    }
    let Ok((pos, obelisk_id, action, weapon)) = local.single() else {
        return;
    };
    let a = &action.0;
    let fired = prev.0 && !a.charging;
    if a.charging {
        if !prev.0 {
            prev.2 = a.skill_slot; // latch selection at press (matches the server's edge machine)
            prev.1 = 0;
        }
        prev.1 = prev.1.saturating_add(1);
    }
    if fired {
        let charge = crate::net::charge_byte_from_hold_ticks(prev.1);
        // Resolve the latched slot against the equipped weapon — the same list the server's
        // detector resolves, so the predicted cosmetic matches the authoritative cast.
        if let Some(skill_id) = weapon.and_then(|w| w.skills.get(prev.2 as usize)) {
            crate::trace::event(
                "cast_edge_sent",
                serde_json::json!({ "skill_id": skill_id, "charge": charge }),
            );
            predicted.write(PredictedCast {
                skill_id: skill_id.to_string(),
                source_id: obelisk_id.0.clone(),
                position: pos.0,
                aim_dir: crate::net::aim_dir(a.yaw, a.pitch),
                charge,
            });
        }
        prev.1 = 0;
    }
    prev.0 = a.charging;
}

/// Ship the local player's [`PartSelection`] to the server when [`CustomizeDirty`] is set (debounced
/// on panel close). The server applies it + broadcasts to all clients (D6). Reliable `RequestChannel`.
/// No-op until we own a local player + the sender exists. Mirrors `send_cast_requests`'s shape.
fn send_customization(
    mut dirty: ResMut<CustomizeDirty>,
    selection: Res<PartSelection>,
    local: Query<(), (With<NetworkedPlayer>, With<LocalNetPlayer>)>,
    sender: Option<Single<&mut MessageSender<CustomizeMessage>>>,
) {
    if !dirty.0 {
        return;
    }
    if local.iter().next().is_none() {
        return; // no local player yet — keep the flag set until we can send
    }
    let Some(mut sender) = sender else {
        return; // sender not ready (pre-connect)
    };
    sender.send::<RequestChannel>(CustomizeMessage { parts: *selection });
    dirty.0 = false;
    crate::trace::event("customize_sent", serde_json::json!({}));
}

/// Trace a remote player's live appearance change (a replicated `PlayerCustomization` component
/// UPDATE — no broadcast message any more, design WS5). The rig re-skin itself is `Changed`-driven
/// in `client::parts::refresh_arena_part_visibility_on_change`; this keeps the D6 harness signal.
#[allow(clippy::type_complexity)]
fn trace_customize_updates(
    changed: Query<
        &NetworkedId,
        (
            With<NetworkedPlayer>,
            Changed<PlayerCustomization>,
            Without<LocalNetPlayer>,
        ),
    >,
) {
    for net_id in &changed {
        crate::trace::event(
            "customize_received",
            serde_json::json!({ "player": net_id.0 }),
        );
    }
}

/// Edge-triggered trace of a REMOTE player's replicated cast phase, so
/// the headless harness can confirm the server's stamped `NetworkedCastState.cast_phase`
/// actually propagates server → this observer (which is what drives the remote cast animation).
/// Fires one `remote_cast_phase` line each time a remote player's `cast_phase` byte changes value
/// (keyed by `NetworkOwner`), not every frame — a cast walks 0→1→2→3→0 so it emits a handful of
/// lines per cast. The local player is excluded (its cast is driven locally, not from the wire).
#[allow(clippy::type_complexity)]
fn trace_remote_cast_phase(
    remotes: Query<
        (&NetworkOwner, &NetworkedCastState),
        (
            With<NetworkedPlayer>,
            Without<LocalNetPlayer>,
            Changed<NetworkedCastState>,
        ),
    >,
    mut last: Local<std::collections::HashMap<u64, u8>>,
) {
    for (owner, cast) in &remotes {
        let prev = last.insert(owner.0, cast.cast_phase);
        if prev != Some(cast.cast_phase) {
            crate::trace::event(
                "remote_cast_phase",
                serde_json::json!({ "owner": owner.0, "cast_phase": cast.cast_phase,
                    "cast_skill": cast.cast_skill }),
            );
        }
    }
}

/// [H] check support: throttled trace of a REMOTE predicted player's avian `Position` (the
/// local player is excluded). Lets the headless movement-replication check confirm the OTHER client
/// observes a moving player's predicted remote pose (driven by rebroadcast inputs) propagate
/// server → this client. Keyed by `NetworkOwner`, gated on `Changed<Position>`, throttled to
/// every 10th change (each line carries the shared wall-clock `ts`, so freshness is measurable).
#[allow(clippy::type_complexity)]
fn trace_received_remote_pose(
    remotes: Query<
        (&NetworkOwner, &Position),
        (
            With<NetworkedPlayer>,
            Without<LocalNetPlayer>,
            Changed<Position>,
        ),
    >,
    mut throttle: Local<u32>,
) {
    for (owner, position) in &remotes {
        *throttle += 1;
        if *throttle % 10 == 1 {
            crate::trace::event(
                "remote_pose",
                serde_json::json!({
                    "owner": owner.0,
                    "pos": [position.0.x, position.0.y, position.0.z],
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the charge → multiplier mapping at its two anchor bytes: the tap default ≈1.0× and a
    /// full hold = 2.0×. Pins the CURRENT mapping (does not redefine it).
    #[test]
    fn charge_mult_anchors() {
        assert!((charge_mult(TAP_CHARGE_BYTE) - 1.0).abs() < 0.01);
        assert!((charge_mult(255) - 2.0).abs() < 1e-6);
    }

    /// `charge_byte_from_frac` hits the documented endpoints (tap byte at 0, 255 at full hold).
    #[test]
    fn charge_byte_endpoints() {
        assert_eq!(charge_byte_from_frac(0.0), TAP_CHARGE_BYTE);
        assert_eq!(charge_byte_from_frac(1.0), 255);
    }
}
