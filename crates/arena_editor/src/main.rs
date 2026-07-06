//! Windowed entry point for the obelisk-arena editor. A thin HOST SHELL over `bevy_modal_editor`'s
//! built-in obelisk Skill mode (see `lib.rs`'s module doc comment for the phase-4 collapse this
//! replaced): composes `DefaultPlugins` + the modal editor (`EditorPlugin`, with the `obelisk`
//! Cargo feature enabled on the `bevy_modal_editor` dep — auto-wires `EditorMode::Skill`, its
//! panel/palette, and the deterministic preview stage) + the game lifecycle (`GamePlugin`), then
//! registers this workspace as the Skill mode's content root.

use bevy::prelude::*;
use bevy_editor_game::RegisterGltfLibraryExt;
use bevy_modal_editor::skill::RegisterObeliskContentExt; // register_obelisk_content
use bevy_modal_editor::{recommended_image_plugin, EditorPlugin, EditorPluginConfig, GamePlugin};
use obelisk_bevy::prelude::ObeliskConfigExt; // add_obelisk_effects

fn main() {
    let root = arena_editor::io::editor_root();
    App::new()
        .add_plugins(
            DefaultPlugins.set(recommended_image_plugin()).set(AssetPlugin {
                // Point the asset server at the WORKSPACE assets/ (character.glb + skill RONs).
                // Bevy's default root is CARGO_MANIFEST_DIR = crates/arena_editor, whose assets/
                // holds only the editor's fs-loaded preset libraries (vfx/materials/prefabs) —
                // `character.glb` would 404 without this override.
                file_path: root.join("assets").to_string_lossy().into_owned(),
                ..default()
            }),
        )
        // EditorPlugin with the `obelisk` dep-feature auto-wires the built-in Skill mode +
        // bevy_effect + EffectLibrary; the preview stage seeds obelisk constants/RNG/sim itself.
        .add_plugins(EditorPlugin::new(EditorPluginConfig {
            add_physics: false,
            add_egui: true,
            ..default()
        }))
        .add_plugins(GamePlugin)
        // Index `character.glb`'s clips/meshes/scenes into the editor's asset libraries (a rig for
        // a future preview cast animation — the built-in preview stage doesn't consume it yet).
        .register_gltf_library("character.glb")
        // register_obelisk_content loads the skill triad (rules TOML + .cast.ron + cues) + the
        // assets/effects + assets/vfx presets — but NOT the stat_core AILMENT effects
        // (config/effects/*.toml, e.g. `burn`), which previews that apply an ailment need.
        .add_obelisk_effects(&root.join("config/effects"))
        .register_obelisk_content(root.clone())
        // add_physics:false skips Avian's PhysicsDebugPlugin, which normally registers the
        // PhysicsGizmos config group the editor's gizmo systems read — register it so boot doesn't
        // panic on a missing GizmoConfigStore entry.
        .init_gizmo_group::<avian3d::prelude::PhysicsGizmos>()
        .run();
}
