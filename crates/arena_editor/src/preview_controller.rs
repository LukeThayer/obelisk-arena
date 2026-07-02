//! The preview lifecycle controller: turns the editor's Play/Reset into a real obelisk duel that
//! casts the CURRENTLY-EDITED timeline, so "Play the real skill" runs exactly what you author.
//!
//! - `spawn_preview_floor` (Startup, persistent): the arena floor sits under the editor the whole
//!   session (it is NOT a `GameEntity`, so Reset leaves it in place).
//! - `start_preview` (on `GameStartedEvent`): registers `EditedSkill`'s timeline (with freshly
//!   derived vfx cues) into `CastTimelineHandles`, spawns a Player `PreviewCaster` + Enemy
//!   `PreviewDummy` duel — both tagged `GameEntity` so the editor despawns them on Reset — grants
//!   the skill, and casts caster→dummy through the real deterministic sim.
//!
//! This retires Task 13's idle `spawn_preview_on_startup`: the floor is persistent (here), the
//! combatants are spawned on Play (not at boot).

use crate::model::{derive_vfx_cues, EditedSkill};
use arena_sim::preview::{PreviewCaster, PreviewDummy};
use arena_sim::spawn::{make_arena_combatant, spawn_arena_floor, SPAWN_MARKERS};
use bevy::prelude::*;
use bevy_editor_game::{GameCamera, GameEntity, GameResetEvent, GameStartedEvent, GameState};
use obelisk_bevy::prelude::{
    ActiveCast, CastSkillExt, CastTimeline, CastTimelineHandles, Faction, ObeliskCommandsExt,
    SkillPhase,
};

/// Registers the preview lifecycle: a persistent floor at Startup + the Play→duel handler.
pub struct PreviewControllerPlugin;

impl Plugin for PreviewControllerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Playhead>()
            .add_systems(Startup, spawn_preview_floor)
            .add_systems(
                Update,
                (
                    start_preview,
                    sync_playhead,
                    clear_playhead_on_reset,
                    keep_editor_camera_during_play,
                ),
            );
    }
}

/// Spawn the persistent arena floor (not a `GameEntity` — survives Reset). Windowed, it also gets a
/// visible slab matching the collider (the editor hides its grid during Play, so a mesh-less floor
/// renders the duel in a void); headless test apps have no `StandardMaterial` assets and skip it.
pub fn spawn_preview_floor(
    mut commands: Commands,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    spawn_arena_floor(&mut commands);
    if let (Some(mut meshes), Some(mut materials)) = (meshes, materials) {
        commands.spawn((
            Name::new("PreviewFloorVisual"),
            Mesh3d(meshes.add(Cuboid::new(40.0, 1.0, 40.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.25, 0.25, 0.28),
                ..default()
            })),
            // Top face 1 mm below world 0 so it doesn't z-fight the editor grid while editing.
            Transform::from_xyz(0.0, -0.501, 0.0),
        ));
    }
}

/// On a `GameStartedEvent`: register the edited timeline (with derived cues) into
/// `CastTimelineHandles`, spawn the `GameEntity`-tagged caster+dummy duel, grant the skill, and cast.
pub fn start_preview(
    mut started: MessageReader<GameStartedEvent>,
    edited: Res<EditedSkill>,
    mut handles: ResMut<CastTimelineHandles>,
    mut timelines: ResMut<Assets<CastTimeline>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut commands: Commands,
) {
    if started.read().next().is_none() {
        return;
    }
    let mut tl = edited.timeline.clone();
    tl.vfx_cues = derive_vfx_cues(&tl);
    let skill_id = tl.skill_id.clone();
    let handle = timelines.add(tl);
    handles.0.insert(skill_id.clone(), handle);

    let caster = make_arena_combatant(
        &mut commands,
        "preview_caster",
        Faction::Player,
        SPAWN_MARKERS[0],
    );
    commands
        .entity(caster)
        // Visibility on the combatant root: the rig scene hangs under it, and a parent without
        // `InheritedVisibility` breaks the child subtree's visibility propagation (Bevy B0004 —
        // the rig silently doesn't render).
        .insert((PreviewCaster, GameEntity, Visibility::default()))
        .grant_skill(skill_id.clone());
    let dummy = make_arena_combatant(
        &mut commands,
        "preview_dummy",
        Faction::Enemy,
        SPAWN_MARKERS[1],
    );
    commands
        .entity(dummy)
        .insert((PreviewDummy, GameEntity, Visibility::default()));
    // Windowed, make the (rig-less) dummy visible so there's something to shoot at; headless test
    // apps have no `StandardMaterial` assets and skip it. The caster is rendered by its glb rig
    // (`preview_rig`), so its capsule stays mesh-less like the game's proxy bodies.
    if let (Some(mut meshes), Some(mut materials)) = (meshes, materials) {
        commands.entity(dummy).insert((
            Mesh3d(meshes.add(Capsule3d::new(
                arena_sim::tuning::PLAYER_CAPSULE_RADIUS,
                arena_sim::tuning::PLAYER_CAPSULE_LENGTH,
            ))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.7, 0.25, 0.2),
                ..default()
            })),
        ));
    }

    // Cast by DIRECTION (caster→dummy), NOT `cast_skill_at(dummy)`: obelisk's entity-aim LOS raycast
    // excludes only the caster body entity, not its CHILD `Hurtbox` sensor, so an arena combatant
    // self-blocks the ray (`NoLineOfSight`). The live game sidesteps this identically with free-aim
    // direction casts (see arena_sim `preview_smoke`), so the preview matches what the game plays.
    //
    // For a BALLISTIC window, loft the aim like a free-looking player would: solve the launch
    // pitch that lands the arc ON the dummy (level aim would ground the bolt short of it).
    let aim = preview_aim(&edited.timeline, SPAWN_MARKERS[0], SPAWN_MARKERS[1]);
    let dir = Dir3::new(aim).unwrap_or(Dir3::X);
    commands.entity(caster).cast_skill_dir(skill_id, dir);
}

