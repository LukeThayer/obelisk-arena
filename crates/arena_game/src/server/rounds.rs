//! Best-of-3 round state machine (guide §7), lobby-centric since the levels-and-lobby design.
//!
//! The server owns the match flow and broadcasts it as a `RoundStateMessage` on the reliable
//! `EventChannel`. Flow:
//!   Lobby              — everyone hangs out in the lobby LEVEL; the HOST starts a match (G).
//!   Countdown(t)       — ~3s pre-round; reset hp/effects + respawn both at the level's match
//!                        slots on ENTRY. Entered ONLY via an accepted `StartMatchMessage`
//!                        (never auto-started by player count).
//!   Active             — the duel; a round ends when a player's obelisk dies (NetEvent::EntityDied).
//!   RoundOver{winner}  — brief pause crediting the SURVIVOR; first to 2 wins → MatchOver.
//!   MatchOver{winner}  — timed banner (`MATCH_OVER_SECS`), then everyone returns to the LOBBY.
//!
//! Damage stays 100% server-authoritative (obelisk resolves it); this machine only reads the death
//! stream + resets state between rounds. The reset heals to max + clears effects (so a leftover burn
//! DoT doesn't pre-damage the next round) + interrupts any in-flight cast + teleports both back to
//! the current level's match spawn slots.
//!
//! The machine never does level IO: it requests switches via `levels::PendingLevelSwitch`
//! (`apply_level_switch` owns despawn/load/spawn). The `faction_for_slot` helper +
//! `cleanup_player_on_disconnect` observer live here too: factions are RE-asserted by sorted-id
//! slot every round, and a disconnect drops the client's score entry + re-evaluates the phase.

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{Connected, MessageSender, RemoteId};
use obelisk_bevy::prelude::*;
use serde_json::json;

use crate::net::protocol::{
    EventChannel, NetworkOwner, NetworkedPlayer, ObeliskNetId, RoundStateMessage,
};
use crate::net::{COUNTDOWN_SECS, MATCH_OVER_SECS, ROUND_OVER_SECS, ROUND_WINS_TO_MATCH};
use crate::trace;

use super::levels::{
    lobby_spawn_index, spawn_rotation, CurrentLevel, HostState, LevelSpawns, PendingLevelSwitch,
};
use super::spawn::{peer_to_u64, ClientPlayerMap};

use arena_sim::level::LOBBY_LEVEL_ID;

// The match-pacing constants (`ROUND_WINS_TO_MATCH`, `COUNTDOWN_SECS`, `ROUND_OVER_SECS`) live in
// the `net` tuning surface (the "match pacing" sub-section) and are imported above.

/// Map a sorted-client-id slot to its `Faction`: slot 0 → `Player`, every other slot → `Enemy`.
/// Extracted as a pure helper so the slot→faction mapping is unit-testable AND so the connect-time
/// spawn and the per-round reset assign factions IDENTICALLY (by the same sorted-id slot). Because
/// the slot comes from the sorted id list, the two duelists land on slots 0 and 1 regardless of
/// connect order, so they can NEVER share a faction — which would make firebolt's `Enemies`
/// hit-filter resolve zero hits and hang the match unwinnable (see invariant §11).
pub(crate) use arena_sim::spawn::faction_for_slot;

