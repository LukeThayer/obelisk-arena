//! Server-side level + host state (levels-and-lobby design): the scanned [`LevelCatalog`], the
//! currently-loaded level ([`CurrentLevel`]/[`LevelSpawns`]), host election ([`HostState`]), and
//! the two level-flow systems — `drain_start_match` (the ONLY `MessageReceiver<StartMatchMessage>`
//! drain; validates + queues a switch) and `apply_level_switch` (despawns the old level's
//! [`LevelEntity`]s, spawns the new one physics-only, replaces the resources, re-places players).
//!
//! The FSM (`rounds.rs`) never does IO: it (and the disconnect observer) requests a level via
//! [`PendingLevelSwitch`]; this module owns the actual load/despawn/spawn.

use avian3d::prelude::{LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{MessageReceiver, RemoteId};
use serde_json::json;

use arena_sim::level::{
    load_level_scene, spawn_level, LevelCatalog, LevelEntity, SpawnDesc, LOBBY_LEVEL_ID,
};

use crate::net::protocol::{NetworkOwner, NetworkedPlayer, StartMatchMessage};
use crate::trace;

use super::rounds::{RoundPhase, RoundState};
use super::spawn::{peer_to_u64, ClientPlayerMap};

/// The id of the currently-loaded level (a `LevelCatalog` stem). Broadcast in
/// `RoundStateMessage.level` so clients mirror the load.
#[derive(Resource, Debug)]
pub(crate) struct CurrentLevel {
    pub id: String,
}

impl Default for CurrentLevel {
    fn default() -> Self {
        Self {
            id: LOBBY_LEVEL_ID.to_string(),
        }
    }
}

/// The current level's spawn points, sorted by slot (the loader sorts). Lobby placement is
/// round-robin over these; match placement is by slot 0/1.
#[derive(Resource, Debug, Default)]
pub(crate) struct LevelSpawns {
    pub slots: Vec<SpawnDesc>,
}

/// Host election: first joiner still connected. `order` is CONNECT order (never re-sorted — the
/// sorted-id ordering used for spawn slots/factions is a different, deliberate ordering); `host`
/// is the first of `order` still present.
#[derive(Resource, Debug, Default)]
pub(crate) struct HostState {
    pub order: Vec<u64>,
    pub host: Option<u64>,
}

impl HostState {
    pub fn on_connect(&mut self, id: u64) {
        if !self.order.contains(&id) {
            self.order.push(id);
        }
        self.host = self.order.first().copied();
    }

    pub fn on_disconnect(&mut self, id: u64) {
        self.order.retain(|&x| x != id);
        self.host = self.order.first().copied();
    }
}

/// A queued level switch (level id). Set by `drain_start_match` (host request) and the FSM/
/// disconnect fallback (back to lobby); consumed by `apply_level_switch`. A switch to the level
/// that is already loaded is a no-op.
#[derive(Resource, Debug, Default)]
pub(crate) struct PendingLevelSwitch(pub Option<String>);

/// Round-robin lobby placement: the `sorted_index`-th player (by sorted client id) takes spawn
/// point `index % slot_count`. Pure so it's unit-testable.
pub(crate) fn lobby_spawn_index(sorted_index: usize, slot_count: usize) -> usize {
    if slot_count == 0 {
        0
    } else {
        sorted_index % slot_count
    }
}

/// The avian `Rotation` for a spawn's authored facing.
pub(crate) fn spawn_rotation(desc: &SpawnDesc) -> Rotation {
    Rotation(Quat::from_axis_angle(Vec3::Y, desc.yaw))
}

/// Startup: scan the catalog, load + spawn the LOBBY (physics-only — the server never renders),
/// and seed `CurrentLevel`/`LevelSpawns`. The lobby is load-bearing: without it there is no floor
/// and every body falls forever, so a missing/broken lobby is a hard startup error.
pub(crate) fn startup_load_levels(mut commands: Commands) {
    let root = crate::arena_root();
    let catalog = LevelCatalog::scan_roots(&[
        root.join("assets/scenes"),
        root.join("crates/arena_editor/assets/scenes"),
    ]);
    info!(
        "level catalog: {:?}",
        catalog.levels.iter().map(|l| &l.id).collect::<Vec<_>>()
    );
    let lobby = catalog
        .get(LOBBY_LEVEL_ID)
        .unwrap_or_else(|| panic!("no '{LOBBY_LEVEL_ID}' level in {:?}", root.join("assets/scenes")));
    let scene = load_level_scene(&lobby.path)
        .unwrap_or_else(|e| panic!("lobby level failed to load: {e}"));
    let spawned = spawn_level(&mut commands, &scene, None);
    trace::event(
        "level_loaded",
        json!({
            "id": LOBBY_LEVEL_ID,
            "statics": scene.statics.len(),
            "spawns": scene.spawns.len(),
            "entities": spawned.len(),
        }),
    );
    commands.insert_resource(LevelSpawns {
        slots: scene.spawns.clone(),
    });
    commands.insert_resource(CurrentLevel {
        id: LOBBY_LEVEL_ID.to_string(),
    });
    commands.insert_resource(catalog);
}

/// The ONLY `MessageReceiver<StartMatchMessage>` drain (single-drain rule, invariant §8). Accepts
/// a start request iff: sender is the elected host ∧ phase is Lobby ∧ ≥2 players are present ∧ the
/// level exists in the catalog (and isn't the reserved lobby) ∧ it loads with both match slots
/// (0 and 1). On accept: queue the level switch, zero the scores, and arm the FSM's Lobby→Countdown
/// edge (`start_requested`). Anything else warns and is dropped.
#[allow(clippy::type_complexity)]
pub(crate) fn drain_start_match(
    mut receivers: Query<(&RemoteId, &mut MessageReceiver<StartMatchMessage>), With<ClientOf>>,
    host: Res<HostState>,
    client_map: Res<ClientPlayerMap>,
    catalog: Option<Res<LevelCatalog>>,
    mut round: ResMut<RoundState>,
    mut pending: ResMut<PendingLevelSwitch>,
) {
    let Some(catalog) = catalog else { return };
    for (RemoteId(peer_id), mut receiver) in &mut receivers {
        let Some(client_id) = peer_to_u64(peer_id) else {
            continue;
        };
        for msg in receiver.receive() {
            if Some(client_id) != host.host {
                warn!("start_match from non-host client {client_id} — ignored");
                continue;
            }
            if round.phase != RoundPhase::Lobby {
                warn!("start_match outside Lobby (phase {:?}) — ignored", round.phase);
                continue;
            }
            if client_map.0.len() < 2 {
                warn!("start_match with {} player(s) — need 2", client_map.0.len());
                continue;
            }
            if msg.level == LOBBY_LEVEL_ID {
                warn!("start_match on the reserved lobby — ignored");
                continue;
            }
            let Some(info) = catalog.get(&msg.level) else {
                warn!("start_match on unknown level '{}' — ignored", msg.level);
                continue;
            };
            // Validate the level actually loads and has both match slots BEFORE committing.
            let scene = match load_level_scene(&info.path) {
                Ok(s) => s,
                Err(e) => {
                    warn!("start_match: level '{}' failed to load: {e}", msg.level);
                    continue;
                }
            };
            let has = |slot: u8| scene.spawns.iter().any(|s| s.slot == slot);
            if !(has(0) && has(1)) {
                warn!(
                    "start_match: level '{}' lacks match spawn slots 0+1 — ignored",
                    msg.level
                );
                continue;
            }
            round.start_requested = Some(msg.level.clone());
            round.reset_scores();
            pending.0 = Some(msg.level.clone());
            trace::event(
                "match_started",
                json!({ "level": msg.level, "host": client_id }),
            );
        }
    }
}

/// Consume a queued [`PendingLevelSwitch`]: despawn every [`LevelEntity`], spawn the new level
/// (physics-only), replace `CurrentLevel`/`LevelSpawns`, and re-place every player round-robin
/// over the new level's spawn points (sorted-client-id order — the same ordering the spawn/reset
/// slots use). Runs after the FSM so a same-frame request lands this frame; a switch to the
/// already-loaded level is a no-op.
#[allow(clippy::type_complexity)]
pub(crate) fn apply_level_switch(
    mut pending: ResMut<PendingLevelSwitch>,
    catalog: Option<Res<LevelCatalog>>,
    mut current: ResMut<CurrentLevel>,
    mut spawns: ResMut<LevelSpawns>,
    level_entities: Query<Entity, With<LevelEntity>>,
    mut players: Query<
        (
            &NetworkOwner,
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
        ),
        With<NetworkedPlayer>,
    >,
    mut commands: Commands,
) {
    let Some(target) = pending.0.take() else { return };
    if target == current.id {
        return;
    }
    let Some(catalog) = catalog else { return };
    let Some(info) = catalog.get(&target) else {
        warn!("level switch to unknown '{target}' — keeping '{}'", current.id);
        return;
    };
    let scene = match load_level_scene(&info.path) {
        Ok(s) => s,
        Err(e) => {
            warn!("level switch: '{target}' failed to load ({e}) — keeping '{}'", current.id);
            return;
        }
    };
    for e in &level_entities {
        commands.entity(e).despawn();
    }
    let spawned = spawn_level(&mut commands, &scene, None);
    trace::event(
        "level_loaded",
        json!({
            "id": target,
            "statics": scene.statics.len(),
            "spawns": scene.spawns.len(),
            "entities": spawned.len(),
        }),
    );
    spawns.slots = scene.spawns.clone();
    current.id = target;

    // Re-place everyone on the new level (round-robin, sorted-client-id order) so nobody is left
    // hovering over despawned geometry. A match start re-places again on the Countdown→Active
    // reset (by match slot), which for slots 0/1 is the same spot.
    let mut ordered: Vec<_> = players.iter_mut().collect();
    ordered.sort_by_key(|(owner, ..)| owner.0);
    let n = spawns.slots.len();
    for (i, (_, mut position, mut rotation, mut lin_vel)) in ordered.into_iter().enumerate() {
        let Some(desc) = spawns.slots.get(lobby_spawn_index(i, n)) else {
            continue;
        };
        position.0 = desc.position;
        *rotation = spawn_rotation(desc);
        lin_vel.0 = Vec3::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Host = FIRST JOINER still connected; on the host leaving, the NEXT in join order inherits;
    /// a later joiner never steals it back.
    #[test]
    fn host_is_first_joiner_and_reelects_in_join_order() {
        let mut h = HostState::default();
        h.on_connect(7);
        h.on_connect(3);
        assert_eq!(h.host, Some(7));
        h.on_disconnect(7);
        assert_eq!(h.host, Some(3));
        h.on_connect(9);
        assert_eq!(h.host, Some(3));
        h.on_disconnect(3);
        assert_eq!(h.host, Some(9));
        h.on_disconnect(9);
        assert_eq!(h.host, None);
    }

    /// Lobby placement is round-robin over the level's spawn points by sorted-id index.
    #[test]
    fn lobby_slot_assignment_is_round_robin() {
        assert_eq!(lobby_spawn_index(0, 4), 0);
        assert_eq!(lobby_spawn_index(1, 4), 1);
        assert_eq!(lobby_spawn_index(5, 4), 1);
        assert_eq!(lobby_spawn_index(3, 0), 0); // degenerate: no spawns → index 0 (fallback)
    }

    /// `spawn_rotation` faces the body along the spawn's authored forward.
    #[test]
    fn spawn_rotation_faces_authored_yaw() {
        let desc = SpawnDesc {
            slot: 0,
            position: Vec3::ZERO,
            yaw: -std::f32::consts::FRAC_PI_2, // faces +X
        };
        let fwd = spawn_rotation(&desc).0 * -Vec3::Z;
        assert!((fwd - Vec3::X).length() < 1e-4, "{fwd:?}");
    }
}
