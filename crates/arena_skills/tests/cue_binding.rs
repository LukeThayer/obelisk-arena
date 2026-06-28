//! Cue-egress integration test: prove that casting firebolt fires obelisk `CueEvent`s that map
//! cleanly onto the M2 serde [`CueMessage`] wire shape.
//!
//! M1 routed cues through a `register_skill_cues` binding that emitted an embedded-`LaneEvent`,
//! `Entity`-keyed bevy `Message`. M2.0 replaced that with a plain serde `CueMessage`
//! `{ cue_id, source_id, position, kind }`: the `cue_id` is the obelisk cue VALUE the `.cast.ron`
//! fires, the `source_id` is the source combatant's stable `ObeliskId` (NOT a local `Entity`).
//!
//! This test casts firebolt through the real obelisk sim, observes every `CueEvent`, builds the
//! serde `CueMessage` from each via the egress helper [`arena_skills::cue_event_to_message`]
//! (resolving `source` → `ObeliskId` exactly as the `arena_game` egress does), and asserts both the
//! on-cast (`firebolt_cast`, anchored on the caster) and on-hit (`firebolt_impact`, anchored on the
//! target) wire cues surface with the right ids.
//!
//! See `cast_smoke.rs` for the full rationale on the `set_current_dir` + `BEVY_ASSET_ROOT` re-root.

use arena_skills::{cue_event_to_message, ArenaSkillsPlugin, CueKind, CueMessage};
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

/// Accumulates every serde [`CueMessage`] derived from an observed obelisk `CueEvent`. Built in an
/// observer (which can't take `Res`), so it stores the raw `CueEvent` fields and the test resolves
/// `source` → `ObeliskId` afterwards via the world query.
#[derive(Resource, Default)]
struct CueLog(Vec<(Entity, String, Vec3, CueKind)>);

#[test]
fn casting_firebolt_emits_serde_cue_messages() {
    enter_obelisk_root();

    // Mirror the testkit recipe (`obelisk-bevy/src/testkit.rs::new`) so we can install observers
    // BEFORE `finish()`/`cleanup()`. `init_test_obelisk()` is the same `Once`-guarded global init.
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

    app.add_plugins(ArenaSkillsPlugin);
    app.init_resource::<CueLog>();
    // Observe every obelisk CueEvent and stash its raw fields (source Entity, cue_id, pos, kind).
    app.add_observer(|cue: On<CueEvent>, mut log: ResMut<CueLog>| {
        log.0.push((
            cue.source,
            cue.cue_id.clone(),
            cue.position,
            cue.kind.into(),
        ));
    });

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

    // Resolve each observed CueEvent's `source` Entity → its stable ObeliskId, exactly as the
    // arena_game egress does, then build the serde CueMessage via the egress helper.
    let raw = std::mem::take(&mut app.world_mut().resource_mut::<CueLog>().0);
    let id_of = |e: Entity| -> String {
        app.world()
            .get::<ObeliskId>(e)
            .map(|o| o.0.clone())
            .unwrap_or_default()
    };
    let msgs: Vec<CueMessage> = raw
        .into_iter()
        .map(|(src, cue_id, pos, kind)| {
            cue_event_to_message(&cue_id, &id_of(src), pos, bevy::math::Vec3::ZERO, kind)
        })
        .collect();

    let summary: Vec<(&str, &str, CueKind)> = msgs
        .iter()
        .map(|m| (m.cue_id.as_str(), m.source_id.as_str(), m.kind))
        .collect();
    assert!(
        !msgs.is_empty(),
        "expected serde CueMessages from the firebolt cast, got none ({summary:?})"
    );

    // The on-cast cue: cue_id == "firebolt_cast", anchored on the caster's ObeliskId.
    let cast = msgs
        .iter()
        .find(|m| m.cue_id == "firebolt_cast")
        .unwrap_or_else(|| {
            panic!("expected a `firebolt_cast` (on-cast) CueMessage; got {summary:?}")
        });
    assert_eq!(
        cast.kind,
        CueKind::OnCast,
        "firebolt_cast is an on-cast cue"
    );
    assert_eq!(
        cast.source_id, "caster",
        "the on-cast cue's source_id is the caster's ObeliskId"
    );

    // The on-hit cue: cue_id == "firebolt_impact", anchored on the hit target's ObeliskId.
    let impact = msgs
        .iter()
        .find(|m| m.cue_id == "firebolt_impact")
        .unwrap_or_else(|| {
            panic!("expected a `firebolt_impact` (on-hit) CueMessage; got {summary:?}")
        });
    assert_eq!(
        impact.kind,
        CueKind::OnHit,
        "firebolt_impact is an on-hit cue"
    );
    assert_eq!(
        impact.source_id, "target",
        "the on-hit cue's source_id is the target's ObeliskId"
    );
}
