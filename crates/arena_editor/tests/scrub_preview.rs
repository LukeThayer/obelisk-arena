//! SIM-BACKED scrubbing end-to-end (UX spec P3, headless): dragging the scrub target restarts
//! the cast on the persistent stage, fast-forwards the REAL deterministic sim, and FREEZES it
//! at the target — the bolt's hitbox is at its true arc position, damage has resolved iff the
//! target is past the true hit moment, and a backward drag replays identically (same seed).

use arena_editor::io::{editor_root, load_cast_timeline};
use arena_editor::model::EditedSkill;
use arena_editor::preview_controller::PreviewControllerPlugin;
use arena_editor::scrub::{drive_scrub, sim_unfrozen, tick_scrub_clock, ScrubMode, ScrubSim};
use arena_sim::preview::ArenaSimPreviewPlugin;
use bevy::prelude::*;
use bevy_editor_game::GameStartedEvent;
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillSource};
use obelisk_bevy::testkit::{init_test_obelisk, EventRecorder, EventRecorderPlugin};
use std::time::Duration;

fn scrub_app() -> App {
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
        .add_plugins(PreviewControllerPlugin)
        // The scrub machinery, wired exactly as the designer plugin does it.
        .init_resource::<ScrubSim>()
        .add_systems(Update, drive_scrub)
        .add_systems(
            FixedUpdate,
            tick_scrub_clock
                .run_if(sim_unfrozen)
                .before(obelisk_bevy::ObeliskSet::Validate),
        );
    {
        use obelisk_bevy::ObeliskSet;
        app.configure_sets(
            FixedUpdate,
            (
                ObeliskSet::Validate.run_if(sim_unfrozen),
                ObeliskSet::Advance.run_if(sim_unfrozen),
                ObeliskSet::Projectiles.run_if(sim_unfrozen),
                ObeliskSet::ResolveHits.run_if(sim_unfrozen),
                ObeliskSet::TickEffects.run_if(sim_unfrozen),
            ),
        );
    }
    app.add_obelisk_skills(SkillSource::Dir(root.join("config/skills")));
    app.seed_combat_rng(0);
    let path = root.join("assets/skills/firebolt.cast.ron");
    let tl = load_cast_timeline(&path).expect("firebolt parses");
    app.insert_resource(EditedSkill::from_timeline(tl, path));
    app.finish();
    app.cleanup();
    app
}

fn step(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

fn set_target(app: &mut App, t: f32) {
    app.world_mut().resource_mut::<ScrubSim>().target = Some(t);
}

#[test]
fn seek_freezes_the_sim_at_the_target_before_the_hit() {
    let mut app = scrub_app();
    step(&mut app, 3); // stage spawns

    // Seek to mid-flight: after the window opens (0.3) but before the true hit (~0.7).
    set_target(&mut app, 0.5);
    step(&mut app, 200); // plenty — seek runs at 24x

    let scrub = app.world().resource::<ScrubSim>();
    assert_eq!(scrub.mode, ScrubMode::Frozen, "seek must land in Frozen");
    assert!(
        (scrub.clock - 0.5).abs() < 0.1,
        "frozen near the target: {}",
        scrub.clock
    );
    // The bolt hitbox EXISTS, frozen mid-flight, and no damage has resolved yet.
    let hitboxes = app
        .world_mut()
        .query::<&obelisk_bevy::prelude::Hitbox>()
        .iter(app.world())
        .count();
    assert_eq!(hitboxes, 1, "bolt frozen mid-flight");
    let rec = app.world().resource::<EventRecorder>();
    assert!(
        rec.damage_resolved.is_empty(),
        "no damage before the true hit moment"
    );

    // FROZEN means frozen: many frames later the clock hasn't moved.
    let clock_before = app.world().resource::<ScrubSim>().clock;
    step(&mut app, 30);
    let clock_after = app.world().resource::<ScrubSim>().clock;
    assert_eq!(clock_before, clock_after, "the sim is paused at the instant");
}

#[test]
fn seeking_past_the_hit_resolves_real_damage_and_backward_replays() {
    let mut app = scrub_app();
    step(&mut app, 3);

    // Past the whole flight: the direct hit + chained blast resolve for real.
    set_target(&mut app, 1.2);
    step(&mut app, 300);
    let first = {
        let rec = app.world().resource::<EventRecorder>();
        let dmg: Vec<f64> = rec.damage_resolved.iter().map(|d| d.total_damage).collect();
        assert!(
            !dmg.is_empty(),
            "seeking past the hit resolves REAL damage (bolt + blast)"
        );
        dmg
    };

    // Backward drag: restart + reseek — deterministic, so the same damage resolves again.
    set_target(&mut app, 1.1);
    step(&mut app, 300);
    let rec = app.world().resource::<EventRecorder>();
    let total = rec.damage_resolved.len();
    assert_eq!(
        total,
        first.len() * 2,
        "the replayed run resolves the same number of hits"
    );
    let second: Vec<f64> = rec
        .damage_resolved
        .iter()
        .skip(first.len())
        .map(|d| d.total_damage)
        .collect();
    assert_eq!(first, second, "same seed, identical replay");
}

#[test]
fn replay_runs_to_the_end_and_freezes() {
    let mut app = scrub_app();
    step(&mut app, 3);
    app.world_mut().resource_mut::<ScrubSim>().replay_requested = true;
    // firebolt strip span = 2.3 s -> 138 fixed ticks at 1x; run enough frames.
    step(&mut app, 200);
    let scrub = app.world().resource::<ScrubSim>();
    assert_eq!(scrub.mode, ScrubMode::Frozen, "replay freezes at the end");
    assert!(scrub.clock >= 2.2, "ran the whole strip: {}", scrub.clock);
    let rec = app.world().resource::<EventRecorder>();
    assert!(!rec.damage_resolved.is_empty(), "the replayed cast hit for real");
}
