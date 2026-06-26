//! Server-side arena gameplay plugin (player spawning, controller, rounds, egress). Filled in by
//! Task 9 onward.
use bevy::prelude::*;

pub struct ArenaServerPlugin;

impl Plugin for ArenaServerPlugin {
    fn build(&self, _app: &mut App) {
        // Task 9 adds sync_networked_players + refresh_replicate_on_connect here.
    }
}