/// Tear down per-client server state when a client's `Connected` is removed (disconnect). Removes
/// the client from `ClientPlayerMap` and drops its score entry from `RoundState` so a stale ghost
/// id can't linger in the HUD/score or perturb the sorted-slot ordering the reset depends on. If
/// this drops the match below 2 players, fall any in-progress phase back to `WaitingForPlayers` so
/// the survivor isn't hung in 'FIGHT!'/countdown forever. (lightyear despawns the disconnected
/// player's replicated entity via its `ControlledBy` lifetime; this only owns the server-side
/// resources keyed off the client.)
pub(crate) fn cleanup_player_on_disconnect(
    trigger: On<Remove, Connected>,
    connections: Query<&RemoteId, With<ClientOf>>,
    mut client_map: ResMut<ClientPlayerMap>,
    mut round: ResMut<RoundState>,
    mut host: ResMut<HostState>,
    mut pending: ResMut<PendingLevelSwitch>,
) {
    let conn_entity = trigger.entity;
    let Ok(RemoteId(peer_id)) = connections.get(conn_entity) else {
        return;
    };
    let Some(client_id) = peer_to_u64(peer_id) else {
        return;
    };
    if client_map.0.remove(&client_id).is_none() {
        return; // not a spawned duelist (idempotent guard)
    }
    // Score is keyed by obelisk_id (`make_combatant` enforces it == "player_{client_id}").
    let obelisk_id = format!("player_{client_id}");
    round.scores.remove(&obelisk_id);

    // Host re-election: first of the join order still connected inherits.
    let prev_host = host.host;
    host.on_disconnect(client_id);
    if host.host != prev_host {
        round.dirty = true; // re-broadcast so the new host learns of its promotion
        if let Some(new_host) = host.host {
            trace::event("host_elected", json!({ "client_id": new_host }));
        }
    }

    // Below 2 players: bail any in-progress (non-terminal) phase back to the LOBBY (level + phase)
    // so the survivor isn't stuck mid-arena. MatchOver is terminal-ish (its timer returns everyone
    // to the lobby anyway); Lobby is already there.
    if client_map.0.len() < 2 {
        match round.phase {
            RoundPhase::Countdown(_) | RoundPhase::Active | RoundPhase::RoundOver { .. } => {
                round.phase = RoundPhase::Lobby;
                round.dirty = true;
                pending.0 = Some(LOBBY_LEVEL_ID.to_string());
            }
            RoundPhase::Lobby | RoundPhase::MatchOver { .. } => {}
        }
    }
    trace::event(
        "player_disconnected",
        json!({ "client_id": client_id, "obelisk_id": obelisk_id }),
    );
}

/// The match phase. Mirrors `RoundStateMessage.phase` (0..=4) but carries the live timer/winner.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RoundPhase {
    /// Hanging out in the lobby level. Exited ONLY by an accepted host start request.
    Lobby,
    Countdown(f32),
    Active,
    RoundOver { winner: String, remaining: f32 },
    MatchOver { winner: String, remaining: f32 },
}

impl RoundPhase {
    /// The wire phase tag (matches the `RoundStateMessage` docstring: 0 lobby, 1 countdown,
    /// 2 active, 3 round-over, 4 match-over).
    fn wire_tag(&self) -> u8 {
        match self {
            RoundPhase::Lobby => 0,
            RoundPhase::Countdown(_) => 1,
            RoundPhase::Active => 2,
            RoundPhase::RoundOver { .. } => 3,
            RoundPhase::MatchOver { .. } => 4,
        }
    }
    fn countdown_secs(&self) -> f32 {
        match self {
            RoundPhase::Countdown(t) => *t,
            RoundPhase::RoundOver { remaining, .. }
            | RoundPhase::MatchOver { remaining, .. } => *remaining,
            _ => 0.0,
        }
    }
    fn winner(&self) -> String {
        match self {
            RoundPhase::RoundOver { winner, .. } | RoundPhase::MatchOver { winner, .. } => {
                winner.clone()
            }
            _ => String::new(),
        }
    }
}

/// Should the Lobby phase hand off to Countdown? ONLY on an explicit, accepted start request with
/// both duelists present — player count alone never starts a match. Pure so it's unit-testable.
pub(crate) fn lobby_should_start(player_count: usize, start_requested: bool) -> bool {
    player_count >= 2 && start_requested
}

/// Server-owned best-of-3 match state. `scores` is keyed by obelisk_id; `needs_round_reset` guards
/// the per-round reset so it runs exactly once on the Countdown→Active transition.
#[derive(Resource)]
pub(crate) struct RoundState {
    pub(crate) phase: RoundPhase,
    /// Round wins per obelisk_id. Populated when 2 players first appear.
    scores: HashMap<String, u8>,
    /// True when the phase/score changed and a `RoundStateMessage` must be (re)broadcast.
    dirty: bool,
    /// Set true on entering `Active`; the reset (heal/respawn) runs on the rising edge.
    needs_round_reset: bool,
    /// An accepted host start request (the level id), set by `levels::drain_start_match` and
    /// consumed by the FSM's Lobby arm the same frame. The level switch itself rides
    /// `PendingLevelSwitch`; this only arms the phase transition.
    pub(crate) start_requested: Option<String>,
}

impl Default for RoundState {
    fn default() -> Self {
        Self {
            phase: RoundPhase::Lobby,
            scores: HashMap::new(),
            dirty: true, // broadcast the initial Lobby once a client can receive it
            needs_round_reset: false,
            start_requested: None,
        }
    }
}

