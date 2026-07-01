//! `.cast.ron` save/load round-trip for the skill designer. Pure filesystem I/O over the now-
//! `Serialize` `CastTimeline` (obelisk-bevy Task 1) — no app, no egui. Proves an authored timeline
//! survives a save→reload cycle field-for-field (the no-`PartialEq` f32 enums compared via
//! `{:?}`) and that the real firebolt asset parses through the same path.

use arena_editor::io::{load_cast_timeline, save_cast_timeline};
use arena_editor::model::blank_cast_timeline;
use obelisk_bevy::assets::{
    CastDelivery, CastTargeting, CollisionShape, CollisionWindow, HitFilter, HitMode, VolumeMotion,
    WindowPhase,
};

#[test]
fn author_save_reload_round_trips() {
    let mut tl = blank_cast_timeline("zap");
    tl.collision_windows.push(CollisionWindow {
        id: "burst".into(),
        spawn_phase: WindowPhase::Active,
        spawn_offset: 0.0,
        active_duration: 0.2,
        shape: CollisionShape::Cone {
            angle: 90.0,
            range: 5.0,
        },
        motion: VolumeMotion::Linear { speed: 8.0 },
        hit_filter: HitFilter::Enemies,
        hit_mode: HitMode::OncePerTarget,
        rehit_interval: None,
    });
    tl.targeting = CastTargeting::Cone {
        angle: 90.0,
        range: 5.0,
    };
    tl.delivery = CastDelivery::Projectile { speed: 12.0 };
    tl.vfx_cues.insert("on_cast".into(), "zap_cast".into());
    let path = std::env::temp_dir().join("arena_editor_rt_zap.cast.ron");
    save_cast_timeline(&tl, &path).expect("save");
    let back = load_cast_timeline(&path).expect("reload");
    assert_eq!(tl.skill_id, back.skill_id);
    assert_eq!(
        format!("{:?}", tl.phase_durations),
        format!("{:?}", back.phase_durations)
    );
    assert_eq!(
        format!("{:?}", tl.collision_windows[0]),
        format!("{:?}", back.collision_windows[0])
    );
    assert_eq!(
        format!("{:?}", tl.targeting),
        format!("{:?}", back.targeting)
    );
    assert_eq!(format!("{:?}", tl.delivery), format!("{:?}", back.delivery));
    assert_eq!(tl.vfx_cues, back.vfx_cues);
}

#[test]
fn loads_the_real_firebolt_asset() {
    let path = arena_editor::io::editor_root().join("assets/skills/firebolt.cast.ron");
    let tl = load_cast_timeline(&path).expect("parses");
    assert_eq!(tl.skill_id, "firebolt");
    assert_eq!(tl.collision_windows.len(), 1);
}
