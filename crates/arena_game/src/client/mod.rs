//! Client-side present + gameplay layer (M1 co-located logic, now lib-housed).
//!
//! M2.1 keeps the M1 single-process gameplay working through `run_windowed_client`, which the
//! `arena-client` bin calls. Later M2 tasks (§6) split this into net-driven prediction +
//! interpolation; for now it's the M1 firebolt-cast + cosmetics + rig stack, unchanged, plus the
//! lightyear client net stack layered on so two clients can connect to the dedicated server.

pub mod controller;
pub mod cosmetics;
pub mod hud;
pub mod net;
pub mod prediction;
pub mod replication;
pub mod rig;

use arena_skills::{cue_event_to_message, SkillFxRegistry};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use controller::{ArenaControllerPlugin, FollowCamera, PlayerController};
use cosmetics::{age_lifetimes, fly_cosmetic_projectiles, spawn_cue_cosmetics, AimDirs, LocalCue};
use obelisk_bevy::prelude::*;
use rig::{ArenaBody, LocalAnimBlend};
use stat_core::StatBlock;
use std::path::PathBuf;

use crate::{add_avian_with_lightyear, arena_root, net::ClientNetPlugin};

/// Wire the M2 co-located cue path: load the [`SkillFxRegistry`] (cue_id → lanes) from
/// `assets/skills`, register the [`LocalCue`] message channel, and install the egress observer that
/// converts every obelisk `CueEvent` into a serde `CueMessage` (wrapped in `LocalCue`).
///
/// This replaces M1's `register_skill_cues` build-time closure binding. The egress observer resolves
/// `CueEvent.source` (a local `Entity`) → the source's stable `ObeliskId` (via `ObeliskEntityIndex`)
/// so the serde `CueMessage.source_id` is process-independent — the same shape that crosses the wire
/// in M2.3. The cosmetics consumer (`spawn_cue_cosmetics`) reads `LocalCue`, re-looks-up the lanes
/// from the registry, and spawns. `arena_skills` stays lightyear-free: this `arena_game` glue owns
/// the bevy `Message` wrapper + the obelisk lookup.
///
/// A missing/empty `assets/skills` dir yields an empty registry (cues then no-op) rather than
/// panicking the binary.
fn register_cue_egress(app: &mut App, root: &std::path::Path) {
    let skills_dir = root.join("assets/skills");
    let registry = SkillFxRegistry::load_dir(&skills_dir);
    let bound: Vec<String> = {
        let mut k: Vec<String> = registry.by_cue.keys().cloned().collect();
        k.sort();
        k
    };
    app.insert_resource(registry);
    app.add_message::<LocalCue>();
    // The egress observer: one observer for ALL CueEvents (filters happen in the consumer via the
    // registry lookup). It needs `Res<ObeliskEntityIndex>` to resolve the source Entity → ObeliskId,
    // which a bevy observer can take as a system param.
    app.add_observer(
        |cue: On<CueEvent>, index: Res<ObeliskEntityIndex>, mut writer: MessageWriter<LocalCue>| {
            let cue = cue.event();
            // The source's stable ObeliskId (caster for OnCast/OnWindow, target for OnHit). If the
            // source has no ObeliskId, skip + warn rather than emit an empty-string id (mirrors
            // obelisk's NetEvent mirror invariant).
            let Some(source_id) = index.id(cue.source) else {
                warn!(
                    "cue {} source {:?} has no ObeliskId — skipping",
                    cue.cue_id, cue.source
                );
                return;
            };
            let msg = cue_event_to_message(&cue.cue_id, source_id, cue.position, cue.kind.into());
            writer.write(LocalCue(msg));
        },
    );
    info!(
        "cue egress wired from {} (bound cues: {:?})",
        skills_dir.display(),
        bound
    );
}

