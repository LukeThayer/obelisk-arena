//! The NETWORKED windowed client: DefaultPlugins + the lightyear client stack + the CLIENT obelisk
//! subset (no authoritative ResolveHits/RNG — Stage A) + net-driven materialized players (rigged),
//! predicted local movement, first-person camera, replicated/predicted cosmetics, and the HUD
//! (hp bars + round banner). The duel is entirely server-authoritative + replicated.
//!
//! Also home to the windowed input bridges that translate the first-person controller's
//! [`controller::CameraYaw`]/[`controller::AimPitch`] + WASD/LMB into [`net::LocalInput`] /
//! [`net::CastIntent`].

use avian3d::prelude::{Position, Rotation};
use bevy::prelude::*;
use lightyear_frame_interpolation::FrameInterpolationPlugin;
use obelisk_bevy::prelude::*;

use super::app_headless::autocast;
use super::controller::{self, ArenaControllerPlugin};
use super::cosmetics::{
    age_lifetimes, fly_cosmetic_projectiles, init_cosmetic_assets, init_effect_library,
    init_vfx_library, spawn_cue_cosmetics, AimDirs,
};
use super::harness::{
    add_frame_interpolation_to_predicted, screenshot_system, smoke_exit_after_frames,
    ScreenshotConfig, SmokeExit,
};
use super::scene::{load_rig, log_registered_skills_once, setup_scene};
use super::{customization, hud, net, parts, present, rig};

use crate::{add_avian_with_lightyear, add_obelisk_sim_client, arena_root};

