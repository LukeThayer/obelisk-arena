//! Portal TELEPORT (server-authoritative) — wisp's `server_portal_teleport` crossing math
//! ported onto skill objects. Teleporting is the canonical "verb obelisk can't express" (it
//! never moves a body); this system watches every traveler's per-tick movement segment against
//! the two portal discs and rewrites avian `Position`/`LinearVelocity` (players) or the obelisk
//! projectile `Transform`/heading (skill hitboxes — yes, a firebolt flies through).
//!
//! wisp semantics kept: BOTH portals required; crossing = sign flip of the segment's
//! along-normal component with the crossing point within the disc radius; velocity remapped
//! through the portal-pair rotation (anti-parallel pairs get the mirror-cancelling flip); a
//! re-entry lockout until the traveler leaves the exit disc's neighborhood. v1 simplification
//! (documented): the caster's facing is NOT snapped through the pair — the camera keeps its
//! heading (wisp rewrites input yaw via a TeleportSnap message).

use avian3d::prelude::{LinearVelocity, Position};
use bevy::prelude::*;
use obelisk_bevy::prelude::Hitbox;
use serde_json::json;
use std::collections::HashMap;

use crate::net::protocol::NetworkedPlayer;
use crate::trace;

use super::skill_objects::{SkillObject, KIND_PORTAL_BLUE, KIND_PORTAL_ORANGE};
use super::verbs::PORTAL_RADIUS;

/// Re-entry lockout radius (wisp `LOCKOUT_RADIUS` = PORTAL_RADIUS × 1.5): after a teleport the
/// traveler must move this far from the EXIT portal before it can cross again.
const LOCKOUT_RADIUS: f32 = PORTAL_RADIUS * 1.5;

/// Per-traveler teleport state: the previous tick's position (crossing segments) and, after a
/// teleport, the exit-portal position the traveler must clear (the lockout).
#[derive(Resource, Default)]
pub(crate) struct PortalTravelers {
    prev: HashMap<Entity, Vec3>,
    lockout: HashMap<Entity, Vec3>,
}

/// One portal's pose for the crossing test.
#[derive(Clone, Copy)]
struct PortalPose {
    position: Vec3,
    normal: Vec3,
    rotation: Quat,
}

/// Does the segment `prev -> cur` cross the portal disc? Returns the crossing point.
/// wisp's test: the along-normal component sign-flips AND the crossing point lies within the
/// disc radius of the center.
fn segment_crosses_disc(prev: Vec3, cur: Vec3, portal: &PortalPose) -> Option<Vec3> {
    let a0 = (prev - portal.position).dot(portal.normal);
    let a1 = (cur - portal.position).dot(portal.normal);
    if a0 == 0.0 && a1 == 0.0 {
        return None; // sliding in the plane, not crossing
    }
    if (a0 > 0.0) == (a1 > 0.0) {
        return None; // no sign flip
    }
    let t = a0 / (a0 - a1);
    let cross = prev.lerp(cur, t.clamp(0.0, 1.0));
    ((cross - portal.position).length() <= PORTAL_RADIUS).then_some(cross)
}

/// The portal-pair mapping for positions/velocities (wisp's remap): rotate through
/// `exit.rotation × flip × entry.rotation⁻¹`, where anti-parallel pairs (normals dot < -0.5 —
/// facing walls) flip about the entry disc's local Z to cancel the mirror, and every other pair
/// flips about local X (so walking "into" the entry comes "out of" the exit).
fn pair_rotation(entry: &PortalPose, exit: &PortalPose) -> Quat {
    let flip = if entry.normal.dot(exit.normal) < -0.5 {
        Quat::from_axis_angle(Vec3::Z, std::f32::consts::PI)
    } else {
        Quat::from_axis_angle(Vec3::X, std::f32::consts::PI)
    };
    exit.rotation * flip * entry.rotation.inverse()
}

/// Map a world point near the entry through the pair onto the exit's frame.
fn map_point(point: Vec3, entry: &PortalPose, exit: &PortalPose) -> Vec3 {
    exit.position + pair_rotation(entry, exit) * (point - entry.position)
}

