//! Windowed-client presentation glue: attach the rigged character to every materialized networked
//! player so BOTH the local + remote players render as the real `character.glb` rig (`rig.rs`)
//! rather than the bare capsule. Windowed-only — the headless client has no rendering.
//!
//! The server replicates each player as a `NetworkedPlayer` + identity + avian `Position`/`Rotation`;
//! `client::net::materialize_predicted_players` (local Dynamic body) /
//! `materialize_interpolated_players` (remotes, lightyear-driven pose) tag each with
//! `MaterializedBody`. This module hangs the `character.glb` `ArenaBody` scene under that body (as a
//! child, with the π gltf-yaw import offset) + the `LocalAnimBlend` the rig animation driver reads, so
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
        app.add_systems(
            Update,
            (
                attach_rig_to_players,
                hide_local_player_body,
                disable_skinned_mesh_culling,
            ),
        );
    }
}

/// Marker on the LOCAL player's `ArenaBody` scene root. Used to:
///   - Keep the local body's meshes on [`SELF_BODY_LAYER`] (`hide_local_player_body`) so the
///     layer-0 first-person camera never renders them — while the PORTAL cameras (layers
///     [0, SELF_BODY_LAYER]) do, so you can see your own character through a portal.
///   - Skip the spine aim-pitch lean on the local body (the controller only
///     leans REMOTE opponents, since the local body is never seen first-person).
///
/// Only attached to the body spawned for the entity carrying [`LocalNetPlayer`];
/// remote (opponent) bodies carry no such marker and stay fully visible.
#[derive(Component)]
pub struct LocalPlayerBody;

/// Marker: this networked player already has its rig scene child attached (so we attach exactly once).
#[derive(Component)]
struct RigAttached;

/// Vertical offset applied to the feet-rooted `character.glb` body so the model's CENTER sits at the
/// player origin (= the hurtbox-capsule center / body center). MEASURED: the glb's feet are ~0.03 and
/// its head ~1.21 above its scene origin (≈ feet-rooted), so the body center is ~0.62 above the scene
/// origin; offsetting the SceneRoot down by 0.62 puts the center at the player origin, the feet at
/// origin−0.59 (= world 0, on the platform / hitbox bottom), and the head at origin+0.59 (hitbox top).
const RIG_FOOT_OFFSET: f32 = -0.62;

/// For every materialized `NetworkedPlayer` lacking a rig, spawn the `character.glb` `ArenaBody`
/// scene as a child (π gltf-yaw offset) + insert `LocalAnimBlend` on the player root (the rig's
/// `drive_animation` reads it). The capsule `Collider` on the proxy body stays (it never renders — no
/// `Mesh3d`), so the only visible thing is the rig. Polls for new replicas, so the second
/// (late-joining) player gets a rig too.
///
/// LOCAL player bodies are tagged [`LocalPlayerBody`]; their meshes are moved onto
/// [`SELF_BODY_LAYER`] by `hide_local_player_body` so the layer-0 first-person camera never
/// sees them while the portal cameras do. REMOTE (opponent) bodies stay on layer 0 (visible).
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
        // The π yaw offset is the gltf import convention (the character's mesh
        // faces +Z after this) so the replicated avian `Rotation` (body facing) reads correctly.
        //
        // The `character.glb` is FEET-ROOTED — the model's feet sit at its scene origin. The player
        // origin is the CENTER of the body capsule (`Collider::capsule(0.35, 0.48)`, spawned in
        // `server::spawn::spawn_player_on_connect`), which rests with its center at world `GROUND_Y` (0.59)
        // and its bottom on the floor (world 0). So shift the body DOWN by `RIG_FOOT_OFFSET` (-0.62)
        // to line the model's feet up with the bottom of the capsule and rest them on the platform.
        let base_tf = Transform::from_translation(Vec3::Y * RIG_FOOT_OFFSET)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        let body = if is_local {
            commands
                .spawn((
                    Name::new("ArenaBody"),
                    ArenaBody,
                    LocalPlayerBody,
                    SceneRoot(scene),
                    base_tf,
                    // VISIBLE, but its meshes live on SELF_BODY_LAYER (layer_local_player_body)
                    // — the first-person main camera renders layer 0 only, so the local body is
                    // invisible to it, while the PORTAL cameras render [0, SELF_BODY_LAYER] and
                    // show your own character through a portal (wisp's SELF_BODY_LAYER).
                    Visibility::default(),
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

/// The render layer the LOCAL player's meshes live on (wisp `SELF_BODY_LAYER`): excluded from
/// the first-person main camera (which renders layer 0 only) but INCLUDED by the portal render
/// cameras, so you can see your own character through a portal.
pub const SELF_BODY_LAYER: usize = 1;

/// Disable CPU frustum culling on every skinned mesh (wisp `disable_skinned_mesh_culling`).
/// A skinned mesh's `Aabb` is the BIND pose in the mesh node's local space — the visual pose
/// comes from joint matrices the culling test never sees, so a camera near/inside the model
/// (the first-person body, anything seen through a portal camera's oblique frustum) culls
/// meshes that are actually on screen. The shadow pass has its own wider test, which is why a
/// culled body still cast a shadow.
pub(super) fn disable_skinned_mesh_culling(
    pending: Query<
        Entity,
        (
            With<bevy::mesh::skinning::SkinnedMesh>,
            Without<bevy::camera::visibility::NoFrustumCulling>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in &pending {
        commands
            .entity(entity)
            .insert(bevy::camera::visibility::NoFrustumCulling);
    }
}

/// Keep the LOCAL body's meshes on [`SELF_BODY_LAYER`] (portal-era replacement for the old
/// root `Visibility::Hidden`): the layer-0 first-person camera never renders them, while the
/// portal cameras (layers [0, SELF_BODY_LAYER]) do — you can see your own character through a
/// portal. While the customizer is open the meshes go back to layer 0 so the third-person
/// preview shows them. Re-asserted every frame: the glb scene streams its mesh entities in
/// asynchronously (and a scene swap would spawn fresh unlayered ones). Cheap — at most one
/// entity is tagged `LocalPlayerBody`, and the insert only fires on a layer mismatch.
fn hide_local_player_body(
    local_bodies: Query<Entity, With<LocalPlayerBody>>,
    children: Query<&Children>,
    meshes: Query<(Entity, Option<&bevy::camera::visibility::RenderLayers>), With<Mesh3d>>,
    customization: Option<Res<crate::client::customization::CustomizationOpen>>,
    mut commands: Commands,
) {
    let preview = customization.map(|c| c.open).unwrap_or(false);
    let target = if preview {
        bevy::camera::visibility::RenderLayers::layer(0)
    } else {
        bevy::camera::visibility::RenderLayers::layer(SELF_BODY_LAYER)
    };
    for root in &local_bodies {
        for node in std::iter::once(root).chain(children.iter_descendants(root)) {
            if let Ok((mesh, layers)) = meshes.get(node) {
                if layers != Some(&target) {
                    commands.entity(mesh).insert(target.clone());
                }
            }
        }
    }
}