/// Run the windowed NETWORKED client. `DefaultPlugins` (windowing + rendering) + the
/// lightyear client stack + the CLIENT obelisk subset (`add_obelisk_sim_client`, NO authoritative
/// ResolveHits/RNG — Stage A) + the net-driven player/present/HUD layers. It connects to the
/// dedicated server, materializes the replicated players (rigged via `present`), runs the
/// first-person camera + predicted movement on the LOCAL player, renders cosmetics from the
/// replicated/predicted cues, and shows the HUD (hp bars + round banner).
///
/// It composes like the SERVER: `add_avian_with_lightyear` is the SOLE avian `PhysicsPlugins`
/// registrant (else `PhysicsSchedulePlugin ... already added` panics), and `add_obelisk_sim_client`
/// supplies the obelisk asset/core infra WITHOUT `ObeliskSpatialPlugin` (no second physics group)
/// and WITHOUT ResolveHits/`ObeliskCombatPlugin` (Stage-A: the client never resolves hits / draws
/// RNG). The client spawns no combatants of its own — the duel is entirely the replicated
/// `NetworkedPlayer`s + the wire cast path.
pub fn run_windowed_client() {
    let root = arena_root();

    let mut app = App::new();
    // Point the AssetServer at the workspace-root `assets/` dir so cast-timeline paths
    // (e.g. "skills/firebolt.cast.ron") + character.glb resolve there rather than under the crate dir.
    app.add_plugins(DefaultPlugins.set(AssetPlugin {
        file_path: root.join("assets").to_string_lossy().into_owned(),
        ..default()
    }));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    // --- lightyear client stack FIRST (ClientPlugins { 1/60 } + ProtocolPlugin + connect), so the
    //     avian-lightyear physics added below sees the replication infra (same order as the server).
    crate::net::client::connect_to_configured(&mut app);

    // --- physics: the SOLE avian `PhysicsPlugins` registrant (after ClientPlugins). ---
    add_avian_with_lightyear(&mut app);

    // --- obelisk CLIENT subset (no ObeliskSpatialPlugin → no 2nd physics group; no ResolveHits/RNG
    //     → Stage-A invariant). Supplies the CastTimeline asset/loader + SkillRegistry/config infra
    //     the cosmetics + predicted-cast cue lookup need. ---
    add_obelisk_sim_client(&mut app);
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&root.join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(root.join("config/skills")));
    // CombatRng is registered by ObeliskCorePlugin; seed it for determinism even though the client
    // never DRAWS from it (Stage A) — keeps the resource present + future-proof. Seed value is
    // irrelevant client-side (no client resolve).
    app.seed_combat_rng(1);

    // GPU particle + effect engines (windowed only — the headless server/observer never add
    // these, so their net-test path is untouched). `VfxLibrary`/`EffectLibrary` are seeded at
    // Startup by `init_vfx_library`/`init_effect_library`; `spawn_cue_cosmetics` (`cosmetics.rs`)
    // resolves each fired cue's `CueBinding` and spawns the named effect, `EffectLibrary` first
    // then `VfxLibrary` (C3).
    app.add_plugins(bevy_vfx::VfxPlugin);
    app.add_plugins(bevy_effect::EffectPlugin);
    // Third-person camera + mouse-look + spine-pitch aim lean (NET-aware: follows the local
    // predicted player, no longer moves a Transform — prediction owns the body).
    app.add_plugins(ArenaControllerPlugin);

    app.init_resource::<AimDirs>();
    // Cast-timeline loading uses the SAME shared `crate::cast_assets` helpers as the headless server.
    app.init_resource::<crate::cast_assets::PendingCastTimelines>();
    // Windowed-only skill-select state (number keys 1/2/3, see `select_skill` below). The headless
    // client never inits this — its autocast path sets `CastIntent` directly.
    app.init_resource::<net::SelectedSkill>();

    // Scene (camera + light + ground) + rig assets + cast timelines. NO co-located combatants.
    app.add_systems(
        Startup,
        (
            setup_scene,
            load_rig,
            crate::cast_assets::load_cast_timelines,
            init_cosmetic_assets,
            init_vfx_library,
            init_effect_library,
        ),
    );

    app.add_systems(
        Update,
        (
            crate::cast_assets::poll_cast_timelines,
            log_registered_skills_once,
        ),
    );

    // Rig: build the animation graph, attach it + the per-player rig (`present`), apply
    // slot-based part visibility (PartsPlugin), drive the per-player animation.
    app.add_plugins(present::ArenaPresentPlugin);
    app.add_plugins(parts::PartsPlugin);
    // Character customizer (D4): K-toggled per-slot panel + third-person preview. Windowed-only.
    app.add_plugins(customization::CustomizationPlugin);
    app.add_systems(
        Update,
        (
            rig::build_graph_when_loaded,
            rig::attach_animation_graph,
            rig::drive_animation.after(rig::attach_animation_graph),
        ),
    );

    // Cosmetics: spawn from the LocalCue channel (fed by replicated + predicted cues), fly + age.
    app.add_systems(
        Update,
        (spawn_cue_cosmetics, fly_cosmetic_projectiles, age_lifetimes),
    );

    if let Some(frames) = std::env::var("ARENA_SMOKE_FRAMES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        app.insert_resource(SmokeExit {
            target: frames,
            count: 0,
        });
        app.add_systems(Update, smoke_exit_after_frames);
    }

    // Screenshot harness (ARENA_SHOT / ARENA_SHOT_FRAME): Bevy off-screen-window capture. Captures
    // the primary window after N frames → ARENA_SHOT.
    if let Some(cfg) = ScreenshotConfig::from_env() {
        app.insert_resource(cfg);
        app.add_systems(Update, screenshot_system);
    }
    // AUTOCAST harness (ARENA_AUTOCAST=1): drive the NETWORKED cast path on a cadence (set
    // `net::CastIntent` → `send_cast_requests` ships a `CastRequestMessage`). This is the same wire
    // cast the headless AUTOCAST uses. Lets a windowed client script firebolt for the visual gate.
    if std::env::var("ARENA_AUTOCAST").ok().as_deref() == Some("1") {
        app.add_systems(Update, autocast);
    }

    // Net-driven player layer: attach the predicted local body + interpolated remote bodies, buffer
    // native input, predict movement (the shared force controller, re-run during rollback). The
    // windowed controller's CameraYaw/AimPitch + WASD feed `LocalInput` → `buffer_arena_input`.
    app.add_plugins(net::ClientNetPlayerPlugin);
    // Visual frame-interpolation of the predicted local player's Position/Rotation between
    // FixedUpdate ticks (the avian_3d_character renderer pattern). Interpolated remotes are already
    // smooth via lightyear interpolation.
    app.add_plugins(FrameInterpolationPlugin::<Position>::default());
    app.add_plugins(FrameInterpolationPlugin::<Rotation>::default());
    app.add_observer(add_frame_interpolation_to_predicted);
    // Trace the replicated NetEvent stream + consume the replicated cues → cosmetics:
    // `register_client_cue_binding` drains CueWireMessage, de-dups the local player's own
    // predicted cue, and feeds survivors to `spawn_cue_cosmetics` via the LocalCue channel. So
    // replicated firebolt VFX play on the windowed client for BOTH peers' casts.
    crate::skills::register_client_event_trace(&mut app);
    crate::skills::register_client_cue_binding(&mut app);
    // Predicted own-cast: play the local on_cast + cosmetic projectile INSTANTLY on cast,
    // with NO ResolveHits / Hitbox / CombatRng (Stage-A invariant). Damage arrives from the server.
    crate::skills::register_predicted_sim(&mut app);
    // HUD: hp bars driven by replicated NetworkedHealth, floating damage + hit flash
    // from DamageResolved, and the round/score banner from RoundStateMessage. Windowed-only.
    app.add_plugins(hud::ArenaHudPlugin);
    app.add_systems(
        Update,
        (
            // Runs before the input bridge so a focus-loss frame clears stuck keys first.
            release_keys_on_focus_loss,
            bridge_windowed_input_to_local_input,
            // Number-key skill-select runs before the cast-hold bridge so a same-frame
            // select-then-click picks up the new selection immediately.
            select_skill,
            bridge_windowed_cast_hold,
        )
            .chain(),
    );

    app.run();
}