/// Run the windowed M1 client app (DefaultPlugins + obelisk sim + cosmetics + rig). This is the
/// M1 `main()` body, lib-housed so the `arena-client` bin can call it and (later tasks) layer the
/// net stack on top. The lightyear client connect is added by `crate::net::ClientNetPlugin` in the
/// bin, not here, so this function stays a pure single-process present app for the M1 regression.
pub fn run_windowed_client() {
    let root = arena_root();

    let mut app = App::new();
    // Point the AssetServer at the workspace-root `assets/` dir so cast-timeline paths
    // (e.g. "skills/firebolt.cast.ron") resolve there rather than under the crate dir.
    app.add_plugins(DefaultPlugins.set(AssetPlugin {
        file_path: root.join("assets").to_string_lossy().into_owned(),
        ..default()
    }));
    // The headless authoritative simulation (assets + spatial + core + combat + net + vfx + loot).
    app.add_plugins(ObeliskSimPlugin);
    // The arena cosmetic-binding layer: registers the SkillFx asset + its `.skillfx.ron` loader.
    app.add_plugins(arena_skills::ArenaSkillsPlugin);
    // Third-person controller: follow camera + camera-relative WASD + chest_joint aim lean.
    app.add_plugins(ArenaControllerPlugin);
    // NB: `TracePlugin` is added by `ClientNetPlugin` (below) for BOTH client modes — adding it here
    // too double-add-panics the moment the windowed client runs with `ARENA_TRACE_FILE` set (Bevy
    // rejects a duplicate plugin). The net plugin's copy covers the windowed client's tracing.
    // Obelisk runs its sim on the 60 Hz fixed timestep.
    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    register_cue_egress(&mut app, &root);

    // Obelisk global config: stat constants + the effect/skill registries + a fixed RNG seed.
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&root.join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(root.join("config/skills")));
    app.seed_combat_rng(1);

    app.add_systems(
        Startup,
        (setup_scene, load_rig, load_cast_assets, spawn_combatants).chain(),
    );
    app.init_resource::<AimDirs>();

    app.add_systems(
        Update,
        (
            poll_cast_assets,
            log_registered_skills_once,
            confirm_combatants_once,
        ),
    );

    app.add_systems(
        Update,
        (
            rig::build_graph_when_loaded,
            rig::attach_animation_graph,
            rig::cull_costume,
            rig::drive_animation.after(rig::attach_animation_graph),
        ),
    );

    app.add_systems(
        Update,
        (spawn_cue_cosmetics, fly_cosmetic_projectiles, age_lifetimes),
    );

    app.add_systems(Update, cast_on_input);

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

    if let Some(cfg) = ScreenshotConfig::from_env() {
        app.insert_resource(cfg);
        app.add_systems(Update, screenshot_system);
    }
    if let Some(cfg) = AutocastConfig::from_env() {
        app.insert_resource(cfg);
        app.add_systems(Update, autocast_system);
    }

    // lightyear client stack (ClientPlugins { 1/60 } + ProtocolPlugin + connect), then the
    // avian-lightyear physics AFTER ClientPlugins (same ordering rule as the server). The windowed
    // client connects to the dedicated server just like the headless client.
    app.add_plugins(ClientNetPlugin);
    let server_addr = crate::net::parse_addr_args(crate::net::default_server_addr());
    let client_id = app
        .world()
        .resource::<crate::net::client::ConnectTo>()
        .client_id;
    app.insert_resource(crate::net::client::ConnectTo {
        server: server_addr,
        client_id,
    });
    add_avian_with_lightyear(&mut app);

    // Net-driven player layer (M2.2): materialize a body per replicated NetworkedPlayer, interpolate
    // remote players, and send local input to the server. The windowed client bridges its existing
    // third-person controller (CameraYaw + WASD keys) into `LocalInput` so server-authoritative
    // movement is driven by real input. The M1 co-located local player/dummy still spawn for the
    // single-process regression; the net layer runs alongside and drives the replicated players.
    app.add_plugins(net::ClientNetPlayerPlugin);
    app.add_plugins(replication::ReplicationSmoothingPlugin);
    // Stage-A own-movement prediction (predict-locally-snap-to-server fallback, see prediction.rs).
    app.add_plugins(prediction::LocalPredictionPlugin);
    // Trace the replicated NetEvent stream (Task 15) + consume the replicated cues → cosmetics
    // (Task 16): `register_client_cue_binding` drains CueWireMessage, de-dups the local player's own
    // predicted cue, and feeds survivors to `spawn_cue_cosmetics` via the LocalCue channel. So
    // replicated firebolt VFX play on the windowed client for BOTH peers' casts.
    crate::skills::register_client_event_trace(&mut app);
    crate::skills::register_client_cue_binding(&mut app);
    // Predicted own-cast (Task 17): play the local on_cast + cosmetic projectile INSTANTLY on cast,
    // with NO ResolveHits / Hitbox / CombatRng (Stage-A invariant). Damage arrives from the server.
    crate::skills::register_predicted_sim(&mut app);
    // HUD (Task 18 + 19): hp bars driven by replicated NetworkedHealth, floating damage + hit flash
    // from DamageResolved, and the round/score banner from RoundStateMessage. Windowed-only.
    app.add_plugins(hud::ArenaHudPlugin);
    app.add_systems(
        Update,
        (
            bridge_windowed_input_to_local_input,
            bridge_windowed_cast_to_intent,
        ),
    );

    app.run();
}

