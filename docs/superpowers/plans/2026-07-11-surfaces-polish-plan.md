# Surfaces — Polish Round: Golden Scenario + Material Cache + Decal Depth + Editor Smalls

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining actionable surfaces follow-ups: #10 the spec-§11 behavior golden
(obelisk `feature_matrix()` scenario + committed `.trace`), #7 material caching (BOTH decal
renderers), #8 decal projection depth vs elevated patches (both copies), and 9b the two editor
smalls (StagedPaints dedup-on-push; de-fragilized test precondition). Annotate the rest
(#9/#11/#12/D9) as explicitly deferred with reasons.

**Architecture:** Three independent repo-scoped tasks, run SEQUENTIALLY (no cross-repo deps —
the obelisk change is TEST-ONLY, so no consumer bumps): Task 1 in `~/src/obelisk-bevy` (branch
`surfaces-golden`), Task 2 in `~/src/obelisk-arena` (branch `surfaces-polish`), Task 3 in
`~/src/bevy_modal_editor` (branch `surfaces-polish`). Follow-ups doc:
`docs/superpowers/plans/2026-07-10-surfaces-followups.md` (obelisk-arena).

## Global Constraints

- obelisk-arena's tree carries USER WIP (`assets/skills/blizzard.cast.ron`, `assets/vfx/*.vfx.ron`
  mods, untracked `Particle_ *`) — **NEVER `git add -A`** there; stage exact paths. The other two
  repos are clean but the same staging discipline applies.
- Determinism is law in obelisk: the new scenario must be seed-stable (surfaces draw no RNG;
  the fixture content already proved two-run determinism) and MUST NOT perturb any existing
  golden (`cargo test --test golden` — every pre-existing `.trace` byte-identical).
- Suites green per task: obelisk `cargo test`; arena `cargo test -p arena_game` + the two
  net-test gates (firebolt + glacier, jq checks; pkill -x; ≤3 retries); editor
  `cargo test --features obelisk`.
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

---

### Task 1 (obelisk-bevy, branch `surfaces-golden`): the spec-§11 surfaces behavior golden

**Files:** Modify `src/scenario/library.rs` (new scenario + `feature_matrix()` registration);
Create `tests/golden/<scenario_name>.trace` (recorded). Possibly `src/scenario/mod.rs`/`run.rs`
ONLY if the Scenario API genuinely cannot express the script (see the STOP rule).

**Why:** the two-run determinism test locks *reproducibility* (run A == run B in one binary);
a committed `.trace` locks *behavior* — a refactor that changes a damage number or paint
position must fail loudly, not pass because both runs agree.

- [ ] **Step 1: Learn the Scenario API** — read `src/scenario/mod.rs` (the `Scenario`/`Action`
  types), 2–3 sibling scenarios in `library.rs` (`blizzard_emitter` is the closest shape — it
  loads a cast asset, spawns actors, casts, runs N ticks), and `tests/golden.rs` (the
  `UPDATE_GOLDEN=1` record flow, `golden_path`, and how `feature_matrix()` feeds it).

