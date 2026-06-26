//! Round-trip test for the `.skillfx.ron` authoring format.
//!
//! Reads the real `assets/skills/firebolt.skillfx.ron` fixture and deserializes it with the same
//! `ron` path the `SkillFxLoader` uses (`ron::de::from_str::<SkillFx>`), proving the fixture matches
//! the type set in `arena_skills::SkillFx`. This is the data-only half of the loader contract; the
//! AssetServer-driven load is exercised once the binding layer lands (next task).

use arena_skills::{CueKind, SkillFx};

/// Resolve the fixture relative to the crate dir (the test's compile-time `CARGO_MANIFEST_DIR` is
/// `crates/arena_skills`). The fixture lives at the workspace-root `assets/`, two levels up.
fn fixture_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/skills/firebolt.skillfx.ron")
}

#[test]
fn firebolt_skillfx_ron_round_trips() {
    let path = fixture_path();
    assert!(
        path.exists(),
        "expected the firebolt skillfx fixture at {}",
        path.display()
    );
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    let fx: SkillFx = ron::de::from_str(&text)
        .unwrap_or_else(|e| panic!("deserialize {}: {e}", path.display()));

    assert_eq!(fx.skill_id, "firebolt");
    assert!(
        fx.lanes.contains_key("firebolt_cast"),
        "missing the firebolt_cast lane (keys: {:?})",
        fx.lanes.keys().collect::<Vec<_>>()
    );
    assert!(
        fx.lanes.contains_key("firebolt_impact"),
        "missing the firebolt_impact lane (keys: {:?})",
        fx.lanes.keys().collect::<Vec<_>>()
    );

    // The OnCast lane carries the cosmetic projectile; the OnHit impact lane does not.
    let cast = &fx.lanes["firebolt_cast"];
    assert_eq!(cast.lane_id, "firebolt_muzzle");
    assert_eq!(cast.kind, CueKind::OnCast);
    assert!(
        cast.projectile.is_some(),
        "firebolt_cast lane should declare a cosmetic projectile"
    );
    assert!(cast.particle.is_some(), "firebolt_cast lane has a muzzle particle");
    assert!(cast.anim.is_some(), "firebolt_cast lane has a cast_release anim");

    let impact = &fx.lanes["firebolt_impact"];
    assert_eq!(impact.kind, CueKind::OnHit);
    assert!(
        impact.projectile.is_none(),
        "firebolt_impact lane should NOT spawn a projectile"
    );
    assert!(impact.particle.is_some(), "firebolt_impact lane has an impact particle");
}
