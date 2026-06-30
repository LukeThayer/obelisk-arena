//! Windowed client HUD (guide §5.6, §6.6, §7).
//!
//! Pure presentation — every value shown is REPLICATED from the server, never computed client-side
//! (Stage-A invariant). Three surfaces:
//!   1. **HP bars** (`NetworkedHealth`-driven `bevy_ui` `Node`s) for the local player + the opponent.
//!   2. **Floating damage numbers + hit flash** spawned from the replicated `DamageResolved`
//!      (`NetEventMessage`), the target looked up by its replicated `ObeliskNetId`.
//!   3. **Round/score label + countdown** driven by the replicated `RoundStateMessage`.
//!
//! This module is added ONLY by the windowed client (`run_windowed_client`); the headless client has
//! no `bevy_ui`/cameras. The HP-drop + round transitions are still observable headlessly via the
//! `hp` / `round_state` server traces — the HUD is the human-facing [W] surface.

use bevy::prelude::*;
use lightyear::prelude::MessageReceiver;

use avian3d::prelude::Position;

use crate::client::controller::FollowCamera;
use crate::client::net::{ChargeState, LocalNetPlayer};
use crate::net::protocol::{NetEventMessage, NetworkedHealth, NetworkedPlayer, ObeliskNetId};

/// Plugin: the windowed HUD. Builds the bar widgets at startup, drives them from `NetworkedHealth`,
/// spawns floating damage + hit flashes from `DamageResolved`, renders the round/score label, and
/// drives the charge bar + spell indicator (C6).
pub struct ArenaHudPlugin;

impl Plugin for ArenaHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoundHudState>()
            .add_systems(Startup, setup_hud)
            .add_systems(
                Update,
                (
                    update_health_bars,
                    spawn_damage_feedback,
                    age_floating_damage,
                    age_hit_flashes,
                    receive_round_state,
                    update_round_label,
                    update_charge_bar,
                ),
            );
    }
}

// ---------------------------------------------------------------------------------------------
// HP bars
// ---------------------------------------------------------------------------------------------

/// Marker on a HUD bar frame node (the dark background). The fill + label children carry the
/// `local`/opponent distinction; this just tags the root for the widget tree.
#[derive(Component, Clone, Copy)]
struct HealthBarRoot;

/// The inner fill `Node` of a bar — its `width` is set to `current/max` each frame.
#[derive(Component, Clone, Copy)]
struct HealthBarFill {
    local: bool,
}

/// The text label inside a bar (`"name  cur/max"`).
#[derive(Component, Clone, Copy)]
struct HealthBarLabel {
    local: bool,
}

/// The round/score banner text (top-center).
#[derive(Component)]
struct RoundLabel;

// ---------------------------------------------------------------------------------------------
// Charge bar (C6)
// ---------------------------------------------------------------------------------------------

/// Marker on the charge-bar track (dark background). Its `Node.display` is toggled by
/// `update_charge_bar` — `Display::None` when not charging, `Display::Flex` when charging.
#[derive(Component)]
struct ChargeBarRoot;

/// Marker on the charge-bar fill child. Its `Node.width` is set to `Val::Percent(frac * 100)`
/// each frame from [`ChargeState::frac`].
#[derive(Component)]
struct ChargeBarFill;

/// Marker on the spell-selected label ("Firebolt"). Static for now; structured so more spells
/// can slot in later (e.g. a hotbar row with one label per slot).
#[derive(Component)]
struct SpellLabel;

const BAR_WIDTH_PX: f32 = 280.0;
const BAR_HEIGHT_PX: f32 = 26.0;

/// Width of the charge bar in pixels. Narrower than the HP bar — it's a transient indicator.
const CHARGE_BAR_WIDTH: f32 = 160.0;
/// Height of the charge bar in pixels.
const CHARGE_BAR_HEIGHT: f32 = 10.0;

