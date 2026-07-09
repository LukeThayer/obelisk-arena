//! Content well-formedness for the wisp weapon ports (potted_spring's glacier chain +
//! needle_and_thread's portal pair). The server-side VERBS these skills drive (skill objects,
//! tile trail, spire eruption, teleport) live in `server/{skill_objects,verbs,portals}.rs` with
//! their own unit tests; here we prove the CONTENT parses as v2, carries the authored trigger
//! chain, and keeps the cue slots the arena verb layer matches on.
use bevy::prelude::App;
use obelisk_bevy::assets::{Acquisition, CastTimeline, MotionDirection, VolumeMotion, WindowAnchor};
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillRegistry, SkillSource};

fn read(rel: &str) -> String {
    std::fs::read_to_string(arena_game::arena_root().join(rel)).expect(rel)
}

fn timeline(id: &str) -> CastTimeline {
    let tl: CastTimeline = ron::from_str(&read(&format!("assets/skills/{id}.cast.ron")))
        .unwrap_or_else(|e| panic!("{id}.cast.ron parses as v2 CastTimeline: {e}"));
    assert_eq!(tl.skill_id, id);
    tl
}

#[test]
fn glacier_timelines_parse_and_chain_hugs_the_ground() {
    let ball = timeline("rolling_glacier");
    assert!(matches!(ball.acquisition, Acquisition::Aim));
    assert!(matches!(
        ball.collision_windows[0].motion,
        VolumeMotion::Ballistic { .. }
    ));
    assert!(ball.chargeable, "a held lob flies flatter/further");

    // The roll executes at the ball's ground-impact point and must FLATTEN the inherited
    // (descending) direction — Horizontal is the whole reason the variant exists.
    let roll = timeline("glacier_roll");
    let w = &roll.collision_windows[0];
    assert!(matches!(w.anchor, WindowAnchor::CastPoint));
    assert!(matches!(w.motion, VolumeMotion::Linear { .. }));
    assert!(matches!(w.motion_direction, MotionDirection::Horizontal));
    assert!(
        w.emitter.is_none(),
        "the tile trail is the ARENA poller (drop_glacier_trail), not an emitter — Template \
         windows would share the skill's lifecycle triggers and re-fire the burst per tile"
    );

    let burst = timeline("glacier_burst");
    assert!(matches!(
        burst.collision_windows[0].anchor,
        WindowAnchor::CastPoint
    ));
    assert!(matches!(
        burst.collision_windows[0].motion,
        VolumeMotion::Static
    ));
}

#[test]
fn spire_and_portals_keep_the_verb_cue_slots() {
    // The arena verb layer (server/verbs.rs::skill_verbs_on_cue) matches on EXACTLY these
    // (skill_id, cue slot) pairs; renaming a window renames its slot and silently kills the verb.
    let spire = timeline("frost_spire");
    assert!(matches!(spire.acquisition, Acquisition::GroundPoint { .. }));
    assert!(spire.vfx_cues.contains_key("on_window_spike"));

    for id in ["portal_orange", "portal_blue"] {
        let p = timeline(id);
        // Plain Aim: the server VERB does the wisp placement raycast itself (surface stick or
        // air float) — no acquisition point involved.
        assert!(matches!(p.acquisition, Acquisition::Aim));
        assert!(p.vfx_cues.contains_key("on_window_portal_mark"));
        let w = &p.collision_windows[0];
        assert!(!w.strikes, "the mark is pure cue emission — no hits, no damage");
    }
}

#[test]
fn glacier_rules_carry_the_trigger_chain() {
    let mut app = App::new();
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&arena_game::arena_root().join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(arena_game::arena_root().join("config/skills")));
    let reg = app.world().resource::<SkillRegistry>();

    // ball: ground impact -> roll, mid-air fuse -> burst.
    let ball = reg.0.get("rolling_glacier").expect("registered");
    assert!(ball
        .conditions
        .iter()
        .any(|c| c.trigger_skill == "glacier_roll" && c.additional));
    assert!(ball
        .conditions
        .iter()
        .any(|c| c.trigger_skill == "glacier_burst" && c.additional));

    // roll: EVERY ending (wall / fuse) -> burst.
    let roll = reg.0.get("glacier_roll").expect("registered");
    let to_burst: Vec<_> = roll
        .conditions
        .iter()
        .filter(|c| c.trigger_skill == "glacier_burst")
        .collect();
    assert_eq!(to_burst.len(), 2, "on_impact + on_expire");
    assert!(to_burst.iter().all(|c| c.additional));

    for id in ["glacier_burst", "frost_spire", "portal_orange", "portal_blue"] {
        assert!(reg.0.contains_key(id), "{id} registered");
    }
}
