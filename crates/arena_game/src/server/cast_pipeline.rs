//! Cast pipeline: client cast_request → server-resolved `CastAim` → obelisk cast.
//!
//! The client sends a `CastRequestMessage` (camera-forward `aim_dir` + charge) on the reliable
//! `CastChannel`; it NEVER validates or resolves (Stage A). The server maps the sender's `RemoteId`
//! → caster via `ClientPlayerMap`, then resolves a CANDIDATE `CastAim` from the skill timeline's
//! authored `Acquisition` (`resolve_cast_aim`): a `HitscanEntity` skill raycasts the aim ray for a
//! target entity, a `GroundPoint` skill raycasts for a ground point, and `Aim`/`SelfPoint` cast by
//! direction. It inserts a `PendingCast`; obelisk's `validate_casts` → `resolve_acquisition`
//! (FixedUpdate) does the AUTHORITATIVE range/filter/fallback walk against that candidate and gates
//! mana/cooldown/already-casting, emitting `CastBegan` or `CastRejected`. A direction cast that hits
//! nothing lets the authored fallback fizzle — intentional.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{MessageReceiver, RemoteId};
use obelisk_bevy::assets::Acquisition;
use obelisk_bevy::prelude::*;
use obelisk_bevy::timeline::cast::{CastAim, PendingCast};
use serde_json::json;

use crate::net::protocol::{CastRequestMessage, NetworkedPlayer};
use crate::trace;

use super::spawn::{peer_to_u64, ClientPlayerMap};

/// What a host raycast along the aim ray can return.
enum RayHit {
    Entity(Entity),
}

/// Pick the candidate `CastAim` shape from the timeline's authored `Acquisition`, using host
/// raycast closures. The sim (`validate_casts`) does the real range/filter/LOS + fallback walk
/// against this candidate — we only choose which shape to attempt:
///   HitscanEntity -> raycast for an entity in range; hit => Entity, miss => Direction.
///   GroundPoint   -> raycast to the ground; hit => Point, miss => Direction.
///   Aim/SelfPoint -> Direction (never fails; SelfPoint's cast point is produced by the sim).
fn resolve_cast_aim(
    acq: &Acquisition,
    dir: Dir3,
    mut cast_entity: impl FnMut(f32) -> Option<RayHit>,
    mut cast_ground: impl FnMut(f32) -> Option<Vec3>,
) -> CastAim {
    match acq {
        Acquisition::HitscanEntity { range, .. } => match cast_entity(*range) {
            Some(RayHit::Entity(e)) => CastAim::Entity(e),
            None => CastAim::Direction(dir),
        },
        Acquisition::GroundPoint { range, .. } => match cast_ground(*range) {
            Some(p) => CastAim::Point(p),
            None => CastAim::Direction(dir),
        },
        Acquisition::Aim | Acquisition::SelfPoint => CastAim::Direction(dir),
    }
}

