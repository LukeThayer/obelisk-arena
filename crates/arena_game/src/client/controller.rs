//! Third-person over-the-shoulder controller for the arena player.
//!
//! Three concerns, all kinematic (no avian rigidbody on the player — obelisk's
//! spatial model owns the authoritative hitboxes; the player moves by writing
//! `Transform.translation` directly):
//!
//!   1. **Follow camera** — an over-the-shoulder cam positioned behind + above
//!      the player in the player's yaw frame, looking at the head. Camera yaw is
//!      driven by accumulated mouse-X each frame; the cam follows the player's
//!      `Transform.translation`.
//!   2. **Camera-relative WASD movement** — moves the player's `Transform`
//!      directly (`dir * speed * dt`); rotates the body to face the movement
//!      direction; records `world_velocity` (frame delta) for the Task 12
//!      locomotion blend.
//!   3. **Aim spine-pitch** — the `chest_joint` lean, copied verbatim from
//!      wisp's `apply_aim_pitch_to_local_spine` (`wisp/src/player/controller.rs`),
//!      renaming the body marker to [`ArenaBody`] and reading the pitch from the
//!      [`AimPitch`] resource (mouse-Y). Scheduled in `PostUpdate` AFTER
//!      `AnimationSystems`, BEFORE `TransformSystems::Propagate` so the lean is
//!      folded into the per-frame `GlobalTransform` propagation.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use super::rig::ArenaBody;

/// Marker on the player combatant root that this controller drives. Spawned in
/// Marker on the over-the-shoulder follow camera.
#[derive(Component)]
pub struct FollowCamera;

/// The follow camera's mutable `Transform`, disjoint from the local player's. Aliased to keep
/// [`follow_local_net_player`]'s signature under clippy's `type_complexity` bar.
type CameraTransform<'w, 's> = Single<
    'w,
    's,
    &'static mut Transform,
    (
        With<FollowCamera>,
        Without<crate::client::net::LocalNetPlayer>,
    ),
>;

/// Mouse sensitivity (radians of yaw/pitch per pixel of accumulated motion).
const MOUSE_SENSITIVITY: f32 = 0.0035;

/// Aim-pitch clamp (radians). Matches wisp's 85°. Keeps the spine lean inside a
/// readable range and stops the camera from flipping over the top/bottom.
const PITCH_LIMIT: f32 = 85.0_f32 * std::f32::consts::PI / 180.0;

/// Over-the-shoulder camera offset in the player's yaw frame: behind (-Z is
/// "forward", so +Z*4 puts the cam 4 units *behind*) and 2 units above.
const CAM_OFFSET: Vec3 = Vec3::new(0.0, 2.0, 4.0);

/// Where the camera looks, relative to the player root (≈ head height).
const CAM_LOOK_AT: Vec3 = Vec3::new(0.0, 1.5, 0.0);

/// Name of the spine bone that drives upper-body aim lean. The Polysplit rig's
/// spine chain is `pelvis_joint → waist_joint → chest_joint → neck_joint`;
/// rotating `chest_joint` leans the torso, leaving the hips and legs planted.
/// Copied verbatim from `wisp/src/player/controller.rs:212`.
pub const AIM_PITCH_BONE: &str = "chest_joint";

/// Camera yaw (radians, around +Y) accumulated from mouse-X. The player body is
/// rotated to this yaw when moving so WASD stays camera-relative.
#[derive(Resource, Default)]
pub struct CameraYaw(pub f32);

/// Aim pitch (radians) accumulated from mouse-Y, fed to
/// [`apply_aim_pitch_to_local_spine`]. Positive = looking up.
///
/// Debug override: if `ARENA_TEST_PITCH` is set, the controller seeds this with
/// that value and `update_camera_and_aim` leaves it untouched, so the torso
/// holds a fixed lean for headless screenshot verification. Default off → driven
/// by mouse-Y.
#[derive(Resource, Default)]
pub struct AimPitch(pub f32);

