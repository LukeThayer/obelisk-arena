//! Level-geometry physics repro: a Dynamic capsule (the player body recipe) must REST on a
//! `spawn_level`-spawned floor under the ARENA's exact avian composition (`PhysicsPlugins::new(
//! FixedUpdate)` with the transform-sync/interpolation/island plugins disabled — the same
//! composition `add_avian_with_lightyear` builds, minus the lightyear layer, which plays no part
//! in static-vs-dynamic collision).
//!
//! Written against the net-test failure where both players fell through every level floor
//! (server poses y → -296): the level colliders spawned by `arena_sim::level::spawn_level` did
//! not collide. The fixture asserts the END-TO-END load→spawn→collide path.

use avian3d::prelude::*;
use bevy::prelude::*;

use arena_sim::level::{load_level_scene, spawn_level};

fn physics_app(with_lightyear_avian: bool) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    if with_lightyear_avian {
        app.add_plugins(lightyear::avian3d::plugin::LightyearAvianPlugin {
            replication_mode: lightyear::avian3d::plugin::AvianReplicationMode::Position,
            ..default()
        });
    }
    app.add_plugins(
        PhysicsPlugins::new(FixedUpdate)
            .build()
            .disable::<PhysicsTransformPlugin>()
            .disable::<PhysicsInterpolationPlugin>()
            .disable::<IslandPlugin>()
            .disable::<IslandSleepingPlugin>(),
    );
    app.insert_resource(Gravity(Vec3::new(0.0, -20.0, 0.0)));
    app
}

/// Step the fixed-timestep sim `secs` of simulated time.
fn step(app: &mut App, secs: f32) {
    let steps = (secs * 60.0).ceil() as u32;
    for _ in 0..steps {
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_micros(16_667));
        app.world_mut().run_schedule(FixedUpdate);
    }
}

fn run_capsule_rest_check(with_lightyear_avian: bool, stomp_identity_transform: bool) {
    let mut app = physics_app(with_lightyear_avian);

    let path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/scenes/arena_flat.scn.ron"
    ));
    let scene = load_level_scene(path).expect("arena_flat loads");

    // Spawn the level exactly like the server does (physics-only) + the player body recipe
    // (same capsule/position as `make_arena_combatant`) at the slot-1 spawn.
    let level_entities;
    {
        let world = app.world_mut();
        let mut commands = world.commands();
        level_entities = spawn_level(&mut commands, &scene, None);
        commands.spawn((
            RigidBody::Dynamic,
            Collider::capsule(0.35, 0.48),
            LockedAxes::ROTATION_LOCKED,
            Position(Vec3::new(4.0, 0.59, 0.0)),
        ));
        world.flush();
    }
    // Reproduce what lightyear's avian Position-mode sync does on the live server: physics
    // entities acquire an identity `Transform` (scale ONE). avian's `update_collider_scale` then
    // resets the collider scale to that Transform's — which stomped a `set_scale`d collider to a
    // 1m cube (the fall-through-the-world bug). Shape-baked colliders are invariant under it.
    if stomp_identity_transform {
        for e in &level_entities {
            app.world_mut()
                .entity_mut(*e)
                .insert(Transform::IDENTITY);
        }
    }

    step(&mut app, 2.0);

    let mut q = app
        .world_mut()
        .query_filtered::<&Position, With<RigidBody>>();
    let ys: Vec<f32> = q.iter(app.world()).map(|p| p.0.y).collect();
    let body_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        body_y > 0.4,
        "the capsule must REST on the level floor (~0.59), not fall through — got y={body_y} \
         (all body ys: {ys:?}, lightyear_avian={with_lightyear_avian}, \
         stomped={stomp_identity_transform})"
    );
}

#[test]
fn player_capsule_rests_on_arena_flat_floor() {
    run_capsule_rest_check(false, false);
}

/// The same check under the SERVER's real physics composition (`LightyearAvianPlugin` +
/// `AvianReplicationMode::Position` — what `add_avian_with_lightyear` adds).
#[test]
fn player_capsule_rests_on_arena_flat_floor_with_lightyear_avian() {
    run_capsule_rest_check(true, false);
}

/// REGRESSION (the net-test fall-through): an identity `Transform` landing on level entities
/// (lightyear's Position-mode sync does this on the live server) must NOT shrink the collider —
/// avian resets collider scale to the Transform scale, so only shape-baked dimensions survive.
#[test]
fn identity_transform_on_level_entities_does_not_break_collision() {
    run_capsule_rest_check(true, true);
}
