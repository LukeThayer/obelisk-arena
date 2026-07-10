//! Server-side surfaces bridge (spec §7): attach replication to every sim-spawned
//! [`SurfacePatch`] (the skill-object pattern — the sim entity IS the replicated entity;
//! lightyear despawn-replication handles every removal path: decay, consume, evict, round
//! reset), and trace the paint/remove stream for the net-test harness.
use avian3d::prelude::{Position, Rotation};
use bevy::prelude::*;
use lightyear::prelude::{NetworkTarget, Replicate};
use obelisk_bevy::surfaces::{SurfacePainted, SurfacePatch, SurfaceRemoved};
use serde_json::json;

use crate::net::protocol::{NetworkOwner, NetworkedSurfacePatch};
use crate::trace;

/// Attach replication to freshly-painted patches. Runs in Update (the sim paints in
/// FixedUpdate; `Added` is observed on the next Update pass — same cadence as the rig/visual
/// attach systems). The patch is STATIC: `Position` is set once from the sim `Transform`.
pub(crate) fn attach_patch_replication(
    q: Query<(Entity, &SurfacePatch, &Transform), Added<SurfacePatch>>,
    owners: Query<&NetworkOwner>,
    mut commands: Commands,
) {
    for (e, p, tf) in &q {
        let owner = owners.get(p.owner).map(|o| o.0).unwrap_or(0);
        commands.entity(e).insert((
            Name::new(format!("SurfacePatch({})", p.surface)),
            NetworkedSurfacePatch {
                surface: p.surface.clone(),
                owner,
                radius: p.radius,
            },
            Position(tf.translation),
            Rotation::default(),
            Replicate::to_clients(NetworkTarget::All),
        ));
    }
}

/// Trace observers (the harness substrate — `kind` key reserved, use `surface`/`reason`).
pub(crate) fn trace_surface_painted(ev: On<SurfacePainted>) {
    let e = ev.event();
    trace::event(
        "surface_painted",
        json!({ "surface": e.surface,
                "pos": [e.position.x, e.position.y, e.position.z] }),
    );
}

pub(crate) fn trace_surface_removed(ev: On<SurfaceRemoved>) {
    let e = ev.event();
    trace::event(
        "surface_removed",
        json!({ "surface": e.surface, "reason": format!("{:?}", e.reason) }),
    );
}
