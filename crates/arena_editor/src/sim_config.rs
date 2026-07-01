//! `PreviewSimConfigPlugin` — loads obelisk's global config (constants + effect registry + skills)
//! and seeds the combat RNG for the editor preview, mirroring the game's server/client composition
//! (`arena_game` `bin/server.rs:47-50` / `client/app_windowed.rs:65-71`). This is what makes the
//! "Play the real skill" preview run the SAME deterministic obelisk sim the game does: same skill
//! rules, same effects, a fixed seed. The `ObeliskConfigExt` calls populate their resources
//! synchronously at plugin-build time, so no `update()` is needed to observe them.

use crate::io::editor_root;
use bevy::prelude::*;
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillSource};

/// Loads obelisk constants (defaults) + `config/effects` + `config/skills` from the arena workspace
/// root and seeds the combat RNG with a fixed seed (the editor preview is single-player and never
/// contends over the seed; determinism is what matters).
pub struct PreviewSimConfigPlugin;

impl Plugin for PreviewSimConfigPlugin {
    fn build(&self, app: &mut App) {
        let root = editor_root();
        app.add_obelisk_config_constants_default();
        app.add_obelisk_effects(&root.join("config/effects"));
        app.add_obelisk_skills(SkillSource::Dir(root.join("config/skills")));
        app.seed_combat_rng(1);
    }
}
