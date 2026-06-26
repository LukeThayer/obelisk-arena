mod trace;

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use obelisk_bevy::prelude::*;
use stat_core::StatBlock;
use std::path::PathBuf;

/// The arena workspace root, holding `assets/` (cast timelines) and `config/` (skill + effect
/// rules). Resolved so the binary works regardless of the launch directory: under `cargo run`,
/// `CARGO_MANIFEST_DIR` is `crates/arena_game`, so the root is two levels up; otherwise fall back
/// to the current working directory. Anchoring everything here keeps the AssetServer's file root
/// (which Bevy bases on `CARGO_MANIFEST_DIR`, i.e. the crate dir) consistent with the `std::fs`
/// config dirs (which resolve relative to the cwd).
fn arena_root() -> PathBuf {
    match std::env::var_os("CARGO_MANIFEST_DIR") {
        Some(dir) => PathBuf::from(dir)
            .ancestors()
            .nth(2)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

fn main() {
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
    app.add_plugins(trace::TracePlugin);
    // Obelisk runs its sim on the 60 Hz fixed timestep.
    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    // Obelisk global config: stat constants + the effect/skill registries + a fixed RNG seed.
    // (Mirrors examples/playground.rs's real recipe; `add_obelisk_effects` is the arena-level
    // verb wrapping the guarded `stat_core::init_effect_registry`.)
    app.add_obelisk_config_constants_default();
    // firebolt's .toml applies the `burn` effect, so the effect registry must be populated from
    // the effects dir before the skill is used. `add_obelisk_effects` is idempotent (guarded).
    app.add_obelisk_effects(&root.join("config/effects"));
    // Load every skill `.toml` (the obelisk `Skill` rules) from config/skills into SkillRegistry.
    app.add_obelisk_skills(SkillSource::Dir(root.join("config/skills")));
    // Single deterministic combat RNG seed.
    app.seed_combat_rng(1);

    // `spawn_combatants` runs after `setup_scene` (mesh/material assets + camera) and after the
    // skill wiring above, so the player can be granted "firebolt" once the registry is populated.
    app.add_systems(
        Startup,
        (setup_scene, load_cast_assets, spawn_combatants).chain(),
    );
    // Poll the pending cast timelines each frame; move loaded ones into CastTimelineHandles.
    app.add_systems(
        Update,
        (
            poll_cast_assets,
            log_registered_skills_once,
            confirm_combatants_once,
        ),
    );

    // Non-interactive smoke verification: if ARENA_SMOKE_FRAMES is set, exit
    // after that many rendered frames so the renderer can be verified without a
    // human closing the window. Without the env var, the window stays open.
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

    // Visual-verification harness (inert unless its env vars are set):
    //   ARENA_SHOT=<path>  -> save a PNG of the primary window, then exit.
    //   ARENA_AUTOCAST=1   -> fire one firebolt from player at dummy.
    // Both are env-gated inside their systems, so they're always registered but
    // no-op when the env var is absent. See `screenshot_config`/`autocast_config`.
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
        Transform::from_xyz(0.0, 6.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
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

/// Spawn the two combatants the arena starts with: a `Player`-faction "player" at the origin,
/// granted "firebolt" and drawn as a blue capsule; and an `Enemy`-faction "dummy" 6 units down +Z,
/// no skills, drawn as a red capsule. Mirrors the examples' real spawn pattern
/// (`spawn_empty().make_combatant(block).insert((...)).id()` then `grant_skill`), building each
/// `StatBlock` with the real `StatBlock::with_id` constructor (the guide's `make_stat_block(id)`
/// placeholder). `make_combatant` sets `ObeliskId == block.id`, the netcode invariant.
fn spawn_combatants(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let capsule = meshes.add(Capsule3d::new(0.3, 1.0));

    // Player: id "player", origin, blue, granted firebolt.
    let player = commands
        .spawn_empty()
        .make_combatant(StatBlock::with_id("player"))
        .insert((
            Faction::Player,
            Transform::from_xyz(0.0, 0.0, 0.0),
            Mesh3d(capsule.clone()),
            MeshMaterial3d(materials.add(Color::srgb(0.2, 0.5, 1.0))),
        ))
        .id();
    commands.entity(player).grant_skill("firebolt");

    // Dummy: id "dummy", 6 units down +Z, red, no skills.
    commands
        .spawn_empty()
        .make_combatant(StatBlock::with_id("dummy"))
        .insert((
            Faction::Enemy,
            Transform::from_xyz(0.0, 0.0, 6.0),
            Mesh3d(capsule),
            MeshMaterial3d(materials.add(Color::srgb(1.0, 0.3, 0.2))),
        ));
}

/// One-shot readback (fires once): confirm both combatants spawned with `ObeliskId == StatBlock.id`
/// (the `make_combatant` invariant) and log their ids + hp. Runs after the spawn commands have
/// flushed, so it sees the inserted components.
fn confirm_combatants_once(
    mut done: Local<bool>,
    combatants: Query<(&ObeliskId, &Attributes, &SkillSlots), With<Combatant>>,
) {
    if *done {
        return;
    }
    let mut summary: Vec<String> = Vec::new();
    for (id, attrs, slots) in &combatants {
        // The invariant make_combatant enforces: the stable string id equals the StatBlock id.
        assert_eq!(
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
        // Single concise confirmation: both combatants spawned, ObeliskId == StatBlock.id each.
        info!(
            "combatants spawned, ObeliskId == StatBlock.id confirmed: {}",
            summary.join(", ")
        );
        *done = true;
    }
}

/// The cast-timeline handles being polled to load (skill id -> handle). Drained into
/// `CastTimelineHandles` once each asset finishes loading. Mirrors examples/playground.rs.
#[derive(Resource, Default)]
struct PendingCastAssets(Vec<(String, Handle<CastTimeline>)>);

/// Kick off loading a `.cast.ron` for every registered skill. `DefaultPlugins` sets
/// `AssetPlugin::file_path = "assets"`, so paths are relative to that folder
/// (e.g. "skills/firebolt.cast.ron").
fn load_cast_assets(
    mut commands: Commands,
    assets: Res<AssetServer>,
    skills: Res<SkillRegistry>,
) {
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
            false // loaded — drop from the pending list
        } else {
            true // still loading
        }
    });
}

/// Log the registered skills + loaded cast timelines exactly once, the first frame all pending
/// cast assets have finished loading. Proves the two-file skill registration (`.toml` rules +
/// `.cast.ron` timeline) is fully wired.
fn log_registered_skills_once(
    mut done: Local<bool>,
    pending: Option<Res<PendingCastAssets>>,
    skills: Res<SkillRegistry>,
    casts: Res<CastTimelineHandles>,
) {
    if *done {
        return;
    }
    // Wait until every cast asset has drained out of the pending list.
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

/// Sends `AppExit::Success` once `SmokeExit.target` frames have elapsed.
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
///
/// At `shot_frame` it spawns `Screenshot::primary_window()` with a `save_to_disk(path)`
/// observer (Bevy 0.18's built-in async capture); `shot_frame + 12` frames later it sends
/// `AppExit::Success`, giving the async readback time to flush the PNG to disk. This lets
/// later visual tasks render the scene to a file and diff it without a human in the loop.
#[derive(Resource)]
struct ScreenshotConfig {
    /// Output PNG path (the value of `ARENA_SHOT`).
    path: PathBuf,
    /// Frame on which to spawn the screenshot capture (`ARENA_SHOT_FRAME`, default 120).
    shot_frame: u64,
    /// Frames elapsed so far (incremented every `Update`).
    count: u64,
    /// Whether the capture has already been spawned (fire-once latch).
    fired: bool,
}

impl ScreenshotConfig {
    /// Build from env, or `None` if `ARENA_SHOT` is unset (the harness stays inert).
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

/// At `shot_frame`, capture the primary window to `path`; at `shot_frame + 12`, exit.
///
/// Only registered when `ARENA_SHOT` is set (see `main`), so it never runs in normal play.
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

    // Give the async screenshot save ~12 frames to flush before exiting.
    if cfg.fired && cfg.count >= cfg.shot_frame + 12 {
        info!("arena_game shot: capture flushed, exiting");
        exit.write(AppExit::Success);
    }
}

/// Auto-cast-harness config, present as a resource only when `ARENA_AUTOCAST=1`.
///
/// At `cast_frame` it fires one `firebolt` from the player at the dummy so later tasks can
/// capture a cast in progress. At this stage (pre-Task 9) there are no cosmetics, so the
/// cast just resolves obelisk damage — that's expected.
#[derive(Resource)]
struct AutocastConfig {
    /// Frame on which to fire the cast (`ARENA_AUTOCAST_FRAME`, default 30).
    cast_frame: u64,
    /// Frames elapsed so far.
    count: u64,
    /// Fire-once latch.
    fired: bool,
}

impl AutocastConfig {
    /// Build from env, or `None` unless `ARENA_AUTOCAST == "1"` (the harness stays inert).
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

/// At `cast_frame`, find the player + dummy combatants and cast `firebolt` from player at dummy.
///
/// Only registered when `ARENA_AUTOCAST=1` (see `main`). Identifies combatants by their stable
/// `ObeliskId` ("player" / "dummy") set by `make_combatant`.
fn autocast_system(
    mut commands: Commands,
    mut cfg: ResMut<AutocastConfig>,
    combatants: Query<(Entity, &ObeliskId), With<Combatant>>,
) {
    cfg.count += 1;
    if cfg.fired || cfg.count < cfg.cast_frame {
        return;
    }

    let mut player = None;
    let mut dummy = None;
    for (entity, id) in &combatants {
        match id.0.as_str() {
            "player" => player = Some(entity),
            "dummy" => dummy = Some(entity),
            _ => {}
        }
    }

    if let (Some(player), Some(dummy)) = (player, dummy) {
        info!(
            "arena_game autocast: frame {}, player casts firebolt at dummy",
            cfg.count
        );
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
