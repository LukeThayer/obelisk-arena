//! Movement: lightyear-native server controller (the shared force controller over the replicated
//! `ActionState<ArenaInput>` lightyear keeps in sync for each client's controlled entity).
//!
//! `Without<Predicted>` is a host-server safety guard (the server has no Predicted entities on a
//! dedicated build, so it's a no-op there) mirroring `simple_box`/`avian_3d_character`.

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::Predicted;
use serde_json::json;

use crate::net::input::ArenaInput;
use crate::net::protocol::{NetworkOwner, NetworkedPlayer};
use crate::shared_controller::{apply_arena_movement, apply_arena_yaw};
use crate::trace;

/// Face each authoritative character to its input yaw (writes avian `Rotation`).
#[allow(clippy::type_complexity)]
pub(crate) fn server_apply_yaw(
    mut q: Query<
        (&ActionState<ArenaInput>, &mut Rotation),
        (With<NetworkedPlayer>, Without<Predicted>),
    >,
) {
    for (action, mut rot) in &mut q {
        apply_arena_yaw(&action.0, &mut rot);
    }
}

/// Apply the planar movement force + jump impulse for each authoritative character. `grounded`
/// comes from the shared raycast check (self + child hurtbox excluded so the ray can't ground on
/// the caster) — real level geometry, not a flat-floor Y threshold.
#[allow(clippy::type_complexity)]
pub(crate) fn server_apply_movement(
    time: Res<Time>,
    spatial: SpatialQuery,
    children: Query<&Children>,
    mut q: Query<
        (Entity, &Position, &ComputedMass, &ActionState<ArenaInput>, Forces),
        (With<NetworkedPlayer>, Without<Predicted>),
    >,
) {
    let dt = time.delta_secs();
    for (entity, position, mass, action, forces) in &mut q {
        let mut exclude = vec![entity];
        if let Ok(kids) = children.get(entity) {
            exclude.extend(kids.iter());
        }
        let grounded = crate::shared_controller::grounded_by_ray(&spatial, position.0, &exclude);
        apply_arena_movement(mass, dt, &action.0, forces, grounded);
    }
}

/// Throttled trace of each player's authoritative avian `Position` so the headless
/// movement-replication check can confirm the server's ground-truth pose changes for a moving
/// player. Keyed by `NetworkOwner`; gated on `Changed<Position>` so an idle player is silent.
#[allow(clippy::type_complexity)]
pub(crate) fn trace_server_pose(
    q: Query<(Entity, &Position, &NetworkOwner), (With<NetworkedPlayer>, Changed<Position>)>,
    mut throttle: Local<HashMap<Entity, u32>>,
) {
    for (entity, position, owner) in &q {
        let n = throttle.entry(entity).or_insert(0);
        *n += 1;
        if *n % 30 == 1 {
            trace::event(
                "server_pose",
                json!({
                    "owner": owner.0,
                    "pos": [position.0.x, position.0.y, position.0.z],
                }),
            );
        }
    }
}
