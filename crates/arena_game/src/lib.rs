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

/// Spawn the STATIC arena floor collider on this peer. The Dynamic player bodies rest on it. A
/// cuboid spanning `±FLOOR_HALF` horizontally + 1 m thick, positioned so its TOP face is at world
/// Y = 0 (the green visual plane); a capsule(0.35, 0.48) body (half-height 0.59) then rests with
/// its origin at [`net::GROUND_Y`] = 0.59 and its feet at world 0. Spawned locally (identical on
/// every peer — no need to replicate static geometry).
pub fn spawn_arena_floor(commands: &mut Commands) {
    const FLOOR_SIZE: f32 = 40.0;
    const FLOOR_THICKNESS: f32 = 1.0;
    commands.spawn((
        Name::new("ArenaFloor"),
        RigidBody::Static,
        Collider::cuboid(FLOOR_SIZE, FLOOR_THICKNESS, FLOOR_SIZE),
        Position(Vec3::new(0.0, -FLOOR_THICKNESS / 2.0, 0.0)),
        Rotation::default(),
    ));
}

/// Compose obelisk-bevy's sim sub-plugins INDIVIDUALLY — the shared body behind
/// [`add_obelisk_sim_headless`] and [`add_obelisk_sim_client`]. Both paths deliberately omit
/// `ObeliskSpatialPlugin` (which unconditionally adds `PhysicsPlugins::new(FixedUpdate)` and would
/// double-add-panic against [`add_avian_with_lightyear`], guide risk #3). The shared sub-plugins
/// (`ObeliskAssetsPlugin`, `ObeliskCorePlugin`, `ObeliskNetPlugin`, `ObeliskCuePlugin`,
/// `ObeliskLootPlugin`) are headless-capable and carry no physics.
///
/// `resolve_hits` selects the authority tier — it is the ONLY difference between the two variants:
/// - `true` (headless server): additionally adds `ObeliskCombatPlugin`, the `detect_overlaps`
///   (`ObeliskSet::ResolveHits`) system, and the second (pre-`detect`) spatial-pipeline refresh.
/// - `false` (client, Stage-A invariant, guide risk #2): omits all three — **the client never
///   resolves hits and never touches `CombatRng`.** Hit resolution + damage are 100%
///   server-authoritative; the client only predicts cast initiation + projectile MOTION (timeline +
///   `move_projectiles`) for zero-latency cosmetics, and renders the server's replicated
///   `DamageResolved`. The `ResolveHits` set is still CONFIGURED in the chain (so anything ordering
///   against it resolves) but no system runs in it.
///
/// This reproduces `ObeliskSimPlugin::build` VERBATIM (obelisk-bevy `src/lib.rs:107-139`) minus the
/// `ObeliskSpatialPlugin` line (and, for the client, minus combat), so the lightyear-avian physics
/// is the sole `PhysicsPlugins` registrant. The `ObeliskSet` chain `configure_sets` + the
/// timeline/projectile/detect `add_systems` live in `ObeliskSimPlugin::build` itself (not in a
/// sub-plugin), so they MUST be re-added here — omitting them would silently break all
/// casting/hit-detection. All referenced items (`ObeliskSet`, `timeline::advance::*`,
/// `spatial::{projectile,detect}::*`) are `pub`.
///
/// Keep this in sync with `ObeliskSimPlugin::build` if obelisk-bevy's sim composition changes.
fn add_obelisk_sim(app: &mut App, resolve_hits: bool) {
    use obelisk_bevy::{assets, combat, core, loot, net, spatial, timeline, vfx, ObeliskSet};

    app.add_plugins(assets::ObeliskAssetsPlugin)
        // NB: ObeliskSpatialPlugin deliberately OMITTED in BOTH paths — it adds the avian
        // PhysicsPlugins group that `add_avian_with_lightyear` owns. Everything else matches
        // ObeliskSimPlugin.
        .add_plugins(core::ObeliskCorePlugin);
    if resolve_hits {
        // ObeliskCombatPlugin is server-only — Stage-A: the client NEVER runs the resolve funnel /
        // RNG. Inserted between Core and Net to match `ObeliskSimPlugin::build`'s ordering.
        app.add_plugins(combat::ObeliskCombatPlugin);
    }
    app.add_plugins(net::ObeliskNetPlugin)
        .add_plugins(vfx::ObeliskCuePlugin)
        .add_plugins(loot::ObeliskLootPlugin);

    app.configure_sets(
        FixedUpdate,
        (
            ObeliskSet::Validate,
            ObeliskSet::Advance,
            ObeliskSet::Projectiles,
            // ResolveHits is always CONFIGURED so ordering resolves; on the client (resolve_hits ==
            // false) no system runs in it — the Stage-A hard exclusion.
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
        ),
    );
    if resolve_hits {
        // Hit detection is server-only. Adding `detect_overlaps` on the client would draw
        // `CombatRng` and desync (Stage-A) — it stays out of the client's `ResolveHits` set.
        app.add_systems(
            FixedUpdate,
            spatial::detect::detect_overlaps.in_set(ObeliskSet::ResolveHits),
        );
    }

    // Refresh the avian spatial-query pipeline right before obelisk reads it, every FixedUpdate.
    //
    // WHY: obelisk's `validate_casts` (LOS raycast + range) and `detect_overlaps`
    // (`shape_intersections` hitbox↔hurtbox) read the `SpatialQueryPipeline`. Avian normally
    // refreshes it once per physics step in `PhysicsStepSystems::SpatialQuery`. Under
    // `LightyearAvianPlugin::Position` (which disables `PhysicsTransformPlugin` and reshuffles the
    // physics sets into `RunFixedMainLoop`/`FixedPostUpdate`), that auto-refresh no longer lands
    // before obelisk's `FixedUpdate` sets — empirically the pipeline read by `detect_overlaps` was
    // EMPTY, so the firebolt window flew straight through the target hurtbox and never resolved
    // damage (a manual `update_pipeline()` immediately found the hurtboxes). This explicit refresh,
    // ordered before `ObeliskSet::Validate` (so the whole chain sees a populated pipeline), restores
    // the M1/M0 invariant that obelisk's spatial queries see the current colliders. Cheap: a BVH
    // rebuild over the handful of arena colliders each tick. (obelisk's own `ObeliskSpatialPlugin`
    // — which we deliberately omit to avoid double-adding the physics group — relies on the same
    // auto-refresh, so re-establishing it here is required for headless authority.)
    use avian3d::prelude::PhysicsSystems;
    // Before validation (LOS raycast + range). Ordered AFTER avian's physics step so our rebuild
    // isn't immediately clobbered by avian's own pipeline update. Required on BOTH peers (a client
    // `Validate` LOS read could otherwise see an empty pipeline) — harmless on the client.
    app.add_systems(
        FixedUpdate,
        refresh_spatial_pipeline
            .after(PhysicsSystems::StepSimulation)
            .before(ObeliskSet::Validate),
    );
    if resolve_hits {
        // AND immediately before hit detection (after the projectile moved + after the physics
        // step), so `detect_overlaps` reads a freshly-built pipeline. Under `LightyearAvianPlugin`
        // (disabled `PhysicsTransformPlugin`, physics step in `PhysicsSystems::StepSimulation`
        // within FixedUpdate), avian's own per-step pipeline refresh produced an EMPTY view for
        // obelisk's reads — the firebolt window flew straight through the target hurtbox and never
        // resolved damage. Rebuilding the pipeline here, after the step and right before detect,
        // restores M0/M1's hit detection. Server-only: the client runs no `detect_overlaps`.
        app.add_systems(
            FixedUpdate,
            refresh_spatial_pipeline_pre_detect
                .after(PhysicsSystems::StepSimulation)
                .after(ObeliskSet::Projectiles)
                .before(ObeliskSet::ResolveHits),
        );
    }
    // Also refresh in Update: the server's `drain_cast_requests` re-validation (`nearest_enemy`)
    // runs in Update, where the pipeline would otherwise reflect only the last FixedUpdate.
    app.add_systems(Update, refresh_spatial_pipeline);
}

