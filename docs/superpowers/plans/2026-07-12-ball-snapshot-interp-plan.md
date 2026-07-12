# Skill-Object Snapshot Interpolation — fix the 30 Hz ball jitter

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the rolling boulder renders smoothly on clients. Replicated-only (non-predicted) skill
objects get classic SNAPSHOT INTERPOLATION: buffer received (Position, Rotation, recv-time)
samples and render the VISUAL ~one snapshot interval in the past, interpolating between real
authoritative states — no extrapolation, no overshoot at bounces, collider mirror stays raw.

**Why (proven, not guessed):** velocity is now per-entity excluded from the ball's replication
(the sink fix, `83f4fe8`), so the mirror's Position STEPS at the 30 Hz send cadence (7 m/s ⇒
~23 cm jumps) — previously the (buggy) velocity integration acted as accidental dead reckoning
horizontally. The earlier exponential smoothing CANNOT fix this: the sink investigation's
schedule graph proved avian's `position_to_transform` writeback pins the physics-body ROOT's
Transform to raw Position every frame (`Position.y == Transform.y`, divergence 0.000000, n=2926)
— the root belongs to avian; a smooth visual must live OUT of the writeback's reach.

**Architecture:** windowed-only, client/skill_objects.rs (+ its plugin). The replicated ROOT
keeps: kinematic mirror collider (fresh raw pose — shove parity), PointLight is fine either way
(move it with the mesh). The MESH (+ light) moves to a CHILD entity; a per-frame system computes
the child's LOCAL transform so its WORLD pose equals the interpolated snapshot pose:
`local_pos = parent_rot⁻¹ · (target_world_pos − parent_pos)`, `local_rot = parent_rot⁻¹ ·
target_world_rot` (pure math — unit-test it). The buffer holds the last ≤4 samples pushed on
`Changed<Position>` (avian Position on the replicated entity changes exactly on snapshot
arrival); render time = now − INTERP_DELAY where `INTERP_DELAY = 1.5 × (1/REPLICATION_SEND_HZ)`
= 50 ms (1.5 intervals absorbs one-interval jitter; named const, reference
`net::REPLICATION_SEND_HZ`... verify the const's actual home — CLAUDE.md names
`REPLICATION_SEND_HZ=30` in net/server.rs). Clamp: before 2 samples exist, sit on the newest;
if the buffer's newest is older than 3 intervals (stream quiet — object at rest), converge to
the newest raw pose; NEVER extrapolate past the newest sample. Teleport handling: a sample-pair
gap > 2.0 m interpolates as a SNAP (jump at the pair boundary, mirroring the sink-round's snap
threshold — portal warps must not glide).

The old exponential smoothing system (defeated by writeback, proven no-op on the root) is
DELETED — replaced by this. Applies to ALL `NetworkedSkillObject` visuals whose mesh rides the
child (balls now; spires' rise gets faithful-delayed motion too — restructure their recipe the
same way; portals' visuals are bespoke (PortalMaterial discs + cameras) — LEAVE portals'
existing structure alone unless their mesh already uses the generic recipe path; check first).

## Global Constraints

- obelisk-arena tree carries USER WIP (`assets/skills/blizzard.cast.ron`, `assets/vfx/*` mods,
  untracked `Particle_ *`) — **NEVER `git add -A`**; exact paths.
- NO obelisk-bevy changes; NO protocol/wire changes; NO server changes (client-render-only).
  Headless paths untouched: the interp system + mesh-child restructure are windowed-only; the
  kinematic mirror collider + assertion-(11) trace read the RAW replicated components and must
  be provably unaffected.
- Gate scripts + assertions (1)-(11) byte-untouched. Glacier ×2 + firebolt ×1 at the end
  (no-regression proof; the jitter itself is windowed-only — final verdict is the user's pass).
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

---

### Task 1 (obelisk-arena, branch `ball-snapshot-interp`): the interpolated visual child

**Files:** Modify `crates/arena_game/src/client/skill_objects.rs` (recipe restructure + buffer +
interp system; delete the old smoothing), possibly `crates/arena_game/src/client/mod.rs`
(module wiring only if a new file is cleaner — implementer's call, keep one clear home).

- [ ] **Step 1 — pure math first (TDD):** `#[cfg(test)]`-covered helpers:
  (a) `sample_pose_at(buffer, t) -> (Vec3, Quat)` — binary-search the bracketing pair, lerp/slerp
  by the pair-local fraction; clamp to newest/oldest; snap (no blend) across a >2.0 m pair gap.
  Tests: exact midpoint lerp; clamp-before-two-samples; clamp-past-newest (never extrapolates);
  snap-pair behavior; slerp shortest-path sanity.
  (b) `child_local_for(parent_pos, parent_rot, target_pos, target_rot) -> Transform` — the
  counter-transform; test: composing parent × result == target world pose (position + rotation,
  epsilon 1e-5), including a rotated parent.
  RED first (stubs), then implement — paste one RED excerpt in the report.
- [ ] **Step 2 — restructure the recipes:** glacier ball (and spires if they share the generic
  mesh recipe — verify portals first, leave them if bespoke): mesh + material + light move onto
  a spawned CHILD (`SkillObjectVisual` marker moves with the mesh child; keep names/labels);
  root keeps collider/mirror + replicated components. Verify nothing else queries
  `SkillObjectVisual` expecting the root (grep; the subfloor trace reads NetworkedSkillObject +
  Position on the ROOT — untouched).
- [ ] **Step 3 — buffer + interp systems (windowed plugin):** push samples on
  `Changed<Position>` (store `(recv_secs, pos, rot)`, cap 4, drop oldest); per-frame (Update)
  compute the delayed target via `sample_pose_at` and write the mesh child's local transform via
  `child_local_for`. Buffer lives as a component on the root; despawns with it. First-frame
  before any sample: child sits at identity (root pose).
- [ ] **Step 4 — delete the defeated smoothing** (system + its constants + registration; the
  >2m snap semantics live on in the interp's snap-pair rule — keep the comment pointing at the
  sink-round precedent).
- [ ] **Step 5 — verify:** `cargo test -p arena_game` (new unit tests + suite); glacier ×2
  consecutive PASS (REBUILD; pkill -x between; ≤3 retries) + firebolt ×1 — all assertions
  (1)-(11) green (the trace/collider paths must be untouched — if (11) flakes, you touched the
  wrong layer, STOP and re-read). State plainly: smoothness itself lands on the user's windowed
  pass.
- [ ] **Step 6 — commit** (exact paths):
```
fix(client): snapshot-interpolated skill-object visuals — the ball renders between real states, not 30Hz steps

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

## Post-plan notes (controller)

- Merge + push per the standing pattern after review.
- User acceptance: the boulder rolls smoothly (a constant ~50 ms render delay on skill objects
  is the design trade — imperceptible on a boulder; players are unaffected, they're predicted).
- If the user still sees roughness after this, the next lever is the per-group replication send
  frequency for the ball (lightyear ReplicationGroup send_frequency) — named follow-up, not now.
