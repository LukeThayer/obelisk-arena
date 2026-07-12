//! The rolling-glacier boulder — THE AUTHORITY FLIP. A REAL avian `RigidBody::Dynamic` ice sphere
//! (wisp `assets/bodies/glacier_ball.body.ron` physics, VERBATIM) that owns its own trajectory from
//! the CAST moment: launched FLAT at `9·charge` along the aim, it arcs under gravity, bounces/rolls
//! with wisp's exact heavy-but-lively feel, and physically shoves players.
//!
//! obelisk keeps 100% authority over damage / the frost trail / the glacier_roll+burst chain by
//! PINNING its collision windows to the ball. Both windows are `Static` (obelisk never moves them),
//! so [`pin_glacier_hitboxes`] is the SOLE mover: it writes the ball's avian `Position` into the
//! pinned hitbox's `Transform` each FixedUpdate, ordered BEFORE obelisk's overlap detection +
//! trail painting (both `ObeliskSet::ResolveHits`) — so `detect_overlaps` (contact damage) and
//! `paint_surfaces` (the `frost` Trail) read the ball's live position and Just Work, following the
//! ball's REAL path (bank shots included).
//!
//! Phase transitions stay obelisk but are PHYSICS-fired: [`detect_glacier_landing`] watches avian
//! `CollisionStart` for the ball's first GROUND contact while its FLIGHT hitbox is live and fires
//! the SAME `obelisk_bevy::events::HitboxWorldHit` event `arena_sim::obelisk::report_world_hits`
//! fires. obelisk ends the flight hitbox as `EndReason::HitWorld`, which chains `glacier_roll`
//! (rules `on_impact`) at the contact point. The `glacier_roll` `on_window_roll` verb then RE-PINS
//! this ball to the new roll hitbox (NO new ball); the roll's 6.5s fuse (`on_end_roll`) despawns
//! the ball. Server-only (the client mirror is a Kinematic copy — see `client/net.rs`).

use avian3d::prelude::{
    AngularDamping, Collider, CollisionEventsEnabled, CollisionLayers, CollisionStart, Friction,
    LayerMask, LinearDamping, LinearVelocity, Mass, Position, Restitution, RigidBody, Sensor,
};
use bevy::prelude::*;
use obelisk_bevy::prelude::Hitbox;
use serde_json::json;

use crate::net::protocol::NetworkedPlayer;
use crate::trace;

// --- Wisp physics (glacier_ball.body.ron, VERBATIM) ---------------------------------------------

/// Ball collider radius — wisp `mesh`/`collider: Sphere(radius: 0.32)`.
pub(crate) const GLACIER_BALL_RADIUS: f32 = 0.32;
/// Ball mass — wisp `mass: 6.0` (avian explicit [`Mass`], overriding the collider's density-derived
/// mass — "high so impacts hit hard").
const GLACIER_BALL_MASS: f32 = 6.0;
/// Low friction so the thrown ball keeps momentum once it lands — wisp `friction: 0.2`.
const GLACIER_BALL_FRICTION: f32 = 0.2;
/// Lively restitution — wisp `restitution: 0.4`.
const GLACIER_BALL_RESTITUTION: f32 = 0.4;
/// Wisp `linear_damping: 0.05`.
const GLACIER_BALL_LINEAR_DAMPING: f32 = 0.05;
/// Wisp `angular_damping: 0.05`.
const GLACIER_BALL_ANGULAR_DAMPING: f32 = 0.05;
/// Flat throw speed — wisp `rolling_glacier.spell.ron` `launch: Throw(forward: 9.0, up: 0.0)`. The
/// hand height provides the drop; the throw itself is horizontal (do NOT launch along the pitched
/// eye ray). Scaled by the cast's `charge_mult` at spawn.
pub(crate) const GLACIER_BALL_THROW_SPEED: f32 = 9.0;

/// Ball lifetime cap: flight fuse (2.0, the mid-air `active_duration`) + roll fuse (6.5) + margin,
/// so the ball outlives its roll and the `on_end_roll` cue despawns it FIRST; the lifetime only
/// reaps a ball whose end cue was dropped (evicted by the cap, or the cue lost).
pub(crate) const GLACIER_BALL_LIFETIME: f32 = 2.0 + 6.5 + 0.5;