/// Map a number key to a grantable skill id (windowed skill-select). `None` for other keys.
fn skill_for_key(key: KeyCode) -> Option<&'static str> {
    let idx = match key {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        _ => return None,
    };
    crate::net::ARENA_SKILLS.get(idx).copied()
}

/// Windowed skill-select: number keys 1/2/3 choose which granted skill `bridge_windowed_cast_hold`
/// casts next (`net::SelectedSkill`). Headless-never: this system is registered only by
/// `run_windowed_client`, and the headless autocast path sets `CastIntent` directly without ever
/// reading `SelectedSkill`.
fn select_skill(keys: Res<ButtonInput<KeyCode>>, mut selected: ResMut<net::SelectedSkill>) {
    for key in keys.get_just_pressed() {
        if let Some(id) = skill_for_key(*key) {
            selected.0 = id.to_string();
        }
    }
}

/// Hold-to-charge cast input: replaces the old press-to-cast `bridge_windowed_cast_to_intent`.
///
/// While the cast button (Space or LMB) is held, `ChargeState.secs` accumulates (clamped to
/// `MAX_CHARGE_SECS`). On release, the accumulated hold time maps to a charge byte via:
///   `frac = secs / MAX_CHARGE_SECS`
///   `charge = (85 + frac * 170).round()` — 85 ≈ instant tap (≈1.0×), 255 = full hold (2.0×)
/// The charge is locked into `ChargeState.pending_charge` and `CastIntent` is set to whichever
/// skill is currently selected (`net::SelectedSkill`, see `select_skill`).
///
/// The autocast path (`autocast`) bypasses this and sets `CastIntent` directly;
/// `send_cast_requests` uses the `pending_charge` tap-default (85) for those.
fn bridge_windowed_cast_hold(
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    mut intent: ResMut<net::CastIntent>,
    mut charge: ResMut<net::ChargeState>,
    selected: Res<net::SelectedSkill>,
    customization: Option<Res<customization::CustomizationOpen>>,
) {
    // While the customizer is open, LMB clicks the panel buttons — don't charge/cast.
    if customization.map(|c| c.open).unwrap_or(false) {
        charge.secs = 0.0;
        charge.charging = false;
        return;
    }
    // Cast is LEFT-MOUSE only: Space is reserved for jumping (see `bridge_windowed_input_to_local_input`).
    let held = mouse.pressed(MouseButton::Left);
    let just_released = mouse.just_released(MouseButton::Left);

    if held {
        charge.secs = (charge.secs + time.delta_secs()).min(net::MAX_CHARGE_SECS);
        charge.charging = true;
    } else {
        if just_released && charge.charging {
            // Lock in the charge and emit the cast intent on release.
            let frac = charge.frac();
            charge.pending_charge = net::charge_byte_from_frac(frac);
            if intent.0.is_none() {
                intent.0 = Some(selected.0.clone());
            }
        }
        // Reset regardless — keeps state consistent when button is not held.
        charge.secs = 0.0;
        charge.charging = false;
    }
}

