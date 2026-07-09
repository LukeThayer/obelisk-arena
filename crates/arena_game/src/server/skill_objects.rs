//! Server-side SKILL OBJECTS — the generic persistence layer the wisp weapon ports stand on:
//! world objects spawned by skill verbs (portals, frost tiles, frost spires) with per-kind
//! instance limits (max-N, replace-oldest — wisp's portal-slot semantics generalized), lifetimes,
//! and replication. One mechanism where wisp had three bespoke networked types.
//!
//! What obelisk deliberately can't express (its windows are anonymous, per-cast, and never
//! counted across casts) lives here; the OBELISK half of each spell (volumes, damage, charge,
//! mana) stays pure content. The bridge between them is `server/verbs.rs`.

use avian3d::prelude::{Collider, Position, RigidBody, Rotation};
use bevy::prelude::*;
use lightyear::prelude::{NetworkTarget, Replicate};
use serde_json::json;

use crate::net::protocol::NetworkedSkillObject;
use crate::trace;

/// Server bookkeeping for one skill object. The replicated half is
/// [`NetworkedSkillObject`]; this carries what only the server needs.
#[derive(Component, Debug)]
pub(crate) struct SkillObject {
    pub kind: &'static str,
    /// Spawning client id (0 = none).
    pub owner: u64,
    /// `Time::elapsed_secs` at spawn — the replace-oldest ordering key.
    pub spawned_at: f32,
    /// Absolute expiry (elapsed secs); `f32::INFINITY` = lives until replaced/despawned.
    pub expires_at: f32,
}

/// Kind ids (the cross-layer contract: server behavior, client visuals, trace fields).
pub(crate) use crate::portals_shared::{KIND_PORTAL_BLUE, KIND_PORTAL_ORANGE};
pub(crate) const KIND_FROST_TILE: &str = "frost_tile";
pub(crate) const KIND_FROST_SPIRE: &str = "frost_spire";

/// Per-kind instance cap (GLOBAL, wisp semantics: portals are shared world state, one per slot;
/// tiles/spires are capped to keep long matches bounded). Exceeding the cap despawns the OLDEST.
fn max_instances(kind: &str) -> usize {
    match kind {
        KIND_PORTAL_ORANGE | KIND_PORTAL_BLUE => 1,
        KIND_FROST_TILE => 64,
        KIND_FROST_SPIRE => 12,
        _ => 32,
    }
}

/// Spawn a skill object: bookkeeping + replication + pose (+ optional physics), enforcing the
/// kind's replace-oldest cap. Returns the new entity.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_skill_object(
    commands: &mut Commands,
    existing: &Query<(Entity, &SkillObject)>,
    now: f32,
    kind: &'static str,
    owner: u64,
    position: Vec3,
    rotation: Quat,
    lifetime: Option<f32>,
    physics: Option<(RigidBody, Collider)>,
) -> Entity {
    // Replace-oldest: despawn enough of this kind that the new one fits the cap.
    let cap = max_instances(kind);
    let mut same_kind: Vec<(Entity, f32)> = existing
        .iter()
        .filter(|(_, o)| o.kind == kind)
        .map(|(e, o)| (e, o.spawned_at))
        .collect();
    if same_kind.len() + 1 > cap {
        same_kind.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (e, _) in same_kind.iter().take(same_kind.len() + 1 - cap) {
            if let Ok(mut ec) = commands.get_entity(*e) {
                ec.despawn();
            }
        }
    }

    let mut ec = commands.spawn((
        Name::new(format!("SkillObject({kind})")),
        SkillObject {
            kind,
            owner,
            spawned_at: now,
            expires_at: lifetime.map(|l| now + l).unwrap_or(f32::INFINITY),
        },
        NetworkedSkillObject {
            kind: kind.to_string(),
            owner,
        },
        Position(position),
        Rotation(rotation),
        Replicate::to_clients(NetworkTarget::All),
    ));
    if let Some((body, collider)) = physics {
        ec.insert((body, collider));
    }
    let entity = ec.id();
    trace::event(
        "skill_object_spawned",
        json!({ "object_kind": kind, "owner": owner,
                "pos": [position.x, position.y, position.z] }),
    );
    entity
}

/// Despawn expired skill objects.
pub(crate) fn reap_skill_objects(
    time: Res<Time>,
    q: Query<(Entity, &SkillObject)>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    for (e, obj) in &q {
        if now >= obj.expires_at {
            if let Ok(mut ec) = commands.get_entity(e) {
                ec.despawn();
            }
            trace::event("skill_object_expired", json!({ "object_kind": obj.kind }));
        }
    }
}