/// Whether `ARENA_TEST_PITCH` pinned the aim pitch (debug hook). When true,
/// `update_camera_and_aim` skips the mouse-Y → pitch update so the forced lean
/// stays put for the screenshot.
#[derive(Resource, Default)]
pub struct AimPitchLocked(pub bool);

/// Plugin: registers the controller resources + the NET-CLIENT-appropriate systems.
///
/// M2.5 Task 21 repurposed this from the M1 co-located controller (which moved a `PlayerController`
/// Transform directly) to the NETWORKED windowed client. Movement is now server-authoritative +
/// client-predicted (`client::prediction` integrates the local body's avian `Position` from
/// `LocalInput`), so this plugin NO LONGER moves a Transform — `move_player` is gone. What it keeps
/// is the net-agnostic mouse-look + the camera follow + the spine-pitch aim lean:
///
/// `Update`: `cursor_grab`, then `accumulate_mouse_look` (mouse → `CameraYaw`/`AimPitch` resources,
/// read by `client::mod::bridge_windowed_input_to_local_input` to drive the server controller), then
/// `follow_local_net_player` (positions the [`FollowCamera`] behind the local predicted player).
/// `PostUpdate`: `apply_aim_pitch_to_local_spine`, ordered between `AnimationSystems` and
/// `TransformSystems::Propagate` (the load-bearing order).
pub struct ArenaControllerPlugin;

impl Plugin for ArenaControllerPlugin {
    fn build(&self, app: &mut App) {
        // Debug pitch override: a fixed lean for headless screenshot verification.
        let (pitch0, locked) = std::env::var("ARENA_TEST_PITCH")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .map(|p| (p, true))
            .unwrap_or((0.0, false));

        // Initial camera yaw. `ARENA_CAM_YAW` (radians) seeds a 3/4 view for screenshot verification
        // so the cast pose + muzzle particle + flying projectile are all legible (a straight-behind
        // shot hides them behind the character). Default 0 → straight behind; mouse-X drives it.
        let yaw0 = std::env::var("ARENA_CAM_YAW")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);

