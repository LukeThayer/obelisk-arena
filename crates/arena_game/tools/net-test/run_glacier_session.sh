#!/usr/bin/env bash
# obelisk-arena SURFACES net regression — the GLACIER session (surfaces-nettest Task 3).
#
# A second scripted session (sibling to run_session.sh's firebolt gate) that exercises the
# obelisk SURFACES pipeline end-to-end over the wire: paint -> gate -> consume -> ground-flush
# erupt. It equips `potted_spring` on BOTH observers and scripts observer-0 to ALTERNATE
# `rolling_glacier` (a ballistic lob that lands and ROLLS, painting a `frost` Trail) and
# `frost_spire` (a GroundPoint cast whose `on_surface: frost` acquisition gate must land on a
# trail patch — snapping to + CONSUMING it — then erupting a ground-flush spire). The gate is
# check_glacier_session.sh (jq); this script only PRODUCES the session dir.
#
# What check_glacier_session.sh then asserts over the merged per-process JSONL:
#   - the roll painted the frost trail server-side (surface_painted{frost}) AND it replicated to
#     both observers (replicated_surface_patch{frost});
#   - a frost_spire cast was ACCEPTED (server_net_cast_began{frost_spire} — the on_surface gate
#     matched a trail patch), which CONSUMED its fuel (surface_removed{frost, reason:Consumed});
#   - the spire erupted GROUND-FLUSH (spire_erupted anchor |y| <= 0.25 — the Task-4 float-above-
#     the-floor regression's e2e pin);
#   - the chain ran (>= 2 rolling_glacier casts).
#
# GEOMETRY (why the default params close the chain — arena_flat, floor top Y=0):
#   - caster (observer-0, slot 0) stands at (-4, 0.59, 0) facing +X (ARENA_CAM_YAW=-pi/2); eye at
#     Y ~ 1.09 (body center 0.59 + ARENA_EYE_HEIGHT 0.5); it does NOT move (NO ARENA_AUTOMOVE) so
#     the trail geometry stays deterministic across pulses.
#   - rolling_glacier lobs from the casting HAND (~(-3.75, 1.14, 0.32)) at aim(yaw, pitch=-0.35),
#     speed 9 (tap charge ~1.0x): the ball grounds near x ~ -2.0 (z ~ 0.32) and the roll paints
#     `frost` every 0.8 m of +X travel for up to 6.5 s — a long trail down the +X line.
#   - frost_spire's GroundPoint eye-ray (from the eye, z=0) grounds ~ x -1.0 (pitch -0.35 over the
#     ~1.09 m eye height) — ON the trail: nearest patch within XZ match tolerance (patch_radius
#     0.45 + SURFACE_MATCH_SLACK 0.3 = 0.75; the trail's z~0.32 hand-launch offset stays inside
#     it). Early spire pulses MAY fizzle (paid-nothing CastRejected, off any patch) before the
#     roll has laid enough trail — the gate demands >= 1 accepted spire ACROSS the session, not
#     per pulse. ARENA_TEST_PITCH pitches EVERY pulse (the ball lobs down too, landing/trailing
#     sooner) — tune it against real `surface_painted` pos values, never the assertions.
#
# PULSE TIMING (why the default DURATION is 24, not 14 — tuned against real runs): autocast pulses
# the rotation every ARENA_AUTOCAST_PERIOD (1.2s), but the tap->cast edge DROPS ~50% of the time
# around the lobby->match transition + 3s countdown + mid-cast windows (sub-tick input jitter). With
# the alternation even-pulse=rolling_glacier / odd-pulse=frost_spire, the trail is only laid once the
# first rolling_glacier edge SURVIVES (~+6 s), and only the FEW frost_spire pulses AFTER that can hit
# it. At duration 14 those were just p5/p7/p9 — a run that dropped all three accepted zero spires and
# FAILED (~50% flake). At 24 s the session spans a round reset (the roll clips the stationary target
# for a kill ~once) into a SECOND round of cycles, so several more frost_spire-after-trail pulses fire
# — 10/10 tuning runs accepted 3 spires (consumed 3, all ground-flush). The frost trail persists
# 180 s, so every later frost_spire that lands on it succeeds regardless of the drops. Tune the
# SESSION (period/duration), NEVER the assertions.
#
# NO summarize.py twin: this shell has no python3 (see the arena net-test memory) and, more to the
# point, summarize.py encodes the FIREBOLT contract — running it here would assert the wrong gate.
# The jq check_glacier_session.sh IS this session's contract; this script does not call summarize.
#
# Usage:  bash crates/arena_game/tools/net-test/run_glacier_session.sh [session_dir]
#           then: bash crates/arena_game/tools/net-test/check_glacier_session.sh [session_dir]
#   session_dir defaults to /tmp/arena-glacier-test.
#
# Env overrides:
#   ARENA_NET_TEST_DURATION   seconds the casting observer runs (default 24 — TUNED EMPIRICALLY, see
#                             the PULSE TIMING note below; 14 was flaky at ~50%).
#   ARENA_MATCH_SEED          combat RNG seed (default 42 — deterministic).
#   ARENA_SKIP_BUILD=1        skip the cargo build (reuse the pre-built target/debug binaries).

