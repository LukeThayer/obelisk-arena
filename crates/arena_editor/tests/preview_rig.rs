//! Task 29: the preview-rig anim wiring. `clip_node_for` is the pure clip-lookup helper that maps a
//! short clip name (e.g. `casting_idle`) to the `AnimationLibrary` entry keyed `"{gltf}::{name}"`.

use arena_editor::preview_rig::clip_node_for;
use bevy::prelude::*;
use bevy_editor_game::AnimationLibrary;

#[test]
fn clip_node_for_matches_short_name_suffix() {
    let mut lib = AnimationLibrary::default();
    lib.clips
        .insert("character::casting_idle".into(), Handle::default());
    lib.clips
        .insert("character::idle".into(), Handle::default());
    assert!(clip_node_for(&lib, "casting_idle").is_some());
    assert!(clip_node_for(&lib, "idle").is_some());
    assert!(clip_node_for(&lib, "no_such_clip").is_none());
}
