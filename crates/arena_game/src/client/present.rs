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

use crate::client::net::{LocalNetPlayer, MaterializedBody};
use crate::client::rig::{ArenaBody, LocalAnimBlend};
use crate::net::protocol::NetworkedPlayer;

/// Plugin: attach the rigged body to materialized networked players each frame.
pub struct ArenaPresentPlugin;

impl Plugin for ArenaPresentPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (attach_rig_to_players, hide_local_player_body));
    }
}

/// Marker on the LOCAL player's `ArenaBody` scene root. Used to:
///   - Set `Visibility::Hidden` so the first-person camera is never inside the
///     local player's head mesh (`hide_local_player_body`).
///   - Skip the spine aim-pitch lean on the local body (the controller only
///     leans REMOTE opponents, since the local body is never seen).
///
/// Only attached to the body spawned for the entity carrying [`LocalNetPlayer`];
/// remote (opponent) bodies carry no such marker and stay fully visible.
#[derive(Component)]
pub struct LocalPlayerBody;

/// Marker: this networked player already has its rig scene child attached (so we attach exactly once).
#[derive(Component)]
struct RigAttached;

/// For every materialized `NetworkedPlayer` lacking a rig, spawn the `character.glb` `ArenaBody`
/// scene as a child (π gltf-yaw offset, matching M1's `spawn_combatants`) + insert `LocalAnimBlend`
/// on the player root (the rig's `drive_animation` reads it). The capsule `Collider` on the proxy
/// body stays (it never renders — no `Mesh3d`), so the only visible thing is the rig. Polls for new
/// replicas, so the second (late-joining) player gets a rig too.
///
/// LOCAL player bodies are tagged [`LocalPlayerBody`] and spawned with `Visibility::Hidden` so the
/// first-person camera is never inside the local player's head. REMOTE (opponent) bodies are
/// `Visibility::default()` (inherited → visible).
#[allow(clippy::type_complexity)]
fn attach_rig_to_players(
    new_players: Query<
        (Entity, Has<LocalNetPlayer>),
        (
            With<NetworkedPlayer>,
            With<MaterializedBody>,
            Without<RigAttached>,
        ),
    >,
    assets: Res<AssetServer>,
    mut commands: Commands,
) {
    for (player, is_local) in &new_players {
        let scene: Handle<Scene> =
            assets.load(GltfAssetLabel::Scene(0).from_asset("character.glb"));
        // The π yaw offset is the gltf import convention M1 relied on (the character's mesh
        // faces +Z after this) so `NetworkedPosition.yaw` (body facing) reads correctly.
        let base_tf = Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        let body = if is_local {
            commands
                .spawn((
                    Name::new("ArenaBody"),
                    ArenaBody,
                    LocalPlayerBody,
                    SceneRoot(scene),
                    base_tf,
                    // Hidden: the local player's body is never seen in first-person.
                    Visibility::Hidden,
                ))
                .id()
        } else {
            commands
                .spawn((
                    Name::new("ArenaBody"),
                    ArenaBody,
                    SceneRoot(scene),
                    base_tf,
                    Visibility::default(),
                ))
                .id()
        };
        commands
            .entity(player)
            .insert((LocalAnimBlend::default(), RigAttached))
            .add_child(body);
    }
}

/// Enforce `Visibility::Hidden` on every [`LocalPlayerBody`] each frame. This
/// is idempotent insurance: `apply_arena_part_visibility` (in `parts.rs`) sets
/// per-mesh visibility (some to `Inherited`) which propagates correctly from the
/// `Hidden` root, but if any other system resets the root visibility this restores
/// it. Cheap — at most one entity is tagged `LocalPlayerBody` at any time.
fn hide_local_player_body(mut local_bodies: Query<&mut Visibility, With<LocalPlayerBody>>) {
    for mut vis in &mut local_bodies {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
    }
}
