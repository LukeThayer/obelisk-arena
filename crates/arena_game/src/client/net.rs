//! Client-side net-driven player layer.
//!
//! Two jobs, shared by the windowed and headless clients:
//!   1. **Attach a body** to each lightyear-materialized `NetworkedPlayer`. The local player
//!      arrives as a `Predicted` entity: this module gives it a Dynamic avian body +
//!      [`InputMarker`]/[`ActionState<ArenaInput>`] (so the predicted controller can run + roll
//!      back) and tags it [`LocalNetPlayer`]. Remote players arrive as `Interpolated` entities:
//!      they get no physical body (lightyear drives their avian `Position`/`Rotation`) and are
//!      tagged once their pose has replicated.
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
use lightyear::prelude::{Controlled, Interpolated, MessageReceiver, MessageSender, Predicted};

use crate::client::parts::PartSelection;
use crate::net::input::ArenaInput;
use crate::net::protocol::{
    CastChannel, CastRequestMessage, CustomizeBroadcast, CustomizeMessage, NetworkOwner,
    NetworkedCastState, NetworkedId, NetworkedPlayer, PlayerCustomization,
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
    pub jump: bool,
}

/// One-shot request to cast a skill, set by the windowed cast key or the headless `ARENA_AUTOCAST`
/// hook and consumed by [`send_cast_requests`]. `Some(skill_id)` means "send a cast_request this
/// frame"; the send system clears it back to `None`. The CLIENT never validates or resolves — it
/// only requests; the server re-validates + resolves authoritatively (Stage A, guide §5.2).
#[derive(Resource, Default)]
pub struct CastIntent(pub Option<String>);

/// Maximum hold time for the charge mechanic. A full hold of this duration maps to charge=255
/// (2.0× multiplier); an instant tap maps to charge=[`TAP_CHARGE_BYTE`] (≈1.0×).
pub const MAX_CHARGE_SECS: f32 = 1.5;

/// Charge byte for an instant tap (no hold). The value 85 is ≈ `0.333 * 255` — one-third up the
/// 0–255 charge range — which the server's [`charge_mult`] maps to ≈1.0×. A full hold sends 255
/// (→2.0×). Co-located with [`MAX_CHARGE_SECS`] so the tap/full charge feel is tuned in one place.
pub const TAP_CHARGE_BYTE: u8 = 85;

