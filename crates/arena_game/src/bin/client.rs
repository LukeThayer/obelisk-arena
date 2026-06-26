//! arena-client: the present client (netcode guide §2.3). Two modes:
//!   - default (windowed): M1 gameplay (firebolt cast + cosmetics + rig) + the lightyear connect.
//!   - `ARENA_HEADLESS=1`: MinimalPlugins + the net stack only, so connectivity is verifiable
//!     without a window (the M2.1 GATE in Task 9 brings up two of these).

fn main() {
    if std::env::var("ARENA_HEADLESS").ok().as_deref() == Some("1") {
        arena_game::client::run_headless_client();
    } else {
        arena_game::client::run_windowed_client();
    }
}
