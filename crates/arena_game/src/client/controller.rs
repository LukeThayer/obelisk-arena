//! First-person camera controller for the arena player.
//!
//! Two concerns (movement is NOT here — that's the predicted avian Dynamic body in
//! `client/net.rs`, driven by `ActionState<ArenaInput>` + the shared force controller):
//!
//!   1. **Mouse-look + first-person camera follow** — accumulated mouse-X feeds
//!      [`CameraYaw`], mouse-Y feeds [`AimPitch`]; `follow_local_net_player` then
//!      places the [`FollowCamera`] at the local player's eye height (`EYE_HEIGHT`
//!      above the player root) with rotation
//!      `Quat::from_axis_angle(Y, yaw) * Quat::from_axis_angle(X, pitch)` so the
//!      cam looks exactly where the mouse aims. The yaw/pitch resources are also read
//!      by the input bridge to aim the predicted controller + cast direction. The
//!      local player's own body is hidden (see `present::LocalPlayerBody`) so the
//!      camera is never inside a mesh.
//!   2. **Aim spine-pitch** — the `chest_joint` lean, copied verbatim from
//!      wisp's `apply_aim_pitch_to_local_spine` (`wisp/src/player/controller.rs`),
//!      renaming the body marker to [`ArenaBody`] and reading the pitch from the
//!      [`AimPitch`] resource (mouse-Y). Applied only to REMOTE (opponent) bodies;
//!      the hidden local body is skipped (see [`apply_aim_pitch_to_local_spine`]).
//!      Scheduled in `PostUpdate` AFTER `AnimationSystems`, BEFORE
//!      `TransformSystems::Propagate` so the lean is folded into the per-frame
//!      `GlobalTransform` propagation.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use super::present::LocalPlayerBody;
use super::rig::ArenaBody;

/// Marker on the first-person follow camera.
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

/// Mouse sensitivity (radians of yaw/pitch per pixel of accumulated motion). A `Resource` rather
/// than a `const` so it can be tuned at launch via the `ARENA_MOUSE_SENS` env var (mirroring the
/// other `ARENA_*` hooks); the default `0.0035` is the historical value. Read by
/// [`accumulate_mouse_look`].
#[derive(Resource)]
pub struct MouseSensitivity(pub f32);

impl Default for MouseSensitivity {
    fn default() -> Self {
        Self(0.0035)
    }
}

/// Aim-pitch clamp (radians). Matches wisp's 85°. Keeps the spine lean inside a
/// readable range and stops the camera from flipping over the top/bottom.
const PITCH_LIMIT: f32 = 85.0_f32 * std::f32::consts::PI / 180.0;

/// Camera eye height above the player root transform in world units. Placing the
/// camera at `player.translation + Vec3::Y * EYE_HEIGHT` puts it at roughly head
/// level for the Polysplit rig (capsule half-height ~0.6 + ~1.0 upper body).
///
/// Aliased to the shared [`crate::net::ARENA_EYE_HEIGHT`] so the camera eye and the server muzzle
/// offset (which fires the firebolt from this exact height) can never drift apart — that shared
/// height is what makes the crosshair ray equal the bolt path.
const EYE_HEIGHT: f32 = crate::net::ARENA_EYE_HEIGHT;

/// Name of the spine bone that drives upper-body aim lean. The Polysplit rig's
/// spine chain is `pelvis_joint → waist_joint → chest_joint → neck_joint`;
/// rotating `chest_joint` leans the torso, leaving the hips and legs planted.
/// Copied verbatim from `wisp/src/player/controller.rs:212`.
pub const AIM_PITCH_BONE: &str = "chest_joint";

/// Marker stamped ONCE on the REMOTE `chest_joint` spine bone so the per-frame aim
/// lean is a direct `With<SpinePitchBone>` query instead of a full-world `Name` scan
/// + string compare every frame.
///
/// [`stamp_spine_pitch_bones`] inserts it on the `Added<Name>` edge for any bone named
/// [`AIM_PITCH_BONE`] whose `ChildOf` chain crosses an [`ArenaBody`] but does NOT carry
/// [`LocalPlayerBody`] (the hidden local body is never leaned), so the marked set is
/// exactly the visible opponent torsos.
#[derive(Component)]
pub struct SpinePitchBone;

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
/// Movement is server-authoritative + client-predicted: `net::client_apply_movement` (with the
/// shared force controller) runs on the local `Predicted` entity under lightyear rollback, so this
/// plugin NO LONGER moves a Transform. What it keeps is the net-agnostic mouse-look + the camera
/// follow + the spine-pitch aim lean:
///
/// `Update`: `cursor_grab`, then `accumulate_mouse_look` (mouse → `CameraYaw`/`AimPitch` resources,
/// read by `client::mod::bridge_windowed_input_to_local_input` to drive the server controller), then
/// `follow_local_net_player` (positions the [`FollowCamera`] behind the local predicted player).
/// `stamp_spine_pitch_bones` (tags the remote chest bone with [`SpinePitchBone`] once it appears).
/// `PostUpdate`: `apply_aim_pitch_to_local_spine`, ordered between `AnimationSystems` and
/// `TransformSystems::Propagate` (the load-bearing order).
pub struct ArenaControllerPlugin;

