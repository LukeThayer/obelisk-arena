//! The Skill designer: registers a custom **Skill** editor mode (key `K`, right-side panel) via the
//! generic `register_editor_mode` seam, and (later milestones) drives the bottom-dock timeline that
//! authors obelisk skills + the "Play the real skill" preview.

use crate::model::{blank_cast_timeline, blank_skillfx, EditedSkill, EditedSkillFx};
use bevy::prelude::*;
use bevy_modal_editor::{CustomModeDef, PanelSide, RegisterEditorModeExt};

/// The interned id of the Skill mode (matches `EditorMode::Custom(CustomModeId(SKILL_MODE_ID))`).
pub const SKILL_MODE_ID: &str = "skill";

/// Register the Skill mode with the editor's `CustomModeRegistry`: key `K`, a right-side panel, and
/// the `draw_skill_panel` system. Idempotently callable on any `App` that has a `CustomModeRegistry`
/// (the editor's `EditorStatePlugin` inits it).
pub fn register_skill_mode(app: &mut App) {
    let panel = app.register_system(crate::panel::draw_skill_panel);
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
        // The preview lifecycle: a persistent floor at Startup + the Play→duel handler (retires the
        // old idle-startup combatant spawn — combatants now spawn on `GameStartedEvent`).
        app.add_plugins(crate::preview_controller::PreviewControllerPlugin);
        // Draw the selected hit-window's shape as a viewport gizmo while in Skill mode.
        app.add_systems(Update, crate::gizmo::draw_window_gizmo);
        // Index named rig sockets under the preview caster so cosmetic lanes can bind to bones.
        app.init_resource::<crate::socket::RigSockets>()
            .add_systems(Update, crate::socket::index_rig_sockets);
        // Spawn the `character.glb` rig under the preview caster on Play + build/attach its anim graph.
        app.add_plugins(crate::preview_rig::PreviewRigPlugin);
        // Seed the designer with firebolt's real `.cast.ron` if it parses, else a blank timeline
        // pointed at that canonical path (load-or-blank).
        let path = crate::io::default_cast_path("firebolt");
        let timeline = crate::io::load_cast_timeline(&path)
            .unwrap_or_else(|_| blank_cast_timeline("firebolt"));
        app.insert_resource(EditedSkill::from_timeline(timeline, path));
        // Load firebolt's `.skillfx.ron` cosmetic layer if it parses, else a blank one pointed at
        // the canonical path (load-or-blank). Save writes this alongside the `.cast.ron`.
        let fx_path = crate::io::default_skillfx_path("firebolt");
        let fx = crate::io::load_skillfx(&fx_path).unwrap_or_else(|_| blank_skillfx("firebolt"));
        app.insert_resource(EditedSkillFx::from_fx(fx, fx_path));
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
