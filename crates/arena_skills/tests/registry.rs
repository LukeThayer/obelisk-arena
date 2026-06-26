//! `SkillFxRegistry` load test: prove the registry flattens every `.skillfx.ron` in a dir into a
//! `cue_id -> [LaneEvent]` map and resolves firebolt's cues.

use arena_skills::SkillFxRegistry;
use std::path::Path;

#[test]
fn registry_loads_firebolt_cues() {
    let reg = SkillFxRegistry::load_dir(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/skills"
    )));
    assert!(reg.lanes("firebolt_cast").map_or(false, |l| !l.is_empty()));
    assert!(reg
        .lanes("firebolt_impact")
        .map_or(false, |l| !l.is_empty()));
    assert!(reg.lanes("nonexistent").is_none());
}
