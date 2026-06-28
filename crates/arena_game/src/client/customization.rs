//! Character customization panel (Milestone D4).
//!
//! Toggles on the `K` key. One row per [`PartSelection`] slot (Top, Bottom,
//! Headwear, Hair, Eyes, Eyebrows, Mouth) with `<` / `>` buttons that cycle
//! that slot's variant table; an auto-updating label shows the current
//! variant. Edits land in the LOCAL [`PartSelection`] resource — the parts
//! visibility systems (`client::parts`) pick the change up via the resource's
//! `Changed` flag and re-skin the local rig.
//!
//! ## First-person preview
//!
//! In normal play the local player's body is `Visibility::Hidden` (you never
//! see yourself in first person), so edits would be invisible. While the panel
//! is OPEN we therefore free the cursor (visible + unlocked) so the buttons are
//! clickable, un-hide the local body (`hide_local_player_body` is gated on
//! [`CustomizationOpen`]), and place the camera in a third-person orbit around
//! the local player (A/D rotate) so the edits are visible.
//! On close we re-lock the cursor, re-hide the local body, and the normal
//! first-person follow resumes. Everything is gated behind [`CustomizationOpen`]
//! so ordinary play is unchanged.
//!
//! Adapted from `wisp/src/ui/customization.rs`; the color swatches + recolor
//! integration are intentionally dropped (no recolor system was ported), and
//! wisp's `InputMode`/bei layer is replaced by the [`CustomizationOpen`] gate.

use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::client::controller::FollowCamera;
use crate::client::net::{CustomizeDirty, LocalNetPlayer};
use crate::client::parts::{PartSelection, Slot as PartSlot, SLOTS};
use crate::client::present::LocalPlayerBody;

pub struct CustomizationPlugin;

impl Plugin for CustomizationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CustomizationOpen>()
            .init_resource::<OrbitCamera>()
            .add_systems(
                Update,
                (
                    toggle_panel_on_key,
                    handle_button_interactions,
                    refresh_slot_labels,
                    orbit_with_ad_keys,
                    position_preview_camera,
                ),
            );
    }
}

/// Whether the customizer panel is currently open. Read by the windowed
/// gameplay systems so they can suspend first-person behaviour while editing:
/// `controller::cursor_grab` (don't re-lock the cursor on click),
/// `controller::follow_local_net_player` + `controller::accumulate_mouse_look`
/// (the orbit preview owns the camera), `present::hide_local_player_body`
/// (keep the body visible for preview), and the `client::mod` input bridges
/// (don't move/cast/look while editing). `root` is the spawned panel entity.
#[derive(Resource, Default)]
pub struct CustomizationOpen {
    pub open: bool,
    root: Option<Entity>,
}

/// Preview-camera orbit yaw (radians) while the customizer is open. A/D rotate
/// it; [`position_preview_camera`] places the [`FollowCamera`] around the local
/// player at this yaw. Yaw 0 frames the character from the front.
#[derive(Resource, Default)]
pub struct OrbitCamera {
    pub yaw: f32,
}

/// The local player's read-only `Transform` for the preview camera to frame.
/// Aliased to keep [`position_preview_camera`]'s signature under clippy's
/// `type_complexity` bar.
type LocalPlayerTransform<'w, 's> =
    Query<'w, 's, &'static Transform, (With<LocalNetPlayer>, Without<FollowCamera>)>;

#[derive(Component)]
struct CustomizationRoot;

#[derive(Component, Clone, Copy)]
enum CustomButton {
    SlotPrev(PartSlot),
    SlotNext(PartSlot),
}

/// Tag on a slot row's variant-name text node so [`refresh_slot_labels`] knows
/// which slot's current label to rewrite when [`PartSelection`] changes.
#[derive(Component, Copy, Clone)]
struct SlotLabel(PartSlot);

/// Newly-pressed customizer buttons. Aliased to keep
/// [`handle_button_interactions`]'s signature under clippy's `type_complexity` bar.
type PressedButtons<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static CustomButton),
    (Changed<Interaction>, With<Button>),
>;