impl RoundState {
    /// The two players' (obelisk_id, wins) in a stable order for the wire `scores` array. Falls back
    /// to empty entries until both players are known.
    fn wire_scores(&self) -> [(String, u8); 2] {
        let mut ids: Vec<(&String, &u8)> = self.scores.iter().collect();
        ids.sort_by(|a, b| a.0.cmp(b.0));
        let mut out = [(String::new(), 0u8), (String::new(), 0u8)];
        for (i, (id, wins)) in ids.into_iter().take(2).enumerate() {
            out[i] = (id.clone(), *wins);
        }
        out
    }

    /// Zero every player's round wins (match start / return to lobby) + mark for re-broadcast.
    pub(crate) fn reset_scores(&mut self) {
        for wins in self.scores.values_mut() {
            *wins = 0;
        }
        self.dirty = true;
    }
}

/// The outcome of the death(s) seen during an `Active` tick. Pure result of `round_outcome` so the
/// winner-selection (and the double-KO draw) is unit-testable without booting an app.
#[derive(Debug, PartialEq)]
enum RoundOutcome {
    /// No CURRENT player died this tick (a stray/non-player death) — the round continues.
    Continue,
    /// BOTH current players died the same tick — a draw: credit no one, replay the round.
    Draw,
    /// Exactly one current player survived — they win the round.
    Winner(String),
}

/// Decide the round outcome from the current players (`all_ids`) and the obelisk ids that died THIS
/// tick (`died_this_tick`). A double-KO (both current players in `died_this_tick`) is a `Draw`
/// rather than arbitrarily crediting whichever corpse the death stream happened to yield first. A
/// death of something that isn't a current player leaves the round running (`Continue`). Pure.
fn round_outcome(all_ids: &[String], died_this_tick: &[String]) -> RoundOutcome {
    let any_current_died = all_ids.iter().any(|id| died_this_tick.contains(id));
    if !any_current_died {
        return RoundOutcome::Continue;
    }
    let mut survivors = all_ids.iter().filter(|id| !died_this_tick.contains(id));
    match (survivors.next(), survivors.next()) {
        // Exactly one survivor among the current players → they win.
        (Some(winner), None) => RoundOutcome::Winner(winner.clone()),
        // Zero survivors (both died) → draw. (More than one survivor is impossible here: a current
        // player died, so a 1v1 leaves at most one survivor.)
        _ => RoundOutcome::Draw,
    }
}

/// Detect a round ending: while `Active`, read obelisk's `EntityDied` stream. Collect ALL deaths
/// this tick (don't break on the first) so a simultaneous double-KO is a DRAW (replay, credit no
/// one) instead of arbitrarily crediting whichever corpse arrived first and maybe handing the
/// match. Otherwise the SURVIVOR wins the round; their score increments and the phase transitions
/// to `RoundOver` (or `MatchOver` at the win threshold). Reads obelisk's `NetEvent` (stable string
/// ids) via an independent cursor from the egress/trace readers.
pub(crate) fn detect_round_end(
    mut net: MessageReader<obelisk_bevy::net::NetEvent>,
    mut round: ResMut<RoundState>,
    players: Query<&ObeliskNetId, With<NetworkedPlayer>>,
) {
    use obelisk_bevy::net::NetEvent;
    // Only deaths during the live round count.
    if round.phase != RoundPhase::Active {
        // Still drain the stream so a death during countdown/reset isn't mis-attributed next round.
        for _ in net.read() {}
        return;
    }
    // Collect every death this tick (not just the first) so a double-KO can be told apart.
    let died_this_tick: Vec<String> = net
        .read()
        .filter_map(|ev| match ev {
            NetEvent::EntityDied { target, .. } => Some(target.clone()),
            _ => None,
        })
        .collect();
    if died_this_tick.is_empty() {
        return;
    }
    // Build the current-player id list only now that a relevant death occurred (not every Active
    // frame).
    let all_ids: Vec<String> = players.iter().map(|o| o.0.clone()).collect();
    match round_outcome(&all_ids, &died_this_tick) {
        RoundOutcome::Continue => {}
        RoundOutcome::Draw => {
            // Replay the round (same pause, no score change). RoundOver with an empty winner.
            trace::event("round_draw", json!({ "died": died_this_tick }));
            round.phase = RoundPhase::RoundOver {
                winner: String::new(),
                remaining: ROUND_OVER_SECS,
            };
            round.dirty = true;
        }
        RoundOutcome::Winner(winner) => {
            let loser = died_this_tick.first().cloned().unwrap_or_default();
            let wins = {
                let w = round.scores.entry(winner.clone()).or_insert(0);
                *w += 1;
                *w
            };
            trace::event(
                "round_won",
                json!({ "winner": winner, "loser": loser, "wins": wins }),
            );
            if wins >= ROUND_WINS_TO_MATCH {
                round.phase = RoundPhase::MatchOver {
                    winner: winner.clone(),
                    remaining: MATCH_OVER_SECS,
                };
                trace::event("match_over", json!({ "winner": winner, "wins": wins }));
            } else {
                round.phase = RoundPhase::RoundOver {
                    winner,
                    remaining: ROUND_OVER_SECS,
                };
            }
            round.dirty = true;
        }
    }
}