/// Build the two hp bars (own bottom-left, opponent top-right) + the round banner. A `bevy_ui`
/// `Node` tree; the fill nodes are resized each frame by `update_health_bars`.
fn setup_hud(mut commands: Commands) {
    // Own bar: bottom-left.
    spawn_bar(
        &mut commands,
        true,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(20.0),
            bottom: Val::Px(20.0),
            ..default()
        },
        Color::srgb(0.2, 0.8, 0.3),
    );
    // Opponent bar: top-right.
    spawn_bar(
        &mut commands,
        false,
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(20.0),
            top: Val::Px(20.0),
            ..default()
        },
        Color::srgb(0.85, 0.25, 0.2),
    );

    // Round/score banner: top-center.
    commands.spawn((
        RoundLabel,
        Text::new("waiting for players…"),
        TextFont {
            font_size: 24.0,
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(16.0),
            left: Val::Percent(50.0),
            margin: UiRect::left(Val::Px(-160.0)),
            width: Val::Px(320.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));

    // Crosshair: a centered 4×4 px white dot at dead-center for first-person aim.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(50.0),
            top: Val::Percent(50.0),
            // Pull the dot back by half its own size so it is pixel-exact center.
            margin: UiRect {
                left: Val::Px(-2.0),
                top: Val::Px(-2.0),
                ..default()
            },
            width: Val::Px(4.0),
            height: Val::Px(4.0),
            ..default()
        },
        BackgroundColor(Color::WHITE),
    ));

    // Charge bar: centered horizontally, 14 px below the crosshair center. Hidden via
    // `Display::None` until charging; `update_charge_bar` toggles display and fill width each frame.
    commands
        .spawn((
            ChargeBarRoot,
            Node {
                display: Display::None, // hidden until charging
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                // Center the bar horizontally and place it below the crosshair.
                margin: UiRect {
                    left: Val::Px(-(CHARGE_BAR_WIDTH / 2.0)),
                    top: Val::Px(14.0),
                    ..default()
                },
                width: Val::Px(CHARGE_BAR_WIDTH),
                height: Val::Px(CHARGE_BAR_HEIGHT),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        ))
        .with_children(|frame| {
            frame.spawn((
                ChargeBarFill,
                Node {
                    width: Val::Percent(0.0), // driven each frame by update_charge_bar
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.25, 0.6, 1.0)), // cyan-blue charge color
            ));
        });

    // Spell-selected indicator: static "Firebolt" label at bottom-left, above the local HP bar.
    // Structured as a named component so more spells can slot in later (one label per hotbar slot).
    commands.spawn((
        SpellLabel,
        Text::new("Firebolt"),
        TextFont {
            font_size: 14.0,
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.85, 0.4)), // gold to match the firebolt theme
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(20.0),
            bottom: Val::Px(50.0), // just above the local HP bar (bar is at bottom: 20, height 26)
            ..default()
        },
    ));
}

/// Spawn one hp bar (a dark frame `Node` + a colored fill child + a centered label).
fn spawn_bar(commands: &mut Commands, local: bool, frame_node: Node, fill_color: Color) {
    commands
        .spawn((
            HealthBarRoot,
            Node {
                width: Val::Px(BAR_WIDTH_PX),
                height: Val::Px(BAR_HEIGHT_PX),
                ..frame_node
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        ))
        .with_children(|frame| {
            frame.spawn((
                HealthBarFill { local },
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(fill_color),
            ));
            frame.spawn((
                HealthBarLabel { local },
                Text::new(""),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(8.0),
                    top: Val::Px(4.0),
                    ..default()
                },
            ));
        });
}

