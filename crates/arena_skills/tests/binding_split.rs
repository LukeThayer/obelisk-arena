//! Binding-split test: prove the consumer half (`resolve_cue`) resolves the lanes for a fired cue
//! from the registry, and no-ops (empty, no panic) for an unbound cue.

use arena_skills::{resolve_cue, CueKind, CueMessage, SkillFxRegistry};
use bevy::math::Vec3;
use std::path::Path;

#[test]
fn consumer_resolves_lanes_for_a_cue() {
    let reg = SkillFxRegistry::load_dir(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/skills"
    )));
    let m = CueMessage {
        cue_id: "firebolt_cast".into(),
        source_id: "player".into(),
        position: Vec3::ZERO,
        aim_dir: Vec3::ZERO,
        kind: CueKind::OnCast,
    };
    let lanes = resolve_cue(&reg, &m);
    assert!(
        !lanes.is_empty(),
        "firebolt_cast should resolve at least one lane"
    );
    // missing cue → empty, no panic
    let miss = CueMessage {
        cue_id: "nope".into(),
        source_id: "x".into(),
        position: Vec3::ZERO,
        aim_dir: Vec3::ZERO,
        kind: CueKind::OnHit,
    };
    assert!(resolve_cue(&reg, &miss).is_empty());
}
