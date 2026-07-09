//! Portal traversal, CLIENT side — the PREDICTED half of the shared pipeline
//! (`crate::portals_shared` holds the math; `server/portals.rs` is the authority). Runs the
//! SAME floor pass-through + teleport over the client's Predicted bodies (both players — this
//! client simulates the opponent from rebroadcast inputs too) so a portal crossing needs no
//! server round-trip: positions match the server's own computation and rollback stays quiet.
//! Mispredicts (a crossing the server resolves differently) surface as an ordinary rollback —
//! teleport-scale corrections SNAP via `snap_large_corrections`.
//!
//! The LOCAL player's teleport additionally rotates the camera ([`controller::CameraYaw`])
//! through the pair — the next input tick carries the new heading, so the server body turns
//! with the camera (wisp ships a TeleportSnap message for this; predicting it locally is
//! zero-latency and needs no wire change).
//!
//! Both FixedUpdate systems are ROLLBACK-GUARDED (the `PortalTravelers`-style prev/lockout maps
//! are not rollback-managed state — replaying them double-teleports; skipping replay means a
//! rollback across a crossing resolves to the server's result, which is the safe direction).

use avian3d::prelude::{CollisionLayers, LayerMask, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::{Predicted, Rollback};
use serde_json::json;
use std::collections::HashMap;

use crate::net::input::ArenaInput;
use crate::net::protocol::{NetworkedPlayer, NetworkedSkillObject};
use crate::portals_shared::{
    collect_pairs, in_floor_slab, pitch_from_forward, player_crossing, portal_virtual_transform,
    velocity_rotation, yaw_from_forward, PortalPose, EXIT_STAND_OFFSET, LOCKOUT_RADIUS,
};
use crate::trace;

use super::controller::{AimPitch, CameraYaw};
use super::net::LocalNetPlayer;

/// After a predicted teleport snaps the camera, further snaps are suppressed this long — a
/// mispredicted crossing (server resolves it a tick later) can rollback the BODY to the entry
/// side and legitimately re-cross, but rotating the VIEW twice reads as a wild spin. Positions
/// and velocity still teleport; only the camera snap is debounced.
const CAMERA_SNAP_DEBOUNCE_SECS: f32 = 0.3;

/// Client-side per-predicted-body teleport state (prev position + exit lockout). NOT
/// rollback-managed — both systems skip rollback replay entirely.
#[derive(Resource, Default)]
pub struct PredictedPortalTravelers {
    prev: HashMap<Entity, Vec3>,
    lockout: HashMap<Entity, Vec3>,
    /// `time.elapsed_secs()` of the last local-camera snap (the debounce clock).
    last_camera_snap: f32,
}

/// The replicated portal discs, as complete per-owner pairs (same collector as the server).
fn live_pairs(
    portals: &Query<(&NetworkedSkillObject, &Position, &Rotation), Without<Predicted>>,
) -> Vec<(PortalPose, PortalPose)> {
    collect_pairs(portals.iter().filter_map(|(o, p, r)| {
        let kind: &'static str = match o.kind.as_str() {
            k if k == crate::portals_shared::KIND_PORTAL_ORANGE => {
                crate::portals_shared::KIND_PORTAL_ORANGE
            }
            k if k == crate::portals_shared::KIND_PORTAL_BLUE => {
                crate::portals_shared::KIND_PORTAL_BLUE
            }
            _ => return None,
        };
        Some((o.owner, kind, p.0, r.0))
    }))
}

/// Predicted mirror of the server's floor pass-through: drop `Ground` from a predicted body
/// standing in an un-locked floor-portal slab. Must compute the SAME answer as the server or
/// the predicted body collides where the authoritative one falls (rollback fight while standing
/// on a portal).
#[allow(clippy::type_complexity)]
pub fn predicted_floor_passthrough(
    rollback: Query<(), With<Rollback>>,
    portals: Query<(&NetworkedSkillObject, &Position, &Rotation), Without<Predicted>>,
    travelers: Res<PredictedPortalTravelers>,
    mut players: Query<
        (Entity, &Position, &mut CollisionLayers),
        (With<NetworkedPlayer>, With<Predicted>),
    >,
) {
    if !rollback.is_empty() {
        return;
    }
    let pairs = live_pairs(&portals);
    for (e, pos, mut layers) in &mut players {
        let locked_near = travelers.lockout.get(&e).copied();
        let in_slab = pairs.iter().any(|(a, b)| {
            [a, b].into_iter().any(|disc| {
                if let Some(exit_pos) = locked_near {
                    if (disc.position - exit_pos).length() < 0.01 {
                        return false;
                    }
                }
                in_floor_slab(pos.0, disc)
            })
        });
        let target = if in_slab {
            CollisionLayers::new(
                arena_sim::GameLayer::Player,
                LayerMask::ALL & !LayerMask::from(arena_sim::GameLayer::Ground),
            )
        } else {
            CollisionLayers::new(arena_sim::GameLayer::Player, LayerMask::ALL)
        };
        if *layers != target {
            *layers = target;
        }
    }
}

