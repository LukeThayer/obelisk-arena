//! Shared force-based character controller, driven by native [`ArenaInput`].
//!
//! Ported from the canonical `avian_3d_character/src/shared.rs::apply_character_action`, with the
//! leafwing `ActionState<CharacterAction>` swapped for the arena's native `ActionState<ArenaInput>`
//! (the `simple_box` input pattern). Runs in `FixedUpdate` on BOTH peers:
//!   - server: over every authoritative character (`Without<Predicted>`);
//!   - client: over the local `Predicted` character — lightyear re-runs it during rollback, so the
//!     `ActionState<ArenaInput>` is always correct for the tick being (re)simulated.
//!
//! The body is `RigidBody::Dynamic` with all rotation axes locked. Movement applies a world-space
//! FORCE that accelerates the planar velocity toward `move_dir * MAX_SPEED` (avian's `move_towards`
//! recipe); a grounded jump applies an upward impulse. Body facing (yaw) is a SEPARATE write to
//! avian `Rotation` (avian's `Forces` borrows `Rotation` internally, so the two can't share a
//! query). Ground state comes from [`grounded_by_ray`] — a short downward raycast from the capsule
//! bottom (levels are real geometry now: platforms/ramps at any height, not one flat floor) —
//! computed by the CALLER and passed into [`apply_arena_movement`], keeping the controller pure
//! and rollback-friendly (both peers compute it identically from the same pipeline state, with the
//! caster body + its hurtbox child excluded so the ray can't ground on itself).

use avian3d::prelude::forces::ForcesItem;
use avian3d::prelude::*;
use bevy::prelude::*;

use crate::input::ArenaInput;

/// Top planar movement speed (m/s). Matches the old kinematic `MOVE_SPEED` so the feel target
/// (≈4 m/s) is preserved after the kinematic→Dynamic switch.
pub const MAX_SPEED: f32 = 4.0;
/// Max planar acceleration (m/s²): how fast the body ramps to/from `MAX_SPEED`. Higher = snappier.
pub const MAX_ACCELERATION: f32 = 30.0;
/// Upward velocity (m/s) imparted on a grounded jump. With the arena `Gravity` (−20 m/s²) this gives
/// an apex of `JUMP_SPEED² / (2·g)` ≈ 1.22 m (the spec's 1.0–1.2 m target).
pub const JUMP_SPEED: f32 = 7.0;
/// Fraction of [`MAX_ACCELERATION`] available while airborne (`!grounded`). The movement force is
/// applied UNCONDITIONALLY of ground state, but the per-tick velocity delta is scaled by this when
/// airborne. `1.0` = full air control, identical to a grounded tick — today's behavior, preserved
/// exactly. Lower it (<1.0) to make mid-air direction changes feel floatier/committed.
pub const AIR_CONTROL: f32 = 1.0;

/// How far below the capsule BOTTOM the ground may be while still counting as grounded (m).
/// Generous enough to keep the jump responsive across solver micro-separation, small enough that
/// mid-jump (apex ≈ 1.22 m) is unambiguously airborne. (The old flat-floor threshold allowed
/// 0.05 m above rest height; rest already sits ~0 m from the floor, so 0.15 m of ray is the same
/// feel with margin for ramps/steps.)
pub const GROUNDED_RAY_SLACK: f32 = 0.15;

/// The world-space BOTTOM point of the player capsule for a body centered at `center`
/// (half-height = half-length + radius).
pub fn capsule_bottom(center: Vec3) -> Vec3 {
    center
        - Vec3::Y * (crate::tuning::PLAYER_CAPSULE_LENGTH * 0.5 + crate::tuning::PLAYER_CAPSULE_RADIUS)
}

/// Raycast ground check: a ray straight DOWN from the capsule bottom; a hit within
/// [`GROUNDED_RAY_SLACK`] ⇒ grounded. `exclude` must carry the body + its child colliders (the
/// hurtbox sensor) so the ray can't ground on the caster itself. `solid: true` so a
/// solver-penetrated floor still reads as ground (hit at distance 0). Works on ANY level geometry
/// (platforms, ramps) — this replaced the flat-floor `pos_y <= GROUND_Y + 0.05` check when levels
/// became real colliders (levels-and-lobby). Both peers call it against the same refreshed
/// `SpatialQueryPipeline`, so prediction and the server agree.
pub fn grounded_by_ray(spatial: &SpatialQuery, body_center: Vec3, exclude: &[Entity]) -> bool {
    // Start slightly ABOVE the capsule bottom: a body resting tangent on a floor puts the bottom
    // EXACTLY on the surface, and a ray originating on a face is a degenerate no-hit in parry.
    const LIFT: f32 = 0.05;
    let filter = SpatialQueryFilter::default().with_excluded_entities(exclude.iter().copied());
    spatial
        .cast_ray(
            capsule_bottom(body_center) + Vec3::Y * LIFT,
            Dir3::NEG_Y,
            LIFT + GROUNDED_RAY_SLACK,
            true,
            &filter,
        )
        .is_some()
}

