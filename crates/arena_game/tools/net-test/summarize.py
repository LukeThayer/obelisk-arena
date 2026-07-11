#!/usr/bin/env python3
"""Summarize + ASSERT the obelisk-arena M2 net regression (M2.5 Task 20, guide §8.3).

Reads the per-process JSONL traces in a session dir (server.jsonl, observer-0.jsonl,
observer-1.jsonl), merges them, prints a compact summary, and checks the M2 gate assertions:

  1. server emits CastBegan(caster=client-0's obelisk_id, skill=firebolt)
  2. server emits DamageResolved(caster=client-0's obelisk_id, target=client-1's obelisk_id,
     skill=firebolt)
  3. BOTH observers receive a CastBegan AND a DamageResolved for that cast
  4. the damage value BOTH observers echo MATCHES the server's authoritative number

The harness pins ARENA_CLIENT_ID so the obelisk ids are deterministic: client-0 → "player_1"
(slot 0, caster) and client-1 → "player_2" (slot 1, target). We resolve those ids from the
server's `player_spawned` events keyed by client_id rather than hard-coding, so the check stays
robust if the id scheme changes.

Exit 0 + print "PASS" iff every assertion holds; else print the failures + "FAIL" and exit 1.
This is the objective headless M2 gate.
"""

import collections
import json
import sys
from pathlib import Path