/// Compose obelisk-bevy's HEADLESS (server-authoritative) sim — thin wrapper over
/// [`add_obelisk_sim`] with `resolve_hits = true`. Adds every shared sub-plugin PLUS
/// `ObeliskCombatPlugin` + `detect_overlaps` (`ObeliskSet::ResolveHits`) + the pre-`detect` spatial
/// refresh, deliberately omitting only `ObeliskSpatialPlugin` (physics is
/// [`add_avian_with_lightyear`]'s sole job). See [`add_obelisk_sim`] for the full rationale.
pub fn add_obelisk_sim_headless(app: &mut App) {
    add_obelisk_sim(app, true);
}

/// Compose the CLIENT-appropriate obelisk subset (netcode guide §6.4, Stage-A invariant) — thin
/// wrapper over [`add_obelisk_sim`] with `resolve_hits = false`. Identical to
/// [`add_obelisk_sim_headless`] EXCEPT it omits `ObeliskCombatPlugin`, the `detect_overlaps`
/// (`ObeliskSet::ResolveHits`) system, and the pre-`detect` refresh — the Stage-A invariant (guide
/// risk #2): **the client never resolves hits and never touches `CombatRng`.**
///
/// It still adds `ObeliskAssetsPlugin` (the `CastTimeline` asset + `.cast.ron` loader the
/// cosmetics + `register_predicted_sim` cue lookup need), `ObeliskCorePlugin` (`SkillRegistry` /
/// `CombatRng` / config infra that `add_obelisk_skills` / `seed_combat_rng` populate +
/// `ObeliskEntityIndex`), `ObeliskCuePlugin`, `ObeliskNetPlugin`, `ObeliskLootPlugin`, and the
/// timeline/projectile systems (Validate / Advance / Projectiles) — but **not** ResolveHits.
///
/// Why include the timeline/projectile sets when the client issues no obelisk casts today: they are
/// inert without an `ActiveCast` on a client entity (the networked client's cast goes over the wire,
/// the materialized players are render proxies), so they cost nothing, and they keep the door open
/// for a future predicted-local-obelisk-cast pass without a re-compose. The pre-validation
/// spatial-pipeline refresh is kept for the same `LightyearAvianPlugin` reason as the server (a
/// `Validate` LOS read could otherwise see an empty pipeline) — harmless on the client.
pub fn add_obelisk_sim_client(app: &mut App) {
    add_obelisk_sim(app, false);
}

/// Force the avian `SpatialQueryPipeline` to reflect the current collider set. See the call sites in
/// [`add_obelisk_sim_headless`] for why this explicit refresh is required under
/// `LightyearAvianPlugin`. Takes `SpatialQuery` mutably so it can call `update_pipeline()`.
fn refresh_spatial_pipeline(mut spatial: avian3d::prelude::SpatialQuery) {
    spatial.update_pipeline();
}

/// Second instance of [`refresh_spatial_pipeline`], a distinct system so it can carry its own
/// ordering constraints (immediately before `detect_overlaps`) without colliding with the
/// pre-validation instance.
fn refresh_spatial_pipeline_pre_detect(mut spatial: avian3d::prelude::SpatialQuery) {
    spatial.update_pipeline();
}
