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

use crate::net::protocol::{InputChannel, NetworkOwner, NetworkedPlayer, PlayerInputMessage};

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
        app.init_resource::<LocalInput>().add_systems(
            Update,
            (
                materialize_replicated_players,
                send_local_player_input,
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