impl Plugin for ArenaControllerPlugin {
    fn build(&self, app: &mut App) {
        // Camera-aim env hooks via the SHARED parser (`harness::EnvConfig`), so the headless client
        // (`app_headless.rs`) reads `ARENA_CAM_YAW`/`ARENA_TEST_PITCH` the exact same way.
        //   - `ARENA_TEST_PITCH`: a fixed lean for headless screenshot verification; when set it
        //     pins the pitch (`locked`) so `accumulate_mouse_look` skips the mouse-Y update.
        //   - `ARENA_CAM_YAW` (radians): seeds a 3/4 view for screenshot verification so the cast
        //     pose + muzzle particle + flying projectile are all legible (a straight-behind shot
        //     hides them behind the character). Default 0 → straight behind; mouse-X drives it.
        let env = super::harness::EnvConfig::from_env();
        let (pitch0, locked) = (env.test_pitch, env.test_pitch_locked);
        let yaw0 = env.cam_yaw;

        // Mouse sensitivity: `ARENA_MOUSE_SENS` (radians/pixel) overrides the 0.0035 default.
        let mouse_sens = std::env::var("ARENA_MOUSE_SENS")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .map(MouseSensitivity)
            .unwrap_or_default();

        app.insert_resource(CameraYaw(yaw0))
            .insert_resource(AimPitch(pitch0))
            .insert_resource(AimPitchLocked(locked))
            .insert_resource(mouse_sens)
            .add_systems(
                Update,
                (
                    (cursor_grab, accumulate_mouse_look, follow_local_net_player).chain(),
                    stamp_spine_pitch_bones,
                ),
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
    customization: Option<Res<crate::client::customization::CustomizationOpen>>,
) {
    // While the customizer is open it owns the cursor (free + visible for clicking buttons);
    // a stray LMB on a panel button must not re-lock it.
    if customization.map(|c| c.open).unwrap_or(false) {
        return;
    }
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
    sensitivity: Res<MouseSensitivity>,
    customization: Option<Res<crate::client::customization::CustomizationOpen>>,
) {
    // While the customizer is open the cursor is free (moving it to click buttons) and the orbit
    // preview owns the camera — don't accumulate mouse-look (it would spin the view on reopen).
    if customization.map(|c| c.open).unwrap_or(false) {
        return;
    }
    let delta = motion.delta;
    yaw.0 -= delta.x * sensitivity.0;
    if !pitch_locked.0 {
        // Invert mouse-Y: pushing up (negative delta.y) looks up (positive pitch).
        pitch.0 = (pitch.0 - delta.y * sensitivity.0).clamp(-PITCH_LIMIT, PITCH_LIMIT);
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

/// Place the first-person camera at the LOCAL predicted player's eye position each frame.
///
/// Translation: `player.translation + Vec3::Y * EYE_HEIGHT` (head level).
/// Rotation: `Quat::from_axis_angle(Y, yaw) * Quat::from_axis_angle(X, pitch)` — yaw from
/// mouse-X, pitch from mouse-Y — so the camera looks exactly where the player aims.
///
/// Replaces M1's over-the-shoulder `update_camera_and_aim` with a true first-person placement
/// following the materialized [`LocalNetPlayer`]. No-op until the local player is bodied
/// (pre-connect / pre-replication).
fn follow_local_net_player(
    player: Option<LocalNetPlayerTransform>,
    cam: Option<CameraTransform>,
    yaw: Res<CameraYaw>,
    pitch: Res<AimPitch>,
    customization: Option<Res<crate::client::customization::CustomizationOpen>>,
) {
    // While the customizer is open the orbit preview (`customization`) owns the camera.
    if customization.map(|c| c.open).unwrap_or(false) {
        return;
    }
    let (Some(player), Some(mut cam)) = (player, cam) else {
        return;
    };
    cam.translation = player.translation + Vec3::Y * EYE_HEIGHT;
    cam.rotation = Quat::from_axis_angle(Vec3::Y, yaw.0) * Quat::from_axis_angle(Vec3::X, pitch.0);
}

/// Stamp the [`SpinePitchBone`] marker on the REMOTE chest spine bone the frame its
/// `Name` first appears (the glTF scene spawner adds the bone entities with `Name` +
/// `ChildOf` together, so the [`ArenaBody`]/[`LocalPlayerBody`] ancestry is already
/// resolvable here). Matches `name == AIM_PITCH_BONE`, confirms an [`ArenaBody`]
/// ancestor, and SKIPS any bone whose chain carries [`LocalPlayerBody`] — the hidden
/// local body is never leaned. This moves the whole-world `Name` scan + ancestry walk
/// off the per-frame path and onto the one-shot spawn edge.
fn stamp_spine_pitch_bones(
    mut commands: Commands,
    new_named: Query<(Entity, &Name), Added<Name>>,
    parents: Query<&ChildOf>,
    body_marker: Query<(), With<ArenaBody>>,
    local_body_marker: Query<(), With<LocalPlayerBody>>,
) {
    for (entity, name) in &new_named {
        if name.as_str() != AIM_PITCH_BONE {
            continue;
        }
        if !ancestor_has_body_marker(entity, &parents, &body_marker) {
            continue;
        }
        // Local body is hidden in first-person; skip so only the remote
        // (opponent) torso is ever marked + leaned.
        if ancestor_has_local_body(entity, &parents, &local_body_marker) {
            continue;
        }
        commands.entity(entity).insert(SpinePitchBone);
    }
}

/// After animation has set bone Transforms, apply the aim pitch on top of the
/// `chest_joint` spine bone so the body leans with the aim. Behaviorally VERBATIM
/// from `wisp/src/player/controller.rs:219-249` (the spine-pitch system), but the
/// per-frame `Name` scan + ancestry walk is replaced by the [`SpinePitchBone`] marker
/// ([`stamp_spine_pitch_bones`] stamps exactly the REMOTE chest bones), and the pitch
/// source is the [`AimPitch`] resource (mouse-Y) rather than wisp's `Facing.pitch`.
///
/// Only the REMOTE (opponent) bodies carry [`SpinePitchBone`] — the LOCAL body is
/// hidden in first-person, so leaning it is moot and it is never marked.
///
/// Runs in `PostUpdate`, ordered between `AnimationSystems` and
/// `TransformSystems::Propagate`, so the modification is included in the
/// per-frame `GlobalTransform` propagation. Getting this order wrong makes the
/// lean invisible (Propagate already ran) or jittery (fights the clip).
pub fn apply_aim_pitch_to_local_spine(
    aim_pitch: Res<AimPitch>,
    mut bones: Query<&mut Transform, With<SpinePitchBone>>,
) {
    // Bone-local axes on the gltf-imported Polysplit chest bone: X runs along
    // the spine (its "up"), so rotating around X twists the torso. Z is the
    // perpendicular sideways axis we pivot the lean around. Negative sign: pitch
    // is positive when looking up, but the chest's +Z faces the wrong way for
    // "lean back" — invert so up-look bends the upper body back, down-look bends
    // it forward.
    let pitch_quat = Quat::from_axis_angle(Vec3::Z, -aim_pitch.0);
    for mut tf in &mut bones {
        // Post-multiply so the animation's bone rotation is preserved and the
        // aim pitch is added on top in the bone's local frame.
        tf.rotation *= pitch_quat;
    }
}

/// Walk the `ChildOf` parent chain (NOT Transform parents) to confirm a bone
/// belongs to an [`ArenaBody`] before marking it. Copied verbatim from
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

/// Walk the `ChildOf` parent chain to check if a bone belongs to the LOCAL
/// player's body (tagged [`LocalPlayerBody`]). Used by
/// [`stamp_spine_pitch_bones`] to skip marking the hidden local body.
fn ancestor_has_local_body(
    entity: Entity,
    parents: &Query<&ChildOf>,
    local_body: &Query<(), With<LocalPlayerBody>>,
) -> bool {
    let mut cur = entity;
    loop {
        if local_body.contains(cur) {
            return true;
        }
        match parents.get(cur) {
            Ok(p) => cur = p.0,
            Err(_) => return false,
        }
    }
}
