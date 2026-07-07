//! Compose obelisk-bevy's sim sub-plugins under a host-provided physics group.
//!
//! Lifted out of `arena_game` into the transport-agnostic `arena_sim` crate so the live game
//! (lightyear host) and the editor preview (plain-Avian host) share the EXACT obelisk composition.
//! Physics is the HOST's job: this module never installs `PhysicsPlugins` — `arena_game`'s
//! `add_avian_with_lightyear` (game) or `ArenaSimPreviewPlugin` (editor) owns the sole
//! `PhysicsPlugins` registrant, and obelisk's own `ObeliskSpatialPlugin` is deliberately omitted in
//! BOTH paths so the physics group is never double-added.

use bevy::prelude::*;

/// Compose obelisk-bevy's sim sub-plugins INDIVIDUALLY — the shared body behind
/// [`add_obelisk_sim_headless`] and [`add_obelisk_sim_client`]. Both paths deliberately omit
/// `ObeliskSpatialPlugin` (which unconditionally adds `PhysicsPlugins::new(FixedUpdate)` and would
/// double-add-panic against the host's physics registrant, guide risk #3). The shared sub-plugins
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
/// This re-syncs with the post-trigger-reform `ObeliskSimPlugin::build` (obelisk-bevy
/// `src/lib.rs:107-197`) — including the reform's `timeline::advance::tick_emitters` (live
/// hitboxes' emitters) and `timeline::triggered::advance_triggered_execs` (the executor for a
/// rules trigger's spawned `TriggeredExec`, e.g. firebolt's on-impact explosion) in the `Advance`
/// set, in the same order/chaining as upstream — minus the `ObeliskSpatialPlugin` line (and, for
/// the client, minus combat), so the host's physics is the sole `PhysicsPlugins` registrant. The
/// `ObeliskSet` chain `configure_sets` + the timeline/projectile/detect `add_systems` live in
/// `ObeliskSimPlugin::build` itself (not in a sub-plugin), so they MUST be re-added here —
/// omitting them would silently break all casting/hit-detection. All referenced items
/// (`ObeliskSet`, `timeline::advance::*`, `timeline::triggered::*`, `spatial::{projectile,detect}::*`)
/// are `pub`.
///
/// One INTENTIONAL divergence: `ObeliskSimPlugin::build` also pins `FixedUpdate` to the
/// single-threaded executor (obelisk-bevy `src/lib.rs:129-131`) because its standalone sim owns
/// `FixedUpdate` outright and wants structural (not convention-based) determinism. The arena does
/// NOT pin it here — `FixedUpdate` is shared with lightyear's prediction/rollback and avian's
/// physics step, and forcing single-threaded would serialize their systems too, well beyond this
/// module's remit. Damage determinism does not depend on it: the reference content uses fixed
/// min==max damages, and the `chain_ignore_deferred()` ordering above is preserved exactly to
/// close the specific `Advance`-set race the upstream comment calls out.
///
/// Keep this in sync with `ObeliskSimPlugin::build` if obelisk-bevy's sim composition changes.
pub fn add_obelisk_sim(app: &mut App, resolve_hits: bool) {
    use obelisk_bevy::{assets, combat, core, loot, net, spatial, timeline, vfx, ObeliskSet};

    app.add_plugins(assets::ObeliskAssetsPlugin)
        // NB: ObeliskSpatialPlugin deliberately OMITTED in BOTH paths — it adds the avian
        // PhysicsPlugins group that the host owns. Everything else matches ObeliskSimPlugin.
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

    // Host-fired world impacts (report_world_hits below) feed obelisk's `end_hitboxes` funnel
    // via this observer (mirrors `ObeliskSimPlugin::build`).
    app.add_observer(timeline::advance::on_hitbox_world_hit);
    app.add_systems(
        FixedUpdate,
        (
            timeline::advance::validate_casts.in_set(ObeliskSet::Validate),
            (
                timeline::advance::advance_casts,
                // Re-synced with obelisk-bevy `ObeliskSimPlugin::build` (post trigger-reform):
                // `end_hitboxes` BEFORE `tick_emitters` (both `&mut Hitbox`), joined with
                // `chain_ignore_deferred()` — NOT `chain()` — so the ordering holds for the query
                // conflict without inserting a mid-`Advance` `ApplyDeferred` that would flush the
                // unordered systems' command buffers and make hitbox visibility thread-dependent
                // (a lockstep-sim divergence source). See lib.rs:149-185 for the full rationale.
                (
                    timeline::advance::end_hitboxes,
                    timeline::advance::tick_emitters,
                )
                    .chain_ignore_deferred(),
                // The trigger executor: ticks the `TriggeredExec` that a rules trigger's
                // `execute_skill_timeline` spawns, so e.g. firebolt_explosion actually runs its
                // timeline at the impact. Absent here pre-reform => triggered skills silently no-op.
                timeline::triggered::advance_triggered_execs,
            )
                .in_set(ObeliskSet::Advance),
            (
                spatial::projectile::move_projectiles,
                // Arena rule obelisk doesn't know: WORLD GEOMETRY. A projectile hitbox whose
                // movement segment crosses a level collider (floor, wall, pillar) has HIT THE
                // WORLD — report it into obelisk's `end_hitboxes` funnel (HitWorld ending: end
                // event + authored chain reaction at the impact point). After move, before detect.
                report_world_hits,
            )
                .chain()
                .in_set(ObeliskSet::Projectiles),
        ),
    );
    if resolve_hits {
        // Hit detection is server-only. Adding `detect_overlaps` on the client would draw
        // `CombatRng` and desync (Stage-A) — it stays out of the client's `ResolveHits` set.
        // `resolve_beam_hits` is the beam counterpart (strikes the designated target directly).
        app.add_systems(
            FixedUpdate,
            (
                spatial::detect::detect_overlaps,
                spatial::detect::resolve_beam_hits,
            )
                .in_set(ObeliskSet::ResolveHits),
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
/// refresh, deliberately omitting only `ObeliskSpatialPlugin` (physics is the host's sole job). See
/// [`add_obelisk_sim`] for the full rationale.
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

/// The previous tick's hitbox position — inserted by [`report_world_hits`] on first sight of a
/// projectile, then updated each tick. Gives the world-hit check the MOVEMENT SEGMENT to cast
/// (point-in-collider checks alone would tunnel through thin walls at projectile speeds).
#[derive(Component)]
pub struct LastHitboxPos(pub Vec3);

/// Kill plane: a projectile below this world Y has left every conceivable level — end it as a
/// world hit so its window can't fly forever.
const PROJECTILE_KILL_PLANE_Y: f32 = -10.0;

/// Report any moving projectile hitbox whose movement segment (previous tick position → current)
/// intersects WORLD GEOMETRY — any non-sensor collider that isn't a combatant body (hitting a
/// player is `detect_overlaps`' job; hurtboxes are sensors) — as a WORLD HIT into obelisk's
/// termination funnel. The world is the host's knowledge (like physics), so the host detects the
/// impact and fires `HitboxWorldHit`; obelisk's `end_hitboxes` then ends the hitbox with
/// `EndReason::HitWorld` at the impact point — firing the end cue and spawning any authored
/// `on_end` chain (e.g. firebolt's ground explosion). Replaced the flat-floor `y < 0` plane check
/// when levels became real colliders (levels-and-lobby): bolts now burst on walls and pillars,
/// and a projectile that escapes the level dies at the kill plane. Runs on both peers (the client
/// has no `Hitbox` entities).
#[allow(clippy::type_complexity)]
fn report_world_hits(
    mut q: Query<
        (Entity, &Transform, Option<&mut LastHitboxPos>),
        (
            With<obelisk_bevy::prelude::Hitbox>,
            With<obelisk_bevy::spatial::projectile::Projectile>,
            Without<obelisk_bevy::timeline::advance::WorldHit>,
        ),
    >,
    combatants: Query<(), With<obelisk_bevy::prelude::Combatant>>,
    sensors: Query<(), With<avian3d::prelude::Sensor>>,
    spatial: avian3d::prelude::SpatialQuery,
    mut commands: Commands,
) {
    for (e, tf, last) in &mut q {
        let cur = tf.translation;
        // First sight: no segment yet — record and check next tick (spawn is at the muzzle,
        // never inside world geometry).
        let prev = match last {
            Some(mut l) => std::mem::replace(&mut l.0, cur),
            None => {
                commands.entity(e).insert(LastHitboxPos(cur));
                continue;
            }
        };
        if cur.y < PROJECTILE_KILL_PLANE_Y {
            commands.trigger(obelisk_bevy::events::HitboxWorldHit {
                hitbox: e,
                position: cur,
            });
            continue;
        }
        let segment = cur - prev;
        let length = segment.length();
        if length <= 1e-6 {
            continue;
        }
        let Ok(dir) = Dir3::new(segment / length) else {
            continue;
        };
        let hit = spatial.cast_ray_predicate(
            prev,
            dir,
            length,
            true,
            &avian3d::prelude::SpatialQueryFilter::default(),
            &|entity| !combatants.contains(entity) && !sensors.contains(entity),
        );
        if let Some(hit) = hit {
            let position = prev + *dir * hit.distance;
            commands.trigger(obelisk_bevy::events::HitboxWorldHit {
                hitbox: e,
                position,
            });
        }
    }
}

/// Force the avian `SpatialQueryPipeline` to reflect the current collider set. See the call sites in
/// [`add_obelisk_sim`] for why this explicit refresh is required under `LightyearAvianPlugin`. Takes
/// `SpatialQuery` mutably so it can call `update_pipeline()`.
fn refresh_spatial_pipeline(mut spatial: avian3d::prelude::SpatialQuery) {
    spatial.update_pipeline();
}

/// Second instance of [`refresh_spatial_pipeline`], a distinct system so it can carry its own
/// ordering constraints (immediately before `detect_overlaps`) without colliding with the
/// pre-validation instance.
fn refresh_spatial_pipeline_pre_detect(mut spatial: avian3d::prelude::SpatialQuery) {
    spatial.update_pipeline();
}
