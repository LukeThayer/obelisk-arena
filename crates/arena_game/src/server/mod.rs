//! Server-side arena gameplay: the `spawn_player_on_connect` observer spawns one networked combatant
//! per connected client (`Replicate::to_clients(All)` + `PredictionTarget::Single(owner)` +
//! `InterpolationTarget::AllExceptSingle(owner)`), then the authoritative movement controller, cast
//! pipeline, egress bridge, HUD mirror, and best-of-3 round machine all run here.
//!
//! Spawning in the connect OBSERVER (not a polled system) guarantees the owner's replication sender
//! exists before the prediction/interpolation targets resolve.
//!
//! This module is pure plugin wiring; the systems/resources live in cohesive submodules:
//! - [`spawn`] — connect-time player spawn + the shared spawn primitives (`ClientPlayerMap`,
//!   `NetworkedIdAlloc`, `peer_to_u64`).
//! - [`levels`] — server level + host state (`LevelCatalog` scan, lobby startup load,
//!   `CurrentLevel`/`LevelSpawns`/`HostState`/`PendingLevelSwitch`, `drain_start_match`,
//!   `apply_level_switch`).
//! - [`controller`] — the authoritative force controller (`server_apply_yaw`/`server_apply_movement`)
//!   + `trace_server_pose`.
//! - [`cast_pipeline`] — `drain_cast_requests`.
//! - [`customize`] — `drain_customize_requests`.
//! - [`mirrors`] — the HP/cast-state replication mirrors + `trace_server_net_events`.
//! - [`rounds`] — the lobby-centric best-of-3 round FSM (`RoundState`/`RoundPhase`,
//!   `faction_for_slot`, `cleanup_player_on_disconnect`).

use bevy::prelude::*;

mod cast_pipeline;
mod controller;
mod customize;
mod levels;
mod mirrors;
mod rounds;
mod spawn;

use cast_pipeline::detect_cast_edges;
use controller::{server_apply_movement, server_apply_yaw, trace_server_pose};
use customize::drain_customize_requests;
use levels::{
    apply_level_switch, drain_start_match, startup_load_levels, CurrentLevel, HostState,
    LevelSpawns, PendingLevelSwitch,
};
use mirrors::{sync_cast_state, sync_networked_health, trace_server_net_events};
use rounds::{
    broadcast_round_state, cleanup_player_on_disconnect, detect_round_end, run_round_machine,
    RoundState,
};
use spawn::{spawn_player_on_connect, ClientPlayerMap, NetworkedIdAlloc};

/// Plugin: server-authoritative arena gameplay — connect-time player spawn, movement controller,
/// cast pipeline, HP/cast-state mirrors, appearance round-trip, and the best-of-3 round machine.
pub struct ArenaServerPlugin;

impl Plugin for ArenaServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedIdAlloc>()
            .init_resource::<ClientPlayerMap>()
            .init_resource::<RoundState>()
            // Level + host state (levels-and-lobby): `startup_load_levels` replaces these defaults
            // with the scanned catalog + the loaded lobby, but the resources must EXIST from frame
            // zero for the spawn observer / drains that could run the same frame.
            .init_resource::<CurrentLevel>()
            .init_resource::<LevelSpawns>()
            .init_resource::<HostState>()
            .init_resource::<PendingLevelSwitch>()
            // Load the `.cast.ron` cast timelines the authoritative sim needs (firebolt). Without
            // these, obelisk's `validate_casts` rejects every cast with `TimelineMissing`. The
            // windowed client loads these via the SAME shared `crate::cast_assets` helpers; the
            // headless server must too — it is the combat authority.
            .init_resource::<crate::cast_assets::PendingCastTimelines>()
            // `startup_load_levels` scans the level catalog + loads/spawns the LOBBY level
            // (physics-only) — the old hard-coded `spawn_floor` is gone; the floor is level data.
            .add_systems(
                Startup,
                (crate::cast_assets::load_cast_timelines, startup_load_levels),
            )
            .add_systems(
                Update,
                (
                    crate::cast_assets::poll_cast_timelines,
                    trace_server_net_events,
                ),
            )
            .add_systems(
                Update,
                (
                    // Throttled trace of each player's authoritative avian Position so the headless
                    // movement-replication check has the server-side ground truth to compare the
                    // observers' interpolated pose against.
                    trace_server_pose,
                    // Bug 1a: stamp each player's obelisk cast phase into the replicated cast-state
                    // so the OTHER client can animate this player's cast. Every Update (a caster
                    // stands still while casting, so this is NOT gated on Changed).
                    sync_cast_state,
                    // HP mirror: mirror each player's obelisk life → replicated
                    // NetworkedHealth so the client HUD reads server-authoritative hp.
                    sync_networked_health,
                    // Appearance pipeline (D6): drain client CustomizeMessage → update that
                    // player's PlayerCustomization + broadcast CustomizeBroadcast to all clients
                    // (reliable), mirroring the cue broadcast. The ClientPlayerMap is populated by
                    // `spawn_player_on_connect`.
                    drain_customize_requests,
                ),
            )
            // Lobby-centric best-of-3 round machine + level flow, chained so one frame carries a
            // coherent step: `drain_start_match` (validate the host's request → arm the FSM + queue
            // the switch) → `detect_round_end` (death → winner) → `run_round_machine` (the FSM;
            // may queue a lobby return) → `apply_level_switch` (despawn/spawn level entities +
            // replace `CurrentLevel`/`LevelSpawns` + re-place players) → `broadcast_round_state`
            // (ships phase + host + the just-switched level id in ONE message).
            .add_systems(
                Update,
                (
                    drain_start_match,
                    detect_round_end,
                    run_round_machine,
                    apply_level_switch,
                    broadcast_round_state,
                )
                    .chain(),
            )
            // The authoritative controller runs in FixedUpdate so it ticks at the fixed 60 Hz the
            // physics group integrates on. It reads the `ActionState<ArenaInput>` lightyear keeps in
            // sync for each client's controlled entity (no manual input drain). `server_apply_yaw`
            // (writes avian `Rotation`) is a separate system from `server_apply_movement` (avian
            // `Forces`) because avian's `Forces` borrows `Rotation` internally; chained so the body
            // faces the input yaw before the movement force is applied.
            .add_systems(
                FixedUpdate,
                (
                    (server_apply_yaw, server_apply_movement).chain(),
                    // Cast pipeline (design WS2): detect each player's charging falling edge in the
                    // per-tick input stream → resolve a candidate `CastAim` from the skill's
                    // authored `Acquisition` (`resolve_cast_aim`) → insert a `PendingCast` →
                    // obelisk's validate_casts does the authoritative check. Ordered before
                    // ObeliskSet::Validate (commands flush at the ordering edge) so the cast lands
                    // the SAME tick as its input edge.
                    detect_cast_edges.before(obelisk_bevy::prelude::ObeliskSet::Validate),
                ),
            )
            // Spawn each player when its client connects (canonical observer-driven spawn, so the
            // owner's replication sender is ready before Replicate/PredictionTarget resolve).
            .add_observer(spawn_player_on_connect)
            // Clean up the disconnected client's lookup + score entry (and drop the FSM below 2 if
            // it lost a player), so a stale ghost id can't linger in the HUD/score or perturb the
            // reset slot ordering.
            .add_observer(cleanup_player_on_disconnect);
    }
}
