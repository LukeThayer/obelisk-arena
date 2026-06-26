mod trace;

use bevy::prelude::*;
use obelisk_bevy::prelude::*;
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

    app.add_systems(Startup, (setup_scene, load_cast_assets));
    // Poll the pending cast timelines each frame; move loaded ones into CastTimelineHandles.
    app.add_systems(Update, (poll_cast_assets, log_registered_skills_once));

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
