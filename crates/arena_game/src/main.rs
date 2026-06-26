mod controller;
mod cosmetics;
mod rig;
mod trace;

use arena_skills::{cue_event_to_message, SkillFxRegistry};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use controller::{ArenaControllerPlugin, FollowCamera, PlayerController};
use cosmetics::{age_lifetimes, fly_cosmetic_projectiles, spawn_cue_cosmetics, AimDirs, LocalCue};
use obelisk_bevy::prelude::*;
use rig::{ArenaBody, LocalAnimBlend};
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
    // The arena cosmetic-binding layer: registers the SkillFx asset + its `.skillfx.ron` loader.
    // The cue egress (CueEvent → serde CueMessage → LocalCue) is wired by `register_cue_egress`.
    app.add_plugins(arena_skills::ArenaSkillsPlugin);
    // Third-person controller: follow camera + camera-relative WASD movement +
    // the chest_joint aim spine-pitch (scheduled PostUpdate, between animation
    // and transform propagation). Registers CameraYaw/AimPitch/PlayerVelocity.
    app.add_plugins(ArenaControllerPlugin);
    app.add_plugins(trace::TracePlugin);
    // Obelisk runs its sim on the 60 Hz fixed timestep.
    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    // Wire the M2 cue egress: the SkillFxRegistry (cue_id → lanes), the LocalCue message channel,
    // and the CueEvent → serde-CueMessage egress observer (all installed before `run()`). The
    // egress observer resolves source Entity → ObeliskId so the cue is process-independent — the
    // exact shape that crosses the wire in M2.3. The cosmetics consumer re-looks-up the lanes.
    register_cue_egress(&mut app, &root);

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
    // `load_rig` kicks off the async `character.glb` load + inserts `RigAssets` before
    // `spawn_combatants` reads the gltf scene handle for the player's `SceneRoot`.
    app.add_systems(
        Startup,
        (setup_scene, load_rig, load_cast_assets, spawn_combatants).chain(),
    );
    // Per-caster aim direction, written when a cast is issued (autocast / real cast) and read by
    // `spawn_cue_cosmetics` to fly the cosmetic projectile in the right direction.
    app.init_resource::<AimDirs>();

    // Poll the pending cast timelines each frame; move loaded ones into CastTimelineHandles.
    app.add_systems(
        Update,
        (
            poll_cast_assets,
            log_registered_skills_once,
            confirm_combatants_once,
        ),
    );

    // Character rig: build the AnimationGraph once `character.glb` loads, attach an
    // AnimationPlayer (playing idle) to the scene's animation-target entity once it spawns, and
    // costume-cull the unified rig down to ONE outfit (Witch/wizard) once its meshes appear.
    app.add_systems(
        Update,
        (
            rig::build_graph_when_loaded,
            rig::attach_animation_graph,
            rig::cull_costume,
            // Per-frame blend driver: locomotion (PlayerVelocity) + casting layer
            // (player's ActiveCast.phase). Ordered after the graph is attached so the
            // AnimationPlayer exists; its weights are read by the animation systems in
            // PostUpdate this same frame.
            rig::drive_animation.after(rig::attach_animation_graph),
        ),
    );

    // Cosmetics: consume CueMessages → spawn emissive bursts + flying projectiles, fly them, age
    // them out. `spawn_cue_cosmetics` reads the messages the cue observers wrote this frame.
    app.add_systems(
        Update,
        (spawn_cue_cosmetics, fly_cosmetic_projectiles, age_lifetimes),
    );

    // The real cast bind: Space or left-mouse casts firebolt at the nearest enemy (via the
    // obelisk `ObeliskSpatial` facade), recording the aim dir for the cosmetic projectile.
    app.add_systems(Update, cast_on_input);

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
    // Over-the-shoulder follow camera. `FollowCamera` marks it so the controller can place it
    // behind + above the player each frame (it follows the player's translation + camera yaw).
    // The initial transform just frames the spawn origin head-on before the controller takes over.
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

