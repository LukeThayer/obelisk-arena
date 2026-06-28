//! Stage-A own-movement prediction for the LOCAL player (M2.2 Task 12, guide §6.3 + spec §6.1).
//!
//! ## What landed: the snap-to-server fallback (predict-locally-snap-to-server)
//!
//! The plan's Stage-A target was lightyear-0.26-native rollback prediction. After verifying the
//! 0.26 API against the installed registry source (`lightyear_prediction-0.26.4`,
//! `lightyear_replication-0.26.4`), real rollback in 0.26 requires the **Confirmed/Predicted entity
//! split**: the server adds `PredictionTarget::to_clients(...)`, the client then spawns a SEPARATE
//! `Predicted` entity (distinct from the `Confirmed` one holding the replicated state), per-component
//! `ComponentReplicationOverrides::enable_all()` overrides opt Position/Rotation/LinearVelocity into
//! replication, and lightyear re-simulates the client controller on the `Predicted` copy each
//! rollback. That path is:
//!   - **unproven in this lineage** — wisp *registered* `.add_prediction()`/`.add_should_rollback()`
//!     but `disable: true` and NEVER enabled it ("Stage Q reconciliation deferred", wisp CLAUDE.md);
//!     M2 would be the first time it's exercised;
//!   - **architecturally mismatched** with the arena server's KINEMATIC direct-`Position` controller
//!     (M2.2 Task 10) — lightyear's avian rollback is built around the Dynamic-body +
//!     frame-interpolation `avian_3d_character` flow, which `add_avian_with_lightyear` deliberately
//!     does NOT run (no `FrameInterpolationPlugin`);
//!   - a **major restructuring** — the obelisk combatant + rig + hurtbox would have to move onto the
//!     `Predicted` entity.
//!
//! The plan (spec §6.1) explicitly authorizes the fallback and says: "do NOT get stuck chasing
//! perfect rollback; the snap fallback is an acceptable, documented landing." So Stage-A landed as
//! **predict-locally-snap-to-server** (wisp's documented `sync_local_player_from_server`,
//! controller.rs:92-123). The protocol's `Position`/`Rotation`/`LinearVelocity` prediction
//! registration stays `disable: true` (registered-but-dormant forward-prep for a future native
//! rollback pass); nothing enables the per-entity override here.
//!
//! ## How it works
//!
//! For the LOCAL player (`LocalNetPlayer`):
//!   1. **Predict** (`predict_local_movement`): run the SAME kinematic controller as the server
//!      against the local input, integrating the local body's avian `Position` immediately — so
//!      WASD feels instant (zero perceptible latency), no waiting on the round-trip.
//!   2. **Reconcile** (`snap_local_to_server`): each frame, snap the local body toward the server's
//!      authoritative replicated `NetworkedPosition`. The server is the sole authority for what
//!      every other peer sees; this keeps the local view honest. At LAN/low latency the residual
//!      between the locally-predicted pose and the server pose is small (both run the same
//!      deterministic controller off the same input), so the snap is imperceptible; a large
//!      divergence (lag spike, anti-cheat correction) snaps hard, which is correct.
//!
//! The deliverable — responsive local movement + server-authoritative truth — is met either way.

use bevy::prelude::*;

use crate::client::net::{LocalInput, LocalNetPlayer};
use crate::net::protocol::{NetworkedPlayer, NetworkedPosition};

/// How far (metres) the locally-predicted position may drift from the server's authoritative pose
/// before we hard-snap. Below this we keep the smooth local prediction (snapping every frame to a
/// pose that's ~one round-trip behind would itself induce jitter); at or above it we snap so a real
/// divergence (lag spike, server correction) can't persist. `0.5 m` ≈ an eighth of a second of run
/// at `MAX_SPEED 4.0` — small enough that a cheater/desync can't drift the local view meaningfully,
/// large enough that normal round-trip residual doesn't cause visible snapping.
const SNAP_THRESHOLD: f32 = 0.5;

/// Per-frame fraction the local body is pulled toward the server pose while WITHIN the snap
/// threshold. A gentle correction (not a hard set) so small round-trip residual is absorbed
/// smoothly rather than fought frame-to-frame. `0.0` = pure local prediction (drifts), `1.0` = hard
/// snap (jittery at latency); `0.1` reconciles over ~10 frames.
const RECONCILE_LERP: f32 = 0.1;

/// Plugin: Stage-A local-player prediction (predict-locally-snap-to-server). Registers the predict
/// + reconcile systems, chained so prediction integrates first and reconciliation corrects after.
pub struct LocalPredictionPlugin;

impl Plugin for LocalPredictionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (predict_local_movement, snap_local_to_server).chain(),
        );
    }
}

