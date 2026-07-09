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

/// Deconflict the preview rig's stacked part variants: `character.glb` ships EVERY variant mesh
/// (all hats/tops/hairs at once); the GAME hides non-selected ones per `arena_sim::parts`. Apply
/// the same DEFAULT selection to named nodes under the editor's preview rig — the caster reads
/// as the default in-game witch instead of a heap of outfits. Visibility inherits, so hiding the
/// named node hides its mesh subtree; `Added<Name>` covers the glb scene streaming in.
fn deconflict_preview_rig_parts(
    new_names: Query<(Entity, &Name), Added<Name>>,
    parents: Query<&ChildOf>,
    rig_scenes: Query<(), With<bevy_modal_editor::skill::preview::rig::PreviewCasterRigScene>>,
    mut commands: Commands,
) {
    let selection = arena_sim::parts::PartSelection::default();
    for (entity, name) in &new_names {
        let mut cur = entity;
        let mut under = rig_scenes.contains(cur);
        while !under {
            match parents.get(cur) {
                Ok(p) => {
                    cur = p.0;
                    under = rig_scenes.contains(cur);
                }
                Err(_) => break,
            }
        }
        if !under {
            continue;
        }
        if !selection.is_visible(name.as_str()) {
            commands.entity(entity).insert(Visibility::Hidden);
        }
    }
}

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
            // The glb's single scene is NAMED "Scene" (SceneLibrary key = "{stem}::{name}"); the
            // index fallback would be "character::scene_0".
            scene_key: "character::Scene".to_string(),
            offset: bevy::math::Vec3::new(0.0, -0.62, 0.0),
            // The preview duel line runs along +X (caster at SPAWN_MARKERS[0] = -4, dummy at
            // +4) and the caster body carries an identity Rotation — the GAME's π import yaw
            // (which pairs with a live aim-driven body rotation) reads 90° off here. π/2 faces
            // the model down the duel line at the dummy.
            yaw: std::f32::consts::FRAC_PI_2,
            // Baseline pose — without it every seeded clip stays muted and the rig T-poses.
            idle_clip: Some("character::idle".to_string()),
        })
        // character.glb ships EVERY part variant stacked; hide non-selected ones with the GAME's
        // own default selection (arena_sim::parts — the same rules arena_game's customizer uses).
        .add_systems(Update, deconflict_preview_rig_parts)
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