/// Bridge the windowed cast key (Space / left-mouse) into [`net::CastIntent`] so the local player's
/// cast goes over the wire as a `CastRequestMessage` (server re-validates + resolves — Stage A,
/// Task 14). This is the windowed counterpart of the headless AUTOCAST hook; it replaces M1's direct
/// `cast_skill_at` for the NET player. (The M1 co-located `cast_on_input` still runs for the
/// single-process dummy regression, but the authoritative duel cast now flows server-side.)
fn bridge_windowed_cast_to_intent(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut intent: ResMut<net::CastIntent>,
) {
    let pressed = keys.just_pressed(KeyCode::Space) || mouse.just_pressed(MouseButton::Left);
    if pressed && intent.0.is_none() {
        intent.0 = Some("firebolt".to_string());
    }
}

/// Bridge the windowed third-person controller's input into [`net::LocalInput`] so the local
/// player's movement is sent to the server (server-authoritative Stage-A movement). Reads the
/// camera yaw (mouse-X driven) + WASD keys in the same camera-relative frame the controller uses
/// (`controller::move_player`): forward = -Z, strafe = +X, both in the camera-yaw frame.
fn bridge_windowed_input_to_local_input(
    keys: Res<ButtonInput<KeyCode>>,
    yaw: Res<controller::CameraYaw>,
    pitch: Res<controller::AimPitch>,
    mut local_input: ResMut<net::LocalInput>,
) {
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
    // No jump in Stage A (the server controller is kinematic + flat-arena); Space is the cast key.
    local_input.jump = false;
}

/// Run the headless connectivity/movement client: MinimalPlugins + LogPlugin (no window, no
/// rendering, no M1 gameplay) + the lightyear client net stack + the avian-lightyear physics + the
/// net-driven player layer (materialize bodies + send input). Gated by `ARENA_HEADLESS=1` so two
/// clients can be brought up under the net-test harness to verify connection + replication +
/// movement without windows.
///
/// It materializes a body for every replicated `NetworkedPlayer` (tracing `materialized_player`
/// with the owner's client_id + local flag so the late-joiner check still passes), runs the M2.2
/// Task 11 smoothing, and — under `ARENA_AUTOMOVE=1` — feeds a constant forward input so the
/// movement-replication check can drive the server controller headlessly.
pub fn run_headless_client() {
    let root = arena_root();
    let mut app = App::new();

    app.add_plugins((
        MinimalPlugins,
        bevy::log::LogPlugin::default(),
        AssetPlugin {
            file_path: root.join("assets").to_string_lossy().into_owned(),
            ..default()
        },
        TransformPlugin,
        bevy::mesh::MeshPlugin,
        bevy::scene::ScenePlugin,
    ));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    app.add_plugins(ClientNetPlugin);
    let server_addr = crate::net::parse_addr_args(crate::net::default_server_addr());
    let client_id = app
        .world()
        .resource::<crate::net::client::ConnectTo>()
        .client_id;
    app.insert_resource(crate::net::client::ConnectTo {
        server: server_addr,
        client_id,
    });
    add_avian_with_lightyear(&mut app);

    // Net-driven player layer: materialize a body per replicated NetworkedPlayer + send input +
    // (Task 11) interpolate remote players. Also traces replicated/materialized players for the
    // late-joiner check.
    app.add_plugins(net::ClientNetPlayerPlugin);
    app.add_plugins(replication::ReplicationSmoothingPlugin);
    // Stage-A own-movement prediction (predict-locally-snap-to-server fallback, see prediction.rs).
    app.add_plugins(prediction::LocalPredictionPlugin);
    // [H] Trace the replicated combat events (NetEventMessage), and consume the replicated cues
    // (CueWireMessage → trace + de-dup + dispatch). Headless has no cosmetics reader, so the
    // dispatched LocalCues clear harmlessly; the trace lines are what the [H] checks assert on.
    crate::skills::register_client_event_trace(&mut app);
    crate::skills::register_client_cue_binding(&mut app);
    app.add_systems(
        Update,
        (
            trace_replicated_players,
            trace_replicated_health,
            trace_replicated_round_state,
        ),
    );

    // [H] AUTOMOVE hook: feed a constant forward movement input so the headless movement-replication
    // check can drive the server controller without a keyboard. Off unless ARENA_AUTOMOVE=1.
    if std::env::var("ARENA_AUTOMOVE").ok().as_deref() == Some("1") {
        app.add_systems(Update, automove_input);
    }

    // [H] AUTOCAST hook: once we own a local player AND an opponent is replicated, set the cast
    // intent once so `net::send_cast_requests` fires a `CastRequestMessage` (Task 14). This is the
    // headless verification vehicle for M2.3 — a server `CastBegan` (and downstream the egress of
    // damage + cues) follows. Off unless ARENA_AUTOCAST=1. (Distinct from the windowed/M1 AUTOCAST
    // which directly `cast_skill_at`s a co-located dummy — the headless one goes over the wire.)
    if std::env::var("ARENA_AUTOCAST").ok().as_deref() == Some("1") {
        app.add_systems(Update, headless_autocast);
    }

    app.run();
}

