//! Timeline scrubbing end-to-end (headless): dragging the scrub head across a cue moment fires the
//! bound lane through the SAME `on_preview_cue` observer the live sim drives — with NO duel
//! entities in the world (edit mode). The impact lane spawns a world-space cosmetic staged at the
//! dummy marker, and `age_preview_cosmetics` despawns it once its `CosmeticLifetime` expires.

use arena_editor::model::{EditedSkill, EditedSkillFx};
use arena_editor::preview_controller::Playhead;
use arena_editor::preview_cosmetics::{
    age_preview_cosmetics, on_preview_cue, PreviewCharge, PreviewCosmetic,
};
use arena_editor::preview_rig::PreviewAnimGraph;
use arena_editor::scrub::{fire_scrub_cues, ScrubState};
use arena_editor::socket::RigSockets;
use arena_sim::spawn::SPAWN_MARKERS;
use arena_skills::{CueKind, LaneEvent, ParticleSpec, SkillFx};
use bevy::prelude::*;
use bevy_vfx::data::VfxLibrary;
use obelisk_bevy::assets::{
    CollisionShape, CollisionWindow, HitFilter, HitMode, VolumeMotion, WindowPhase,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

fn impact_only_fx() -> SkillFx {
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
            offset: Vec3::ZERO,
            param_bindings: Vec::new(),
        }),
        projectile: None,
        anim: None,
    };
    SkillFx {
        skill_id: "firebolt".into(),
        lanes: HashMap::from([("firebolt_impact".to_string(), impact)]),
    }
}

fn firebolt_like_timeline() -> obelisk_bevy::assets::CastTimeline {
    let mut tl = arena_editor::blank_cast_timeline("firebolt");
    tl.collision_windows.push(CollisionWindow {
        id: "bolt".into(),
        spawn_phase: WindowPhase::Active,
        spawn_offset: 0.0,
        active_duration: 2.0,
        shape: CollisionShape::Sphere { radius: 0.5 },
        motion: VolumeMotion::Linear { speed: 20.0 },
        hit_filter: HitFilter::Enemies,
        hit_mode: HitMode::FirstOnly,
        rehit_interval: None,
    });
    tl
}

#[test]
fn scrubbing_past_the_hit_moment_spawns_and_then_ages_out_the_impact() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f64(0.3),
        ))
        .init_resource::<RigSockets>()
        .init_resource::<PreviewAnimGraph>()
        .init_resource::<VfxLibrary>()
        .init_resource::<PreviewCharge>()
        .init_resource::<Playhead>()
        .init_resource::<ScrubState>()
        .insert_resource(EditedSkill::from_timeline(
            firebolt_like_timeline(),
            PathBuf::from("firebolt.cast.ron"),
        ))
        .insert_resource(EditedSkillFx::from_fx(
            impact_only_fx(),
            PathBuf::from("firebolt.skillfx.ron"),
        ))
        .add_observer(on_preview_cue)
        .add_systems(Update, (fire_scrub_cues, age_preview_cosmetics));

    // Drag from t=0 forward past the hit moment (window close at 0.3 + 2.0 = 2.3).
    {
        let mut scrub = app.world_mut().resource_mut::<ScrubState>();
        scrub.fired_up_to = Some(0.0);
        scrub.time = Some(2.5);
    }
    app.update();

    let cosmetics: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .collect();
    assert_eq!(cosmetics.len(), 1, "the impact lane fires once on crossing");
    let tf = app.world().get::<Transform>(cosmetics[0]).unwrap();
    assert_eq!(
        tf.translation, SPAWN_MARKERS[1],
        "impact staged at the dummy marker"
    );
    assert!(
        app.world().get::<ChildOf>(cosmetics[0]).is_none(),
        "no caster exists — the cosmetic must be a world-space root"
    );

    // Holding still re-fires nothing.
    app.update();
    let n = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .count();
    assert_eq!(n, 1, "a stationary scrub head must not re-fire the cue");

    // The 0.5 s CosmeticLifetime expires after two more 0.3 s ticks, then the entity survives
    // two grace frames (effect stopped, entity alive for bevy_vfx's queued cleanup) before the
    // despawn lands.
    for _ in 0..5 {
        app.update();
    }
    let n = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .count();
    assert_eq!(n, 0, "the impact cosmetic ages out");
}
