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

/// Spawn the camera. That's ALL the fixed scene now: geometry (floor/walls), colliders, and
/// lights are LEVEL DATA — `client::level::sync_level_from_round_state` spawns them from the
/// server-announced `.scn.ron` (lobby on join). The old hard-coded green plane +
/// `spawn_arena_floor` collider + directional light are gone with the levels-and-lobby design.
pub(super) fn setup_scene(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        // HDR is REQUIRED for bevy_vfx's billboard render pipeline: its node renders only into an
        // `Rgba16Float` (HDR) view target and silently bails on an LDR one
        // (`bevy_vfx/src/render/billboard.rs`: `main_texture_format() != TEXTURE_FORMAT_HDR`).
        // Without this, every cue effect spawns + simulates but never draws. `Hdr` is a marker
        // component in Bevy 0.18 (not a `Camera` field), matching how the editor enables it.
        bevy::render::view::Hdr,
        FollowCamera,
        // The main camera renders LAYER 0 ONLY (implicit default) — the local player's own
        // body lives on `present::SELF_BODY_LAYER`, visible only to the portal cameras.
        Transform::from_xyz(0.0, 2.0, 4.0).looking_at(Vec3::new(0.0, 1.5, 0.0), Vec3::Y),
    ));
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
