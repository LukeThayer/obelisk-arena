//! Client-side net-driven player layer (M2.2 Task 10+).
//!
//! Two jobs, shared by the windowed and headless clients:
//!   1. **Materialize a body** for every replicated `NetworkedPlayer`: the server replicates the
//!      `NetworkedPlayer` marker + identity components + the `NetworkedPosition` pose stream, but
//!      NOT a renderable/physical body. This module spawns a local avian body (so the M2.2 Task 11
//!      smoothing has Transform + avian `Position`/`Rotation` to drive) and tags the LOCAL player
//!      (the one whose `NetworkOwner` matches our `LocalId`) with [`LocalNetPlayer`].
//!   2. **Send local input**: pack the local player's WASD/yaw/pitch/jump into a
//!      [`PlayerInputMessage`] each `Update` on the unreliable `InputChannel`. The server runs its
//!      authoritative controller against it (server/mod.rs) and ships the pose back.
//!
//! Input is sourced from a [`LocalInput`] resource so the windowed controller (real keyboard +
//! mouse-yaw) and the headless `ARENA_AUTOMOVE` hook can both feed the same send path. The send
//! path mirrors `wisp/src/net/client.rs:116-146` (`Single<&mut MessageSender<…>>`).

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{LocalId, MessageSender, PeerId};

use crate::net::protocol::{
    CastChannel, CastRequestMessage, InputChannel, NetworkOwner, NetworkedPlayer,
    NetworkedPosition, PlayerInputMessage,
};

/// The local player's current input, written each frame by whichever input source is active
/// (the windowed controller, or the headless `ARENA_AUTOMOVE` hook) and read by
/// [`send_local_player_input`]. Camera-relative: `movement.x` strafes +right, `movement.y` is
/// forward; `yaw` is the camera/body yaw the server faces the body toward.
#[derive(Resource, Default, Clone, Copy)]
pub struct LocalInput {
    pub movement: Vec2,
    pub yaw: f32,
    pub pitch: f32,
    pub jump: bool,
}

/// One-shot request to cast a skill, set by the windowed cast key or the headless `ARENA_AUTOCAST`
/// hook and consumed by [`send_cast_requests`]. `Some(skill_id)` means "send a cast_request this
/// frame"; the send system clears it back to `None`. The CLIENT never validates or resolves — it
/// only requests; the server re-validates + resolves authoritatively (Stage A, guide §5.2).
#[derive(Resource, Default)]
pub struct CastIntent(pub Option<String>);

/// Maximum hold time for the charge mechanic. A full hold of this duration maps to charge=255
/// (2.0× multiplier); an instant tap maps to charge=85 (≈1.0×).
pub const MAX_CHARGE_SECS: f32 = 1.5;

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
    /// Charge byte locked in at release; consumed (and reset to tap-default 85) by
    /// `send_cast_requests`. Initialized to 85 so autocast paths get ≈1.0× strength.
    pub(crate) pending_charge: u8,
}

