//! Task 31: the preview cosmetics observer. On a `CueEvent`, `on_preview_cue` looks up the
//! `EditedSkillFx` lane bound to the fired `cue_id` and (a) drives the caster's anim clip and
//! (b) spawns a `bevy_vfx` effect at the resolved rig socket with baked params. Verified
//! headlessly (no render): a particle lane bound to `"firebolt_cast"` fires exactly one
//! `PreviewCosmetic` child of the caster (the socket falls back to the rig root when unnamed).

use arena_editor::model::EditedSkillFx;
use arena_editor::preview_cosmetics::{on_preview_cue, PreviewCharge, PreviewCosmetic};
use arena_editor::preview_rig::PreviewAnimGraph;
use arena_editor::socket::RigSockets;
use arena_sim::preview::PreviewCaster;
use arena_skills::{AnimLayer, CueKind, LaneEvent, ParticleSpec, ProjectileCosmetic, SkillFx};
use bevy::prelude::*;
use bevy_vfx::data::VfxLibrary;
use obelisk_bevy::events::{CueEvent, CueKind as ObeliskCueKind};
use std::collections::HashMap;
use std::path::PathBuf;

/// A firebolt cast lane with BOTH an anim layer (drives a clip) and a particle burst (spawns the
/// cosmetic). The anim half no-ops headlessly (no `AnimationPlayer`/graph nodes); the particle
/// half is what spawns the single `PreviewCosmetic`.
fn firebolt_cast_fx() -> SkillFx {
    let lane = LaneEvent {
        lane_id: "firebolt_muzzle".into(),
        kind: CueKind::OnCast,
        particle: Some(ParticleSpec {
            count: 8,
            lifetime: 0.5,
            color: [1.0, 0.5, 0.1],
            speed: 3.0,
            effect: None,
            socket: None,
            offset: Vec3::new(0.0, 0.0, 0.5),
            param_bindings: Vec::new(),
        }),
        projectile: None,
        anim: Some(AnimLayer {
            state: "cast_release".into(),
            clip: Some("casting_idle".into()),
            layer: 0,
            weight: 1.0,
        }),
    };
    let impact = LaneEvent {
        lane_id: "firebolt_impact".into(),
        kind: CueKind::OnHit,
        particle: Some(ParticleSpec {
            count: 20,
            lifetime: 0.5,
            color: [1.0, 0.3, 0.05],
            speed: 5.0,
            effect: None,
            socket: None,
            offset: Vec3::new(0.0, 0.25, 0.0),
            param_bindings: Vec::new(),
        }),
        projectile: None,
        anim: None,
    };
    let mut lanes = HashMap::new();
    lanes.insert("firebolt_cast".to_string(), lane);
    lanes.insert("firebolt_impact".to_string(), impact);
    SkillFx {
        skill_id: "firebolt".into(),
        lanes,
    }
}

fn setup_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<RigSockets>()
        .init_resource::<PreviewAnimGraph>()
        .init_resource::<VfxLibrary>()
        .init_resource::<PreviewCharge>()
        .insert_resource(EditedSkillFx::from_fx(
            firebolt_cast_fx(),
            PathBuf::from("firebolt.skillfx.ron"),
        ))
        .add_observer(on_preview_cue);
    app
}

#[test]
fn cue_spawns_one_preview_cosmetic_child_for_the_bound_lane() {
    let mut app = setup_app();
    let caster = app.world_mut().spawn(PreviewCaster).id();
    app.world_mut().trigger(CueEvent {
        cue_id: "firebolt_cast".into(),
        source: caster,
        position: Vec3::new(0.0, 1.0, 0.0),
        kind: ObeliskCueKind::OnCast,
    });
    app.world_mut().flush();

    let cosmetics: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .collect();
    assert_eq!(
        cosmetics.len(),
        1,
        "exactly one PreviewCosmetic should spawn for the particle lane"
    );
    // The socket was unnamed, so it falls back to the rig root (the caster).
    let parent = app
        .world()
        .get::<ChildOf>(cosmetics[0])
        .expect("cosmetic parented to its socket");
    assert_eq!(parent.0, caster);
}

