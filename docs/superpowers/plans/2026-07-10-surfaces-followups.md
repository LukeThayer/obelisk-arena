# Surfaces — Tracked Follow-ups (post arena increment)

Filed 2026-07-10 at the `surfaces-arena` final review's direction (verdict: ready to merge,
conditioned on filing these — the coverage gap must not live only in the SDD ledger).

## Required (the merge condition)

1. **Glacier/spire e2e net-test extension.** The gate only casts firebolt, so `Trail` painting,
   `on_surface` gate/snap/CONSUME, and the spire eruption have no automated runtime coverage —
   the one real bug of the increment (the spire Y-float) lived exactly in this blind spot.
   Extend the net-test session (or add a second scripted session) to: equip `potted_spring`,
   cast `rolling_glacier` (assert `surface_painted{frost}` on the server + replication on both
   observers), cast `frost_spire` at the trail (assert `surface_removed{reason:"Consumed"}` +
   a ground-flush `spire_erupted` — `pos[1] ≈ 0.0`), and one `burning_ground_tick`
   `server_net_damage_resolved` from a victim standing in a scorch. Also assert the D9
   round-reset clear when feasible.

## Editor increment 6 (pre-warned footguns, fold into that plan)

2. `DepthPrepass` is main-camera-only (`client/scene.rs`) — the editor preview/stage cameras
   render NO decals until they add it; the D5 stage-paint tool depends on seeing them.
3. Staged `PaintSurface` paints must be RE-TRIGGERED each scrub re-sim (they spawn real
   entities; scrub re-simulates from t=0) — from the core's final review.
4. `crates/arena_editor` needs its own `cargo update -p obelisk-bevy` (isolated workspace).
5. The `arena-skill-design` skill must gain `paints:` / `on_surface:` authoring docs.

## Hardening (post-merge, small)

6. Exclude combatants from the spire ground-snap ray (`server/verbs.rs` — a body standing on
   the fuel patch can float the visual spire; damage capsule unaffected).
7. Cache one `ForwardDecalMaterial` per surface type (`client/surfaces.rs` currently allocates
   per patch — a frost roll churns ~50 identical materials).
8. Decal projection depth (`Y scale 1.0`) vs elevated patches — a torso-hit/air-fuse burning
   patch can out-range the projection box (gameplay unaffected; `SURFACE_Y_TOLERANCE` covers
   standers). Grow the box or ground-snap paint positions.
9. `python3 -m py_compile crates/arena_game/tools/net-test/summarize.py` on the next
   python3-capable run (edited in lockstep with the jq gate but unexecuted in this shell).

## Obelisk core (carried from the core's final review)

10. Spec-§11 `.trace` golden once surfaces enter obelisk's `feature_matrix()` (the two-run
    determinism test locks reproducibility, not behavior).
11. Cross-OBSERVER burst-evict residual (same-tick OnEnd/PaintSurface paints at cap are
    snapshot-blind; the system path is guarded).
12. `SurfaceRemoveReason` has no wire mapping (Debug-only) — hand-map if a client ever needs
    removal REASONS rather than despawns.
