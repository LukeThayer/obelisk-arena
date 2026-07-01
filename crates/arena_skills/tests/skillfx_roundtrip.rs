use arena_skills::{
    AnimLayer, LaneEvent, ParticleSpec, ProjectileCosmetic, SkillFx, SkillFxRegistry,
    VfxBindSource, VfxParamBinding,
};
use bevy::math::Vec3;
use std::collections::HashMap;
fn extended_lane() -> LaneEvent {
    LaneEvent {
        lane_id: "x_muzzle".into(),
        kind: arena_skills::CueKind::OnCast,
        particle: Some(ParticleSpec {
            count: 12,
            lifetime: 0.4,
            color: [1.0, 0.5, 0.1],
            speed: 4.0,
            effect: Some("fire_burst".into()),
            socket: Some("wand_tip".into()),
            offset: Vec3::new(0.0, 0.1, 0.2),
            param_bindings: vec![VfxParamBinding {
                param: "scale".into(),
                source: VfxBindSource::Charge,
                min: 0.2,
                max: 1.0,
            }],
        }),
        projectile: Some(ProjectileCosmetic {
            speed: 20.0,
            color: [1.0, 0.4, 0.05],
            radius: 0.2,
            effect: Some("fire_trail".into()),
            socket: Some("wand_tip".into()),
        }),
        anim: Some(AnimLayer {
            state: String::new(),
            clip: Some("casting_idle".into()),
            layer: 1,
            weight: 0.8,
        }),
    }
}
#[test]
fn extended_lane_round_trips_through_ron() {
    let fx = SkillFx {
        skill_id: "x".into(),
        lanes: HashMap::from([("x_cast".to_string(), extended_lane())]),
    };
    let s = ron::ser::to_string(&fx).expect("ser");
    let back: SkillFx = ron::de::from_str(&s).expect("de");
    let l = back.lanes.get("x_cast").unwrap();
    let p = l.particle.as_ref().unwrap();
    assert_eq!(p.effect.as_deref(), Some("fire_burst"));
    assert_eq!(p.socket.as_deref(), Some("wand_tip"));
    assert_eq!(p.offset, Vec3::new(0.0, 0.1, 0.2));
    assert_eq!(p.param_bindings.len(), 1);
    assert_eq!(p.param_bindings[0].source, VfxBindSource::Charge);
    assert_eq!(
        l.projectile.as_ref().unwrap().effect.as_deref(),
        Some("fire_trail")
    );
    let a = l.anim.as_ref().unwrap();
    assert_eq!(a.clip.as_deref(), Some("casting_idle"));
    assert_eq!(a.layer, 1);
    assert!((a.weight - 0.8).abs() < 1e-6);
}
#[test]
fn existing_firebolt_asset_still_parses_with_defaults() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf();
    let s = std::fs::read_to_string(root.join("assets/skills/firebolt.skillfx.ron")).expect("read");
    let fx: SkillFx = ron::de::from_str(&s).expect("legacy parses");
    let p = fx
        .lanes
        .get("firebolt_cast")
        .unwrap()
        .particle
        .as_ref()
        .unwrap();
    assert_eq!(p.count, 12);
    assert!(p.effect.is_none());
    assert!(p.param_bindings.is_empty());
    assert_eq!(p.offset, Vec3::ZERO);
}
#[test]
fn registry_resolves_lanes_with_new_fields() {
    let dir = std::env::temp_dir().join("arena_m3_1_skillfx");
    std::fs::create_dir_all(&dir).unwrap();
    let fx = SkillFx {
        skill_id: "x".into(),
        lanes: HashMap::from([("x_cast".to_string(), extended_lane())]),
    };
    std::fs::write(dir.join("x.skillfx.ron"), ron::ser::to_string(&fx).unwrap()).unwrap();
    let reg = SkillFxRegistry::load_dir(&dir);
    let lanes = reg.lanes("x_cast").expect("bound");
    assert_eq!(lanes.len(), 1);
    assert_eq!(
        lanes[0].particle.as_ref().unwrap().effect.as_deref(),
        Some("fire_burst")
    );
}
