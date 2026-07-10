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
use super::{customization, hud, level, net, parts, present, radial, rig, skill_objects, weapon_select};

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
    // Surface types (config/surfaces) — AFTER add_obelisk_skills so the loader validates tick_skill
    // refs against the SkillRegistry. The client only LOADS the registry (for [visuals] / render
    // mirrors); surface BEHAVIOR is server-only (ObeliskSurfacesPlugin is omitted from the client sim).
    app.add_obelisk_surfaces(&root.join("config/surfaces"));
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
    // Skill-select state (the radial wheel writes it; `init_resource` is idempotent with
    // ClientNetPlayerPlugin's own init).
    app.init_resource::<net::SelectedSkill>();
    // The weapon catalog (obelisk items, protocol v4) — the I panel's list + the wheel's weapon
    // name. Same files every peer loads.
    app.insert_resource(crate::net::weapons::WeaponCatalog::load(
        &root.join("config/items"),
    ));

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
    // Level sync + lobby UX (levels-and-lobby): mirrors the server-announced level (WITH visuals —
    // meshes/materials/lights from the level file) + the host's G-key level-select panel.
    app.add_plugins(level::ClientLevelPlugin { windowed: true });
    // Weapons (protocol v4): the lobby's I-key equip panel + the hold-F radial skill wheel
    // (wisp's, 1:1) selecting among the equipped weapon's skills.
    app.add_plugins(weapon_select::WeaponSelectPlugin);
    app.add_plugins(radial::RadialWheelPlugin);
    // Replicated skill-object visuals (wisp weapon ports: portals, frost tiles, spires).
    app.add_plugins(skill_objects::SkillObjectVisualsPlugin);
    // Replicated surface-patch visuals (spec §6/D10: tinted ForwardDecal + optional looping vfx
    // per NetworkedSurfacePatch). Windowed-only — decals need PbrPlugin's render infra + the
    // camera DepthPrepass (added in scene.rs); the headless observer never renders.
    app.add_plugins(crate::client::surfaces::SurfaceVisualsPlugin);
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
    // Charge-hold presentation (designer charge tiers, `CastTimeline::charge_cues`): the per-rig
    // bone-socket index + the tier driver (local ChargeState + the remote's predicted
    // ActionState.charging telegraph) + one-shot cast-anim overlay expiry.
    app.add_systems(
        Update,
        (
            super::sockets::index_rig_sockets,
            super::charge_cues::drive_charge_cues,
            rig::expire_cue_anim_overlays,
        ),
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
    // [W] UI probe hook (ARENA_DEBUG_UI=wheel|weapons): synthetically hold F (the radial wheel)
    // or tap I (the weapon panel) from frame 150, so a screenshot probe can capture the UI
    // without scripted OS input. Mirrors the ARENA_* headless hook idiom.
    if let Ok(which) = std::env::var("ARENA_DEBUG_UI") {
        let key = match which.as_str() {
            "wheel" => Some((KeyCode::KeyF, true)),
            "weapons" => Some((KeyCode::KeyI, false)),
            _ => None,
        };
        if let Some((key, hold)) = key {
            // PreUpdate, AFTER bevy's keyboard system clears just_pressed — so every Update
            // system sees the synthetic press that same frame (an Update-schedule press would
            // race the consumers' system order).
            app.add_systems(
                PreUpdate,
                (move |mut frames: bevy::prelude::Local<u64>,
                       mut keys: bevy::prelude::ResMut<ButtonInput<KeyCode>>| {
                    *frames += 1;
                    if *frames == 150 || (hold && *frames > 150) {
                        keys.press(key);
                    } else if !hold && *frames == 152 {
                        keys.release(key);
                    }
                })
                .after(bevy::input::InputSystems),
            );
        }
    }

    // AUTOCAST harness (ARENA_AUTOCAST=1): drive the NETWORKED cast path on a cadence (set
    // `net::CastIntent` → `send_cast_requests` ships a `CastRequestMessage`). This is the same wire
    // cast the headless AUTOCAST uses. Lets a windowed client script firebolt for the visual gate.
    if std::env::var("ARENA_AUTOCAST").ok().as_deref() == Some("1") {
        // AFTER the cast-hold bridge: the bridge writes ChargeState from the real mouse every
        // frame, which would clobber a scripted ARENA_AUTOCAST_HOLD charge.
        app.add_systems(Update, autocast.after(bridge_windowed_cast_hold));
    }

    // AUTOEQUIP harness (ARENA_AUTOEQUIP=<weapon_id>): the headless equip hook, available
    // windowed too so visual probes can script a non-starter weapon (e.g. the portal pair).
    if std::env::var("ARENA_AUTOEQUIP").is_ok() {
        app.add_systems(Update, super::app_headless::autoequip_weapon);
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
    // Teleport snap (design P8): a round-reset moves the body arena-widths in one tick; the
    // resulting VisualCorrection error would otherwise decay-glide for ~1s. Snap corrections
    // beyond teleport scale BEFORE lightyear applies/decays them this frame.
    app.add_systems(
        PostUpdate,
        super::harness::snap_large_corrections
            .before(lightyear::prediction::rollback::RollbackSystems::VisualCorrection),
    );
    // Trace the replicated NetEvent stream + consume the replicated cues → cosmetics:
    // `register_client_cue_binding` drains CueWireMessage, de-dups the local player's own
    // predicted cue, and feeds survivors to `spawn_cue_cosmetics` via the LocalCue channel. So
    // replicated firebolt VFX play on the windowed client for BOTH peers' casts.
    crate::skills::register_client_event_trace(&mut app);
    crate::skills::register_client_cue_binding(&mut app);
    // Predicted own-cast: play the local on_cast + cosmetic projectile INSTANTLY on cast,
    // with NO ResolveHits / Hitbox / CombatRng (Stage-A invariant). Damage arrives from the server.
    crate::skills::register_predicted_sim(&mut app);
    // Surfaces (spec §7): trace every replicated patch as it materializes (headless-safe — the
    // same signal both client roots register; Task 3 adds the windowed visuals alongside).
    app.add_systems(Update, crate::client::surfaces::trace_replicated_patches);
    // HUD: hp bars driven by replicated NetworkedHealth, floating damage + hit flash
    // from DamageResolved, and the round/score banner from RoundStateMessage. Windowed-only.
    app.add_plugins(hud::ArenaHudPlugin);
    app.add_systems(
        Update,
        (
            // Runs before the input bridge so a focus-loss frame clears stuck keys first.
            release_keys_on_focus_loss,
            bridge_windowed_input_to_local_input,
            // (Skill selection is the hold-F radial wheel now — `client::radial`, wisp's 1:1.
            // The old number-key select is gone with the fixed ARENA_SKILLS roster.)
            bridge_windowed_cast_hold,
        )
            .chain(),
    );

    // AUTOMOVE harness (ARENA_AUTOMOVE=1): the headless constant-forward hook, available
    // windowed too for visual probes. AFTER the bridge so its movement deterministically wins
    // over the (idle) keyboard state; it writes the same yaw/pitch the bridge does.
    if std::env::var("ARENA_AUTOMOVE").ok().as_deref() == Some("1") {
        app.add_systems(
            Update,
            super::app_headless::automove_input.after(bridge_windowed_input_to_local_input),
        );
    }

    app.run();
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
    mut charge: ResMut<net::ChargeState>,
    customization: Option<Res<customization::CustomizationOpen>>,
    level_select: Option<Res<level::LevelSelectOpen>>,
    weapon_select: Option<Res<weapon_select::WeaponSelectOpen>>,
    wheel: Option<Res<radial::RadialWheel>>,
) {
    // While any selection UI is up (customizer / level select / weapon panel / radial wheel) —
    // don't charge/cast.
    if customization.map(|c| c.open).unwrap_or(false)
        || level_select.map(|l| l.open).unwrap_or(false)
        || weapon_select.map(|w| w.open).unwrap_or(false)
        || wheel.map(|w| w.open).unwrap_or(false)
    {
        charge.secs = 0.0;
        charge.charging = false;
        return;
    }
    // Cast is LEFT-MOUSE only: Space is reserved for jumping (see `bridge_windowed_input_to_local_input`).
    // The cast itself is the input stream's charging FALLING EDGE (design WS2): press sets
    // `charging` + the tap latch (so a sub-tick tap still occupies one input tick); release drops
    // `charging`, and both peers' edge detectors fire — no message, no locked-in charge byte (the
    // byte derives from the held tick count on both peers).
    let held = mouse.pressed(MouseButton::Left);

    if held {
        if mouse.just_pressed(MouseButton::Left) {
            charge.tap_latch = true;
        }
        charge.secs = (charge.secs + time.delta_secs()).min(net::MAX_CHARGE_SECS);
        charge.charging = true;
    } else {
        // A scripted charge hold (ARENA_AUTOCAST_HOLD, visual probes) owns ChargeState while
        // active — the idle mouse must not zero its in-flight hold (autocast runs after this
        // bridge and releases it itself).
        if charge.charging && std::env::var("ARENA_AUTOCAST_HOLD").is_ok() {
            return;
        }
        // Reset — the falling edge in the sampled input stream carries the cast.
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
    level_select: Option<Res<level::LevelSelectOpen>>,
    weapon_select: Option<Res<weapon_select::WeaponSelectOpen>>,
    wheel: Option<Res<radial::RadialWheel>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    // While any selection UI is up (customizer / level select / weapon panel / radial wheel) —
    // don't drive movement. The wheel case is wisp's "keys stay held at the OS level but the
    // movement context is off" behavior: zero input here = the same brake.
    if customization.map(|c| c.open).unwrap_or(false)
        || level_select.map(|l| l.open).unwrap_or(false)
        || weapon_select.map(|w| w.open).unwrap_or(false)
        || wheel.map(|w| w.open).unwrap_or(false)
    {
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

// (The number-key skill-select tests left with `select_skill` itself — skill selection is the
// hold-F radial wheel now, whose hit-test math + selection flow are unit-tested in
// `client::radial`. The `weapon_skill_slot_for` slot mapping is pinned in `net::tests`.)