/// Map a hold fraction `[0, 1]` to a charge byte: tap (`frac = 0`) → [`TAP_CHARGE_BYTE`], full hold
/// (`frac = 1`) → 255, linear between. The inverse-direction partner of [`charge_mult`].
pub fn charge_byte_from_frac(frac: f32) -> u8 {
    let span = 255.0 - TAP_CHARGE_BYTE as f32; // 170.0
    (TAP_CHARGE_BYTE as f32 + frac * span)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// The server's charge → multiplier mapping, mirrored here for the unit test + as the single
/// documented reference: `charge_mult(c) = 0.5 + (c / 255) * 1.5`, so [`TAP_CHARGE_BYTE`] (85) ≈ 1.0×
/// and 255 = 2.0×. Obelisk's `cast_skill_dir_charged` owns the AUTHORITATIVE scaling on the damage
/// path; this is the reference mapping, not a second source of truth.
pub fn charge_mult(byte: u8) -> f32 {
    0.5 + (byte as f32 / 255.0) * 1.5
}

/// Per-frame charge-hold state for the local player's cast. Written by `bridge_windowed_cast_hold`
/// (real keyboard/mouse) and read by `send_cast_requests` + the charge-bar HUD.
///
/// Lifecycle per cast:
///   1. Cast button held → `charging = true`, `secs` accumulates.
///   2. Button released → `pending_charge` is locked in, `CastIntent` is set, state resets.
///   3. `send_cast_requests` fires → reads `pending_charge`, sends it on the wire, resets to
///      the tap default (85) so the next autocast gets normal-strength casts.
#[derive(Resource)]
pub struct ChargeState {
    /// Accumulated hold time this charge (clamped to [`MAX_CHARGE_SECS`]).
    pub secs: f32,
    /// True while the cast button is held but not yet released.
    pub charging: bool,
    /// Charge byte locked in at release; consumed (and reset to the tap default
    /// [`TAP_CHARGE_BYTE`]) by `send_cast_requests`. Initialized to it so autocast paths get ≈1.0×.
    pub(crate) pending_charge: u8,
}

impl Default for ChargeState {
    fn default() -> Self {
        Self {
            secs: 0.0,
            charging: false,
            pending_charge: TAP_CHARGE_BYTE, // frac=0 → charge_mult(TAP_CHARGE_BYTE) ≈ 1.0×
        }
    }
}

impl ChargeState {
    /// Normalized hold fraction `[0, 1]` for the charge-bar HUD.
    pub fn frac(&self) -> f32 {
        (self.secs / MAX_CHARGE_SECS).clamp(0.0, 1.0)
    }
}

/// Emitted by [`send_cast_requests`] the instant a local cast_request goes out, so the client can
/// play the PREDICTED own-cast cosmetics immediately (zero latency) instead of waiting for the
/// server's replicated cue (Task 17, guide §6.4). Carries the local caster's stable `ObeliskId`, its
/// world position, and the aim direction. Consumed by `skills::predicted_local_cast`, which is the
/// presentation-only predicted half — it spawns the on_cast muzzle + cosmetic projectile, NEVER an
/// obelisk `Hitbox` and NEVER touching `CombatRng` (the server resolves damage authoritatively).
#[derive(Message, Clone, Debug)]
pub struct PredictedCast {
    pub skill_id: String,
    pub source_id: String,
    pub position: Vec3,
    pub aim_dir: Vec3,
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
            .init_resource::<CastIntent>()
            .init_resource::<ChargeState>()
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
            // The shared force controller, predicted on the local `Predicted` entity (lightyear
            // re-runs it during rollback). Chained so the body yaws before the movement force.
            .add_systems(
                FixedUpdate,
                (client_apply_yaw, client_apply_movement).chain(),
            )
            .add_systems(
                Update,
                (
                    materialize_predicted_players,
                    materialize_interpolated_players,
                    send_cast_requests,
                    send_customization,
                    drain_customize_broadcasts,
                    trace_received_remote_pose,
                    trace_remote_cast_phase,
                ),
            );
    }
}

/// Buffer the staged [`LocalInput`] (+ charge-hold) onto the predicted entity's
/// `ActionState<ArenaInput>` each `FixedPreUpdate`. lightyear ships it to the server (which applies
/// it to that client's authoritative entity) and re-applies it during rollback. Mirrors
/// `simple_box::buffer_input`. No-op until the `InputMarker<ArenaInput>` entity exists (post-spawn).
fn buffer_arena_input(
    input: Res<LocalInput>,
    charge: Res<ChargeState>,
    mut query: Query<&mut ActionState<ArenaInput>, With<InputMarker<ArenaInput>>>,
) {
    let Ok(mut action_state) = query.single_mut() else {
        return;
    };
    action_state.0 = ArenaInput {
        movement: input.movement,
        yaw: input.yaw,
        jump: input.jump,
        charging: charge.charging,
    };
}

/// Predict the local player's body yaw from its input (avian `Rotation`), `With<Predicted>`.
fn client_apply_yaw(mut q: Query<(&ActionState<ArenaInput>, &mut Rotation), With<Predicted>>) {
    for (action, mut rot) in &mut q {
        apply_arena_yaw(&action.0, &mut rot);
    }
}

/// Predict the local player's movement force + jump, `With<Predicted>`. lightyear keeps
/// `ActionState<ArenaInput>` correct for the (re)simulated tick during rollback.
fn client_apply_movement(
    time: Res<Time>,
    mut q: Query<(&ComputedMass, &ActionState<ArenaInput>, Forces), With<Predicted>>,
) {
    let dt = time.delta_secs();
    for (mass, action, forces) in &mut q {
        apply_arena_movement(mass, dt, &action.0, forces);
    }
}

