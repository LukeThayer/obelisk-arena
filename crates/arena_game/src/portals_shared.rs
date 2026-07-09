//! Portal math shared VERBATIM by the server's authoritative teleport, the client's PREDICTED
//! teleport, and the client's through-portal render cameras — wisp's `spells/portal.rs` +
//! `server_portal_teleport` ported onto arena skill objects. One module so the three consumers
//! can never drift: under lightyear prediction the client re-runs the same teleport the server
//! runs, and any math skew would rollback-fight every crossing.
//!
//! Conventions (wisp's): a portal disc's LOCAL +Y is its outward surface normal
//! ([`disc_rotation`] keeps world-up aligned with local −Z so the through-view doesn't roll).
//! Pairs are per-owner (orange + blue of the same caster); BOTH ends must exist to function.

use bevy::prelude::*;

pub use crate::net::PORTAL_RADIUS;

/// The portal skill-object kinds (shared: the server spawns them, the client keys visuals +
/// predicted teleports off the replicated `NetworkedSkillObject.kind`).
pub const KIND_PORTAL_ORANGE: &str = "portal_orange";
pub const KIND_PORTAL_BLUE: &str = "portal_blue";

// --- Tuning (wisp's constants, arena-adapted where noted) ---

/// Disc mesh thickness (wisp `PORTAL_THICKNESS`).
pub const PORTAL_THICKNESS: f32 = 0.04;
/// Placement raycast range (wisp `PORTAL_RAYCAST_RANGE`).
pub const PORTAL_RAYCAST_RANGE: f32 = 15.0;
/// A miss floats the portal in the air this far ahead, facing back at the caster (wisp
/// `AIR_PORTAL_DISTANCE`).
pub const AIR_PORTAL_DISTANCE: f32 = 2.5;
/// Placement offset off the hit surface (wisp `SURFACE_INSET`).
pub const SURFACE_INSET: f32 = 0.02;
/// |normal.y| at or above this = a HORIZONTAL (floor/ceiling) portal (wisp
/// `HORIZONTAL_NORMAL_DOT_Y`).
pub const HORIZONTAL_NORMAL_DOT_Y: f32 = 0.7;
/// Half-depth of the floor-portal slab in which a player's Ground collisions drop so it falls
/// INTO the disc (wisp `HORIZONTAL_DISC_DEPTH`).
pub const HORIZONTAL_DISC_DEPTH: f32 = 2.0;
/// Re-entry lockout: after a teleport the traveler must leave this radius of the EXIT before it
/// can cross again (wisp `LOCKOUT_RADIUS`).
pub const LOCKOUT_RADIUS: f32 = PORTAL_RADIUS * 1.5;
/// Wall-portal trigger band: a player whose center comes within this of a vertical disc's plane
/// (from farther away) teleports — the capsule (r 0.35) pressed against the wall can never
/// sign-flip across it, so the band IS the wall crossing (wisp's retired local rule, r widened
/// past the arena capsule radius so wall contact reliably triggers).
pub const WALL_TRIGGER_ALONG: f32 = 0.45;
/// Wall→floor exits stand the capsule on the disc: bottom of the arena capsule
/// (radius 0.35 + half-length 0.24) plus clearance.
pub const EXIT_STAND_OFFSET: f32 = 0.62;

/// One disc's pose for all portal math.
#[derive(Clone, Copy, Debug)]
pub struct PortalPose {
    pub position: Vec3,
    pub normal: Vec3,
    pub rotation: Quat,
}

impl PortalPose {
    pub fn new(position: Vec3, rotation: Quat) -> Self {
        Self {
            position,
            normal: (rotation * Vec3::Y).normalize_or_zero(),
            rotation,
        }
    }

    /// Floor/ceiling disc?
    pub fn is_horizontal(&self) -> bool {
        self.normal.y.abs() >= HORIZONTAL_NORMAL_DOT_Y
    }
}

/// Disc orientation from a surface normal (wisp `disc_rotation`): local +Y = normal, and local
/// −Z stays as close to world-up as possible so the rendered through-view doesn't roll.
pub fn disc_rotation(normal: Vec3) -> Quat {
    let y_axis = normal.normalize_or_zero();
    if y_axis == Vec3::ZERO {
        return Quat::IDENTITY;
    }
    let world_up = Vec3::Y;
    let z_axis = if y_axis.cross(world_up).length_squared() < 1e-6 {
        Vec3::Z
    } else {
        (world_up - y_axis * y_axis.dot(world_up)).normalize()
    };
    let x_axis = y_axis.cross(z_axis);
    Quat::from_mat3(&Mat3::from_cols(x_axis, y_axis, z_axis))
}

