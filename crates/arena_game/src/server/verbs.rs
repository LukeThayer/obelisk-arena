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
//! | `frost_spire`            | `on_window_spike`        | consume nearest frost tile → spire  |
//! | `glacier_roll` (window)  | (polled, not cue)        | drop a frost tile every 0.8m        |
//!
//! Everything spawned here is a [`super::skill_objects`] object (limits/lifetime/replication).

use avian3d::prelude::{Collider, Position, RigidBody};
use bevy::prelude::*;
use obelisk_bevy::events::{CueEvent, CueKind};
use obelisk_bevy::prelude::Hitbox;
use serde_json::json;

use crate::trace;

use super::skill_objects::{
    spawn_skill_object, SkillObject, KIND_FROST_SPIRE, KIND_FROST_TILE, KIND_PORTAL_BLUE,
    KIND_PORTAL_ORANGE,
};
use super::spawn::ClientPlayerMap;

// --- Frost tuning (ported from wisp's ice.rs, adapted where noted) ---

/// Frost tile radius (wisp `GLACIER_TRAIL_RADIUS`).
pub(crate) const FROST_TILE_RADIUS: f32 = 0.45;
/// Distance the glacier roll travels between tile drops (wisp `GLACIER_TRAIL_STEP`).
const TRAIL_STEP: f32 = 0.8;
/// Tile lifetime (wisp `FROZEN_GROUND_LIFETIME`).
const FROST_TILE_LIFETIME: f32 = 180.0;
/// How close (XZ) the frost_spire cast point must be to a tile to consume it — the tile radius
/// plus wisp's `FROST_SPIRE_MATCH_SLACK`.
pub(crate) const SPIRE_MATCH_RANGE: f32 = FROST_TILE_RADIUS + 0.3;
/// Spire body (wisp `FROST_SPIKE_BASE_WIDTH`/`_HEIGHT`); v1 fixed scale (no tile-size scaling,
/// no spire chaining — documented deferrals).
const SPIRE_WIDTH: f32 = 0.55;
pub(crate) const SPIRE_HEIGHT: f32 = 1.6;
/// The spire erupts over this long (wisp `FROST_SPIKE_RISE_DURATION`) then settles Static.
const SPIRE_RISE_SECS: f32 = 0.15;
/// Spire lifetime (wisp `FROST_SPIKE_LIFETIME`).
const SPIRE_LIFETIME: f32 = 180.0;

// --- Portal tuning (ported from wisp's portal.rs) ---

/// Portal disc radius — the SHARED tuning const (client mesh + server crossing test agree).
pub(crate) use crate::net::PORTAL_RADIUS;
/// Placement offset off the hit surface (wisp `SURFACE_INSET`, widened for our thicker discs).
const PORTAL_SURFACE_INSET: f32 = 0.06;

/// A rising frost spire: kinematic until `settle_at`, then Static terrain (wisp's
/// `settle_frost_spike`). While rising its collider physically shoves dynamic bodies.
#[derive(Component)]
pub(crate) struct SpireRise {
    pub settle_at: f32,
}

