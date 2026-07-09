//! Per-rig bone-socket index (the editor's `skill::preview::sockets` pattern, per-player): scans
//! `Name`d descendants of each [`ArenaBody`] rig as the glb scene streams in, so the cue layers
//! can anchor effects to named joints (`CueAttach::Bone` — charge sparks on `hand_R`, a muzzle
//! flash at the palm). Resolution is best-effort by contract: an unknown socket falls back to
//! the rig root, and a player with no rig yet falls back to the player entity itself — authoring
//! against a missing bone never panics. Windowed-only (registered with the present layer).

use bevy::prelude::*;
use std::collections::HashMap;

use super::rig::ArenaBody;

/// Name → bone entity for one rig. Lives ON the [`ArenaBody`] scene root (inserted by
/// [`index_rig_sockets`] on first sighting).
#[derive(Component, Default)]
pub struct RigSockets {
    pub by_name: HashMap<String, Entity>,
}

/// Record newly-named entities under an [`ArenaBody`] into that body's [`RigSockets`]. First
/// name wins (glb joint names are unique in practice; a duplicate keeps the earlier entity).
pub fn index_rig_sockets(
    new_names: Query<(Entity, &Name), Added<Name>>,
    parents: Query<&ChildOf>,
    bodies: Query<(), With<ArenaBody>>,
    mut sockets: Query<&mut RigSockets>,
    mut commands: Commands,
    mut pending: Local<Vec<(Entity, String, Entity)>>,
) {
    for (entity, name) in &new_names {
        // Ascend to the owning ArenaBody (if any).
        let mut cur = entity;
        let body = loop {
            if bodies.contains(cur) {
                break Some(cur);
            }
            match parents.get(cur) {
                Ok(p) => cur = p.0,
                Err(_) => break None,
            }
        };
        let Some(body) = body else { continue };
        if let Ok(mut index) = sockets.get_mut(body) {
            index.by_name.entry(name.as_str().to_string()).or_insert(entity);
        } else {
            // RigSockets not on the body yet — insert it, then queue this entry for next frame
            // (the command applies after this system).
            commands.entity(body).insert(RigSockets::default());
            pending.push((body, name.as_str().to_string(), entity));
        }
    }
    // Drain entries queued while the index component was still a pending command.
    pending.retain(|(body, name, entity)| match sockets.get_mut(*body) {
        Ok(mut index) => {
            index.by_name.entry(name.clone()).or_insert(*entity);
            false
        }
        Err(_) => true,
    });
}

/// Resolve a socket name for `player` to an attach parent: the named bone if the rig has it,
/// else the rig root, else the player entity itself.
pub fn resolve_socket(
    player: Entity,
    socket: &str,
    children: &Query<&Children>,
    bodies: &Query<(Entity, Option<&RigSockets>), With<ArenaBody>>,
) -> Entity {
    let body = children
        .get(player)
        .ok()
        .and_then(|kids| kids.iter().find(|k| bodies.contains(*k)));
    let Some(body) = body else {
        return player;
    };
    if let Ok((_, Some(index))) = bodies.get(body) {
        if let Some(bone) = index.by_name.get(socket) {
            return *bone;
        }
    }
    body
}
