#!/usr/bin/env bash
# Conditioned net gate (design WS6): the standard session under ~100ms RTT + jitter + loss.
# Both observers get 50ms incoming latency (≈100ms RTT vs the local server), 10ms jitter, 2% loss.
# The M2 gate assertions must still hold — casts ride the redundant input stream, events ride
# reliable channels, so nothing may be LOST (only delayed). Knobs override via the same env vars.
set -uo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
session="${1:-/tmp/arena-net-conditioned}"
export ARENA_NET_LATENCY_MS="${ARENA_NET_LATENCY_MS:-50}"
export ARENA_NET_JITTER_MS="${ARENA_NET_JITTER_MS:-10}"
export ARENA_NET_LOSS="${ARENA_NET_LOSS:-0.02}"
# Longer session: the added RTT delays the first cast/damage round-trips.
export ARENA_NET_TEST_DURATION="${ARENA_NET_TEST_DURATION:-12}"
bash "$script_dir/run_session.sh" "$session" || true
bash "$script_dir/check_session.sh" "$session"
