# Surfaces — Decal Ground-Snap: fix the floating/artifacting patches + editor dummy overlay

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the two reported render bugs — game frost patches floating at Y≈0.35 with
angle-dependent smearing, and editor decals drawing on top of the dummy — by ground-snapping the
decal + VFX children at attach time in BOTH renderers and deleting the inert `y_span` knob.

**Architecture:** Render-layer-only fix; NO obelisk, wire, or golden changes. The sim patch
keeps its authored Y (gameplay uses `SURFACE_Y_TOLERANCE`); only the visuals conform to the
ground, mirroring the spire-eruption ground-snap precedent. Task 1 in `~/src/bevy_modal_editor`
(has the headless preview harness = the TDD witness), Task 2 in `~/src/obelisk-arena` (verbatim
mirror + the `crates/arena_editor` lockfile bump so the user's editor binary picks up Task 1).
Sequential: Task 2 bumps to Task 1's pushed rev.

## Root cause (investigated, file:line evidence in the session)

bevy 0.18 `ForwardDecal` (bevy_pbr `decal/forward.rs` + `forward_decal.wgsl`):
- The decal mesh is a FLAT 1×1 quad rotated to face +Y (`forward.rs:32-39`) — there is no
  projection box. **`scale.y` is inert** (the polish-round `y_span` growth was a no-op).
- `depth_compare = Always` (`forward.rs:128`): the quad is never occluded by nearer opaque
  geometry — it draws OVER a character standing in its screen footprint.
- It projects onto whatever the depth prepass saw at each pixel — floor OR character; there is
  **no receiver masking**. `depth_fade_factor` (ours: 1.0 ⇒ 1 m) is the real projection bound:
  `alpha = saturate(1 - normal_depth/depth_fade_factor)` (`forward_decal.wgsl:44-49`).
- Parallax UV reconstruction smears at grazing view angles proportionally to `normal_depth`
  (the quad's elevation above the receiving surface); back-face culling hides it from below.

Data flow: `glacier_roll.cast.ron` rolls its trail-painting hitbox at `anchor_offset (0,0.35,0)`
with `Linear`/`Horizontal` motion (constant Y); obelisk records the hitbox translation verbatim
(`obelisk-bevy src/surfaces/systems.rs:76` → `patch.rs:196`); the server stamps that as the
replicated `Position` verbatim; the client renders the patch (and its decal child at local
translation 0) at that Y verbatim (`client/surfaces.rs:85-87,122`). Result: a quad hovering at
0.35 m (the float + smear), and in the editor the same quad reality paints full-alpha across the
dummy (elevated frost at shin height; torso-hit scorch at body height within the 1 m fade).

**One fix serves both:** with the quad flush on the floor, `normal_depth` for a standing
character's torso pixels exceeds the 1 m fade ⇒ alpha ≈ 0 (no overlay; feet/ankles keep a faint
tint — the natural "standing in it" look), and the float/smear vanish (elevation ⇒ 0). The
residual feet-tint is intrinsic to ForwardDecal (no receiver masking) and is ACCEPTED here.

## Global Constraints

- obelisk-arena tree carries USER WIP (`assets/skills/blizzard.cast.ron`, `assets/vfx/*` mods,
  untracked `Particle_ *`) — **NEVER `git add -A`** there; stage exact paths only. Same staging
  discipline in bevy_modal_editor.
- bevy_modal_editor pushes to the **`lukethayer` remote, never `origin`**.
- `crates/arena_editor` is its OWN cargo workspace — run cargo from that directory.
- NO obelisk-bevy changes; NO wire/protocol changes; goldens untouched.
- The final acceptance gate is the USER's visual pass (flush patches, no dummy overlay) — code
  merges do not close the bug; say so in reports.
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

---

### Task 1 (bevy_modal_editor, branch `surfaces-decal-snap`): ground-snap the preview decals

**Files:**
- Modify: `src/skill/preview/surfaces.rs` (`attach_patch_visuals`)
- Test: `tests/skill_preview.rs`

**Interfaces:** `attach_patch_visuals` gains a `SpatialQuery` system param (avian; already used
elsewhere in the preview — `stage.rs:248`). The stage floor is a STATIC cuboid whose top face is
world Y = 0 (`stage.rs:98-101`, marker `StageFloor`).

- [ ] **Step 1: Write the failing test** (append to `tests/skill_preview.rs`, follow the file's
  existing harness idioms for app construction + painting — e.g. the
  `frozen_scrub_reset_reapplies_the_staged_patch` / `stage_reset_rezeroes_surface_and_spawn_streams`
  setup). Paint a patch at an ELEVATED position, then assert the decal child renders flush:

```rust
/// An elevated paint (glacier trails ride at hitbox height ≈0.35) must still render its decal
/// flush on the stage floor: bevy 0.18 ForwardDecal is a flat quad — an elevated quad floats,
/// smears with view angle, and overlays characters instead of projecting onto the ground.
#[test]
fn elevated_patch_decal_snaps_to_stage_floor() {
    // harness: build the preview app exactly like the sibling surface tests, then:
    // trigger PaintSurface { surface: "frost", position: Vec3::new(0.0, 0.35, 2.0), owner } and
    // run updates until the patch's decal child exists (same polling the sibling tests use).
    // Query: the ForwardDecal child of the SurfacePatch entity, take its GlobalTransform.
    let decal_y = /* decal child GlobalTransform.translation().y */;
    assert!(
        (decal_y - 0.01).abs() < 0.05,
        "decal must sit flush on the floor (top face y=0, +0.01 bias), got y={decal_y}"
    );
}
```

- [ ] **Step 2: Run it — must FAIL** with `decal_y ≈ 0.35` (today the child inherits the patch
  Y verbatim). `cd ~/src/bevy_modal_editor && cargo test --features obelisk elevated_patch_decal_snaps_to_stage_floor`

- [ ] **Step 3: Implement** in `attach_patch_visuals`:
  - Add `spatial: SpatialQuery` to the system params.
  - Before spawning the children, resolve the ground under the patch:

```rust
    // bevy 0.18 ForwardDecal is a FLAT +Y quad: scale.y is inert, depth_compare=Always (never
    // occluded), and depth_fade_factor bounds the projection (1.0 => 1 m). An elevated quad
    // therefore floats, parallax-smears at grazing angles, and draws over characters standing
    // in it. Snap the visual to the ground so only sub-1m receivers (floor, feet) catch it;
    // the SIM patch keeps its authored Y (gameplay is SURFACE_Y_TOLERANCE-based).
    let origin = patch_pos + Vec3::Y * 2.0;
    let ground_y = spatial
        .cast_ray_predicate(
            origin,
            Dir3::NEG_Y,
            50.0,
            true,
            &SpatialQueryFilter::default(),
            &|entity| floor_query.contains(entity), // STATIC stage geometry only (StageFloor)
        )
        .map(|hit| origin.y - hit.distance)
        .unwrap_or(0.0); // flat-stage fallback: floor top face is world Y = 0
    let visual_y = ground_y - patch_pos.y + 0.01; // child-local: lift 1 cm off the floor
```

  (Adapt names to the function's actuals: `floor_query: Query<(), With<StageFloor>>` — positive
  filter on the floor marker beats an exclusion list here; avian API signatures per the pinned
  0.18-compatible avian — verify `cast_ray_predicate`'s exact parameter shape against its docs
  in the vendored source.)
  - Decal child: `Transform::from_xyz(0.0, visual_y, 0.0).with_scale(Vec3::new(p.radius * 2.0, 1.0, p.radius * 2.0))`
    — the `y_span` computation is DELETED (inert; keep a one-line comment pointing at the quad
    reality so it doesn't come back).
  - VFX child: apply the same `visual_y` translation so embers sit on the ground, not at 0.35.

- [ ] **Step 4: Run the new test — PASS**; then the full suite:
  `cargo test --features obelisk` (update any existing test that asserted the old `y_span`
  scale honestly — the assertions should now expect scale.y == 1.0 / the snapped child Y).

- [ ] **Step 5: Commit** (surfaces.rs + tests):
```
fix(skill/surfaces): decals ground-snap at attach — bevy 0.18 ForwardDecal is a flat quad (y_span was inert)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```
- [ ] **Step 6 — controller merges + pushes `lukethayer main`** (not this agent).

---

### Task 2 (obelisk-arena, branch `surfaces-decal-snap`): mirror in the game client + editor bump

**Files:**
- Modify: `crates/arena_game/src/client/surfaces.rs` (`attach_surface_visuals`),
  `crates/arena_editor/Cargo.lock` (bump), `docs/superpowers/plans/2026-07-10-surfaces-followups.md`.

**Interfaces:** consumes Task 1's pushed bevy_modal_editor rev. The arena client has
`SpatialQuery` (`client/net.rs:272`); level colliders are baked STATIC geometry.

- [ ] **Step 1: Mirror Task 1 exactly** in `attach_surface_visuals` — same comment block, same
  raycast + `visual_y` math, same scale.y = 1.0 deletion of `y_span`, same VFX-child snap. One
  deviation: the client has no `StageFloor` marker — the predicate accepts only STATIC colliders
  (look up `RigidBody::Static` on the hit entity via a query in the predicate closure), so
  characters/skill objects standing on the paint point can never ground the visual (the spire
  ray's exclusion principle). Fallback `0.0` (arena_flat floor; the glacier gate's spire
  assertion (5) already pins that plane).
- [ ] **Step 2: Verify arena**: `cargo test -p arena_game`; then ONE glacier gate run
  (`ARENA_SKIP_BUILD` unset — surfaces.rs changed, rebuild needed; pkill -x between; ≤3
  retries): the gate exercises the attach path headlessly (assertions (1)-(9) must stay green).
- [ ] **Step 3: Bump the editor workspace** so the user's visual re-check gets Task 1:
  `cd ~/src/obelisk-arena/crates/arena_editor && cargo update -p bevy_modal_editor && cargo test`
  (own workspace; verify the dep's actual crate name in its Cargo.toml first).
- [ ] **Step 4: Annotate** `docs/superpowers/plans/2026-07-10-surfaces-followups.md` item #8:
  the `y_span` box-growth halves are INERT under bevy 0.18 (flat quad — scale.y unused) and are
  SUPERSEDED by the decal ground-snap (this plan); keep the item's history, append the
  correction.
- [ ] **Step 5: Commit** (exact paths: `crates/arena_game/src/client/surfaces.rs`,
  `crates/arena_editor/Cargo.lock`, the followups doc):
```
fix(client/surfaces): decals ground-snap at attach (mirror editor); y_span was inert under bevy 0.18

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

## Post-plan notes (controller)

- Merge + push per the standing pattern after each task's review.
- The bug CLOSES only on the user's visual re-check: game — frost trail flush + stable from all
  angles; editor — no decal overlay on the dummy (feet tint acceptable). If feet-tint is deemed
  unacceptable, the escalation is a custom decal material variant (depth_compare Normal /
  receiver masking) — explicitly OUT of this plan (YAGNI until the user judges the result).
