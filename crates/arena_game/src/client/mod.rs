//! Client-side present + gameplay layer (M1 co-located logic, now lib-housed).
//!
//! M2.1 keeps the M1 single-process gameplay working through `run_windowed_client`, which the
//! `arena-client` bin calls. Later M2 tasks (§6) split this into net-driven prediction +
//! interpolation; for now it's the M1 firebolt-cast + cosmetics + rig stack, unchanged, plus the
//! lightyear client net stack layered on so two clients can connect to the dedicated server.

pub mod controller;
pub mod cosmetics;
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

use crate::arena_root;

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
        |cue: On<CueEvent>,
         index: Res<ObeliskEntityIndex>,
         mut writer: MessageWriter<LocalCue>| {
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
    app.add_plugins(crate::trace::TracePlugin);
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

    app.run();
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
