//! Arena level vocabulary + (in later tasks) the level catalog/loader/spawner. Levels are
//! editor-authored `.scn.ron` scenes (bevy `DynamicScene` RON); this module owns everything the
//! GAME needs to consume them and the marker types the EDITOR saves into them.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A player spawn point authored into a level scene. Match levels need slots 0 and 1 (duelist
/// spawns — faction by slot, exactly like the old `SPAWN_MARKERS`); the lobby uses any number of
/// points (players placed round-robin by sorted-id index % count). The entity's Transform
/// provides the spawn position AND facing (yaw from its forward vector). Registered in the editor
/// by the arena_editor shell (`register_custom_entity`) so it is palette-insertable and
/// round-trips through scene saves; registered in the game's level `TypeRegistry` so the loader
/// reads it back.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Component, Default)]
pub struct ArenaSpawnPoint {
    pub slot: u8,
}
