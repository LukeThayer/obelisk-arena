//! Server-side arena gameplay: spawn one networked combatant per connected client + keep the
//! late-joiner replication targets fresh (netcode guide §5.1, §5.7). Later M2 tasks add the
//! movement controller, cast pipeline, egress bridge, HUD mirror, and round machine here.
//!
//! `refresh_replicate_on_connect` is copied VERBATIM from `wisp/src/net/server.rs:208-232`.
//! `sync_networked_players` is adapted from `wisp/src/net/server.rs:284-360` (obelisk combatant in
//! place of wisp's wizard rig).

use std::collections::{HashMap, HashSet};

use avian3d::prelude::*;
use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{Connected, MessageReceiver, PeerId, RemoteId, Replicate};
use obelisk_bevy::prelude::*;
use serde_json::json;
use stat_core::StatBlock;

use crate::net::protocol::{
    NetworkOwner, NetworkedHealth, NetworkedId, NetworkedPlayer, NetworkedPosition, ObeliskNetId,
    PlayerInputMessage,
};
use crate::trace;

pub struct ArenaServerPlugin;

impl Plugin for ArenaServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedIdAlloc>()
            .init_resource::<ClientPlayerMap>()
            .add_systems(
                Update,
                (
                    sync_networked_players,
                    refresh_replicate_on_connect,
                    // Movement pipeline (Task 10): drain client input (Update) → controller
                    // (FixedUpdate, below) → ship the avian-integrated pose back (Update).
                    drain_player_inputs,
                    sync_player_positions,
                ),
            )
            // The authoritative controller runs in FixedUpdate so it ticks at the fixed 60 Hz the
            // physics group integrates on. `apply_player_rotation` (writes avian `Rotation`) is a
            // separate system from `run_player_controller` (writes avian `Position`) to keep each
            // a small, single-responsibility write — mirroring wisp's two-system split. Both write
            // avian components, never Transform (guide §1.2: the per-tick sync clobbers Transform).
            .add_systems(
                FixedUpdate,
                (apply_player_rotation, run_player_controller).chain(),
            );
    }
}

/// Lookup: connected client id → their `NetworkedPlayer` entity. Populated by
/// `sync_networked_players`; later tasks read it for cast attribution / input routing.
#[derive(Resource, Default)]
pub struct ClientPlayerMap(pub HashMap<u64, Entity>);

/// Monotonic counter assigning each replicated entity a peer-stable `NetworkedId`. Starts at 1;
/// 0 is reserved for "unset".
#[derive(Resource, Default)]
pub struct NetworkedIdAlloc {
    next: u64,
}

