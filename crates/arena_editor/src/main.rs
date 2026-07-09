//! Windowed entry point for the obelisk-arena editor. A thin HOST SHELL over `bevy_modal_editor`'s
//! built-in obelisk Skill mode (see `lib.rs`'s module doc comment for the phase-4 collapse this
//! replaced): composes `DefaultPlugins` + the modal editor (`EditorPlugin`, with the `obelisk`
//! Cargo feature enabled on the `bevy_modal_editor` dep — auto-wires `EditorMode::Skill`, its
//! panel/palette, and the deterministic preview stage) + the game lifecycle (`GamePlugin`), then
//! registers this workspace as the Skill mode's content root.

use arena_sim::level::ArenaSpawnPoint;
use bevy::prelude::*;
use bevy_editor_game::{CustomEntityType, RegisterCustomEntityExt, RegisterGltfLibraryExt};
use bevy_modal_editor::skill::RegisterObeliskContentExt; // register_obelisk_content
use bevy_modal_editor::{recommended_image_plugin, EditorPlugin, EditorPluginConfig, GamePlugin};
use obelisk_bevy::prelude::ObeliskConfigExt; // add_obelisk_effects

/// Gizmo for a placed [`ArenaSpawnPoint`]: a lime sphere at the point + an arrow showing the
/// spawn FACING (players spawn looking along the marker's forward).
fn draw_spawn_point_gizmo(gizmos: &mut Gizmos, transform: &GlobalTransform) {
    let pos = transform.translation();
    gizmos.sphere(
        Isometry3d::from_translation(pos),
        0.4,
        bevy::color::palettes::css::LIME,
    );
    gizmos.arrow(
        pos,
        pos + transform.forward() * 1.2,
        bevy::color::palettes::css::LIME,
    );
}

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
        // Index `character.glb`'s clips/meshes/scenes into the editor's asset libraries, and hang
        // that rig under the Skill mode's preview caster (PreviewCasterRig): the bone picker lists
        // its real joints, authored anims play on it, and bone-anchored cue/charge previews ride
        // the animated skeleton. Offset/yaw = the GAME's rig conventions
        // (arena_game::client::present RIG_FOOT_OFFSET -0.62 + π gltf import yaw).
        .register_gltf_library("character.glb")
        .insert_resource(bevy_modal_editor::PreviewCasterRig {
            scene_key: "character::scene0".to_string(),
            offset: bevy::math::Vec3::new(0.0, -0.62, 0.0),
            yaw: std::f32::consts::PI,
        })
        // register_obelisk_content loads the skill triad (rules TOML + .cast.ron + cues) + the
        // assets/effects + assets/vfx presets — but NOT the stat_core AILMENT effects
        // (config/effects/*.toml, e.g. `burn`), which previews that apply an ailment need.
        .add_obelisk_effects(&root.join("config/effects"))
        .register_obelisk_content(root.clone())
        // Arena level vocabulary: spawn points are palette-insertable ("Game" category), draw a
        // facing gizmo, and round-trip through scene saves (register_custom_entity adds the type
        // to the scene-save allow-list). The GAME's level loader reads them back
        // (arena_sim::level) — match levels need slots 0 and 1; the lobby any number.
        .register_custom_entity::<ArenaSpawnPoint>(CustomEntityType {
            name: "Arena Spawn Point",
            category: "Game",
            keywords: &["spawn", "player", "start", "arena", "lobby"],
            default_position: Vec3::new(0.0, 0.6, 0.0),
            spawn: |commands, position, rotation| {
                commands
                    .spawn((
                        ArenaSpawnPoint::default(),
                        Transform::from_translation(position).with_rotation(rotation),
                        Visibility::default(),
                    ))
                    .id()
            },
            draw_inspector: None,
            draw_gizmo: Some(draw_spawn_point_gizmo),
            regenerate: None,
        })
        // add_physics:false skips Avian's PhysicsDebugPlugin, which normally registers the
        // PhysicsGizmos config group the editor's gizmo systems read — register it so boot doesn't
        // panic on a missing GizmoConfigStore entry.
        .init_gizmo_group::<avian3d::prelude::PhysicsGizmos>()
        .run();
}
