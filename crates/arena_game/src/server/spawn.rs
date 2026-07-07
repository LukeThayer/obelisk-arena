//! Connect-time player spawn + the shared spawn primitives. `spawn_player_on_connect` spawns one
//! networked combatant per connected client (`Replicate::to_clients(All)` +
//! `PredictionTarget::Single(owner)` + `InterpolationTarget::AllExceptSingle(owner)`) in the
//! `On<Add, Connected>` OBSERVER so the owner's replication sender exists before the targets resolve.
//! Also owns the `ClientPlayerMap`/`NetworkedIdAlloc` resources and the `peer_to_u64` id helper.
//! Spawn POSITIONS come from the current level's `LevelSpawns` (the old hard-coded floor +
//! `SPAWN_MARKERS` are gone — `levels::startup_load_levels` loads the lobby level instead).

use std::collections::{HashMap, HashSet};

use avian3d::prelude::Rotation;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{Connected, PeerId, RemoteId, Replicate};
use lightyear::prelude::{ControlledBy, NetworkTarget, PredictionTarget};
use obelisk_bevy::prelude::*;
use serde_json::json;

use crate::net::input::ArenaInput;
use crate::net::protocol::{
    NetworkOwner, NetworkedCastState, NetworkedHealth, NetworkedId, NetworkedPlayer, ObeliskNetId,
    PlayerCustomization,
};
use crate::trace;

use super::levels::{lobby_spawn_index, spawn_rotation, HostState, LevelSpawns};
use super::rounds::faction_for_slot;

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
/// `Faction::Player` + all three reference skills (`grant_skill("firebolt")` +
/// `grant_skill("chain_lightning")` + `grant_skill("blizzard")`; the windowed client picks which one
/// to cast via number-key `SelectedSkill`, see `client/net.rs`) + a CHILD hurtbox + the replicated networked
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
    mut host: ResMut<HostState>,
    spawns: Res<LevelSpawns>,
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

    // Host election: first joiner (by CONNECT order) still connected is the host.
    let prev_host = host.host;
    host.on_connect(client_id);
    if host.host != prev_host {
        if let Some(new_host) = host.host {
            trace::event("host_elected", json!({ "client_id": new_host }));
        }
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
        .unwrap_or(0);
    // Place at the CURRENT level's spawn points, round-robin by sorted-id index (lobby levels can
    // have any number of points; a match level's slots 0/1 coincide with the match placement).
    let spawn_desc = spawns
        .slots
        .get(lobby_spawn_index(slot, spawns.slots.len()))
        .copied();
    let spawn = spawn_desc
        .map(|d| d.position)
        .unwrap_or(Vec3::new(0.0, arena_sim::tuning::GROUND_Y, 0.0));
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

    // The transport-agnostic combatant recipe (shared with the editor preview): obelisk combatant
    // (`make_combatant(StatBlock::with_id(...))` → `Combatant`/`Attributes`/`ObeliskId`) + the slot
    // `Faction` + the server-authoritative Dynamic avian body (rotation axes locked, zero friction so
    // the shared force controller owns planar velocity; capsule half-height 0.59 rests on the floor
    // with feet at world 0; `Position` set to the spawn marker) + a CHILD `Hurtbox` `Sensor` capsule
    // (on a child so the root stays Dynamic; tracks the moving body in the SpatialQuery pipeline).
    let player = arena_sim::spawn::make_arena_combatant(&mut commands, &obelisk_id, faction, spawn);

    // Layer the networked + replicated component set on top of the bare combatant. The body's
    // Position/Rotation/LinearVelocity/AngularVelocity (set by `make_arena_combatant`) replicate
    // (predicted on the owner, interpolated elsewhere).
    commands
        .entity(player)
        .insert((
            // Face the spawn point's authored direction (identity if the level has no spawns).
            spawn_desc
                .as_ref()
                .map(spawn_rotation)
                .unwrap_or(Rotation::default()),
            Name::new(format!("NetworkedPlayer({client_id})")),
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
            // Server-side cast-edge memory (design WS2). Not replicated.
            super::cast_pipeline::PrevCastInput::default(),
        ))
        .insert((
            Replicate::to_clients(NetworkTarget::All),
            // Predict on EVERY client (avian_3d_character pattern): the owner predicts from its own
            // inputs; the opponent's client predicts this body from the server-rebroadcast inputs.
            // No InterpolationTarget — nothing is interpolated any more (design WS1; approach C in
            // the spec flips this back if extrapolation feel loses to delay under the conditioner).
            PredictionTarget::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: conn_entity,
                lifetime: Default::default(),
            },
        ))
        .grant_skill(crate::net::ARENA_SKILLS[0])
        .grant_skill(crate::net::ARENA_SKILLS[1])
        .grant_skill(crate::net::ARENA_SKILLS[2]);

    client_map.0.insert(client_id, player);
}
