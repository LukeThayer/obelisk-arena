//! arena-client: the present client (netcode guide §2.3). Two modes:
//!   - default (windowed): the networked windowed client (server-authoritative duel + cosmetics +
//!     rig + HUD) + the lightyear connect.
//!   - `ARENA_HEADLESS=1`: MinimalPlugins + the net stack only, so connectivity is verifiable
//!     without a window (the net-test harness brings up two of these).

fn main() {
    if std::env::var("ARENA_HEADLESS").ok().as_deref() == Some("1") {
        arena_game::client::run_headless_client();
    } else {
        arena_game::client::run_windowed_client();
    }
}
