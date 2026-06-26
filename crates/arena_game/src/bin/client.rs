//! arena-client: the windowed present client. M2.1 keeps the M1 single-process gameplay (firebolt
//! cast + cosmetics + rig) via `run_windowed_client`; Task 8 layers the lightyear connect on top.

fn main() {
    arena_game::client::run_windowed_client();
}