impl NetworkedIdAlloc {
    /// Allocate the next stable id. (Named `allocate`, not `next`, to avoid clippy's
    /// `Iterator::next` confusion lint — semantics match wisp's `NetworkedIdAlloc::next`.)
    pub fn allocate(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

/// The two fixed arena spawn markers (spec §11 hard-coded geometry). Players are placed by
/// connection order: the first connected client at marker 0, the second at marker 1. Facing each
/// other across the +Z axis.
const SPAWN_MARKERS: [Vec3; 2] = [Vec3::new(-4.0, 1.0, 0.0), Vec3::new(4.0, 1.0, 0.0)];

/// Resolve a netcode `PeerId` to its `u64` client id, matching every id-carrying variant.
fn peer_to_u64(peer: &PeerId) -> Option<u64> {
    match peer {
        PeerId::Netcode(id) | PeerId::Steam(id) | PeerId::Local(id) | PeerId::Entity(id) => {
            Some(*id)
        }
        _ => None,
    }
}

/// Poll each frame to ensure exactly one `NetworkedPlayer` per connected client. A regular system,
/// NOT an observer on `Add<Connected>`, to avoid `Replicate`'s on-insert hook resolving senders
/// before the connection lifecycle is settled (wisp's rationale, `server.rs:279-283`).
///
/// Each player is a full obelisk combatant: `make_combatant(StatBlock::with_id(...))` +
/// `Faction::Player` + `grant_skill("firebolt")` + a hurtbox + the replicated networked component
/// set (`NetworkedPlayer`/`NetworkOwner`/`NetworkedId`/`ObeliskNetId`/`NetworkedHealth`/
/// `NetworkedPosition`) + a server-authoritative dynamic avian body. Replicated with
/// `Replicate::manual(current_senders)` (NOT `NetworkTarget::All`, which snapshots senders at
/// insert and silently breaks the 2nd client — guide §1.2, §5.7).
#[allow(clippy::type_complexity)] // the lightyear ClientOf+Connected filter query is idiomatic
fn sync_networked_players(
    connections: Query<(Entity, &RemoteId), (With<ClientOf>, With<Connected>)>,
    existing: Query<&NetworkOwner>,
    mut commands: Commands,
    mut id_alloc: ResMut<NetworkedIdAlloc>,
    mut client_map: ResMut<ClientPlayerMap>,
) {
    let existing_ids: HashSet<u64> = existing.iter().map(|o| o.0).collect();
    let senders: Vec<Entity> = connections.iter().map(|(e, _)| e).collect();

    for (_, RemoteId(peer_id)) in &connections {
        let Some(client_id) = peer_to_u64(peer_id) else {
            continue;
        };
        if existing_ids.contains(&client_id) {
            continue;
        }

        // Place by connection order (count of already-spawned players), not by raw id, so the two
        // players land at the two fixed markers regardless of their netcode ids.
        let slot = client_map.0.len().min(SPAWN_MARKERS.len() - 1);
        let spawn = SPAWN_MARKERS[slot];
        let net_id = id_alloc.allocate();
        // Stable obelisk id per client. `make_combatant` enforces ObeliskId == StatBlock.id.
        let obelisk_id = format!("player_{client_id}");

        info!(
            "Spawning NetworkedPlayer for client {client_id} (obelisk_id={obelisk_id}, \
             net_id={net_id}, senders={})",
            senders.len()
        );
        trace::event(
            "player_spawned",
            json!({
                "client_id": client_id,
                "net_id": net_id,
                "obelisk_id": obelisk_id,
                "pos": [spawn.x, spawn.y, spawn.z],
            }),
        );

        // Spawn the combatant root + networked + physics components.
        let player = commands
            .spawn_empty()
            .make_combatant(StatBlock::with_id(obelisk_id.clone()))
            .insert((
                Name::new(format!("NetworkedPlayer({client_id})")),
                Faction::Player,
                NetworkedPlayer,
                NetworkOwner(client_id),
                NetworkedId(net_id),
                ObeliskNetId(obelisk_id.clone()),
                NetworkedHealth::default(),
                NetworkedPosition::from_vec3(spawn),
                // Latest input from this client, written by `drain_player_inputs` (Update) and read
                // by the FixedUpdate controller. Defaults to no-movement so an idle player stands.
                PlayerInputState::default(),
            ))
            .insert((
                // Server-authoritative KINEMATIC body. The controller integrates the avian
                // `Position`/`Rotation` directly (kinematic velocity integration) rather than
                // applying forces to a Dynamic body. Rationale: under `LightyearAvianPlugin`'s
                // `Position` mode WITHOUT lightyear frame-interpolation, the `transform_to_position`
                // sync in `RunFixedMainLoop` resets a Dynamic body's Position/velocity from the
                // (stale) Transform each tick, so a force-driven Dynamic body never accumulates
                // motion (empirically: lv stayed 0 despite an 800N force; the plugin's own line-124
                // TODO documents this no-frame-interp footgun). A Kinematic body the server moves by
                // writing `Position` sidesteps that entirely, satisfies the "write avian
                // Position/Rotation, never Transform" rule (guide §1.2), stays deterministic, and is
                // sufficient for the flat hard-coded arena geometry (spec §11). The body still
                // collides (kinematic capsule) so obelisk hit-detection sees it.
                Transform::from_translation(spawn),
                Position(spawn),
                Rotation::default(),
                LinearVelocity::default(),
                RigidBody::Kinematic,
                Collider::capsule(0.4, 1.2),
                LockedAxes::ROTATION_LOCKED,
            ))
            .insert(Replicate::manual(senders.clone()))
            .id();

        commands.entity(player).grant_skill("firebolt");
        // Hurtbox so the server-side hit detection can resolve firebolt hits against this player.
        // `insert_hurtbox` (re)sets the entity Transform to `spawn`, keeping it at the marker.
        insert_hurtbox(&mut commands, player, 0.6, spawn);

        client_map.0.insert(client_id, player);
    }
}

/// When the set of connected clients changes, refresh `Replicate` on every `NetworkedPlayer` with
/// a fresh `manual(senders)` list rebuilt from the currently-connected `ClientOf` set. Required so
/// a late-joining 2nd client receives the 1st client's already-spawned player.
///
/// Copied VERBATIM from `wisp/src/net/server.rs:208-232` (adapted to the arena's single
/// `NetworkedPlayer` target — no lantern/prop/portal classes). `NetworkTarget::All` snapshots the
/// sender list at spawn and doesn't widen on later connects; `manual` fed on the count delta does.
fn refresh_replicate_on_connect(
    senders: Query<Entity, (With<ClientOf>, With<Connected>)>,
    targets: Query<Entity, With<NetworkedPlayer>>,
    mut commands: Commands,
    mut prev_count: Local<usize>,
) {
    let current: Vec<Entity> = senders.iter().collect();
    if current.len() == *prev_count {
        return;
    }
    *prev_count = current.len();
    for entity in &targets {
        commands
            .entity(entity)
            .insert(Replicate::manual(current.clone()));
    }
}

// ---------------------------------------------------------------------------------------------
// Movement (Task 10): server-authoritative controller (guide §5.3a, §5.4, §5.6).
//
// Stage-A movement is server-authoritative (the netcode's option (b), wisp's proven path): the
// client sends only input (`PlayerInputMessage`), the server runs the controller against the
// dynamic avian body, avian integrates, and the resulting pose ships back via `NetworkedPosition`.
// `drain_player_inputs`/`apply_player_rotation`/`run_player_controller`/`sync_player_positions` are
// adapted from `wisp/src/net/server.rs:402-551`, with two arena divergences:
//   1. the pose carries obelisk cast state (`cast_phase`/`cast_elapsed`/`cast_skill`) instead of
//      wisp's single `casting` bool — those are stamped from the player's `ActiveCast` in M2.3;
//      for M2.2 they stay at their `NetworkedPosition::default()` (no-cast) values.
//   2. movement is camera-relative third-person and matches the CLIENT `controller::move_player`
//      convention EXACTLY (movement.x = strafe +right, movement.y = forward; world dir built in
//      the camera-yaw frame, forward = -Z) so the M2.2 client-side predicted controller and the
//      server agree on the same motion.
// ---------------------------------------------------------------------------------------------

/// Per-player latest input, written each `Update` by `drain_player_inputs`, consumed each
/// `FixedUpdate` by `run_player_controller`/`apply_player_rotation`. Defaults to no movement.
#[derive(Component, Default, Clone, Copy)]
pub struct PlayerInputState {
    /// Camera-relative WASD axis: x = strafe (+right), y = forward.
    movement: Vec2,
    /// Camera yaw (radians) the client is facing; the body faces this.
    yaw: f32,
    /// Aim pitch (radians); cosmetic spine lean, replicated for remote cast animation.
    pitch: f32,
    jump: bool,
}

/// Drain `PlayerInputMessage`s from each connected client onto that client's `PlayerInputState`.
/// Latest-wins (the channel is unreliable / latest-wins), matching wisp's `drain_player_inputs`
/// (`server.rs:414-444`).
fn drain_player_inputs(
    mut receivers: Query<(&RemoteId, &mut MessageReceiver<PlayerInputMessage>), With<ClientOf>>,
    mut players: Query<(&NetworkOwner, &mut PlayerInputState), With<NetworkedPlayer>>,
) {
    for (RemoteId(peer_id), mut receiver) in &mut receivers {
        let Some(client_id) = peer_to_u64(peer_id) else {
            continue;
        };
        let mut latest: Option<PlayerInputMessage> = None;
        for msg in receiver.receive() {
            latest = Some(msg);
        }
        let Some(msg) = latest else { continue };
        for (owner, mut input) in &mut players {
            if owner.0 == client_id {
                input.movement = Vec2::new(msg.movement[0], msg.movement[1]);
                input.yaw = msg.yaw;
                input.pitch = msg.pitch;
                input.jump = msg.jump;
            }
        }
    }
}

/// Set each player's body yaw from its input. Writes avian `Rotation` (NOT Transform). Separate
/// from `run_player_controller` because avian's `Forces` borrows `Rotation` internally
/// (wisp/src/net/server.rs:453-459).
fn apply_player_rotation(mut q: Query<(&PlayerInputState, &mut Rotation), With<NetworkedPlayer>>) {
    for (input, mut rot) in &mut q {
        rot.0 = Quat::from_axis_angle(Vec3::Y, input.yaw);
    }
}

/// Server controller (FixedUpdate): integrate the player's avian `Position` from camera-relative
/// input (kinematic velocity integration), then store the planar velocity so the M2.4 locomotion
/// blend + airborne derivation can read it. Writes avian `Position` directly (NOT Transform — guide
/// §1.2). The movement frame matches the client controller exactly (forward = -Z in the camera-yaw
/// frame, movement.x strafes +right, movement.y is forward) so the M2.2 Task 12 client-side
/// predicted controller and the server agree on the same motion.
///
/// Kinematic (not force-driven Dynamic) per the spawn-site rationale: the `LightyearAvianPlugin`
/// Position-mode transform→position sync clobbers a Dynamic body's velocity without frame interp.
/// We exponentially approach the desired ground velocity (a cheap accel feel) and step Position.
#[allow(clippy::type_complexity)]
fn run_player_controller(
    time: Res<Time>,
    mut players: Query<
        (&PlayerInputState, &mut Position, &mut LinearVelocity),
        With<NetworkedPlayer>,
    >,
) {
    const MAX_SPEED: f32 = 4.0; // matches client controller::MOVE_SPEED
                                // Per-tick smoothing toward the desired velocity (~accel feel). 1.0 = instant, smaller = softer.
    const ACCEL_LERP: f32 = 0.35;
    let dt = time.delta_secs().max(1e-5);

    for (input, mut position, mut lin_vel) in &mut players {
        // Camera-relative WASD → world direction. Matches client `controller::move_player`:
        // local = (strafe, 0, -forward); world = RotY(yaw) * local.
        let local = Vec3::new(input.movement.x, 0.0, -input.movement.y);
        let world_dir = Quat::from_axis_angle(Vec3::Y, input.yaw) * local;
        let desired = world_dir.normalize_or_zero() * MAX_SPEED;

        // Approach the desired planar velocity (keep the existing Y for any future gravity/jump).
        let cur = Vec3::new(lin_vel.0.x, 0.0, lin_vel.0.z);
        let new_planar = cur.lerp(desired, ACCEL_LERP);
        lin_vel.0.x = new_planar.x;
        lin_vel.0.z = new_planar.z;

        // Integrate Position from the planar velocity (kinematic). Y is held at the spawn height for
        // the flat arena (no gravity/jump in Stage A; jump is a later cosmetic concern).
        position.0.x += new_planar.x * dt;
        position.0.z += new_planar.z * dt;
    }
}

/// Each `Update` on `Changed<Position>`, copy the avian-integrated pose into the replicated
/// `NetworkedPosition` so clients see the authoritative position + facing. Reads avian `Position`
/// (the canonical source the controller writes — guide §1.2 — not Transform, which lags one
/// position→transform sync behind). Derives `airborne` server-side (never trusts the client).
/// Adapted from `wisp/src/net/server.rs:519-551`; the cast fields stay at default until M2.3 stamps
/// them from `ActiveCast`.
#[allow(clippy::type_complexity)]
fn sync_player_positions(
    mut q: Query<
        (
            Entity,
            &Position,
            &PlayerInputState,
            &NetworkOwner,
            &mut NetworkedPosition,
        ),
        (With<NetworkedPlayer>, Changed<Position>),
    >,
    spatial: SpatialQuery,
    mut throttle: Local<HashMap<Entity, u32>>,
) {
    for (entity, position, input, owner, mut netpos) in &mut q {
        netpos.x = position.0.x;
        netpos.y = position.0.y;
        netpos.z = position.0.z;
        netpos.yaw = input.yaw;
        netpos.pitch = input.pitch;
        // Derive airborne server-side, excluding THIS player from the ray (see the controller note).
        let ray_origin = position.0 + Vec3::new(0.0, -1.0, 0.0);
        let grounded = spatial
            .cast_ray(
                ray_origin,
                Dir3::NEG_Y,
                0.2,
                true,
                &SpatialQueryFilter::from_excluded_entities([entity]),
            )
            .is_some();
        netpos.airborne = !grounded;

        // Throttled pose trace (every ~30th change) so the headless movement-replication check can
        // confirm the server's authoritative NetworkedPosition is changing for the moving player.
        let n = throttle.entry(entity).or_insert(0);
        *n += 1;
        if *n % 30 == 1 {
            trace::event(
                "server_pose",
                json!({
                    "owner": owner.0,
                    "pos": [netpos.x, netpos.y, netpos.z],
                    "yaw": netpos.yaw,
                }),
            );
        }
    }
}
