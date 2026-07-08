//! Cast pipeline: input-stream cast edges → server-resolved `CastAim` → obelisk cast.
//!
//! Casts ride the native input (design WS2): a cast is the FALLING EDGE of `ArenaInput.charging`
//! (true→false across consecutive ticks) in the per-tick `ActionState<ArenaInput>` lightyear
//! maintains for each player. The server counts held ticks for the charge byte
//! (`net::charge_byte_from_hold_ticks`), latches the skill slot at press, reconstructs the aim ray
//! from the input's yaw+pitch (`net::aim_dir` — the same math as the client camera), resolves a
//! CANDIDATE `CastAim` from the skill's authored `Acquisition` (`resolve_cast_aim`), and inserts a
//! `PendingCast`; obelisk's `validate_casts` → `resolve_acquisition` does the AUTHORITATIVE
//! range/filter/fallback walk against that candidate and gates mana/cooldown/already-casting,
//! emitting `CastBegan` or `CastRejected`. Packet loss fills missing input ticks as
//! SameAsPrecedent, so a lost release DELAYS the edge 1-2 ticks rather than dropping the cast.

use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use obelisk_bevy::assets::Acquisition;
use obelisk_bevy::prelude::*;
use obelisk_bevy::timeline::cast::{CastAim, PendingCast};
use serde_json::json;

use crate::net::input::ArenaInput;
use crate::net::protocol::NetworkedPlayer;
use crate::trace;

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

/// Per-player cast-edge memory: last tick's `charging`, how many ticks it has been held, and the
/// skill slot latched at press. Server-only (inserted by `spawn_player_on_connect`).
#[derive(Component, Default, Debug, PartialEq)]
pub(crate) struct PrevCastInput {
    charging: bool,
    hold_ticks: u32,
    slot: u8,
}

/// Advance the per-tick edge state machine. Returns `Some((slot, charge_byte))` exactly on the
/// falling edge of `charging`. Pure — unit-testable without an app.
fn step_cast_edge(prev: &mut PrevCastInput, charging: bool, slot: u8) -> Option<(u8, u8)> {
    let fired = match (prev.charging, charging) {
        (true, false) => Some((
            prev.slot,
            crate::net::charge_byte_from_hold_ticks(prev.hold_ticks),
        )),
        _ => None,
    };
    if charging {
        if !prev.charging {
            prev.slot = slot; // latch selection at press
            prev.hold_ticks = 0;
        }
        prev.hold_ticks = prev.hold_ticks.saturating_add(1);
    } else {
        prev.hold_ticks = 0;
    }
    prev.charging = charging;
    fired
}