def load(path: Path):
    events = []
    if not path.exists():
        return events
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    return events


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: summarize.py <session_dir>", file=sys.stderr)
        return 2
    session = Path(sys.argv[1])

    server = load(session / "server.jsonl")
    obs0 = load(session / "observer-0.jsonl")
    obs1 = load(session / "observer-1.jsonl")

    # --- compact per-source event-kind summary ---
    print(f"session: {session}")
    for name, evs in (("server", server), ("observer-0", obs0), ("observer-1", obs1)):
        counts = collections.Counter(e.get("kind", "?") for e in evs)
        kinds = ", ".join(f"{k}={v}" for k, v in sorted(counts.items()))
        print(f"  [{name}] {len(evs)} events: {kinds}")

    failures = []

    # --- resolve the two clients' obelisk ids from server player_spawned (keyed by client_id) ---
    # The harness launches observer-0 with ARENA_CLIENT_ID=1 (caster) and observer-1 with
    # ARENA_CLIENT_ID=2 (target). Map client_id -> obelisk_id from the server's spawn events.
    spawned = {e["client_id"]: e["obelisk_id"]
               for e in server if e.get("kind") == "player_spawned" and "client_id" in e}
    caster_id = spawned.get(1)
    target_id = spawned.get(2)
    if caster_id is None or target_id is None:
        failures.append(
            f"server did not spawn both expected players (spawned={spawned})")

    # --- (1) server CastBegan(caster=client-0) ---
    server_casts = [e for e in server
                    if e.get("kind") == "server_net_cast_began"
                    and e.get("skill_id") == "firebolt"
                    and e.get("caster") == caster_id]
    if not server_casts:
        failures.append(
            f"server emitted no CastBegan(caster={caster_id}, skill=firebolt)")

    # --- (2) server DamageResolved(caster=client-0, target=client-1) ---
    server_dmgs = [e for e in server
                   if e.get("kind") == "server_net_damage_resolved"
                   and e.get("skill_id") == "firebolt"
                   and e.get("caster") == caster_id
                   and e.get("target") == target_id]
    if not server_dmgs:
        failures.append(
            f"server emitted no DamageResolved(caster={caster_id}, target={target_id}, "
            f"skill=firebolt)")
    server_dmg_value = server_dmgs[0]["total_damage"] if server_dmgs else None

    # --- (3) BOTH observers received a CastBegan AND a DamageResolved for that cast ---
    for name, evs in (("observer-0", obs0), ("observer-1", obs1)):
        got_cast = [e for e in evs
                    if e.get("kind") == "client_net_cast_began"
                    and e.get("skill_id") == "firebolt"
                    and e.get("caster") == caster_id]
        got_dmg = [e for e in evs
                   if e.get("kind") == "client_net_damage_resolved"
                   and e.get("skill_id") == "firebolt"
                   and e.get("caster") == caster_id
                   and e.get("target") == target_id]
        if not got_cast:
            failures.append(f"{name} received no CastBegan(caster={caster_id}, firebolt)")
        if not got_dmg:
            failures.append(
                f"{name} received no DamageResolved(caster={caster_id}, target={target_id})")

        # --- (4) the damage value the observer echoes matches the server's ---
        if got_dmg and server_dmg_value is not None:
            echoed = got_dmg[0]["total_damage"]
            if echoed != server_dmg_value:
                failures.append(
                    f"{name} echoed damage {echoed} != server's authoritative "
                    f"{server_dmg_value}")

    # --- (5) server DamageResolved(skill=firebolt_explosion) — firebolt's on-impact trigger fired
    #         end-to-end (a SEPARATE triggered sub-cast, distinct from the bolt's direct hit). ---
    server_expl = [e for e in server
                   if e.get("kind") == "server_net_damage_resolved"
                   and e.get("skill_id") == "firebolt_explosion"
                   and e.get("caster") == caster_id
                   and e.get("target") == target_id]
    if not server_expl:
        failures.append(
            f"server emitted no DamageResolved(caster={caster_id}, target={target_id}, "
            f"skill=firebolt_explosion) — firebolt's on-impact trigger did not fire")

    # --- (6) BOTH observers received the triggered explosion damage ---
    for name, evs in (("observer-0", obs0), ("observer-1", obs1)):
        got_expl = [e for e in evs
                    if e.get("kind") == "client_net_damage_resolved"
                    and e.get("skill_id") == "firebolt_explosion"
                    and e.get("caster") == caster_id
                    and e.get("target") == target_id]
        if not got_expl:
            failures.append(
                f"{name} received no DamageResolved(caster={caster_id}, "
                f"target={target_id}, skill=firebolt_explosion)")

    # --- (7) surfaces: the explosion scorches the ground — the server paints a `burning` patch
    #         (firebolt_explosion's blast window paints OnEnd; spec §4/§7). ---
    server_painted = [e for e in server
                      if e.get("kind") == "surface_painted"
                      and e.get("surface") == "burning"]
    if not server_painted:
        failures.append(
            "server painted no burning surface (firebolt_explosion paints OnEnd)")

    # --- (9) burning standing tick: the scorch under the target resolves burning_ground_tick
    #         damage (painter-attributed), proving the standing-payload path end-to-end. ---
    server_burn_tick = [e for e in server
                        if e.get("kind") == "server_net_damage_resolved"
                        and e.get("skill_id") == "burning_ground_tick"
                        and e.get("caster") == caster_id
                        and e.get("target") == target_id]
    if not server_burn_tick:
        failures.append("server resolved no burning_ground_tick standing damage")

    # --- (8) ...and the burning patch REPLICATED to BOTH observers (wire v6) ---
    for name, evs in (("observer-0", obs0), ("observer-1", obs1)):
        got_patch = [e for e in evs
                     if e.get("kind") == "replicated_surface_patch"
                     and e.get("surface") == "burning"]
        if not got_patch:
            failures.append(f"{name} received no replicated burning surface patch")

    # --- (10) ...and each observer echoes the standing-tick damage. ---
    for name, evs in (("observer-0", obs0), ("observer-1", obs1)):
        got_burn_tick = [e for e in evs
                         if e.get("kind") == "client_net_damage_resolved"
                         and e.get("skill_id") == "burning_ground_tick"
                         and e.get("caster") == caster_id
                         and e.get("target") == target_id]
        if not got_burn_tick:
            failures.append(f"{name} received no burning_ground_tick damage")

    # --- verdict ---
    print()
    print("M2 gate assertions:")
    print(f"  caster (client-0) obelisk_id = {caster_id}")
    print(f"  target (client-1) obelisk_id = {target_id}")
    print(f"  server CastBegan firebolt by caster:        {len(server_casts)} event(s)")
    print(f"  server DamageResolved caster->target:       {len(server_dmgs)} event(s)")
    print(f"  server authoritative damage value:          {server_dmg_value}")
    print(f"  server DamageResolved firebolt_explosion (trigger):  {len(server_expl)} event(s)")
    print(f"  server surface_painted burning (OnEnd):     {len(server_painted)} event(s)")
    print(f"  server burning_ground_tick (standing):      {len(server_burn_tick)} event(s)")
    for name, evs in (("observer-0", obs0), ("observer-1", obs1)):
        n_cast = sum(1 for e in evs if e.get("kind") == "client_net_cast_began"
                     and e.get("caster") == caster_id)
        n_dmg = sum(1 for e in evs if e.get("kind") == "client_net_damage_resolved"
                    and e.get("caster") == caster_id and e.get("target") == target_id)
        echoed = next((e["total_damage"] for e in evs
                       if e.get("kind") == "client_net_damage_resolved"
                       and e.get("caster") == caster_id), None)
        print(f"  {name}: CastBegan={n_cast}, DamageResolved={n_dmg}, echoed_damage={echoed}")

    print()
    if failures:
        print("FAILURES:")
        for f in failures:
            print(f"  - {f}")
        print("\nFAIL")
        return 1
    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