/// Forward clearance: spawn the ball this far forward of the casting hand along the FLAT aim, so a
/// mass-6 sphere never materializes INSIDE the caster's capsule (r 0.35) and violently shoves the
/// (light, gate-critical, stationary) caster at spawn. The hand sits ~0.41 m horizontally from the
/// caster centre (`CAST_HAND_OFFSET (0.32, .., -0.25)`); nudging 0.5 m forward puts the ball centre
/// ≥ 0.35+0.32 = 0.67 m out.
const GLACIER_BALL_SPAWN_CLEARANCE: f32 = 0.5;

/// The ball's collision layers — member `Default`, filters `Ground | Player`: it lands + rolls on
/// the ground and physically SHOVES players, but is not itself Ground/Player (so it never blocks a
/// world-hit exemption or collides with another ball). IDENTICAL on the server (the authoritative
/// Dynamic body) and the client mirror (a Kinematic copy) — a mismatch rubber-bands predicted
/// players through the shove — so it lives here, shared by both spawn sites.
pub(crate) fn glacier_ball_layers() -> CollisionLayers {
    CollisionLayers::new(
        arena_sim::GameLayer::Default,
        LayerMask::from(arena_sim::GameLayer::Ground)
            | LayerMask::from(arena_sim::GameLayer::Player),
    )
}

