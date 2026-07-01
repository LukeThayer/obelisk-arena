//! Task 14: `PreviewSimConfigPlugin` loads obelisk constants/effects/skills + seeds the RNG so the
//! editor preview runs the same deterministic sim as the game. Verified on the (headless) editor app
//! because the config ext populates its resources synchronously at plugin-build time (no `update()`).

use obelisk_bevy::prelude::{CombatRng, SkillRegistry};

#[test]
fn preview_sim_config_loads_firebolt_and_seeds_rng() {
    let mut app = arena_editor::build_editor_app();
    app.add_plugins(arena_editor::sim_config::PreviewSimConfigPlugin);
    assert!(
        app.world()
            .resource::<SkillRegistry>()
            .0
            .contains_key("firebolt"),
        "PreviewSimConfigPlugin should load the firebolt skill from config/skills"
    );
    assert!(
        app.world().get_resource::<CombatRng>().is_some(),
        "PreviewSimConfigPlugin should seed a CombatRng"
    );
}
