//! Content well-formedness for the blizzard v2 triad: `GroundPoint` acquisition + an `Emitter`
//! that rains `Template` shard windows. The emitter FIRING (shards spawning on `SpawnRng`, never
//! `CombatRng`) is proven by obelisk-bevy's own `tests/emitters.rs::blizzard_timeline()`; here we
//! prove the arena content parses as reformed v2 and the emitter is correctly wired — its
//! `window` names a real `Template` window (the one semantic invariant a parse test can cheaply
//! check, mirroring what `validate_timeline` enforces at load).
use bevy::prelude::App;
use obelisk_bevy::assets::CastTimeline;
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillRegistry, SkillSource};

fn read(rel: &str) -> String {
    std::fs::read_to_string(arena_game::arena_root().join(rel)).expect(rel)
}

#[test]
fn blizzard_timeline_parses_as_v2_ground_point_and_emitter() {
    // deny_unknown_fields makes any v1 leftover (targeting/delivery/on_end) fail here.
    let bz: CastTimeline = ron::from_str(&read("assets/skills/blizzard.cast.ron"))
        .expect("blizzard.cast.ron parses as v2 CastTimeline");
    assert_eq!(bz.skill_id, "blizzard");
    assert!(matches!(
        bz.acquisition,
        obelisk_bevy::assets::Acquisition::GroundPoint { .. }
    ));
    assert_eq!(
        bz.collision_windows.len(),
        2,
        "storm carrier + shard Template"
    );

    let storm = bz
        .collision_windows
        .iter()
        .find(|w| w.id == "storm")
        .expect("storm window present");
    assert!(!storm.strikes, "storm is a non-striking carrier");
    let emitter = storm.emitter.as_ref().expect("storm carries an emitter");

    let shard = bz
        .collision_windows
        .iter()
        .find(|w| w.id == "shard")
        .expect("shard window present");
    assert!(matches!(
        shard.spawn,
        obelisk_bevy::assets::WindowSpawn::Template
    ));
    assert!(matches!(
        shard.motion_direction,
        obelisk_bevy::assets::MotionDirection::Down
    ));

    // Emitter wiring invariant (mirrors validate_timeline): emitter.window must name a window
    // that exists in collision_windows AND whose spawn is Template — an emitter may only ever
    // instantiate a Template window.
    assert_eq!(
        emitter.window, "shard",
        "storm's emitter must target the shard window by id"
    );
    let target = bz
        .collision_windows
        .iter()
        .find(|w| w.id == emitter.window)
        .expect("emitter.window must name a window that exists in collision_windows");
    assert!(
        matches!(target.spawn, obelisk_bevy::assets::WindowSpawn::Template),
        "emitter.window must name a Template window"
    );

    // vfx_cues must be populated (slot==value) or the cues never fire. An emitted Template
    // instance fires `emit_{id}` (CueKind::OnEmit), not `on_window_{id}`.
    assert!(bz.vfx_cues.contains_key("emit_shard") && bz.cues.contains_key("emit_shard"));
    assert!(bz.vfx_cues.contains_key("on_hit") && bz.cues.contains_key("on_hit"));
}

#[test]
fn blizzard_registered_in_skill_registry() {
    let mut app = App::new();
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&arena_game::arena_root().join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(arena_game::arena_root().join("config/skills")));
    let reg = app.world().resource::<SkillRegistry>();
    assert!(reg.0.contains_key("blizzard"), "blizzard registered");
}