/// Drain `CastRequestMessage`s from each connected client and cast the caster's skill.
///
/// Resolves a candidate `CastAim` from the skill timeline's authored `Acquisition` via
/// [`resolve_cast_aim`] (host raycasts along the client's `aim_dir` for `HitscanEntity`/`GroundPoint`;
/// direction otherwise), then inserts a `PendingCast` for obelisk's `validate_casts` to check + gate.
/// Skips a caster already mid-cast (`AlreadyCasting` avoidance). The caster entity must exist in the
/// `ClientPlayerMap`; otherwise the request is silently dropped.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_cast_requests(
    mut receivers: Query<(&RemoteId, &mut MessageReceiver<CastRequestMessage>), With<ClientOf>>,
    client_map: Res<ClientPlayerMap>,
    casters: Query<&ObeliskId, With<NetworkedPlayer>>,
    active: Query<(), With<ActiveCast>>,
    handles: Res<CastTimelineHandles>,
    timelines: Res<Assets<CastTimeline>>,
    transforms: Query<&bevy::prelude::Transform>,
    hurtboxes: Query<(Entity, &Hurtbox)>,
    spatial: avian3d::prelude::SpatialQuery,
    mut commands: Commands,
) {
    for (RemoteId(peer_id), mut receiver) in &mut receivers {
        let Some(client_id) = peer_to_u64(peer_id) else {
            continue;
        };
        for req in receiver.receive() {
            let Some(&caster) = client_map.0.get(&client_id) else {
                continue;
            };
            if active.get(caster).is_ok() {
                // Already casting; obelisk would reject. Drop silently.
                continue;
            }
            let Ok(caster_id) = casters.get(caster) else {
                continue;
            };
            // Fire along the client's camera-forward direction. Fall back to -Z (straight forward)
            // if the vector is degenerate (shouldn't happen from a well-formed client).
            let dir = Dir3::new(Vec3::from(req.aim_dir)).unwrap_or(Dir3::NEG_Z);
            trace::event(
                "cast_request_accepted",
                json!({ "caster": caster_id.0, "skill_id": req.skill_id,
                        "aim_dir": req.aim_dir, "charge": req.charge }),
            );
            // Use the charged variant; the byte's gameplay meaning is documented client-side by
            // `client::net::charge_mult` (`0.5 + (c/255)*1.5`) and produced by `charge_byte_from_frac`:
            // charge=85 (`TAP_CHARGE_BYTE`) ≈ 1.0× (instant tap), charge=255 = 2.0× (full hold).
            // `u8` is inherently bounded [0, 255] — no extra clamp needed.
            //
            // Fire from the caster's EYE (`origin + Y*ARENA_EYE_HEIGHT`), the same height the client
            // camera sits at. The client's `aim_dir` is the camera-forward ray FROM that eye, so a
            // muzzle at the eye makes the bolt travel along the crosshair ray — a shot aimed at the
            // opponent lands (Bug 1). Without the offset the bolt spawns at the feet (Y=1.0) and
            // undershoots a crosshair-aimed shot.
            let muzzle_offset = Vec3::Y * crate::net::ARENA_EYE_HEIGHT;

            // AIM ACQUISITION: pick the candidate `CastAim` shape from the skill timeline's
            // authored `Acquisition`. We do NOT pre-validate range/filter here — obelisk's
            // `validate_casts` does the real range/filter/LOS check against whatever candidate we
            // hand it, and walks the authored `fallback` chain on a miss (a `Direction` fallback
            // either fizzles or casts anyway, per the skill's authoring). If the timeline isn't
            // loaded yet, default to `Acquisition::Aim` (fire by direction).
            let acq = timelines
                .get(handles.0.get(&req.skill_id).map(|h| h.id()).unwrap_or_default())
                .map(|tl| tl.acquisition.clone())
                .unwrap_or(Acquisition::Aim);

            // HITSCAN ACQUISITION: a ray along the aim from the eye, first hurtbox/body hit
            // (excluding the caster's own) within `range` → entity aim. A miss falls through to
            // `Direction` inside `resolve_cast_aim`.
            let cast_entity = |range: f32| -> Option<RayHit> {
                let origin = transforms.get(caster).ok()?.translation + muzzle_offset;
                let own: Vec<Entity> = hurtboxes
                    .iter()
                    .filter(|(_, h)| h.owner == caster)
                    .map(|(e, _)| e)
                    .chain([caster])
                    .collect();
                let filter =
                    avian3d::prelude::SpatialQueryFilter::default().with_excluded_entities(own);
                let hit = spatial.cast_ray(origin, dir, range, true, &filter)?;
                // The ray can meet the target's HURTBOX child (owner) or its BODY collider
                // (a combatant entity) — accept both.
                hurtboxes
                    .get(hit.entity)
                    .map(|(_, h)| h.owner)
                    .ok()
                    .or_else(|| casters.get(hit.entity).is_ok().then_some(hit.entity))
                    .map(RayHit::Entity)
            };
            // GROUND ACQUISITION: first world hit along the aim (excluding the caster's own
            // colliders) within `range` → point aim. Blizzard (C6) is its first real consumer; a
            // horizontal shot that hits nothing falls through to `Direction`, which the sim's
            // authored fallback handles.
            let cast_ground = |range: f32| -> Option<Vec3> {
                let origin = transforms.get(caster).ok()?.translation + muzzle_offset;
                let own: Vec<Entity> = hurtboxes
                    .iter()
                    .filter(|(_, h)| h.owner == caster)
                    .map(|(e, _)| e)
                    .chain([caster])
                    .collect();
                let filter =
                    avian3d::prelude::SpatialQueryFilter::default().with_excluded_entities(own);
                let hit = spatial.cast_ray(origin, dir, range, true, &filter)?;
                Some(origin + *dir * hit.distance)
            };

            let aim = resolve_cast_aim(&acq, dir, cast_entity, cast_ground);
            // Compute the trace label before `insert` moves `aim`.
            let aim_shape = match &aim {
                CastAim::Entity(_) => "Entity",
                CastAim::Point(_) => "Point",
                CastAim::Direction(_) => "Direction",
            };
            trace::event(
                "cast_acquired",
                json!({ "caster": caster_id.0, "skill_id": req.skill_id, "aim": aim_shape }),
            );
            commands.entity(caster).insert(PendingCast {
                skill_id: req.skill_id.clone(),
                aim,
                charge: Some(req.charge),
                muzzle_offset,
            });
        }
    }
}

#[cfg(test)]
mod acq_tests {
    use super::*;
    use bevy::prelude::{Dir3, Entity, Vec3};
    use obelisk_bevy::assets::{AcqFallback, Acquisition, HitFilter};

    #[test]
    fn hitscan_entity_hit_yields_entity_aim() {
        let acq = Acquisition::HitscanEntity {
            range: 15.0,
            filter: HitFilter::Enemies,
            fallback: AcqFallback::Fizzle,
        };
        let e = Entity::from_raw_u32(7).unwrap();
        let aim = resolve_cast_aim(&acq, Dir3::NEG_Z, |_range| Some(RayHit::Entity(e)), |_r| None);
        assert!(matches!(aim, CastAim::Entity(x) if x == e));
    }
    #[test]
    fn hitscan_miss_falls_through_to_direction() {
        let acq = Acquisition::HitscanEntity {
            range: 15.0,
            filter: HitFilter::Enemies,
            fallback: AcqFallback::Fizzle,
        };
        let aim = resolve_cast_aim(&acq, Dir3::NEG_Z, |_r| None, |_r| None);
        assert!(matches!(aim, CastAim::Direction(_)));
    }
    #[test]
    fn ground_point_hit_yields_point_aim() {
        let acq = Acquisition::GroundPoint {
            range: 20.0,
            fallback: AcqFallback::Fizzle,
        };
        let p = Vec3::new(1.0, 0.0, 2.0);
        let aim = resolve_cast_aim(&acq, Dir3::NEG_Z, |_r| None, |_r| Some(p));
        assert!(matches!(aim, CastAim::Point(x) if x == p));
    }
    #[test]
    fn aim_and_selfpoint_yield_direction() {
        for acq in [Acquisition::Aim, Acquisition::SelfPoint] {
            let aim = resolve_cast_aim(&acq, Dir3::NEG_Z, |_r| None, |_r| None);
            assert!(matches!(aim, CastAim::Direction(_)));
        }
    }
}
