#!/usr/bin/env bash
# jq mirror of summarize.py (python3 is unavailable in some dev shells). Asserts the M2 gate over
# a finished session dir produced by run_session.sh. Usage: check_session.sh [session_dir]
set -uo pipefail
session="${1:-/tmp/arena-net-test}"
fail=0
note() { echo "  - $*"; fail=1; }

server="$session/server.jsonl"; obs0="$session/observer-0.jsonl"; obs1="$session/observer-1.jsonl"
for f in "$server" "$obs0" "$obs1"; do
    [[ -s "$f" ]] || { echo "missing/empty trace: $f"; echo FAIL; exit 1; }
done

caster=$(jq -rs '[.[] | select(.kind=="player_spawned" and .client_id==1)][0].obelisk_id // empty' "$server")
target=$(jq -rs '[.[] | select(.kind=="player_spawned" and .client_id==2)][0].obelisk_id // empty' "$server")
[[ -n "$caster" && -n "$target" ]] || note "server did not spawn both players (caster='$caster' target='$target')"

# (1) server CastBegan(caster, firebolt)
n=$(jq -s --arg c "$caster" '[.[] | select(.kind=="server_net_cast_began" and .skill_id=="firebolt" and .caster==$c)] | length' "$server")
[[ "$n" -ge 1 ]] || note "server emitted no CastBegan(caster=$caster, firebolt)"

# (2) server DamageResolved(caster->target, firebolt) + capture the authoritative value
dmg=$(jq -rs --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="server_net_damage_resolved" and .skill_id=="firebolt" and .caster==$c and .target==$t)][0].total_damage // empty' "$server")
[[ -n "$dmg" ]] || note "server emitted no DamageResolved(caster=$caster, target=$target, firebolt)"

# (5) firebolt_explosion trigger fired end-to-end on the server
n=$(jq -s --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="server_net_damage_resolved" and .skill_id=="firebolt_explosion" and .caster==$c and .target==$t)] | length' "$server")
[[ "$n" -ge 1 ]] || note "server emitted no DamageResolved(firebolt_explosion)"

# (7) surfaces: the explosion scorches the ground (server paints burning)...
n=$(jq -s '[.[] | select(.kind=="surface_painted" and .surface=="burning")] | length' "$server")
[[ "$n" -ge 1 ]] || note "server painted no burning surface (firebolt_explosion paints OnEnd)"

# (3)+(4)+(6) per observer: echoed cast + damage (matching value) + explosion
for name in observer-0 observer-1; do
    f="$session/$name.jsonl"
    n=$(jq -s --arg c "$caster" '[.[] | select(.kind=="client_net_cast_began" and .skill_id=="firebolt" and .caster==$c)] | length' "$f")
    [[ "$n" -ge 1 ]] || note "$name received no CastBegan"
    echoed=$(jq -rs --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="client_net_damage_resolved" and .skill_id=="firebolt" and .caster==$c and .target==$t)][0].total_damage // empty' "$f")
    [[ -n "$echoed" ]] || note "$name received no DamageResolved"
    if [[ -n "$echoed" && -n "$dmg" && "$echoed" != "$dmg" ]]; then
        note "$name echoed damage $echoed != server's $dmg"
    fi
    n=$(jq -s --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="client_net_damage_resolved" and .skill_id=="firebolt_explosion" and .caster==$c and .target==$t)] | length' "$f")
    [[ "$n" -ge 1 ]] || note "$name received no DamageResolved(firebolt_explosion)"

    # (8) ...and the patch replicated to this observer.
    n=$(jq -s '[.[] | select(.kind=="replicated_surface_patch" and .surface=="burning")] | length' "$f")
    [[ "$n" -ge 1 ]] || note "$name received no replicated burning surface patch"
done

echo "caster=$caster target=$target server_damage=$dmg"
if [[ "$fail" -ne 0 ]]; then echo FAIL; exit 1; fi
echo PASS
