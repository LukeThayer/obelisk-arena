# Surfaces — Glacier/Spire E2E Net-Test + Ray Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the surfaces follow-ups doc's REQUIRED item #1 — automated e2e runtime coverage of
the glacier→spire chain (paint → gate → consume → ground-flush erupt) and the burning standing
tick — plus hardening item #6 (exclude combatants from the spire ground-snap ray). This is the
named coverage gap where the arena increment's one real bug (the spire Y-float) lived.

**Architecture:** Three surgical pieces in obelisk-arena, branch **`surfaces-nettest`** off
`master`: (1) burning-tick assertions bolt onto the EXISTING firebolt gate (the session already
scorches the target — firebolt → explosion → paints burning → target stands in it →
`burning_ground_tick` resolves); (2) the spire ray gains combatant exclusions in
`server/verbs.rs`; (3) a NEW glacier session script + jq/python checks scripts the chain with
`ARENA_AUTOEQUIP` + `ARENA_AUTOCAST_SKILL` rotation and asserts paint/replication/consume/
ground-flush end-to-end. Follow-ups doc: `docs/superpowers/plans/2026-07-10-surfaces-followups.md`.

**Tech Stack:** bash + jq (the local gate; python3 UNAVAILABLE locally — summarize.py edited in
lockstep but exercised only by CI), the arena harness env knobs (`ARENA_AUTOCAST_SKILL` comma
rotation, `ARENA_AUTOEQUIP` on EVERY observer, `ARENA_TEST_PITCH`, no AUTOMOVE for the glacier
session), avian `SpatialQueryFilter` exclusions.

## Global Constraints

- Repo `~/src/obelisk-arena`, branch **`surfaces-nettest`** off `master` (create in Task 1).
- **NEVER `git add -A` / `git add .`** — the tree carries USER WIP (`assets/skills/blizzard.cast.ron`,
  modified `assets/vfx/*.vfx.ron`, untracked `assets/vfx/Particle_ *.vfx.ron`). Stage exact paths.
- Process hygiene: `pkill -x arena-server; pkill -x arena-client; pkill -x arena-observer` (exact
  names). The harness is wall-clock flaky — retry a failing session up to 3×; ONE PASS is green.
- `run_session.sh`'s trailing `python3 summarize.py` exits 127 in this shell — that is EXPECTED;
  the LOCAL verdict is `check_session.sh` (jq), run separately. Same split for the new glacier
  scripts.
- The FIREBOLT gate must stay green throughout (`run_session.sh` + `check_session.sh`).
- Session geometry facts (from `run_session.sh` + `server/spawn.rs`): slot-0 caster at (-4,1,0),
  slot-1 target at (4,1,0), yaw `-1.5707963` aims +X; `ARENA_AUTOCAST_PERIOD` default 0.8;
  `ARENA_AUTOCAST_SKILL` rotates a comma list one entry per pulse; `ARENA_AUTOEQUIP=<item_id>`
  must be set on EVERY observer (equips are Lobby-gated; AUTOEQUIP paces AUTOSTART);
  `ARENA_TEST_PITCH` stamps the aim pitch for ALL autocasts.
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

## File Structure

- Modify: `crates/arena_game/tools/net-test/check_session.sh`, `tools/net-test/summarize.py`
  (Task 1); `crates/arena_game/src/server/verbs.rs` (Task 2); `crates/arena_game/CLAUDE.md` +
  `docs/superpowers/plans/2026-07-10-surfaces-followups.md` (Task 3).
- Create: `crates/arena_game/tools/net-test/run_glacier_session.sh`,
  `crates/arena_game/tools/net-test/check_glacier_session.sh` (Task 3).

---

### Task 1: Burning standing-tick assertions on the existing firebolt gate

**Files:**
- Modify: `crates/arena_game/tools/net-test/check_session.sh`,
  `crates/arena_game/tools/net-test/summarize.py`

**Why this works with ZERO session changes:** observer-0's firebolts hit observer-1 →
`firebolt_explosion` executes at the hit and `paints burning OnEnd` (radius 1.2, 4s) → the
stationary target stands inside → `apply_standing_payloads` executes `burning_ground_tick`
against it every 0.5s, attributed to the painter (player_1) — so the server emits
`server_net_damage_resolved` with `skill_id=="burning_ground_tick"`, `caster==$caster`,
`target==$target`, and both observers echo `client_net_damage_resolved` for it.

- [ ] **Step 0: Branch**

```bash
cd ~/src/obelisk-arena && git checkout -b surfaces-nettest
```

- [ ] **Step 1: Prove the signal exists BEFORE asserting (the TDD analogue for harness work)**