/// Kick off the async load of the player character rig (`character.glb`) and insert
/// `RigAssets` holding the `Handle<Gltf>`. `build_graph_when_loaded` polls this handle each
/// frame and builds the `AnimationGraph` once the gltf resolves. Runs in the Startup chain
/// before `spawn_combatants` so the rig resource exists when the player's `SceneRoot` is spawned.
fn load_rig(mut commands: Commands, assets: Res<AssetServer>) {
    let gltf: Handle<bevy::gltf::Gltf> = assets.load("character.glb");
    commands.insert_resource(rig::RigAssets::new(gltf));
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
    assets: Res<AssetServer>,
) {
    let capsule = meshes.add(Capsule3d::new(0.3, 1.0));

    // Player: id "player", origin, rendered as the rigged character (idle clip). The body is a
    // child entity carrying the `SceneRoot` of `character.glb` + the `ArenaBody` marker — mirrors
    // wisp's `spawn_player` body hierarchy. The glTF's default forward is +Z but Bevy uses -Z, so
    // the body is yawed by π to face the camera. The combatant root sits at y=0 and the character
    // glTF's origin is at its feet, so no vertical offset is needed for the feet to rest on the
    // ground plane.
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
            // `PlayerController` marks the combatant root the third-person controller drives
            // (camera-relative WASD writes this Transform directly; the follow cam tracks it).
            PlayerController,
            // Persisted cast-animation blend state, eased toward the `ActiveCast.phase` target
            // each frame by `drive_animation` so the casting layer cross-fades in/out.
            LocalAnimBlend::default(),
            Transform::from_xyz(0.0, 0.0, 0.0),
            Visibility::default(),
        ))
        .id();
    commands.entity(player).add_child(player_body);
    commands.entity(player).grant_skill("firebolt");

    // Dummy: id "dummy", 6 units down +Z, red, no skills.
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
    // Give the dummy a hurtbox so the firebolt's moving Sphere(0.5) collision window can overlap it
    // and obelisk fires the OnHit cue (→ the impact burst). Without this, only the muzzle cue fires.
    // `insert_hurtbox` (re)sets the entity's Transform to `pos`, so pass the dummy's spawn position
    // to keep it in place. Radius 0.6 mirrors the examples + the Task 6 smoke test.
    insert_hurtbox(&mut commands, dummy, 0.6, dummy_pos);
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
        // debug_assert so a shipping build can't panic here — make_combatant already guarantees it.
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
        // Single concise confirmation: both combatants spawned, ObeliskId == StatBlock.id each.
        info!(
            "combatants spawned, ObeliskId == StatBlock.id confirmed: {}",
            summary.join(", ")
        );
        *done = true;
    }
}

/// The real cast bind (Task 12 GATE): on `Space` or left-mouse, cast the player's first granted
/// skill (firebolt) at the nearest enemy, found via the obelisk `ObeliskSpatial` facade.
///
/// Mirrors `examples/playground.rs::free_cast_on_space`: pick the Player-faction combatant with a
/// skill, `nearest_enemy(origin, range, faction)`, then `cast_skill_at`. The aim direction
/// (player → target) is recorded into [`AimDirs`] (keyed by the caster) so `spawn_cue_cosmetics`
/// flies the cosmetic projectile the right way — the `OnCast` cue carries only the caster position.
/// Skips while a cast is already active (obelisk rejects concurrent casts anyway).
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
    // firebolt's `.cast.ron` targeting range is 15.0; use a slightly wider acquisition radius so a
    // target just at the edge is still picked, then obelisk's `validate_casts` enforces the rule.
    let Some(target) = spatial.nearest_enemy(tf.translation, 20.0, *faction) else {
        info!("cast {skill}: no enemy in range");
        return;
    };
    // Stash the aim dir (caster → target) for the cosmetic projectile, keyed by the caster's
    // stable ObeliskId (matching the serde CueMessage.source_id the OnCast cue carries).
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

/// The cast-timeline handles being polled to load (skill id -> handle). Drained into
/// `CastTimelineHandles` once each asset finishes loading. Mirrors examples/playground.rs.
#[derive(Resource, Default)]
struct PendingCastAssets(Vec<(String, Handle<CastTimeline>)>);

/// Kick off loading a `.cast.ron` for every registered skill. `DefaultPlugins` sets
/// `AssetPlugin::file_path = "assets"`, so paths are relative to that folder
/// (e.g. "skills/firebolt.cast.ron").
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
/// At `cast_frame` it fires one `firebolt` from the player at the dummy so a cast can be
/// captured in progress. It drives the full cosmetics pipeline: the system stashes the
/// caster → target aim into [`AimDirs`], and the resulting cues fire the particle burst +
/// cosmetic projectile alongside the obelisk damage resolution.
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
        // Stash the aim direction (caster → target) for the cosmetic projectile, keyed by the
        // caster's stable ObeliskId ("player") — `spawn_cue_cosmetics` reads this by the OnCast
        // cue's `source_id`.
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
