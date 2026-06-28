//! Client-side interpolation/smoothing of replicated players (M2.2 Task 11, guide §6.2).
//!
//! The server ships a per-tick `NetworkedPosition` stream. Applying it raw to Transform stutters
//! (server ticks at 60 Hz, the client renders faster and the packets arrive jittered). This module
//! ports wisp's `net/replication.rs:82-217` render-interpolation: keep the two most-recent samples
//! and render at `now - span` (one sample behind) so the render time is always bracketed by two
//! real samples — artifact-free, no extrapolation overshoot, at the cost of one tick of latency.
//!
//! Divergence from wisp: it writes BOTH Transform AND avian `Position`/`Rotation` (the arena's
//! materialized remote bodies carry a `RigidBody`, so under `LightyearAvianPlugin` we must keep
//! avian's components in step with Transform or the per-tick sync fights us — guide §1.2). The
//! teleport-snap (>3m/tick) and the `t.clamp(0,1)` late-packet guard are carried verbatim.
//!
//! The LOCAL player (`LocalNetPlayer`) is EXCLUDED here: M2.2 Task 12 owns its pose (predicted +
//! snapped). Remote players are interpolated; the local player is not (it would feel laggy).

use bevy::prelude::*;

use crate::client::net::LocalNetPlayer;
use crate::net::protocol::{NetworkedPlayer, NetworkedPosition};

/// Plugin: registers the smoothing buffer + the capture/render systems for remote players.
pub struct ReplicationSmoothingPlugin;

impl Plugin for ReplicationSmoothingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // capture (writes the buffer) must run before render (reads it). Both in Update.
                attach_smoothing_buffers,
                capture_networked_position_samples,
                smooth_networked_transforms,
            )
                .chain(),
        );
    }
}

/// Two-sample render-interpolation buffer for a remote `NetworkedPlayer`. We render at `now - span`
/// (one sample behind the latest) so we always interpolate BETWEEN two real samples instead of
/// extrapolating past the newest one. Copied from `wisp/src/net/replication.rs:82-96`.
#[derive(Component, Default, Debug)]
pub struct NetworkedPositionSmoothing {
    /// Previous (older) sample. Until a second sample arrives, equals cur.
    prev_pos: Vec3,
    prev_yaw: f32,
    prev_time: f32,
    /// Latest received sample.
    cur_pos: Vec3,
    cur_yaw: f32,
    cur_time: f32,
    /// False until the first sample is captured.
    initialized: bool,
}

impl NetworkedPositionSmoothing {
    /// World-space velocity estimate from the two most-recent pose samples
    /// (`(cur_pos - prev_pos) / max(cur_time - prev_time, 1e-3)`). Drives the remote rig's
    /// locomotion blend (Bug 2) so a moving opponent plays the correct directional walk clip
    /// instead of sliding while idle. `Vec3::ZERO` until the buffer has captured a sample.
    pub fn velocity(&self) -> Vec3 {
        if !self.initialized {
            return Vec3::ZERO;
        }
        let dt = (self.cur_time - self.prev_time).max(1e-3);
        (self.cur_pos - self.prev_pos) / dt
    }
}

