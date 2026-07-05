//! Content well-formedness for the chain_lightning v2 triad. Chaining is now RULES-driven (a
//! single Beam window + rules `can_chain`/`chain_count` + the timeline's `chain_radius`) — the
//! actual chain-hop FIRING mechanism is covered by obelisk-bevy's own chain tests
//! (`tests/beam_retarget.rs`); here we prove the arena content parses as reformed v2 (no leftover
//! v1 `hop` window / `Retarget` / `Chained`) and the rules carry the authored chain config.
use bevy::prelude::App;
use obelisk_bevy::assets::CastTimeline;
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillRegistry, SkillSource};

fn read(rel: &str) -> String {
    std::fs::read_to_string(arena_game::arena_root().join(rel)).expect(rel)
}

#[test]
fn chain_lightning_timeline_parses_as_v2_single_beam_window() {
    // deny_unknown_fields makes any v1 leftover (hop window/on_end/Retarget/Chained/targeting/
    // delivery) fail here.
    let cl: CastTimeline = ron::from_str(&read("assets/skills/chain_lightning.cast.ron"))
        .expect("chain_lightning.cast.ron parses as v2 CastTimeline");
    assert_eq!(cl.skill_id, "chain_lightning");
    assert_eq!(
        cl.collision_windows.len(),
        1,
        "no authored hop window in v2 — chaining is rules-driven"
    );
    assert!(matches!(
        cl.collision_windows[0].motion,
        obelisk_bevy::assets::VolumeMotion::Beam
    ));
    assert!(matches!(
        cl.acquisition,
        obelisk_bevy::assets::Acquisition::HitscanEntity { .. }
    ));
    assert!(cl.chain_radius > 0.0, "chain_radius must be authored (top-level timeline field)");
    // vfx_cues must be populated (slot==value) or the cues never fire.
    assert!(cl.vfx_cues.contains_key("on_window_arc") && cl.cues.contains_key("on_window_arc"));
    assert!(cl.vfx_cues.contains_key("on_hit") && cl.cues.contains_key("on_hit"));
}

#[test]
fn chain_lightning_rules_carry_can_chain_and_chain_count() {
    let mut app = App::new();
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&arena_game::arena_root().join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(arena_game::arena_root().join("config/skills")));
    let reg = app.world().resource::<SkillRegistry>();
    let cl = reg.0.get("chain_lightning").expect("chain_lightning registered");
    assert!(cl.damage.can_chain, "rules must carry can_chain = true");
    assert_eq!(cl.damage.chain_count, 3, "rules must carry chain_count = 3");
}
