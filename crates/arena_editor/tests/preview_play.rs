//! Task 20: `PreviewControllerPlugin` drives the "Play the real skill" preview lifecycle. On a
//! `GameStartedEvent`, `start_preview` registers the currently-edited timeline (with derived vfx
//! cues) into `CastTimelineHandles`, spawns a `PreviewCaster` + `PreviewDummy` duel (both
//! `GameEntity`-tagged so the editor despawns them on Reset), grants the skill, and casts.
//!
//! Verified on the `arena_sim` headless harness (plain-Avian + `add_obelisk_sim` via
//! `ArenaSimPreviewPlugin`), NOT the full editor (which can't advance a frame headlessly). The
//! deterministic obelisk sim then resolves real damage on the dummy — proving what you author is
//! what the game plays.

use arena_editor::io::{editor_root, load_cast_timeline};
use arena_editor::model::EditedSkill;
use arena_editor::preview_controller::PreviewControllerPlugin;
use arena_sim::preview::{ArenaSimPreviewPlugin, PreviewCaster};
use bevy::prelude::*;
use bevy_editor_game::{GameEntity, GameStartedEvent};
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillSource};
use obelisk_bevy::testkit::{init_test_obelisk, EventRecorder, EventRecorderPlugin};
use std::time::Duration;

fn run(seed: u64, ticks: usize) -> App {
    init_test_obelisk();
    let root = editor_root();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin {
            file_path: ".".into(),
            ..default()
        })
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(EventRecorderPlugin)
        .add_plugins(bevy::state::app::StatesPlugin)
        .init_state::<bevy_editor_game::GameState>()
        .add_message::<GameStartedEvent>()
        .add_message::<bevy_editor_game::GameResetEvent>()
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f64(1.0 / 60.0),
        ))
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .add_plugins(ArenaSimPreviewPlugin)
        .add_plugins(PreviewControllerPlugin);
    app.add_obelisk_skills(SkillSource::Dir(root.join("config/skills")));
    app.seed_combat_rng(seed);
    let tl = load_cast_timeline(&root.join("assets/skills/firebolt.cast.ron")).expect("firebolt");
    app.insert_resource(EditedSkill::from_timeline(
        tl,
        root.join("assets/skills/firebolt.cast.ron"),
    ));
    app.finish();
    app.cleanup();
    app.world_mut().write_message(GameStartedEvent);
    for _ in 0..ticks {
        app.update();
    }
    app
}

#[test]
fn play_resolves_damage_on_the_dummy() {
    let app = run(0xC0FFEE, 90);
    let rec = app.world().resource::<EventRecorder>();
    assert!(!rec.cast_began.is_empty());
    assert!(!rec.damage_resolved.is_empty());
    assert!(
        rec.damage_resolved
            .iter()
            .map(|d| d.total_damage)
            .sum::<f64>()
            > 0.0
    );
}

#[test]
fn play_spawns_game_entity_tagged_caster() {
    let mut app = run(1, 3);
    let n = app
        .world_mut()
        .query_filtered::<Entity, (With<PreviewCaster>, With<GameEntity>)>()
        .iter(app.world())
        .count();
    assert_eq!(n, 1);
}

#[test]
fn preview_is_deterministic() {
    let total = |s: u64| {
        run(s, 90)
            .world()
            .resource::<EventRecorder>()
            .damage_resolved
            .iter()
            .map(|d| d.total_damage)
            .sum::<f64>()
    };
    let (a, b) = (total(0xABCDEF), total(0xABCDEF));
    assert!(a > 0.0);
    assert_eq!(a, b);
}
