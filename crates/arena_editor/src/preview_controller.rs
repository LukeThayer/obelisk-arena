//! The preview lifecycle controller (UX spec P3): a PERSISTENT STAGE the whole session long.
//!
//! - `spawn_preview_floor` (Startup): the arena floor + visible slab, never despawned.
//! - `ensure_stage` (Update): the caster (+rig) and dummies exist while EDITING — visible
//!   during scrubbing, synced to the edited skill (retargeting skills get a second dummy).
//!   Stage entities are NOT `GameEntity`.
//! - `start_preview` (on `GameStartedEvent` / Play): `reset_stage()` + `stage_cast()` — the
//!   same deterministic reset/cast path the scrubber uses, at live speed.
//! - `reset_stage_on_reset` (on `GameResetEvent`): heal + reposition + clear in-flight state
//!   instead of despawning.
//! - `PreviewStageReset`/`stage_cast` are the shared reset/cast machinery (scrub restart, Play,
//!   editor Reset), keeping every path identical: reseeded RNG, cleared cooldowns, refilled
//!   mana — every replay is the same replay.

use crate::model::{derive_vfx_cues, EditedSkill};
use arena_sim::preview::{PreviewCaster, PreviewDummy};
use arena_sim::spawn::{make_arena_combatant, spawn_arena_floor, SPAWN_MARKERS};
use bevy::prelude::*;
use bevy_editor_game::{GameCamera, GameResetEvent, GameStartedEvent, GameState};
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
                    // Chained: the stage must exist (commands applied at the auto sync point)
                    // before Play's cast looks for the caster.
                    (ensure_stage, start_preview).chain(),
                    reset_stage_on_reset,
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

/// Marks a persistent stage combatant's home position (heal/reposition target on reset).
#[derive(Component)]
pub struct StagePost(pub Vec3);

