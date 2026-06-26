//! obelisk-arena shared library: gameplay + netcode shared by the `arena-server`, `arena-client`,
//! and `arena-observer` bins under `src/bin/`. Each bin composes its own plugin set on top of these
//! modules — the server adds `MinimalPlugins` + `ServerNetPlugin` + `ArenaServerPlugin`; the client
//! runs `client::run_windowed_client` (DefaultPlugins) + `ClientNetPlugin`.
//!
//! M2 reshaped M1's co-located `main.rs` into this lib + the three bins (netcode guide §2).

use avian3d::prelude::*;
use bevy::prelude::*;
use std::path::PathBuf;

pub mod client;
pub mod net;
pub mod server;
pub mod trace;

/// The arena workspace root, holding `assets/` (cast timelines) and `config/` (skill + effect
/// rules). Resolved so the binaries work regardless of launch directory: under `cargo run`,
/// `CARGO_MANIFEST_DIR` is `crates/arena_game`, so the root is two levels up; otherwise fall back
/// to the current working directory.
pub fn arena_root() -> PathBuf {
    match std::env::var_os("CARGO_MANIFEST_DIR") {
        Some(dir) => PathBuf::from(dir)
            .ancestors()
            .nth(2)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

/// Add avian physics with the lightyear integration plugin. Copied from wisp's
/// `add_avian_with_lightyear` (`wisp/src/lib.rs:57-70`), which mirrors lightyear's
/// `avian_3d_character` example at tag 0.26.4:
///
/// - `LightyearAvianPlugin` in `AvianReplicationMode::Position` so replicated `Position` carries the
///   authoritative pose and lightyear owns the Transform ↔ Position sync.
/// - `PhysicsTransformPlugin` disabled — lightyear's plugin replaces it (else both race to set
///   Transform from Position and jitter).
/// - `PhysicsInterpolationPlugin` disabled — we use lightyear's `add_linear_interpolation`.
/// - `IslandPlugin` + `IslandSleepingPlugin` disabled — sleeping bodies misbehave under rollback.
///
/// Must be called AFTER `ServerPlugins`/`ClientPlugins` so `LightyearAvianPlugin` sees the
/// replication infrastructure.
pub fn add_avian_with_lightyear(app: &mut App) {
    app.add_plugins(lightyear::avian3d::plugin::LightyearAvianPlugin {
        replication_mode: lightyear::avian3d::plugin::AvianReplicationMode::Position,
        ..default()
    });
    app.add_plugins(
        PhysicsPlugins::default()
            .build()
            .disable::<PhysicsTransformPlugin>()
            .disable::<PhysicsInterpolationPlugin>()
            .disable::<IslandPlugin>()
            .disable::<IslandSleepingPlugin>(),
    );
}
