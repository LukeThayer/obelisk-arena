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
