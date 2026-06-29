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
//! query). Ground state is read from the body's world Y against the flat arena floor (no raycast —
//! the arena platform is flat + hard-coded, and a position threshold sidesteps the
//! ray-hits-own-hurtbox + `LightyearAvianPlugin` spatial-refresh-ordering footguns).

use avian3d::prelude::forces::ForcesItem;
use avian3d::prelude::*;
use bevy::prelude::*;

use crate::net::input::ArenaInput;

/// Top planar movement speed (m/s). Matches the old kinematic `MOVE_SPEED` so the feel target
/// (≈4 m/s) is preserved after the kinematic→Dynamic switch.
pub const MAX_SPEED: f32 = 4.0;
/// Max planar acceleration (m/s²): how fast the body ramps to/from `MAX_SPEED`. Higher = snappier.
pub const MAX_ACCELERATION: f32 = 30.0;
/// Upward velocity (m/s) imparted on a grounded jump. With the arena `Gravity` (−20 m/s²) this gives
/// an apex of `JUMP_SPEED² / (2·g)` ≈ 1.22 m (the spec's 1.0–1.2 m target).
pub const JUMP_SPEED: f32 = 7.0;

/// Apply the planar movement force + jump impulse for one character from its `ArenaInput`. World Y
/// of the body is used for the ground check (flat arena floor at world 0; body rests at
/// [`crate::net::GROUND_Y`]).
pub fn apply_arena_movement(
    mass: &ComputedMass,
    dt: f32,
    input: &ArenaInput,
    mut forces: ForcesItem,
) {
    let dt = dt.max(1e-5);
    let max_velocity_delta_per_tick = MAX_ACCELERATION * dt;

    // Camera-relative WASD → world direction (matches the client camera frame: forward = -Z,
    // strafe = +X, both in the yaw frame). Clamp the input magnitude so a diagonal isn't faster.
    let mut mv = input.movement;
    if mv.length_squared() > 1.0 {
        mv = mv.normalize();
    }
    let local = Vec3::new(mv.x, 0.0, -mv.y);
    let world_dir = Quat::from_axis_angle(Vec3::Y, input.yaw) * local;
    let desired_ground_velocity = world_dir * MAX_SPEED;

    let linear_velocity = forces.linear_velocity();
    let ground_velocity = Vec3::new(linear_velocity.x, 0.0, linear_velocity.z);
    let new_ground_velocity =
        ground_velocity.move_towards(desired_ground_velocity, max_velocity_delta_per_tick);
    let required_acceleration = (new_ground_velocity - ground_velocity) / dt;
    forces.apply_force(required_acceleration * mass.value());

    // Jump: on any grounded tick where jump is held, bring vertical velocity up to JUMP_SPEED.
    // Reading position drops the immutable borrow before the mutable impulse call.
    let pos_y = forces.position().0.y;
    let grounded = pos_y <= crate::net::GROUND_Y + 0.05;
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
