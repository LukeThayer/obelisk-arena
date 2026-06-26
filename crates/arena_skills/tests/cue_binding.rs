//! Cue-binding integration test: prove the `register_skill_cues` binding layer turns obelisk
//! `CueEvent`s into arena `CueMessage`s end-to-end.
//!
//! We mirror the M0 `cast_smoke.rs` harness exactly (`ObeliskTestApp`, the same re-root onto the
//! sibling `obelisk-bevy` crate, the same firebolt geometry) and then add the cosmetic layer on
//! top: deserialize firebolt's `.skillfx.ron` straight into a `SkillFx`, register its lanes via
//! `arena_skills::register_skill_cues`, and assert that casting firebolt produces a
//! `CueMessage { lane_id: "firebolt_muzzle", .. }` (the on-cast lane) — and, since the cast
//! resolves a hit, a `firebolt_impact` on-hit `CueMessage` too.
//!
//! See `cast_smoke.rs` for the full rationale on why we `set_current_dir` + set `BEVY_ASSET_ROOT`
//! to the obelisk-bevy crate root (the testkit resolves both its fixture file IO and its
//! `AssetServer` `.cast.ron` reads against that crate, not the `arena_skills` crate).

use arena_skills::{register_skill_cues, ArenaSkillsPlugin, CueKind, CueMessage, SkillFx};
use bevy::prelude::*;
use obelisk_bevy::prelude::*;
use obelisk_bevy::testkit::init_test_obelisk;
use stat_core::StatBlock;
use std::time::Duration;

/// Re-root the process onto the sibling `obelisk-bevy` crate so the testkit's CWD-relative fixture
/// IO and its `AssetServer` reads both resolve (see `cast_smoke.rs::enter_obelisk_root`).
fn enter_obelisk_root() {
    let arena_skills_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let obelisk = arena_skills_dir
        .ancestors()
        .nth(2) // crates/arena_skills -> crates -> obelisk-arena
        .expect("arena_skills is nested two levels under the workspace root")
        .parent()
        .expect("workspace root has a parent (src/)")
        .join("obelisk-bevy");
    assert!(
        obelisk.join("assets/skills/firebolt.cast.ron").exists(),
        "expected obelisk-bevy cast asset at {}",
        obelisk.display()
    );
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

/// Accumulates every `CueMessage` emitted over the whole run. `Messages<T>` double-buffers and
/// drops messages older than ~2 frames, so a single end-of-run read would miss the on-cast cue
/// (fired ~tick 0) by the time the on-hit cue fires (~tick 20). This collector drains the reader
/// every `Update` so nothing is lost.
#[derive(Resource, Default)]
struct CueLog(Vec<CueMessage>);

fn collect_cues(mut log: ResMut<CueLog>, mut reader: MessageReader<CueMessage>) {
    for m in reader.read() {
        log.0.push(m.clone());
    }
}

/// Load firebolt's `.skillfx.ron` straight into a `SkillFx` from the arena's own fixture (the
/// authoring artifact this crate owns), independent of the obelisk-bevy re-root used for the sim.
fn load_firebolt_skillfx() -> SkillFx {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("assets/skills/firebolt.skillfx.ron");
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()));
    ron::de::from_bytes::<SkillFx>(&bytes)
        .unwrap_or_else(|e| panic!("deserialize firebolt.skillfx.ron failed: {e}"))
}

