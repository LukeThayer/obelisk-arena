//! Connect-time player spawn + the shared spawn primitives. `spawn_player_on_connect` spawns one
//! networked combatant per connected client (`Replicate::to_clients(All)` +
//! `PredictionTarget::Single(owner)` + `InterpolationTarget::AllExceptSingle(owner)`) in the
//! `On<Add, Connected>` OBSERVER so the owner's replication sender exists before the targets resolve.
//! Also owns the `ClientPlayerMap`/`NetworkedIdAlloc` resources, the `SPAWN_MARKERS` geometry, the
//! `peer_to_u64` id helper, and the static floor spawn — all shared by spawn AND the per-round reset.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::*;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{Connected, PeerId, RemoteId, Replicate};
use lightyear::prelude::{ControlledBy, InterpolationTarget, NetworkTarget, PredictionTarget};
use obelisk_bevy::prelude::*;
use serde_json::json;
use stat_core::StatBlock;

use crate::net::input::ArenaInput;
use crate::net::protocol::{
    NetworkOwner, NetworkedCastState, NetworkedHealth, NetworkedId, NetworkedPlayer, ObeliskNetId,
    PlayerCustomization,
};
use crate::net::{PLAYER_CAPSULE_LENGTH, PLAYER_CAPSULE_RADIUS};
use crate::trace;

use super::rounds::faction_for_slot;

/// Spawn the static arena floor collider (server-side) the Dynamic player bodies rest on.
pub(crate) fn spawn_floor(mut commands: Commands) {
    crate::spawn_arena_floor(&mut commands);
}

/// Lookup: connected client id → their `NetworkedPlayer` entity. Populated by
/// `spawn_player_on_connect`; read for cast attribution / input routing.
#[derive(Resource, Default)]
pub(crate) struct ClientPlayerMap(pub HashMap<u64, Entity>);