/// The PERSISTENT STAGE (UX spec P3): the caster + dummies exist the whole session — visible
/// while editing and scrubbing, reused by Play. Spawns whatever is missing and syncs the dummy
/// count to the edited skill (retargeting skills stage a second dummy to hop to). NOT
/// `GameEntity`: the editor's Reset heals/repositions instead of despawning (see
/// `reset_stage_on_reset`).
pub fn ensure_stage(
    edited: Res<EditedSkill>,
    casters: Query<(), With<PreviewCaster>>,
    dummies: Query<Entity, With<PreviewDummy>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut commands: Commands,
) {
    type MeshMat<'w> = Option<(ResMut<'w, Assets<Mesh>>, ResMut<'w, Assets<StandardMaterial>>)>;
    let mut meshmat: MeshMat = meshes.zip(materials);
    if casters.is_empty() {
        let caster = make_arena_combatant(
            &mut commands,
            "preview_caster",
            Faction::Player,
            SPAWN_MARKERS[0],
        );
        commands.entity(caster).insert((
            PreviewCaster,
            StagePost(SPAWN_MARKERS[0]),
            // Visibility on the root: a parent without InheritedVisibility silently unrenders
            // the rig subtree (Bevy B0004).
            Visibility::default(),
        ));
    }
    let retargets = edited.timeline.collision_windows.iter().any(|w| {
        [&w.on_end.hit, &w.on_end.world, &w.on_end.fuse]
            .into_iter()
            .flatten()
            .any(|r| matches!(r, obelisk_bevy::assets::EndReaction::Retarget { .. }))
    });
    let want = if retargets { 2 } else { 1 };
    let have = dummies.iter().count();
    for i in have..want {
        let pos = SPAWN_MARKERS[1] + Vec3::new(0.0, 0.0, 2.5) * i as f32;
        let dummy = make_arena_combatant(
            &mut commands,
            if i == 0 { "preview_dummy" } else { "preview_dummy_2" },
            Faction::Enemy,
            pos,
        );
        commands
            .entity(dummy)
            .insert((PreviewDummy, StagePost(pos), Visibility::default()));
        // Windowed, give the (rig-less) dummy a visible capsule; headless test apps have no
        // StandardMaterial assets and skip it.
        if let Some((meshes, materials)) = meshmat.as_mut() {
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
    }
    for (i, e) in dummies.iter().enumerate() {
        if i >= want {
            commands.entity(e).despawn();
        }
    }
}

/// Everything a stage reset / stage cast needs, bundled (shared by the scrub restart, Play,
/// and the editor's Reset).
#[derive(bevy::ecs::system::SystemParam)]
pub struct PreviewStageReset<'w, 's> {
    pub commands: Commands<'w, 's>,
    casters: Query<'w, 's, Entity, With<PreviewCaster>>,
    dummies: Query<'w, 's, Entity, (With<PreviewDummy>, Without<PreviewCaster>)>,
    stage: Query<
        'w,
        's,
        (
            &'static mut obelisk_bevy::prelude::Attributes,
            &'static StagePost,
            &'static mut avian3d::prelude::Position,
            &'static mut avian3d::prelude::LinearVelocity,
        ),
    >,
    hitboxes: Query<'w, 's, Entity, With<obelisk_bevy::prelude::Hitbox>>,
    cosmetics: Query<'w, 's, &'static mut crate::preview_cosmetics::CosmeticLifetime>,
    rng: ResMut<'w, obelisk_bevy::prelude::CombatRng>,
    cooldowns: ResMut<'w, obelisk_bevy::prelude::Cooldowns>,
    handles: ResMut<'w, CastTimelineHandles>,
    timelines: ResMut<'w, Assets<CastTimeline>>,
}

impl PreviewStageReset<'_, '_> {
    /// Reset the stage to a clean, deterministic instant: despawn in-flight hitboxes and
    /// cosmetics, interrupt any live cast, heal + refill everyone, reposition to home posts,
    /// reseed the combat RNG (seed-0 default), clear cooldowns.
    pub fn reset_stage(&mut self) {
        for e in self.hitboxes.iter() {
            self.commands.entity(e).try_despawn();
        }
        // Cosmetics EXPIRE in place (the grace path despawns them) rather than hard-despawn:
        // bevy_vfx queues component inserts on fresh vfx entities the same frame, and a
        // same-frame despawn makes those commands panic on a dead entity.
        for mut life in self.cosmetics.iter_mut() {
            life.elapsed = life.duration;
        }
        for e in self.casters.iter() {
            self.commands.entity(e).interrupt_cast();
        }
        for (mut attrs, post, mut pos, mut vel) in self.stage.iter_mut() {
            attrs.0.current_life = attrs.0.max_life.base;
            attrs.0.current_mana = attrs.0.max_mana.base;
            pos.0 = post.0;
            vel.0 = Vec3::ZERO;
        }
        *self.rng = obelisk_bevy::prelude::CombatRng::default();
        *self.cooldowns = Default::default();
    }
}

/// Register `tl` under its skill id and cast it on the stage caster: entity-aimed at the first
/// dummy for beam skills (hitscan acquisition already resolved in the real game), otherwise by
/// direction with the ballistic loft solved to land ON the dummy. `charge` is the cast's
/// charge byte (None = uncharged 1.0×; 85 ≈ a tap).
pub fn stage_cast(reset: &mut PreviewStageReset, tl: CastTimeline, charge: Option<u8>) {
    let skill_id = tl.skill_id.clone();
    let is_beam_skill = tl
        .collision_windows
        .iter()
        .any(|w| matches!(w.motion, obelisk_bevy::assets::VolumeMotion::Beam));
    let aim = preview_aim(&tl, SPAWN_MARKERS[0], SPAWN_MARKERS[1]);
    let handle = reset.timelines.add(tl);
    reset.handles.0.insert(skill_id.clone(), handle);
    let Some(caster) = reset.casters.iter().next() else {
        return;
    };
    let dummy = reset.dummies.iter().next();
    reset.commands.entity(caster).grant_skill(skill_id.clone());
    match (is_beam_skill, dummy) {
        (true, Some(dummy)) => {
            reset
                .commands
                .entity(caster)
                .cast_skill_at_charged(skill_id, dummy, charge.unwrap_or(85));
        }
        _ => {
            let dir = Dir3::new(aim).unwrap_or(Dir3::X);
            match charge {
                Some(c) => reset
                    .commands
                    .entity(caster)
                    .cast_skill_dir_charged(skill_id, dir, c),
                None => reset.commands.entity(caster).cast_skill_dir(skill_id, dir),
            };
        }
    }
}

/// On a `GameStartedEvent` (Play): reset the stage and cast the edited skill on it — the same
/// deterministic path the scrubber uses, at live speed.
pub fn start_preview(
    mut started: MessageReader<GameStartedEvent>,
    edited: Res<EditedSkill>,
    scrub: Option<Res<crate::scrub::ScrubSim>>,
    mut reset: PreviewStageReset,
) {
    if started.read().next().is_none() {
        return;
    }
    let mut tl = edited.timeline.clone();
    tl.vfx_cues = derive_vfx_cues(&tl);
    let charge = scrub.map(|s| s.charge);
    reset.reset_stage();
    stage_cast(&mut reset, tl, charge);
}

/// The editor's Reset heals + repositions the persistent stage (it is not `GameEntity`, so the
/// upstream despawn pass leaves it alone).
pub fn reset_stage_on_reset(mut ev: MessageReader<GameResetEvent>, mut reset: PreviewStageReset) {
    if ev.read().next().is_some() {
        reset.reset_stage();
    }
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