/// The teleport system: players (avian bodies) + obelisk projectile hitboxes cross when both
/// portals exist. Runs in FixedUpdate after physics/projectile motion.
#[allow(clippy::type_complexity)]
pub(crate) fn portal_teleport(
    portals: Query<(&SkillObject, &Position, &avian3d::prelude::Rotation)>,
    mut players: Query<
        (Entity, &mut Position, &mut LinearVelocity),
        (With<NetworkedPlayer>, Without<SkillObject>),
    >,
    mut projectiles: Query<
        (Entity, &mut Transform, &mut Hitbox),
        (
            With<obelisk_bevy::spatial::projectile::Projectile>,
            Without<SkillObject>,
        ),
    >,
    mut travelers: ResMut<PortalTravelers>,
    mut commands: Commands,
) {
    // Resolve the pair (both required — wisp).
    let find = |kind: &str| {
        portals
            .iter()
            .find(|(o, ..)| o.kind == kind)
            .map(|(_, p, r)| PortalPose {
                position: p.0,
                normal: r.0 * Vec3::Y,
                rotation: r.0,
            })
    };
    let pair = match (find(KIND_PORTAL_ORANGE), find(KIND_PORTAL_BLUE)) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    };

    // (Lockouts clear per-traveler inside the loops below, once the body has moved
    // LOCKOUT_RADIUS away from its exit portal.)
    let mut teleport_events: Vec<(Entity, &'static str)> = Vec::new();

    // --- Players ---
    for (e, mut pos, mut vel) in &mut players {
        let prev = travelers.prev.insert(e, pos.0).unwrap_or(pos.0);
        if let Some(exit_pos) = travelers.lockout.get(&e).copied() {
            if (pos.0 - exit_pos).length() > LOCKOUT_RADIUS {
                travelers.lockout.remove(&e);
            } else {
                continue;
            }
        }
        let Some((orange, blue)) = pair else { continue };
        for (entry, exit) in [(&orange, &blue), (&blue, &orange)] {
            let Some(cross) = segment_crosses_disc(prev, pos.0, entry) else {
                continue;
            };
            let out_pos = map_point(cross, entry, exit) + exit.normal * 0.6;
            let out_vel = pair_rotation(entry, exit) * vel.0;
            pos.0 = out_pos;
            vel.0 = out_vel;
            travelers.lockout.insert(e, exit.position);
            travelers.prev.insert(e, out_pos);
            teleport_events.push((e, "player"));
            break;
        }
    }

    // --- Obelisk projectiles (Transform-flown hitboxes) ---
    for (e, mut tf, mut hitbox) in &mut projectiles {
        let prev = travelers.prev.insert(e, tf.translation).unwrap_or(tf.translation);
        if let Some(exit_pos) = travelers.lockout.get(&e).copied() {
            if (tf.translation - exit_pos).length() > LOCKOUT_RADIUS {
                travelers.lockout.remove(&e);
            } else {
                continue;
            }
        }
        let Some((orange, blue)) = pair else { continue };
        for (entry, exit) in [(&orange, &blue), (&blue, &orange)] {
            let Some(cross) = segment_crosses_disc(prev, tf.translation, entry) else {
                continue;
            };
            let rot = pair_rotation(entry, exit);
            let out_pos = map_point(cross, entry, exit) + exit.normal * 0.3;
            tf.translation = out_pos;
            // Remap the heading; obelisk's move_projectiles integrates along the Transform's
            // stored velocity direction on the Projectile component — rotate the hitbox aim and
            // the transform rotation so the flight continues out of the exit.
            hitbox.aim = (rot * hitbox.aim).normalize_or_zero();
            tf.rotation = rot * tf.rotation;
            travelers.lockout.insert(e, exit.position);
            travelers.prev.insert(e, out_pos);
            teleport_events.push((e, "projectile"));
            break;
        }
    }

    // Forget travelers that no longer exist (cheap periodic hygiene).
    if travelers.prev.len() > 512 {
        travelers.prev.clear();
    }

    for (e, kind_of) in teleport_events {
        let _ = commands.get_entity(e); // touch to assert liveness in traces only
        trace::event("portal_teleport", json!({ "traveler": kind_of }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(position: Vec3, normal: Vec3) -> PortalPose {
        let rotation = Quat::from_rotation_arc(Vec3::Y, normal);
        PortalPose {
            position,
            normal,
            rotation,
        }
    }

    /// A segment passing through the disc center crosses; one outside the radius doesn't; one
    /// that stays on one side doesn't (wisp's sign-flip + radial test).
    #[test]
    fn crossing_detection_matches_wisp() {
        let portal = pose(Vec3::ZERO, Vec3::Z);
        assert!(
            segment_crosses_disc(Vec3::new(0.0, 0.0, -0.5), Vec3::new(0.0, 0.0, 0.5), &portal)
                .is_some()
        );
        // Off-disc (radial > 1.4).
        assert!(
            segment_crosses_disc(Vec3::new(2.0, 0.0, -0.5), Vec3::new(2.0, 0.0, 0.5), &portal)
                .is_none()
        );
        // Same side.
        assert!(
            segment_crosses_disc(Vec3::new(0.0, 0.0, 0.2), Vec3::new(0.0, 0.0, 0.8), &portal)
                .is_none()
        );
    }

    /// Walking into a floor portal comes out of a wall portal moving along ITS normal: the
    /// velocity through the pair rotation points out of the exit.
    #[test]
    fn velocity_remaps_out_of_the_exit() {
        let entry = pose(Vec3::ZERO, Vec3::Y); // floor portal, walk down into it
        let exit = pose(Vec3::new(10.0, 2.0, 0.0), Vec3::Z); // wall portal
        let v_in = Vec3::new(0.0, -3.0, 0.0); // falling into the floor disc
        let v_out = pair_rotation(&entry, &exit) * v_in;
        // Moving INTO the entry (against its normal) must come OUT of the exit (along its
        // normal).
        assert!(
            v_out.dot(exit.normal) > 0.5,
            "v_out={v_out:?} should exit along +Z"
        );
        assert!((v_out.length() - v_in.length()).abs() < 1e-4, "speed preserved");
    }

    /// Anti-parallel portals (facing walls) don't mirror: entering one wall exits the other
    /// moving AWAY from it.
    #[test]
    fn anti_parallel_pair_cancels_the_mirror() {
        let entry = pose(Vec3::new(0.0, 1.0, 0.0), Vec3::Z);
        let exit = pose(Vec3::new(0.0, 1.0, 10.0), Vec3::NEG_Z);
        let v_in = Vec3::new(0.0, 0.0, -2.0); // walking into the entry face
        let v_out = pair_rotation(&entry, &exit) * v_in;
        assert!(
            v_out.dot(exit.normal) > 0.5,
            "v_out={v_out:?} should exit along -Z"
        );
    }
}