impl Default for ChargeState {
    fn default() -> Self {
        Self {
            secs: 0.0,
            charging: false,
            pending_charge: 85, // frac=0 → charge_mult(85) ≈ 1.0×
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

/// Marker on the local player's materialized body — the replicated `NetworkedPlayer` whose
/// `NetworkOwner` equals our own `LocalId`. The windowed client attaches the follow camera + rig to
/// this; M2.2 Task 12 marks the predicted/controlled body.
#[derive(Component)]
pub struct LocalNetPlayer;

/// Marker on a replicated `NetworkedPlayer` that this client has already materialized a body for, so
/// `materialize_replicated_players` is idempotent (it runs every frame, polling for new replicas).
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
            .add_message::<PredictedCast>()
            .add_systems(
                Update,
                (
                    materialize_replicated_players,
                    send_local_player_input,
                    send_cast_requests,
                    trace_received_remote_pose,
                ),
            );
    }
}

/// Resolve a netcode `PeerId` to its `u64` client id (matches every id-carrying variant).
fn peer_to_u64(peer: &PeerId) -> Option<u64> {
    match peer {
        PeerId::Netcode(id) | PeerId::Steam(id) | PeerId::Local(id) | PeerId::Entity(id) => {
            Some(*id)
        }
        _ => None,
    }
}

/// The local client's own id, read off the `LocalId` on the connected (non-`ClientOf`) client
/// entity. `None` until the handshake completes.
fn local_client_id(local_ids: &Query<&LocalId, Without<ClientOf>>) -> Option<u64> {
    local_ids.iter().next().and_then(|l| peer_to_u64(&l.0))
}

/// For every replicated `NetworkedPlayer` we haven't bodied yet, attach a local avian body
/// (`Transform` + `Position` + `Rotation` + a kinematic-ish capsule) so the pose stream has
/// something to drive, and tag the local one with [`LocalNetPlayer`]. Idempotent via
/// [`MaterializedBody`]. Polled (not an `Add` observer) because `NetworkOwner` and
/// `NetworkedPosition` arrive in separate replication packets — we wait until `NetworkOwner` is
/// present so the local/remote tag is correct on the first body.
#[allow(clippy::type_complexity)]
fn materialize_replicated_players(
    new_players: Query<
        (
            Entity,
            &NetworkOwner,
            &crate::net::protocol::NetworkedPosition,
        ),
        (With<NetworkedPlayer>, Without<MaterializedBody>),
    >,
    local_ids: Query<&LocalId, Without<ClientOf>>,
    mut commands: Commands,
) {
    let my_id = local_client_id(&local_ids);
    for (entity, owner, netpos) in &new_players {
        let is_local = my_id == Some(owner.0);
        let spawn = netpos.to_vec3();
        // A local avian body so M2.2 Task 11 smoothing (which writes Transform + Position/Rotation)
        // has a body to drive, and so the local controller (Task 12) can run physics on it. The
        // pose is authoritatively driven by the server via NetworkedPosition; this body is a render
        // proxy, NOT a second authority. Kinematic so it never fights the smoothing/snap writes.
        commands.entity(entity).insert((
            MaterializedBody,
            Transform::from_translation(spawn),
            Visibility::default(),
            avian3d::prelude::RigidBody::Kinematic,
            avian3d::prelude::Position(spawn),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::Collider::capsule(0.4, 1.2),
        ));
        if is_local {
            commands.entity(entity).insert(LocalNetPlayer);
        }
        info!(
            "materialized {} body for NetworkedPlayer owner={} at {spawn:?}",
            if is_local { "LOCAL" } else { "remote" },
            owner.0
        );
        crate::trace::event(
            "materialized_player",
            serde_json::json!({ "owner": owner.0, "local": is_local,
                "pos": [spawn.x, spawn.y, spawn.z] }),
        );
    }
}

/// Send the local player's input to the server each `Update`. Reads [`LocalInput`] (written by the
/// windowed controller or the `ARENA_AUTOMOVE` hook) and ships it on the unreliable `InputChannel`.
/// `Single<&mut MessageSender<…>>` — multiple senders would panic (guide §1.2); there's exactly one
/// client→server message sender per peer. No-op until the sender exists (post-connect).
fn send_local_player_input(
    input: Res<LocalInput>,
    local_players: Query<(), (With<NetworkedPlayer>, With<LocalNetPlayer>)>,
    sender: Option<Single<&mut MessageSender<PlayerInputMessage>>>,
) {
    // Only send once we actually own a local player (else the server has nothing to apply it to).
    if local_players.iter().next().is_none() {
        return;
    }
    let Some(mut sender) = sender else {
        return;
    };
    sender.send::<InputChannel>(PlayerInputMessage {
        movement: [input.movement.x, input.movement.y],
        yaw: input.yaw,
        pitch: input.pitch,
        jump: input.jump,
    });
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
        (&NetworkedPosition, &crate::net::protocol::ObeliskNetId),
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
    let here = local_pos.to_vec3();

    // Compute aim direction from the camera look vector (yaw + pitch = first-person forward).
    // Matches the camera placement in `controller::follow_local_net_player`: the camera rotation
    // is `Quat::from_axis_angle(Y, yaw) * Quat::from_axis_angle(X, pitch)`, so the forward
    // vector is that rotation applied to `-Z`. Full 3D: pitch is included so looking up/down
    // aims the bolt there. The projectile can miss — this is intentional (free aim).
    let rot = Quat::from_axis_angle(Vec3::Y, yaw.0) * Quat::from_axis_angle(Vec3::X, pitch.0);
    let aim_dir_vec = (rot * -Vec3::Z).normalize();
    let aim_dir = [aim_dir_vec.x, aim_dir_vec.y, aim_dir_vec.z];

    // Consume the locked-in charge byte. Autocast paths leave `pending_charge` at the tap default
    // (85, ≈1.0×); `bridge_windowed_cast_hold` sets it on release from the hold duration.
    let charge = charge_state.pending_charge;
    // Reset to tap-default so the next autocast (if any) gets normal-strength, not a stale value.
    charge_state.pending_charge = 85;

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

/// [H] check support: throttled trace of a REMOTE player's received `NetworkedPosition` (the local
/// player is excluded). Lets the headless movement-replication check confirm that the OTHER client
/// observes a moving player's pose change propagate server → this client. Keyed by `NetworkOwner`.
#[allow(clippy::type_complexity)]
fn trace_received_remote_pose(
    remotes: Query<
        (&NetworkOwner, &crate::net::protocol::NetworkedPosition),
        (
            With<NetworkedPlayer>,
            Without<LocalNetPlayer>,
            Changed<crate::net::protocol::NetworkedPosition>,
        ),
    >,
    mut throttle: Local<u32>,
) {
    for (owner, netpos) in &remotes {
        *throttle += 1;
        if *throttle % 30 == 1 {
            crate::trace::event(
                "remote_pose",
                serde_json::json!({
                    "owner": owner.0,
                    "pos": [netpos.x, netpos.y, netpos.z],
                    "yaw": netpos.yaw,
                }),
            );
        }
    }
}