#[test]
fn casting_firebolt_emits_lane_cue_messages() {
    // SkillFx must load from the arena fixture BEFORE we re-root the process onto obelisk-bevy
    // (after the re-root, CARGO_MANIFEST_DIR still points at arena_skills so the relative join
    // below is unaffected, but reading first keeps the two concerns independent).
    let fx = load_firebolt_skillfx();
    assert_eq!(fx.skill_id, "firebolt");

    enter_obelisk_root();

    // Build the headless app manually rather than via `ObeliskTestApp::new` — the testkit calls
    // `app.finish()`/`app.cleanup()` inside `new()`, after which plugins/observers (which
    // `register_skill_cues`/`ArenaSkillsPlugin` add) can no longer be installed. We mirror the
    // testkit recipe (`obelisk-bevy/src/testkit.rs::new`) verbatim, then add the cosmetic layer,
    // THEN finalize. `init_test_obelisk()` is the same `Once`-guarded global init the testkit uses.
    init_test_obelisk();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin {
            file_path: ".".into(),
            ..default()
        })
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(ObeliskSimPlugin)
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f64(1.0 / 60.0),
        ))
        .insert_resource(Time::<Fixed>::from_hz(60.0));
    app.add_obelisk_skills(SkillSource::Dir(std::path::PathBuf::from(
        "tests/fixtures/skills",
    )));
    app.seed_combat_rng(0xC0FFEE);

    // Cosmetic layer: the SkillFx asset/loader/message channel + the cue->message binding observers.
    app.add_plugins(ArenaSkillsPlugin);
    app.init_resource::<CueLog>();
    app.add_systems(Update, collect_cues);
    // Install the observers BEFORE the cast (and before finalize) so they're live when obelisk
    // fires CueEvent.
    register_skill_cues(&mut app, std::slice::from_ref(&fx));

    app.finish();
    app.cleanup();

    // --- firebolt sim setup (mirrors cast_smoke.rs::run_firebolt) ---
    let handle: Handle<CastTimeline> = app
        .world()
        .resource::<AssetServer>()
        .load("assets/skills/firebolt.cast.ron");
    for _ in 0..2000 {
        app.update();
        if app
            .world()
            .resource::<Assets<CastTimeline>>()
            .get(&handle)
            .is_some()
        {
            break;
        }
    }
    assert!(
        app.world()
            .resource::<Assets<CastTimeline>>()
            .get(&handle)
            .is_some(),
        "firebolt.cast.ron should have loaded"
    );
    app.world_mut()
        .resource_mut::<CastTimelineHandles>()
        .0
        .insert("firebolt".into(), handle);

    let caster = app
        .world_mut()
        .spawn((
            Combatant,
            Attributes(make_block("caster", 100.0, 100.0)),
            Faction::Player,
            ObeliskId("caster".into()),
            Transform::from_xyz(0.0, 0.0, 0.0),
        ))
        .id();
    let target = app
        .world_mut()
        .spawn((
            Combatant,
            Attributes(make_block("target", 25.0, 0.0)),
            Faction::Enemy,
            ObeliskId("target".into()),
            Transform::from_xyz(0.0, 0.0, 2.0),
        ))
        .id();
    {
        let mut commands = app.world_mut().commands();
        insert_hurtbox(&mut commands, target, 0.6, Vec3::new(0.0, 0.0, 2.0));
    }
    app.update();

    app.world_mut()
        .commands()
        .entity(caster)
        .cast_skill_at("firebolt", target);
    for _ in 0..40 {
        app.update();
    }

    // --- assert the binding layer surfaced the lanes ---
    let log = &app.world().resource::<CueLog>().0;
    let summary: Vec<(String, CueKind)> = log.iter().map(|m| (m.lane_id.clone(), m.kind)).collect();
    assert!(
        !log.is_empty(),
        "expected CueMessages from the firebolt cast, got none ({summary:?})"
    );

    let muzzle = log
        .iter()
        .find(|m| m.lane_id == "firebolt_muzzle")
        .unwrap_or_else(|| {
            panic!("expected a `firebolt_muzzle` (on-cast) CueMessage; got {summary:?}")
        });
    assert_eq!(
        muzzle.kind,
        CueKind::OnCast,
        "the firebolt_muzzle lane reacts to the on-cast cue"
    );
    assert_eq!(
        muzzle.source, caster,
        "the on-cast cue is anchored to the caster"
    );
    assert_eq!(
        muzzle.event.lane_id, "firebolt_muzzle",
        "the carried LaneEvent is the muzzle lane"
    );

    // Bonus: the cast resolves a hit, so the on-hit `firebolt_impact` lane fires too.
    let impact = log
        .iter()
        .find(|m| m.lane_id == "firebolt_impact")
        .unwrap_or_else(|| {
            panic!("expected a `firebolt_impact` (on-hit) CueMessage; got {summary:?}")
        });
    assert_eq!(
        impact.kind,
        CueKind::OnHit,
        "the firebolt_impact lane reacts to the on-hit cue"
    );
    assert_eq!(
        impact.source, target,
        "the on-hit cue is anchored to the hit target"
    );
}
