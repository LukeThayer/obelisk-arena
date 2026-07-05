//! Scene + asset setup shared by the windowed + headless clients: the minimal 3D scene
//! ([`setup_scene`]), the player rig load ([`load_rig`]), and the one-shot registered-skills/
//! cast-timeline log ([`log_registered_skills_once`]). Cast-timeline loading itself rides the shared
//! `crate::cast_assets` helpers; this module just owns the scene/rig scaffolding the
//! app-composition roots call into.
//!
//! NOTE: cue cosmetic *rendering* (formerly this module's cosmetic-binding registry load) is
//! stubbed pending C3 (which restores it via obelisk's `CueBinding` + `bevy_effect`); there is
//! nothing for this module to load for that path right now.

use bevy::prelude::*;
use obelisk_bevy::prelude::*;

use super::controller::FollowCamera;
use super::rig;

/// Spawn a minimal 3D scene: a camera looking at the origin, a directional
/// light, and a green ground plane.
pub(super) fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        FollowCamera,
        Transform::from_xyz(0.0, 2.0, 4.0).looking_at(Vec3::new(0.0, 1.5, 0.0), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(20.0, 20.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
    ));
    // Static floor collider the predicted Dynamic player body rests on (top face at world 0).
    crate::spawn_arena_floor(&mut commands);
}

/// Kick off the async load of the player character rig (`character.glb`) and insert `RigAssets`.
pub(super) fn load_rig(mut commands: Commands, assets: Res<AssetServer>) {
    let gltf: Handle<bevy::gltf::Gltf> = assets.load("character.glb");
    commands.insert_resource(rig::RigAssets::new(gltf));
}

/// Log the registered skills + loaded cast timelines exactly once.
pub(super) fn log_registered_skills_once(
    mut done: Local<bool>,
    pending: Res<crate::cast_assets::PendingCastTimelines>,
    skills: Res<SkillRegistry>,
    casts: Res<CastTimelineHandles>,
) {
    if *done {
        return;
    }
    if !pending.0.is_empty() {
        return;
    }
    let mut skill_ids: Vec<&String> = skills.0.keys().collect();
    skill_ids.sort();
    let mut cast_ids: Vec<&String> = casts.0.keys().collect();
    cast_ids.sort();
    info!(
        "obelisk skills registered: {:?}; cast timelines loaded: {:?}",
        skill_ids, cast_ids
    );
    *done = true;
}