set -uo pipefail

# Resolve repo root from this script's location: tools/net-test → crates/arena_game → root.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../../.." && pwd)"
cd "$repo_root"

session_dir="${1:-/tmp/arena-glacier-test}"
duration="${ARENA_NET_TEST_DURATION:-24}"
seed="${ARENA_MATCH_SEED:-42}"

rm -rf "$session_dir"
mkdir -p "$session_dir"

server_bin="$repo_root/target/debug/arena-server"
client_bin="$repo_root/target/debug/arena-client"

# Build the bins unless told to reuse the pre-built ones. The observer bin IS the arena-client's
# ARENA_HEADLESS mode (src/bin/observer.rs → client::run_headless_client), so building arena-client
# + arena-server is sufficient; build arena-observer too so its alias stays in sync.
if [[ "${ARENA_SKIP_BUILD:-0}" != "1" ]]; then
    echo "[arena-glacier-test] building binaries (set ARENA_SKIP_BUILD=1 to reuse pre-built)…" >&2
    if ! cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer \
        >"$session_dir/build.log" 2>&1; then
        echo "[arena-glacier-test] build FAILED — see $session_dir/build.log" >&2
        tail -30 "$session_dir/build.log" >&2
        exit 1
    fi
fi
# Prefer arena-observer if it built; fall back to arena-client ARENA_HEADLESS (same code path).
observer_bin="$repo_root/target/debug/arena-observer"
if [[ ! -x "$observer_bin" ]]; then
    observer_bin="$client_bin"
fi

if [[ ! -x "$server_bin" ]]; then
    echo "[arena-glacier-test] missing $server_bin (build first, or unset ARENA_SKIP_BUILD)" >&2
    exit 1
fi

