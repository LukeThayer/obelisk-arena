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
}

/// Compose obelisk-bevy's headless sim sub-plugins INDIVIDUALLY, deliberately omitting
/// `ObeliskSpatialPlugin` (which unconditionally adds `PhysicsPlugins::new(FixedUpdate)` and would
/// double-add-panic against [`add_avian_with_lightyear`]). All other obelisk sub-plugins
/// (`ObeliskAssetsPlugin`, `ObeliskCorePlugin`, `ObeliskCombatPlugin`, `ObeliskNetPlugin`,
/// `ObeliskCuePlugin`, `ObeliskLootPlugin`) are headless-capable and carry no physics.
///
/// This reproduces `ObeliskSimPlugin::build` VERBATIM (obelisk-bevy `src/lib.rs:107-139`) minus the
/// `ObeliskSpatialPlugin` line, so the lightyear-avian physics is the sole `PhysicsPlugins`
/// registrant. The `ObeliskSet` chain `configure_sets` + the timeline/projectile/detect
/// `add_systems` live in `ObeliskSimPlugin::build` itself (not in a sub-plugin), so they MUST be
/// re-added here — omitting them would silently break all casting/hit-detection. All referenced
/// items (`ObeliskSet`, `timeline::advance::*`, `spatial::{projectile,detect}::*`) are `pub`.
///
/// Keep this in sync with `ObeliskSimPlugin::build` if obelisk-bevy's sim composition changes.
pub fn add_obelisk_sim_headless(app: &mut App) {
    use obelisk_bevy::{assets, combat, core, loot, net, spatial, timeline, vfx, ObeliskSet};

    app.add_plugins(assets::ObeliskAssetsPlugin)
        // NB: ObeliskSpatialPlugin deliberately OMITTED — it adds the avian PhysicsPlugins group
        // that `add_avian_with_lightyear` owns. Everything else matches ObeliskSimPlugin.
        .add_plugins(core::ObeliskCorePlugin)
        .add_plugins(combat::ObeliskCombatPlugin)
        .add_plugins(net::ObeliskNetPlugin)
        .add_plugins(vfx::ObeliskCuePlugin)
        .add_plugins(loot::ObeliskLootPlugin);

    app.configure_sets(
        FixedUpdate,
        (
            ObeliskSet::Validate,
            ObeliskSet::Advance,
            ObeliskSet::Projectiles,
            ObeliskSet::ResolveHits,
            ObeliskSet::TickEffects,
        )
            .chain(),
    );

    app.add_systems(
        FixedUpdate,
        (
            timeline::advance::validate_casts.in_set(ObeliskSet::Validate),
            (
                timeline::advance::advance_casts,
                timeline::advance::expire_hitboxes,
            )
                .in_set(ObeliskSet::Advance),
            spatial::projectile::move_projectiles.in_set(ObeliskSet::Projectiles),
            spatial::detect::detect_overlaps.in_set(ObeliskSet::ResolveHits),
        ),
    );
}
