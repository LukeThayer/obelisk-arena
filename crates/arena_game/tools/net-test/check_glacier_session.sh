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
#   (8) D9: a Countdown->Active round reset CLEARED painted ground end-to-end (>= 1 surfaces_reset_cleared
#       — the reset's patch-clear loop wiped the frost; see server/rounds.rs::run_round_machine). NOTE:
#       satisfiable by the match-start clear ALONE (needs_round_reset is set on EVERY Countdown->Active
#       edge, including initial match start) — the death cycle is (9)'s job to pin.
#   (9) someone actually died (>= 1 server_net_entity_died) — pins that (8)'s reset-clear came from a
#       death cycle, not just match start.
#  (10) the rolling boulder actually spawned (>= 2 skill_object_spawned{glacier_ball} — the roll's
#       visible physical companion; server/verbs.rs spawns one per roll window in lockstep with it).
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
# (8) D9: a Countdown->Active round reset cleared the painted ground end-to-end (match-start OR death cycle).
reset_clears=$(jq -s '[.[] | select(.kind=="surfaces_reset_cleared")] | length' "$server")
[[ "$reset_clears" -ge 1 ]] || note "no round reset cleared surfaces (the reset's patch-clear loop wiped nothing)"
# (9) someone actually died — pins that (8)'s reset-clear came from a death cycle, not just match start.
deaths=$(jq -s '[.[] | select(.kind=="server_net_entity_died")] | length' "$server")
[[ "$deaths" -ge 1 ]] || note "no entity died ((8) alone is satisfiable by the match-start clear)"
# (10) the rolling boulder spawned (>= 2 glacier_ball skill objects — the roll's visible companion).
balls=$(jq -s '[.[] | select(.kind=="skill_object_spawned" and .object_kind=="glacier_ball")] | length' "$server")
[[ "$balls" -ge 2 ]] || note "no rolling boulder spawned (glacier_ball skill objects missing)"

echo "caster=$caster target=$target frost_paints=$paints spire_accepted=$accepted consumed=$consumed spires=$ok off_ground=$bad damage=$damage reset_clears=$reset_clears deaths=$deaths balls=$balls"
if [[ "$fail" -ne 0 ]]; then echo FAIL; exit 1; fi
echo PASS