server_pid=""
obs0_pid=""
obs1_pid=""
cleanup() {
    for pid in "$obs0_pid" "$obs1_pid" "$server_pid"; do
        [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT INT TERM

# --- the whole session runs inside ONE backgrounded block using real `sleep` (foreground sleep is
#     blocked in this harness environment); the block sequences server→observers→drain→teardown. ---
{
    # Silence bash job-control "Terminated: 15" notices for the children we kill on teardown.
    set +m
    # Server (authority). Bind first; pin the match seed for deterministic damage.
    ARENA_TRACE_FILE="$session_dir/server.jsonl" ARENA_TRACE_SRC="server" \
    ARENA_MATCH_SEED="$seed" RUST_LOG="${RUST_LOG:-warn}" \
        "$server_bin" >"$session_dir/server.log" 2>&1 &
    server_pid=$!
    echo "$server_pid" >"$session_dir/server.pid"

    # Let the server bind its UDP socket + load the cast timeline before clients connect.
    sleep 2

    if ! kill -0 "$server_pid" 2>/dev/null; then
        echo "[arena-glacier-test] server failed to start — see $session_dir/server.log" >&2
        exit 1
    fi

    # observer-0: the CASTER (STATIONARY). ARENA_CLIENT_ID=1 → server slot 0 → obelisk_id "player_1".
    # ARENA_AUTOEQUIP=potted_spring: swap off the starter to the glacier weapon (rolling_glacier
    #   slot 0, frost_spire slot 1). Lobby-gated server-side; ARENA_AUTOEQUIP also PACES the host's
    #   ARENA_AUTOSTART (the host holds its start until its own equip round-trips), so it is set on
    #   BOTH observers (whichever the server elected host must pace its own equip).
    # ARENA_AUTOCAST_SKILL rotates one entry per pulse: rolling_glacier, then frost_spire, then …
    # ARENA_AUTOCAST_PERIOD 1.2: slow enough that a roll lays trail before the next spire pulse.
    # ARENA_TEST_PITCH -0.35: autocast stamps yaw+pitch onto the input stream itself, so the caster
    #   aims correctly WITHOUT ARENA_AUTOMOVE — and a STATIONARY caster keeps the trail (and the
    #   spire's ground point) deterministic. See the GEOMETRY block above for the landing math.
    # ARENA_CAM_YAW -pi/2: the look direction rot(-Z) = (1,0,0) — the caster faces +X (toward slot 1).
    # ARENA_AUTOSTART_LEVEL (BOTH observers): the elected host requests arena_flat once both players
    #   stand in the lobby (AND its own equip has round-tripped); the non-host's hook no-ops.
    ARENA_HEADLESS=1 ARENA_CLIENT_ID=1 ARENA_AUTOCAST=1 ARENA_AUTOCAST_PERIOD=1.2 \
    ARENA_AUTOCAST_SKILL="rolling_glacier,frost_spire" \
    ARENA_AUTOEQUIP=potted_spring \
    ARENA_AUTOSTART_LEVEL=arena_flat \
    ARENA_CAM_YAW="-1.5707963" ARENA_TEST_PITCH="-0.35" \
    ARENA_TRACE_FILE="$session_dir/observer-0.jsonl" ARENA_TRACE_SRC="observer-0" \
    ARENA_MATCH_SEED="$seed" RUST_LOG="${RUST_LOG:-warn}" \
        "$observer_bin" >"$session_dir/observer-0.log" 2>&1 &
    obs0_pid=$!
    echo "$obs0_pid" >"$session_dir/observer-0.pid"

    # observer-1: the TARGET. ARENA_CLIENT_ID=2 → server slot 1 → obelisk_id "player_2".
    # No AUTOCAST — it connects, receives the replicated frost patches, and echoes them. It gets
    # ARENA_AUTOEQUIP=potted_spring too (the equip-pacing rule above — either peer may be host).
    ARENA_HEADLESS=1 ARENA_CLIENT_ID=2 \
    ARENA_AUTOEQUIP=potted_spring \
    ARENA_AUTOSTART_LEVEL=arena_flat \
    ARENA_TRACE_FILE="$session_dir/observer-1.jsonl" ARENA_TRACE_SRC="observer-1" \
    ARENA_MATCH_SEED="$seed" RUST_LOG="${RUST_LOG:-warn}" \
        "$observer_bin" >"$session_dir/observer-1.log" 2>&1 &
    obs1_pid=$!
    echo "$obs1_pid" >"$session_dir/observer-1.pid"

    # Run the session long enough for several lob→land→roll→trail→spire cycles to paint, replicate,
    # gate, consume, and erupt on both peers.
    sleep "$duration"

    # Tear down: observers first, then the server. `wait` each killed pid explicitly so this
    # subshell reaps them quietly (avoids the parent's job-control "Terminated: 15" notices).
    kill "$obs0_pid" "$obs1_pid" 2>/dev/null || true
    sleep 1
    kill "$server_pid" 2>/dev/null || true
    sleep 1
    for pid in "$obs0_pid" "$obs1_pid" "$server_pid"; do
        wait "$pid" 2>/dev/null || true
    done
} &
session_pid=$!
# Block until the session subshell finishes (it runs the real `sleep`s + teardown internally).
wait "$session_pid" 2>/dev/null || true

echo
echo "[arena-glacier-test] session written: $session_dir" >&2
echo "[arena-glacier-test] gate it with: bash $script_dir/check_glacier_session.sh $session_dir" >&2
