//! Test/verification harness hooks shared by the windowed + headless clients.
//!
//! These are the `ARENA_*`-gated, non-gameplay scaffolds the screenshot/smoke/net-test rigs hang
//! off of: deterministic frame-count exit ([`SmokeExit`]), off-screen-window capture
//! ([`ScreenshotConfig`]), the render-smoothing observer for the predicted local body
//! ([`add_frame_interpolation_to_predicted`]), and [`EnvConfig`] — the SINGLE parser for the
//! camera-aim env hooks (`ARENA_CAM_YAW` / `ARENA_TEST_PITCH`) used by BOTH the windowed
//! `ArenaControllerPlugin` and the headless client, so the twice-read values can never drift.

use avian3d::prelude::{Position, Rotation};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use lightyear::prediction::correction::VisualCorrection;
use lightyear::prelude::Predicted;
use lightyear_frame_interpolation::FrameInterpolate;
use std::path::PathBuf;

/// Parse an `f32` env var, falling back to `default` if unset or unparseable.
fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

/// Parse a `u64` env var, falling back to `default` if unset or unparseable.
fn env_frame(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

/// Shared parse of the camera-aim env hooks, used by BOTH the windowed `ArenaControllerPlugin`
/// (`controller.rs`) and the headless client (`app_headless.rs`) so the twice-read `ARENA_CAM_YAW` /
/// `ARENA_TEST_PITCH` values can never drift apart.
///
/// - `cam_yaw`: `ARENA_CAM_YAW` (radians), default `0.0`.
/// - `test_pitch` / `test_pitch_locked`: `ARENA_TEST_PITCH` (radians) — when set+parseable, the pitch
///   is pinned and `locked` is true (the controller then skips the mouse-Y update so the forced lean
///   holds for screenshots); when unset/unparseable, `(0.0, false)`. The headless client reads only
///   the pitch value.
pub(super) struct EnvConfig {
    pub(super) cam_yaw: f32,
    pub(super) test_pitch: f32,
    pub(super) test_pitch_locked: bool,
}

impl EnvConfig {
    pub(super) fn from_env() -> Self {
        // ARENA_TEST_PITCH: when set+parseable, pin the pitch and mark it locked.
        let (test_pitch, test_pitch_locked) = std::env::var("ARENA_TEST_PITCH")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .map(|p| (p, true))
            .unwrap_or((0.0, false));
        Self {
            cam_yaw: env_f32("ARENA_CAM_YAW", 0.0),
            test_pitch,
            test_pitch_locked,
        }
    }
}

/// Insert `FrameInterpolate<Position/Rotation>` on a newly-`Predicted` player so its render is
/// smoothed between FixedUpdate ticks. Triggered on `Add<Position>` (avian adds Position after the
/// RigidBody, by which point `Predicted` is present), mirroring the avian_3d_character renderer.
pub(super) fn add_frame_interpolation_to_predicted(
    trigger: On<Add, Position>,
    query: Query<(), With<Predicted>>,
    mut commands: Commands,
) {
    if !query.contains(trigger.entity) {
        return;
    }
    commands.entity(trigger.entity).insert((
        FrameInterpolate::<Position> {
            trigger_change_detection: true,
            ..default()
        },
        FrameInterpolate::<Rotation> {
            trigger_change_detection: true,
            ..default()
        },
    ));
}

/// Round-reset teleports produce a huge rollback error that `CorrectionPolicy` would otherwise
/// GLIDE across the arena over ~1s (design P8). Any correction error beyond this is a teleport,
/// not a mispredict — snap it (remove the visual correction).
const CORRECTION_SNAP_DISTANCE: f32 = 2.0;

/// Remove `VisualCorrection` when its error is teleport-sized so resets snap instead of gliding.
pub(super) fn snap_large_corrections(
    q: Query<(Entity, &VisualCorrection<Position>), With<Predicted>>,
    mut commands: Commands,
) {
    for (e, corr) in &q {
        if corr.error.0.length() > CORRECTION_SNAP_DISTANCE {
            commands
                .entity(e)
                .remove::<(VisualCorrection<Position>, VisualCorrection<Rotation>)>();
        }
    }
}

/// Counts rendered frames so the smoke run can exit deterministically.
#[derive(Resource)]
pub(super) struct SmokeExit {
    pub(super) target: u64,
    pub(super) count: u64,
}

pub(super) fn smoke_exit_after_frames(
    mut smoke: ResMut<SmokeExit>,
    mut exit: MessageWriter<AppExit>,
) {
    smoke.count += 1;
    if smoke.count >= smoke.target {
        info!("arena_game smoke: reached {} frames, exiting", smoke.count);
        exit.write(AppExit::Success);
    }
}

/// Screenshot-harness config, present as a resource only when `ARENA_SHOT` is set.
#[derive(Resource)]
pub(super) struct ScreenshotConfig {
    path: PathBuf,
    shot_frame: u64,
    count: u64,
    fired: bool,
}

impl ScreenshotConfig {
    pub(super) fn from_env() -> Option<Self> {
        let path = std::env::var_os("ARENA_SHOT")?;
        Some(Self {
            path: PathBuf::from(path),
            shot_frame: env_frame("ARENA_SHOT_FRAME", 120),
            count: 0,
            fired: false,
        })
    }
}

pub(super) fn screenshot_system(
    mut commands: Commands,
    mut cfg: ResMut<ScreenshotConfig>,
    mut exit: MessageWriter<AppExit>,
) {
    cfg.count += 1;

    if !cfg.fired && cfg.count >= cfg.shot_frame {
        let path = cfg.path.clone();
        info!(
            "arena_game shot: frame {}, capturing primary window -> {}",
            cfg.count,
            path.display()
        );
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        cfg.fired = true;
    }

    if cfg.fired && cfg.count >= cfg.shot_frame + 12 {
        info!("arena_game shot: capture flushed, exiting");
        exit.write(AppExit::Success);
    }
}