/// Attach the local player's physics body + input marker to its newly-`Predicted` `NetworkedPlayer`
/// (the avian_3d_character `handle_new_character` pattern). lightyear creates exactly one Predicted
/// entity per client — the one it `Controlled` — so this is the local player. The Dynamic body lets
/// the client predict + roll back physics; `InputMarker`/`ActionState` carry native input. Tags
/// [`LocalNetPlayer`] (camera + hidden rig hang off it) + [`MaterializedBody`] (rig attach poll).
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
        }
        info!(
            "materialized LOCAL (predicted) NetworkedPlayer owner={} controlled={is_controlled}",
            owner.0
        );
        crate::trace::event(
            "materialized_player",
            serde_json::json!({ "owner": owner.0, "local": is_controlled }),
        );
    }
}

/// Mark a remote player's newly-`Interpolated` `NetworkedPlayer` as materialized so the rig attaches.
/// No physics body — lightyear interpolation drives its `Position`/`Rotation`. We wait until BOTH
/// `Position` AND `Rotation` are present (the avian Position↔Transform sync only runs when both
/// exist, else the rig would briefly sit at `Transform::default()` — lightyear_avian's documented
/// caveat).
#[allow(clippy::type_complexity)]
fn materialize_interpolated_players(
    new_players: Query<
        (Entity, &NetworkOwner),
        (
            With<NetworkedPlayer>,
            With<Interpolated>,
            With<Position>,
            With<Rotation>,
            Without<MaterializedBody>,
        ),
    >,
    mut commands: Commands,
) {
    for (entity, owner) in &new_players {
        commands
            .entity(entity)
            .insert((MaterializedBody, Visibility::default()));
        info!(
            "materialized remote (interpolated) NetworkedPlayer owner={}",
            owner.0
        );
        crate::trace::event(
            "materialized_player",
            serde_json::json!({ "owner": owner.0, "local": false }),
        );
    }
}

