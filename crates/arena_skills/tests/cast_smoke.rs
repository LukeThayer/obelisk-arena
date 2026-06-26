//! Headless determinism smoke test: cast firebolt and assert the hit resolves damage.
//!
//! This proves obelisk's deterministic spatiotemporal combat works end-to-end from the
//! `obelisk-arena` workspace, using obelisk-bevy's REAL `test-support` harness
//! (`obelisk_bevy::testkit::ObeliskTestApp`) — the same path its own `tests/vertical_slice.rs`
//! exercises. We mirror that test's structure exactly.
//!
//! ## Why we re-root onto the obelisk-bevy crate
//! `ObeliskTestApp::new(seed)` (see `obelisk-bevy/src/testkit.rs`) resolves two kinds of path
//! against obelisk-bevy's own crate, and under `cargo test -p arena_skills` neither default holds:
//!
//!   1. **Fixture file IO** — it inits the effect registry from `tests/fixtures/effects` and loads
//!      skills from `tests/fixtures/skills`, both via plain file IO relative to the **process CWD**.
//!      Under `cargo test` the CWD is `crates/arena_skills`, where these don't exist, so we
//!      `set_current_dir` to the obelisk-bevy crate root.
//!   2. **The `.cast.ron` AssetServer load** — `AssetPlugin.file_path` is `"."`, which Bevy's
//!      `FileAssetReader` joins onto `get_base_path()`. That base is `BEVY_ASSET_ROOT` if set, else
//!      the **compile-time `CARGO_MANIFEST_DIR`** (here `crates/arena_skills`) — NOT the runtime CWD.
//!      So `set_current_dir` alone does not redirect asset reads; we also set `BEVY_ASSET_ROOT` to
//!      the obelisk-bevy crate root so `assets/skills/firebolt.cast.ron` resolves there.
//!
//! The arena ships byte-identical copies of `firebolt.toml` / `firebolt.cast.ron` (verified), so
//! this loads exactly the content the arena owns, through the real harness, unmodified.

use bevy::prelude::*;
use obelisk_bevy::prelude::*;
use obelisk_bevy::testkit::ObeliskTestApp;
use stat_core::StatBlock;

/// Re-root the process onto the sibling `obelisk-bevy` crate so the testkit's CWD-relative fixture
/// IO and its `AssetServer` reads both resolve. Sets the process CWD (for the skill/effect fixture
/// file IO) AND `BEVY_ASSET_ROOT` (for the `file_path: "."` asset reader, which ignores the CWD and
/// keys off `BEVY_ASSET_ROOT`/`CARGO_MANIFEST_DIR`). Idempotent: both tests target the same dir, so
/// the global state is consistent regardless of parallelism.
fn enter_obelisk_root() {
    // crates/arena_skills -> ../.. = obelisk-arena ; sibling obelisk-bevy
    let arena_skills_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let obelisk = arena_skills_dir
        .ancestors()
        .nth(2) // crates/arena_skills -> crates -> obelisk-arena
        .expect("arena_skills is nested two levels under the workspace root")
        .parent()
        .expect("workspace root has a parent (src/)")
        .join("obelisk-bevy");
    assert!(
        obelisk.join("tests/fixtures/skills/firebolt.toml").exists(),
        "expected obelisk-bevy fixtures at {}",
        obelisk.display()
    );
    assert!(
        obelisk.join("assets/skills/firebolt.cast.ron").exists(),
        "expected obelisk-bevy cast asset at {}",
        obelisk.display()
    );
    // The asset reader's base path (`get_base_path`): BEVY_ASSET_ROOT wins over CARGO_MANIFEST_DIR.
    std::env::set_var("BEVY_ASSET_ROOT", &obelisk);
    std::env::set_current_dir(&obelisk)
        .unwrap_or_else(|e| panic!("set_current_dir({}) failed: {e}", obelisk.display()));
}

fn make_block(id: &str, life: f64, mana: f64) -> StatBlock {
    let mut b = StatBlock::with_id(id);
    b.max_life.base = life;
    b.current_life = life;
    b.max_mana.base = mana;
    b.current_mana = mana;
    b
}

