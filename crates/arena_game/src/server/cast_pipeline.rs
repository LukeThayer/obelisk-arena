//! Cast pipeline: client cast_request → server fire along aim_dir.
//!
//! The client sends a `CastRequestMessage` on the reliable `CastChannel` (it NEVER validates or
//! resolves — Stage A). The server maps the sender's `RemoteId` → caster entity via the
//! `ClientPlayerMap` and fires along the client's `aim_dir` (camera forward, full 3D) via
//! `cast_skill_dir` — free aim, no auto-acquire. obelisk's `validate_casts` (FixedUpdate) gates
//! mana/cooldown/already-casting and emits `CastBegan` or `CastRejected`. The projectile can miss
//! if the client was not aimed at the target — this is intentional (free-aim design).

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{MessageReceiver, RemoteId};
use obelisk_bevy::assets::CastTargeting;
use obelisk_bevy::prelude::*;
use obelisk_bevy::timeline::cast::{CastAim, PendingCast};
use serde_json::json;

use crate::net::protocol::{CastRequestMessage, NetworkedPlayer};
use crate::trace;

use super::spawn::{peer_to_u64, ClientPlayerMap};

/// Drain `CastRequestMessage`s from each connected client and fire along the client's aim direction.
///
/// Fires the caster's skill via `cast_skill_dir` with the `aim_dir` from the message (the client's
/// camera forward vector). No server-side target re-acquisition — the bolt goes where the client
/// aimed (free aim). Skips a caster already mid-cast (`AlreadyCasting` avoidance). The caster
/// entity must exist in the `ClientPlayerMap`; otherwise the request is silently dropped.
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

            // HITSCAN ACQUISITION for entity-targeted skills: when the skill's timeline declares
            // `CastTargeting::SingleEntity`, the server resolves "what is the client looking at"
            // — a ray along the aim from the eye, first hurtbox hit (excluding the caster's own)
            // within the targeting range → entity-aimed cast (obelisk then validates range/LOS
            // and the beam window strikes that entity). A MISS still casts by direction: the
            // beam window has no designated target, so the skill fizzles AFTER paying mana +
            // cooldown — the authored miss cost.
            let acquired = timelines
                .get(handles.0.get(&req.skill_id).map(|h| h.id()).unwrap_or_default())
                .and_then(|tl| match tl.targeting {
                    CastTargeting::SingleEntity { range } => Some(range),
                    _ => None,
                })
                .and_then(|range| {
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
                });
            if let Some(target) = acquired {
                trace::event(
                    "cast_hitscan_acquired",
                    json!({ "caster": caster_id.0, "skill_id": req.skill_id }),
                );
                commands.entity(caster).insert(PendingCast {
                    skill_id: req.skill_id.clone(),
                    aim: CastAim::Entity(target),
                    charge: Some(req.charge),
                    muzzle_offset,
                });
            } else {
                commands.entity(caster).cast_skill_dir_charged_from(
                    req.skill_id.clone(),
                    dir,
                    req.charge,
                    muzzle_offset,
                );
            }
        }
    }
}
