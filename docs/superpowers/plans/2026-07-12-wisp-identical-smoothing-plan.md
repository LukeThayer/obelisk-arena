# Wisp-Identical Ball Smoothing — Static mirror + adaptive-span delay

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** make the glacier ball's replicated motion look like wisp's — by porting wisp's ACTUAL
mechanism (deep-read spec, session 2026-07-12), which is pure snapshot interpolation with two
properties the arena lacks: a CLEAN once-per-snapshot sample stream and an ADAPTIVE render delay.

**The verified wisp mechanism (from ~/src/wisp source, file:line in the session spec):** client
prop = `RigidBody::Static` collider + mesh on one entity (replication.rs:896 — Static is not a
SolverBody, so its Position is never solver-re-marked); NO velocity on the wire ever; a
two-sample (prev/cur) buffer captured on change with a >3m teleport snap (replication.rs:131-143);
render law = `span = cur_time − prev_time (floor 1e-3); render_time = now − span;
t = clamp((render_time − prev_time)/span)` — the delay ADAPTS to the actual last inter-sample
gap so the lerp sweeps 0→1 in lockstep with arrival (replication.rs:182-216); hard-set, no
exponential smoothing, no epsilon, never extrapolates. Wisp sends at 60Hz unthrottled; adaptive
delay makes the arena's 30Hz equivalent-looking (60Hz = optional later knob, NOT this plan —
the arena's send interval also sets mispredict cadence).

**Why not byte-identical:** wisp has no predicted players and its sample source
(`NetworkedPosition`) is not avian `Position`. The arena MUST keep: collider root pinned to the
raw replicated `Position` (rollback + shove + witness) and the smoothed pose on the
NON-replicated mesh child (invariant 1: never write the replicated root's Transform;
`AvianReplicationMode::Position` would feed a root-write back into the sample stream). The
child split IS the correct wisp adaptation.

## The two deltas (everything else stays)

1. **`client_ball_mirror_bundle` (server/glacier_ball.rs:~148): `RigidBody::Kinematic` →
   `RigidBody::Static`.** Kills the 60Hz solver re-marking at the root (Static has no
   SolverBody) → `Changed<Position>` fires once per real snapshot. LAYERS AND THE EVERY-CLIENT
   (headless-included) ATTACH ARE UNTOUCHED — invariant 16. Shove semantics unchanged
   (position-stepped depenetration works identically for Static; the mirror never had velocity).
   Update the bundle comment (why Static: wisp parity + no SolverBody = clean change stream).
2. **Adaptive render delay (client/skill_objects.rs): replace the fixed
   `INTERP_DELAY_SECS = 1.5/30` with wisp's law** — delay = the NEWEST PAIR's span:
   `span = (newest.recv − prev.recv).max(1e-3); render_at = now − span`, then the existing
   `sample_pose_at` clamp/lerp/snap machinery (already wisp-equivalent: never extrapolates,
   2.0m snap — keep 2.0, note wisp's is 3.0 and ours predates for portal warps).

## Simplify (the scar tissue the deltas obsolete — delete, don't keep)

- The value-dedup (`POSE_DEDUP_DIST_SQ`/`POSE_DEDUP_ANGLE` + the push guard) and the cap-8
  depth rationale existed ONLY to survive the Kinematic re-mark flood. With a clean stream:
  shrink to wisp's shape — keep the buffer type but `POSE_BUFFER_CAP = 2` (prev/cur) and delete
  the dedup guard + its consts + its test arms (fold the surviving assertions — cap eviction,
  rotation recording — into a lean test). The comment chain gets ONE final honest rewrite:
  Static mirror ⇒ once-per-snapshot stream ⇒ 2 samples suffice ⇒ delay = last span (wisp law,
  cite the spec). Do NOT preserve the dead workarounds "for safety" — they mask regressions.
- KEEP: `sample_pose_at` core (pair lerp/slerp, clamps, snap), `child_local_for`, the mesh
  child, `mirror_skill_object_pose` (bodyless spire), velocity exclusion (verbs.rs — wisp
  sends no velocity either), the subfloor witness (now guards an impossible condition — a
  Static body cannot integrate; keep as regression tripwire), `glacier_ball_layers()`.

## Global Constraints

- obelisk-arena tree carries USER WIP (`assets/skills/blizzard.cast.ron`, `assets/vfx/*` mods,
  untracked `Particle_ *`) — **NEVER `git add -A`**; exact paths.
- NO obelisk-bevy / protocol / wire changes; server ball physics untouched (Dynamic, wisp
  params). Gate scripts + assertions (1)-(11) byte-untouched.
- The windowed probe is the acceptance instrument (DISPLAY=:0 works — the interp-inert round
  proved it): pre/post numbers REQUIRED.
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

---

### Task 1 (obelisk-arena, branch `wisp-smoothing`): both deltas + the simplification

**Files:** `crates/arena_game/src/server/glacier_ball.rs` (mirror bundle),
`crates/arena_game/src/client/skill_objects.rs` (delay law + buffer shrink + tests).

- [ ] **Step 1 — unit tests first for the new law:** rewrite/extend the interp tests: (a)
  adaptive delay — with samples at recv {t0, t0+33ms}, a query at `now` where
  `now − span = t0 + k·span` lands at fraction k (parametrize k = 0, 0.5, 1.0); (b) the
  never-extrapolate clamp and snap-pair tests survive verbatim; (c) cap-2 eviction (three
  pushes keep the newest two); (d) rotation-only change still recorded (no dedup to block it
  now — the test flips meaning: it must STILL record, trivially). RED where the law differs
  from the fixed-delay implementation, then implement.
- [ ] **Step 2 — the two deltas + deletion** per above. Comments rewritten once, honestly,
  citing wisp replication.rs:182-216 and the Static/SolverBody mechanism.
- [ ] **Step 3 — the windowed probe (pre/post):** re-add the `interp-inert` round's probe
  TEMPORARILY (or gate behind `ARENA_INTERP_PROBE`): sample-arrival cadence, render_t position
  within the pair, clamp-event count, child per-frame glide. Run the real windowed client vs a
  local server with autocast glacier, ~30s. REQUIRED post numbers: sample stream ≈ once per
  snapshot (~30Hz, no 60Hz pollution), clamp events ≈ 0, t sweeping the pair, uniform glide.
  Capture PRE numbers on the unmodified code first if cheap (one run) — the delta is the story.
  Remove/gate the probe before the final commit.
- [ ] **Step 4 — verify:** `cargo test -p arena_game`; glacier ×2 consecutive PASS (REBUILD;
  pkill -x between; ≤3 retries) — assertions (1)-(11) green (the witness must stay green with
  the Static mirror); firebolt ×1.
- [ ] **Step 5 — commit** (exact paths):
```
fix(client): wisp-identical ball smoothing — Static mirror (clean sample stream) + adaptive-span delay

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

## Post-plan notes (controller)

- Merge + push per the standing pattern after review.
- If the user STILL sees roughness after this, the remaining wisp delta is send cadence (wisp
  60Hz vs arena 30Hz) — a deliberate global-knob discussion (mispredict cadence), not a patch.
- The predicted ghost-ball (zero-latency own-cast) remains the separate named follow-up.
