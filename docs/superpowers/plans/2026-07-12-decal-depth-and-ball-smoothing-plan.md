# Decal Depth-Test Fork + Ball Pose Smoothing — the two glacier visual bugs

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the two user-reported glacier visuals: (A) frost ground renders THROUGH the
boulder (and residually through dummy/players) — bevy 0.18 `ForwardDecal` forces
`depth_compare: Always`, so the flush decal quad draws over any nearer opaque geometry; fork the
decal material with a NORMAL depth test in BOTH renderers so occluders occlude. (B) the boulder
visibly sinks into the ground then pops back — replicated skill-object poses are mirrored RAW at
the ~30 Hz snapshot cadence with no smoothing, exposing solver penetration/bounce frames;
investigate evidence-first, then smooth (and CCD the server ball if penetration is real).

**Verified facts:** ball material is opaque (`client/skill_objects.rs:199-206` — no alpha), so
(A) is the decal overlay, not ball transparency. The Always override lives in bevy_pbr's
`ForwardDecalMaterialExt::specialize` (`decal/forward.rs:128`); the quad mesh comes from the
`ForwardDecal` component's on-add hook (mesh only — reusable with a different material type).
`mirror_skill_object_pose` copies replicated avian `Position`/`Rotation` straight into the
render `Transform` (no interpolation; players get `FrameInterpolate`, skill objects don't).

## Global Constraints

- obelisk-arena tree carries USER WIP (`assets/skills/blizzard.cast.ron`, `assets/vfx/*` mods,
  untracked `Particle_ *`) — **NEVER `git add -A`**; exact paths.
- NO obelisk-bevy changes. bevy_modal_editor pushes to the **`lukethayer` remote**.
- `crates/arena_editor` is its own workspace (cargo from that dir).
- The forked decal must keep the headless-safe gating exactly as today (material registration +
  attach are windowed-only in the arena; the mirror-collider path is untouched).
- Glacier gate assertions (1)-(10) + scripts byte-untouched; glacier ×2 + firebolt ×1 at the end
  (a temp trace may be ADDED then REMOVED/cfg-gated — final committed scripts unchanged).
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

---

### Task 1 (obelisk-arena, branch `glacier-visual-fixes`): evidence, smoothing, arena decal fork

**Files:** Modify `crates/arena_game/src/client/surfaces.rs`, `crates/arena_game/src/client/skill_objects.rs`,
`crates/arena_game/src/server/glacier_ball.rs` (CCD, only if evidence demands),
NEW `crates/arena_game/src/client/decal_material.rs` (the fork; registered from `app_windowed.rs`),
possibly `assets/shaders/` (vendored wgsl — see Step 2).

- [ ] **Step 0 — EVIDENCE for (B), before any fix.** Add a TEMPORARY server-side trace (e.g.
  `glacier_ball_pose { y, vy }` every 2 ticks while a ball is live, in glacier_ball.rs). Run ONE
  glacier session (rebuild). Analyze with jq: the ball's minimum center Y across its life
  (center below `GLACIER_BALL_RADIUS = 0.32` minus ~0.02 ⇒ REAL solver penetration), the settle
  profile after landing (bounce peaks), and whether penetration spikes correlate with landing
  frames. WRITE THE FINDING (numbers) in the report, then REMOVE the trace (or gate it behind an
  `ARENA_TRACE_BALL_POSE` env check if it earns its keep — your call, justify).
- [ ] **Step 1 — fix (B) per evidence.**
  - Always: client-side visual smoothing for skill-object pose mirroring — the mirror lerps the
    render Transform toward the latest replicated pose (position + slerped rotation, rate ≈
    `1 - exp(-dt * 20)`) instead of snapping, with a TELEPORT SNAP threshold (> 2.0 m ⇒ snap —
    portal warps and round resets must not glide; mirror the `snap_large_corrections` precedent).
    Applies to all `NetworkedSkillObject` visuals (spire rise gets smoother too — fine); keep
    the very first pose a snap (spawn).
  - Only if Step 0 showed real penetration (min center Y < 0.30): add avian CCD to the server
    ball (`SweptCcd` — verify the avian 0.5 component name/semantics in the vendored source) and
    re-run the evidence session to show the min-Y improvement.
- [ ] **Step 2 — the decal fork (A), arena copy.** New `client/decal_material.rs`:
  `DepthTestedDecalExt` — a `MaterialExtension` clone of bevy's `ForwardDecalMaterialExt`
  (same `depth_fade_factor` uniform, same shader) WITHOUT the `depth_compare = Always` override
  (leave the pipeline's standard LessEqual). Shader: reuse bevy's `forward_decal.wgsl` via its
  public shader path/handle if accessible; otherwise VENDOR the file into `assets/shaders/`
  verbatim with an attribution comment (it is small). Register
  `MaterialPlugin::<ExtendedMaterial<StandardMaterial, DepthTestedDecalExt>>` in
  `app_windowed.rs` (windowed-only, exactly like today's guarded decal plugin add). Swap
  `attach_surface_visuals` to the forked material (keep the `ForwardDecal` marker for its quad
  mesh hook IF it is material-agnostic — verify in bevy source; if the hook or plugin assumes
  the stock material type, insert the rotated `Rectangle` mesh directly instead and drop the
  marker). Keep the ground-snap, scale, fade 1.0, material cache, and the +0.01 bias (now ALSO
  the z-offset that wins the depth test vs the floor — say so in the comment).
  **Escape hatch:** if the fork fights bevy internals beyond bounded effort (pipeline
  specialization the extension can't reach), fall back to shrinking `depth_fade_factor` to 0.35
  on the stock material (band-aid: only the bottom sliver of occluders catches frost),
  DONE_WITH_CONCERNS with the analysis — do not ship a broken material.
- [ ] **Step 3 — verify.** `cargo test -p arena_game`; glacier ×2 consecutive PASS (REBUILD,
  final scripts byte-identical; pkill -x between; ≤3 retries); firebolt ×1. The decal fork is
  windowed-only — gates prove no headless regression; visual correctness lands on the user's
  pass (say so in the report).
- [ ] **Step 4 — commit** (exact paths):
```
fix(client): depth-tested decal fork (frost no longer draws through occluders) + smoothed skill-object poses

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

### Task 2 (bevy_modal_editor, branch `decal-depth`, then the arena bump): editor twin

**Files:** bevy_modal_editor `src/skill/preview/surfaces.rs` (+ a small module for the forked
material mirroring Task 1's — same name/structure), `tests/skill_preview.rs` (only if an
existing test pins the material type); then obelisk-arena `crates/arena_editor/Cargo.lock`.

- [ ] **Step 1:** Mirror Task 1's fork exactly in the editor preview (`attach_patch_visuals`
  swaps to the forked material; same vendored-shader decision as Task 1 — copy what Task 1
  chose). The preview's decal plumbing is windowed-gated the same way — keep it.
- [ ] **Step 2:** `cargo test --features obelisk` green (the ground-snap test asserts the child
  TRANSFORM, not the material type — should hold; update honestly if a test pins the material).
- [ ] **Step 3:** Commit in bevy_modal_editor:
```
fix(skill/surfaces): depth-tested decal fork — patches no longer draw through the dummy/casters

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```
- [ ] **Step 4 (obelisk-arena, on master after Task 1 merges):** controller pushes the editor;
  then `cd crates/arena_editor && cargo update -p bevy_modal_editor && cargo test`; commit the
  lockfile bump (exact path) with:
```
chore(editor): bump bevy_modal_editor — depth-tested decal fork

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

## Post-plan notes (controller)

- Merge + push per the standing pattern after each task's review.
- User acceptance: frost never draws over the boulder/dummy/players (occluders occlude; a faint
  genuine top-projection on surfaces hugging the ground remains = icing); the boulder lands and
  settles smoothly (no sink-pop). Both land on the user's next visual pass.
