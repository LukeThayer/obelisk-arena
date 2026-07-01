//! Windowed entry point for the obelisk-arena skill designer. Composes `DefaultPlugins` + the modal
//! editor (`EditorPlugin`, `add_physics:false` — the preview owns physics) + the game lifecycle
//! (`GamePlugin`). The Skill mode + preview are layered in by later milestones (`SkillDesignerPlugin`).

use bevy::prelude::*;
use bevy_modal_editor::{recommended_image_plugin, EditorPlugin, EditorPluginConfig, GamePlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(recommended_image_plugin()))
        .add_plugins(EditorPlugin::new(EditorPluginConfig {
            add_physics: false,
            add_egui: true,
            ..default()
        }))
        .add_plugins(GamePlugin)
        .add_plugins(arena_editor::SkillDesignerPlugin)
        .run();
}
