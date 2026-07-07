#!/usr/bin/env bash
# obelisk-arena net regression (M2.5 Task 20, netcode guide §8.3).
#
# The objective HEADLESS M2 gate: launch the dedicated `arena-server` + two headless cast clients
# (`arena-observer`, distinct trace srcs/files), script observer-0 to cast firebolt at observer-1,
# then ASSERT over the merged per-process JSONL traces (via summarize.py) that:
#   - the server emits CastBegan(caster=client-0) + DamageResolved(caster=client-0, target=client-1);
#   - BOTH observers receive a CastBegan AND a DamageResolved for that cast;
#   - the damage value both observers echo MATCHES the server's authoritative number.
#
# Exits 0 / prints PASS iff every assertion holds. This is fully headless (no window).
#
# Usage:  bash crates/arena_game/tools/net-test/run_session.sh [session_dir]
#   session_dir defaults to /tmp/arena-net-test.
#
# Env overrides:
#   ARENA_NET_TEST_DURATION   seconds the casting observer runs (default 8 — ≥2 firebolt casts).
#   ARENA_MATCH_SEED          combat RNG seed (default 42 — deterministic damage).
#   ARENA_SKIP_BUILD=1        skip the cargo build (use the pre-built target/debug binaries).

set -uo pipefail

# Resolve repo root from this script's location: tools/net-test → crates/arena_game → root.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../../.." && pwd)"
cd "$repo_root"

session_dir="${1:-/tmp/arena-net-test}"
duration="${ARENA_NET_TEST_DURATION:-8}"
seed="${ARENA_MATCH_SEED:-42}"

rm -rf "$session_dir"
mkdir -p "$session_dir"

server_bin="$repo_root/target/debug/arena-server"
client_bin="$repo_root/target/debug/arena-client"

# Build the bins unless told to reuse the pre-built ones. The observer bin IS the arena-client's
# ARENA_HEADLESS mode (src/bin/observer.rs → client::run_headless_client), so building arena-client
# + arena-server is sufficient; build arena-observer too so its alias stays in sync.
if [[ "${ARENA_SKIP_BUILD:-0}" != "1" ]]; then
    echo "[arena-net-test] building binaries (set ARENA_SKIP_BUILD=1 to reuse pre-built)…" >&2
    if ! cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer \
        >"$session_dir/build.log" 2>&1; then
        echo "[arena-net-test] build FAILED — see $session_dir/build.log" >&2
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
    echo "[arena-net-test] missing $server_bin (build first, or unset ARENA_SKIP_BUILD)" >&2
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
        echo "[arena-net-test] server failed to start — see $session_dir/server.log" >&2
        exit 1
    fi

    # observer-0: the CASTER + MOVER. ARENA_CLIENT_ID=1 → server slot 0 → obelisk_id "player_1".
    # ARENA_AUTOCAST scripts a firebolt cast (over the wire) once it owns a player + sees an opponent.
    # ARENA_AUTOSTART_LEVEL (BOTH observers carry it): whichever peer the server elected HOST (first
    #   completed handshake — launch order does NOT guarantee it) requests the arena_flat match as
    #   soon as both players stand in the lobby; the non-host's hook no-ops (host-gated in code).
    #   The harness's stand-in for pressing G — the lobby never auto-starts on player count.
    # ARENA_AUTOMOVE drives the predicted force controller forward ALONG the aim axis (CameraYaw), so
    #   the caster walks toward the target while firing — its avian Position changes (movement-
    #   replication check) yet it stays on the firing line, so the cast still lands.
    # ARENA_CAM_YAW: slot-0 spawns at (-4,1,0), slot-1 at (4,1,0).  The look direction
    #   rot(-Z) = (1,0,0) requires yaw = -π/2 ≈ -1.5707963 so the bolt flies along +X toward slot-1.
    ARENA_HEADLESS=1 ARENA_CLIENT_ID=1 ARENA_AUTOCAST=1 ARENA_AUTOCAST_PERIOD=0.8 \
    ARENA_AUTOMOVE=1 \
    ARENA_AUTOSTART_LEVEL=arena_flat \
    ARENA_CAM_YAW="-1.5707963" \
    ARENA_TRACE_FILE="$session_dir/observer-0.jsonl" ARENA_TRACE_SRC="observer-0" \
    ARENA_MATCH_SEED="$seed" RUST_LOG="${RUST_LOG:-warn}" \
        "$observer_bin" >"$session_dir/observer-0.log" 2>&1 &
    obs0_pid=$!
    echo "$obs0_pid" >"$session_dir/observer-0.pid"

    # observer-1: the TARGET. ARENA_CLIENT_ID=2 → server slot 1 → obelisk_id "player_2".
    # No AUTOCAST — it connects, gets bodied, and echoes the replicated cast + damage it receives.
    # ARENA_AUTOSTART_LEVEL: see observer-0 — the elected host (either peer) starts the match.
    ARENA_HEADLESS=1 ARENA_CLIENT_ID=2 \
    ARENA_AUTOSTART_LEVEL=arena_flat \
    ARENA_TRACE_FILE="$session_dir/observer-1.jsonl" ARENA_TRACE_SRC="observer-1" \
    ARENA_MATCH_SEED="$seed" RUST_LOG="${RUST_LOG:-warn}" \
        "$observer_bin" >"$session_dir/observer-1.log" 2>&1 &
    obs1_pid=$!
    echo "$obs1_pid" >"$session_dir/observer-1.pid"

    # Run the duel long enough for ≥2 scripted casts + their replicated damage to land on both peers.
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
# bash may print cosmetic job-control "Terminated: 15" notices to stderr when the SIGTERM'd children
# are reaped — those are harmless (the verdict is on stdout, exit code is from summarize.py below);
# the meaningful per-process diagnostics are in the session dir's *.log files.
wait "$session_pid" 2>/dev/null || true

# --- assert + summarize (Python — jq is unavailable in this environment) ----------------------
echo
python3 "$script_dir/summarize.py" "$session_dir"
status=$?

if [[ $status -eq 0 ]]; then
    echo "[arena-net-test] session PASS: $session_dir" >&2
else
    echo "[arena-net-test] session FAIL: $session_dir (see *.log + *.jsonl)" >&2
fi
exit $status