/// Attach a [`NetworkedPositionSmoothing`] buffer to every remote `NetworkedPlayer` that lacks one.
/// The LOCAL player is excluded (Task 12 owns its pose). Idempotent (polls for new replicas).
#[allow(clippy::type_complexity)]
fn attach_smoothing_buffers(
    new_remotes: Query<
        Entity,
        (
            With<NetworkedPlayer>,
            Without<NetworkedPositionSmoothing>,
            Without<LocalNetPlayer>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in &new_remotes {
        commands
            .entity(entity)
            .insert(NetworkedPositionSmoothing::default());
    }
}

/// Shift `cur → prev` and stamp the new sample whenever `NetworkedPosition` changes. A jump > 3m in
/// a single tick is a teleport (well beyond normal per-tick motion at 60 Hz) → snap both prev and
/// cur so the lerp pops instantly instead of sliding across the arena. Copied from
/// `wisp/src/net/replication.rs:101-146`.
fn capture_networked_position_samples(
    time: Res<Time>,
    mut q: Query<(&NetworkedPosition, &mut NetworkedPositionSmoothing), Changed<NetworkedPosition>>,
) {
    let now = time.elapsed_secs();
    for (np, mut s) in &mut q {
        let pos = np.to_vec3();
        if !s.initialized {
            s.prev_pos = pos;
            s.prev_yaw = np.yaw;
            s.prev_time = now;
            s.cur_pos = pos;
            s.cur_yaw = np.yaw;
            s.cur_time = now;
            s.initialized = true;
        } else {
            let teleport = pos.distance_squared(s.cur_pos) > 9.0;
            if teleport {
                s.prev_pos = pos;
                s.prev_yaw = np.yaw;
                s.prev_time = now;
            } else {
                s.prev_pos = s.cur_pos;
                s.prev_yaw = s.cur_yaw;
                s.prev_time = s.cur_time;
            }
            s.cur_pos = pos;
            s.cur_yaw = np.yaw;
            s.cur_time = now;
        }
    }
}

/// Per-frame render interpolation: lerp prev → cur at `t = (now - span - prev_time) / span`, clamped
/// to `[0, 1]` (late packets can push it past 1 — guide §1.2 / risk #4). Writes BOTH Transform and
/// avian `Position`/`Rotation` so the materialized remote body's physics components don't fight the
/// smoothed Transform under `LightyearAvianPlugin`. Adapted from `wisp/src/net/replication.rs:163-217`
/// (the arena has no frost-spike branch; players rotate around Y so yaw-wrap is the only special case).
#[allow(clippy::type_complexity)]
fn smooth_networked_transforms(
    time: Res<Time>,
    mut q: Query<(
        &NetworkedPositionSmoothing,
        &mut Transform,
        Option<&mut avian3d::prelude::Position>,
        Option<&mut avian3d::prelude::Rotation>,
    )>,
) {
    use core::f32::consts::PI;
    let now = time.elapsed_secs();
    for (s, mut tf, pos_opt, rot_opt) in &mut q {
        if !s.initialized {
            continue;
        }
        let span = (s.cur_time - s.prev_time).max(1e-3);
        let render_time = now - span;
        let t = ((render_time - s.prev_time) / span).clamp(0.0, 1.0);
        let pos = s.prev_pos.lerp(s.cur_pos, t);
        let mut dyaw = s.cur_yaw - s.prev_yaw;
        if dyaw > PI {
            dyaw -= 2.0 * PI;
        } else if dyaw < -PI {
            dyaw += 2.0 * PI;
        }
        let yaw = s.prev_yaw + dyaw * t;
        let rot = Quat::from_axis_angle(Vec3::Y, yaw);
        tf.translation = pos;
        tf.rotation = rot;
        if let Some(mut p) = pos_opt {
            p.0 = pos;
        }
        if let Some(mut r) = rot_opt {
            *r = avian3d::prelude::Rotation::from(rot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lerp `t` MUST clamp to `[0, 1]` even when a late packet pushes `render_time` past
    /// `cur_time` (guide §1.2 / risk #4: "late-arriving position packets make the interp t exceed 1
    /// → clamp"). This pins the clamp logic the render system relies on without booting an app.
    #[test]
    fn interp_t_clamps_to_unit_interval() {
        let prev_time = 0.0_f32;
        let cur_time = 0.1_f32;
        let span = (cur_time - prev_time).max(1e-3);

        // A render_time well past cur_time (a stale `cur` sample, no new packet) → raw t > 1.
        let now_late = 1.0_f32; // render_time = 0.9, raw t = (0.9 - 0.0)/0.1 = 9.0
        let render_time = now_late - span;
        let raw_t = (render_time - prev_time) / span;
        assert!(raw_t > 1.0, "precondition: a late packet yields raw t > 1");
        let t = raw_t.clamp(0.0, 1.0);
        assert_eq!(t, 1.0, "t must clamp to 1.0 — no extrapolation past cur");

        // A render_time before prev_time → raw t < 0, must clamp to 0.
        let now_early = 0.05_f32; // render_time = -0.05, raw t = -0.5
        let render_time_early = now_early - span;
        let raw_t_early = (render_time_early - prev_time) / span;
        assert!(
            raw_t_early < 0.0,
            "precondition: an early render yields raw t < 0"
        );
        assert_eq!(raw_t_early.clamp(0.0, 1.0), 0.0, "t must clamp to 0.0");
    }

    /// At the clamped endpoints the lerp returns exactly prev (t=0) / cur (t=1) — no overshoot.
    #[test]
    fn lerp_endpoints_are_exact() {
        let prev = Vec3::new(1.0, 0.0, 0.0);
        let cur = Vec3::new(5.0, 0.0, 0.0);
        assert_eq!(prev.lerp(cur, 0.0_f32.clamp(0.0, 1.0)), prev);
        assert_eq!(prev.lerp(cur, 1.0_f32.clamp(0.0, 1.0)), cur);
        // A would-be-extrapolating t clamps so the result never passes cur.
        assert_eq!(prev.lerp(cur, 9.0_f32.clamp(0.0, 1.0)), cur);
    }
}