/// The cast lane's cosmetic projectile spawns world-space at the cue position and carries a
/// `PreviewFlight` with the AUTHORED speed and gravity along the duel axis — the visible bolt
/// traces the same ballistic arc the sim's hitbox flies.
#[test]
fn cast_cue_projectile_lane_flies_the_authored_arc() {
    use arena_editor::preview_cosmetics::PreviewFlight;
    let mut app = setup_app();
    app.world_mut()
        .resource_mut::<EditedSkillFx>()
        .fx
        .lanes
        .get_mut("firebolt_cast")
        .unwrap()
        .projectile = Some(ProjectileCosmetic {
        speed: 20.0,
        gravity: 9.8,
        color: [1.0, 0.4, 0.05],
        radius: 0.2,
        effect: None,
        socket: None,
    });
    let caster = app.world_mut().spawn(PreviewCaster).id();
    let cast_pos = Vec3::new(-4.0, 0.59, 0.0);
    app.world_mut().trigger(CueEvent {
        cue_id: "firebolt_cast".into(),
        source: caster,
        position: cast_pos,
        kind: ObeliskCueKind::OnCast,
    });
    app.world_mut().flush();

    let mut flights = app.world_mut().query::<(&PreviewFlight, &Transform)>();
    let (flight, tf) = flights.single(app.world()).expect("one flying bolt");
    assert_eq!(tf.translation, cast_pos, "launches from the cue position");
    assert_eq!(flight.gravity, 9.8);
    // The launch is LOFTED (the same ballistic solve the preview cast uses) so the arc lands on
    // the dummy marker instead of grounding short of it.
    let expected = arena_sim::ballistics::ballistic_launch_dir(
        cast_pos,
        arena_sim::spawn::SPAWN_MARKERS[1],
        20.0,
        9.8,
    ) * 20.0;
    assert!(
        (flight.velocity - expected).length() < 1e-4,
        "lofted launch at authored speed: {:?}",
        flight.velocity
    );
    assert!(flight.velocity.y > 0.5, "visibly pitched up");
}

/// An `OnHit` cue carries the authoritative hit position — the impact cosmetic must spawn as a
/// world-space root at `cue.position + lane offset`, NOT parented to a caster socket (which would
/// render the explosion on the caster).
#[test]
fn on_hit_cue_spawns_the_impact_at_the_cue_world_position_unparented() {
    let mut app = setup_app();
    let caster = app.world_mut().spawn(PreviewCaster).id();
    let hit_pos = Vec3::new(4.0, 0.8, 0.0);
    app.world_mut().trigger(CueEvent {
        cue_id: "firebolt_impact".into(),
        source: caster,
        position: hit_pos,
        kind: ObeliskCueKind::OnHit,
    });
    app.world_mut().flush();

    let cosmetics: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .collect();
    assert_eq!(cosmetics.len(), 1);
    assert!(
        app.world().get::<ChildOf>(cosmetics[0]).is_none(),
        "impact cosmetic must be a world-space root, not socket-parented"
    );
    let tf = app
        .world()
        .get::<Transform>(cosmetics[0])
        .expect("impact cosmetic has a Transform");
    assert_eq!(tf.translation, hit_pos + Vec3::new(0.0, 0.25, 0.0));
}

#[test]
fn cue_with_no_bound_lane_spawns_nothing() {
    let mut app = setup_app();
    let caster = app.world_mut().spawn(PreviewCaster).id();
    app.world_mut().trigger(CueEvent {
        cue_id: "unbound_cue".into(),
        source: caster,
        position: Vec3::ZERO,
        kind: ObeliskCueKind::OnCast,
    });
    app.world_mut().flush();

    let n = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .count();
    assert_eq!(n, 0, "an unbound cue should spawn no cosmetics");
}
