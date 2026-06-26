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
use lightyear::prelude::{Connected, PeerId, RemoteId, Replicate};
use obelisk_bevy::prelude::*;
use serde_json::json;
use stat_core::StatBlock;

use crate::net::protocol::{
    NetworkOwner, NetworkedHealth, NetworkedId, NetworkedPlayer, NetworkedPosition, ObeliskNetId,
};
use crate::trace;

pub struct ArenaServerPlugin;

impl Plugin for ArenaServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedIdAlloc>()
            .init_resource::<ClientPlayerMap>()
            .add_systems(
                Update,
                (sync_networked_players, refresh_replicate_on_connect),
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
            ))
            .insert((
                // Server-authoritative dynamic body (same shape as wisp's player; the M2.2
                // controller drives it via forces). Position is avian-canonical; Transform mirrors.
                Transform::from_translation(spawn),
                Position(spawn),
                Rotation::default(),
                LinearVelocity::default(),
                RigidBody::Dynamic,
                Collider::capsule(0.4, 1.2),
                LockedAxes::ROTATION_LOCKED,
                Mass(80.0),
                Friction::new(0.0),
                LinearDamping(0.5),
                Restitution::new(0.0),
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