/// Which obelisk hitbox is currently PINNED to a glacier ball, and the ball's phase. [`pin_glacier_hitboxes`]
/// drags `hitbox`'s Transform to the ball; [`detect_glacier_landing`] fires the world-hit on `hitbox`
/// on the first ground contact while `phase == Flight`, then the `glacier_roll` verb re-points
/// `hitbox` at the roll window + flips `phase` (the re-pin).
#[derive(Component)]
pub(crate) struct PinnedBall {
    pub hitbox: Entity,
    pub phase: BallPhase,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum BallPhase {
    /// Thrown, arcing to the ground — the flight window is pinned; the next ground contact lands it.
    Flight,
    /// Landed + rolling — the roll window is pinned; ground contacts are floor, not a landing.
    Rolling,
}

/// The BACK-link (ball ← hitbox): stamped on a PINNED obelisk hitbox, naming the glacier ball that
/// drags it — the mirror of [`PinnedBall::hitbox`]. It exists ONLY so [`end_orphaned_glacier_hitboxes`]
/// (the watchdog) can still find a hitbox whose ball is GONE: `PinnedBall` lives ON the ball and
/// despawns WITH it (cap eviction / lifetime reap), so the forward link vanishes the instant the ball
/// dies — but this back-link rides the (still-live, sole-mover-gone) hitbox and survives. Stamped at
/// BOTH pin sites (`server/verbs.rs`: flight spawn + roll re-pin), right alongside the `PinnedBall`
/// write; obelisk despawns the hitbox on end, taking this with it (normal fuse-end never orphans).
#[derive(Component)]
pub(crate) struct PinnedHitbox {
    pub ball: Entity,
}

// --- Bundles (shared spawn recipes) -------------------------------------------------------------

/// wisp's full Dynamic physics for a freshly-spawned ball (the `RigidBody::Dynamic` + `Collider`
/// come from `spawn_skill_object`'s physics tuple; this adds the rest): mass/friction/restitution/
/// damping VERBATIM, the shove layers, collision-event opt-in (for the landing detector), the flat
/// launch velocity, and the flight-hitbox pin link.
pub(crate) fn ball_physics_bundle(launch: Vec3, flight_hitbox: Entity) -> impl Bundle {
    (
        Mass(GLACIER_BALL_MASS),
        Friction::new(GLACIER_BALL_FRICTION),
        Restitution::new(GLACIER_BALL_RESTITUTION),
        LinearDamping(GLACIER_BALL_LINEAR_DAMPING),
        AngularDamping(GLACIER_BALL_ANGULAR_DAMPING),
        LinearVelocity(launch),
        glacier_ball_layers(),
        CollisionEventsEnabled,
        PinnedBall {
            hitbox: flight_hitbox,
            phase: BallPhase::Flight,
        },
    )
}

/// The client's LOCAL mirror of the ball: a `RigidBody::Kinematic` sphere with the IDENTICAL
/// collider + layers, so predicted Dynamic players physically collide with it (the shove doesn't
/// rubber-band). Driven by the replicated `Position` (AvianReplicationMode::Position). Gameplay
/// parity — attached on EVERY client (headless included), unlike the windowed-only visuals.
pub(crate) fn client_ball_mirror_bundle() -> impl Bundle {
    (
        RigidBody::Kinematic,
        Collider::sphere(GLACIER_BALL_RADIUS),
        glacier_ball_layers(),
    )
}

// --- Pure helpers (unit-tested) -----------------------------------------------------------------

/// Flatten an aim direction onto the XZ ground plane and normalize (wisp throws FLAT — `up: 0.0`;
/// the hand height provides the drop). `None` if the aim was purely vertical.
pub(crate) fn flat_launch(aim: Vec3) -> Option<Vec3> {
    Vec3::new(aim.x, 0.0, aim.z).try_normalize()
}

/// The ball spawn position: the casting `hand` nudged forward along the FLAT aim by
/// [`GLACIER_BALL_SPAWN_CLEARANCE`], so a mass-6 sphere clears the caster capsule at spawn.
pub(crate) fn ball_spawn_pos(hand: Vec3, flat_aim: Vec3) -> Vec3 {
    hand + flat_aim * GLACIER_BALL_SPAWN_CLEARANCE
}

// --- Systems (server) ---------------------------------------------------------------------------

/// The pin: write each pinned ball's avian `Position` into its obelisk hitbox's `Transform` each
/// FixedUpdate. Registered ordered `.after(StepSimulation).after(Projectiles).before(ResolveHits)`
/// (server/mod.rs) — the same proven slot as `refresh_spatial_pipeline_pre_detect` — so
/// `detect_overlaps` + `paint_surfaces` read the ball's just-integrated position. A ball whose
/// pinned hitbox has despawned (the flight→roll handoff, or the roll's fuse) is skipped; the roll
/// verb re-pins a tick or two after landing.
pub(crate) fn pin_glacier_hitboxes(
    balls: Query<(&Position, &PinnedBall)>,
    mut hitboxes: Query<&mut Transform, With<Hitbox>>,
) {
    for (pos, pin) in &balls {
        if let Ok(mut tf) = hitboxes.get_mut(pin.hitbox) {
            if tf.translation != pos.0 {
                tf.translation = pos.0;
            }
        }
    }
}

/// Fire the genuine `HitboxWorldHit` on a flight ball's first GROUND contact — obelisk ends the
/// flight hitbox as `HitWorld`, which chains `glacier_roll` at the contact point (rules `on_impact`,
/// advance.rs: `HitWorld → OnImpact`). Reads avian `CollisionStart` (the ball carries
/// `CollisionEventsEnabled`). A contact whose OTHER collider is a player is a SHOVE (ignore — the
/// ball's whole point); a sensor is a hurtbox/surface patch (ignore); anything else is the world
/// geometry the ball landed on (the SAME "non-combatant, non-sensor" rule `report_world_hits` uses).
/// Flips the ball to `Rolling` so a rolling ball's endless floor contacts don't re-fire.
pub(crate) fn detect_glacier_landing(
    mut contacts: MessageReader<CollisionStart>,
    mut balls: Query<(&Position, &mut PinnedBall)>,
    players: Query<(), With<NetworkedPlayer>>,
    sensors: Query<(), With<Sensor>>,
    mut commands: Commands,
) {
    for ev in contacts.read() {
        // Identify which side is a ball and what it touched (balls never collide with balls).
        let (ball_entity, other) = if balls.contains(ev.collider1) {
            (ev.collider1, ev.collider2)
        } else if balls.contains(ev.collider2) {
            (ev.collider2, ev.collider1)
        } else {
            continue;
        };
        // A player body is a shove; a sensor is a hurtbox/patch — neither is a landing.
        if players.contains(other) || sensors.contains(other) {
            continue;
        }
        let Ok((pos, mut pin)) = balls.get_mut(ball_entity) else {
            continue;
        };
        if pin.phase != BallPhase::Flight {
            continue; // already landed — a rolling ball hits the floor every tick
        }
        pin.phase = BallPhase::Rolling;
        // The genuine world-hit event: obelisk ends the flight hitbox (HitWorld → OnImpact →
        // glacier_roll executes AT this position). Position = the ball centre (the pin retargets
        // to the roll hitbox next; the roll's CastPoint anchor only needs the landing XZ).
        commands.trigger(obelisk_bevy::events::HitboxWorldHit {
            hitbox: pin.hitbox,
            position: pos.0,
        });
        trace::event(
            "glacier_ball_landed",
            json!({ "pos": [pos.0.x, pos.0.y, pos.0.z] }),
        );
    }
}

/// The ORPHAN WATCHDOG — the guarantee that a pinned window can NEVER outlive its ball. A ball can
/// vanish while its (`motion: Static`, sole-mover-gone) obelisk hitbox is still live: evicted by the
/// per-caster cap (your OWN recast supersedes the previous boulder) or reaped by the lifetime cap. If
/// nothing acts, that hitbox freezes as an INVISIBLE damage sphere (`Enemies`/`OncePerTarget`) that
/// `detect_overlaps` keeps processing until its own fuse (up to 6.5 s of phantom damage from nothing).
///
/// Instead, for every pinned hitbox whose ball no longer exists, fire the SAME `HitboxWorldHit` the
/// landing detector uses — at the LAST pinned pose (the hitbox `Transform` the pin wrote the tick
/// before the ball died = the eviction point). obelisk ends the window `HitWorld → on_impact`, so an
/// evicted FLIGHT hitbox chains `glacier_roll` and an evicted ROLL hitbox fires `glacier_burst`: a
/// graceful burst at the eviction point, never a phantom. Then clear the back-link (exactly once;
/// obelisk despawns the hitbox the same tick, so the removal is belt-and-suspenders).
///
/// Runs in the pin's own FixedUpdate slot (`.after(Projectiles).before(ResolveHits)`) right after
/// [`pin_glacier_hitboxes`] + [`detect_glacier_landing`] — the proven slot for firing `HitboxWorldHit`
/// (the landing detector chains from exactly here). A ball that is merely mid-handoff (alive, its
/// `PinnedBall.hitbox` momentarily pointing at a despawning flight hitbox) is left ALONE — the guard
/// keys off the ball's existence, not on which hitbox it currently names.
pub(crate) fn end_orphaned_glacier_hitboxes(
    balls: Query<(), With<PinnedBall>>,
    orphans: Query<(Entity, &Transform, &PinnedHitbox), With<Hitbox>>,
    mut commands: Commands,
) {
    for (hitbox, tf, link) in &orphans {
        if balls.contains(link.ball) {
            continue; // ball alive — the pin is dragging this hitbox; not orphaned
        }
        commands.trigger(obelisk_bevy::events::HitboxWorldHit {
            hitbox,
            position: tf.translation,
        });
        commands.entity(hitbox).remove::<PinnedHitbox>();
        trace::event(
            "glacier_ball_orphan_ended",
            json!({ "pos": [tf.translation.x, tf.translation.y, tf.translation.z] }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_launch_flattens_and_normalizes() {
        // A pitched-DOWN aim flattens to a unit XZ heading (wisp throws flat; the hand height, not
        // the aim pitch, provides the drop).
        let d = flat_launch(Vec3::new(1.0, -0.5, 0.0)).expect("a horizontal component exists");
        assert!((d.length() - 1.0).abs() < 1e-6, "normalized");
        assert_eq!(d.y, 0.0, "flattened to the ground plane");
        assert!(d.x > 0.99, "keeps the +X heading");
        // A purely vertical aim has no ground heading.
        assert!(flat_launch(Vec3::Y).is_none());
    }

    #[test]
    fn ball_spawns_clear_of_the_caster_capsule() {
        // Hand ~0.41 m from the caster centre (CAST_HAND_OFFSET (0.32, .., -0.25)); nudged forward
        // 0.5 m the ball centre is ≥ caster_radius(0.35)+ball_radius(0.32)=0.67 clear, so a mass-6
        // sphere never spawns inside the caster (which would shove the stationary gate caster).
        let hand = Vec3::new(0.25, 1.14, 0.32); // caster at origin facing +X
        let ball = ball_spawn_pos(hand, Vec3::X);
        let horiz = Vec3::new(ball.x, 0.0, ball.z).length();
        assert!(
            horiz >= 0.35 + GLACIER_BALL_RADIUS,
            "ball centre {horiz} m out must clear caster(0.35)+ball(0.32)=0.67",
        );
    }
}