/// Release all held keys/mouse buttons the instant the window loses focus. winit can DROP the
/// key-RELEASE event across a focus change (alt-tab, clicking another window), which otherwise
/// leaves `ButtonInput::pressed(..)` stuck "true" after refocus — the player then walks forever.
/// `release_all` marks every held key as just-released so the next real key event re-establishes the
/// true state. Pairs with the focus gate in `bridge_windowed_input_to_local_input`.
fn release_keys_on_focus_loss(
    mut focus: MessageReader<bevy::window::WindowFocused>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
) {
    for ev in focus.read() {
        if !ev.focused {
            keys.release_all();
            mouse.release_all();
        }
    }
}

/// Bridge the windowed first-person controller's input into [`net::LocalInput`] so the local
/// player's movement is sent to the server (server-authoritative Stage-A movement). Reads the
/// camera yaw (mouse-X driven) + WASD keys in the same camera-relative frame the controller uses
/// (matching the server controller): forward = -Z, strafe = +X, both in the camera-yaw frame.
fn bridge_windowed_input_to_local_input(
    keys: Res<ButtonInput<KeyCode>>,
    yaw: Res<controller::CameraYaw>,
    pitch: Res<controller::AimPitch>,
    mut local_input: ResMut<net::LocalInput>,
    customization: Option<Res<customization::CustomizationOpen>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    // While the customizer is open, A/D orbit the preview camera — don't drive movement.
    if customization.map(|c| c.open).unwrap_or(false) {
        local_input.movement = Vec2::ZERO;
        local_input.jump = false;
        return;
    }
    // Window not focused (alt-tab / clicked to another window): winit can DROP the key-RELEASE
    // event, leaving `keys.pressed(..)` stuck "true" — so the player keeps walking in one direction
    // and never stops. Treat an unfocused window as zero input. (Defends the common "starts walking
    // and won't stop" symptom; on refocus the live key state resumes.)
    let focused = windows.iter().next().map(|w| w.focused).unwrap_or(true);
    if !focused {
        local_input.movement = Vec2::ZERO;
        local_input.jump = false;
        return;
    }
    let mut movement = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        movement.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        movement.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        movement.x += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        movement.x -= 1.0;
    }
    local_input.movement = movement;
    local_input.yaw = yaw.0;
    local_input.pitch = pitch.0;
    // SPACE jumps: the server controller (and the local prediction) apply manual gravity + a ground
    // clamp, so a grounded player holding Space launches up (JUMP_SPEED) and falls back to GROUND_Y.
    local_input.jump = keys.pressed(KeyCode::Space);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the number-key → skill-id mapping `select_skill` relies on: 1/2/3 map to the three
    /// granted reference skills (`server/spawn.rs`'s `grant_skill` chain), any other key selects
    /// nothing.
    #[test]
    fn number_keys_map_to_the_three_skills() {
        assert_eq!(skill_for_key(KeyCode::Digit1), Some("firebolt"));
        assert_eq!(skill_for_key(KeyCode::Digit2), Some("chain_lightning"));
        assert_eq!(skill_for_key(KeyCode::Digit3), Some("blizzard"));
        assert_eq!(skill_for_key(KeyCode::KeyW), None);
    }

    /// Runtime-verifies the keyboard→resource path the dynamic HUD spell label depends on: a
    /// `Digit3` press must flip [`net::SelectedSkill`] to blizzard, and `Digit1` back to firebolt,
    /// through the real `select_skill` system reading a real `ButtonInput<KeyCode>`. This is the
    /// headless proxy for the user-facing "pressing 1/2/3 changes the selected spell"; the windowed
    /// winit→`ButtonInput` plumbing is Bevy's own and is already exercised by WASD movement (which
    /// reads the very same resource in `bridge_windowed_input_to_local_input`).
    #[test]
    fn select_skill_flips_selected_on_number_key() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<net::SelectedSkill>();
        app.add_systems(Update, select_skill);

        // Default selection is firebolt.
        assert_eq!(app.world().resource::<net::SelectedSkill>().0, "firebolt");

        // Press "3" → blizzard.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Digit3);
        app.update();
        assert_eq!(app.world().resource::<net::SelectedSkill>().0, "blizzard");

        // Fresh frame (clear just_pressed), press "1" → back to firebolt.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Digit1);
        app.update();
        assert_eq!(app.world().resource::<net::SelectedSkill>().0, "firebolt");
    }
}
