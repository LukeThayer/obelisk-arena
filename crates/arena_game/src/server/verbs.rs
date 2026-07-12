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
//! | `frost_spire`            | `on_window_spike`        | erupt a ground-snapped spire at the cue position |
//! | `rolling_glacier`        | `on_window_flight`       | spawn the ONE avian Dynamic boulder + pin the flight hitbox to it |
//! | `glacier_roll`           | `on_window_roll`         | RE-PIN that boulder to the roll hitbox (no new ball) |
//! | `glacier_roll`           | `on_end_roll`            | despawn the boulder at the roll's fuse |
//!
//! Everything spawned here is a [`super::skill_objects`] object (limits/lifetime/replication).

use avian3d::prelude::{AngularVelocity, Collider, LinearVelocity, Position, RigidBody};
use bevy::prelude::*;
use lightyear::prelude::ComponentReplicationOverrides;
use obelisk_bevy::events::{CueEvent, CueKind, HitboxWorldHit};
use obelisk_bevy::prelude::charge_mult;
use obelisk_bevy::spatial::Hitbox;
use serde_json::json;

use crate::trace;

use super::glacier_ball::{
    ball_physics_bundle, ball_spawn_pos, flat_launch, BallPhase, PinnedBall, PinnedHitbox,
    GLACIER_BALL_LIFETIME, GLACIER_BALL_RADIUS, GLACIER_BALL_THROW_SPEED,
};
use super::skill_objects::{
    spawn_skill_object, SkillObject, KIND_FROST_SPIRE, KIND_GLACIER_BALL, KIND_PORTAL_BLUE,
    KIND_PORTAL_ORANGE,
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
/// frost_spire's damage window authors `anchor: CastPoint, anchor_offset: (0.0, 0.8, 0.0)`, so
/// obelisk's `on_window_spike` cue fires 0.8 m ABOVE the cast point (which is itself the patch
/// PAINT height, not the floor). The ray-miss fallback strips this authored lift; see
/// [`spire_eruption_anchor`].
const WINDOW_ANCHOR_OFFSET_Y: f32 = 0.8;

// --- Rolling-glacier boulder VERB tuning. THE AUTHORITY FLIP: the ball is a REAL avian Dynamic
// body now (wisp physics + lifetime in `server/glacier_ball.rs`); it owns its own trajectory and
// obelisk's (Static) windows are PINNED to it. The flight verb spawns it at cast, the roll verb
// RE-PINS it, the end verb despawns it. Only the cue-correlation radii live here. ---

/// Correlate a just-spawned obelisk hitbox to its OPEN cue: the hitbox is AT the cue (spawn)
/// position, so a live hitbox for this caster+skill within this radius is it — the flight hitbox at
/// the casting hand, the roll hitbox at the landing point.
const GLACIER_BALL_CORRELATE_RADIUS: f32 = 1.0;
/// Re-pin (roll open) / despawn (roll end) the boulder nearest the cue position within this radius.
/// Slightly larger than the correlate radius to absorb the tick or two the ball rolls between the
/// landing world-hit and the roll cue firing (lockstep travel ⇒ it stands near the point).
const GLACIER_BALL_DESPAWN_RADIUS: f32 = 1.5;

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

/// Ground-snap the frost-spire eruption anchor. PURE (the [`skill_verbs_on_cue`] I/O half resolves
/// `ground_hit` from a downward `SpatialQuery` ray); split out so this — the seat of the Task-4
/// float-above-the-floor regression — is unit-testable without the lightyear/obelisk server the
/// verb observer's spawn path needs.
///
/// obelisk's `on_surface` acquisition already snapped the cue's XZ to the consumed patch's center,
/// so KEEP it; only the Y is wrong — the cue fires at the damage window's HITBOX transform, which
/// carries the authored `anchor_offset` (+0.8) on top of the patch's paint height, NOT the floor.
/// `ground_hit == Some(y)` (ray hit) seats the spire base flush with the ground; `None` (ray miss)
/// strips the authored window offset as a best effort (leaving the patch paint height).
pub(crate) fn spire_eruption_anchor(cue_pos: Vec3, ground_hit: Option<f32>) -> Vec3 {
    let y = ground_hit.unwrap_or(cue_pos.y - WINDOW_ANCHOR_OFFSET_Y);
    Vec3::new(cue_pos.x, y, cue_pos.z)
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
    players: Query<Entity, With<crate::net::protocol::NetworkedPlayer>>,
    hitboxes: Query<(Entity, &Hitbox, &Transform)>,
    actions: Query<&lightyear::prelude::input::native::ActionState<crate::net::input::ArenaInput>>,
    mut commands: Commands,
) {
    let ev = cue.event();
    // Window-OPEN cues drive the place / erupt / spawn verbs; the glacier boulder additionally
    // needs the roll's window-END cue (`on_end_roll`, `CueKind::OnEnd`) to despawn at the wall or
    // fuse. Every other cue kind (cast / hit / emit) is presentation-only here — ignore it.
    if !matches!(ev.kind, CueKind::OnWindow | CueKind::OnEnd) {
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

        // --- Frost spire: erupt at the cue position, GROUND-SNAPPED. obelisk's on_surface
        // acquisition (snap: true, consume: true) already gated the cast, snapped the cast
        // point's XZ to the consumed frost patch's center, and spent the fuel at accept — so
        // `ev.position` carries the right XZ. But this cue fires at the damage window's HITBOX
        // transform, and frost_spire's window authors `anchor: CastPoint, anchor_offset: +0.8Y`
        // ON TOP OF the patch's own paint height (~0.35) — so `ev.position.y` floats ~1 m above
        // the floor. Raycast DOWN (the deleted tile-drop poller's exact pattern) so the physical
        // spire's base sits FLUSH with the ground (the detached-from-its-damage-capsule
        // regression); fall back to stripping the authored window offset if the ray misses.
        //
        // The ray wants LEVEL geometry ONLY (floor / settled spire-terrain) — the spire is AIMED
        // at enemies, so a combatant standing on the fuel patch at cast time is directly under it
        // and would be the nearest hit, floating the VISUAL spire on the body instead of seating
        // it on the floor (the damage capsule stays CastPoint-anchored, so only the cosmetic
        // detaches). So EXCLUDE the caster + its child hurtbox and every skill object (built like
        // the portal arm's `exclude`), PLUS every combatant body and its hurtbox children (the
        // material addition over that poller's default filter). With nobody on the patch the floor
        // is still the first hit — identical to before. (The glacier net-test gate proves the
        // GROUND-FLUSH eruption e2e; the combatant-exclusion path itself is covered by inspection
        // only — no scripted scenario places a body under an eruption.)
        ("frost_spire", "on_window_spike") => {
            // Exclude the caster + its child hurtbox and every skill object (the portal arm's
            // set), then ADD every combatant body + its hurtbox children — so the ground ray only
            // ever hits LEVEL geometry, never a body on the fuel patch.
            let mut exclude = vec![ev.source];
            if let Ok(kids) = children.get(ev.source) {
                exclude.extend(kids.iter());
            }
            exclude.extend(objects.iter().map(|(e, _)| e));
            for player in &players {
                exclude.push(player);
                if let Ok(kids) = children.get(player) {
                    exclude.extend(kids.iter());
                }
            }
            // Resolve the ground Y under the cue: cast down from just ABOVE the cue (origin lifted
            // 0.2 so a spire erupting on a step/another spire still finds the surface), converting
            // the hit distance back to a world Y. `None` on a miss → the fallback in the helper.
            let ground_hit = spatial
                .cast_ray(
                    ev.position + Vec3::Y * 0.2,
                    Dir3::NEG_Y,
                    4.0,
                    true,
                    &avian3d::prelude::SpatialQueryFilter::default()
                        .with_excluded_entities(exclude),
                )
                .map(|hit| ev.position.y + 0.2 - hit.distance);
            let anchor = spire_eruption_anchor(ev.position, ground_hit);
            // Erupt: kinematic riser buried one height below the anchor, rising at height/0.15
            // (wisp's exact emergence); `settle_spires` freezes it into Static terrain — which
            // our raycast grounding and wall-aware projectiles then treat as real level geometry
            // (bolts burst on spires for free).
            let rest = anchor + Vec3::Y * (SPIRE_HEIGHT * 0.5 - 0.08);
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
                        "pos": [anchor.x, anchor.y, anchor.z] }),
            );
        }

        // --- Rolling glacier: the FLIGHT window OPENED (at the casting hand) — spawn the ONE real
        // avian Dynamic boulder (wisp physics, server/glacier_ball.rs) and PIN this flight hitbox to
        // it. The ball owns its trajectory: launched FLAT at 9·charge along the aim (wisp `up: 0.0` —
        // the hand height drops it, NOT the pitched eye ray). obelisk keeps damage/chain through the
        // pinned Static hitbox; on the ball's first ground contact the landing detector fires the
        // world-hit → obelisk chains glacier_roll. The `skill_object_spawned{glacier_ball}` trace
        // (gate assertion 10) rides `spawn_skill_object`.
        ("rolling_glacier", "on_window_flight") => {
            // Correlate the flight hitbox obelisk just spawned AT the hand for this cast, and read
            // its aim. No hitbox ⇒ nothing to pin — skip (rare; a dropped window cue).
            let Some((_, flight_hitbox, aim)) = hitboxes
                .iter()
                .filter(|(_, hb, _)| hb.caster == ev.source && hb.skill_id == ev.skill_id)
                .filter_map(|(e, hb, tf)| {
                    let d = tf.translation.distance(ev.position);
                    (d <= GLACIER_BALL_CORRELATE_RADIUS).then_some((d, e, hb.aim))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
            else {
                trace::event("glacier_ball_no_flight_hitbox", json!({ "owner": owner_id }));
                return;
            };
            // Flat launch heading: the flight hitbox's aim flattened to XZ (fallback: the caster's
            // live look flattened, then +X).
            let dir = flat_launch(aim)
                .or_else(|| {
                    let look = actions
                        .get(ev.source)
                        .map(|a| crate::net::aim_dir(a.0.yaw, a.0.pitch))
                        .unwrap_or(Vec3::NEG_Z);
                    flat_launch(look)
                })
                .unwrap_or(Vec3::X);
            // charge_mult(ev.charge): a held lob throws the ball faster/further (wisp).
            let launch = dir * GLACIER_BALL_THROW_SPEED * charge_mult(ev.charge);
            // Spawn nudged forward of the hand so the mass-6 sphere clears the caster capsule
            // instead of shoving the caster at spawn.
            let spawn_pos = ball_spawn_pos(ev.position, dir);
            let ball = spawn_skill_object(
                &mut commands,
                &objects,
                now,
                KIND_GLACIER_BALL,
                owner_id,
                spawn_pos,
                Quat::IDENTITY,
                Some(GLACIER_BALL_LIFETIME),
                Some((RigidBody::Dynamic, Collider::sphere(GLACIER_BALL_RADIUS))),
            );
            commands.entity(ball).insert((
                ball_physics_bundle(launch, flight_hitbox),
                // POSE-ONLY replication (THE SINK FIX): the ball's avian Position/Rotation ride the
                // wire, but its LinearVelocity/AngularVelocity are per-entity EXCLUDED here. The client
                // mirror is a `RigidBody::Kinematic` copy (glacier_ball.rs::client_ball_mirror_bundle),
                // and avian INTEGRATES a kinematic body's LinearVelocity into Position every tick with
                // NO client gravity/contacts to oppose it — so the rolling ball's velocity replicated
                // straight into the mirror, driving it under the floor between 30Hz Position snapshots
                // ("sinks over 3-4s then pops"). Disabling velocity replication for THIS entity ONLY
                // (players keep it — their prediction depends on it) makes the mirror follow the
                // replicated Position alone. lightyear's per-entity override
                // (`ComponentReplicationOverrides<C>::disable_all()` — the same mechanism lightyear uses
                // internally to keep `Controlled` off the wire); portals/spires are untouched.
                ComponentReplicationOverrides::<LinearVelocity>::default().disable_all(),
                ComponentReplicationOverrides::<AngularVelocity>::default().disable_all(),
            ));
            // Back-link the flight hitbox to its ball (mirrors the `PinnedBall` forward-link in the
            // bundle) so `end_orphaned_glacier_hitboxes` can gracefully burst this window if the
            // ball is evicted mid-flight before it ever lands.
            commands.entity(flight_hitbox).insert(PinnedHitbox { ball });
        }

        // --- Rolling glacier: the roll window OPENED (obelisk chained it at the landing world-hit,
        // spawning the roll hitbox AT the landing point). RE-PIN the existing ball (the one that
        // flew here) to this new roll hitbox — NO new ball — so the pin drags the roll hitbox
        // through the ball's REAL rolling path (frost Trail + contact damage continue on the far
        // side of bank shots). The roll's 6.5 s fuse (`on_end_roll`) is the only end (wisp parity).
        ("glacier_roll", "on_window_roll") => {
            // The roll hitbox: this caster's glacier_roll hitbox nearest the cue (spawn = landing).
            let Some(roll_hitbox) = hitboxes
                .iter()
                .filter(|(_, hb, _)| hb.caster == ev.source && hb.skill_id == ev.skill_id)
                .filter_map(|(e, _, tf)| {
                    let d = tf.translation.distance(ev.position);
                    (d <= GLACIER_BALL_CORRELATE_RADIUS).then_some((d, e))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, e)| e)
            else {
                trace::event("glacier_roll_no_hitbox", json!({ "owner": owner_id }));
                return;
            };
            // The ball to re-pin: this owner's glacier_ball nearest the landing cue.
            match objects
                .iter()
                .filter(|(_, o)| o.kind == KIND_GLACIER_BALL && o.owner == owner_id)
                .filter_map(|(e, _)| {
                    let d = positions.get(e).ok()?.0.distance(ev.position);
                    (d <= GLACIER_BALL_DESPAWN_RADIUS).then_some((d, e))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, e)| e)
            {
                Some(ball) => {
                    commands.entity(ball).insert(PinnedBall {
                        hitbox: roll_hitbox,
                        phase: BallPhase::Rolling,
                    });
                    // Back-link the roll hitbox to the ball (mirrors the PinnedBall forward-link)
                    // so the watchdog can burst this window if the ball is later evicted mid-roll.
                    commands.entity(roll_hitbox).insert(PinnedHitbox { ball });
                }
                None => {
                    // No ball to drag this fresh roll hitbox — the flight ball was evicted before it
                    // landed, so the chain fired but its boulder is gone. Leaving the roll hitbox
                    // unpinned would freeze it as a phantom damage sphere (Static, no mover) until
                    // its own 6.5 s fuse — the exact orphan the watchdog closes. End it NOW via the
                    // landing detector's world-hit at the chain-landing cue: obelisk ends it
                    // HitWorld → on_impact → glacier_burst (a graceful burst where the chain landed).
                    trace::event("glacier_roll_no_ball", json!({ "owner": owner_id }));
                    commands.trigger(HitboxWorldHit {
                        hitbox: roll_hitbox,
                        position: ev.position,
                    });
                }
            }
        }

        // --- Rolling glacier: the roll ENDED (wall `on_impact` / 6.5 s `on_expire`). obelisk fires
        // `on_end_roll` AT the stop position; stop the boulder THERE by despawning this caster's
        // nearest glacier_ball (lockstep travel ⇒ it stands on the end point). None in range
        // (already evicted by the cap, or reaped by its fuse) = a clean no-op.
        ("glacier_roll", "on_end_roll") => {
            let victim = objects
                .iter()
                .filter(|(_, o)| o.kind == KIND_GLACIER_BALL && o.owner == owner_id)
                .filter_map(|(e, _)| {
                    let d = positions.get(e).ok()?.0.distance(ev.position);
                    (d <= GLACIER_BALL_DESPAWN_RADIUS).then_some((d, e))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, e)| e);
            if let Some(e) = victim {
                if let Ok(mut ec) = commands.get_entity(e) {
                    ec.despawn();
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The cue the migrated verb mis-anchored: the consumed patch's center (2, _, 3) at paint
    /// height ~0.35, lifted by the damage window's authored `anchor_offset` +0.8 → y = 1.15. The
    /// eruption must NOT use this Y.
    const CUE: Vec3 = Vec3::new(2.0, 1.15, 3.0);

    #[test]
    fn ray_hit_snaps_to_the_ground_and_keeps_the_patch_xz() {
        // Floor found at y=0: anchor at the ground, discarding the hitbox-height Y entirely. XZ
        // (obelisk-snapped to the patch center) is preserved.
        assert_eq!(spire_eruption_anchor(CUE, Some(0.0)), Vec3::new(2.0, 0.0, 3.0));
    }

    #[test]
    fn ray_hit_tracks_an_arbitrary_ground_height() {
        // Uneven floor / spire-on-a-step: the anchor Y follows the ray hit exactly, XZ untouched.
        assert_eq!(
            spire_eruption_anchor(Vec3::new(-1.0, 2.4, 5.0), Some(0.7)),
            Vec3::new(-1.0, 0.7, 5.0),
        );
    }

    #[test]
    fn ray_miss_strips_the_authored_window_offset() {
        // No collider under the cue: strip only the 0.8 window offset (leaving the patch paint
        // height ~0.35) — NEVER anchor at the full hitbox height, and never move the XZ.
        let anchor = spire_eruption_anchor(CUE, None);
        assert!((anchor.y - 0.35).abs() < 1e-6, "1.15 - 0.8 = 0.35, got {}", anchor.y);
        assert_eq!((anchor.x, anchor.z), (2.0, 3.0));
    }

    #[test]
    fn ground_snap_defeats_the_float_above_the_floor_regression() {
        // Pin the exact defect: the settle/rest height is `anchor.y + (H/2 - 0.08)`. Ground-snapped
        // → 0.72 (base flush with the floor); anchoring at the cue position (the migrated bug) →
        // 1.87, floating the whole ~1 m spire above its damage capsule.
        let rest = |anchor_y: f32| anchor_y + (SPIRE_HEIGHT * 0.5 - 0.08);
        let snapped = rest(spire_eruption_anchor(CUE, Some(0.0)).y);
        let bug = rest(CUE.y); // what the pre-fix `ev.position`-anchored code produced
        assert!((snapped - 0.72).abs() < 1e-6, "ground-snapped rest ≈ 0.72, got {snapped}");
        assert!(
            bug - snapped > 1.0,
            "the regression floats the spire ~1 m up (bug rest {bug} vs snapped {snapped})",
        );
    }
}
