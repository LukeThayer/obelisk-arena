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
use arena_skills::{AnimLayer, CueKind, LaneEvent, ParticleSpec, SkillFx};
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
    let mut lanes = HashMap::new();
    lanes.insert("firebolt_cast".to_string(), lane);
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
