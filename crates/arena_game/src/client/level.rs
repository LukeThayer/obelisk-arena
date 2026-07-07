//! Client-side level sync + lobby UX (levels-and-lobby design).
//!
//! The server broadcasts the authoritative level id in `RoundStateMessage.level`; this module
//! mirrors it locally: on change it despawns every [`LevelEntity`] and spawns the named level from
//! the SAME `.scn.ron` the server loaded (all peers ship identical level files) — physics on every
//! peer, meshes/materials/lights only on the windowed client.
//!
//! The `RoundStateMessage` single-drain rule (invariant §8) is preserved via a FAN-OUT: the two
//! existing drains (windowed `hud::receive_round_state`, headless
//! `app_headless::trace_replicated_round_state`) re-emit each message as a local
//! [`RoundStateChanged`] Bevy message; this module (and anything else) reads the fan-out, never
//! the `MessageReceiver`.
//!
//! Also home to the host's level-select panel (G key, windowed, host + Lobby phase only — the
//! K-customizer's sibling) and the `ARENA_AUTOSTART_LEVEL` headless hook (the harness's stand-in
//! for pressing G).

use bevy::prelude::*;
use lightyear::prelude::MessageSender;

use arena_sim::level::{load_level_scene, spawn_level, LevelCatalog, LevelEntity};

use crate::net::client::ConnectTo;
use crate::net::protocol::{RequestChannel, RoundStateMessage, StartMatchMessage};
use crate::trace;

use super::app_headless::{LocalPlayerFilter, RemotePlayerFilter};

/// Local fan-out of each received `RoundStateMessage` (see the module doc). ONE writing site per
/// app (the app's single drain); any number of readers.
#[derive(bevy::prelude::Message, Clone, Debug)]
pub struct RoundStateChanged(pub RoundStateMessage);

/// The latest round-state snapshot relevant to level flow: which level SHOULD be loaded (server
/// truth), which is CURRENTLY loaded locally, the phase, and the elected host.
#[derive(Resource, Debug, Default)]
pub struct ClientLevel {
    /// The locally-loaded level id (None until the first sync).
    pub current: Option<String>,
    /// Last received wire phase tag (0 lobby, 1 countdown, 2 active, 3 round-over, 4 match-over).
    pub phase: u8,
    /// Last received elected-host client id (0 = none).
    pub host: u64,
}

/// Whether this peer renders level visuals (meshes/materials/lights) in addition to physics.
#[derive(Resource, Clone, Copy)]
struct ClientLevelConfig {
    windowed: bool,
}

/// Client level sync + lobby UX. `windowed: true` additionally spawns level visuals and registers
/// the G-key level-select panel; `false` (headless observer) is physics + state only.
pub struct ClientLevelPlugin {
    pub windowed: bool,
}

impl Plugin for ClientLevelPlugin {
    fn build(&self, app: &mut App) {
        // Same roots as the server's scan — all peers ship the same level files.
        let root = crate::arena_root();
        let catalog = LevelCatalog::scan_roots(&[
            root.join("assets/scenes"),
            root.join("crates/arena_editor/assets/scenes"),
        ]);
        app.insert_resource(catalog)
            .insert_resource(ClientLevelConfig {
                windowed: self.windowed,
            })
            .init_resource::<ClientLevel>()
            .add_message::<RoundStateChanged>()
            .add_systems(Update, sync_level_from_round_state);
        if self.windowed {
            app.init_resource::<LevelSelectOpen>().add_systems(
                Update,
                (level_select_panel, refresh_level_select_highlight).chain(),
            );
        }
        // [H]/[W] AUTOSTART hook: the HOST peer requests a match on the named level whenever the
        // lobby is ready — the harness's (or a keyboard-less soak's) stand-in for pressing G.
        if std::env::var("ARENA_AUTOSTART_LEVEL").is_ok() {
            app.add_systems(Update, autostart_level);
        }
    }
}

/// Mirror the server's level locally: read the [`RoundStateChanged`] fan-out, and when
/// `msg.level` differs from what's loaded, despawn every [`LevelEntity`] and spawn the named
/// level — visuals iff windowed. Also snapshots phase/host for the panel + autostart.
fn sync_level_from_round_state(
    mut changes: MessageReader<RoundStateChanged>,
    mut state: ResMut<ClientLevel>,
    config: Res<ClientLevelConfig>,
    catalog: Res<LevelCatalog>,
    level_entities: Query<Entity, With<LevelEntity>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut commands: Commands,
) {
    let mut target: Option<String> = None;
    for RoundStateChanged(msg) in changes.read() {
        state.phase = msg.phase;
        state.host = msg.host;
        if !msg.level.is_empty() && state.current.as_deref() != Some(msg.level.as_str()) {
            target = Some(msg.level.clone());
        }
    }
    let Some(target) = target else { return };
    let Some(info) = catalog.get(&target) else {
        warn!("server says level '{target}' but it's not in the local catalog — out of sync?");
        return;
    };
    let scene = match load_level_scene(&info.path) {
        Ok(s) => s,
        Err(e) => {
            warn!("level '{target}' failed to load locally: {e}");
            return;
        }
    };
    for e in &level_entities {
        commands.entity(e).despawn();
    }
    // Visuals only when windowed AND the render asset stores exist (headless registers neither
    // StandardMaterial nor a renderer).
    let spawned = match (config.windowed, meshes, materials) {
        (true, Some(mut meshes), Some(mut materials)) => spawn_level(
            &mut commands,
            &scene,
            Some((meshes.as_mut(), materials.as_mut())),
        ),
        _ => spawn_level(&mut commands, &scene, None),
    };
    trace::event(
        "level_loaded",
        serde_json::json!({
            "id": target,
            "statics": scene.statics.len(),
            "spawns": scene.spawns.len(),
            "entities": spawned.len(),
        }),
    );
    state.current = Some(target);
}

