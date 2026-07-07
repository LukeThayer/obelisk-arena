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

/// The two REAL physics compositions level colliders must work under:
/// - `game_lightyear: false` — full plain avian (the editor-preview / scratch-tool shape).
/// - `game_lightyear: true` — the live game's `add_avian_with_lightyear` shape
///   (`LightyearAvianPlugin` Position mode + the transform/interp/island plugins disabled).
fn physics_app(game_lightyear: bool) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // avian's collider cache (`clear_unused_colliders`, Update) reads mesh asset events — same
    // reason the headless server composition carries the asset/mesh/scene plugins. TransformPlugin
    // matches the live headless composition too (bin/server.rs adds it explicitly).
    app.add_plugins((
        bevy::asset::AssetPlugin::default(),
        bevy::mesh::MeshPlugin,
        bevy::scene::ScenePlugin,
        TransformPlugin,
    ));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    if game_lightyear {
        app.add_plugins(lightyear::avian3d::plugin::LightyearAvianPlugin {
            replication_mode: lightyear::avian3d::plugin::AvianReplicationMode::Position,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::new(FixedUpdate)
                .build()
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>()
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        );
    } else {
        app.add_plugins(PhysicsPlugins::new(FixedUpdate));
    }
    app.insert_resource(Gravity(Vec3::new(0.0, -20.0, 0.0)));
    // Drive time manually at exactly one fixed tick per `app.update()` (the preview_smoke
    // pattern) — running `FixedUpdate` directly never advances avian's clock, so physics
    // silently doesn't integrate and every assertion goes vacuous.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ));
    // Finalize plugins (avian registers its diagnostics resources in `finish`).
    app.finish();
    app.cleanup();
    app
}

/// Step the sim `secs` of simulated time (one fixed tick per update, via `ManualDuration`).
fn step(app: &mut App, secs: f32) {
    let steps = (secs * 60.0).ceil() as u32;
    for _ in 0..steps {
        app.update();
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
        // Spawn ABOVE rest height: the assertion then also proves physics actually integrated
        // (it must FALL to ~0.59 and stop there — staying at 1.5 or reaching -∞ both fail).
        commands.spawn((
            RigidBody::Dynamic,
            Collider::capsule(0.35, 0.48),
            LockedAxes::ROTATION_LOCKED,
            Position(Vec3::new(4.0, 1.5, 0.0)),
        ));
        world.flush();
    }
    // Reproduce what lightyear's avian Position-mode sync does on the live server: physics
    // entities acquire a `Transform` copied from their Position — with scale ONE. avian's
    // `update_collider_scale` then resets the collider scale to that Transform's — which stomped
    // a `set_scale`d collider to a 1m cube (the fall-through-the-world bug). Shape-baked
    // colliders are invariant under it (scale 1 → reset to 1 is a no-op).
    if stomp_identity_transform {
        let stamped: Vec<(Entity, Vec3)> = level_entities
            .iter()
            .filter_map(|e| {
                app.world()
                    .get::<Position>(*e)
                    .map(|p| (*e, p.0))
            })
            .collect();
        for (e, pos) in stamped {
            app.world_mut()
                .entity_mut(e)
                .insert(Transform::from_translation(pos));
        }
    }

    step(&mut app, 2.0);

    let mut q = app
        .world_mut()
        .query_filtered::<&Position, With<RigidBody>>();
    let ys: Vec<f32> = q.iter(app.world()).map(|p| p.0.y).collect();
    let body_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    // The band proves BOTH that physics ran (it fell from 1.5) and that the floor held (it
    // stopped at rest height ~0.59 instead of tunneling to -∞).
    assert!(
        body_y > 0.4 && body_y < 0.8,
        "the capsule must FALL from 1.5 and REST on the level floor (~0.59) — got y={body_y} \
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

/// `grounded_by_ray` over real level geometry (the game's lightyear-avian composition): a capsule
/// settling on the arena_flat floor reads grounded; a hoisted one reads airborne. (The flat-floor
/// Y-threshold check this replaced couldn't handle platforms/ramps.)
#[test]
fn grounded_by_ray_tracks_level_floor() {
    #[derive(Resource, Default)]
    struct GroundedProbe(std::collections::HashMap<Entity, bool>);

    fn probe(
        mut spatial: SpatialQuery,
        bodies: Query<(Entity, &Position), With<Collider>>,
        mut out: ResMut<GroundedProbe>,
    ) {
        // Same explicit refresh the game's obelisk composition runs before its own
        // spatial reads (`refresh_spatial_pipeline`) — an Update-schedule read otherwise
        // sees a stale pipeline.
        spatial.update_pipeline();
        for (e, p) in &bodies {
            out.0.insert(
                e,
                arena_game::shared_controller::grounded_by_ray(&spatial, p.0, &[e]),
            );
        }
    }

    let mut app = physics_app(true);
    app.init_resource::<GroundedProbe>();
    app.add_systems(Update, probe);

    let path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/scenes/arena_flat.scn.ron"
    ));
    let scene = load_level_scene(path).expect("arena_flat loads");
    let (resting, hoisted);
    {
        let world = app.world_mut();
        let mut commands = world.commands();
        spawn_level(&mut commands, &scene, None);
        // Spawned above rest height — settling to ~0.59 proves the sim integrates before the
        // grounded probe reads.
        resting = commands
            .spawn((
                RigidBody::Dynamic,
                Collider::capsule(0.35, 0.48),
                LockedAxes::ROTATION_LOCKED,
                Position(Vec3::new(4.0, 1.0, 0.0)),
            ))
            .id();
        hoisted = commands
            .spawn((
                RigidBody::Static, // static so it HOLDS its hoisted pose for the probe
                Collider::capsule(0.35, 0.48),
                Position(Vec3::new(-4.0, 1.59, 0.0)),
            ))
            .id();
        world.flush();
    }
    step(&mut app, 1.0);
    app.update();

    // The resting capsule must have SETTLED (spawned at 1.0) — proves physics integrated before
    // the probe read.
    let resting_pos = app.world().get::<Position>(resting).unwrap().0;
    assert!(
        resting_pos.y < 0.75,
        "capsule must settle onto the floor before the grounded probe — y={}",
        resting_pos.y
    );
    let probe_out = app.world().resource::<GroundedProbe>();
    assert_eq!(
        probe_out.0.get(&resting).copied(),
        Some(true),
        "resting capsule grounded"
    );
    assert_eq!(
        probe_out.0.get(&hoisted).copied(),
        Some(false),
        "hoisted capsule airborne"
    );
}