/// **Predict**: run the SAME kinematic controller as the server (server/mod.rs::run_player_controller)
/// against `LocalInput`, integrating the local body's avian `Position` + `Rotation` immediately for
/// zero-latency feedback. Identical constants + movement frame to the server so the predicted pose
/// matches what the server computes off the same input (minimising the reconcile residual).
#[allow(clippy::type_complexity)]
fn predict_local_movement(
    time: Res<Time>,
    input: Res<LocalInput>,
    mut local: Query<
        (
            &mut avian3d::prelude::Position,
            &mut avian3d::prelude::Rotation,
            &mut avian3d::prelude::LinearVelocity,
        ),
        (With<NetworkedPlayer>, With<LocalNetPlayer>),
    >,
) {
    // Keep in lockstep with the server controller (server/mod.rs).
    const MAX_SPEED: f32 = 4.0;
    const ACCEL_LERP: f32 = 0.35;
    let dt = time.delta_secs().max(1e-5);

    let Ok((mut position, mut rotation, mut lin_vel)) = local.single_mut() else {
        return;
    };

    // Body yaw from input (matches server apply_player_rotation).
    rotation.0 = Quat::from_axis_angle(Vec3::Y, input.yaw);

    // Camera-relative WASD → world dir (matches server run_player_controller).
    let local_dir = Vec3::new(input.movement.x, 0.0, -input.movement.y);
    let world_dir = Quat::from_axis_angle(Vec3::Y, input.yaw) * local_dir;
    let desired = world_dir.normalize_or_zero() * MAX_SPEED;

    let cur = Vec3::new(lin_vel.0.x, 0.0, lin_vel.0.z);
    let new_planar = cur.lerp(desired, ACCEL_LERP);
    lin_vel.0.x = new_planar.x;
    lin_vel.0.z = new_planar.z;

    position.0.x += new_planar.x * dt;
    position.0.z += new_planar.z * dt;

    // Vertical physics: the SAME manual gravity + jump + ground clamp the server runs
    // (server::run_player_controller), using the SAME shared constants + `LocalInput.jump`, so the
    // predicted local player jumps/falls identically to the server and the reconcile (snap_local_to
    // _server) has no residual Y to fight.
    let grounded = position.0.y <= crate::net::GROUND_Y + 0.001;
    if input.jump && grounded {
        lin_vel.0.y = crate::net::JUMP_SPEED;
    }
    lin_vel.0.y -= crate::net::GRAVITY * dt;
    position.0.y += lin_vel.0.y * dt;
    if position.0.y <= crate::net::GROUND_Y {
        position.0.y = crate::net::GROUND_Y;
        lin_vel.0.y = 0.0;
    }
}

/// **Reconcile**: snap the local body toward the server's authoritative `NetworkedPosition`. Within
/// `SNAP_THRESHOLD` we gently lerp (absorbs round-trip residual without per-frame jitter); at/above
/// it we hard-snap (a real divergence can't persist — server-authoritative). Matches the spirit of
/// wisp's `sync_local_player_from_server` (controller.rs:92-123), extended with the soft-lerp band.
///
/// The LOCAL player entity carries BOTH the replicated server pose (`NetworkedPosition`, written by
/// replication) AND our predicted avian `Position` (integrated by `predict_local_movement`), so a
/// single query over the local entity has everything — no cross-entity id matching needed. The
/// `LocalId == NetworkOwner` guard from materialization already established this is our player.
#[allow(clippy::type_complexity)]
fn snap_local_to_server(
    mut local: Query<
        (
            &NetworkedPosition,
            &mut avian3d::prelude::Position,
            &mut Transform,
        ),
        (With<NetworkedPlayer>, With<LocalNetPlayer>),
    >,
) {
    let Ok((server_pose, mut position, mut transform)) = local.single_mut() else {
        return;
    };
    let server_pos = server_pose.to_vec3();
    let predicted = position.0;
    let drift = (server_pos - predicted).length();

    let corrected = if drift >= SNAP_THRESHOLD {
        // Hard-snap: a real divergence (lag spike / server correction) — the server is authoritative.
        server_pos
    } else {
        // Soft reconcile: pull gently toward the server pose so round-trip residual is absorbed
        // without per-frame jitter.
        predicted.lerp(server_pos, RECONCILE_LERP)
    };

    position.0 = corrected;
    // Keep Transform in step (under LightyearAvianPlugin we own both; guide §1.2 writes Position,
    // and we mirror Transform so any same-frame Transform reader sees the corrected pose too).
    transform.translation = corrected;

    // [H] divergence trace (env-gated): the residual between the locally-PREDICTED pose and the
    // server's authoritative NetworkedPosition. The Task 12 check asserts this stays bounded (no
    // persistent drift) — the snap-to-server reconcile keeps it small.
    if std::env::var("ARENA_DEBUG_PREDICT").ok().as_deref() == Some("1") {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n.is_multiple_of(30) {
            crate::trace::event(
                "predict_divergence",
                serde_json::json!({
                    "drift": drift,
                    "predicted": [predicted.x, predicted.y, predicted.z],
                    "server": [server_pos.x, server_pos.y, server_pos.z],
                }),
            );
        }
    }
}
