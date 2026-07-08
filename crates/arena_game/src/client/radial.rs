//! The radial skill wheel — wisp's `ui/radial_menu.rs` ported 1:1 (constants, layout math,
//! hit-test, colors, interaction model), selecting among the EQUIPPED WEAPON's skills.
//!
//! Interaction (wisp-identical): HOLD `F` to open — the cursor is confined + hidden + warped to
//! screen center; move the mouse to pick (direction from center, 5px deadzone, nearest-sector
//! snap — no "between segments" gap); RELEASE `F` to confirm (release inside the deadzone =
//! cancel, no Escape handling). No pause, no time scaling: movement/look/cast are suppressed
//! while open (the bridges + controller gate on [`RadialWheel::open`], the arena's equivalent of
//! wisp's input-context switch). Weapon choice is a different surface (the lobby's I panel) —
//! the wheel picks a skill FROM the current weapon, exactly like wisp's.
//!
//! Windowed-only: the headless client selects via `ARENA_AUTOCAST_SKILL`.

use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::client::net::SelectedSkill;
use crate::net::protocol::EquippedWeapon;
use crate::net::weapons::WeaponCatalog;

use super::app_headless::LocalPlayerFilter;

/// wisp's wheel constants, verbatim.
const MAX_SEGMENTS: usize = 8;
const RING_RADIUS: f32 = 140.0;
const SEGMENT_SIZE: f32 = 88.0;

/// Wheel state. `open` is read by the input bridges + the controller (movement/cast/look are
/// suppressed while the wheel is up). `skills` is snapshotted at open so a mid-hold weapon
/// replication can't reshuffle the segments under the cursor.
#[derive(Resource, Default)]
pub struct RadialWheel {
    pub open: bool,
    selected: Option<usize>,
    skills: Vec<String>,
    root: Option<Entity>,
}

#[derive(Component, Clone, Copy)]
struct RadialSegment {
    index: usize,
}

pub struct RadialWheelPlugin;

impl Plugin for RadialWheelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RadialWheel>().add_systems(
            Update,
            (open_on_hold, track_cursor, confirm_on_release).chain(),
        );
    }
}

/// Segment `index`'s ring angle: slot 0 at 12 o'clock, stepping counter-clockwise on screen
/// (wisp's `segment_angle`, verbatim).
fn segment_angle(index: usize, segment_count: usize) -> f32 {
    let step = std::f32::consts::TAU / segment_count.max(1) as f32;
    std::f32::consts::FRAC_PI_2 + step * index as f32
}

/// Map the cursor's offset-from-center to a segment: 5px deadzone (`length_squared < 25`), then
/// a nearest-sector-center snap — the full circle always maps to the closest slot (wisp's
/// `pick_segment`, verbatim).
fn pick_segment(offset: Vec2, segment_count: usize) -> Option<usize> {
    if offset.length_squared() < 25.0 {
        return None;
    }
    let angle = (-offset.y).atan2(offset.x);
    let step = std::f32::consts::TAU / segment_count.max(1) as f32;
    let normalized =
        (angle - std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU) % std::f32::consts::TAU;
    Some(((normalized / step).round() as usize) % segment_count)
}