/// FixedUpdate: detect each player's cast edge from its per-tick `ActionState<ArenaInput>` and
/// insert a `PendingCast` (obelisk validates + executes). Runs `.before(ObeliskSet::Validate)` so
/// the cast lands the SAME tick as its input edge. Skips a caster already mid-`ActiveCast`
/// (obelisk would reject; skipping keeps the edge state clean).
///
/// Fires from the caster's EYE (`origin + Y*ARENA_EYE_HEIGHT`), the same height the client camera
/// sits at, so the bolt travels along the crosshair ray. The charge byte's gameplay meaning:
/// `charge_mult(c) = 0.5 + (c/255)*1.5` — tap ≈ 1.0×, full 1.5s hold = 2.0×.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn detect_cast_edges(
    mut players: Query<
        (
            Entity,
            &ObeliskId,
            &ActionState<ArenaInput>,
            &mut PrevCastInput,
            &crate::net::protocol::EquippedWeapon,
        ),
        With<NetworkedPlayer>,
    >,
    // Filter-only twin of `players` for the closures' "is a networked player body" check (no
    // component access → no borrow conflict with the mutable iteration).
    player_bodies: Query<(), With<NetworkedPlayer>>,
    active: Query<(), With<ActiveCast>>,
    handles: Res<CastTimelineHandles>,
    timelines: Res<Assets<CastTimeline>>,
    transforms: Query<&bevy::prelude::Transform>,
    hurtboxes: Query<(Entity, &Hurtbox)>,
    spatial: avian3d::prelude::SpatialQuery,
    mut commands: Commands,
) {
    for (caster, caster_id, action, mut prev, weapon) in &mut players {
        let Some((slot, charge)) =
            step_cast_edge(&mut prev, action.0.charging, action.0.skill_slot)
        else {
            continue;
        };
        if active.get(caster).is_ok() {
            continue; // mid-cast; obelisk would reject anyway
        }
        // The slot indexes the EQUIPPED WEAPON's skill list (protocol v4) — this lookup IS the
        // "can't cast an unequipped skill" gate: a stale/bad slot simply resolves to no skill.
        let Some(skill_id) = weapon.skills.get(slot as usize).cloned() else {
            continue; // out-of-range slot (weapon swap raced the input, or a bad client): ignore
        };
        // Aim ray from the input's yaw+pitch — the identical math the client camera + predicted
        // cast use (`net::aim_dir`). Degenerate never happens (aim_dir normalizes), but keep the
        // -Z fallback for safety.
        let dir = Dir3::new(crate::net::aim_dir(action.0.yaw, action.0.pitch)).unwrap_or(Dir3::NEG_Z);
        trace::event(
            "cast_edge",
            json!({ "caster": caster_id.0, "skill_id": skill_id, "charge": charge }),
        );
        let muzzle_offset = Vec3::Y * crate::net::ARENA_EYE_HEIGHT;

        // AIM ACQUISITION: pick the candidate `CastAim` shape from the skill timeline's authored
        // `Acquisition`. We do NOT pre-validate range/filter here — obelisk's `validate_casts`
        // does the real range/filter/LOS check against whatever candidate we hand it, and walks
        // the authored `fallback` chain on a miss. If the timeline isn't loaded yet, default to
        // `Acquisition::Aim` (fire by direction).
        let acq = timelines
            .get(handles.0.get(&skill_id).map(|h| h.id()).unwrap_or_default())
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
            // (a networked player entity) — accept both.
            hurtboxes
                .get(hit.entity)
                .map(|(_, h)| h.owner)
                .ok()
                .or_else(|| player_bodies.contains(hit.entity).then_some(hit.entity))
                .map(RayHit::Entity)
        };
        // GROUND ACQUISITION: first world hit along the aim (excluding the caster's own
        // colliders) within `range` → point aim (blizzard). A horizontal shot that hits nothing
        // falls through to `Direction`, which the sim's authored fallback handles.
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
            json!({ "caster": caster_id.0, "skill_id": skill_id, "aim": aim_shape }),
        );
        commands.entity(caster).insert(PendingCast {
            skill_id,
            aim,
            charge: Some(charge),
            muzzle_offset,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{Dir3, Entity, Vec3};
    use obelisk_bevy::assets::{AcqFallback, Acquisition, HitFilter};

    /// The falling-edge detector: a cast fires exactly when charging goes true→false, with the
    /// charge byte derived from the number of held ticks.
    #[test]
    fn cast_edge_fires_on_release_with_held_charge() {
        let mut prev = PrevCastInput::default();
        // idle → no cast
        assert_eq!(step_cast_edge(&mut prev, false, 0), None);
        // press + hold 3 ticks → no cast while held
        assert_eq!(step_cast_edge(&mut prev, true, 0), None);
        assert_eq!(step_cast_edge(&mut prev, true, 0), None);
        assert_eq!(step_cast_edge(&mut prev, true, 0), None);
        // release → cast with 3 held ticks' charge
        let fired = step_cast_edge(&mut prev, false, 0);
        assert_eq!(fired, Some((0, crate::net::charge_byte_from_hold_ticks(3))));
        // staying released → nothing
        assert_eq!(step_cast_edge(&mut prev, false, 0), None);
    }

    /// The slot recorded at release is the slot held during the charge (latched at press),
    /// so a selection change mid-charge can't retarget an in-flight charge.
    #[test]
    fn cast_edge_latches_slot_at_press() {
        let mut prev = PrevCastInput::default();
        assert_eq!(step_cast_edge(&mut prev, true, 2), None);
        let fired = step_cast_edge(&mut prev, false, 1);
        assert_eq!(fired, Some((2, crate::net::charge_byte_from_hold_ticks(1))));
    }

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
