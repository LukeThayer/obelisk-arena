//! Task 20: `PreviewControllerPlugin` drives the "Play the real skill" preview lifecycle. On a
//! `GameStartedEvent`, `start_preview` registers the currently-edited timeline (with derived vfx
//! cues) into `CastTimelineHandles`, spawns a `PreviewCaster` + `PreviewDummy` duel (both
//! `GameEntity`-tagged so the editor despawns them on Reset), grants the skill, and casts.
//!
//! Verified on the `arena_sim` headless harness (plain-Avian + `add_obelisk_sim` via
//! `ArenaSimPreviewPlugin`), NOT the full editor (which can't advance a frame headlessly). The
//! deterministic obelisk sim then resolves real damage on the dummy — proving what you author is
//! what the game plays.

use arena_editor::io::{editor_root, load_cast_timeline};
use arena_editor::model::EditedSkill;
use arena_editor::preview_controller::PreviewControllerPlugin;
use arena_sim::preview::{ArenaSimPreviewPlugin, PreviewCaster};
use bevy::prelude::*;
use bevy_editor_game::{GameEntity, GameStartedEvent};
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillSource};
use obelisk_bevy::testkit::{init_test_obelisk, EventRecorder, EventRecorderPlugin};
use std::time::Duration;

fn run(seed: u64, ticks: usize) -> App {
    run_skill(seed, ticks, "firebolt")
}

fn run_skill(seed: u64, ticks: usize, skill: &str) -> App {
    init_test_obelisk();
    let root = editor_root();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin {
            file_path: ".".into(),
            ..default()
        })
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(EventRecorderPlugin)
        .add_plugins(bevy::state::app::StatesPlugin)
        .init_state::<bevy_editor_game::GameState>()
        .add_message::<GameStartedEvent>()
        .add_message::<bevy_editor_game::GameResetEvent>()
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f64(1.0 / 60.0),
        ))
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .add_plugins(ArenaSimPreviewPlugin)
        .add_plugins(PreviewControllerPlugin)
        // The rig attach (the `character.glb` handle never resolves headlessly — no GltfPlugin —
        // but the SceneRoot child must still hang under the caster). No AnimationPlugin headlessly,
        // so register the graph asset the plugin's builder system writes to.
        .init_resource::<bevy_editor_game::AnimationLibrary>()
        .init_asset::<AnimationGraph>()
        .add_plugins(arena_editor::preview_rig::PreviewRigPlugin);
    app.add_obelisk_skills(SkillSource::Dir(root.join("config/skills")));
    app.seed_combat_rng(seed);
    let path = root.join(format!("assets/skills/{skill}.cast.ron"));
    let tl = load_cast_timeline(&path).expect("timeline parses");
    app.insert_resource(EditedSkill::from_timeline(tl, path));
    app.finish();
    app.cleanup();
    app.world_mut().write_message(GameStartedEvent);
    for _ in 0..ticks {
        app.update();
    }
    app
}

#[test]
fn play_resolves_damage_on_the_dummy() {
    let app = run(0xC0FFEE, 90);
    let rec = app.world().resource::<EventRecorder>();
    assert!(!rec.cast_began.is_empty());
    assert!(!rec.damage_resolved.is_empty());
    assert!(
        rec.damage_resolved
            .iter()
            .map(|d| d.total_damage)
            .sum::<f64>()
            > 0.0
    );
}

/// Regression: while Playing, upstream `sync_camera_states` deactivates the editor camera and
/// activates only `GameCamera`s — the preview spawns none (Play must not move the view, and the
/// vfx billboard pipeline renders on the editor camera), so `keep_editor_camera_during_play` must
/// re-assert the editor camera as the active view.
#[test]
fn play_keeps_the_editor_camera_active() {
    let mut app = run(5, 3);
    // Simulate the upstream sync having deactivated the editor camera on the Playing transition.
    let cam = app
        .world_mut()
        .spawn((
            Camera {
                is_active: false,
                ..Default::default()
            },
            bevy_modal_editor::EditorCamera,
        ))
        .id();
    app.world_mut()
        .resource_mut::<NextState<bevy_editor_game::GameState>>()
        .set(bevy_editor_game::GameState::Playing);
    app.update();
    app.update();
    assert!(
        app.world().get::<Camera>(cam).unwrap().is_active,
        "the editor camera must be re-activated while Playing with no GameCamera"
    );
}

#[test]
fn play_spawns_game_entity_tagged_caster() {
    let mut app = run(1, 3);
    let n = app
        .world_mut()
        .query_filtered::<Entity, (With<PreviewCaster>, With<GameEntity>)>()
        .iter(app.world())
        .count();
    assert_eq!(n, 1);
}