/// [H] AUTOCAST: set [`net::CastIntent`] to firebolt on a CADENCE (default ~0.8s), once both the
/// local player and an opponent are materialized so `send_cast_requests` has a target hint. Repeating
/// (not one-shot) so a headless AUTOCAST client drives a FULL best-of-3 match: it keeps casting across
/// rounds, killing the opponent each round until one side reaches the match-win threshold (Task 19).
///
/// Cadence is `ARENA_AUTOCAST_PERIOD` seconds (default 0.8) — comfortably above firebolt's ~0.6s cast
/// time so requests don't pile up behind an in-flight cast (the server skips a caster mid-cast). The
/// server is authoritative for whether a given cast lands; firing continuously is safe — a stray cast
/// during a countdown/round-over window is harmless (the per-round reset heals both players on entry
/// to Active, and only deaths during Active count).
fn headless_autocast(
    time: Res<Time>,
    mut accum: Local<f32>,
    mut intent: ResMut<net::CastIntent>,
    local: Query<
        (),
        (
            With<crate::net::protocol::NetworkedPlayer>,
            With<net::LocalNetPlayer>,
        ),
    >,
    remotes: Query<
        (),
        (
            With<crate::net::protocol::NetworkedPlayer>,
            Without<net::LocalNetPlayer>,
        ),
    >,
) {
    // Need our own player + an opponent before the request carries a useful target hint.
    if local.iter().next().is_none() || remotes.iter().next().is_none() {
        return;
    }
    let period = std::env::var("ARENA_AUTOCAST_PERIOD")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.8);
    *accum += time.delta_secs();
    if *accum >= period {
        *accum = 0.0;
        if intent.0.is_none() {
            intent.0 = Some("firebolt".to_string());
        }
    }
}

/// Polling tracer kept from the M2.1 GATE: emit `replicated_player` once per replicated
/// `NetworkedPlayer` (keyed by `NetworkOwner.client_id`) so the late-joiner check still has its
/// signal after the temp scaffolding became real body materialization.
fn trace_replicated_players(
    players: Query<
        (
            Entity,
            &crate::net::protocol::NetworkOwner,
            Option<&crate::net::protocol::ObeliskNetId>,
        ),
        With<crate::net::protocol::NetworkedPlayer>,
    >,
    mut seen: Local<std::collections::HashSet<u64>>,
) {
    for (entity, owner, obelisk_id) in &players {
        if seen.insert(owner.0) {
            let obelisk_id = obelisk_id.map(|o| o.0.clone());
            bevy::log::info!(
                "client received replicated NetworkedPlayer: entity={entity:?} owner={} obelisk_id={obelisk_id:?}",
                owner.0
            );
            crate::trace::event(
                "replicated_player",
                serde_json::json!({ "owner": owner.0, "obelisk_id": obelisk_id }),
            );
        }
    }
}

