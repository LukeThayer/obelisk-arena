//! Task 26: `EditedSkillFx` + `blank_skillfx` + `save_skillfx`/`load_skillfx` round-trip; the
//! canonical firebolt `.skillfx.ron` loads its lane; the designer's `EditedSkillFx` resource is
//! initialized by `SkillDesignerPlugin`.

use arena_editor::io::{default_skillfx_path, load_skillfx, save_skillfx};
use arena_editor::model::{blank_skillfx, EditedSkillFx};
use arena_skills::{AnimLayer, CueKind, LaneEvent, ParticleSpec, SkillFx};
use bevy::math::Vec3;
use std::collections::HashMap;

fn zap_fx() -> SkillFx {
    let mut lanes = HashMap::new();
    lanes.insert(
        "zap_cast".to_string(),
        LaneEvent {
            lane_id: "zap_muzzle".into(),
            kind: CueKind::OnCast,
            particle: Some(ParticleSpec {
                count: 12,
                lifetime: 0.4,
                color: [0.2, 0.4, 1.0],
                speed: 4.0,
                effect: Some("spark".into()),
                socket: Some("wand_tip".into()),
                offset: Vec3::Y * 0.1,
                param_bindings: Vec::new(),
            }),
            projectile: None,
            anim: Some(AnimLayer {
                state: String::new(),
                clip: Some("casting_idle".into()),
                layer: 0,
                weight: 1.0,
            }),
        },
    );
    SkillFx {
        skill_id: "zap".into(),
        lanes,
    }
}

#[test]
fn save_then_load_round_trips_a_skillfx() {
    let dir = std::env::temp_dir().join("arena_editor_skillfx_io_test");
    let path = dir.join("zap.skillfx.ron");
    let _ = std::fs::remove_file(&path);
    let fx = zap_fx();
    save_skillfx(&fx, &path).expect("save");
    let loaded = load_skillfx(&path).expect("load");
    assert_eq!(loaded.skill_id, "zap");
    let lane = loaded.lanes.get("zap_cast").expect("zap_cast lane");
    assert_eq!(lane.lane_id, "zap_muzzle");
    let particle = lane.particle.as_ref().expect("particle");
    assert_eq!(particle.effect.as_deref(), Some("spark"));
    assert_eq!(particle.socket.as_deref(), Some("wand_tip"));
    assert_eq!(particle.offset, Vec3::Y * 0.1);
    let anim = lane.anim.as_ref().expect("anim");
    assert_eq!(anim.clip.as_deref(), Some("casting_idle"));
}

#[test]
fn load_skillfx_reads_the_canonical_firebolt_file() {
    let fx = load_skillfx(&default_skillfx_path("firebolt")).expect("load firebolt.skillfx.ron");
    assert_eq!(fx.skill_id, "firebolt");
    assert!(fx.lanes.contains_key("firebolt_cast"));
}

#[test]
fn blank_skillfx_is_an_empty_lane_map() {
    let fx = blank_skillfx("newskill");
    assert_eq!(fx.skill_id, "newskill");
    assert!(fx.lanes.is_empty());
}

#[test]
fn skill_designer_plugin_inits_the_edited_skillfx_resource() {
    let mut app = arena_editor::build_editor_app();
    app.add_plugins(arena_editor::SkillDesignerPlugin);
    assert!(app.world().contains_resource::<EditedSkillFx>());
    let fx = app.world().resource::<EditedSkillFx>();
    assert_eq!(fx.fx.skill_id, "firebolt");
}
