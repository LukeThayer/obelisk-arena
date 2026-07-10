//! Skill VERBS — the arena-side bridge from obelisk timeline moments to game actions obelisk
//! deliberately doesn't model (placing/consuming persistent objects, moving bodies). The pattern
//! (the wisp-port design's core): obelisk fires `CueEvent`s at every authored timeline moment
//! with position + source + charge; this module OBSERVES them and matches `(skill_id, cue_id)`
//! to a typed verb. Content TOMLs/RONs stay pure obelisk; the verb table below is the arena's
//! whole custom-gameplay vocabulary:
//!
//! | skill                    | moment                   | verb                                |
//! |--------------------------|--------------------------|-------------------------------------|
//! | `portal_orange`/`_blue`  | `on_window_portal_mark`  | place/replace that portal slot      |
//! | `frost_spire`            | `on_window_spike`        | erupt spire at the (pre-snapped) cue position |
//!
//! Everything spawned here is a [`super::skill_objects`] object (limits/lifetime/replication).

use avian3d::prelude::{Collider, Position, RigidBody};
use bevy::prelude::*;
use obelisk_bevy::events::{CueEvent, CueKind};
use serde_json::json;

use crate::trace;

use super::skill_objects::{
    spawn_skill_object, SkillObject, KIND_FROST_SPIRE, KIND_PORTAL_BLUE, KIND_PORTAL_ORANGE,
};
use super::spawn::ClientPlayerMap;

// --- Frost spire tuning (ported from wisp's ice.rs, adapted where noted). The frost-PATCH
// numbers (patch radius, lifetime, trail step, spire match slack) live in
// `config/surfaces/frost.toml` + obelisk's `SURFACE_MATCH_SLACK` now — the surfaces core owns
// them. ---

/// Spire body (wisp `FROST_SPIKE_BASE_WIDTH`/`_HEIGHT`); v1 fixed scale (no tile-size scaling,
/// no spire chaining — documented deferrals).
const SPIRE_WIDTH: f32 = 0.55;
pub(crate) const SPIRE_HEIGHT: f32 = 1.6;
/// The spire erupts over this long (wisp `FROST_SPIKE_RISE_DURATION`) then settles Static.
const SPIRE_RISE_SECS: f32 = 0.15;
/// Spire lifetime (wisp `FROST_SPIKE_LIFETIME`).
const SPIRE_LIFETIME: f32 = 180.0;

// --- Portal tuning: lives in `crate::portals_shared` (shared with the client's predicted
// teleport + render cameras) ---

use crate::portals_shared::{
    disc_rotation, AIR_PORTAL_DISTANCE, PORTAL_RAYCAST_RANGE, SURFACE_INSET,
};

/// A rising frost spire: kinematic until `settle_at`, then Static terrain (wisp's
/// `settle_frost_spike`). While rising its collider physically shoves dynamic bodies.
#[derive(Component)]
pub(crate) struct SpireRise {
    pub settle_at: f32,
}