/// [H] Trace the replicated `NetworkedHealth` whenever it CHANGES on this client — proves the
/// server-authoritative hp mirror (Task 18) replicates over the wire and drops on a hit (50 → 30).
/// `Changed<NetworkedHealth>` fires only on a real delta, so the trace mirrors the server's `hp`
/// stream from the receiving end. Keyed by the replicated `ObeliskNetId`.
#[allow(clippy::type_complexity)]
fn trace_replicated_health(
    changed: Query<
        (
            &crate::net::protocol::ObeliskNetId,
            &crate::net::protocol::NetworkedHealth,
        ),
        (
            With<crate::net::protocol::NetworkedPlayer>,
            Changed<crate::net::protocol::NetworkedHealth>,
        ),
    >,
) {
    for (net_id, health) in &changed {
        crate::trace::event(
            "client_hp",
            serde_json::json!({ "obelisk_id": net_id.0, "current": health.current,
                "max": health.max }),
        );
    }
}

/// [H] Trace each replicated `RoundStateMessage` the headless client receives, so the round-machine
/// check (Task 19) can assert the best-of-3 flow replicates over the wire (phase transitions, the
/// score increments, and the terminal `MatchOver`). Dedups consecutive identical (phase, scores,
/// winner) so the trace carries transitions, not the ~1/sec countdown re-broadcasts.
#[allow(clippy::type_complexity)]
fn trace_replicated_round_state(
    mut receivers: Query<
        &mut lightyear::prelude::MessageReceiver<crate::net::protocol::RoundStateMessage>,
    >,
    mut last: Local<Option<(u8, [(String, u8); 2], String)>>,
) {
    for mut rx in &mut receivers {
        for msg in rx.receive() {
            let key = (msg.phase, msg.scores.clone(), msg.winner.clone());
            if last.as_ref() == Some(&key) {
                continue; // skip the per-second countdown re-broadcasts
            }
            *last = Some(key);
            crate::trace::event(
                "client_round_state",
                serde_json::json!({ "phase": msg.phase, "countdown": msg.countdown,
                    "scores": msg.scores, "winner": msg.winner }),
            );
        }
    }
}

/// [H] AUTOMOVE: write a constant forward movement into [`net::LocalInput`] so the headless client
/// drives the server controller. `movement.y = 1.0` (full forward), `yaw = 0` (faces -Z). The
/// server's controller turns this into motion; the resulting NetworkedPosition change is what the
/// movement-replication check asserts on (server pose changes + the OTHER client observes it).
fn automove_input(mut input: ResMut<net::LocalInput>) {
    input.movement = Vec2::new(0.0, 1.0);
    input.yaw = 0.0;
    input.pitch = 0.0;
    input.jump = false;
}

/// Spawn a minimal 3D scene: a camera looking at the origin, a directional
/// light, and a green ground plane.
fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        FollowCamera,
        Transform::from_xyz(0.0, 2.0, 4.0).looking_at(Vec3::new(0.0, 1.5, 0.0), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(20.0, 20.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
    ));
}

/// Kick off the async load of the player character rig (`character.glb`) and insert `RigAssets`.
fn load_rig(mut commands: Commands, assets: Res<AssetServer>) {
    let gltf: Handle<bevy::gltf::Gltf> = assets.load("character.glb");
    commands.insert_resource(rig::RigAssets::new(gltf));
}