/// Display label for a skill id: `chain_lightning` → `Chain Lightning`.
fn skill_label(id: &str) -> String {
    id.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// HOLD `F` opens the wheel over the equipped weapon's skills (guard: not already open, a local
/// player with a weapon exists). Cursor → confined + hidden + warped to screen center; selection
/// reads the invisible cursor's offset from that center (wisp's `on_open_radial`).
#[allow(clippy::type_complexity)]
fn open_on_hold(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: ResMut<RadialWheel>,
    weapon: Query<&EquippedWeapon, LocalPlayerFilter>,
    catalog: Option<Res<WeaponCatalog>>,
    selected: Res<SelectedSkill>,
    mut window: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::KeyF) || wheel.open {
        return;
    }
    let Ok(weapon) = weapon.single() else {
        return;
    };
    if weapon.skills.is_empty() {
        return;
    }
    let Ok((mut window, mut cursor)) = window.single_mut() else {
        return;
    };

    wheel.skills = weapon.skills.iter().take(MAX_SEGMENTS).cloned().collect();
    wheel.selected = None;
    wheel.open = true;

    cursor.grab_mode = CursorGrabMode::Confined;
    cursor.visible = false;
    let center = Vec2::new(window.width(), window.height()) * 0.5;
    window.set_cursor_position(Some(center));

    // Center label: "Weapon · CurrentSkill" (wisp's center box).
    let weapon_name = catalog
        .as_deref()
        .and_then(|c| c.get(&weapon.item_id))
        .map(|w| w.name.clone())
        .unwrap_or_else(|| weapon.item_id.clone());
    let center_text = format!("{} · {}", weapon_name, skill_label(&selected.0));

    let segment_count = wheel.skills.len().max(1);
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            // Full-screen scrim (wisp: srgba(0,0,0,0.25)).
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.25)),
        ))
        .with_children(|root| {
            // Zero-size anchor at screen center; segments hang off it absolutely.
            root.spawn(Node {
                width: Val::Px(0.0),
                height: Val::Px(0.0),
                ..default()
            })
            .with_children(|anchor| {
                for (index, skill) in wheel.skills.iter().enumerate() {
                    let angle = segment_angle(index, segment_count);
                    let dx = angle.cos() * RING_RADIUS;
                    let dy = -angle.sin() * RING_RADIUS;
                    anchor.spawn((
                        RadialSegment { index },
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(dx - SEGMENT_SIZE * 0.5),
                            top: Val::Px(dy - SEGMENT_SIZE * 0.5),
                            width: Val::Px(SEGMENT_SIZE),
                            height: Val::Px(SEGMENT_SIZE),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.1, 0.1, 0.15, 0.85)),
                        children![(
                            Text::new(skill_label(skill)),
                            TextFont {
                                font_size: 16.0,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        )],
                    ));
                }
                // Center label box (wisp: 220×48, srgba(0,0,0,0.6), 18px white).
                anchor.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(-110.0),
                        top: Val::Px(-24.0),
                        width: Val::Px(220.0),
                        height: Val::Px(48.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
                    children![(
                        Text::new(center_text),
                        TextFont {
                            font_size: 18.0,
                            ..default()
                        },
                        TextColor(Color::WHITE),
                    )],
                ));
            });
        })
        .id();
    wheel.root = Some(root);
}

/// Per-frame while open: offset from center → picked segment → recolor (wisp's `track_cursor`;
/// selected = orange 0.85, others = the dark slate default).
fn track_cursor(
    mut wheel: ResMut<RadialWheel>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut segments: Query<(&RadialSegment, &mut BackgroundColor)>,
) {
    if !wheel.open {
        return;
    }
    let Ok(window) = window.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let center = Vec2::new(window.width(), window.height()) * 0.5;
    let offset = cursor_pos - center;
    wheel.selected = pick_segment(offset, wheel.skills.len().max(1));

    for (segment, mut bg) in &mut segments {
        *bg = if Some(segment.index) == wheel.selected {
            BackgroundColor(Color::from(bevy::color::palettes::css::ORANGE).with_alpha(0.85))
        } else {
            BackgroundColor(Color::srgba(0.1, 0.1, 0.15, 0.85))
        };
    }
}

/// RELEASE `F` confirms: a picked segment writes [`SelectedSkill`]; release inside the deadzone
/// (nothing picked) just closes (wisp's `detect_release`). Cursor returns to the FPS default
/// (locked + hidden).
fn confirm_on_release(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: ResMut<RadialWheel>,
    mut selected: ResMut<SelectedSkill>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut commands: Commands,
) {
    if !wheel.open || !keys.just_released(KeyCode::KeyF) {
        return;
    }
    if let Some(index) = wheel.selected {
        if let Some(skill) = wheel.skills.get(index) {
            selected.0 = skill.clone();
        }
    }
    if let Some(root) = wheel.root.take() {
        if let Ok(mut ec) = commands.get_entity(root) {
            ec.despawn();
        }
    }
    wheel.open = false;
    if let Ok(mut cursor) = cursor.single_mut() {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wisp's hit-test, pinned: 5px deadzone → None; straight up picks slot 0 (12 o'clock);
    /// nearest-sector snap partitions the full circle (no gaps).
    #[test]
    fn pick_segment_matches_wisp() {
        // Deadzone (5px): no pick.
        assert_eq!(pick_segment(Vec2::new(3.0, -3.0), 3), None);
        // Straight UP on screen = negative y = slot 0.
        assert_eq!(pick_segment(Vec2::new(0.0, -100.0), 3), Some(0));
        // 3 segments step CCW from 12 o'clock: slot 1 at 210° screen-left-down, slot 2 right-down.
        assert_eq!(pick_segment(Vec2::new(-100.0, 58.0), 3), Some(1));
        assert_eq!(pick_segment(Vec2::new(100.0, 58.0), 3), Some(2));
        // Two segments: up = 0, down = 1.
        assert_eq!(pick_segment(Vec2::new(0.0, 100.0), 2), Some(1));
        // Just past the deadzone still picks (full-circle partition, no dead angles).
        assert_eq!(pick_segment(Vec2::new(0.0, -6.0), 3), Some(0));
    }

    #[test]
    fn skill_labels_prettify() {
        assert_eq!(skill_label("chain_lightning"), "Chain Lightning");
        assert_eq!(skill_label("firebolt"), "Firebolt");
    }
}