/// Apply the planar movement force + jump impulse for one character from its `ArenaInput`.
/// `grounded` is computed by the CALLER (via [`grounded_by_ray`]) — passed in so this stays pure
/// (no spatial access) and lightyear can re-run it during rollback from the query row alone.
pub fn apply_arena_movement(
    mass: &ComputedMass,
    dt: f32,
    input: &ArenaInput,
    mut forces: ForcesItem,
    grounded: bool,
) {
    let dt = dt.max(1e-5);

    // The movement force is applied unconditionally of ground state; only the per-tick velocity
    // delta is scaled by AIR_CONTROL when airborne (AIR_CONTROL = 1.0 ⇒ full air control, no scale).
    let air_factor = if grounded { 1.0 } else { AIR_CONTROL };
    let max_velocity_delta_per_tick = MAX_ACCELERATION * dt * air_factor;

    // Camera-relative WASD → world direction (matches the client camera frame: forward = -Z,
    // strafe = +X, both in the yaw frame). Clamp the input magnitude so a diagonal isn't faster.
    let mut mv = input.movement;
    if mv.length_squared() > 1.0 {
        mv = mv.normalize();
    }
    let local = Vec3::new(mv.x, 0.0, -mv.y);
    let world_dir = Quat::from_axis_angle(Vec3::Y, input.yaw) * local;
    let desired_ground_velocity = world_dir * MAX_SPEED;

    // `move_towards` ramps planar velocity toward the desired velocity by at most
    // `max_velocity_delta_per_tick`. Releasing input makes `desired = 0`, so the body DECELERATES at
    // the full `MAX_ACCELERATION` to a dead stop — there is no momentum slide. This is deliberate:
    // avian `Friction` is 0 on the body (the controller fully owns planar velocity), so the
    // zero-target `move_towards` IS the stopping mechanism. A non-zero friction would fight it and
    // make the stop non-deterministic under rollback.
    let linear_velocity = forces.linear_velocity();
    let ground_velocity = Vec3::new(linear_velocity.x, 0.0, linear_velocity.z);
    let new_ground_velocity =
        ground_velocity.move_towards(desired_ground_velocity, max_velocity_delta_per_tick);
    let required_acceleration = (new_ground_velocity - ground_velocity) / dt;
    forces.apply_force(required_acceleration * mass.value());

    // Jump: on any grounded tick where jump is held, bring vertical velocity up to JUMP_SPEED.
    if input.jump && grounded {
        let dvy = (JUMP_SPEED - linear_velocity.y).max(0.0);
        if dvy > 0.0 {
            forces.apply_linear_impulse(Vec3::Y * dvy * mass.value());
        }
    }
}

/// Face the body to the input yaw. Written directly to avian `Rotation` each tick (the body's
/// rotation axes are locked, so physics never rotates it); deterministic under rollback because the
/// yaw comes from the per-tick `ArenaInput`.
pub fn apply_arena_yaw(input: &ArenaInput, rotation: &mut Rotation) {
    rotation.0 = Quat::from_axis_angle(Vec3::Y, input.yaw);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The desired-velocity math: full-forward input (movement.y = 1, yaw = 0) yields a world
    /// desired velocity of `-Z * MAX_SPEED`; full-strafe (movement.x = 1) yields `+X * MAX_SPEED`.
    #[test]
    fn desired_velocity_frame() {
        let yaw = 0.0;
        let forward = Quat::from_axis_angle(Vec3::Y, yaw) * Vec3::new(0.0, 0.0, -1.0) * MAX_SPEED;
        assert!((forward - Vec3::new(0.0, 0.0, -MAX_SPEED)).length() < 1e-5);
        let strafe = Quat::from_axis_angle(Vec3::Y, yaw) * Vec3::new(1.0, 0.0, 0.0) * MAX_SPEED;
        assert!((strafe - Vec3::new(MAX_SPEED, 0.0, 0.0)).length() < 1e-5);
    }
}