/// Spawn the two M1 combatants: a `Player`-faction "player" at the origin granted "firebolt", and an
/// `Enemy`-faction "dummy" 6 units down +Z.
fn spawn_combatants(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
) {
    let capsule = meshes.add(Capsule3d::new(0.3, 1.0));

    let player_scene: Handle<Scene> =
        assets.load(GltfAssetLabel::Scene(0).from_asset("character.glb"));
    let player_body = commands
        .spawn((
            Name::new("ArenaBody"),
            ArenaBody,
            SceneRoot(player_scene),
            Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
            Visibility::default(),
        ))
        .id();

    let player = commands
        .spawn_empty()
        .make_combatant(StatBlock::with_id("player"))
        .insert((
            Faction::Player,
            PlayerController,
            LocalAnimBlend::default(),
            Transform::from_xyz(0.0, 0.0, 0.0),
            Visibility::default(),
        ))
        .id();
    commands.entity(player).add_child(player_body);
    commands.entity(player).grant_skill("firebolt");

    let dummy_pos = Vec3::new(0.0, 0.0, 6.0);
    let dummy = commands
        .spawn_empty()
        .make_combatant(StatBlock::with_id("dummy"))
        .insert((
            Faction::Enemy,
            Transform::from_translation(dummy_pos),
            Mesh3d(capsule),
            MeshMaterial3d(materials.add(Color::srgb(1.0, 0.3, 0.2))),
        ))
        .id();
    insert_hurtbox(&mut commands, dummy, 0.6, dummy_pos);
}

/// One-shot readback (fires once): confirm both combatants spawned with `ObeliskId == StatBlock.id`.
fn confirm_combatants_once(
    mut done: Local<bool>,
    combatants: Query<(&ObeliskId, &Attributes, &SkillSlots), With<Combatant>>,
) {
    if *done {
        return;
    }
    let mut summary: Vec<String> = Vec::new();
    for (id, attrs, slots) in &combatants {
        debug_assert_eq!(
            id.0, attrs.0.id,
            "ObeliskId ({}) must equal StatBlock.id ({})",
            id.0, attrs.0.id
        );
        summary.push(format!(
            "{:?}(hp={}/{}, skills={:?})",
            id.0, attrs.0.current_life, attrs.0.max_life.base, slots.0
        ));
    }
    if summary.len() >= 2 {
        summary.sort();
        info!(
            "combatants spawned, ObeliskId == StatBlock.id confirmed: {}",
            summary.join(", ")
        );
        *done = true;
    }
}

/// The M1 cast bind: on `Space` or left-mouse, cast firebolt at the nearest enemy.
#[allow(clippy::type_complexity)]
fn cast_on_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut commands: Commands,
    spatial: ObeliskSpatial,
    mut aim_dirs: ResMut<AimDirs>,
    players: Query<
        (Entity, &ObeliskId, &Transform, &Faction, &SkillSlots),
        (With<PlayerController>, Without<ActiveCast>),
    >,
    transforms: Query<&Transform, With<Combatant>>,
) {
    if !keys.just_pressed(KeyCode::Space) && !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Ok((player, player_id, tf, faction, slots)) = players.single() else {
        return;
    };
    let Some(skill) = slots.0.first().cloned() else {
        return;
    };
    let Some(target) = spatial.nearest_enemy(tf.translation, 20.0, *faction) else {
        info!("cast {skill}: no enemy in range");
        return;
    };
    let target_pos = transforms
        .get(target)
        .map(|t| t.translation)
        .unwrap_or(tf.translation + Vec3::Z);
    let dir = (target_pos - tf.translation)
        .try_normalize()
        .unwrap_or(Vec3::Z);
    aim_dirs.0.insert(player_id.0.clone(), dir);
    info!("cast {skill} at nearest enemy");
    commands.entity(player).cast_skill_at(skill, target);
}

/// The cast-timeline handles being polled to load (skill id -> handle).
#[derive(Resource, Default)]
struct PendingCastAssets(Vec<(String, Handle<CastTimeline>)>);

/// Kick off loading a `.cast.ron` for every registered skill.
fn load_cast_assets(mut commands: Commands, assets: Res<AssetServer>, skills: Res<SkillRegistry>) {
    let mut ids: Vec<String> = skills.0.keys().cloned().collect();
    ids.sort();

    let mut pending = PendingCastAssets::default();
    for id in ids {
        let handle: Handle<CastTimeline> = assets.load(format!("skills/{id}.cast.ron"));
        pending.0.push((id, handle));
    }
    commands.insert_resource(pending);
}