```bash
pkill -x arena-server; pkill -x arena-client; pkill -x arena-observer; sleep 1
bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-burning-probe
jq -s '[.[] | select(.kind=="server_net_damage_resolved" and .skill_id=="burning_ground_tick")] | length' /tmp/arena-burning-probe/server.jsonl
jq -s '[.[] | select(.kind=="client_net_damage_resolved" and .skill_id=="burning_ground_tick")] | length' /tmp/arena-burning-probe/observer-0.jsonl /tmp/arena-burning-probe/observer-1.jsonl
```
Expected: nonzero counts on all three. If the server count is ZERO, STOP and report
DONE_WITH_CONCERNS with the trace evidence (the assertion below would institutionalize a flake) —
diagnose first (is `surface_painted burning` present? is the patch under the target?). Do not
proceed to Step 2 with a zero.

- [ ] **Step 2: Add the assertions**

`check_session.sh` — after the existing surface checks (the `(7)` server block ~line 32), add:
```bash
# (9) burning standing tick: the scorch under the target resolves burning_ground_tick damage
#     (painter-attributed), proving the standing-payload path end-to-end.
n=$(jq -s --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="server_net_damage_resolved" and .skill_id=="burning_ground_tick" and .caster==$c and .target==$t)] | length' "$server")
[[ "$n" -ge 1 ]] || note "server resolved no burning_ground_tick standing damage"
```
and inside the per-observer loop (after the `(8)` replicated-patch check):
```bash
    # (10) ...and each observer echoes the standing-tick damage.
    n=$(jq -s --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="client_net_damage_resolved" and .skill_id=="burning_ground_tick" and .caster==$c and .target==$t)] | length' "$f")
    [[ "$n" -ge 1 ]] || note "$name received no burning_ground_tick damage"
```
`summarize.py` — mirror both, following the existing surface-assertion blocks 1:1 (same
kind/skill/caster/target filters, failure strings matching the shell's phrasing verbatim).

- [ ] **Step 3: Run the gate green + commit**

```bash
pkill -x arena-server; pkill -x arena-client; pkill -x arena-observer; sleep 1
bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-test
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-test   # expect PASS
git add crates/arena_game/tools/net-test/check_session.sh crates/arena_game/tools/net-test/summarize.py
git commit -m "test(surfaces): firebolt gate asserts the burning standing tick end-to-end

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Spire ground-snap ray excludes combatants (follow-up #6)

**Files:**
- Modify: `crates/arena_game/src/server/verbs.rs`

**The issue:** the eruption anchor's downward ray uses `SpatialQueryFilter::default()` — a body
standing on the fuel patch at cast time can be hit instead of the floor, floating the VISUAL
spire (the damage capsule is CastPoint-anchored, unaffected). The old poller had the same quirk
along a roll path; at a cast target the odds are materially higher (the spire is aimed AT enemies).

- [ ] **Step 1: Implement the exclusion**

In `skill_verbs_on_cue`'s `("frost_spire", "on_window_spike")` arm: build the exclusion set the
same way the PORTAL arm does (it already collects `exclude` = caster + its `children` +
`objects.iter()` skill objects — read that block, ~verbs.rs:121-125, and mirror it). For the
spire ray, exclude: the CASTER (`ev.source`) + its children (hurtbox) + ALL skill objects + —
the material addition — every combatant body: add a query param
`players: Query<Entity, With<crate::net::protocol::NetworkedPlayer>>` to `skill_verbs_on_cue`
and extend the exclusion with `players.iter()` AND each player's `children`. Replace the ray's
`&avian3d::prelude::SpatialQueryFilter::default()` with
`&avian3d::prelude::SpatialQueryFilter::default().with_excluded_entities(exclude)`.
Update the arm's comment: the ray wants LEVEL geometry only (floor/spire-terrain); combatants
standing on the fuel patch must not float the spire. Keep the miss-fallback unchanged
(`spire_eruption_anchor`'s `cue_pos.y - 0.8`).

- [ ] **Step 2: Verify + commit**

```bash
cargo test -p arena_game 2>&1 | tail -4   # verbs unit tests + everything green
cargo check -p arena_game --all-targets 2>&1 | grep -cE "^error"   # 0
git add crates/arena_game/src/server/verbs.rs
git commit -m "fix(surfaces): spire ground-snap ray excludes combatants and skill objects

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```
(The pure-math `spire_eruption_anchor` tests are unaffected — the exclusion changes which hit
feeds the helper, not the math. The e2e proof is Task 3's ground-flush assertion with the target
standing in the trail area.)

---

### Task 3: The glacier session — paint → gate → consume → ground-flush erupt over the wire

**Files:**
- Create: `crates/arena_game/tools/net-test/run_glacier_session.sh`,
  `crates/arena_game/tools/net-test/check_glacier_session.sh`
- Modify: `crates/arena_game/CLAUDE.md`, `docs/superpowers/plans/2026-07-10-surfaces-followups.md`

**Session design (starting parameters — TUNE EMPIRICALLY against real runs, that is this task's
nature):**
- `run_glacier_session.sh` = a copy of `run_session.sh` with a different default session dir
  (`/tmp/arena-glacier-test`), duration default **14** (the chain needs lob→land→roll→trail→spire
  cycles), and observer-0's env changed to:
  `ARENA_AUTOCAST=1 ARENA_AUTOCAST_PERIOD=1.2 ARENA_AUTOCAST_SKILL="rolling_glacier,frost_spire"
  ARENA_AUTOEQUIP=potted_spring ARENA_TEST_PITCH="-0.35" ARENA_CAM_YAW="-1.5707963"` and **NO
  `ARENA_AUTOMOVE`** (a stationary caster keeps the trail geometry deterministic). observer-1
  gains `ARENA_AUTOEQUIP=potted_spring` too (memory rule: AUTOEQUIP on EVERY observer so the
  host's start request paces its own equip round-trip). Keep `ARENA_AUTOSTART_LEVEL=arena_flat`,
  trace files/srcs, seed, teardown identical. Update the header comment to describe THIS gate.
- Geometry reasoning to start from (document in the script header): caster eye ~(-4, ~2.1, 0),
  pitch −0.35 → the eye ray grounds ~4.1m ahead (x≈0.1); the ballistic ball (speed 9) launched
  at −0.35 lands x≈−1.5..0 and the roll paints +X every 0.8m for up to 6.5s — so by the SECOND
  rotation cycle the spire's ground point (x≈0.1) lies ON the trail (patch radius 0.45 + match
  slack 0.3). Early spire pulses may fizzle (paid-nothing CastRejected) — the assertions demand
  ≥1 success across the session, not per-pulse success.
- `check_glacier_session.sh` = same skeleton as `check_session.sh` (player resolution block
  verbatim), asserting over the glacier session dir (default `/tmp/arena-glacier-test`):
```bash
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
```
  End with the same `PASS`/`FAIL` protocol. Assertions are the CONTRACT — keep them; tune only
  the SESSION parameters (pitch/period/duration) until they pass. If after tuning the spire gate
  genuinely never matches (geometry dead-end), STOP and report DONE_WITH_CONCERNS with the trace
  evidence (`cast_aim_rejected`? `CastRejected` reasons? patch positions from `surface_painted`
  pos fields) — do not weaken the assertions to force a PASS.
- No summarize.py twin for the glacier gate (python3-free local; the jq script IS the contract).
  Note that decision in the script header.

- [ ] **Step 1: Write both scripts, chmod +x, iterate**

```bash
pkill -x arena-server; pkill -x arena-client; pkill -x arena-observer; sleep 1
bash crates/arena_game/tools/net-test/run_glacier_session.sh
bash crates/arena_game/tools/net-test/check_glacier_session.sh   # iterate params until PASS
```
Use `jq` over `/tmp/arena-glacier-test/server.jsonl` between runs to see where the chain breaks
(`surface_painted` pos values tell you where the trail actually is; adjust `ARENA_TEST_PITCH`
so the ground point lands on it). Also confirm the FIREBOLT gate still passes once at the end.

- [ ] **Step 2: Docs**

- `crates/arena_game/CLAUDE.md` net-test section: add the glacier session (one short paragraph:
  scripts, what it asserts — trail paint+replication, gate accept, consume-once, ground-flush
  eruption; the env recipe).
- `docs/superpowers/plans/2026-07-10-surfaces-followups.md`: mark item #1 DONE (strike or
  annotate "DONE 2026-07-11 — run_glacier_session.sh/check_glacier_session.sh; burning tick
  folded into the firebolt gate") and item #6 DONE (the ray exclusion).

- [ ] **Step 3: Commit**

```bash
git add crates/arena_game/tools/net-test/run_glacier_session.sh \
  crates/arena_game/tools/net-test/check_glacier_session.sh \
  crates/arena_game/CLAUDE.md docs/superpowers/plans/2026-07-10-surfaces-followups.md
git commit -m "test(surfaces): glacier/spire e2e net-test — paint, gate, consume, ground-flush erupt

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Post-plan notes (controller)

- The D9 round-reset clear stays unasserted (rounds only cycle on deaths; scripting a kill into
  the glacier session is a separate escalation — note as remaining if desired).
- Remaining follow-ups after this plan: material caching (#7 both copies), decal depth (#8),
  9b editor items, py_compile (#9, CI), obelisk feature_matrix golden (#10) + burst-evict
  residual (#11) + wire-reason mapping (#12).