/// Drive the round FSM by wall/real time each `Update`. Handles: lobby → countdown (ONLY on an
/// accepted host start request) → active (with the per-round reset on entry) → round-over pause →
/// next countdown; MatchOver holds `MATCH_OVER_SECS` then returns everyone to the lobby. The reset
/// (heal/clear-effects/respawn) runs here on the Countdown→Active edge via `reset_for_new_round`.
#[allow(clippy::type_complexity)]
pub(crate) fn run_round_machine(
    time: Res<Time>,
    mut round: ResMut<RoundState>,
    mut players: Query<
        (
            Entity,
            &ObeliskNetId,
            &mut Attributes,
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &NetworkOwner,
            &mut Faction,
        ),
        With<NetworkedPlayer>,
    >,
    mut commands: Commands,
    client_map: Res<ClientPlayerMap>,
    spawns: Res<LevelSpawns>,
    mut pending: ResMut<PendingLevelSwitch>,
) {
    let dt = time.delta_secs();
    let player_count = players.iter().count();

    // Lazily register both players in `scores` (0 wins) once they exist, so the wire `scores` array
    // carries both obelisk_ids from the first broadcast.
    if player_count >= 2 {
        for (_, net_id, ..) in &players {
            if !round.scores.contains_key(&net_id.0) {
                round.scores.insert(net_id.0.clone(), 0);
                round.dirty = true;
            }
        }
    }

    match round.phase.clone() {
        RoundPhase::Lobby => {
            // No auto-start: the ONLY exit is an accepted host start request (drain_start_match
            // validated it and queued the level switch this same frame).
            let requested = round.start_requested.take();
            if lobby_should_start(player_count, requested.is_some()) {
                round.phase = RoundPhase::Countdown(COUNTDOWN_SECS);
                round.dirty = true;
                trace::event("round_phase", json!({ "phase": "countdown" }));
            }
        }
        RoundPhase::Countdown(t) => {
            // If a player vanished mid-countdown, fall back to the lobby.
            if player_count < 2 {
                round.phase = RoundPhase::Lobby;
                round.dirty = true;
                pending.0 = Some(LOBBY_LEVEL_ID.to_string());
                return;
            }
            let nt = t - dt;
            if nt <= 0.0 {
                round.phase = RoundPhase::Active;
                round.needs_round_reset = true;
                round.dirty = true;
                trace::event("round_phase", json!({ "phase": "active" }));
            } else {
                // Re-broadcast only when the displayed (ceil'd) second changes, not every frame, so
                // the wire carries ~1 countdown update/sec instead of 60.
                round.dirty |= t.ceil() != nt.ceil();
                round.phase = RoundPhase::Countdown(nt);
            }
        }
        RoundPhase::Active => {
            // If a player vanished mid-duel, fall back to the lobby (mirrors the Countdown arm) so
            // a disconnect can't hang the survivor in 'FIGHT!' forever.
            if player_count < 2 {
                round.phase = RoundPhase::Lobby;
                round.dirty = true;
                pending.0 = Some(LOBBY_LEVEL_ID.to_string());
                return;
            }
            // On the rising edge into Active, reset + respawn both players for the new round.
            if round.needs_round_reset {
                round.needs_round_reset = false;
                reset_for_new_round(&mut players, &mut commands, &client_map, &spawns);
                trace::event("round_reset", json!({ "players": player_count }));
            }
        }
        RoundPhase::RoundOver { winner, remaining } => {
            let nr = remaining - dt;
            if nr <= 0.0 {
                // Next round: back to countdown (the reset happens on the Countdown→Active edge).
                round.phase = RoundPhase::Countdown(COUNTDOWN_SECS);
                round.dirty = true;
                trace::event("round_phase", json!({ "phase": "countdown" }));
            } else {
                // Throttle the round-over countdown re-broadcast to ~1/sec (see Countdown above).
                round.dirty |= remaining.ceil() != nr.ceil();
                round.phase = RoundPhase::RoundOver {
                    winner,
                    remaining: nr,
                };
            }
        }
        RoundPhase::MatchOver { winner, remaining } => {
            // Timed banner, then everyone returns to the LOBBY (level switch + fresh scores).
            let nr = remaining - dt;
            if nr <= 0.0 {
                round.phase = RoundPhase::Lobby;
                round.reset_scores();
                pending.0 = Some(LOBBY_LEVEL_ID.to_string());
                trace::event("round_phase", json!({ "phase": "lobby" }));
            } else {
                round.dirty |= remaining.ceil() != nr.ceil();
                round.phase = RoundPhase::MatchOver {
                    winner,
                    remaining: nr,
                };
            }
        }
    }
}