- [ ] **Step 2: Author the scenario** — name it `surfaces_paint_stand_ignite`. Script (the
  proven shape from `tests/surfaces.rs::surfaces_pipeline_is_deterministic_across_runs`, but
  expressed as a Scenario): a painter actor + a victim actor (with hurtbox) positioned so that
  (a) a `paint_roller` cast (Trail frost — cast asset `tests/fixtures/cast/paint_roller.cast.ron`)
  lays a trail, (b) a pre-painted or bolt-painted `burning` patch under the victim ticks
  `burning_ground_tick` (cast asset `tests/fixtures/cast/burning_tick.cast.ron`), and (c) a
  `fire_probe` cast crosses an `oil` patch and executes `test_ignite` with consumption.
  Surface types load from `tests/fixtures/surfaces/` — check how `run_scenario` initializes
  registries (it calls `add_obelisk_skills(fixtures)` — it must ALSO gain
  `add_obelisk_surfaces(tests/fixtures/surfaces)`, or the Scenario struct needs a
  surfaces-fixtures field; prefer the UNCONDITIONAL `add_obelisk_surfaces` call in
  `run_scenario` (registry loading is inert for surface-less scenarios — verify existing
  goldens stay byte-identical, which also proves the inertness claim). Pre-painting (for (b))
  can use the public `PaintSurface` trigger if the Action vocabulary has a world-hook, else
  skip (b)'s pre-paint and let a `paint_blast`-style OnEnd cast paint it — use whatever the
  API expresses cleanly. **STOP rule:** if the Scenario API fundamentally can't express ANY
  surfaces interaction without invasive extension, report DONE_WITH_CONCERNS with the analysis
  instead of bolting a new Action system on — the golden's value must exceed its machinery.
- [ ] **Step 3: Record + verify** — `UPDATE_GOLDEN=1 cargo test --test golden` (records the new
  trace), then `cargo test --test golden` twice more (byte-stable), then
  `git diff tests/golden/` shows ONLY the new file (existing goldens untouched), then full
  `cargo test`. Inspect the recorded trace by eye: it must contain paint events, standing-tick
  damage, the ignite execution, and a consumption — a trace without those is a vacuous golden
  (STOP and fix the script).
- [ ] **Step 4: Commit** (`library.rs` + the new `.trace` (+ `run.rs` if touched)):
```
test(surfaces): feature-matrix golden — paint, standing tick, contact ignite (spec §11)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

### Task 2 (obelisk-arena, branch `surfaces-polish`): material cache + decal depth (arena copy)

**Files:** Modify `crates/arena_game/src/client/surfaces.rs`,
`docs/superpowers/plans/2026-07-10-surfaces-followups.md` (annotations).

- [ ] **Step 1: Material cache (#7)** — in `attach_surface_visuals`, replace the per-patch
  `decal_materials.add(...)` with a per-surface-type cache:
  `mut material_cache: Local<std::collections::HashMap<String, Handle<ForwardDecalMaterial<StandardMaterial>>>>`
  — key = the patch's `surface` id; on miss, build the material exactly as today and insert;
  on hit, clone the cached handle. (Registry visuals are static per type at runtime — note
  that assumption in a comment; a future hot-reload of surface TOMLs would need cache
  invalidation.)
- [ ] **Step 2: Decal depth (#8)** — the decal child's Y scale is `1.0` (±0.5 box): an elevated
  patch (torso-hit scorch at y≈1.2, air-fuse) can out-range the floor. Change the scale to
  reach the ground with margin:
```rust
        // Elevated patches (torso-hit scorch, air fuse) must still project to the floor: the
        // decal box spans ±half the Y scale around the patch, so grow it to cover |y| + margin.
        let y_span = (pos.0.y.abs() * 2.0 + 1.0).max(1.0);
        // ... Transform::from_scale(Vec3::new(p.radius * 2.0, y_span, p.radius * 2.0)),
```
- [ ] **Step 3: Verify** — `cargo test -p arena_game` green; then BOTH net-test gates
  (`run_session.sh`+`check_session.sh`, `run_glacier_session.sh`+`check_glacier_session.sh`,
  ARENA_SKIP_BUILD=1 after the first build, pkill -x between, ≤3 retries each) — the glacier
  gate exercises replicated patch visuals attach paths end-to-end headlessly (visual correctness
  itself is windowed-only; the gates prove no regression in the attach/replication plumbing).
- [ ] **Step 4: Followups annotations** — mark #7 arena-half DONE (editor half lands in Task 3 —
  annotate accordingly), #8 arena-half DONE; annotate the explicit deferrals with one-line
  reasons: #9 (python3 = CI-only), #11 (cross-observer burst-evict needs a shared-tick-scratch
  design decision; system path guarded), #12 (no consumer needs removal REASONS on the wire —
  YAGNI until one does), D9 assertion (needs a scripted kill; reset behavior code-reviewed +
  round-crossing glacier session passes through it benignly).
- [ ] **Step 5: Commit** (surfaces.rs + followups doc):
```
perf(surfaces): per-surface decal material cache + elevated-patch projection depth

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

### Task 3 (bevy_modal_editor, branch `surfaces-polish`): editor copy + 9b smalls

**Files:** Modify `src/skill/preview/surfaces.rs`, `src/skill/preview/stage.rs` OR
`src/ui/command_palette/skill_preset.rs` (wherever the StagedPaints push lives),
`tests/skill_preview.rs`.

- [ ] **Step 1: Mirror Task 2 exactly** in `attach_patch_visuals` (same `Local` cache keyed by
  surface id, same `y_span` formula — patches in the preview carry `Transform`, use its
  translation.y). Keep the marker-child/headless gating untouched.
- [ ] **Step 2: StagedPaints dedup-on-push (9b)** — at the palette's push site: skip the push if
  an identical entry exists (`staged.0.iter().any(|s| s.surface == id && s.position == pos)`),
  still trigger the instant paint (obelisk dedups it — same behavior, no Vec growth). One-line
  comment.
- [ ] **Step 3: De-fragilize the test precondition (9b)** — in
  `tests/skill_preview.rs::stage_reset_rezeroes_surface_and_spawn_streams`, the
  `!inside_prev.is_empty()` precondition depends on the caster geometrically standing in the
  trail's first splat. Make it robust: paint a patch DIRECTLY AT the caster's own position via
  `PaintSurface` (query the caster transform in the test) before the assert, so the standing
  overlap is by construction, not by stage-layout accident. Keep every assertion identical.
- [ ] **Step 4: Verify + commit** — `cargo test --features obelisk` green (all suites);
```
perf(skill/surfaces): decal material cache + projection depth; staged-paint dedup; robust reset test

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

## Post-plan notes (controller)

- After all three: push obelisk-bevy `main` (origin) and bevy_modal_editor `main`
  (**`lukethayer` remote, NOT origin**); merge the arena branch per the user's standing pattern.
  No consumer lockfile bumps needed (Task 1 is test-only; Tasks 2–3 are leaf changes).
- Remaining after this round: nothing actionable — the followups doc becomes a record of
  explicit deferrals only.