/// Send a `CastRequestMessage` on the reliable `CastChannel` when [`CastIntent`] is set.
/// `aim_dir` is the camera forward vector built from [`CameraYaw`] + [`AimPitch`] (free aim — the
/// projectile flies exactly where the camera looks, pitch included; it can miss). The server fires
/// along this direction via `cast_skill_dir_charged` — no auto-acquire. The client NEVER validates
/// or resolves. Clears the intent after sending (one cast per intent). `Single` sender
/// (multiple would panic — guide §1.2); no-op until the sender exists + we own a local player.
///
/// Reads [`ChargeState::pending_charge`], which is set by `bridge_windowed_cast_hold` on button
/// release and defaults to 85 (≈1.0×) for autocast paths. Resets `pending_charge` to 85 after
/// consuming so the next autocast gets normal-strength casts regardless of the previous hold.
#[allow(clippy::type_complexity)]
fn send_cast_requests(
    mut intent: ResMut<CastIntent>,
    mut charge_state: ResMut<ChargeState>,
    local: Query<
        (&Position, &crate::net::protocol::ObeliskNetId),
        (With<NetworkedPlayer>, With<LocalNetPlayer>),
    >,
    sender: Option<Single<&mut MessageSender<CastRequestMessage>>>,
    mut predicted: MessageWriter<PredictedCast>,
    yaw: Res<super::controller::CameraYaw>,
    pitch: Res<super::controller::AimPitch>,
) {
    let Some(skill_id) = intent.0.clone() else {
        return;
    };
    let Ok((local_pos, local_obelisk_id)) = local.single() else {
        return; // no local player yet
    };
    let Some(mut sender) = sender else {
        return; // sender not ready (pre-connect)
    };
    let here = local_pos.0;

    // Compute aim direction from the camera look vector (yaw + pitch = first-person forward).
    // Matches the camera placement in `controller::follow_local_net_player`: the camera rotation
    // is `Quat::from_axis_angle(Y, yaw) * Quat::from_axis_angle(X, pitch)`, so the forward
    // vector is that rotation applied to `-Z`. Full 3D: pitch is included so looking up/down
    // aims the bolt there. The projectile can miss — this is intentional (free aim).
    let rot = Quat::from_axis_angle(Vec3::Y, yaw.0) * Quat::from_axis_angle(Vec3::X, pitch.0);
    let aim_dir_vec = (rot * -Vec3::Z).normalize();
    let aim_dir = [aim_dir_vec.x, aim_dir_vec.y, aim_dir_vec.z];

    // Consume the locked-in charge byte. Autocast paths leave `pending_charge` at the tap default
    // ([`TAP_CHARGE_BYTE`], ≈1.0×); `bridge_windowed_cast_hold` sets it on release via
    // `charge_byte_from_frac`.
    let charge = charge_state.pending_charge;
    // Reset to tap-default so the next autocast (if any) gets normal-strength, not a stale value.
    charge_state.pending_charge = TAP_CHARGE_BYTE;

    sender.send::<CastChannel>(CastRequestMessage {
        skill_id: skill_id.clone(),
        aim_dir,
        charge,
    });
    crate::trace::event(
        "cast_request_sent",
        serde_json::json!({ "skill_id": skill_id, "aim_dir": aim_dir, "charge": charge }),
    );

    // PREDICTED own-cast feedback (Task 17): fire the local on_cast cosmetics IMMEDIATELY so the
    // windup + cosmetic projectile start with zero perceptible latency. Presentation-only — the
    // server still resolves damage authoritatively (its replicated OnCast cue for this same player
    // is de-duped on this client by `skills::consume_replicated_cues`). NO obelisk Hitbox / RNG.
    predicted.write(PredictedCast {
        skill_id: skill_id.clone(),
        source_id: local_obelisk_id.0.clone(),
        position: here,
        aim_dir: Vec3::from_array(aim_dir),
    });

    intent.0 = None;
}

/// Ship the local player's [`PartSelection`] to the server when [`CustomizeDirty`] is set (debounced
/// on panel close). The server applies it + broadcasts to all clients (D6). Reliable `CastChannel`.
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
    sender.send::<CastChannel>(CustomizeMessage { parts: *selection });
    dirty.0 = false;
    crate::trace::event("customize_sent", serde_json::json!({}));
}

/// Drain the server's [`CustomizeBroadcast`]s and apply each to the matching player's
/// [`PlayerCustomization`] (keyed by the replicated [`NetworkedId`]). Setting the component trips
/// `Changed<PlayerCustomization>`, which `client::parts::refresh_arena_part_visibility_on_change`
/// picks up to re-skin that player's REMOTE rig. The local player's own rig is driven by the local
/// [`PartSelection`] resource, so a loopback broadcast for self is harmless. Added by both client
/// modes (headless just traces — no rig).
fn drain_customize_broadcasts(
    mut receivers: Query<&mut MessageReceiver<CustomizeBroadcast>>,
    mut players: Query<(&NetworkedId, &mut PlayerCustomization), With<NetworkedPlayer>>,
) {
    for mut rx in &mut receivers {
        for msg in rx.receive() {
            for (net_id, mut cust) in &mut players {
                if net_id.0 == msg.player {
                    cust.parts = msg.parts;
                }
            }
            crate::trace::event(
                "customize_received",
                serde_json::json!({ "player": msg.player }),
            );
        }
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

/// [H] check support: throttled trace of a REMOTE (Interpolated) player's avian `Position` (the
/// local player is excluded). Lets the headless movement-replication check confirm the OTHER client
/// observes a moving player's interpolated pose propagate server → this client. Keyed by
/// `NetworkOwner`, gated on `Changed<Position>` (lightyear interpolation writes Position each frame).
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
        if *throttle % 30 == 1 {
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
