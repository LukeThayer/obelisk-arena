# Surfaces — Editor Increment (6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make surfaces authorable and previewable in the skill designer: the `SurfaceRegistry`
loads from content roots, the preview stage runs the real surfaces sim (patches render as
decals+VFX, scrub/Reset deterministic), the Behavior panel authors `paints` and `on_surface`
with blocking validation, and a stage paint tool makes gated casts (frost_spire) testable
in-editor for the first time. Ends with the cross-repo bump into `arena_editor` and the
`arena-skill-design` skill docs.

**Architecture:** Tasks 1–5 live in **`~/src/bevy_modal_editor`** (the LukeThayer fork — the
Skill mode's home; `arena_editor` is a thin shell), branch **`surfaces-editor`**. Task 6 is the
obelisk-arena tail (editor-workspace `cargo update`, shell test, skill docs) and runs AFTER
pushing the editor's `main`. Spec: `docs/superpowers/specs/2026-07-09-surfaces-ground-effects-design.md`
§9 + `docs/superpowers/plans/2026-07-10-surfaces-followups.md` items 2–5 (both in obelisk-arena).

**Tech Stack:** Rust, Bevy 0.18, egui (the panel idiom: `egui::Grid` + `grid_label` +
`DragValue`/`ComboBox` — see `src/skill/panel/behavior.rs:243-292` for the exact house style),
obelisk-bevy @ cd860e2 (surfaces API), the editor's own headless test harness
(`tests/skill_preview.rs` — in-memory `SkillLibrary` fixtures + the real sim).

## Global Constraints

- Tasks 1–5 repo: `~/src/bevy_modal_editor`, branch **`surfaces-editor`** off `main` (create in
  Task 1). Task 6 repo: `~/src/obelisk-arena` (branch `surfaces-editor-shell` off `master`).
- obelisk-bevy is pinned `branch = "main"` in the editor's Cargo.toml — `cargo update -p
  obelisk-bevy` pulls `cd860e2`. stat_core stays `bf9f026` (obelisk-bevy still pins it — no
  lockstep bump).
- The obelisk bump makes `CollisionWindow` + `Acquisition::GroundPoint` literals non-exhaustive:
  known literal sites = `src/skill/templates.rs` (4× CollisionWindow + the zone template's
  GroundPoint), `src/skill/edits.rs` (5×), `src/skill/proxies.rs` (2×) — add `paints: None,` /
  `on_surface: None,` mechanically; the compiler finds any others.
- Editor suite gate per task: `cargo test` from the editor repo root (headless preview tests
  build a real render app — they are the regression net; keep them green).
- Match the panel's egui house style exactly (Grid + grid_label + end_row; pickers via
  `ComboBox::from_id_salt`; numeric `DragValue` with range+speed+suffix).
- Preview determinism: any state the surfaces sim mutates must reset in
  `PreviewStageReset::reset_stage` (`src/skill/preview/stage.rs:703`) — patches, `SurfaceSeq`,
  `StandingState`, and (the pre-existing P3 gap, fix it here) `SpawnRng`.
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

## File Structure

- bevy_modal_editor — Modify: `Cargo.lock` (bump), `src/skill/templates.rs`, `src/skill/edits.rs`,
  `src/skill/proxies.rs` (literal fix-ups), `src/skill/library.rs` (registry load + rescan),
  `src/skill/mod.rs` (resource init + validate call-site), `src/skill/preview/stage.rs`
  (sim plugin + reset), `src/editor/camera.rs` (DepthPrepass), `src/skill/panel/behavior.rs`
  (Paints + Require-surface), `src/skill/validation.rs` (+ its call site), 
  `src/ui/command_palette/skill_preset.rs` (stage-paint rows). Create:
  `src/skill/preview/surfaces.rs` (patch visuals + staged paints). Tests: `tests/skill_preview.rs`
  (append) + validation unit tests in `src/skill/validation.rs`.
- obelisk-arena — Modify: `crates/arena_editor/Cargo.lock` (bump), `crates/arena_editor/tests/*`
  (registry smoke assert), `.claude/skills/arena-skill-design/SKILL.md` (paints/on_surface docs).

---

### Task 1: Dep bump + SurfaceRegistry from content roots

**Files (bevy_modal_editor):**
- Modify: `Cargo.lock`, `src/skill/templates.rs`, `src/skill/edits.rs`, `src/skill/proxies.rs`,
  `src/skill/library.rs`, `src/skill/mod.rs`
- Test: `src/skill/library.rs` unit test (append to its `#[cfg(test)] mod`)

**Interfaces:**
- Consumes: obelisk `surfaces::{load_surfaces_dir, SurfaceRegistry, SurfaceType}` (registry =
  `SurfaceRegistry(pub HashMap<String, SurfaceType>)`).
- Produces (Tasks 2–5 rely on): the editor app carries `SurfaceRegistry` as a Resource, loaded
  from every content root's `config/surfaces/` (merge across roots, later roots win on id
  collision), refreshed by Rescan; `load_surfaces_from_dir(registry, dir)` helper in library.rs.

- [ ] **Step 0: Branch + bump**

```bash
cd ~/src/bevy_modal_editor && git checkout -b surfaces-editor && cargo update -p obelisk-bevy 2>&1 | tail -2
```
Expected: `79882ce/57662ef -> cd860e2`. Then `cargo check --all-features 2>&1 | grep -c "^error"`
— nonzero (the missing-field literals). Fix each reported `CollisionWindow` literal with
`paints: None,` and each `Acquisition::GroundPoint` literal with `on_surface: None,` (known
sites in Global Constraints; matches with `..` need nothing). Re-check → 0 errors.

- [ ] **Step 1: Write the failing test** (append inside `src/skill/library.rs` `mod tests`)

```rust
    /// (Surfaces) `scan_and_merge_root` loads `config/surfaces/*.toml` into the registry,
    /// merging across roots.
    #[test]
    fn scan_loads_surface_types_into_the_registry() {
        let dir = tempdir_for_test("surf_scan"); // follow the module's existing tempdir helper;
        // if none exists, use std::env::temp_dir() + a unique subdir like the sibling tests do.
        std::fs::create_dir_all(dir.join("config/surfaces")).unwrap();
        std::fs::write(
            dir.join("config/surfaces/frost.toml"),
            "id = \"frost\"\nlifetime = 180.0\n",
        )
        .unwrap();
        let mut skills = SkillLibrary::default();
        let mut effects = EffectLibrary::default();
        let mut vfx = VfxLibrary::default();
        let mut surfaces = obelisk_bevy::surfaces::SurfaceRegistry::default();
        scan_and_merge_root(&dir, &mut skills, &mut effects, &mut vfx, &mut surfaces);
        assert!(surfaces.0.contains_key("frost"));
        assert_eq!(surfaces.0["frost"].lifetime, 180.0);
    }
```
(Adapt the tempdir + default-construction lines to the module's existing test idiom — read the
sibling tests first; `SkillLibrary`/`EffectLibrary`/`VfxLibrary` construction must match how
`scan_root_pairs_rules_with_timelines` (library.rs:524) builds them.)

- [ ] **Step 2: Verify failure** — `cargo test -p bevy_modal_editor --features obelisk --lib library 2>&1 | tail -4`
(adapt the feature flag to how the crate's own CI invokes it — check `Cargo.toml` `[features]`;
if `obelisk` is the feature name, all skill-mode tests already require it). Expected: compile
fail (scan_and_merge_root has no 5th param).

- [ ] **Step 3: Implement**

`src/skill/library.rs`:
- Add to `scan_and_merge_root` (library.rs:283) a 5th param
  `surface_registry: &mut obelisk_bevy::surfaces::SurfaceRegistry` and, after the vfx loads:
```rust
    // Surfaces (ground effects): load `config/surfaces/*.toml` into the shared registry.
    // Skill-ref validation is deliberately skipped here (pass None) — the editor's own
    // ValidationRegistry (validate_skill) is the user-facing surface for dangling refs, same
    // split as the trigger_skill warn-don't-drop above. Later roots win on id collision
    // (BTreeMap-insert semantics via extend).
    match obelisk_bevy::surfaces::load_surfaces_dir(&root.join("config").join("surfaces"), None) {
        Ok(map) => surface_registry.0.extend(map),
        Err(e) => warn!("Failed to load surfaces from {:?}: {e}", root),
    }
```
  NOTE: `load_surfaces_dir` returns `Err` on a MISSING dir (its `read_dir` fails) — guard with
  `root.join("config/surfaces").is_dir()` first so rootless-surfaces content stays warning-free.
- `scan_registered_content_roots` (library.rs:310) gains
  `mut surface_registry: ResMut<obelisk_bevy::surfaces::SurfaceRegistry>` and passes it through.
- Find the Rescan palette path (it calls `scan_and_merge_root` — grep) and thread the registry
  there too.

`src/skill/mod.rs`: `init_resource::<obelisk_bevy::surfaces::SurfaceRegistry>()` beside the
other library resource inits (grep `init_resource::<SkillLibrary>` for the site).

- [ ] **Step 4: Verify pass + full suite + commit**

`cargo test 2>&1 | tail -4` (whole editor suite — the headless preview tests must stay green
under the bumped obelisk).
```bash
git add Cargo.lock src/skill/templates.rs src/skill/edits.rs src/skill/proxies.rs \
  src/skill/library.rs src/skill/mod.rs
git commit -m "feat(skill/surfaces): obelisk cd860e2 bump + SurfaceRegistry from content roots

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Preview stage runs the surfaces sim; deterministic reset (incl. the P3 SpawnRng fix)

**Files (bevy_modal_editor):**
- Modify: `src/skill/preview/stage.rs`
- Test: `tests/skill_preview.rs` (append)

**Interfaces:**
- Consumes: obelisk `surfaces::{ObeliskSurfacesPlugin, SurfacePatch, SurfaceSeq, PaintSurface}`,
  `surfaces::systems::StandingState` (exported via `obelisk_bevy::surfaces::StandingState`? —
  check the crate's re-exports; it is exported from `surfaces::systems`, re-exported at the
  `surfaces` root — grep `pub use systems::` in obelisk's surfaces/mod.rs and import accordingly),
  `core::spawn_rng::SpawnRng`.
- Produces: patches paint/decay/gate inside the preview sim; `reset_stage` clears patches and
  reseeds `SurfaceSeq`/`StandingState`/`SpawnRng` (fixed seeds — scrub's "same seed → identical"
  now holds for emitter AND surface content).

- [ ] **Step 1: Write the failing test** (append to `tests/skill_preview.rs` — mirror the file's
  existing fixture/harness idiom exactly; it inserts in-memory `SkillEntry` fixtures and drives
  the real stage):

Two tests:
1. `painting_skill_produces_patches_and_reset_clears_them` — insert a fixture skill whose single
   window authors `paints: Some(PaintSpec { surface: "frost".into(), radius: 0.45, mode:
   PaintMode::Trail { step: 0.5 }, lifetime: None })` with `Linear { speed: 8.0 }` motion and a
   frost `SurfaceType` inserted directly into the `SurfaceRegistry` resource (in-memory — no
   disk); Play it; advance ~1s of fixed ticks; assert `world.query::<&SurfacePatch>()` count >= 3;
   run `reset_stage` (the file has a precedent for invoking `PreviewStageReset` via
   `run_system_once` — see scrub.rs:337 for the exact call shape); assert count == 0.
2. `surface_scrub_restart_is_deterministic` — same fixture; Play, record the sorted patch
   positions (quantized `(p * 1000.) as i64`); reset + Play again with the same charge; assert
   the two position sets are IDENTICAL (this pins the SurfaceSeq/SpawnRng/StandingState resets).

- [ ] **Step 2: Verify failure** — the first test fails with 0 patches (plugin not composed).

- [ ] **Step 3: Implement** (`src/skill/preview/stage.rs`)

1. In `add_obelisk_sim` (stage.rs:308), after the `loot::ObeliskLootPlugin` add:
```rust
    // Surfaces (ground effects): paint/decay/contact/standing run in the preview sim exactly
    // as on a server (the plugin's systems live in ObeliskSet::Advance/ResolveHits, so the
    // EditorMode::Skill set gate above covers them for free).
    app.add_plugins(obelisk_bevy::surfaces::ObeliskSurfacesPlugin);
```
2. Extend `PreviewStageReset` (stage.rs:673): add
```rust
    patches: Query<'w, 's, Entity, With<obelisk_bevy::surfaces::SurfacePatch>>,
    surface_seq: ResMut<'w, obelisk_bevy::surfaces::SurfaceSeq>,
    standing: ResMut<'w, obelisk_bevy::surfaces::StandingState>,
    spawn_rng: ResMut<'w, obelisk_bevy::core::spawn_rng::SpawnRng>,
```
   and in `reset_stage` (after the hitbox despawn loop):
```rust
        // Surfaces: a reset returns the stage to bare ground and re-zeroes every stream the
        // surfaces sim draws from, so scrub's "same seed -> identical" holds for painted
        // content too. SpawnRng was previously NOT reseeded here (pre-surfaces gap): emitter
        // jitter differed on every scrub restart. 0x5EED_5EED mirrors seed_combat_rng(0)'s
        // derived spawn seed (seed ^ SPAWN_RNG_SEED_XOR with seed 0).
        for e in self.patches.iter() {
            self.commands.entity(e).try_despawn();
        }
        self.surface_seq.0 = 0;
        *self.standing = Default::default();
        self.spawn_rng.0 = rand_chacha::ChaCha8Rng::seed_from_u64(0x5EED_5EED);
```
   (Imports: `rand::SeedableRng`, `rand_chacha::ChaCha8Rng` — the editor already depends on
   these transitively; if not direct deps, add them matching obelisk's versions `rand 0.8` /
   `rand_chacha 0.3`. If `SpawnRng`'s inner field is private, use its public constructor or
   `*self.spawn_rng = SpawnRng(...)` — check obelisk's `core/spawn_rng.rs` and adapt; the field
   is `pub` there (`SpawnRng(pub ChaCha8Rng)`).)

- [ ] **Step 4: Verify pass + full suite + commit**

```bash
cargo test 2>&1 | tail -4
git add src/skill/preview/stage.rs tests/skill_preview.rs
git commit -m "feat(skill/surfaces): preview runs the surfaces sim; deterministic reset (patches + SpawnRng)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Preview patch visuals (decal + VFX) + DepthPrepass

**Files (bevy_modal_editor):**
- Create: `src/skill/preview/surfaces.rs`
- Modify: `src/skill/preview/mod.rs` (module + registration — find where cosmetics' systems
  register and mirror), `src/editor/camera.rs`
- Test: `tests/skill_preview.rs` (append)

**Interfaces:**
- Consumes: `SurfacePatch` (+ its `Transform`), `SurfaceRegistry` `[visuals]`
  (`SurfaceVisuals { decal, color, vfx }`), bevy `ForwardDecal`/`ForwardDecalMaterial<StandardMaterial>`,
  the preview's existing vfx-spawn helper (`src/skill/preview/cosmetics.rs` — grep how
  `spawn_cue_effect`/its equivalent resolves a `VfxLibrary` name and spawns the effect; REUSE
  that helper, hoisting visibility if needed, exactly like the arena did).
- Produces: every live patch in the stage renders a tinted decal child (+ looping vfx child when
  authored); children despawn with the patch (obelisk despawns are plain `despawn`, recursive).

- [ ] **Step 1: Write the failing test** (append to `tests/skill_preview.rs`)

`patches_get_decal_visual_children`: reuse Task 2's painting fixture; Play; advance; find a
`SurfacePatch` entity; assert it has at least one child carrying `ForwardDecal` (the headless
harness builds a real render app per the repo's own docs, so pbr components are constructible;
if `ForwardDecal`'s on-add hook requires render resources absent headlessly and panics, fall
back to asserting the child carries the plugin's own marker component — define
`SurfacePatchVisual` in the new module and assert THAT instead; note which assertion shipped).

- [ ] **Step 2: Verify failure** — no children.

- [ ] **Step 3: Implement**

`src/skill/preview/surfaces.rs` (mirror the arena's `client/surfaces.rs` shape, adapted to the
preview: patches are LOCAL sim entities with `Transform` already — no Position mirror needed):
- `SurfacePatchVisual` marker component.
- `attach_patch_visuals` system (`Update`, `.run_if(in_state(EditorMode::Skill))`): on
  `Added<SurfacePatch>`, look up `SurfaceRegistry` visuals (Option-safe, defaults = white 0.8 +
  `"textures/decal_splat.png"`), spawn a child with `SurfacePatchVisual` + `ForwardDecal` +
  `MeshMaterial3d(ForwardDecalMaterial { base: StandardMaterial { base_color: tint,
  base_color_texture: asset_server.load(texture), alpha_mode: Blend, perceptual_roughness: 1.0,
  .. }, extension: ForwardDecalMaterialExt { depth_fade_factor: 1.0 } })` +
  `Transform::from_scale(Vec3::new(r * 2.0, 1.0, r * 2.0))`; guard the
  `MaterialPlugin::<ForwardDecalMaterial<StandardMaterial>>` registration with
  `is_plugin_added` in this module's plugin/registration fn (same as the arena).
- VFX child: if `visuals.vfx` names a `VfxLibrary` key, spawn via the preview's existing
  effect-spawn helper as a looping child (no lifetime bound — dies with the patch). If the
  helper's shape resists parenting, spawn at the patch translation with a small follow/cleanup
  note in the report.
- Register in `src/skill/preview/mod.rs` beside the cosmetics systems.

`src/editor/camera.rs` (~line 163, the `Camera3d::default()` spawn tuple): add
```rust
        // ForwardDecal (surface patches, skill preview) samples the depth prepass to project
        // onto the stage floor.
        bevy::core_pipeline::prepass::DepthPrepass,
```

- [ ] **Step 4: Verify pass + full suite + commit**

```bash
cargo test 2>&1 | tail -4
git add src/skill/preview/surfaces.rs src/skill/preview/mod.rs src/editor/camera.rs tests/skill_preview.rs
git commit -m "feat(skill/surfaces): preview patch decal+vfx visuals; DepthPrepass on the editor camera

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Authoring UI — Paints section + Require-surface + blocking validation

**Files (bevy_modal_editor):**
- Modify: `src/skill/panel/behavior.rs`, `src/skill/validation.rs` (+ the `validate_skill`
  call site in `src/skill/mod.rs`)
- Test: validation unit tests in `src/skill/validation.rs` `mod tests` (follow its precedent)

**Interfaces:**
- Consumes: `PaintSpec`/`PaintMode`/`SurfaceRequirement` (obelisk assets), `SurfaceRegistry`
  (picker source + validation), the panel idiom (behavior.rs:243-292).
- Produces: in-UI authoring of `window.paints` and `GroundPoint.on_surface`;
  `validate_skill` gains a `surfaces: &SurfaceRegistry` param with new BLOCKING rules.

- [ ] **Step 1: Validation TDD first** (append tests in `src/skill/validation.rs`)

Follow the module's existing test style (construct a `SkillEntry` fixture, call
`validate_skill`, assert on `problems`). New rules to pin:
- window `paints.surface` unknown to the registry → blocking, target `format!("window:{id}")`
  (match the module's existing window-target convention — grep how window-scoped problems are
  targeted; if none exist, use `window:{index}` mirroring `condition:{i}`).
- `on_surface.surface` unknown → blocking, target `"acquisition"`.
- Mirror obelisk's `validate_timeline` numeric paint rules as PRE-SAVE blocking (radius <= 0,
  Trail step <= 0, lifetime override <= 0, empty surface id) — same rationale as the existing
  charge-tier mirror block (validation.rs:222-236: "a violating file fails the game asset load").
- Known surface + all numerics valid → no problems.

Run → compile-fail (no `surfaces` param).

- [ ] **Step 2: Implement validation**

`validate_skill` (validation.rs:117) gains `surfaces: &obelisk_bevy::surfaces::SurfaceRegistry`;
add the two lookup rules + the numeric mirror block (place beside the charge-tier mirror).
Update the call site (`src/skill/mod.rs` — grep `validate_skill(`) to pass the resource
(`Res<SurfaceRegistry>` is available after Task 1). Tests green.

- [ ] **Step 3: Panel UI**

`src/skill/panel/behavior.rs`:
- **Paints section** on the window card (place after the emitter section — grep the emitter
  rows for the exact card structure): an enable `ui.checkbox(&mut has_paints, "Paints surface")`
  toggling `window.paints` between `None` and
  `Some(PaintSpec { surface: <first registry id or "frost">, radius: 0.45, mode: PaintMode::OnEnd, lifetime: None })`;
  when Some — Grid rows: `Surface` = ComboBox over `SurfaceRegistry` ids (sorted; free-text is
  NOT offered — the registry is the source of truth, unknown ids are blocked anyway); `Radius` =
  DragValue `0.05..=10.0` speed 0.05 suffix " m"; `Mode` = ComboBox `OnEnd` / `Trail…` (Trail
  reveals a `Step` DragValue `0.1..=10.0` speed 0.05 suffix " m"); `Lifetime` = optional
  override (checkbox + DragValue secs, mirroring however the panel handles `rehit_interval`'s
  Option — grep it and copy the idiom).
- **Require-surface** in `draw_acquisition_card`'s `GroundPoint` arm (behavior.rs:272-278) AND
  the fallback-inner GroundPoint arm (behavior.rs:354-360 — same rows, keyed ids): after Range —
  `On surface` checkbox toggling `on_surface` None ↔
  `Some(SurfaceRequirement { surface: <first registry id>, snap: true, consume: false })`;
  when Some — `Surface` ComboBox (registry ids) + `Snap` checkbox + `Consume` checkbox rows.
- Both sections take the registry: `draw_behavior_region`'s signature chain must thread
  `&SurfaceRegistry` down from the panel's system (grep how `EffectLibrary` reaches
  presentation.rs's pickers and copy that threading pattern).
- Every mutation sets the same `changed` flag the surrounding card uses (dirty-tracking).

- [ ] **Step 4: Verify + commit**

`cargo test 2>&1 | tail -4` (validation tests + whole suite; the UI itself is exercised by
compile + the existing panel smoke coverage).
```bash
git add src/skill/panel/behavior.rs src/skill/validation.rs src/skill/mod.rs
git commit -m "feat(skill/surfaces): author paints + on_surface in the Behavior panel; blocking validation

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Stage paint tool (staged paints survive reset/scrub — frost_spire testable)

**Files (bevy_modal_editor):**
- Modify: `src/skill/preview/surfaces.rs` (StagedPaints), `src/skill/preview/stage.rs`
  (re-apply in reset), `src/ui/command_palette/skill_preset.rs` (palette rows)
- Test: `tests/skill_preview.rs` (append — the flagship test)

**Interfaces:**
- Consumes: `PaintSurface` (obelisk's public paint request — the pre-built editor seam),
  `SurfaceRegistry` (row source), the palette row pattern (`SkillRow` enum,
  skill_preset.rs:22-120), `PreviewStageReset`.
- Produces: `StagedPaints(pub Vec<StagedPaint>)` resource (`StagedPaint { surface: String,
  position: Vec3 }`); palette rows "Stage: paint <surface> patch" (one per registry id) +
  "Stage: clear staged paints"; every `reset_stage` re-applies staged paints AFTER clearing —
  so Play, editor Reset, and scrub restart all start from the same staged ground.

- [ ] **Step 1: Write the flagship failing test** (append to `tests/skill_preview.rs`)

`staged_frost_makes_a_gated_cast_succeed`:
- Fixture skill `spire_probe`: rules = trivial damage; timeline = one Static `CastPoint`-anchored
  window, `acquisition: GroundPoint { range: 60.0, fallback: AcqFallback::Fizzle, on_surface:
  Some(SurfaceRequirement { surface: "frost".into(), snap: true, consume: true }) }`. Insert a
  frost `SurfaceType` into the registry (in-memory).
- WITHOUT staging: Play → assert the cast was REJECTED (the harness observes `CastRejected` —
  grep the file for its existing CastBegan/CastRejected recorder idiom; `resolve_stage_acquisition`
  pre-resolves a Point at the stage ground marker, so the sim-side on_surface check is what
  rejects).
- Stage a frost paint AT the stage's ground-point marker position (grep `resolve_stage_acquisition`
  / `StageAimContext` for where a GroundPoint stage cast aims — use THAT position so the gate
  matches): push into `StagedPaints`, run the re-apply (reset), Play → assert `CastBegan` AND
  the patch was CONSUMED (patch count back to 0 after the cast, while a re-reset re-stages it).

`staged_paints_survive_scrub_restart`: stage one paint; reset twice; assert exactly one patch
exists after each reset (re-applied, not duplicated — the reset clears first).

- [ ] **Step 2: Verify failure.**

- [ ] **Step 3: Implement**

`src/skill/preview/surfaces.rs`:
```rust
/// Session-scoped staged ground state (spec §9 / D12's stage-setup direction): pre-painted
/// patches the designer placed via the palette, re-applied on EVERY stage reset (Play, editor
/// Reset, scrub restart re-sim from t=0) so gated casts (frost_spire) are testable and the
/// scrubber stays honest. Never serialized — pure session state.
#[derive(Resource, Default)]
pub struct StagedPaints(pub Vec<StagedPaint>);

#[derive(Clone, Debug)]
pub struct StagedPaint {
    pub surface: String,
    pub position: Vec3,
}
```
`stage.rs`: `PreviewStageReset` gains `staged: Res<'w, StagedPaints>`; at the END of
`reset_stage` (after the surfaces clearing added in Task 2), re-apply:
```rust
        // Staged paints re-apply AFTER the clear — deterministic pre-state for every replay.
        // Owner = the preview caster (faction snapshot at paint; the caster always exists once
        // the stage is built; a stage-less reset just no-ops the trigger next tick).
        if let Some((caster, _)) = self.casters.iter().next() {
            for staged in &self.staged.0 {
                self.commands.trigger(obelisk_bevy::surfaces::PaintSurface {
                    surface: staged.surface.clone(),
                    position: staged.position,
                    owner: caster,
                });
            }
        }
```
(`PaintSurface` is an observer-trigger — obelisk's `on_paint_surface` handles dedup/caps; the
commands-trigger fires when the reset's commands flush, same tick semantics as the despawns.)

`skill_preset.rs`: extend `SkillRow` with `StagePaint(String)` and `StageClearPaints`; rows
appear only when the registry is non-empty (mirror the `always_visible`/`is_enabled` pattern);
`StagePaint(surface)` pushes `StagedPaint { surface, position: <the stage ground-aim point> }`
into `StagedPaints` and immediately triggers the same `PaintSurface` (so the patch appears
without waiting for a reset); `StageClearPaints` clears the resource and despawns live patches.
The stage ground-aim point: reuse the SAME constant/position `resolve_stage_acquisition` uses
for `GroundPoint` stage casts (grep it — likely a ground marker near the dummy) so a staged
patch is exactly where a gated cast will aim.

- [ ] **Step 4: Verify + commit**

```bash
cargo test 2>&1 | tail -4
git add src/skill/preview/surfaces.rs src/skill/preview/stage.rs src/ui/command_palette/skill_preset.rs tests/skill_preview.rs
git commit -m "feat(skill/surfaces): stage paint tool — staged patches survive reset/scrub; gated casts testable

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Cross-repo finish — push, arena_editor bump, skill docs

**Files:**
- bevy_modal_editor: none (push only — coordinated by the controller, NOT this task's agent;
  the agent starts from the already-pushed state).
- obelisk-arena: Modify `crates/arena_editor/Cargo.lock`, the arena_editor smoke test
  (`crates/arena_editor/tests/` — find the test asserting on `SkillLibrary` after
  `build_editor_app`; append a `SurfaceRegistry` assert), `.claude/skills/arena-skill-design/SKILL.md`.

**Interfaces:**
- Consumes: pushed bevy_modal_editor `main` + obelisk-bevy `cd860e2`.
- Produces: the editor shell builds against the new editor with surfaces loaded from the arena
  root; the authoring guide teaches `paints`/`on_surface`.

- [ ] **Step 0 (controller, before dispatch):** merge `surfaces-editor` → editor `main`, push.

- [ ] **Step 1: Bump the editor workspace** (obelisk-arena branch `surfaces-editor-shell`)

```bash
cd ~/src/obelisk-arena && git checkout -b surfaces-editor-shell
cd crates/arena_editor && cargo update -p bevy_modal_editor -p bevy_editor_game -p obelisk-bevy 2>&1 | tail -4
cargo check 2>&1 | tail -3
```
(All three pins are `branch = "main"` git deps in THIS crate's own workspace — never `-p` from
the repo root.)

- [ ] **Step 2: Registry smoke assert**

In the arena_editor test that builds `build_editor_app` and asserts `SkillLibrary` contents
(grep `SkillLibrary` under `crates/arena_editor/tests/`), append after its existing asserts:
```rust
    // Surfaces (increment 6): the content root's config/surfaces loads into the registry.
    let surfaces = app.world().resource::<obelisk_bevy::surfaces::SurfaceRegistry>();
    assert!(surfaces.0.contains_key("frost"), "frost surface loaded from the arena root");
    assert!(surfaces.0.contains_key("burning"));
```
(Remember `register_obelisk_content` only QUEUES roots — the existing test already runs
`app.update()` before asserting; keep the assert after that point.)
Run: `cd crates/arena_editor && cargo test 2>&1 | tail -4` (editor workspace — own lockfile).

- [ ] **Step 3: Skill docs** (`.claude/skills/arena-skill-design/SKILL.md`)

Surgical additions matching the doc's voice:
- §1 "Expressible today": add persistent painted surfaces (paint/require/consume + standing +
  contact) to the expressible list; REMOVE "bespoke mechanics (frost tiles)" from the
  server-verbs bullet's examples where frost tiles are cited (portals/spawned objects remain).
- §3b field-reference table: add the `paints` row
  (`Some(( surface, radius, mode: Trail(step)|OnEnd, lifetime: Some(secs)? ))`) and note
  `GroundPoint`'s `on_surface: Some(( surface, snap, consume ))`.
- §3b add a fifth archetype snippet "surface-gated consumer" (frost_spire's real acquisition
  line) and extend Archetype 1's causality note: a triggered sub-skill can `paints: OnEnd`
  (firebolt_explosion's real line).
- New §3d "Surfaces — `config/surfaces/<id>.toml`": the schema block (copy
  `config/surfaces/burning.toml` verbatim + one line per field family: standing payloads,
  on_skill_contact, [visuals] decal/color/vfx), loader fail-loud note, and the editor notes
  (registry-fed pickers; stage paint tool for testing gated casts).
- §4 verify: add "stage paint tool: palette → 'Stage: paint frost patch' before Playing a gated
  skill".
- §5 pitfalls: add — paints/on_surface surface ids must exist in `config/surfaces/` (editor
  blocks, loader warns-and-skips paint / silently gates); contact tags must be real SkillTags
  (loader rejects); chained/triggered/standing hits are mana-free (budget note already exists —
  extend with surfaces).

- [ ] **Step 4: Commit**

```bash
cd ~/src/obelisk-arena && git add crates/arena_editor/Cargo.lock crates/arena_editor/tests \
  .claude/skills/arena-skill-design/SKILL.md
git commit -m "feat(editor): surfaces in the designer — workspace bump, registry smoke, skill docs

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```
(Adjust the `git add` test path to the actual test file touched. NEVER `git add -A` — user WIP
in the tree.)

---

## Post-plan notes (controller)

- Manual visual pass after Task 6 (controller or user): `cd crates/arena_editor && cargo run
  --bin arena-editor`, K → open frost_spire → palette "Stage: paint frost patch" → Play; and
  firebolt_explosion scrub shows the scorch decal. Screenshot-worthy but human-judged.
- Remaining follow-ups (unchanged, tracked in 2026-07-10-surfaces-followups.md): glacier/spire
  e2e net-test, material caching, ray hardening, decal depth, obelisk feature_matrix golden.