/// `K` toggles the panel: spawn/despawn it, free/lock the cursor, and show/hide
/// the local body for the preview.
fn toggle_panel_on_key(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<CustomizationOpen>,
    cursor: Option<Single<&mut CursorOptions, With<PrimaryWindow>>>,
    selection: Res<PartSelection>,
    mut dirty: ResMut<CustomizeDirty>,
    mut local_body: Query<&mut Visibility, With<LocalPlayerBody>>,
) {
    if !keys.just_pressed(KeyCode::KeyK) {
        return;
    }
    if state.open {
        if let Some(root) = state.root.take() {
            if let Ok(mut e) = commands.get_entity(root) {
                e.despawn();
            }
        }
        state.open = false;
        // Push the finished selection to the server → opponent (D6, debounced on close).
        dirty.0 = true;
        if let Some(mut cursor) = cursor {
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
        // Re-hide the local body; `hide_local_player_body` resumes enforcing this.
        for mut vis in &mut local_body {
            *vis = Visibility::Hidden;
        }
    } else {
        state.root = Some(spawn_panel(&mut commands, &selection));
        state.open = true;
        if let Some(mut cursor) = cursor {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
        // Un-hide the local body so the orbit preview can show the edits.
        for mut vis in &mut local_body {
            *vis = Visibility::Inherited;
        }
    }
}

fn spawn_panel(commands: &mut Commands, selection: &PartSelection) -> Entity {
    commands
        .spawn((
            CustomizationRoot,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(20.0),
                top: Val::Px(20.0),
                width: Val::Px(300.0),
                padding: UiRect::all(Val::Px(10.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.78)),
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Customize  (K close, A/D orbit)"),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::srgba(0.9, 0.9, 1.0, 1.0)),
            ));

            for &slot in SLOTS {
                spawn_slot_row(root, slot.display(), slot, selection.label(slot));
            }
        })
        .id()
}

fn spawn_slot_row(parent: &mut ChildSpawnerCommands, label: &str, slot: PartSlot, current: &str) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(8.0),
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|row| {
            // Slot label (e.g. "Top").
            row.spawn((
                Node {
                    width: Val::Px(64.0),
                    ..default()
                },
                children![(
                    Text::new(label.to_string()),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(Color::srgba(0.85, 0.85, 0.9, 1.0)),
                )],
            ));
            // Prev button.
            spawn_text_button(row, "<", CustomButton::SlotPrev(slot));
            // Variant name (auto-updates via SlotLabel).
            row.spawn((
                SlotLabel(slot),
                Node {
                    flex_grow: 1.0,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                children![(
                    Text::new(current.to_string()),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(Color::WHITE),
                )],
            ));
            // Next button.
            spawn_text_button(row, ">", CustomButton::SlotNext(slot));
        });
}

fn spawn_text_button(parent: &mut ChildSpawnerCommands, label: &str, kind: CustomButton) {
    parent.spawn((
        Button,
        kind,
        Node {
            width: Val::Px(24.0),
            height: Val::Px(24.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(Color::srgba(0.2, 0.2, 0.25, 1.0)),
        children![(
            Text::new(label.to_string()),
            TextFont {
                font_size: 14.0,
                ..default()
            },
            TextColor(Color::WHITE),
        )],
    ));
}

/// Cycle the LOCAL [`PartSelection`] on a `<` / `>` button press. The resource's
/// `Changed` flag drives `client::parts` to re-skin the local rig (visible in
/// the preview).
fn handle_button_interactions(
    mut interactions: PressedButtons,
    mut selection: ResMut<PartSelection>,
) {
    for (interaction, button) in &mut interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *button {
            CustomButton::SlotPrev(slot) => selection.advance(slot, -1),
            CustomButton::SlotNext(slot) => selection.advance(slot, 1),
        }
    }
}

/// Rewrite each slot row's variant-name label when the selection changes.
fn refresh_slot_labels(
    selection: Res<PartSelection>,
    labels: Query<(&SlotLabel, &Children)>,
    mut texts: Query<&mut Text>,
) {
    if !selection.is_changed() {
        return;
    }
    for (label, children) in &labels {
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                text.0 = selection.label(label.0).to_string();
            }
        }
    }
}

/// A/D orbit the preview camera around the player while the panel is open.
/// Reads the keyboard directly (the windowed input bridges are suspended while
/// customizing, so WASD isn't feeding movement).
fn orbit_with_ad_keys(
    state: Res<CustomizationOpen>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut orbit: ResMut<OrbitCamera>,
) {
    if !state.open {
        return;
    }
    const ORBIT_SPEED: f32 = 2.2;
    let dt = time.delta_secs();
    if keys.pressed(KeyCode::KeyA) {
        orbit.yaw -= ORBIT_SPEED * dt;
    }
    if keys.pressed(KeyCode::KeyD) {
        orbit.yaw += ORBIT_SPEED * dt;
    }
}

/// Place the [`FollowCamera`] in a third-person orbit around the local player
/// while the panel is open (overriding the first-person follow, which is gated
/// off via [`CustomizationOpen`]). Yaw 0 frames the character from the front.
fn position_preview_camera(
    state: Res<CustomizationOpen>,
    orbit: Res<OrbitCamera>,
    player: LocalPlayerTransform,
    mut cam: Query<&mut Transform, With<FollowCamera>>,
) {
    if !state.open {
        return;
    }
    let Ok(player_tf) = player.single() else {
        return;
    };
    let Ok(mut cam_tf) = cam.single_mut() else {
        return;
    };
    // Orbit a fixed distance around the player, looking at roughly chest height.
    let offset = Quat::from_axis_angle(Vec3::Y, orbit.yaw) * Vec3::new(0.0, 0.6, 2.8);
    cam_tf.translation = player_tf.translation + offset;
    cam_tf.look_at(player_tf.translation + Vec3::Y, Vec3::Y);
}