/// Facing walls (normals roughly opposite)? The natural `exit × entry⁻¹` transform mirrors
/// world-X for such pairs; the flip below cancels it (wisp `anti_parallel_normals`).
pub fn anti_parallel(entry: &PortalPose, exit: &PortalPose) -> bool {
    entry.normal.dot(exit.normal) < -0.5
}

/// The VELOCITY mapping through the pair (wisp's `q_vel`): anti-parallel pairs flip about local
/// Z (mirror cancel + into→out-of); every other pair flips about local X (into→out-of only,
/// world-X component preserved — an unconditional Z flip is what broke wisp's ground portals).
pub fn velocity_rotation(entry: &PortalPose, exit: &PortalPose) -> Quat {
    let flip = if anti_parallel(entry, exit) {
        Quat::from_rotation_z(std::f32::consts::PI)
    } else {
        Quat::from_rotation_x(std::f32::consts::PI)
    };
    exit.rotation * flip * entry.rotation.inverse()
}

/// Where a body near the entry maps to on the exit side (wisp's non-player path): local offset
/// preserved, X reflected only for anti-parallel pairs, and a one-sided GROUND exit clamps to
/// the accessible +Y side (the −Y side is buried in the floor).
pub fn map_through_pair(point: Vec3, entry: &PortalPose, exit: &PortalPose) -> Vec3 {
    let mut local = entry.rotation.inverse() * (point - entry.position);
    if anti_parallel(entry, exit) {
        local.x = -local.x;
    }
    if exit.normal.y >= HORIZONTAL_NORMAL_DOT_Y {
        local.y = local.y.abs();
    }
    exit.position + exit.rotation * local
}

/// Player virtual transform through the pair (wisp `portal_virtual_transform`): position via
/// [`map_through_pair`]'s rules, view forward rotated through the pair with the Z-π flip so the
/// player emerges FACING OUT of the exit disc.
pub fn portal_virtual_transform(
    player_tf: Transform,
    entry: &PortalPose,
    exit: &PortalPose,
) -> Transform {
    let rot_flip = Quat::from_rotation_z(std::f32::consts::PI);
    let entry_inv_rot = entry.rotation.inverse();
    let virtual_pos = map_through_pair(player_tf.translation, entry, exit);
    let local_forward = rot_flip * (entry_inv_rot * (player_tf.rotation * Vec3::NEG_Z));
    let forward = exit.rotation * local_forward;
    let mut tf = Transform::from_translation(virtual_pos);
    tf.look_to(forward, fallback_up(forward, &player_tf));
    tf
}

/// Where the render-to-texture portal camera sits (wisp `portal_camera_transform`): the MIRROR
/// of the virtual transform across the exit disc plane — behind the exit, looking through it
/// into the room the player would emerge into. Pair with the oblique near-clip plane (the exit
/// disc plane) to clip out the wall back / entry disc between camera and exit.
pub fn portal_camera_transform(
    viewer_tf: Transform,
    entry: &PortalPose,
    exit: &PortalPose,
) -> Transform {
    let rot_flip = Quat::from_rotation_z(std::f32::consts::PI);
    let needs_x_flip = anti_parallel(entry, exit);
    let entry_inv_rot = entry.rotation.inverse();
    let mut local_pos = entry_inv_rot * (viewer_tf.translation - entry.position);
    local_pos.y = -local_pos.y;
    if needs_x_flip {
        local_pos.x = -local_pos.x;
    }
    // One-sided ground exit: the camera always belongs BEHIND the disc (below the floor) so it
    // renders the room above.
    if exit.normal.y >= HORIZONTAL_NORMAL_DOT_Y {
        local_pos.y = -local_pos.y.abs();
    }
    let local_forward = rot_flip * (entry_inv_rot * (viewer_tf.rotation * Vec3::NEG_Z));
    let virtual_pos = exit.position + exit.rotation * local_pos;
    let forward = exit.rotation * local_forward;
    let mut tf = Transform::from_translation(virtual_pos);
    tf.look_to(forward, fallback_up(forward, &viewer_tf));
    tf
}

