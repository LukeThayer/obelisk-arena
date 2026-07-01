//! Windowed entry point for the obelisk-arena skill designer. Composes `DefaultPlugins` + the modal
//! editor (`EditorPlugin`, `add_physics:false` — the preview owns physics) + the game lifecycle
//! (`GamePlugin`). The Skill mode + preview are layered in by later milestones (`SkillDesignerPlugin`).

use bevy::prelude::*;
use bevy_editor_game::RegisterGltfLibraryExt;
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
        // Index `character.glb`'s clips/meshes/scenes into the editor's asset libraries so the preview
        // rig (`preview_rig.rs`) can build its `AnimationGraph` from the named animation clips.
        .register_gltf_library("character.glb")
        .add_plugins(arena_editor::SkillDesignerPlugin)
        // Load obelisk constants/effects/skills + seed the combat RNG so the preview runs the real
        // deterministic sim (same content as the game).
        .add_plugins(arena_editor::sim_config::PreviewSimConfigPlugin)
        // The preview mini-world runs the real obelisk sim (ArenaSimPreviewPlugin owns plain-Avian
        // physics + Gravity + obelisk). The persistent floor is spawned at Startup and the
        // caster+dummy duel on Play, both by the `PreviewControllerPlugin` in `SkillDesignerPlugin`.
        .add_plugins(arena_sim::preview::ArenaSimPreviewPlugin)
        .run();
}