#[test]
fn playhead_tracks_active_cast_and_clears_on_reset() {
    use arena_editor::preview_controller::Playhead;
    use bevy_editor_game::GameResetEvent;
    use obelisk_bevy::prelude::SkillPhase;

    // Early in the cast the playhead reflects the caster's ActiveCast (windup phase).
    let mut app = run(7, 12);
    {
        let ph = app.world().resource::<Playhead>();
        assert!(ph.active, "playhead should be active mid-cast");
        assert_eq!(ph.phase, Some(SkillPhase::Windup));
        assert!(ph.total > 0.0, "playhead total duration should be > 0");
    }

    // Reset: despawn the GameEntity duel + fire GameResetEvent -> playhead clears, no PreviewCaster.
    let game_entities: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<GameEntity>>()
        .iter(app.world())
        .collect();
    for e in game_entities {
        app.world_mut().entity_mut(e).despawn();
    }
    app.world_mut().write_message(GameResetEvent);
    app.update();

    let ph = app.world().resource::<Playhead>();
    assert!(!ph.active, "playhead should clear on reset");
    assert_eq!(ph.phase, None);
    let casters = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCaster>>()
        .iter(app.world())
        .count();
    assert_eq!(casters, 0, "no PreviewCaster should remain after reset");
}

/// Regression (skill-editor first light): `spawn_preview_rig` used to gate on `GameStartedEvent`
/// and consume it the same frame `start_preview` merely QUEUED the caster spawn — the caster query
/// was still empty, so the rig never attached and Play rendered nothing. It is now driven by
/// `Added<PreviewCaster>`, so the `character.glb` `SceneRoot` must hang under the caster.
#[test]
fn play_attaches_the_rig_scene_under_the_caster() {
    let mut app = run(2, 3);
    let caster = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCaster>>()
        .single(app.world())
        .expect("one caster");
    let has_rig = app
        .world_mut()
        .query::<(&SceneRoot, &ChildOf)>()
        .iter(app.world())
        .any(|(_, child_of)| child_of.parent() == caster);
    assert!(has_rig, "the character.glb SceneRoot should be a child of the PreviewCaster");
}

/// Increment 1 end-to-end on the REAL firebolt v2 asset: the bolt ends (entity hit) and chains
/// the blast AT its end position, which splashes the dummy — two windows confirm hits, and the
/// end event carries the bolt's termination.
#[test]
fn play_chains_the_blast_where_the_bolt_ends() {
    let app = run(21, 90);
    let rec = app.world().resource::<EventRecorder>();
    assert!(
        rec.hit_confirmed.iter().any(|h| h.window_id == "bolt"),
        "direct bolt hit"
    );
    assert!(
        rec.hit_confirmed.iter().any(|h| h.window_id == "blast"),
        "chained blast splashes the dummy"
    );
    let ended = rec
        .hitbox_ended
        .iter()
        .find(|e| e.window_id == "bolt")
        .expect("bolt end event");
    assert_eq!(
        ended.reason,
        obelisk_bevy::events::EndReason::HitEntity,
        "aimed preview cast ends on the dummy"
    );
}

/// Increment 2 end-to-end on the REAL chain_lightning asset: the preview casts ENTITY-AIMED at
/// the first dummy (hitscan-acquired in the game), the arc beam strikes it, and the retarget
/// hop beams onto the second dummy (spawned automatically for retargeting skills) — two
/// victims, both attributed to the caster, with two-anchor beam cues.
#[test]
fn play_chain_lightning_hops_to_the_second_dummy() {
    let mut app = run_skill(31, 90, "chain_lightning");
    let rec = app.world().resource::<EventRecorder>();
    let arc_hits = rec
        .hit_confirmed
        .iter()
        .filter(|h| h.window_id == "arc")
        .count();
    let hop_hits = rec
        .hit_confirmed
        .iter()
        .filter(|h| h.window_id == "hop")
        .count();
    assert_eq!(arc_hits, 1, "the acquired target is struck");
    assert_eq!(hop_hits, 1, "the chain hops to the second dummy");
    assert_eq!(rec.damage_resolved.len(), 2, "full damage each strike");
    let beam_cues = rec
        .cues
        .iter()
        .filter(|c| c.position_from.is_some())
        .count();
    assert_eq!(beam_cues, 2, "each beam window cue carries both anchors");
    let dummies = app
        .world_mut()
        .query_filtered::<Entity, With<arena_sim::preview::PreviewDummy>>()
        .iter(app.world())
        .count();
    assert_eq!(dummies, 2, "retargeting skills stage a second dummy");
}

#[test]
fn preview_is_deterministic() {
    let total = |s: u64| {
        run(s, 90)
            .world()
            .resource::<EventRecorder>()
            .damage_resolved
            .iter()
            .map(|d| d.total_damage)
            .sum::<f64>()
    };
    let (a, b) = (total(0xABCDEF), total(0xABCDEF));
    assert!(a > 0.0);
    assert_eq!(a, b);
}