// ---------------------------------------------------------------------------------------------
// G-key level-select panel (windowed, host + Lobby only)
// ---------------------------------------------------------------------------------------------

/// Level-select panel state. Read by the windowed input bridges (like `CustomizationOpen`) so
/// movement/casting pause while choosing.
#[derive(Resource, Default)]
pub struct LevelSelectOpen {
    pub open: bool,
    highlighted: usize,
    /// The selectable level ids snapshotted at open (stable while the panel is up).
    choices: Vec<String>,
    root: Option<Entity>,
}

/// Marker on a panel row's text; `usize` is the row's index into `LevelSelectOpen.choices`.
#[derive(Component, Clone, Copy)]
struct LevelRow(usize);

/// The G-key panel driver: toggle (host + Lobby only), navigate (arrows), confirm (Enter →
/// `StartMatchMessage`), and force-close when the phase leaves Lobby (e.g. the match started).
#[allow(clippy::too_many_arguments)]
fn level_select_panel(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<ClientLevel>,
    me: Res<ConnectTo>,
    catalog: Res<LevelCatalog>,
    mut panel: ResMut<LevelSelectOpen>,
    sender: Option<Single<&mut MessageSender<StartMatchMessage>>>,
    mut commands: Commands,
) {
    let close = |panel: &mut LevelSelectOpen, commands: &mut Commands| {
        if let Some(root) = panel.root.take() {
            if let Ok(mut e) = commands.get_entity(root) {
                e.despawn();
            }
        }
        panel.open = false;
    };

    let is_lobby_host = state.phase == 0 && state.host != 0 && state.host == me.client_id;

    // The panel is only meaningful for the lobby-phase host; force-close otherwise (phase moved,
    // host migrated).
    if panel.open && !is_lobby_host {
        close(&mut panel, &mut commands);
        return;
    }

    if keys.just_pressed(KeyCode::KeyG) {
        if panel.open {
            close(&mut panel, &mut commands);
        } else if is_lobby_host {
            let choices: Vec<String> = catalog.selectable().map(|l| l.id.clone()).collect();
            if choices.is_empty() {
                warn!("no selectable levels in the catalog");
                return;
            }
            panel.highlighted = 0;
            panel.root = Some(spawn_level_panel(&mut commands, &choices, 0));
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
        let level = panel.choices[panel.highlighted].clone();
        if let Some(mut sender) = sender {
            sender.send::<RequestChannel>(StartMatchMessage {
                level: level.clone(),
            });
            trace::event("start_match_sent", serde_json::json!({ "level": level }));
        }
        close(&mut panel, &mut commands);
    }
}

/// Repaint the row highlight when the selection moves.
fn refresh_level_select_highlight(
    panel: Res<LevelSelectOpen>,
    mut rows: Query<(&LevelRow, &mut TextColor)>,
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

/// Build the panel node tree (the K-customizer's visual style): title + one row per level.
fn spawn_level_panel(commands: &mut Commands, choices: &[String], highlighted: usize) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(38.0),
                top: Val::Percent(30.0),
                width: Val::Px(280.0),
                padding: UiRect::all(Val::Px(12.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.78)),
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Choose arena  (↑/↓, Enter start, G close)"),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::srgba(0.9, 0.9, 1.0, 1.0)),
            ));
            for (i, id) in choices.iter().enumerate() {
                root.spawn((
                    LevelRow(i),
                    Text::new(format!("  {id}")),
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

// ---------------------------------------------------------------------------------------------
// Headless autostart
// ---------------------------------------------------------------------------------------------

/// [H] `ARENA_AUTOSTART_LEVEL=<id>`: the HOST peer sends `StartMatchMessage { <id> }` whenever the
/// lobby is ready (both players present), once per lobby visit — the harness's stand-in for
/// pressing G. Non-hosts (and non-lobby phases) no-op; the flag re-arms when the phase leaves
/// Lobby, so a MatchOver→Lobby return starts the next match too (keeps long soaks running).
fn autostart_level(
    state: Res<ClientLevel>,
    me: Res<ConnectTo>,
    local: Query<(), LocalPlayerFilter>,
    remotes: Query<(), RemotePlayerFilter>,
    sender: Option<Single<&mut MessageSender<StartMatchMessage>>>,
    mut sent_this_lobby: Local<bool>,
) {
    if state.phase != 0 {
        *sent_this_lobby = false; // left the lobby — re-arm for the next visit
        return;
    }
    if *sent_this_lobby
        || state.host == 0
        || state.host != me.client_id
        || local.iter().next().is_none()
        || remotes.iter().next().is_none()
    {
        return;
    }
    let Ok(level) = std::env::var("ARENA_AUTOSTART_LEVEL") else {
        return;
    };
    let Some(mut sender) = sender else { return };
    sender.send::<RequestChannel>(StartMatchMessage {
        level: level.clone(),
    });
    *sent_this_lobby = true;
    trace::event("start_match_sent", serde_json::json!({ "level": level }));
}