/// Per-roll-window trail bookkeeping: where the last tile dropped.
#[derive(Default)]
pub(crate) struct TrailMemory(pub std::collections::HashMap<Entity, Vec3>);

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
        // --- Portals: place/replace the slot's disc at the cast point, oriented to the surface.
        ("portal_orange", "on_window_portal_mark") | ("portal_blue", "on_window_portal_mark") => {
            let kind = if ev.skill_id == "portal_orange" {
                KIND_PORTAL_ORANGE
            } else {
                KIND_PORTAL_BLUE
            };
            // Recover the surface normal: cast a short ray from the caster's eye toward the
            // mark (the acquisition already proved line-of-sight to the point). Exclude the
            // caster AND its child hurtbox (same as `grounded_by_ray`) — the SelfPoint-fallback
            // mark sits at the caster's feet and an unfiltered eye→feet ray hits its own
            // hurtbox. A miss faces the disc up.
            let normal = positions
                .get(ev.source)
                .ok()
                .and_then(|caster| {
                    let eye = caster.0 + Vec3::Y * arena_sim::tuning::ARENA_EYE_HEIGHT;
                    let to = ev.position - eye;
                    let dir = Dir3::new(to).ok()?;
                    let mut exclude = vec![ev.source];
                    if let Ok(kids) = children.get(ev.source) {
                        exclude.extend(kids.iter());
                    }
                    spatial
                        .cast_ray(
                            eye,
                            dir,
                            to.length() + 0.5,
                            true,
                            &avian3d::prelude::SpatialQueryFilter::default()
                                .with_excluded_entities(exclude),
                        )
                        .map(|hit| hit.normal)
                })
                .filter(|n| n.length_squared() > 0.5)
                .unwrap_or(Vec3::Y);
            let position = ev.position + normal * PORTAL_SURFACE_INSET;
            // Disc local +Y = surface normal (wisp's disc_rotation).
            let rotation = Quat::from_rotation_arc(Vec3::Y, normal);
            spawn_skill_object(
                &mut commands, &objects, now, kind, owner_id, position, rotation, None, None,
            );
            trace::event(
                "portal_placed",
                json!({ "slot": kind, "owner": owner_id,
                        "pos": [position.x, position.y, position.z] }),
            );
        }

        // --- Frost spire: consume the nearest tile at the cast point, erupt a spire there.
        ("frost_spire", "on_window_spike") => {
            let mark = ev.position;
            let nearest_tile = objects
                .iter()
                .filter(|(_, o)| o.kind == KIND_FROST_TILE)
                .filter_map(|(e, _)| positions.get(e).ok().map(|p| (e, p.0)))
                .map(|(e, p)| (e, p, Vec2::new(p.x - mark.x, p.z - mark.z).length()))
                .filter(|(_, _, d)| *d <= SPIRE_MATCH_RANGE)
                .min_by(|a, b| a.2.total_cmp(&b.2));
            let Some((tile, tile_pos, _)) = nearest_tile else {
                // The aim validator makes this near-impossible (a tile expired in the windup
                // window) — the obelisk damage window still fired; no spire grows.
                trace::event("spire_fizzled_no_tile", json!({ "owner": owner_id }));
                return;
            };
            // Consume the fuel (wisp: tile despawns).
            if let Ok(mut ec) = commands.get_entity(tile) {
                ec.despawn();
            }
            // Erupt: kinematic riser buried one height below the tile, rising at height/0.15
            // (wisp's exact emergence); `settle_spires` freezes it into Static terrain — which
            // our raycast grounding and wall-aware projectiles then treat as real level
            // geometry (bolts burst on spires for free).
            let rest = tile_pos + Vec3::Y * (SPIRE_HEIGHT * 0.5 - 0.08);
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
                        "pos": [tile_pos.x, tile_pos.y, tile_pos.z] }),
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

/// Drop a frost tile behind every live `glacier_roll` "roll" window each `TRAIL_STEP` meters of
/// travel (wisp's `drop_glacier_trail`, distance-based). Polled off the live obelisk hitbox
/// TRANSFORM (obelisk projectiles fly on `Transform`) rather than a cue: an emitter-based trail
/// would make the tile Templates share the roll skill's lifecycle triggers (every tile expiry
/// would re-fire the on_expire burst) — the polling system sidesteps that schema constraint and
/// matches wisp's per-distance semantics exactly.
#[allow(clippy::type_complexity)]
pub(crate) fn drop_glacier_trail(
    time: Res<Time>,
    rolls: Query<(Entity, &Hitbox, &Transform)>,
    objects: Query<(Entity, &SkillObject)>,
    positions: Query<&Position>,
    owners: Query<&crate::net::protocol::NetworkOwner>,
    spatial: avian3d::prelude::SpatialQuery,
    mut memory: Local<TrailMemory>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    let mut live: Vec<Entity> = Vec::new();
    for (e, hitbox, tf) in &rolls {
        if hitbox.skill_id != "glacier_roll" {
            continue;
        }
        live.push(e);
        let pos = tf.translation;
        let last = memory.0.get(&e).copied();
        let moved = last.map(|l| (pos - l).length()).unwrap_or(f32::INFINITY);
        if moved < TRAIL_STEP {
            continue;
        }
        memory.0.insert(e, pos);
        // Ground-snap the tile (wisp raycasts down up to 4m).
        let ground_y = spatial
            .cast_ray(
                pos + Vec3::Y * 0.2,
                Dir3::NEG_Y,
                4.0,
                true,
                &avian3d::prelude::SpatialQueryFilter::default(),
            )
            .map(|hit| pos.y + 0.2 - hit.distance)
            .unwrap_or(pos.y - 0.3);
        let tile_pos = Vec3::new(pos.x, ground_y + 0.04, pos.z);
        // Dedup vs nearby tiles (wisp GLACIER_TILE_DEDUP_DIST = 0.25 XZ).
        let too_close = objects
            .iter()
            .filter(|(_, o)| o.kind == KIND_FROST_TILE)
            .filter_map(|(te, _)| positions.get(te).ok())
            .any(|p| Vec2::new(p.0.x - tile_pos.x, p.0.z - tile_pos.z).length() < 0.25);
        if too_close {
            continue;
        }
        let owner_id = owners.get(hitbox.caster).map(|o| o.0).unwrap_or(0);
        spawn_skill_object(
            &mut commands,
            &objects,
            now,
            KIND_FROST_TILE,
            owner_id,
            tile_pos,
            Quat::IDENTITY,
            Some(FROST_TILE_LIFETIME),
            None,
        );
        trace::event(
            "glacier_tile_drop",
            json!({ "pos": [tile_pos.x, tile_pos.y, tile_pos.z] }),
        );
    }
    memory.0.retain(|e, _| live.contains(e));
}