/// Monotonic counter assigning each replicated entity a peer-stable `NetworkedId`. Starts at 1;
/// 0 is reserved for "unset".
#[derive(Resource, Default)]
pub(crate) struct NetworkedIdAlloc {
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
pub(crate) const SPAWN_MARKERS: [Vec3; 2] = [
    Vec3::new(-4.0, crate::net::GROUND_Y, 0.0),
    Vec3::new(4.0, crate::net::GROUND_Y, 0.0),
];

/// Resolve a netcode `PeerId` to its `u64` client id, matching every id-carrying variant.
pub(crate) fn peer_to_u64(peer: &PeerId) -> Option<u64> {
    match peer {
        PeerId::Netcode(id) | PeerId::Steam(id) | PeerId::Local(id) | PeerId::Entity(id) => {
            Some(*id)
        }
        _ => None,
    }
}

/// Each player is a full obelisk combatant: `make_combatant(StatBlock::with_id(...))` +
/// `Faction::Player` + `grant_skill("firebolt")` + a CHILD hurtbox + the replicated networked
/// component set (`NetworkedPlayer`/`NetworkOwner`/`NetworkedId`/`ObeliskNetId`/`NetworkedHealth`/
/// `NetworkedCastState`/`PlayerCustomization`) + a Dynamic avian body driven by the shared force
/// controller. Replicated with `Replicate::to_clients(NetworkTarget::All)` +
/// `PredictionTarget::Single(owner)` + `InterpolationTarget::AllExceptSingle(owner)`: lightyear
/// auto-creates a `Predicted` entity on the owner's client and `Interpolated` entities elsewhere.
///
/// Spawn one `NetworkedPlayer` per client in the `On<Add, Connected>` OBSERVER (the canonical
/// lightyear `avian_3d_character`/`simple_box` pattern). Spawning here — NOT the old polled system —
/// guarantees the just-connected client's replication SENDER is established before `Replicate` +
/// `PredictionTarget`/`InterpolationTarget` resolve, so the OWNER reliably receives its `Predicted`
/// entity. (The polled spawn raced the per-connection sender setup: the owner that connected the
/// same frame the spawn ran was skipped by `to_clients(All)` and received NO replication at all —
/// no `Predicted` entity, hence no local player, hence no cast. The old "polled to avoid the
/// on-insert sender race" rationale was for the removed `Replicate::manual` path; the codebase
/// already drives connection setup from `On<Add, Connected>` observers, so this is consistent.)
#[allow(clippy::type_complexity)]
pub(crate) fn spawn_player_on_connect(
    trigger: On<Add, Connected>,
    connections: Query<(Entity, &RemoteId), (With<ClientOf>, With<Connected>)>,
    existing: Query<&NetworkOwner>,
    mut commands: Commands,
    mut id_alloc: ResMut<NetworkedIdAlloc>,
    mut client_map: ResMut<ClientPlayerMap>,
) {
    let conn_entity = trigger.entity;
    let Ok((_, RemoteId(peer_id))) = connections.get(conn_entity) else {
        return;
    };
    let Some(client_id) = peer_to_u64(peer_id) else {
        return;
    };
    let existing_ids: HashSet<u64> = existing.iter().map(|o| o.0).collect();
    if existing_ids.contains(&client_id) {
        return; // already spawned (idempotent guard)
    }

    // Stable slot by SORTED client id over all currently-connected clients (matches
    // `reset_for_new_round`'s sorted-client-id slots so the initial spawn == the reset position and
    // players don't teleport/swap at the first round reset).
    let mut all_ids: Vec<u64> = connections
        .iter()
        .filter_map(|(_, RemoteId(p))| peer_to_u64(p))
        .collect();
    all_ids.sort_unstable();
    all_ids.dedup();
    let slot = all_ids
        .iter()
        .position(|&id| id == client_id)
        .unwrap_or(0)
        .min(SPAWN_MARKERS.len() - 1);
    let spawn = SPAWN_MARKERS[slot];
    // OPPOSING factions so firebolt's `hit_filter: Enemies` (target_faction != caster_faction)
    // can resolve a hit player→player, and `nearest_enemy` acquires the opponent. With obelisk's
    // 3-faction model (Player/Enemy/Neutral), a 2-player duel puts slot 0 on Player and slot 1
    // on Enemy — they are mutual enemies. (If both shared `Faction::Player`, every cast would
    // pass validation but resolve ZERO hits — the filter rejects same-faction targets.) The same
    // sorted-id slot drives `reset_for_new_round`, which RE-asserts this each round (so a
    // connect-order race can't leave them sharing a faction once the round goes Active).
    let faction = faction_for_slot(slot);
    let net_id = id_alloc.allocate();
    // Stable obelisk id per client. `make_combatant` enforces ObeliskId == StatBlock.id.
    let obelisk_id = format!("player_{client_id}");

    info!(
        "Spawning NetworkedPlayer for client {client_id} (obelisk_id={obelisk_id}, \
             net_id={net_id})"
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

    // Spawn the combatant root + networked + Dynamic physics components.
    let player = commands
        .spawn_empty()
        .make_combatant(StatBlock::with_id(obelisk_id.clone()))
        .insert((
            Name::new(format!("NetworkedPlayer({client_id})")),
            faction,
            NetworkedPlayer,
            NetworkOwner(client_id),
            NetworkedId(net_id),
            ObeliskNetId(obelisk_id.clone()),
            NetworkedHealth::default(),
            // Replicated appearance — default witch on spawn; live edits arrive via D6.
            PlayerCustomization::default(),
            // Replicated cast state (remote cast animation). Starts idle.
            NetworkedCastState::default(),
            // Native input — lightyear syncs this with the controlling client's input each tick.
            ActionState::<ArenaInput>::default(),
        ))
        .insert((
            // Server-authoritative Dynamic body driven by the shared force controller. Rotation
            // axes locked (the body only yaws via the controller's direct `Rotation` write);
            // zero friction so the force controller fully owns the planar velocity (avian's
            // `move_towards` recipe). The capsule (half-height 0.59) rests on the static floor
            // with feet at world 0. Position/Rotation/LinearVelocity/AngularVelocity replicate
            // (predicted on the owner, interpolated elsewhere).
            Position(spawn),
            Rotation::default(),
            LinearVelocity::default(),
            AngularVelocity::default(),
            RigidBody::Dynamic,
            Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH),
            LockedAxes::default()
                .lock_rotation_x()
                .lock_rotation_y()
                .lock_rotation_z(),
            Friction::new(0.0).with_combine_rule(CoefficientCombine::Min),
        ))
        .insert((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(*peer_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(*peer_id)),
            ControlledBy {
                owner: conn_entity,
                lifetime: Default::default(),
            },
        ))
        .id();

    commands.entity(player).grant_skill("firebolt");
    // Hurtbox so the server-side hit detection resolves firebolt hits against this player. It
    // lives on a CHILD entity (NOT the player root) so the player stays `RigidBody::Dynamic` —
    // a `RigidBody::Static` hurtbox on the same entity would conflict with the Dynamic body. The
    // child carries only `Hurtbox` + a `Collider` (no RigidBody), so avian attaches it to the
    // parent Dynamic body as a compound child collider that TRACKS the moving/jumping player and
    // stays in the SpatialQuery pipeline (obelisk's `detect_overlaps` resolves the child entity
    // to its `Hurtbox.owner` = the player). A `Sensor` so it adds a queryable volume without
    // contributing contact forces. The shared player capsule spans the body feet→head (origin ±0.59).
    commands.entity(player).with_children(|c| {
        c.spawn((
            Name::new("Hurtbox"),
            Hurtbox { owner: player },
            Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH),
            Sensor,
            Transform::default(),
        ));
    });

    client_map.0.insert(client_id, player);
}
