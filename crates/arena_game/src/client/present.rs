//! Windowed-client presentation glue (M2.5 Task 21): attach the rigged character to every
//! materialized networked player so BOTH the local + remote players render as the real M1 rig
//! (`rig.rs`) rather than the bare capsule. Windowed-only — the headless client has no rendering.
//!
//! The server replicates each player as a `NetworkedPlayer` + identity + pose stream;
//! `client::net::materialize_replicated_players` gives each a local avian render-proxy body + a
//! `Transform`. This module hangs the `character.glb` `ArenaBody` scene under that body (as a child,
//! with the same π gltf-yaw offset M1 used) + the `LocalAnimBlend` the rig animation driver reads, so
//! the player appears as a costumed character. Idempotent via [`RigAttached`] (polls for new
//! replicas + the late joiner).

use bevy::prelude::*;

use crate::client::net::MaterializedBody;
use crate::client::rig::{ArenaBody, LocalAnimBlend};
use crate::net::protocol::NetworkedPlayer;

/// Plugin: attach the rigged body to materialized networked players each frame.
pub struct ArenaPresentPlugin;

impl Plugin for ArenaPresentPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, attach_rig_to_players);
    }
}

/// Marker: this networked player already has its rig scene child attached (so we attach exactly once).
#[derive(Component)]
struct RigAttached;

/// For every materialized `NetworkedPlayer` lacking a rig, spawn the `character.glb` `ArenaBody`
/// scene as a child (π gltf-yaw offset, matching M1's `spawn_combatants`) + insert `LocalAnimBlend`
/// on the player root (the rig's `drive_animation` reads it). The capsule `Collider` on the proxy
/// body stays (it never renders — no `Mesh3d`), so the only visible thing is the rig. Polls for new
/// replicas, so the second (late-joining) player gets a rig too.
#[allow(clippy::type_complexity)]
fn attach_rig_to_players(
    new_players: Query<
        Entity,
        (
            With<NetworkedPlayer>,
            With<MaterializedBody>,
            Without<RigAttached>,
        ),
    >,
    assets: Res<AssetServer>,
    mut commands: Commands,
) {
    for player in &new_players {
        let scene: Handle<Scene> =
            assets.load(GltfAssetLabel::Scene(0).from_asset("character.glb"));
        let body = commands
            .spawn((
                Name::new("ArenaBody"),
                ArenaBody,
                SceneRoot(scene),
                // The π yaw offset is the gltf import convention M1 relied on (the character's mesh
                // faces +Z after this) so `NetworkedPosition.yaw` (body facing) reads correctly.
                Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
                Visibility::default(),
            ))
            .id();
        commands
            .entity(player)
            .insert((LocalAnimBlend::default(), RigAttached))
            .add_child(body);
    }
}
