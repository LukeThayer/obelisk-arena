//! Content well-formedness for the wisp weapon ports (potted_spring's glacier chain +
//! needle_and_thread's portal pair). The server-side VERBS these skills drive (skill objects,
//! tile trail, spire eruption, teleport) live in `server/{skill_objects,verbs,portals}.rs` with
//! their own unit tests; here we prove the CONTENT parses as v2, carries the authored trigger
//! chain, and keeps the cue slots the arena verb layer matches on.
use bevy::prelude::App;
use obelisk_bevy::assets::{Acquisition, CastTimeline, VolumeMotion, WindowAnchor};
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
    // THE AUTHORITY FLIP (glacier-physics-ball): a real avian `RigidBody::Dynamic` boulder owns the
    // ball's trajectory from the CAST moment; obelisk keeps 100% of damage/trail/chain by PINNING
    // its collision windows to the ball. So BOTH windows are now `Static` (the pin is the sole mover,
    // NOT obelisk's own projectile motion) and the flight window's OPEN cue is a VERB slot that
    // spawns the one Dynamic ball — listed in `vfx_cues` (emitted), unbound in `cues` (the ball is
    // the visual; the cosmetic Follow lane is gone).
    let ball = timeline("rolling_glacier");
    assert!(matches!(ball.acquisition, Acquisition::Aim));
    assert!(
        matches!(ball.collision_windows[0].motion, VolumeMotion::Static),
        "the flight window is Static — the avian ball moves, the pin drags the hitbox along",
    );
    assert!(ball.chargeable, "a held lob throws the ball faster/further");
    assert!(
        ball.vfx_cues.contains_key("on_window_flight"),
        "the flight-window OPEN cue is the verb slot that spawns the Dynamic ball (server/verbs.rs)",
    );

    // The roll executes at the ball's ground-impact point. Its window is Static too — the avian ball
    // carries the roll's motion and the pin drags this hitbox along, so the frost Trail + contact
    // damage follow the ball's REAL rolling path (bank shots included).
    let roll = timeline("glacier_roll");
    let w = &roll.collision_windows[0];
    assert!(matches!(w.anchor, WindowAnchor::CastPoint));
    assert!(
        matches!(w.motion, VolumeMotion::Static),
        "the roll window is Static — pinned to the avian ball, not self-propelled",
    );
    // The tile trail is now the authored surfaces painter (spec §8) — the old ARENA poller
    // (drop_glacier_trail) is deleted; painting is a window PROPERTY, so it composes without
    // the Template-lifecycle trap that forced the poller.
    let paints = w.paints.as_ref().expect("roll paints the frost trail");
    assert_eq!(paints.surface, "frost");
    assert!(matches!(
        paints.mode,
        obelisk_bevy::assets::PaintMode::Trail { step } if step == 0.8
    ));

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
    let Acquisition::GroundPoint { on_surface, .. } = &spire.acquisition else {
        unreachable!()
    };
    let req = on_surface.as_ref().expect("spire gates on frost (spec §5.1)");
    assert_eq!(req.surface, "frost");
    assert!(req.snap && req.consume, "snap to the patch center; consume the fuel at accept");
    assert!(spire.vfx_cues.contains_key("on_window_spike"));

    // glacier_roll drives the ROLLING-BOULDER skill-object verbs (server/verbs.rs): the roll
    // window's OPEN cue (`on_window_roll`) spawns the kinematic ice ball in lockstep with the
    // roll; its END cue (`on_end_roll`) despawns the ball where the roll stops (wall / fuse).
    // Both slots must be listed in `vfx_cues` — that is what makes obelisk EMIT them (the verb
    // channel); renaming the "roll" window renames both slots and silently kills the boulder.
    let roll = timeline("glacier_roll");
    assert!(roll.vfx_cues.contains_key("on_window_roll"));
    assert!(roll.vfx_cues.contains_key("on_end_roll"));

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