/// The preview's cast direction from `from` toward `to`: straight for non-ballistic skills, the
/// low-arc ballistic solution (first `Ballistic` window's speed/gravity) for arcing ones — the
/// aim a free-looking player compensating for gravity would take.
pub fn preview_aim(tl: &CastTimeline, from: Vec3, to: Vec3) -> Vec3 {
    let ballistic = tl.collision_windows.iter().find_map(|w| match w.motion {
        obelisk_bevy::assets::VolumeMotion::Ballistic { speed, gravity } => Some((speed, gravity)),
        _ => None,
    });
    match ballistic {
        Some((speed, gravity)) => arena_sim::ballistics::ballistic_launch_dir(from, to, speed, gravity),
        None => (to - from).normalize_or(Vec3::X),
    }
}

/// While Playing, upstream `sync_camera_states` deactivates the editor camera and activates only
/// `GameCamera`-tagged cameras, expecting the game to provide its own view. The skill preview
/// deliberately provides NONE: Play must not move the camera — the duel is watched from exactly
/// the editor camera's current view (which is also the view the `bevy_vfx` billboard pipeline
/// demonstrably renders on). So while Playing with no `GameCamera` in the world, re-assert the
/// editor camera as the active view. Pause/Reset hand control back through the upstream sync as
/// usual, and a future real game view that DOES spawn a `GameCamera` wins automatically.
pub fn keep_editor_camera_during_play(
    game_state: Res<State<GameState>>,
    game_cameras: Query<(), (With<GameCamera>, Without<bevy_modal_editor::EditorCamera>)>,
    mut editor_cameras: Query<&mut Camera, With<bevy_modal_editor::EditorCamera>>,
) {
    if *game_state.get() != GameState::Playing || !game_cameras.is_empty() {
        return;
    }
    for mut cam in &mut editor_cameras {
        if !cam.is_active {
            cam.is_active = true;
        }
    }
}

/// The timeline scrubber the panel reads to draw where playback is: mirrors the `PreviewCaster`'s
/// live `ActiveCast` (phase / elapsed / total effective duration), or an idle default when no cast
/// is in flight. Cleared on Reset so the panel shows a fresh, non-playing timeline.
#[derive(Resource, Default)]
pub struct Playhead {
    pub active: bool,
    pub phase: Option<SkillPhase>,
    pub elapsed: f32,
    pub total: f32,
}

/// Mirror the `PreviewCaster`'s `ActiveCast` into `Playhead` each frame; go idle when there is none.
pub fn sync_playhead(mut ph: ResMut<Playhead>, q: Query<&ActiveCast, With<PreviewCaster>>) {
    if let Ok(ac) = q.single() {
        ph.active = true;
        ph.phase = Some(ac.phase);
        ph.elapsed = ac.elapsed;
        ph.total = ac.total_duration();
    } else {
        ph.active = false;
        ph.phase = None;
    }
}

/// On a `GameResetEvent`, reset the playhead to its idle default.
pub fn clear_playhead_on_reset(mut ph: ResMut<Playhead>, mut ev: MessageReader<GameResetEvent>) {
    if ev.read().next().is_some() {
        *ph = Playhead::default();
    }
}
