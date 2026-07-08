//! The lobby weapon panel (protocol v4): `I` toggles a keyboard-driven list of EVERY weapon in
//! the [`WeaponCatalog`] (obelisk items from `config/items/` — new TOML entries appear here
//! automatically), Enter sends [`EquipWeaponMessage`]. Lobby phase only (the server enforces the
//! same gate), any player — the G level panel's sibling in every idiom (spawn/despawn root,
//! arrows + Enter/Escape, movement + casting suspended while open via the input-bridge gates).
//! Windowed-only.

use bevy::prelude::*;
use lightyear::prelude::MessageSender;

use crate::client::level::ClientLevel;
use crate::net::protocol::{EquipWeaponMessage, EquippedWeapon, RequestChannel};
use crate::net::weapons::WeaponCatalog;
use crate::trace;

use super::app_headless::LocalPlayerFilter;

/// Weapon-panel state. Read by the windowed input bridges (like the customizer + level select)
/// so movement/casting pause while choosing.
#[derive(Resource, Default)]
pub struct WeaponSelectOpen {
    pub open: bool,
    highlighted: usize,
    /// Weapon ids snapshotted at open (stable while the panel is up).
    choices: Vec<String>,
    root: Option<Entity>,
}

/// Marker on a panel row's text; `usize` indexes `WeaponSelectOpen.choices`.
#[derive(Component, Clone, Copy)]
struct WeaponRow(usize);

pub struct WeaponSelectPlugin;

impl Plugin for WeaponSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WeaponSelectOpen>().add_systems(
            Update,
            (weapon_select_panel, refresh_weapon_select_highlight).chain(),
        );
    }
}

/// The I-key panel driver: toggle (Lobby phase only), navigate (arrows), confirm (Enter →
/// `EquipWeaponMessage`), force-close when the phase leaves Lobby.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn weapon_select_panel(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<ClientLevel>,
    catalog: Res<WeaponCatalog>,
    equipped: Query<&EquippedWeapon, LocalPlayerFilter>,
    mut panel: ResMut<WeaponSelectOpen>,
    sender: Option<Single<&mut MessageSender<EquipWeaponMessage>>>,
    mut commands: Commands,
) {
    let close = |panel: &mut WeaponSelectOpen, commands: &mut Commands| {
        if let Some(root) = panel.root.take() {
            if let Ok(mut e) = commands.get_entity(root) {
                e.despawn();
            }
        }
        panel.open = false;
    };

    // Loadouts lock outside the lobby — force-close if the match started under us.
    let in_lobby = state.phase == 0;
    if panel.open && !in_lobby {
        close(&mut panel, &mut commands);
        return;
    }

    if keys.just_pressed(KeyCode::KeyI) {
        if panel.open {
            close(&mut panel, &mut commands);
        } else if in_lobby {
            let choices: Vec<String> = catalog.weapons.iter().map(|w| w.id.clone()).collect();
            if choices.is_empty() {
                warn!("no weapons in the catalog");
                return;
            }
            // Start highlighted on the currently-equipped weapon.
            let current = equipped.single().map(|w| w.item_id.clone()).ok();
            panel.highlighted = current
                .as_deref()
                .and_then(|id| choices.iter().position(|c| c == id))
                .unwrap_or(0);
            panel.root = Some(spawn_weapon_panel(
                &mut commands,
                &catalog,
                &choices,
                panel.highlighted,
                current.as_deref(),
            ));
            panel.choices = choices;
            panel.open = true;
        }
        return;
    }
    if !panel.open {
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        close(&mut panel, &mut commands);
        return;
    }
    let n = panel.choices.len();
    if keys.just_pressed(KeyCode::ArrowDown) {
        panel.highlighted = (panel.highlighted + 1) % n;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        panel.highlighted = (panel.highlighted + n - 1) % n;
    }
    if keys.just_pressed(KeyCode::Enter) {
        let item_id = panel.choices[panel.highlighted].clone();
        if let Some(mut sender) = sender {
            sender.send::<RequestChannel>(EquipWeaponMessage {
                item_id: item_id.clone(),
            });
            trace::event("equip_weapon_sent", serde_json::json!({ "item_id": item_id }));
        }
        close(&mut panel, &mut commands);
    }
}

/// Repaint the row highlight when the selection moves.
fn refresh_weapon_select_highlight(
    panel: Res<WeaponSelectOpen>,
    mut rows: Query<(&WeaponRow, &mut TextColor)>,
) {
    if !panel.is_changed() || !panel.open {
        return;
    }
    for (row, mut color) in &mut rows {
        *color = if row.0 == panel.highlighted {
            TextColor(Color::srgb(1.0, 0.9, 0.3))
        } else {
            TextColor(Color::srgba(0.8, 0.8, 0.85, 1.0))
        };
    }
}

/// Build the panel (the G panel's visual style): title + one row per weapon showing its name and
/// the skills it grants; the equipped one is marked.
fn spawn_weapon_panel(
    commands: &mut Commands,
    catalog: &WeaponCatalog,
    choices: &[String],
    highlighted: usize,
    equipped_id: Option<&str>,
) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(34.0),
                top: Val::Percent(28.0),
                width: Val::Px(380.0),
                padding: UiRect::all(Val::Px(12.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.78)),
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Choose weapon  (↑/↓, Enter equip, I close)"),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::srgba(0.9, 0.9, 1.0, 1.0)),
            ));
            for (i, id) in choices.iter().enumerate() {
                let (name, skills) = catalog
                    .get(id)
                    .map(|w| (w.name.clone(), w.skills.join(", ")))
                    .unwrap_or_else(|| (id.clone(), String::new()));
                let marker = if Some(id.as_str()) == equipped_id {
                    "● "
                } else {
                    "  "
                };
                root.spawn((
                    WeaponRow(i),
                    Text::new(format!("{marker}{name}  —  {skills}")),
                    TextFont {
                        font_size: 15.0,
                        ..default()
                    },
                    if i == highlighted {
                        TextColor(Color::srgb(1.0, 0.9, 0.3))
                    } else {
                        TextColor(Color::srgba(0.8, 0.8, 0.85, 1.0))
                    },
                ));
            }
        })
        .id()
}