/// Poll the pending cast assets each frame; move loaded ones into `CastTimelineHandles`.
fn poll_cast_assets(
    pending: Option<ResMut<PendingCastAssets>>,
    timelines: Res<Assets<CastTimeline>>,
    mut registry: ResMut<CastTimelineHandles>,
) {
    let Some(mut pending) = pending else {
        return;
    };
    pending.0.retain(|(skill, handle)| {
        if timelines.get(handle).is_some() {
            registry.0.insert(skill.clone(), handle.clone());
            false
        } else {
            true
        }
    });
}

/// Log the registered skills + loaded cast timelines exactly once.
fn log_registered_skills_once(
    mut done: Local<bool>,
    pending: Option<Res<PendingCastAssets>>,
    skills: Res<SkillRegistry>,
    casts: Res<CastTimelineHandles>,
) {
    if *done {
        return;
    }
    if pending.map(|p| !p.0.is_empty()).unwrap_or(true) {
        return;
    }
    let mut skill_ids: Vec<&String> = skills.0.keys().collect();
    skill_ids.sort();
    let mut cast_ids: Vec<&String> = casts.0.keys().collect();
    cast_ids.sort();
    info!(
        "obelisk skills registered: {:?}; cast timelines loaded: {:?}",
        skill_ids, cast_ids
    );
    *done = true;
}

/// Counts rendered frames so the smoke run can exit deterministically.
#[derive(Resource)]
struct SmokeExit {
    target: u64,
    count: u64,
}

fn smoke_exit_after_frames(mut smoke: ResMut<SmokeExit>, mut exit: MessageWriter<AppExit>) {
    smoke.count += 1;
    if smoke.count >= smoke.target {
        info!("arena_game smoke: reached {} frames, exiting", smoke.count);
        exit.write(AppExit::Success);
    }
}

/// Parse a `u64` env var, falling back to `default` if unset or unparseable.
fn env_frame(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

/// Screenshot-harness config, present as a resource only when `ARENA_SHOT` is set.
#[derive(Resource)]
struct ScreenshotConfig {
    path: PathBuf,
    shot_frame: u64,
    count: u64,
    fired: bool,
}

impl ScreenshotConfig {
    fn from_env() -> Option<Self> {
        let path = std::env::var_os("ARENA_SHOT")?;
        Some(Self {
            path: PathBuf::from(path),
            shot_frame: env_frame("ARENA_SHOT_FRAME", 120),
            count: 0,
            fired: false,
        })
    }
}

fn screenshot_system(
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

/// Auto-cast-harness config, present as a resource only when `ARENA_AUTOCAST=1`.
#[derive(Resource)]
struct AutocastConfig {
    cast_frame: u64,
    count: u64,
    fired: bool,
}

impl AutocastConfig {
    fn from_env() -> Option<Self> {
        if std::env::var("ARENA_AUTOCAST").ok().as_deref() != Some("1") {
            return None;
        }
        Some(Self {
            cast_frame: env_frame("ARENA_AUTOCAST_FRAME", 30),
            count: 0,
            fired: false,
        })
    }
}

fn autocast_system(
    mut commands: Commands,
    mut cfg: ResMut<AutocastConfig>,
    mut aim_dirs: ResMut<AimDirs>,
    combatants: Query<(Entity, &ObeliskId, &Transform), With<Combatant>>,
) {
    cfg.count += 1;
    if cfg.fired || cfg.count < cfg.cast_frame {
        return;
    }

    let mut player = None;
    let mut dummy = None;
    for (entity, id, tf) in &combatants {
        match id.0.as_str() {
            "player" => player = Some((entity, tf.translation)),
            "dummy" => dummy = Some((entity, tf.translation)),
            _ => {}
        }
    }

    if let (Some((player, player_pos)), Some((dummy, dummy_pos))) = (player, dummy) {
        info!(
            "arena_game autocast: frame {}, player casts firebolt at dummy",
            cfg.count
        );
        let dir = (dummy_pos - player_pos).try_normalize().unwrap_or(Vec3::Z);
        aim_dirs.0.insert("player".to_string(), dir);
        commands.entity(player).cast_skill_at("firebolt", dummy);
        cfg.fired = true;
    } else {
        warn!(
            "arena_game autocast: frame {}, player/dummy not found yet (player={:?}, dummy={:?})",
            cfg.count,
            player.is_some(),
            dummy.is_some()
        );
    }
}
