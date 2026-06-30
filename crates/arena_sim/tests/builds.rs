#[test]
fn crate_links_and_exposes_tick_hz() {
    assert_eq!(arena_sim::ARENA_SIM_TICK_HZ, 60);
}
