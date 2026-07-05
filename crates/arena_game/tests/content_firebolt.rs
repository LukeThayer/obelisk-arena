//! Content well-formedness for the firebolt v2 triad. The trigger's end-to-end FIRING is covered
//! by the net-test (tools/net-test) + obelisk-bevy's own fireball golden traces; here we prove the
//! arena content parses as reformed v2 and the rules carry the authored trigger.
use bevy::prelude::App;
use obelisk_bevy::assets::CastTimeline;
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillRegistry, SkillSource};

fn read(rel: &str) -> String {
    std::fs::read_to_string(arena_game::arena_root().join(rel)).expect(rel)
}

#[test]
fn firebolt_timelines_parse_as_v2() {
    // deny_unknown_fields makes any v1 leftover (spawn_phase/on_end/targeting/delivery) fail here.
    let fb: CastTimeline = ron::from_str(&read("assets/skills/firebolt.cast.ron"))
        .expect("firebolt.cast.ron parses as v2 CastTimeline");
    assert_eq!(fb.skill_id, "firebolt");
    assert!(matches!(fb.acquisition, obelisk_bevy::assets::Acquisition::Aim));
    assert_eq!(fb.collision_windows.len(), 1, "no inline blast window in v2");
    assert!(matches!(
        fb.collision_windows[0].motion,
        obelisk_bevy::assets::VolumeMotion::Ballistic { .. }
    ));
    // vfx_cues must be populated (slot==value) or the cues never fire.
    assert!(fb.vfx_cues.contains_key("on_cast") && fb.cues.contains_key("on_cast"));

    let expl: CastTimeline = ron::from_str(&read("assets/skills/firebolt_explosion.cast.ron"))
        .expect("firebolt_explosion.cast.ron parses");
    assert_eq!(expl.skill_id, "firebolt_explosion");
    assert!(
        matches!(
            expl.collision_windows[0].anchor,
            obelisk_bevy::assets::WindowAnchor::CastPoint
        ),
        "blast anchored at the trigger position"
    );
}

#[test]
fn firebolt_rules_carry_the_explosion_trigger() {
    let mut app = App::new();
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&arena_game::arena_root().join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(arena_game::arena_root().join("config/skills")));
    let reg = app.world().resource::<SkillRegistry>();
    let fb = reg.0.get("firebolt").expect("firebolt registered");
    let trig: Vec<_> = fb
        .conditions
        .iter()
        .filter(|c| c.trigger_skill == "firebolt_explosion")
        .collect();
    assert_eq!(trig.len(), 3, "always + on_impact + on_expire");
    assert!(
        trig.iter().all(|c| c.additional),
        "timeline-target conditions must be additional"
    );
    assert!(
        reg.0.contains_key("firebolt_explosion"),
        "the triggered skill is registered"
    );
}