/// The predicted teleport for BOTH predicted players. Same crossing rules + virtual transform
/// as the server; the LOCAL player additionally snaps the camera yaw through the pair.
#[allow(clippy::type_complexity)]
pub fn predicted_portal_teleport(
    rollback: Query<(), With<Rollback>>,
    portals: Query<(&NetworkedSkillObject, &Position, &Rotation), Without<Predicted>>,
    mut players: Query<
        (
            Entity,
            &mut Position,
            &mut LinearVelocity,
            &ActionState<ArenaInput>,
            Has<LocalNetPlayer>,
        ),
        (With<NetworkedPlayer>, With<Predicted>),
    >,
    mut cam_yaw: Option<ResMut<CameraYaw>>,
    mut aim_pitch: Option<ResMut<AimPitch>>,
    time: Res<Time>,
    mut travelers: ResMut<PredictedPortalTravelers>,
) {
    if !rollback.is_empty() {
        return;
    }
    let pairs = live_pairs(&portals);
    for (e, mut pos, mut vel, action, is_local) in &mut players {
        let prev = travelers.prev.insert(e, pos.0).unwrap_or(pos.0);
        if let Some(exit_pos) = travelers.lockout.get(&e).copied() {
            if (pos.0 - exit_pos).length() > LOCKOUT_RADIUS {
                travelers.lockout.remove(&e);
            } else {
                continue;
            }
        }
        let mut done = false;
        for (orange, blue) in &pairs {
            if done {
                break;
            }
            for (entry, exit) in [(orange, blue), (blue, orange)] {
                let Some(basis) = player_crossing(prev, pos.0, entry) else {
                    continue;
                };
                let view_rot = Quat::from_axis_angle(Vec3::Y, action.0.yaw)
                    * Quat::from_axis_angle(Vec3::X, action.0.pitch);
                let basis_tf = Transform::from_translation(basis).with_rotation(view_rot);
                let virt = portal_virtual_transform(basis_tf, entry, exit);
                let mut out_pos = virt.translation;
                if exit.is_horizontal() && !entry.is_horizontal() {
                    out_pos.y = exit.position.y + EXIT_STAND_OFFSET * exit.normal.y.signum();
                }
                // Full pair mapping (Portal's seamlessness invariant: the teleport IS the
                // through-view transform — walking in continues exactly what the disc showed).
                pos.0 = out_pos;
                vel.0 = velocity_rotation(entry, exit) * vel.0;
                travelers.lockout.insert(e, exit.position);
                travelers.prev.insert(e, out_pos);
                // Camera continuity (local player only): snap the FULL view — yaw AND pitch —
                // to the mapped forward; the next buffered input ships the new yaw, turning
                // the server body to match. Debounced: a mispredicted crossing can re-fire
                // after a rollback, and a second view snap reads as a wild spin.
                if is_local {
                    let now = time.elapsed_secs();
                    if now - travelers.last_camera_snap >= CAMERA_SNAP_DEBOUNCE_SECS {
                        let fwd = virt.rotation * Vec3::NEG_Z;
                        if let (Some(cam), Some(new_yaw)) =
                            (cam_yaw.as_deref_mut(), yaw_from_forward(fwd))
                        {
                            cam.0 = new_yaw;
                        }
                        if let Some(pitch) = aim_pitch.as_deref_mut() {
                            pitch.0 = pitch_from_forward(fwd);
                        }
                        travelers.last_camera_snap = now;
                    }
                }
                trace::event(
                    "predicted_portal_teleport",
                    json!({ "local": is_local,
                            "to": [out_pos.x, out_pos.y, out_pos.z] }),
                );
                done = true;
                break;
            }
        }
    }

    if travelers.prev.len() > 512 {
        travelers.prev.clear();
        travelers.lockout.clear();
    }
}
