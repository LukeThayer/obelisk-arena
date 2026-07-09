//! Portal traversal, SERVER side — the authoritative half of the shared portal pipeline
//! (`crate::portals_shared` holds ALL the math; `client/portals.rs` runs the same logic
//! predictively). Three systems:
//!
//!  - [`portal_floor_passthrough`]: while a player stands inside a FLOOR portal's slab (and the
//!    disc isn't in its lockout), its `Ground` collisions drop so the body falls INTO the disc
//!    (wisp `update_player_falling`). Runs before the physics step.
//!  - [`portal_teleport`]: crossing detection + warp for players (wisp's virtual transform —
//!    emerge in front of the exit facing out of it) and obelisk projectile hitboxes (velocity +
//!    aim + `LastHitboxPos` remapped so the flight continues out of the exit). Any body travels
//!    through any COMPLETE pair (Portal semantics), pairs are per-owner. After the physics step.
//!  - [`refresh_portal_passthrough`]: publishes the live discs into arena_sim's
//!    [`PortalPassthrough`] so `report_world_hits` lets projectiles fly THROUGH portal-bearing
//!    surfaces instead of ending them as world hits.
//!
//! The player's FACING is not snapped server-side — the server's `Rotation` is re-derived from
//! `ArenaInput.yaw` every tick, so only the owning client can turn the camera; the client's
//! PREDICTED teleport rotates its own `CameraYaw` through the pair (an un-predicted server
//! teleport falls back to keep-your-heading, wisp's Q.5b trade-off).

