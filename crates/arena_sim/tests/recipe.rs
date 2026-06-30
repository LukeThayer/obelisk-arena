use arena_sim::spawn::{faction_for_slot, make_arena_combatant, SPAWN_MARKERS};
use arena_sim::tuning::GROUND_Y;
use avian3d::prelude::*;
use bevy::prelude::*;
use obelisk_bevy::prelude::*;

#[test]
fn faction_for_slot_assigns_opposing_factions() {
    assert_eq!(faction_for_slot(0), Faction::Player);
    assert_eq!(faction_for_slot(1), Faction::Enemy);
}

#[test]
fn spawn_markers_are_two_opposed_points() {
    assert_eq!(SPAWN_MARKERS.len(), 2);
    assert_eq!(SPAWN_MARKERS[0], Vec3::new(-4.0, GROUND_Y, 0.0));
    assert_eq!(SPAWN_MARKERS[1], Vec3::new(4.0, GROUND_Y, 0.0));
}

#[test]
fn make_arena_combatant_builds_dynamic_capsule_with_child_hurtbox() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    obelisk_bevy::testkit::init_test_obelisk();
    let player = {
        let mut c = app.world_mut().commands();
        make_arena_combatant(&mut c, "player_0", Faction::Player, SPAWN_MARKERS[0])
    };
    app.world_mut().flush();
    assert!(app.world().get::<RigidBody>(player).is_some());
    assert!(app.world().get::<Faction>(player).is_some());
    assert_eq!(
        app.world().get::<Position>(player).map(|p| p.0),
        Some(SPAWN_MARKERS[0])
    );
    let children = app.world().get::<Children>(player).expect("child hurtbox");
    assert_eq!(children.len(), 1);
    assert!(app.world().get::<Hurtbox>(children[0]).is_some());
}