/// Build a headless obelisk app seeded with `seed`, load + register the firebolt cast timeline,
/// spawn a Player caster at the origin (granted nothing — `cast_skill_at` resolves the timeline
/// from the handle) and an Enemy target with a hurtbox 2m down +Z, then cast firebolt and advance
/// `ticks` fixed ticks. Geometry mirrors `obelisk-bevy/tests/vertical_slice.rs` (target at z=2,
/// hurtbox radius 0.6) so the firebolt's moving Sphere(0.5) collision window overlaps the hurtbox.
fn run_firebolt(seed: u64, ticks: usize) -> ObeliskTestApp {
    let mut t = ObeliskTestApp::new(seed);

    // Load the cast timeline asset (CWD == obelisk-bevy) and register it under the skill id.
    let handle: Handle<CastTimeline> = t
        .app
        .world()
        .resource::<AssetServer>()
        .load("assets/skills/firebolt.cast.ron");
    for _ in 0..2000 {
        t.app.update();
        if t.app
            .world()
            .resource::<Assets<CastTimeline>>()
            .get(&handle)
            .is_some()
        {
            break;
        }
    }
    assert!(
        t.app
            .world()
            .resource::<Assets<CastTimeline>>()
            .get(&handle)
            .is_some(),
        "firebolt.cast.ron should have loaded"
    );
    t.app
        .world_mut()
        .resource_mut::<CastTimelineHandles>()
        .0
        .insert("firebolt".into(), handle);

    // Player caster at origin; Enemy target 2m along +Z. firebolt's `hit_filter: Enemies` matches
    // a Player caster vs an Enemy target.
    let caster = t
        .app
        .world_mut()
        .spawn((
            Combatant,
            Attributes(make_block("caster", 100.0, 100.0)),
            Faction::Player,
            ObeliskId("caster".into()),
            Transform::from_xyz(0.0, 0.0, 0.0),
        ))
        .id();
    let target = t
        .app
        .world_mut()
        .spawn((
            Combatant,
            Attributes(make_block("target", 25.0, 0.0)),
            Faction::Enemy,
            ObeliskId("target".into()),
            Transform::from_xyz(0.0, 0.0, 2.0),
        ))
        .id();
    // The testkit's spawn does NOT add a hurtbox; the firebolt window only fires HitConfirmed when
    // it overlaps a Hurtbox collider, so we add one (mirrors vertical_slice.rs).
    {
        let mut commands = t.app.world_mut().commands();
        insert_hurtbox(&mut commands, target, 0.6, Vec3::new(0.0, 0.0, 2.0));
    }
    t.app.update();

    t.app
        .world_mut()
        .commands()
        .entity(caster)
        .cast_skill_at("firebolt", target);
    t.advance_ticks(ticks);
    t
}

/// THE SMOKE TEST: a firebolt cast resolves damage on the target within ~40 ticks.
#[test]
fn firebolt_cast_resolves_damage() {
    enter_obelisk_root();
    let t = run_firebolt(0xC0FFEE, 40);
    let rec = t.rec();

    // Surface the intermediate pipeline events to make a failure precise (cast -> phase -> window
    // -> hit -> damage), then assert the load-bearing claim: damage resolved.
    assert!(!rec.cast_began.is_empty(), "cast should begin");
    assert!(
        rec.phase_changed
            .iter()
            .any(|p| matches!(p.to, SkillPhase::Active)),
        "should reach the Active phase (spawns the bolt window)"
    );
    assert!(
        !rec.hit_window_opened.is_empty(),
        "the bolt collision window should open"
    );
    assert!(
        !rec.hit_confirmed.is_empty(),
        "the moving bolt window should overlap the target hurtbox"
    );
    assert!(
        !rec.damage_resolved.is_empty(),
        "firebolt should resolve damage by tick 40"
    );

    let total: f64 = rec.damage_resolved.iter().map(|d| d.total_damage).sum();
    assert!(
        total > 0.0,
        "resolved damage should be positive, got {total}"
    );
    // firebolt deals 20 base fire (no crit chance on the caster) -> exactly 20 on the impact hit.
    let dmg = rec.damage_resolved[0].total_damage;
    assert_eq!(dmg, 20.0, "firebolt impact deals its 20 base fire damage");
}

/// Determinism: two runs with the same seed produce the identical total resolved damage
/// (mirrors `obelisk-bevy/tests/vertical_slice.rs::slice_is_deterministic_across_two_runs`).
#[test]
fn firebolt_damage_is_deterministic_across_two_runs() {
    enter_obelisk_root();
    let total = |seed: u64| -> f64 {
        run_firebolt(seed, 60)
            .rec()
            .damage_resolved
            .iter()
            .map(|d| d.total_damage)
            .sum::<f64>()
    };
    let a = total(0xABCDEF);
    let b = total(0xABCDEF);
    assert!(a > 0.0, "the deterministic run should resolve damage");
    assert_eq!(a, b, "same seed -> identical total resolved damage");
}