use avian3d::prelude::{CollisionLayers, LayerMask, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use obelisk_bevy::prelude::Hitbox;
use obelisk_bevy::spatial::projectile::Projectile;
use serde_json::json;
use std::collections::HashMap;

use crate::net::input::ArenaInput;
use crate::net::protocol::NetworkedPlayer;
use crate::portals_shared::{
    collect_pairs, in_floor_slab, map_through_pair, player_crossing, portal_virtual_transform,
    projectile_crossing, velocity_rotation, yaw_from_forward, PortalPose, EXIT_STAND_OFFSET,
    LOCKOUT_RADIUS,
};
use crate::trace;

use super::skill_objects::SkillObject;

/// Per-traveler teleport state: previous tick position (crossing segments) + the exit-portal
/// position the traveler must clear before re-triggering (the lockout).
#[derive(Resource, Default)]
pub(crate) struct PortalTravelers {
    pub prev: HashMap<Entity, Vec3>,
    pub lockout: HashMap<Entity, Vec3>,
}

/// The live complete pairs, computed once per tick and shared by the three systems.
fn live_pairs(
    portals: &Query<(&SkillObject, &Position, &Rotation)>,
) -> Vec<(PortalPose, PortalPose)> {
    collect_pairs(
        portals
            .iter()
            .map(|(o, p, r)| (o.owner, o.kind, p.0, r.0)),
    )
}

/// Drop `Ground` from a player's collision filter while it stands inside a floor-portal slab
/// whose disc isn't locked out for it — the body sinks through the floor INTO the disc. Restores
/// the full mask everywhere else. FixedUpdate, before the physics step.
pub(crate) fn portal_floor_passthrough(
    portals: Query<(&SkillObject, &Position, &Rotation)>,
    travelers: Res<PortalTravelers>,
    mut players: Query<
        (Entity, &Position, &mut CollisionLayers),
        (With<NetworkedPlayer>, Without<SkillObject>),
    >,
) {
    let pairs = live_pairs(&portals);
    for (e, pos, mut layers) in &mut players {
        let locked_near = travelers.lockout.get(&e).copied();
        let in_slab = pairs.iter().any(|(a, b)| {
            [a, b].into_iter().any(|disc| {
                // A disc inside the traveler's lockout neighborhood doesn't re-open the floor —
                // you STAND on the exit disc you just emerged from until you walk away.
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

/// Publish the live discs into arena_sim's world-hit exemption resource so projectiles fly
/// through portal-bearing surfaces (`report_world_hits` skips impacts inside a disc).
pub(crate) fn refresh_portal_passthrough(
    portals: Query<(&SkillObject, &Position, &Rotation)>,
    mut passthrough: ResMut<arena_sim::obelisk::PortalPassthrough>,
) {
    let pairs = live_pairs(&portals);
    passthrough.discs.clear();
    for (a, b) in &pairs {
        for disc in [a, b] {
            passthrough
                .discs
                .push((disc.position, disc.normal, crate::net::PORTAL_RADIUS));
        }
    }
}

/// The authoritative teleport. Players: wisp's virtual transform (emerge in front of the exit,
/// wall→floor exits stand the capsule on the disc) + velocity through the pair rotation.
/// Projectiles: position/heading/`Projectile.velocity` remapped, `LastHitboxPos` reset so the
/// next world-hit segment doesn't span the warp. FixedUpdate, after the physics step + obelisk
/// projectile motion.
#[allow(clippy::type_complexity)]
pub(crate) fn portal_teleport(
    portals: Query<(&SkillObject, &Position, &Rotation)>,
    mut players: Query<
        (
            Entity,
            &mut Position,
            &mut LinearVelocity,
            &ActionState<ArenaInput>,
        ),
        (With<NetworkedPlayer>, Without<SkillObject>),
    >,
    mut projectiles: Query<
        (
            Entity,
            &mut Transform,
            &mut Hitbox,
            &mut Projectile,
            Option<&mut arena_sim::obelisk::LastHitboxPos>,
        ),
        Without<SkillObject>,
    >,
    mut travelers: ResMut<PortalTravelers>,
) {
    let pairs = live_pairs(&portals);

    // --- Players ---
    for (e, mut pos, mut vel, action) in &mut players {
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
                // View rotation at the crossing: the player's own aim (the same yaw+pitch the
                // client's camera carries — ArenaInput is the shared truth).
                let view_rot = Quat::from_axis_angle(Vec3::Y, action.0.yaw)
                    * Quat::from_axis_angle(Vec3::X, action.0.pitch);
                let basis_tf = Transform::from_translation(basis).with_rotation(view_rot);
                let virt = portal_virtual_transform(basis_tf, entry, exit);
                let mut out_pos = virt.translation;
                if exit.is_horizontal() && !entry.is_horizontal() {
                    // Wall→floor: stand the capsule on the disc instead of clipping the floor.
                    out_pos.y = exit.position.y + EXIT_STAND_OFFSET * exit.normal.y.signum();
                }
                let out_vel = velocity_rotation(entry, exit) * vel.0;
                pos.0 = out_pos;
                vel.0 = out_vel;
                travelers.lockout.insert(e, exit.position);
                travelers.prev.insert(e, out_pos);
                let new_yaw = yaw_from_forward(virt.rotation * Vec3::NEG_Z);
                trace::event(
                    "portal_teleport",
                    json!({ "traveler": "player",
                            "to": [out_pos.x, out_pos.y, out_pos.z],
                            "yaw": new_yaw }),
                );
                done = true;
                break;
            }
        }
    }

    // --- Obelisk projectiles (Transform-flown hitboxes) ---
    for (e, mut tf, mut hitbox, mut projectile, mut last_pos) in &mut projectiles {
        let prev = travelers
            .prev
            .insert(e, tf.translation)
            .unwrap_or(tf.translation);
        if let Some(exit_pos) = travelers.lockout.get(&e).copied() {
            if (tf.translation - exit_pos).length() > LOCKOUT_RADIUS {
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
                let Some(basis) = projectile_crossing(prev, tf.translation, entry) else {
                    continue;
                };
                let rot = velocity_rotation(entry, exit);
                let out_pos = map_through_pair(basis, entry, exit);
                tf.translation = out_pos;
                tf.rotation = rot * tf.rotation;
                // The flight continues out of the exit: `move_projectiles` integrates
                // `Projectile.velocity` (gravity keeps pulling world-down), and the hit cone /
                // chain math reads `Hitbox.aim`.
                projectile.velocity = rot * projectile.velocity;
                hitbox.aim = (rot * hitbox.aim).normalize_or_zero();
                if let Some(last) = last_pos.as_mut() {
                    last.0 = out_pos; // next world-hit segment starts at the exit
                }
                travelers.lockout.insert(e, exit.position);
                travelers.prev.insert(e, out_pos);
                trace::event(
                    "portal_teleport",
                    json!({ "traveler": "projectile",
                            "to": [out_pos.x, out_pos.y, out_pos.z] }),
                );
                done = true;
                break;
            }
        }
    }

    // Cheap hygiene: forget state for entities long gone.
    if travelers.prev.len() > 512 {
        travelers.prev.clear();
        travelers.lockout.clear();
    }
}
