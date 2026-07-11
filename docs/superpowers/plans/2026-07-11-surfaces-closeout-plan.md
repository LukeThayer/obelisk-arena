# Surfaces — Closeout Round: Tick Scratch (#11) + Glacier Damage Mystery + D9 Assertion

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the last two actionable surfaces deferrals: **#11** — make the pending design
decision and unify the same-tick paint/evict dedup across ALL paint entry points (a shared
tick-scoped scratch resource in obelisk, killing the cross-observer snapshot-blindness); and
**D9** — first INVESTIGATE why the passing glacier e2e session deals ZERO damage (the roll never
hits the target — possibly a benign geometry artifact, possibly a real latent bug), then make a
death happen, add the missing reset-clear trace signal, and assert D9 (round reset clears
patches) in the glacier gate.

**Architecture:** Task 1 in `~/src/obelisk-bevy` (branch `surfaces-scratch`); Task 2 in
`~/src/obelisk-arena` (branch `surfaces-closeout`, includes the obelisk bump to pick up Task 1).
Sequential (Task 2 bumps to Task 1's pushed rev).

## Global Constraints

- obelisk-arena tree carries USER WIP (`assets/skills/blizzard.cast.ron`, `assets/vfx/*` mods,
  untracked `Particle_ *`) — **NEVER `git add -A`** there; exact paths only.
- obelisk determinism law: goldens byte-identical (`cargo test --test golden` — all 49, incl.
  the new surfaces golden); the scratch is a behavioral NO-OP for single-caster content.
- Arena gates: firebolt + glacier both green at the end (jq checks; pkill -x; ≤3 retries;
  run_session's python3 tail exit-127 expected).
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

---

### Task 1 (obelisk-bevy, branch `surfaces-scratch`): the shared surface tick scratch (#11)

**Files:** Modify `src/surfaces/patch.rs`, `src/surfaces/systems.rs`, `src/surfaces/mod.rs`,
`tests/surfaces.rs`.

**The design (decided):** one tick-scoped Resource unifies the dedup state that today lives in
per-call locals (blind across separate observer invocations within a tick):
```rust
/// Same-tick paint/evict dedup state shared by EVERY paint entry point (the trail system, the
/// OnEnd observer, and PaintSurface request observers — which each run as separate invocations
/// within one tick and were previously blind to each other's deferred spawns/despawns).
/// Cleared at the top of every sim tick; never serialized; deterministic (insertion-order-free:
/// only membership queries, plus the paint batch which is scanned linearly).
#[derive(Resource, Default)]
pub struct SurfaceTickScratch {
    /// Patches despawn-queued by eviction this tick (the evict-once guard).
    pub evicted: std::collections::HashSet<Entity>,
    /// (surface id, position) pairs paint-queued this tick (the merge-dedup batch).
    pub painted: Vec<(String, Vec3)>,
}
```
- `try_paint` signature: REPLACE the two params `painted_this_tick: &mut Vec<(String, Vec3)>`
  and `evicted_this_tick: &mut HashSet<Entity>` with ONE `scratch: &mut SurfaceTickScratch`
  (uses `scratch.painted` / `scratch.evicted` — logic byte-identical otherwise).
- Callers: `paint_surfaces` and the two observers (`on_paint_surface`, `on_hitbox_ended_paint`)
  each take `ResMut<SurfaceTickScratch>` and pass `&mut scratch` — DELETE their per-call locals.
  Cross-invocation blindness dies by construction.
- Clear system: `pub fn clear_surface_tick_scratch(mut s: ResMut<SurfaceTickScratch>) { s.evicted.clear(); s.painted.clear(); }`
  registered in `ObeliskSurfacesPlugin` `.in_set(ObeliskSet::Validate)` (runs before Advance/
  ResolveHits where every paint/evict happens; Validate has no paint sites). Init the resource
  in the plugin.
- Observer caveat: observers fire during command flushes — potentially AFTER this tick's clear
  but interleaved with systems. That is exactly the point: within one tick, every invocation
  shares the scratch; the next tick's Validate clears it. Note in the doc comment that a
  PaintSurface triggered OUTSIDE FixedUpdate (e.g. the editor palette's instant paint, Update
  schedule) shares the scratch of the ENCLOSING tick boundary — still correct (dedup is only
  ever conservative-by-one-tick).

**TDD:**
- [ ] Step 1 — failing test (append `tests/surfaces.rs`): `cross_invocation_burst_evict_is_once_only`
  — pre-fill `capped` (max 3) to cap via sequential PaintSurface trigger+flush; then trigger TWO
  more far-apart capped paints back-to-back WITHOUT an intervening flush/update (two
  `world.trigger(...)` calls, then one flush) — two separate observer invocations in one tick;
  assert the `Evicted` removal stream contains NO duplicate patch Entity and conservation holds
  (painted − evicted == live). Under the current per-invocation sets this yields a duplicate →
  RED. Also a twin `cross_invocation_paint_dedup` — two same-position same-surface PaintSurface
  triggers in one tick → exactly ONE patch (today: two, since each invocation's batch is fresh
  and the committed query can't see the first's deferred spawn) → RED.
- [ ] Step 2 — implement per the design; both tests GREEN.
- [ ] Step 3 — full verify: `cargo test` (all suites; **goldens byte-identical** — single-caster
  scenarios never exercise cross-invocation bursts, and the scratch replaces equivalent local
  state on the single-system path); clippy clean.
- [ ] Step 4 — commit:
```
fix(surfaces): shared tick scratch — paint/evict dedup across all same-tick entry points

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```
- [ ] Step 5 — **controller merges + pushes** (not this agent).

NOTE: `SurfaceTickScratch` reaches the editor's `reset_stage` semantics too — the preview calls
paint via triggers; the scratch clears per tick so no editor change is needed (verify nothing in
the editor constructs `try_paint` directly — it doesn't; it uses the PaintSurface trigger).

---

### Task 2 (obelisk-arena, branch `surfaces-closeout`): the damage mystery + D9

**Files:** Modify `crates/arena_game/src/server/rounds.rs`,
`tools/net-test/run_glacier_session.sh` (tuning if needed), `tools/net-test/check_glacier_session.sh`,
`Cargo.lock` (obelisk bump), `docs/superpowers/plans/2026-07-10-surfaces-followups.md`.

- [ ] **Step 0 — bump**: `git checkout -b surfaces-closeout && cargo update -p obelisk-bevy`
  (picks up Task 1's pushed rev); `cargo test -p arena_game` green (the scratch is additive; if
  the obelisk API change — try_paint's signature — breaks arena compile: it CANNOT, try_paint is
  `pub(crate)` to obelisk; nothing arena-side calls it).

- [ ] **Step 1 — INVESTIGATE the zero-damage session** (evidence-first; do NOT guess):
  Run the glacier session, then interrogate the traces:
```bash
jq -s '[.[] | select(.kind=="server_net_damage_resolved")] | length' /tmp/arena-glacier-test/server.jsonl
jq -s '[.[] | select(.kind=="server_net_cast_began")] | group_by(.skill_id) | map({skill: .[0].skill_id, n: length})' /tmp/arena-glacier-test/server.jsonl
jq -s '[.[] | select(.kind=="surface_painted")] | [first, last] | map(.pos)' /tmp/arena-glacier-test/server.jsonl
jq -s '[.[] | select(.kind=="player_spawned")]' /tmp/arena-glacier-test/server.jsonl
```
  Hypotheses to check in order: (a) the trail's z-line vs the target's actual spawn position in
  `arena_flat` (read the level's spawn slots — `assets/scenes/arena_flat.scn.ron` or the
  `LevelSpawns` trace; the -4/+4 lore may not match the level data); (b) the roll's hitbox Y vs
  the pinned hurtbox capsule (roll sphere r0.32 at ~y0.35+0.16); (c) the ball's flight y at the
  target's x (does it sail over?). Diagnose with the traces + code reads; write the finding in
  the report BEFORE fixing.
  - If it's a SESSION-GEOMETRY artifact (e.g. the roll passes to the side / target offset):
    retune the session (yaw/pitch or a second casting pattern) so the chain HITS.
  - If it's a REAL CODE BUG (the roll structurally can't hit a player): STOP —
    DONE_WITH_CONCERNS with the full diagnosis; the fix becomes its own reviewed change, not a
    drive-by.
- [ ] **Step 2 — make a death happen + D9 signal**: with the chain hitting, deaths follow
  (roll 28 + burst 20 + ball 28 vs ~100hp across repeated cycles) → `detect_round_end` →
  Countdown → the reset. Add the trace signal at the reset's patch-clear loop
  (`rounds.rs` ~line 402, inside `run_round_machine` before `reset_for_new_round`):
```rust
                let cleared = surface_patches.iter().count();
                if cleared > 0 {
                    crate::trace::event(
                        "surfaces_reset_cleared",
                        serde_json::json!({ "count": cleared }),
                    );
                }
                for patch in &surface_patches {
                    commands.entity(patch).despawn();
                }
```
  (Match the file's existing trace import/idiom — `trace::event` + `json!`.)
- [ ] **Step 3 — assert D9 + damage in the glacier gate** (`check_glacier_session.sh`), keeping
  every existing assertion:
```bash
# (7) the glacier chain actually damages the target (guards against a silently pacifist session).
n=$(jq -s --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="server_net_damage_resolved" and .caster==$c and .target==$t)] | length' "$server")
[[ "$n" -ge 1 ]] || note "the glacier chain dealt no damage (session regressed to pacifist)"
# (8) D9: a mid-session round reset cleared the painted ground.
n=$(jq -s '[.[] | select(.kind=="surfaces_reset_cleared")] | length' "$server")
[[ "$n" -ge 1 ]] || note "no round reset cleared surfaces (D9 unproven — did anyone die?)"
```
  Extend the diagnostic echo line with `damage=<n> reset_clears=<n>`. Tune duration if the
  death+reset needs more cycles (keep assertions verbatim; tune only session params). Gate must
  PASS ≥2 consecutive runs; the FIREBOLT gate re-verified once (the rounds.rs change touches it
  too — a firebolt session kill also fires the new trace, harmlessly unasserted there).
- [ ] **Step 4 — annotate**: followups doc — #11 ✅ DONE (obelisk rev + one-line design), D9 ✅
  DONE (trace + assertion, session now lethal); if Step 1 found a real bug, note it instead per
  the STOP rule.
- [ ] **Step 5 — commit** (rounds.rs + both gate scripts as touched + Cargo.lock + followups doc):
```
test(surfaces): D9 reset-clear asserted e2e; glacier session made lethal; obelisk tick-scratch bump

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

## Post-plan notes (controller)

- After both: merge + push all touched repos per the standing pattern. The followups doc then
  holds only #9 (CI) and #12 (YAGNI) — terminal deferrals.
- If Task 2's Step 1 STOPs on a real bug: that diagnosis seeds the next round's plan instead.