/// THE verb observer: match `(skill_id, cue_id)` on every server-side obelisk `CueEvent` and run
/// the arena action. Cues were designed as the presentation channel; they are equally the
/// gameplay-verb channel because they carry exactly what a verb needs (author-named id, world
/// position, source entity, charge) at every authored moment.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn skill_verbs_on_cue(
    cue: On<CueEvent>,
    time: Res<Time>,
    objects: Query<(Entity, &SkillObject)>,
    positions: Query<&Position>,
    owners: Query<&crate::net::protocol::NetworkOwner>,
    client_map: Res<ClientPlayerMap>,
    spatial: avian3d::prelude::SpatialQuery,
    children: Query<&Children>,
    actions: Query<&lightyear::prelude::input::native::ActionState<crate::net::input::ArenaInput>>,
    mut commands: Commands,
) {
    let ev = cue.event();
    if ev.kind != CueKind::OnWindow {
        return;
    }
    let now = time.elapsed_secs();
    let owner_id = owners.get(ev.source).map(|o| o.0).unwrap_or_else(|_| {
        // Triggered execs re-source cues to the ORIGINAL caster entity; fall back to a reverse
        // client-map lookup for safety.
        client_map
            .0
            .iter()
            .find(|(_, e)| **e == ev.source)
            .map(|(id, _)| *id)
            .unwrap_or(0)
    });

    match (ev.skill_id.as_str(), ev.cue_id.as_str()) {
        // --- Portals: wisp's placement — raycast from the caster's eye along its CURRENT aim
        // and stick the disc to the hit surface (any wall/floor/ceiling), or float it in the
        // air at a fixed distance facing back at the caster on a miss. The cue's own position
        // is ignored (acquisition is plain `Aim`); the ray excludes the caster, its child
        // hurtbox, and every skill object (never stick a portal to a portal/spire).
        ("portal_orange", "on_window_portal_mark") | ("portal_blue", "on_window_portal_mark") => {
            let kind = if ev.skill_id == "portal_orange" {
                KIND_PORTAL_ORANGE
            } else {
                KIND_PORTAL_BLUE
            };
            let Ok(caster_pos) = positions.get(ev.source) else {
                return;
            };
            let eye = caster_pos.0 + Vec3::Y * arena_sim::tuning::ARENA_EYE_HEIGHT;
            let fwd = actions
                .get(ev.source)
                .map(|a| crate::net::aim_dir(a.0.yaw, a.0.pitch))
                .unwrap_or(Vec3::NEG_Z);
            let mut exclude = vec![ev.source];
            if let Ok(kids) = children.get(ev.source) {
                exclude.extend(kids.iter());
            }
            exclude.extend(objects.iter().map(|(e, _)| e));
            let hit = Dir3::new(fwd).ok().and_then(|dir| {
                spatial.cast_ray(
                    eye,
                    dir,
                    PORTAL_RAYCAST_RANGE,
                    true,
                    &avian3d::prelude::SpatialQueryFilter::default()
                        .with_excluded_entities(exclude),
                )
            });
            let (position, normal) = match hit {
                Some(hit) => (
                    eye + fwd * hit.distance + hit.normal * SURFACE_INSET,
                    hit.normal,
                ),
                None => (eye + fwd * AIR_PORTAL_DISTANCE, -fwd),
            };
            // Disc local +Y = surface normal, local −Z held toward world-up (wisp's
            // disc_rotation — an arbitrary-roll arc rotation would roll the through-view).
            let rotation = disc_rotation(normal);
            spawn_skill_object(
                &mut commands, &objects, now, kind, owner_id, position, rotation, None, None,
            );
            trace::event(
                "portal_placed",
                json!({ "slot": kind, "owner": owner_id,
                        "pos": [position.x, position.y, position.z],
                        "normal": [normal.x, normal.y, normal.z],
                        "surface": hit.is_some() }),
            );
        }

        // --- Frost spire: erupt at the cue position. The position IS the consumed frost
        // patch's center — obelisk's on_surface acquisition (snap: true, consume: true)
        // gated the cast, snapped the cast point, and spent the fuel at cast-accept; this
        // verb only spawns the PHYSICAL spire (a collider-bearing world object stays host
        // territory — spec §8).
        ("frost_spire", "on_window_spike") => {
            // Erupt: kinematic riser buried one height below the patch center, rising at
            // height/0.15 (wisp's exact emergence); `settle_spires` freezes it into Static
            // terrain — which our raycast grounding and wall-aware projectiles then treat as
            // real level geometry (bolts burst on spires for free).
            let rest = ev.position + Vec3::Y * (SPIRE_HEIGHT * 0.5 - 0.08);
            let start = rest - Vec3::Y * SPIRE_HEIGHT;
            let spire = spawn_skill_object(
                &mut commands,
                &objects,
                now,
                KIND_FROST_SPIRE,
                owner_id,
                start,
                Quat::IDENTITY,
                Some(SPIRE_LIFETIME),
                Some((
                    RigidBody::Kinematic,
                    Collider::cuboid(SPIRE_WIDTH, SPIRE_HEIGHT, SPIRE_WIDTH),
                )),
            );
            commands.entity(spire).insert((
                SpireRise {
                    settle_at: now + SPIRE_RISE_SECS,
                },
                avian3d::prelude::LinearVelocity(Vec3::Y * (SPIRE_HEIGHT / SPIRE_RISE_SECS)),
            ));
            trace::event(
                "spire_erupted",
                json!({ "owner": owner_id,
                        "pos": [ev.position.x, ev.position.y, ev.position.z] }),
            );
        }

        _ => {}
    }
}

/// Settle risen spires: zero velocity, snap to rest height, become Static terrain (wisp's
/// `settle_frost_spike` — the obelisk damage window covers the same 0.15-0.25s, so like wisp the
/// spire only hurts during the eruption).
pub(crate) fn settle_spires(
    time: Res<Time>,
    mut q: Query<(
        Entity,
        &SpireRise,
        &mut Position,
        &mut avian3d::prelude::LinearVelocity,
    )>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    for (e, rise, mut pos, mut vel) in &mut q {
        if now < rise.settle_at {
            continue;
        }
        // Trim the sub-tick overshoot past the settle time so the rest height is exact.
        let overshoot = (now - rise.settle_at).min(0.05);
        pos.0.y -= vel.0.y * overshoot;
        vel.0 = Vec3::ZERO;
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.remove::<SpireRise>();
            ec.insert(RigidBody::Static);
        }
    }
}
