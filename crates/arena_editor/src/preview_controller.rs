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
use bevy_editor_game::{GameEntity, GameResetEvent, GameStartedEvent};
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
                (start_preview, sync_playhead, clear_playhead_on_reset),
            );
    }
}

/// Spawn the persistent arena floor (not a `GameEntity` — survives Reset).
pub fn spawn_preview_floor(mut commands: Commands) {
    spawn_arena_floor(&mut commands);
}

/// On a `GameStartedEvent`: register the edited timeline (with derived cues) into
/// `CastTimelineHandles`, spawn the `GameEntity`-tagged caster+dummy duel, grant the skill, and cast.
pub fn start_preview(
    mut started: MessageReader<GameStartedEvent>,
    edited: Res<EditedSkill>,
    mut handles: ResMut<CastTimelineHandles>,
    mut timelines: ResMut<Assets<CastTimeline>>,
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
        .insert((PreviewCaster, GameEntity))
        .grant_skill(skill_id.clone());
    let dummy = make_arena_combatant(
        &mut commands,
        "preview_dummy",
        Faction::Enemy,
        SPAWN_MARKERS[1],
    );
    commands.entity(dummy).insert((PreviewDummy, GameEntity));

    // Cast by DIRECTION (caster→dummy), NOT `cast_skill_at(dummy)`: obelisk's entity-aim LOS raycast
    // excludes only the caster body entity, not its CHILD `Hurtbox` sensor, so an arena combatant
    // self-blocks the ray (`NoLineOfSight`). The live game sidesteps this identically with free-aim
    // direction casts (see arena_sim `preview_smoke`), so the preview matches what the game plays.
    let dir = Dir3::new(SPAWN_MARKERS[1] - SPAWN_MARKERS[0]).unwrap_or(Dir3::X);
    commands.entity(caster).cast_skill_dir(skill_id, dir);
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
