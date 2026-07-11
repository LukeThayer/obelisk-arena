#!/usr/bin/env bash
# The GLACIER gate (surfaces-nettest Task 3) — the jq contract for a session produced by
# run_glacier_session.sh. Same skeleton as check_session.sh (the player-resolution block is
# verbatim), asserting the obelisk SURFACES chain paint -> gate -> consume -> ground-flush erupt:
#   (1) the roll painted the frost trail (server) ...
#   (2) ... and it replicated to both observers.
#   (3) a frost_spire cast was ACCEPTED (the on_surface gate matched a trail patch).
#   (4) the accepted cast CONSUMED its fuel patch.
#   (5) the spire erupted GROUND-FLUSH (anchor |y| <= 0.25 — the Task-4 float regression's e2e pin).
#   (6) the glacier chain ran (>= 2 rolling_glacier casts).
#
# This jq script IS the contract: there is NO summarize.py twin for the glacier gate (this shell is
# python3-free, and summarize.py encodes the firebolt contract). Keep the assertions; tune only the
# SESSION parameters (run_glacier_session.sh's pitch/period/duration) until they pass.
#
# Usage: check_glacier_session.sh [session_dir]   (default /tmp/arena-glacier-test)
set -uo pipefail
session="${1:-/tmp/arena-glacier-test}"
fail=0
note() { echo "  - $*"; fail=1; }

server="$session/server.jsonl"; obs0="$session/observer-0.jsonl"; obs1="$session/observer-1.jsonl"
for f in "$server" "$obs0" "$obs1"; do
    [[ -s "$f" ]] || { echo "missing/empty trace: $f"; echo FAIL; exit 1; }
done

caster=$(jq -rs '[.[] | select(.kind=="player_spawned" and .client_id==1)][0].obelisk_id // empty' "$server")
target=$(jq -rs '[.[] | select(.kind=="player_spawned" and .client_id==2)][0].obelisk_id // empty' "$server")
[[ -n "$caster" && -n "$target" ]] || note "server did not spawn both players (caster='$caster' target='$target')"

# (1) the roll painted the frost trail (server) ...
n=$(jq -s '[.[] | select(.kind=="surface_painted" and .surface=="frost")] | length' "$server")
[[ "$n" -ge 3 ]] || note "server painted fewer than 3 frost patches (trail)"
# (2) ... and it replicated to both observers.
for name in observer-0 observer-1; do
    f="$session/$name.jsonl"
    n=$(jq -s '[.[] | select(.kind=="replicated_surface_patch" and .surface=="frost")] | length' "$f")
    [[ "$n" -ge 3 ]] || note "$name received fewer than 3 replicated frost patches"
done
# (3) a frost_spire cast was ACCEPTED (the on_surface gate matched a trail patch).
n=$(jq -s --arg c "$caster" '[.[] | select(.kind=="server_net_cast_began" and .skill_id=="frost_spire" and .caster==$c)] | length' "$server")
[[ "$n" -ge 1 ]] || note "no frost_spire cast was accepted (gate never matched the trail)"
# (4) the accepted cast CONSUMED its fuel patch.
n=$(jq -s '[.[] | select(.kind=="surface_removed" and .surface=="frost" and .reason=="Consumed")] | length' "$server")
[[ "$n" -ge 1 ]] || note "no frost patch was consumed"
# (5) the spire erupted GROUND-FLUSH (anchor y ~ 0 — the Task-4 regression's e2e pin).
bad=$(jq -s '[.[] | select(.kind=="spire_erupted") | select((.pos[1] > 0.25) or (.pos[1] < -0.25))] | length' "$server")
ok=$(jq -s '[.[] | select(.kind=="spire_erupted")] | length' "$server")
[[ "$ok" -ge 1 ]] || note "no spire_erupted trace"
[[ "$bad" -eq 0 ]] || note "$bad spire eruption(s) anchored off the ground (|y| > 0.25)"
# (6) the glacier chain's trigger causality fired (roll damage or burst — proves the chain ran).
n=$(jq -s --arg c "$caster" '[.[] | select(.kind=="server_net_cast_began" and .skill_id=="rolling_glacier" and .caster==$c)] | length' "$server")
[[ "$n" -ge 2 ]] || note "fewer than 2 rolling_glacier casts"

echo "caster=$caster target=$target"
if [[ "$fail" -ne 0 ]]; then echo FAIL; exit 1; fi
echo PASS
