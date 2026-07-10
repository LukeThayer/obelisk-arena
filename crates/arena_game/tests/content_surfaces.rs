//! Content well-formedness for the surfaces arena increment (spec §6-§8): the surface-type
//! TOMLs load + validate against the real skills registry, and the painting/gating content
//! carries the authored fields the sim consumes.
use bevy::prelude::App;
use obelisk_bevy::assets::{Acquisition, CastTimeline, PaintMode};
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillRegistry, SkillSource};
use obelisk_bevy::surfaces::load_surfaces_dir;

fn read(rel: &str) -> String {
    std::fs::read_to_string(arena_game::arena_root().join(rel)).expect(rel)
}

fn timeline(id: &str) -> CastTimeline {
    let tl: CastTimeline = ron::from_str(&read(&format!("assets/skills/{id}.cast.ron")))
        .unwrap_or_else(|e| panic!("{id}.cast.ron parses: {e}"));
    assert_eq!(tl.skill_id, id);
    tl
}

#[test]
fn surface_types_load_and_validate_against_the_real_registries() {
    let mut app = App::new();
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&arena_game::arena_root().join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(
        arena_game::arena_root().join("config/skills"),
    ));
    let reg = app.world().resource::<SkillRegistry>();
    let map = load_surfaces_dir(&arena_game::arena_root().join("config/surfaces"), Some(reg))
        .expect("config/surfaces loads + validates");
    // frost: pure spire fuel, tile-parity numbers (verbs.rs consts it replaces).
    let frost = &map["frost"];
    assert_eq!(frost.lifetime, 180.0);
    assert_eq!(frost.patch_radius, 0.45);
    assert_eq!(frost.max_patches, 64);
    assert!(frost.standing.is_none(), "frost is fuel, no standing payload (v1)");
    // burning: standing tick via the triggered-only skill.
    let burning = &map["burning"];
    let standing = burning.standing.as_ref().expect("burning has standing");
    assert_eq!(standing.tick_skill.as_deref(), Some("burning_ground_tick"));
    assert!(reg.0.contains_key("burning_ground_tick"), "tick skill registered");
    // visuals present for the client renderer.
    assert!(frost.visuals.as_ref().is_some_and(|v| v.decal.is_some()));
    assert!(burning.visuals.as_ref().is_some_and(|v| v.decal.is_some()));
}

#[test]
fn firebolt_explosion_paints_burning_on_end() {
    let tl = timeline("firebolt_explosion");
    let paints = tl.collision_windows[0]
        .paints
        .as_ref()
        .expect("blast paints");
    assert_eq!(paints.surface, "burning");
    assert!(matches!(paints.mode, PaintMode::OnEnd));
    assert!(paints.lifetime.is_some(), "short scorch override, not burning's default");
}

#[test]
fn burning_ground_tick_is_a_triggered_only_castpoint_blast() {
    let tl = timeline("burning_ground_tick");
    assert!(matches!(tl.acquisition, Acquisition::SelfPoint));
    assert!(matches!(
        tl.collision_windows[0].anchor,
        obelisk_bevy::assets::WindowAnchor::CastPoint
    ));
}