/// Per-round reset (runs on the Countdown→Active edge): heal every player to full, clear effects
/// (drops a leftover burn DoT), interrupt any in-flight cast, RE-assert each player's `Faction` by
/// sorted-id slot (so a connect-order race can never leave the two duelists sharing a faction —
/// which would make every firebolt resolve zero hits), and teleport both to the CURRENT LEVEL's
/// match spawn slots (avian `Position` + the authored facing as `Rotation`, zeroing
/// `LinearVelocity` so a falling/jumping body lands clean). lightyear replicates the pose reset;
/// the predicted owner rolls back to it. Slot is by sorted client id in `ClientPlayerMap` so the
/// two land at slots 0/1 consistently (`drain_start_match` validated both slots exist).
#[allow(clippy::type_complexity)]
fn reset_for_new_round(
    players: &mut Query<
        (
            Entity,
            &ObeliskNetId,
            &mut Attributes,
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &NetworkOwner,
            &mut Faction,
        ),
        With<NetworkedPlayer>,
    >,
    commands: &mut Commands,
    client_map: &ClientPlayerMap,
    spawns: &LevelSpawns,
) {
    // Stable slot assignment: order client ids the same way `spawn_player_on_connect` did (insertion
    // order isn't stable across a HashMap, so sort by client id for determinism).
    let mut ordered: Vec<(u64, Entity)> = client_map.0.iter().map(|(k, v)| (*k, *v)).collect();
    ordered.sort_by_key(|(cid, _)| *cid);
    let slot_of: HashMap<Entity, usize> = ordered
        .iter()
        .enumerate()
        .map(|(i, (_, e))| (*e, i))
        .collect();

    for (entity, net_id, mut attrs, mut position, mut rotation, mut lin_vel, _owner, mut faction) in
        players.iter_mut()
    {
        // Heal to full + restore mana + clear effects (drop any lingering DoT/buff).
        let max_life = attrs.0.computed_max_life();
        let max_mana = attrs.0.computed_max_mana();
        attrs.0.current_life = max_life;
        attrs.0.current_mana = max_mana;
        attrs.0.effects.clear();

        // Interrupt any in-flight cast so the new round starts clean.
        commands.entity(entity).interrupt_cast();

        // Respawn at the current level's spawn for this player's slot (round-robin degrades
        // gracefully if the level somehow has fewer slots than players); zero velocity so the
        // Dynamic body doesn't carry momentum (or a fall) into the new round. Re-assert the slot's
        // faction too (same sorted-id slot), so the two are guaranteed OPPOSING factions before
        // the round goes Active regardless of connect order.
        let slot = slot_of.get(&entity).copied().unwrap_or(0);
        let Some(desc) = spawns
            .slots
            .get(lobby_spawn_index(slot, spawns.slots.len()))
        else {
            continue; // no spawns loaded (unreachable for validated levels)
        };
        position.0 = desc.position;
        *rotation = spawn_rotation(desc);
        lin_vel.0 = Vec3::ZERO;
        *faction = faction_for_slot(slot);

        trace::event(
            "player_respawn",
            json!({ "obelisk_id": net_id.0,
                    "pos": [desc.position.x, desc.position.y, desc.position.z],
                    "life": max_life }),
        );
    }
}

