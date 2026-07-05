# Phase 4 Arena Migration — Follow-ups, Accepted Scope, and Handoff

Companion to the implementation plan (`obelisk-bevy/docs/superpowers/plans/2026-07-05-phase4-arena-migration.md`). Records what this migration pass deliberately deferred, the scope it accepted, and the steps to make the branch self-consistent.

Branch state at time of writing: arena `feat/phase4-arena-migration` (8 commits off `master` `f6472e4`), obelisk-bevy `feat/cue-skill-id` (1 commit off `main`, adds `CueEvent.skill_id`). The spine — A1, C1, C1b, C2, C4, C5, C6, C7 — is complete; C3 and C8 are deferred (see below).

## Whole-branch review outcome

A whole-branch review (Opus 4.8) returned **READY TO MERGE** for the game/server/net-test deliverable — no Critical or Important findings. It independently re-diffed the entire `add_obelisk_sim` against the upstream `ObeliskSimPlugin::build` and confirmed it is now fully faithful except the two intentional divergences (the C1b fix is complete, nothing else missing), and independently verified: the acquisition rewrite does not double-validate; the cue wire matches the contract exactly; and the firebolt trigger fires **exactly one explosion per ending** (HitEntity→`always`, HitWorld→`on_impact`, Fuse→`on_expire` are mutually exclusive — no double-AoE). The one recommended pre-merge fix (M1: cast-pipeline prose still described the pre-reform free-aim flow, at 7 doc sites) is **fixed** on this branch. The remaining findings are the deferred-C8 editor breakage and the Minor test/dedup nits below.

## Deferred tasks (gated on prerequisite P1)

**P1 = the `skill-mode` branch merged into the user's `bevy_modal_editor` main.** `bevy_effect` and the built-in `EditorMode::Skill` exist only on that unmerged branch, and the arena pins `bevy_vfx`/`bevy_modal_editor` at `branch = "main"`.

- **C3 — client cue rendering via `bevy_effect`.** On this branch, client cue *rendering* is intentionally STUBBED (`spawn_cue_cosmetics` only despawns `OnEnd`-bound cosmetic projectiles + traces `cue_dispatch`). The wire carries everything C3 needs (`skill_id`/`cue_id`/`charge`/`end_reason`). The kept-but-unused scaffolding in `client/cosmetics.rs` (`CosmeticAssets`, `MUZZLE_HEIGHT_OFFSET`, `init_cosmetic_assets`, `AimDirs`) is for C3 to resume writing into. The headless net-test does not render, so it is unaffected. Brief ready.
- **C8 — thin `arena_editor` to a host shell.** Repin `bevy_modal_editor` to the skill-mode-merged main, enable the `obelisk` feature, and collapse `main.rs` to `DefaultPlugins + EditorPlugin(obelisk) + register_obelisk_content + add_obelisk_effects(config/effects) + character.glb + PhysicsGizmos`, deleting the ~24 designer modules. Brief ready.

Both briefs are in the session scratchpad; execute them once P1 lands.

## Handoff steps (make the branch build standalone)

This pass builds the arena against the LOCAL obelisk-bevy sibling (branch `feat/cue-skill-id`) via a gitignored `.cargo/config.toml` `[patch]` — so the committed branch does NOT build standalone yet. To reconcile:

1. Merge obelisk-bevy `feat/cue-skill-id` (the `CueEvent.skill_id` addition, already reviewed) → obelisk-bevy `main`.
2. In the arena: `cargo update -p obelisk-bevy` (picks up the merged `main`), remove the gitignored `.cargo/config.toml` patch, and commit the synced `Cargo.lock`.
3. (`stat_core` is already pinned to the exact rev `bf9f026` the sim uses — see the C1 commit; keep it in lockstep with obelisk-bevy's own `stat_core` pin on future bumps.)
4. After P1: execute C3 + C8.

## Discovered during execution

- **Sim-composition drift — fixed as C1b, but the fragility remains.** `crates/arena_sim/src/obelisk.rs::add_obelisk_sim` is a HAND-COPY of obelisk-bevy's `ObeliskSimPlugin::build`. The trigger reform added `advance_triggered_execs` (the executor that ticks a rules-triggered skill's timeline) and `tick_emitters` to the `Advance` set; the arena's copy predated them and silently dropped both — it compiled clean, but triggered skills (firebolt_explosion) and emitters (blizzard) were dead at runtime. This was caught by a controller **net-test dry-run**, NOT by any per-task build or unit gate — a stale hand-copy is invisible to both. **Ticket:** de-fragilize this seam. Options: (a) obelisk-bevy exposes a composable `add_sim_systems(app)` the arena calls (so it structurally cannot drift), or (b) an arena test that asserts `add_obelisk_sim`'s system set matches `ObeliskSimPlugin::build`'s. Until then: any obelisk-bevy sim-composition change MUST be mirrored here.
- **python3 absent in the dev shell.** The net-test's `summarize.py` gate cannot run in this environment; results were validated by grepping the raw `/tmp/arena-net-test/*.jsonl` traces. The green gate runs in the user's CI.

## Accepted scope reductions

- **Effect (ailment) authoring is out of Skill-mode scope.** The built-in Skill mode authors the three-artifact skill triad (rules / behavior / presentation), NOT `stat_core` ailment effects (`config/effects/*.toml`, e.g. `burn`). The v1 `arena_editor`'s `effects_panel.rs`/`stat_ui.rs` are deleted in C8. Ailments stay hand-authored TOML; the editor shell loads them via `add_obelisk_effects(config/effects)` so previews that apply an ailment resolve correctly. **Ticket:** a future editor Effect-authoring mode.
- **Rig-less editor preview.** The ported preview stage does not consume the arena's `character.glb`; the editor caster is a capsule, and `anim` cue bindings are inert in editor preview. **Ticket:** wire a host-provided rig into the preview stage (a `bevy_modal_editor` enhancement).

## Carried obelisk-bevy tickets (now arena-relevant)

- **Spatial `TriggerFired` observability.** The reform's hit-trigger executor (`combat/system.rs`) and lifecycle-trigger site emit no `TriggerFired`; the arena observes triggered skills via their own `DamageResolved`/cues (sufficient — the net-test asserts firebolt_explosion this way). A `TriggerFired` for the trigger EDGE would aid tracing.
- **`nearest_retarget_candidate` has no liveness check** — chains can hop to corpses. Now gameplay-visible via `chain_lightning`.
- **Facade `Vec3::ZERO` fallback** places transform-less triggered executions at the world origin — relevant to triggered explosions.

## Per-task Minor findings (triage at merge)

- A1: the test casts a single skill id, so it wouldn't catch a hardcoded `skill_id` (all four fire sites were verified to use `e.skill_id`). Optional: exercise a second skill.
- C1: doc drift — **FIXED** (whole-branch review M1, commit `124e9b8`): 7 sites (cast_pipeline.rs header + fn doc, CLAUDE.md ×2, server/mod.rs, protocol.rs, client/net.rs) that still described the removed `cast_skill_dir_charged_from` free-aim flow now describe the Acquisition-resolved flow.
- C1: nit — `cast_pipeline.rs` `cast_entity`/`cast_ground` closures duplicate origin/exclusion/filter setup; could share a helper.
- C2: nit — `arena_editor/Cargo.toml` comment prose still mentions "arena_skills" (C8 rewrites arena_editor).
- C4: nits — the parse test checks the `vfx_cues↔cues` pairing only for `on_cast` (all three verified correct); the conditions test asserts count/`additional` but not three distinct types (verified always/on_impact/on_expire present).
- C5: nit — `chain_radius: 6.0` equals the schema default, so the `> 0.0` test check can't distinguish authored from defaulted (content does author it).

## CLAUDE.md

`crates/arena_game/CLAUDE.md` was refreshed mid-branch (arena_skills removed, `net/cue.rs`, stubbed cosmetics, `add_obelisk_sim` note). A final sweep at merge should confirm it fully reflects the migration (no lingering references to deleted APIs) and add the CueBinding render once C3 lands.
