#[test]
fn crate_links_and_exposes_tick_hz() {
    assert_eq!(arena_sim::ARENA_SIM_TICK_HZ, 60);
}

#[test]
fn tuning_and_input_are_exposed() {
    assert_eq!(arena_sim::tuning::GROUND_Y, 0.59);
    assert_eq!(arena_sim::tuning::GRAVITY, 20.0);
    assert_eq!(arena_sim::tuning::PLAYER_CAPSULE_RADIUS, 0.35);
    assert_eq!(arena_sim::tuning::PLAYER_CAPSULE_LENGTH, 0.48);
    let i = arena_sim::input::ArenaInput::default();
    assert!(!i.jump && !i.charging);
}

#[test]
fn add_obelisk_sim_composes_under_plain_avian_without_panicking() {
    use avian3d::prelude::*;
    use bevy::prelude::*;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin {
            file_path: ".".into(),
            ..default()
        })
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(PhysicsPlugins::new(FixedUpdate))
        .insert_resource(Gravity(Vec3::new(0.0, -arena_sim::tuning::GRAVITY, 0.0)))
        .insert_resource(Time::<Fixed>::from_hz(60.0));
    arena_sim::obelisk::add_obelisk_sim(&mut app, true);
    app.finish();
    app.cleanup();
    app.update();
}
