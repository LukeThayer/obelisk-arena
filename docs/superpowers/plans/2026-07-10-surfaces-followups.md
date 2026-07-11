# Surfaces — Tracked Follow-ups (post arena increment)

Filed 2026-07-10 at the `surfaces-arena` final review's direction (verdict: ready to merge,
conditioned on filing these — the coverage gap must not live only in the SDD ledger).

## Required (the merge condition)

1. **Glacier/spire e2e net-test extension.** ✅ **DONE 2026-07-11** — added the GLACIER session
   `crates/arena_game/tools/net-test/run_glacier_session.sh` + `check_glacier_session.sh` (a
   second scripted session, sibling to the firebolt `run_session.sh`/`check_session.sh`): equips
   `potted_spring` on both observers, alternates `rolling_glacier` (asserts ≥3 `surface_painted{frost}`
   server + ≥3 `replicated_surface_patch{frost}` on both observers) and `frost_spire` at the trail
   (asserts ≥1 `server_net_cast_began{frost_spire}` = the `on_surface` gate matched, ≥1
   `surface_removed{frost, reason:"Consumed"}`, and every `spire_erupted` ground-flush `|pos[1]| ≤ 0.25`),
   plus ≥2 `rolling_glacier` casts. 10/10 repeatable (jq gate; no summarize.py twin — python3-free).
   The **`burning_ground_tick`** coverage (a victim standing in a scorch) was folded into the FIREBOLT
   gate instead (Task 1 — `check_session.sh` assertions (9)/(10)), where the firebolt_explosion already
   paints `burning` under the target. REMAINING (deliberately deferred): the **D9 round-reset clear
   stays unasserted** — pinning it needs a SCRIPTED KILL (rounds only cycle on deaths), a separate
   escalation. Deferral reason: the reset code is code-reviewed AND the glacier session crosses a
   round reset BENIGNLY (it incidentally resets once and surfaces behave through the crossing), so
   the D9 path is exercised end-to-end even though no assertion pins the clear itself.
   The one real bug of the increment (the spire Y-float) is now pinned e2e by assertion (5).

   ~~The gate only casts firebolt, so `Trail` painting, `on_surface` gate/snap/CONSUME, and the spire
   eruption have no automated runtime coverage — the one real bug of the increment (the spire Y-float)
   lived exactly in this blind spot.~~

## Editor increment 6 (pre-warned footguns, fold into that plan)

2. `DepthPrepass` is main-camera-only (`client/scene.rs`) — the editor preview/stage cameras
   render NO decals until they add it; the D5 stage-paint tool depends on seeing them.
3. Staged `PaintSurface` paints must be RE-TRIGGERED each scrub re-sim (they spawn real
   entities; scrub re-simulates from t=0) — from the core's final review.
4. `crates/arena_editor` needs its own `cargo update -p obelisk-bevy` (isolated workspace).
5. The `arena-skill-design` skill must gain `paints:` / `on_surface:` authoring docs.

## Hardening (post-merge, small)

6. ✅ **DONE 2026-07-11** (Task 2) — Exclude combatants from the spire ground-snap ray
   (`server/verbs.rs::skill_verbs_on_cue`, `frost_spire`/`on_window_spike` arm: the down-ray now
   excludes the caster + its hurtbox child, every skill object, AND every combatant body + its
   hurtbox children, so the ground ray only ever hits LEVEL geometry — a body standing on the fuel
   patch can no longer float the visual spire; damage capsule was always CastPoint-anchored). The
   glacier gate's assertion (5) proves ground-flush eruptions e2e; the combatant-exclusion
   branch is inspection-covered — no scripted scenario puts a body under an eruption).
7. Cache one `ForwardDecalMaterial` per surface type — BOTH copies: the arena's
   `client/surfaces.rs` AND the editor's `skill/preview/surfaces.rs::attach_patch_visuals`
   (same per-patch `materials.add` pattern; flagged again by the editor increment's final review).
   ✅ **ARENA HALF DONE 2026-07-11** (Task 2) — `client/surfaces.rs::attach_surface_visuals` now
   keys a `Local<HashMap<surface_id, Handle<ForwardDecalMaterial>>>` cache (one material per surface
   TYPE; the static-registry assumption + the hot-reload-invalidation caveat are commented at the
   use site). The EDITOR half (`skill/preview/surfaces.rs::attach_patch_visuals`, in
   `bevy_modal_editor`) is **Task 3** — NOT done here.
8. Decal projection depth (`Y scale 1.0`) vs elevated patches — a torso-hit/air-fuse burning
   patch can out-range the projection box (gameplay unaffected; `SURFACE_Y_TOLERANCE` covers
   standers). Grow the box or ground-snap paint positions.
   ✅ **ARENA HALF DONE 2026-07-11** (Task 2) — grew the box: the decal child's Y scale is now
   `y_span = (pos.y.abs() * 2.0 + 1.0).max(1.0)`, so an elevated patch's ±half-Y box still spans
   down to the floor (+1 m margin). The EDITOR preview's matching `attach_patch_visuals` grows the
   same way — ✅ **EDITOR HALF DONE 2026-07-11** (bevy_modal_editor `0abe936`; both #7 and #8 are
   now fully closed across both renderers).
9. `python3 -m py_compile crates/arena_game/tools/net-test/summarize.py` on the next
   python3-capable run (edited in lockstep with the jq gate but unexecuted in this shell).
   **DEFERRED — python3 = CI-only.** This dev shell is python3-free, so the jq `check_*.sh` gates
   are the local verdict and `summarize.py` gets compiled/run on the next (CI) python3-capable run.
9b. Editor increment's final-review small items: `StagedPaints` dedup-on-push guard (duplicate
    palette clicks accumulate identical entries — benign, merge-dedup collapses them);
    de-fragilize `stage_reset_rezeroes_surface_and_spawn_streams`' geometry-dependent
    `inside_prev` precondition (bevy_modal_editor tests/skill_preview.rs).
    ✅ **DONE 2026-07-11** — `push_staged_dedup` pure fn + unit test; the reset test paints at
    the caster's queried position (overlap by construction). bevy_modal_editor `0abe936`.

## Obelisk core (carried from the core's final review)

10. Spec-§11 `.trace` golden once surfaces enter obelisk's `feature_matrix()` (the two-run
    determinism test locks reproducibility, not behavior).
    ✅ **DONE 2026-07-11** — `surfaces_paint_stand_ignite` (feature_matrix #49: paint trail +
    standing tick + contact ignite + consume), byte-stable, all 48 prior goldens untouched.
    obelisk-bevy `1dc2a12`.
11. Cross-OBSERVER burst-evict residual (same-tick OnEnd/PaintSurface paints at cap are
    snapshot-blind; the system path is guarded).
    **DEFERRED** — a real fix needs a shared-tick-scratch design decision (dedup across observers
    within one tick), not yet made; the system path is already guarded, so this is a latent edge,
    not a live bug.
12. `SurfaceRemoveReason` has no wire mapping (Debug-only) — hand-map if a client ever needs
    removal REASONS rather than despawns.
    **DEFERRED (YAGNI)** — no consumer needs removal REASONS on the wire today; add the hand-map
    only when one does.
