//! obelisk-arena shared library: gameplay + netcode shared by the `arena-server`, `arena-client`,
//! and `arena-observer` bins under `src/bin/`. Each bin composes its own plugin set on top of these
//! modules — the server adds `MinimalPlugins` + `ServerNetPlugin` + `ArenaServerPlugin`; the client
//! runs `client::run_windowed_client` (DefaultPlugins) + `ClientNetPlugin`.
//!
//! M2 reshaped M1's co-located `main.rs` into this lib + the three bins (netcode guide §2).

use avian3d::prelude::*;
use bevy::prelude::*;
use std::path::PathBuf;

pub mod cast_assets;
pub mod client;
pub mod net;
pub mod server;
pub mod shared_controller;
pub mod skills;
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

/// Add avian physics with the lightyear integration plugin. Adapted from wisp's
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
/// ARENA DIVERGENCE FROM WISP: the physics group runs on `FixedUpdate` (`PhysicsPlugins::new(
/// FixedUpdate)`, not wisp's `::default()`) to match obelisk-bevy's spatial layer, whose
/// `ObeliskSet` schedule + hit detection live in `FixedUpdate` (obelisk-bevy CLAUDE.md). obelisk's
/// own `ObeliskSpatialPlugin` would add the SAME group and double-add-panic, so the obelisk sim is
/// composed via [`add_obelisk_sim_headless`] which omits `ObeliskSpatialPlugin`; this function is
/// the single physics-adding site.
///
/// Must be called AFTER `ServerPlugins`/`ClientPlugins` so `LightyearAvianPlugin` sees the
/// replication infrastructure, and AFTER [`add_obelisk_sim_headless`] is NOT required (order-free).
pub fn add_avian_with_lightyear(app: &mut App) {
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
    // Arena gravity (snappier-than-Earth arcade feel). With JUMP_SPEED = 7 this gives a jump apex
    // of ≈1.22 m. Shared by every peer so prediction + the server integrate identically.
    app.insert_resource(Gravity(Vec3::new(0.0, -crate::net::GRAVITY, 0.0)));
}

/// The STATIC arena floor collider spawn (the Dynamic player bodies rest on it) — now lifted into the
/// transport-agnostic `arena_sim` crate and re-exported here so the game keeps its
/// `crate::spawn_arena_floor` call sites unchanged.
pub use arena_sim::spawn::spawn_arena_floor;

/// The obelisk sim composition — `add_obelisk_sim` (+ the headless/client wrappers + the
/// `refresh_spatial_pipeline*` systems) now lives in the transport-agnostic `arena_sim` crate so the
/// live game (lightyear host, via [`add_avian_with_lightyear`]) and the editor preview (plain-Avian
/// host) share the EXACT obelisk composition; physics is parameterized to the host. Re-exported here
/// so the bins keep their `add_obelisk_sim_headless`/`add_obelisk_sim_client` call sites unchanged.
pub use arena_sim::obelisk::{add_obelisk_sim, add_obelisk_sim_client, add_obelisk_sim_headless};
