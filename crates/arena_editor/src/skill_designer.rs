//! The Skill designer: registers a custom **Skill** editor mode (key `K`, right-side panel) via the
//! generic `register_editor_mode` seam, and (later milestones) drives the bottom-dock timeline that
//! authors obelisk skills + the "Play the real skill" preview.

use bevy::prelude::*;
use bevy_modal_editor::{CustomModeDef, PanelSide, RegisterEditorModeExt};

/// The interned id of the Skill mode (matches `EditorMode::Custom(CustomModeId(SKILL_MODE_ID))`).
pub const SKILL_MODE_ID: &str = "skill";

/// The Skill-mode panel. Stub for now (M2 grows it into the bottom-dock timeline).
fn draw_skill_panel() {}

/// Register the Skill mode with the editor's `CustomModeRegistry`: key `K`, a right-side panel, and
/// the `draw_skill_panel` system. Idempotently callable on any `App` that has a `CustomModeRegistry`
/// (the editor's `EditorStatePlugin` inits it).
pub fn register_skill_mode(app: &mut App) {
    let panel = app.register_system(draw_skill_panel);
    app.register_editor_mode(CustomModeDef {
        id: SKILL_MODE_ID,
        name: "SKILL",
        activation_key: KeyCode::KeyK,
        panel_side: PanelSide::Right,
        panel,
    });
}

/// Plugin that registers the Skill mode. Added by the windowed binary + the headless editor app.
pub struct SkillDesignerPlugin;

impl Plugin for SkillDesignerPlugin {
    fn build(&self, app: &mut App) {
        register_skill_mode(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_modal_editor::{CustomModeId, CustomModeRegistry};

    /// `register_skill_mode` adds the Skill mode to the registry. Tested on a MINIMAL app (just the
    /// registry) — the full editor app can't advance a frame headlessly (egui/render), and this is
    /// what we actually own: the registration wiring.
    #[test]
    fn register_skill_mode_adds_the_skill_mode_to_the_registry() {
        let mut app = App::new();
        app.init_resource::<CustomModeRegistry>();
        register_skill_mode(&mut app);
        let reg = app.world().resource::<CustomModeRegistry>();
        assert!(
            reg.lookup(CustomModeId(SKILL_MODE_ID)).is_some(),
            "Skill mode should be registered under CustomModeId(\"skill\")"
        );
    }
}
