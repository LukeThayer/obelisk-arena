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
//!   `NetworkedIdAlloc`, `SPAWN_MARKERS`, `peer_to_u64`, the floor spawn).
//! - [`controller`] — the authoritative force controller (`server_apply_yaw`/`server_apply_movement`)
//!   + `trace_server_pose`.
//! - [`cast_pipeline`] — `drain_cast_requests`.
//! - [`customize`] — `drain_customize_requests`.
//! - [`mirrors`] — the HP/cast-state replication mirrors + `trace_server_net_events`.
//! - [`rounds`] — the best-of-3 round FSM (`RoundState`/`RoundPhase`, `faction_for_slot`,
//!   `cleanup_player_on_disconnect`).

use bevy::prelude::*;

mod cast_pipeline;
mod controller;
mod customize;
mod mirrors;
mod rounds;
mod spawn;

use cast_pipeline::drain_cast_requests;
use controller::{server_apply_movement, server_apply_yaw, trace_server_pose};
use customize::drain_customize_requests;
use mirrors::{sync_cast_state, sync_networked_health, trace_server_net_events};
use rounds::{
    broadcast_round_state, cleanup_player_on_disconnect, detect_round_end, run_round_machine,
    RoundState,
};
use spawn::{spawn_floor, spawn_player_on_connect, ClientPlayerMap, NetworkedIdAlloc};

/// Plugin: server-authoritative arena gameplay — connect-time player spawn, movement controller,
/// cast pipeline, HP/cast-state mirrors, appearance round-trip, and the best-of-3 round machine.
pub struct ArenaServerPlugin;

impl Plugin for ArenaServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedIdAlloc>()
            .init_resource::<ClientPlayerMap>()
            .init_resource::<RoundState>()
            // Load the `.cast.ron` cast timelines the authoritative sim needs (firebolt). Without
            // these, obelisk's `validate_casts` rejects every cast with `TimelineMissing`. The
            // windowed client loads these via the SAME shared `crate::cast_assets` helpers; the
            // headless server must too — it is the combat authority.
            .init_resource::<crate::cast_assets::PendingCastTimelines>()
            .add_systems(
                Startup,
                (crate::cast_assets::load_cast_timelines, spawn_floor),
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
                    // Cast pipeline: drain client cast_requests → free-aim
                    // `cast_skill_dir_charged_from` (fires along the client's `aim_dir`, no
                    // re-acquire) → obelisk's validate_casts gates the rest. The ClientPlayerMap is
                    // populated by `spawn_player_on_connect`; ordered after the lib's Update spatial
                    // refresh (in add_obelisk_sim_headless) so spatial reads see a fresh pipeline.
                    drain_cast_requests,
                    // Appearance pipeline (D6): drain client CustomizeMessage → update that
                    // player's PlayerCustomization + broadcast CustomizeBroadcast to all clients
                    // (reliable), mirroring the cue broadcast. The ClientPlayerMap is populated by
                    // `spawn_player_on_connect`.
                    drain_customize_requests,
                ),
            )
            // Best-of-3 round machine (guide §7). `detect_round_end` reads the death stream
            // and credits the winner; `run_round_machine` drives the FSM (wait → countdown → active →
            // round/match over) + resets/respawns on each new round; `broadcast_round_state` ships
            // the `RoundStateMessage` to every client on a phase/score/countdown change.
            // `detect_round_end` runs before `run_round_machine` so a death this frame is consumed by
            // the FSM the same frame.
            .add_systems(
                Update,
                (detect_round_end, run_round_machine, broadcast_round_state).chain(),
            )
            // The authoritative controller runs in FixedUpdate so it ticks at the fixed 60 Hz the
            // physics group integrates on. It reads the `ActionState<ArenaInput>` lightyear keeps in
            // sync for each client's controlled entity (no manual input drain). `server_apply_yaw`
            // (writes avian `Rotation`) is a separate system from `server_apply_movement` (avian
            // `Forces`) because avian's `Forces` borrows `Rotation` internally; chained so the body
            // faces the input yaw before the movement force is applied.
            .add_systems(
                FixedUpdate,
                (server_apply_yaw, server_apply_movement).chain(),
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
