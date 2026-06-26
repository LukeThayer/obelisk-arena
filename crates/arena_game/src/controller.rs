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

use crate::rig::ArenaBody;

/// Marker on the player combatant root that this controller drives. Spawned in
/// `main::spawn_combatants` alongside the `Combatant`. Separate from `ArenaBody`
/// (the rig scene-root child) so the controller can move the root while the
/// spine-pitch system mutates a bone deep inside the body subtree.
#[derive(Component)]
pub struct PlayerController;

/// Marker on the over-the-shoulder follow camera.
#[derive(Component)]
pub struct FollowCamera;

/// The player's read-only `Transform`, disjoint from the camera's. Aliased to keep
/// [`update_camera_and_aim`]'s signature under clippy's `type_complexity` bar.
type PlayerTransform<'w, 's> =
    Single<'w, 's, &'static Transform, (With<PlayerController>, Without<FollowCamera>)>;

/// The follow camera's mutable `Transform`, disjoint from the player's. Aliased to keep
/// [`update_camera_and_aim`]'s signature under clippy's `type_complexity` bar.
type CameraTransform<'w, 's> =
    Single<'w, 's, &'static mut Transform, (With<FollowCamera>, Without<PlayerController>)>;

/// Player horizontal move speed (world units/sec). Roughly matches wisp's
/// `MAX_SPEED` (4.2) so the locomotion clips read at a sensible cadence later.
const MOVE_SPEED: f32 = 4.0;

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

/// Player velocity this frame (world units/sec), derived from the
/// `Transform.translation` delta. Read by the Task 12 locomotion blend to pick
/// between idle / directional-walk clips. Kinematic movement means avian gives
/// us no `LinearVelocity`, so we track it ourselves.
#[derive(Resource, Default)]
pub struct PlayerVelocity(pub Vec3);

/// Plugin: registers the controller resources + systems.
///
/// `Update`: `cursor_grab`, then `update_camera_and_aim` (mouse → yaw/pitch +
/// cam pose), then `move_player` (WASD → translation + body yaw + velocity).
/// `PostUpdate`: `apply_aim_pitch_to_local_spine`, ordered between
/// `AnimationSystems` and `TransformSystems::Propagate` (the load-bearing order).
pub struct ArenaControllerPlugin;

impl Plugin for ArenaControllerPlugin {
    fn build(&self, app: &mut App) {
        // Debug pitch override: a fixed lean for headless screenshot verification.
        let (pitch0, locked) = std::env::var("ARENA_TEST_PITCH")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .map(|p| (p, true))
            .unwrap_or((0.0, false));

        // Initial camera yaw. `ARENA_CAM_YAW` (radians) seeds a 3/4 view for headless
        // screenshot verification so the cast pose + muzzle particle + flying projectile
        // are all legible (a straight-behind shot hides them behind the character).
        // Default 0 → straight behind; mouse-X drives it from there in normal play.
        let yaw0 = std::env::var("ARENA_CAM_YAW")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);

        app.insert_resource(CameraYaw(yaw0))
            .insert_resource(AimPitch(pitch0))
            .insert_resource(AimPitchLocked(locked))
            .init_resource::<PlayerVelocity>()
            .add_systems(
                Update,
                (cursor_grab, update_camera_and_aim, move_player).chain(),
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

/// Accumulate mouse motion into [`CameraYaw`] + [`AimPitch`], then place the
/// follow camera behind + above the player looking at its head.
///
/// Mouse-X → yaw (turn the camera around the player); mouse-Y → pitch (aim lean,
/// inverted so pushing the mouse up looks up). Pitch is skipped when
/// `ARENA_TEST_PITCH` pinned it (debug hook). The camera position is recomputed
/// from the player's translation every frame so it follows movement.
fn update_camera_and_aim(
    motion: Res<AccumulatedMouseMotion>,
    mut yaw: ResMut<CameraYaw>,
    mut pitch: ResMut<AimPitch>,
    pitch_locked: Res<AimPitchLocked>,
    player: Option<PlayerTransform>,
    cam: Option<CameraTransform>,
) {
    let delta = motion.delta;
    yaw.0 -= delta.x * MOUSE_SENSITIVITY;
    if !pitch_locked.0 {
        // Invert mouse-Y: pushing up (negative delta.y) looks up (positive pitch).
        pitch.0 = (pitch.0 - delta.y * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    let (Some(player), Some(mut cam)) = (player, cam) else {
        return;
    };
    let yaw_rot = Quat::from_axis_angle(Vec3::Y, yaw.0);
    let focus = player.translation + CAM_LOOK_AT;
    cam.translation = player.translation + yaw_rot * CAM_OFFSET;
    cam.look_at(focus, Vec3::Y);
}

/// Camera-relative WASD: move the player's `Transform` directly (kinematic),
/// rotate the body to face the movement direction, and record the per-frame
/// world velocity for the locomotion blend.
///
/// W/S move along the camera's forward/back, A/D strafe — all in the
/// [`CameraYaw`] frame so movement tracks where the camera looks. The player
/// root's yaw is snapped toward the movement heading; when idle the body keeps
/// its last facing (so it doesn't snap back to a default).
fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    yaw: Res<CameraYaw>,
    mut velocity: ResMut<PlayerVelocity>,
    player: Option<Single<&mut Transform, With<PlayerController>>>,
) {
    let Some(mut tf) = player else {
        velocity.0 = Vec3::ZERO;
        return;
    };

    // WASD in the camera-yaw frame. Forward = -Z, right = +X.
    let mut input = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        input.z -= 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        input.z += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        input.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        input.x += 1.0;
    }

    let dt = time.delta_secs();
    if input == Vec3::ZERO {
        velocity.0 = Vec3::ZERO;
        return;
    }

    let yaw_rot = Quat::from_axis_angle(Vec3::Y, yaw.0);
    let world_dir = (yaw_rot * input.normalize()).normalize();
    let step = world_dir * MOVE_SPEED * dt;
    tf.translation += step;
    velocity.0 = world_dir * MOVE_SPEED;

    // Face the movement direction. The ArenaBody child carries a fixed π glTF
    // yaw offset, so the character's WORLD forward is `root_rot * +Z`. Setting
    // `root_rot = RotY(heading)` with `heading = atan2(dir.x, dir.z)` makes that
    // world forward equal `world_dir` — no extra π here (that's already on the
    // body child); adding it would double-flip and the character would moonwalk.
    let heading_yaw = world_dir.x.atan2(world_dir.z);
    tf.rotation = Quat::from_axis_angle(Vec3::Y, heading_yaw);
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
