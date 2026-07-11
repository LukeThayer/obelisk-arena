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
#   (7) the chain actually DAMAGED the target (>= 1 server_net_damage_resolved caster->target — guards
#       against a silently pacifist session: the roll must reach the stationary target, not just paint).
#   (8) D9: a mid-session round reset CLEARED painted ground (>= 1 surfaces_reset_cleared) — proof that
#       a death happened and the reset wiped the frost (see server/rounds.rs::run_round_machine).
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
paints=$(jq -s '[.[] | select(.kind=="surface_painted" and .surface=="frost")] | length' "$server")
[[ "$paints" -ge 3 ]] || note "server painted fewer than 3 frost patches (trail)"
# (2) ... and it replicated to both observers.
for name in observer-0 observer-1; do
    f="$session/$name.jsonl"
    n=$(jq -s '[.[] | select(.kind=="replicated_surface_patch" and .surface=="frost")] | length' "$f")
    [[ "$n" -ge 3 ]] || note "$name received fewer than 3 replicated frost patches"
done
# (3) a frost_spire cast was ACCEPTED (the on_surface gate matched a trail patch).
accepted=$(jq -s --arg c "$caster" '[.[] | select(.kind=="server_net_cast_began" and .skill_id=="frost_spire" and .caster==$c)] | length' "$server")
[[ "$accepted" -ge 1 ]] || note "no frost_spire cast was accepted (gate never matched the trail)"
# (4) the accepted cast CONSUMED its fuel patch.
consumed=$(jq -s '[.[] | select(.kind=="surface_removed" and .surface=="frost" and .reason=="Consumed")] | length' "$server")
[[ "$consumed" -ge 1 ]] || note "no frost patch was consumed"
# (5) the spire erupted GROUND-FLUSH (anchor y ~ 0 — the Task-4 regression's e2e pin).
bad=$(jq -s '[.[] | select(.kind=="spire_erupted") | select((.pos[1] > 0.25) or (.pos[1] < -0.25))] | length' "$server")
ok=$(jq -s '[.[] | select(.kind=="spire_erupted")] | length' "$server")
[[ "$ok" -ge 1 ]] || note "no spire_erupted trace"
[[ "$bad" -eq 0 ]] || note "$bad spire eruption(s) anchored off the ground (|y| > 0.25)"
# (6) the glacier chain's trigger causality fired (roll damage or burst — proves the chain ran).
n=$(jq -s --arg c "$caster" '[.[] | select(.kind=="server_net_cast_began" and .skill_id=="rolling_glacier" and .caster==$c)] | length' "$server")
[[ "$n" -ge 2 ]] || note "fewer than 2 rolling_glacier casts"
# (7) the glacier chain actually damages the target (guards against a silently pacifist session).
damage=$(jq -s --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="server_net_damage_resolved" and .caster==$c and .target==$t)] | length' "$server")
[[ "$damage" -ge 1 ]] || note "the glacier chain dealt no damage (session regressed to pacifist)"
# (8) D9: a mid-session round reset cleared the painted ground.
reset_clears=$(jq -s '[.[] | select(.kind=="surfaces_reset_cleared")] | length' "$server")
[[ "$reset_clears" -ge 1 ]] || note "no round reset cleared surfaces (D9 unproven — did anyone die?)"

echo "caster=$caster target=$target frost_paints=$paints spire_accepted=$accepted consumed=$consumed spires=$ok off_ground=$bad damage=$damage reset_clears=$reset_clears"
if [[ "$fail" -ne 0 ]]; then echo FAIL; exit 1; fi
echo PASS