fn fallback_up(forward: Vec3, player_tf: &Transform) -> Vec3 {
    let world_up = Vec3::Y;
    if forward.cross(world_up).length_squared() > 1e-6 {
        world_up
    } else {
        let player_fwd = player_tf.rotation * Vec3::NEG_Z;
        Vec3::new(player_fwd.x, 0.0, player_fwd.z)
            .try_normalize()
            .unwrap_or(Vec3::X)
    }
}

/// The camera yaw matching a world forward vector (wisp's atan2 — the post-teleport heading the
/// local camera snaps to). `None` for a degenerate straight-up/down forward.
pub fn yaw_from_forward(forward: Vec3) -> Option<f32> {
    let horiz_len_sq = forward.x * forward.x + forward.z * forward.z;
    if horiz_len_sq <= 1e-6 {
        return None;
    }
    let inv = horiz_len_sq.sqrt().recip();
    Some((-forward.x * inv).atan2(-forward.z * inv))
}

/// PLAYER crossing test. Horizontal (floor/ceiling) discs use the plain segment sign-flip (the
/// pass-through layer drop lets the body actually sink through the plane); vertical (wall/air)
/// discs use the pressed-band rule — the center comes from beyond [`WALL_TRIGGER_ALONG`] to
/// within it (a capsule against a solid wall can never sign-flip). Returns the BASIS position
/// the exit mapping should transform (the entry-side point, wisp's `basis_pos`).
pub fn player_crossing(prev: Vec3, cur: Vec3, entry: &PortalPose) -> Option<Vec3> {
    let prev_along = (prev - entry.position).dot(entry.normal);
    let curr_along = (cur - entry.position).dot(entry.normal);
    let (crossed, target_along) = if entry.is_horizontal() {
        ((prev_along > 0.0) != (curr_along > 0.0), 0.0)
    } else {
        (
            prev_along.abs() > WALL_TRIGGER_ALONG && curr_along.abs() <= WALL_TRIGGER_ALONG,
            WALL_TRIGGER_ALONG * prev_along.signum(),
        )
    };
    if !crossed {
        return None;
    }
    let denom = curr_along - prev_along;
    let t = if denom.abs() > 1e-6 {
        ((target_along - prev_along) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let cross = prev.lerp(cur, t);
    let rel = cross - entry.position;
    let radial = (rel - entry.normal * rel.dot(entry.normal)).length();
    if radial > PORTAL_RADIUS {
        return None;
    }
    // Basis: sign-flip crossings map the endpoint on the side the body CAME from; band
    // crossings map the band-entry point itself (it still sits on the approach side).
    if entry.is_horizontal() {
        Some(if prev_along > 0.0 { prev } else { cur })
    } else {
        Some(cross)
    }
}

/// PROJECTILE crossing test (wisp's traveler rule): plain sign flip + radial gate. Returns the
/// basis position (entry-side endpoint).
pub fn projectile_crossing(prev: Vec3, cur: Vec3, entry: &PortalPose) -> Option<Vec3> {
    let prev_along = (prev - entry.position).dot(entry.normal);
    let curr_along = (cur - entry.position).dot(entry.normal);
    if prev_along == 0.0 && curr_along == 0.0 {
        return None;
    }
    if (prev_along > 0.0) == (curr_along > 0.0) {
        return None;
    }
    let t = (prev_along / (prev_along - curr_along)).clamp(0.0, 1.0);
    let cross = prev.lerp(cur, t);
    let rel = cross - entry.position;
    let radial = (rel - entry.normal * rel.dot(entry.normal)).length();
    (radial <= PORTAL_RADIUS).then_some(if prev_along > 0.0 { prev } else { cur })
}

/// Is `pos` inside a HORIZONTAL disc's pass-through slab (wisp `update_player_falling`'s
/// threshold)? While true (and the disc isn't locked out for this body), the body's Ground
/// collisions drop so it falls into the floor disc.
pub fn in_floor_slab(pos: Vec3, portal: &PortalPose) -> bool {
    if !portal.is_horizontal() {
        return false;
    }
    let rel = pos - portal.position;
    let along = rel.dot(portal.normal);
    let radial = (rel - portal.normal * along).length();
    radial < PORTAL_RADIUS && along.abs() < HORIZONTAL_DISC_DEPTH
}

/// Complete per-owner pairs from any (owner, kind, position, rotation) view of the live portal
/// objects — the SAME collector on the server (skill objects) and the client (replicated
/// `NetworkedSkillObject`s). Returns (orange, blue) per owner with both ends placed.
pub fn collect_pairs(
    portals: impl Iterator<Item = (u64, &'static str, Vec3, Quat)>,
) -> Vec<(PortalPose, PortalPose)> {
    use std::collections::HashMap;
    let mut by_owner: HashMap<u64, (Option<PortalPose>, Option<PortalPose>)> = HashMap::new();
    for (owner, kind, pos, rot) in portals {
        let slot = by_owner.entry(owner).or_default();
        match kind {
            k if k == KIND_PORTAL_ORANGE => {
                slot.0 = Some(PortalPose::new(pos, rot));
            }
            k if k == KIND_PORTAL_BLUE => {
                slot.1 = Some(PortalPose::new(pos, rot));
            }
            _ => {}
        }
    }
    let mut pairs: Vec<_> = by_owner
        .into_iter()
        .filter_map(|(owner, (o, b))| Some((owner, (o?, b?))))
        .collect();
    // Deterministic order (owner id) — prediction + server must scan pairs identically.
    pairs.sort_by_key(|(owner, _)| *owner);
    pairs.into_iter().map(|(_, pair)| pair).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor_at(p: Vec3) -> PortalPose {
        PortalPose::new(p, disc_rotation(Vec3::Y))
    }
    fn wall_at(p: Vec3, n: Vec3) -> PortalPose {
        PortalPose::new(p, disc_rotation(n))
    }

    /// disc_rotation keeps +Y = normal and never rolls a wall disc.
    #[test]
    fn disc_rotation_maps_up_to_normal() {
        for n in [Vec3::Y, Vec3::NEG_Y, Vec3::X, Vec3::Z, Vec3::new(0.6, 0.0, 0.8)] {
            let rot = disc_rotation(n);
            assert!((rot * Vec3::Y - n.normalize()).length() < 1e-4, "n={n:?}");
        }
    }

    /// Falling into a floor portal exits a wall portal moving OUT of it, speed preserved.
    #[test]
    fn velocity_remaps_out_of_the_exit() {
        let entry = floor_at(Vec3::ZERO);
        let exit = wall_at(Vec3::new(10.0, 2.0, 0.0), Vec3::Z);
        let v_out = velocity_rotation(&entry, &exit) * Vec3::new(0.0, -3.0, 0.0);
        assert!(v_out.dot(exit.normal) > 0.5, "v_out={v_out:?}");
        assert!((v_out.length() - 3.0).abs() < 1e-4);
    }

    /// Anti-parallel walls (facing each other) cancel the mirror: walking INTO one comes OUT of
    /// the other, and the world-X of a sideways drift is mirrored exactly once (not zero, not
    /// twice) so the player emerges drifting the same apparent direction.
    #[test]
    fn anti_parallel_pair_cancels_the_mirror() {
        let entry = wall_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Z);
        let exit = wall_at(Vec3::new(0.0, 1.0, 10.0), Vec3::NEG_Z);
        assert!(anti_parallel(&entry, &exit));
        let v_out = velocity_rotation(&entry, &exit) * Vec3::new(0.0, 0.0, -2.0);
        assert!(v_out.dot(exit.normal) > 0.5, "v_out={v_out:?}");
    }

    /// Ground-exit one-sidedness: a body mapped through to a floor exit always lands on the
    /// accessible +Y side, whichever side of the entry it came from.
    #[test]
    fn ground_exit_clamps_to_the_accessible_side() {
        let entry = wall_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Z);
        let exit = floor_at(Vec3::new(8.0, 0.0, 0.0));
        // A point BEHIND the entry (local -Y) would naively map below the floor.
        let behind = Vec3::new(0.0, 1.0, -0.5);
        let out = map_through_pair(behind, &entry, &exit);
        assert!(out.y > exit.position.y, "out={out:?} must be above the floor disc");
    }

    /// The player emerges FACING OUT of the exit: walking forward into a wall portal, the
    /// virtual forward points along the exit normal.
    #[test]
    fn virtual_transform_faces_out_of_the_exit() {
        let entry = wall_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Z);
        let exit = wall_at(Vec3::new(10.0, 1.0, 5.0), Vec3::X);
        let player = Transform::from_translation(Vec3::new(0.0, 1.0, 0.5))
            .looking_to(Vec3::NEG_Z, Vec3::Y); // walking -Z, INTO the entry face
        let virt = portal_virtual_transform(player, &entry, &exit);
        let fwd = virt.rotation * Vec3::NEG_Z;
        assert!(fwd.dot(exit.normal) > 0.5, "fwd={fwd:?} should exit along +X");
        let yaw = yaw_from_forward(fwd).unwrap();
        assert!((yaw - (-std::f32::consts::FRAC_PI_2)).abs() < 1e-3, "yaw={yaw}");
    }

    /// The render camera sits BEHIND the exit disc (opposite side from the emergence point),
    /// looking through it.
    #[test]
    fn camera_sits_behind_the_exit() {
        let entry = wall_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Z);
        let exit = wall_at(Vec3::new(10.0, 1.0, 5.0), Vec3::X);
        let viewer = Transform::from_translation(Vec3::new(0.0, 1.0, 2.0))
            .looking_to(Vec3::NEG_Z, Vec3::Y);
        let cam = portal_camera_transform(viewer, &entry, &exit);
        let side = (cam.translation - exit.position).dot(exit.normal);
        assert!(side < 0.0, "camera at {:?} must be behind the exit plane", cam.translation);
        let fwd = cam.rotation * Vec3::NEG_Z;
        assert!(fwd.dot(exit.normal) > 0.0, "camera looks THROUGH the exit");
    }

    /// Horizontal discs cross on the sign flip; walls trigger on the pressed band (a capsule
    /// against a wall never sign-flips).
    #[test]
    fn crossing_rules_horizontal_vs_wall() {
        let floor = floor_at(Vec3::ZERO);
        assert!(player_crossing(Vec3::new(0.0, 0.3, 0.0), Vec3::new(0.0, -0.1, 0.0), &floor)
            .is_some());
        assert!(player_crossing(Vec3::new(2.0, 0.3, 0.0), Vec3::new(2.0, -0.1, 0.0), &floor)
            .is_none(), "outside the radius");

        let wall = wall_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Z);
        // Pressed from 0.6 to 0.37 in front: band rule fires without a sign flip.
        assert!(player_crossing(
            Vec3::new(0.0, 1.0, 0.6),
            Vec3::new(0.0, 1.0, 0.37),
            &wall
        )
        .is_some());
        // Hovering inside the band without having entered it this tick: no re-trigger.
        assert!(player_crossing(
            Vec3::new(0.0, 1.0, 0.30),
            Vec3::new(0.0, 1.0, 0.28),
            &wall
        )
        .is_none());
    }

    /// Projectiles: plain sign flip + radial gate.
    #[test]
    fn projectile_crossing_matches_wisp() {
        let wall = wall_at(Vec3::ZERO, Vec3::Z);
        assert!(projectile_crossing(
            Vec3::new(0.0, 0.0, -0.5),
            Vec3::new(0.0, 0.0, 0.5),
            &wall
        )
        .is_some());
        assert!(projectile_crossing(
            Vec3::new(2.0, 0.0, -0.5),
            Vec3::new(2.0, 0.0, 0.5),
            &wall
        )
        .is_none());
        assert!(projectile_crossing(
            Vec3::new(0.0, 0.0, 0.2),
            Vec3::new(0.0, 0.0, 0.8),
            &wall
        )
        .is_none());
    }

    /// Only complete per-owner pairs collect; two owners = two independent pairs.
    #[test]
    fn pairs_are_per_owner_and_complete_only() {
        let o = KIND_PORTAL_ORANGE;
        let b = KIND_PORTAL_BLUE;
        let rot = disc_rotation(Vec3::Y);
        let pairs = collect_pairs(
            [
                (1u64, o, Vec3::ZERO, rot),
                (1u64, b, Vec3::new(5.0, 0.0, 0.0), rot),
                (2u64, o, Vec3::new(9.0, 0.0, 0.0), rot), // blue missing — incomplete
            ]
            .into_iter(),
        );
        assert_eq!(pairs.len(), 1);
        assert!((pairs[0].1.position.x - 5.0).abs() < 1e-6);
    }
}