        app.insert_resource(CameraYaw(yaw0))
            .insert_resource(AimPitch(pitch0))
            .insert_resource(AimPitchLocked(locked))
            .add_systems(
                Update,
                (cursor_grab, accumulate_mouse_look, follow_local_net_player).chain(),
            )
            .add_systems(
                PostUpdate,
                apply_aim_pitch_to_local_spine
                    .after(bevy::app::AnimationSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

/// Click-to-grab / Esc-to-release the cursor so mouse-look doesn't drift the OS
/// pointer off the window. Mirrors wisp's `cursor_grab`.
fn cursor_grab(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Option<Single<&mut CursorOptions, With<PrimaryWindow>>>,
) {
    let Some(mut cursor) = cursor else {
        return;
    };
    if cursor.visible {
        if mouse.just_pressed(MouseButton::Left) {
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
    } else if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// Accumulate mouse motion into [`CameraYaw`] + [`AimPitch`] (net-agnostic). The yaw/pitch are read
/// by `client::mod::bridge_windowed_input_to_local_input` (→ `LocalInput` → the server controller +
/// local prediction) and by the spine-pitch aim lean; the camera placement is a SEPARATE system
/// ([`follow_local_net_player`]) so this stays a pure input system with no player dependency.
///
/// Mouse-X → yaw (turn the camera around the player); mouse-Y → pitch (aim lean, inverted so pushing
/// the mouse up looks up). Pitch is skipped when `ARENA_TEST_PITCH` pinned it (debug hook).
fn accumulate_mouse_look(
    motion: Res<AccumulatedMouseMotion>,
    mut yaw: ResMut<CameraYaw>,
    mut pitch: ResMut<AimPitch>,
    pitch_locked: Res<AimPitchLocked>,
) {
    let delta = motion.delta;
    yaw.0 -= delta.x * MOUSE_SENSITIVITY;
    if !pitch_locked.0 {
        // Invert mouse-Y: pushing up (negative delta.y) looks up (positive pitch).
        pitch.0 = (pitch.0 - delta.y * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }
}

/// The local predicted player's read-only Transform, for the camera to follow. The networked client
/// has no `PlayerController` entity (the M1 co-located player is gone) — the local player is the
/// materialized [`LocalNetPlayer`] (a replicated `NetworkedPlayer` whose `NetworkOwner == LocalId`).
/// Aliased to keep [`follow_local_net_player`]'s signature under clippy's `type_complexity` bar.
type LocalNetPlayerTransform<'w, 's> = Single<
    'w,
    's,
    &'static Transform,
    (
        With<crate::client::net::LocalNetPlayer>,
        Without<FollowCamera>,
    ),
>;

/// Place the over-the-shoulder follow camera behind + above the LOCAL predicted player, looking at
/// its head, recomputed each frame in the [`CameraYaw`] frame so the cam follows the predicted body
/// and tracks mouse-look. Replaces M1's `PlayerController`-following branch of `update_camera_and_aim`
/// with one that follows the materialized [`LocalNetPlayer`]. No-op until the local player is bodied
/// (pre-connect / pre-replication).
fn follow_local_net_player(
    player: Option<LocalNetPlayerTransform>,
    cam: Option<CameraTransform>,
    yaw: Res<CameraYaw>,
) {
    let (Some(player), Some(mut cam)) = (player, cam) else {
        return;
    };
    let yaw_rot = Quat::from_axis_angle(Vec3::Y, yaw.0);
    let focus = player.translation + CAM_LOOK_AT;
    cam.translation = player.translation + yaw_rot * CAM_OFFSET;
    cam.look_at(focus, Vec3::Y);
}

/// After animation has set bone Transforms, apply the aim pitch on top of the
/// `chest_joint` spine bone so the body leans with the aim. Copied VERBATIM from
/// `wisp/src/player/controller.rs:219-249` (the spine-pitch system), with the
/// body marker renamed to [`ArenaBody`] and the pitch source switched from
/// wisp's first-person `Facing.pitch` to the [`AimPitch`] resource.
///
/// Runs in `PostUpdate`, ordered between `AnimationSystems` and
/// `TransformSystems::Propagate`, so the modification is included in the
/// per-frame `GlobalTransform` propagation. Getting this order wrong makes the
/// lean invisible (Propagate already ran) or jittery (fights the clip).
pub fn apply_aim_pitch_to_local_spine(
    aim_pitch: Res<AimPitch>,
    bones: Query<(Entity, &Name)>,
    parents: Query<&ChildOf>,
    body_marker: Query<(), With<ArenaBody>>,
    mut transforms: Query<&mut Transform>,
) {
    // Bone-local axes on the gltf-imported Polysplit chest bone: X runs along
    // the spine (its "up"), so rotating around X twists the torso. Z is the
    // perpendicular sideways axis we pivot the lean around. Negative sign: pitch
    // is positive when looking up, but the chest's +Z faces the wrong way for
    // "lean back" — invert so up-look bends the upper body back, down-look bends
    // it forward.
    let pitch_quat = Quat::from_axis_angle(Vec3::Z, -aim_pitch.0);
    for (entity, name) in &bones {
        if name.as_str() != AIM_PITCH_BONE {
            continue;
        }
        if !ancestor_has_body_marker(entity, &parents, &body_marker) {
            continue;
        }
        if let Ok(mut tf) = transforms.get_mut(entity) {
            // Post-multiply so the animation's bone rotation is preserved and the
            // aim pitch is added on top in the bone's local frame.
            tf.rotation *= pitch_quat;
        }
    }
}

/// Walk the `ChildOf` parent chain (NOT Transform parents) to confirm a bone
/// belongs to an [`ArenaBody`] before mutating it. Copied verbatim from
/// `wisp/src/player/controller.rs:251-266`, marker renamed.
fn ancestor_has_body_marker(
    entity: Entity,
    parents: &Query<&ChildOf>,
    marker: &Query<(), With<ArenaBody>>,
) -> bool {
    let mut cur = entity;
    loop {
        if marker.contains(cur) {
            return true;
        }
        match parents.get(cur) {
            Ok(p) => cur = p.0,
            Err(_) => return false,
        }
    }
}