/// Broadcast the current `RoundStateMessage` to every connected client on the reliable `EventChannel`
/// whenever the round state is `dirty` (phase/score/countdown changed). Clears the flag after sending.
/// `match_seed` is the replicated session seed (forward-prep for Stage B; informational in Stage A).
pub(crate) fn broadcast_round_state(
    mut round: ResMut<RoundState>,
    mut senders: Query<&mut MessageSender<RoundStateMessage>, With<ClientOf>>,
    host: Res<HostState>,
    current: Res<CurrentLevel>,
) {
    if !round.dirty {
        return;
    }
    // Don't clear `dirty` until at least one sender exists, else the initial states are lost before
    // a client connects (the reliable channel only delivers to currently-connected senders).
    let mut sent = false;
    let msg = RoundStateMessage {
        phase: round.phase.wire_tag(),
        countdown: round.phase.countdown_secs(),
        scores: round.wire_scores(),
        winner: round.phase.winner(),
        match_seed: crate::net::session_seed(),
        host: host.host.unwrap_or(0),
        level: current.id.clone(),
    };
    for mut sender in &mut senders {
        sender.send::<EventChannel>(msg.clone());
        sent = true;
    }
    if sent {
        round.dirty = false;
        trace::event(
            "round_state",
            json!({ "phase": msg.phase, "countdown": msg.countdown,
                    "scores": msg.scores, "winner": msg.winner,
                    "host": msg.host, "level": msg.level }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{faction_for_slot, lobby_should_start, round_outcome, RoundOutcome};
    use obelisk_bevy::prelude::Faction;

    /// The lobby NEVER auto-starts on player count — only an explicit accepted start request with
    /// both duelists present exits it. (The pre-lobby FSM auto-started at 2 players; the net-test's
    /// autostart hook now supplies the explicit request instead.)
    #[test]
    fn lobby_does_not_autostart_with_two_players() {
        assert!(!lobby_should_start(2, false));
        assert!(!lobby_should_start(1, true));
        assert!(!lobby_should_start(0, false));
        assert!(lobby_should_start(2, true));
    }

    /// Slot 0 → Player, slot 1 → Enemy, and the two are always OPPOSING — so the two duelists can
    /// never share a faction (which would make firebolt's `Enemies` filter resolve zero hits and
    /// hang the match). The net-test uses ascending ids so it never exercises this; pin it here.
    #[test]
    fn faction_for_slot_assigns_opposing_factions() {
        assert_eq!(faction_for_slot(0), Faction::Player);
        assert_eq!(faction_for_slot(1), Faction::Enemy);
        assert_ne!(faction_for_slot(0), faction_for_slot(1));
    }

    /// The slot→faction mapping is CONNECT-ORDER-INDEPENDENT: whichever client connected first,
    /// each client keeps the SAME faction (derived from its position in the sorted id list), and
    /// the two are always opposing. This is the property the faction-order fix guarantees.
    #[test]
    fn faction_slotting_is_connect_order_independent() {
        // Map a pair of (client_id) given in CONNECT order → each client's faction (by sorted slot).
        let factions_for = |connect_order: [u64; 2]| -> [Faction; 2] {
            let mut sorted = connect_order.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            connect_order.map(|id| {
                let slot = sorted.iter().position(|&s| s == id).unwrap();
                faction_for_slot(slot)
            })
        };
        let ab = factions_for([7, 42]); // client 7 connected first
        let ba = factions_for([42, 7]); // client 42 connected first
                                        // Client 7 keeps its faction regardless of who connected first; likewise client 42.
        assert_eq!(ab[0], ba[1]); // client 7
        assert_eq!(ab[1], ba[0]); // client 42
                                  // ...and the two are always opposing.
        assert_ne!(ab[0], ab[1]);
        assert_ne!(ba[0], ba[1]);
    }

    /// A simultaneous double-KO (BOTH current players in the same tick's death set) is a `Draw`
    /// (credit no one, replay) — NOT an arbitrary winner taken from whichever corpse arrived first.
    #[test]
    fn round_outcome_double_ko_is_draw() {
        let players = vec!["player_1".to_string(), "player_2".to_string()];
        // Both orderings of the death set must still be a draw (order-independence is the point).
        for died in [
            vec!["player_1".to_string(), "player_2".to_string()],
            vec!["player_2".to_string(), "player_1".to_string()],
        ] {
            assert_eq!(round_outcome(&players, &died), RoundOutcome::Draw);
        }
    }

    /// One death credits the SURVIVOR; a death of something that isn't a current player leaves the
    /// round running (`Continue`).
    #[test]
    fn round_outcome_single_death_and_stray() {
        let players = vec!["player_1".to_string(), "player_2".to_string()];
        assert_eq!(
            round_outcome(&players, &["player_1".to_string()]),
            RoundOutcome::Winner("player_2".to_string())
        );
        assert_eq!(
            round_outcome(&players, &["minion_99".to_string()]),
            RoundOutcome::Continue
        );
    }
}
