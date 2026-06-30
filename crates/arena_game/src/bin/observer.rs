//! arena-observer: the scripted headless trace client (M2.5 Task 20, netcode guide §8.2).
//!
//! A headless lightyear client (no window, no rendering) that connects to the dedicated server,
//! receives replication, scripts a firebolt cast over the wire (a real `CastRequestMessage`, the
//! same send path the windowed client uses), and emits a `trace::event` for every replicated
//! `NetEventMessage` (`client_net_cast_began` / `client_net_damage_resolved` / …),
//! `CueWireMessage` (`client_cue_received`), and the interpolated avian `Position` (`remote_pose`) it
//! observes.
//!
//! Rather than fabricate a second, parallel headless client, the observer IS the existing
//! `client::run_headless_client` — the `arena-client` binary's `ARENA_HEADLESS=1` mode. That mode
//! already: connects + materializes replicated players, drains + traces the replicated
//! `NetEventMessage`/`CueWireMessage` streams, traces remote pose/health/round-state, and (under
//! `ARENA_AUTOCAST=1`) scripts firebolt casts via `send_cast_requests` (the wire cast path). So the
//! observer is a thin alias — the harness sets `ARENA_AUTOCAST=1` on the casting observer and leaves
//! it off on the watching one. This keeps a single, tested headless-client codebase (the guide's
//! "reuse the existing ARENA_HEADLESS client + AUTOCAST, whichever is less code").
//!
//! Env knobs the harness uses: `ARENA_CLIENT_ID` (pins the spawn slot), `ARENA_AUTOCAST=1` +
//! `ARENA_AUTOCAST_PERIOD` (script firebolt casts), `ARENA_TRACE_FILE`/`ARENA_TRACE_SRC`
//! (per-process JSONL), `ARENA_MATCH_SEED` (deterministic combat RNG mirror).

fn main() {
    arena_game::client::run_headless_client();
}