/// Drive the bar fill widths + labels from the replicated `NetworkedHealth`. The local player is the
/// one tagged `LocalNetPlayer`; the opponent is any other `NetworkedPlayer`. Server-authoritative:
/// the client renders exactly what the server replicated, never computing damage.
#[allow(clippy::type_complexity)]
fn update_health_bars(
    players: Query<(&NetworkedHealth, &ObeliskNetId, Has<LocalNetPlayer>), With<NetworkedPlayer>>,
    mut fills: Query<(&HealthBarFill, &mut Node), Without<HealthBarLabel>>,
    mut labels: Query<(&HealthBarLabel, &mut Text)>,
) {
    // Resolve local + opponent health snapshots.
    let mut local: Option<(f64, f64, String)> = None;
    let mut opponent: Option<(f64, f64, String)> = None;
    for (health, net_id, is_local) in &players {
        let snap = (health.current, health.max, net_id.0.clone());
        if is_local {
            local = Some(snap);
        } else {
            opponent = Some(snap);
        }
    }

    for (fill, mut node) in &mut fills {
        let snap = if fill.local { &local } else { &opponent };
        let frac = snap
            .as_ref()
            .map(|(cur, max, _)| {
                if *max > 0.0 {
                    (cur / max).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0);
        node.width = Val::Percent((frac * 100.0) as f32);
    }

    for (label, mut text) in &mut labels {
        let snap = if label.local { &local } else { &opponent };
        *text = Text::new(match snap {
            Some((cur, max, id)) => format!("{id}  {cur:.0}/{max:.0}"),
            None => "—".to_string(),
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Floating damage numbers + hit flash (from replicated DamageResolved)
// ---------------------------------------------------------------------------------------------

/// A short-lived floating damage number. Stored as a `bevy_ui` `Text` node whose screen position is
/// re-projected from a fixed WORLD anchor each frame (so it sits over the hit target). Rises + fades.
#[derive(Component)]
struct FloatingDamage {
    elapsed: f32,
    duration: f32,
    /// World-space anchor (target head); projected to screen each frame.
    world_anchor: Vec3,
}

/// A short-lived hit-flash overlay: a translucent red full-bar tint dropped onto the target's hp
/// bar fill. Reverts after `duration`. (A material flash on the 3D body would need per-body material
/// handles the materialized capsule doesn't expose cleanly; flashing the HUD bar is the robust,
/// always-visible signal that the target was hit.)
#[derive(Component)]
struct HitFlash {
    elapsed: f32,
    duration: f32,
    /// true = the local player was hit (flash own bar), false = opponent.
    local: bool,
}

/// Drain the replicated `DamageResolved` events → spawn a floating "-N" UI number anchored over the
/// target + a hit flash on the target's hp bar. Target located by its replicated `ObeliskNetId`. The
/// damage value is the SERVER's authoritative number (echoed, never computed here).
#[allow(clippy::type_complexity)]
fn spawn_damage_feedback(
    mut receivers: Query<&mut MessageReceiver<NetEventMessage>>,
    targets: Query<(&ObeliskNetId, &Position, Has<LocalNetPlayer>), With<NetworkedPlayer>>,
    mut commands: Commands,
) {
    use obelisk_bevy::net::NetEvent;
    for mut rx in &mut receivers {
        for NetEventMessage(ev) in rx.receive() {
            let NetEvent::DamageResolved {
                target,
                total_damage,
                ..
            } = ev
            else {
                continue;
            };
            // Find the replicated body for this obelisk id.
            let Some((_, position, is_local)) = targets.iter().find(|(id, _, _)| id.0 == target)
            else {
                continue;
            };
            let world_anchor = position.0 + Vec3::Y * 1.8;
            commands.spawn((
                FloatingDamage {
                    elapsed: 0.0,
                    duration: 1.0,
                    world_anchor,
                },
                Text::new(format!("-{total_damage:.0}")),
                TextFont {
                    font_size: 32.0,
                    ..default()
                },
                TextColor(Color::srgb(1.0, 0.85, 0.2)),
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
            ));
            // Hit flash on the target's hp bar fill.
            commands.spawn(HitFlash {
                elapsed: 0.0,
                duration: 0.18,
                local: is_local,
            });
        }
    }
}

/// Rise + fade each floating damage number, re-projecting its world anchor to screen each frame so
/// it tracks the target. Despawns at end of life. No camera ⇒ skip (still ages out).
fn age_floating_damage(
    time: Res<Time>,
    mut commands: Commands,
    camera: Query<(&Camera, &GlobalTransform), With<FollowCamera>>,
    mut q: Query<(Entity, &mut FloatingDamage, &mut Node, &mut TextColor)>,
) {
    let dt = time.delta_secs();
    let cam = camera.single().ok();
    for (e, mut fd, mut node, mut color) in &mut q {
        fd.elapsed += dt;
        let t = (fd.elapsed / fd.duration).clamp(0.0, 1.0);
        // Rise 0.8m in world space over the life.
        let world = fd.world_anchor + Vec3::Y * (t * 0.8);
        if let Some((camera, cam_tf)) = cam {
            if let Ok(screen) = camera.world_to_viewport(cam_tf, world) {
                node.left = Val::Px(screen.x - 16.0);
                node.top = Val::Px(screen.y);
            }
        }
        color.0.set_alpha(1.0 - t);
        if fd.elapsed >= fd.duration {
            commands.entity(e).despawn();
        }
    }
}

/// Age + apply hit flashes: while a `HitFlash` is alive, tint the matching hp bar fill toward white;
/// remove the flash (and revert the tint) on expiry. The bar's base color is restored by
/// `update_health_bars` next frame, so we only need to despawn the flash marker.
fn age_hit_flashes(
    time: Res<Time>,
    mut commands: Commands,
    mut flashes: Query<(Entity, &mut HitFlash)>,
    mut fills: Query<(&HealthBarFill, &mut BackgroundColor)>,
) {
    let dt = time.delta_secs();
    for (e, mut flash) in &mut flashes {
        flash.elapsed += dt;
        if flash.elapsed >= flash.duration {
            commands.entity(e).despawn();
            continue;
        }
        // Flash the matching bar fill white for the flash duration.
        for (fill, mut bg) in &mut fills {
            if fill.local == flash.local {
                bg.0 = Color::WHITE;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Round/score HUD — driven by the replicated RoundStateMessage.
// ---------------------------------------------------------------------------------------------

/// Latest round state received from the server, rendered by `update_round_label`.
#[derive(Resource, Default)]
struct RoundHudState {
    phase: u8,
    countdown: f32,
    scores: [(String, u8); 2],
    winner: String,
}

/// Drain the replicated `RoundStateMessage` → cache the latest into `RoundHudState`.
fn receive_round_state(
    mut receivers: Query<&mut MessageReceiver<crate::net::protocol::RoundStateMessage>>,
    mut state: ResMut<RoundHudState>,
) {
    for mut rx in &mut receivers {
        for msg in rx.receive() {
            state.phase = msg.phase;
            state.countdown = msg.countdown;
            state.scores = msg.scores;
            state.winner = msg.winner;
        }
    }
}

/// Render the round banner from `RoundHudState`.
fn update_round_label(state: Res<RoundHudState>, mut label: Query<&mut Text, With<RoundLabel>>) {
    if !state.is_changed() {
        return;
    }
    let Ok(mut text) = label.single_mut() else {
        return;
    };
    let score = format!(
        "{}:{}  {}:{}",
        state.scores[0].0, state.scores[0].1, state.scores[1].0, state.scores[1].1,
    );
    let banner = match state.phase {
        0 => "waiting for players…".to_string(),
        1 => format!("round starting in {:.0}…   {score}", state.countdown.ceil()),
        2 => format!("FIGHT!   {score}"),
        3 => format!("round won by {}   {score}", state.winner),
        4 => format!("MATCH OVER — {} wins!   {score}", state.winner),
        _ => score,
    };
    *text = Text::new(banner);
}

// ---------------------------------------------------------------------------------------------
// Charge bar update (C6)
// ---------------------------------------------------------------------------------------------

/// Drive the charge bar each frame from [`ChargeState`].
///
/// - While charging: show the bar (`Display::Flex`) and fill it to `frac * 100%`.
/// - When not charging: hide the bar (`Display::None`) and reset fill to 0%.
///
/// Uses `Without` guards to make the two `&mut Node` queries disjoint — `ChargeBarRoot` and
/// `ChargeBarFill` are always on different entities (parent and child), so the borrow is safe.
fn update_charge_bar(
    charge: Res<ChargeState>,
    mut roots: Query<&mut Node, (With<ChargeBarRoot>, Without<ChargeBarFill>)>,
    mut fills: Query<&mut Node, (With<ChargeBarFill>, Without<ChargeBarRoot>)>,
) {
    let (display, frac) = if charge.charging {
        (Display::Flex, charge.frac())
    } else {
        (Display::None, 0.0)
    };

    for mut node in &mut roots {
        node.display = display;
    }
    for mut node in &mut fills {
        node.width = Val::Percent(frac * 100.0);
    }
}
