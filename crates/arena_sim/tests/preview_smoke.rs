use arena_sim::obelisk::add_obelisk_sim;
use arena_sim::spawn::{make_arena_combatant, spawn_arena_floor};
use arena_sim::tuning::GRAVITY;
use avian3d::prelude::*;
use bevy::prelude::*;
use obelisk_bevy::prelude::*;
use obelisk_bevy::testkit::{init_test_obelisk, EventRecorder, EventRecorderPlugin};
use std::time::Duration;

fn enter_obelisk_root() {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let obelisk = d
        .ancestors()
        .nth(2)
        .expect("obelisk-arena")
        .parent()
        .expect("parent")
        .join("obelisk-bevy");
    assert!(obelisk.join("assets/skills/firebolt.cast.ron").exists());
    std::env::set_var("BEVY_ASSET_ROOT", &obelisk);
    std::env::set_current_dir(&obelisk).expect("re-root");
}

fn run(seed: u64, ticks: usize) -> App {
    run_with_floor(seed, ticks, |app| {
        spawn_arena_floor(&mut app.world_mut().commands());
    })
}

/// The same combat world, but the floor comes from the caller — lets the level-loader test swap
/// the legacy `spawn_arena_floor` cuboid for a `spawn_level`-spawned `.scn.ron` level and prove
/// obelisk hit resolution is floor-source-agnostic.
fn run_with_floor(seed: u64, ticks: usize, spawn_floor: impl FnOnce(&mut App)) -> App {
    init_test_obelisk();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin {
            file_path: ".".into(),
            ..default()
        })
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(PhysicsPlugins::new(FixedUpdate))
        .insert_resource(Gravity(Vec3::new(0.0, -GRAVITY, 0.0)))
        .add_plugins(EventRecorderPlugin)
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f64(1.0 / 60.0),
        ))
        .insert_resource(Time::<Fixed>::from_hz(60.0));
    add_obelisk_sim(&mut app, true);
    use obelisk_bevy::core::config::SkillSource;
    use obelisk_bevy::prelude::ObeliskConfigExt;
    app.add_obelisk_skills(SkillSource::Dir(std::path::PathBuf::from(
        "tests/fixtures/skills",
    )));
    app.seed_combat_rng(seed);
    app.finish();
    app.cleanup();
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
    app.world_mut()
        .resource_mut::<CastTimelineHandles>()
        .0
        .insert("firebolt".into(), handle);
    spawn_floor(&mut app);
    let caster = make_arena_combatant(
        &mut app.world_mut().commands(),
        "caster",
        Faction::Player,
        Vec3::new(0.0, 0.59, 0.0),
    );
    // The dummy is the Enemy the projectile resolves against (firebolt's `Enemies` hit-filter).
    let _dummy = make_arena_combatant(
        &mut app.world_mut().commands(),
        "target",
        Faction::Enemy,
        Vec3::new(0.0, 0.59, 2.0),
    );
    app.world_mut().flush();
    app.update();
    // Cast by DIRECTION (toward the dummy at +Z), matching the live game's
    // `cast_skill_dir_charged_from` path. Entity-aim (`cast_skill_at`) is unusable for arena
    // combatants: obelisk's LOS raycast excludes only the caster body entity, not its CHILD
    // `Hurtbox` sensor, so the caster's own hurtbox self-blocks the ray (`NoLineOfSight`). The
    // game sidesteps this the same way — free-aim direction casts skip the LOS gate.
    let aim = Dir3::new(Vec3::new(0.0, 0.0, 2.0)).unwrap();
    app.world_mut()
        .commands()
        .entity(caster)
        .cast_skill_dir("firebolt", aim);
    for _ in 0..ticks {
        app.update();
    }
    app
}

#[test]
fn firebolt_resolves_damage_on_the_dummy() {
    enter_obelisk_root();
    let app = run(0xC0FFEE, 60);
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

/// REGRESSION (levels-and-lobby): the same firebolt→damage flow over a floor spawned by the LEVEL
/// LOADER (`spawn_level(arena_flat)`) instead of the legacy `spawn_arena_floor` cuboid. Obelisk
/// hit resolution must be floor-source-agnostic — this is the plain-avian half of the net-test's
/// "damage on a loaded level" contract.
#[test]
fn firebolt_resolves_damage_on_a_loaded_level_floor() {
    // Absolute path BEFORE enter_obelisk_root's chdir (load_level_scene reads the fs directly).
    let level_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenes/arena_flat.scn.ron");
    enter_obelisk_root();
    let app = run_with_floor(0xC0FFEE, 60, move |app| {
        let scene = arena_sim::level::load_level_scene(&level_path).expect("arena_flat loads");
        arena_sim::level::spawn_level(&mut app.world_mut().commands(), &scene, None);
    });
    let rec = app.world().resource::<EventRecorder>();
    assert!(!rec.cast_began.is_empty());
    assert!(
        !rec.damage_resolved.is_empty(),
        "firebolt must resolve damage over a level-loader floor"
    );
}

#[test]
fn firebolt_damage_is_deterministic() {
    enter_obelisk_root();
    let total = |s: u64| {
        run(s, 60)
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
