//! Task 13: the editor's preview mini-world spawns a caster + a dummy via the `arena_editor` wrapper
//! (`spawn_preview_on_startup`) over `arena_sim::preview`. Verified on a headless obelisk app (not
//! the full editor, which can't advance a frame headlessly) + `run_system_once` — what's under test
//! is the wrapper wiring; the combatant recipe itself is arena_sim's (proven in its preview_smoke).

use arena_sim::preview::{ArenaSimPreviewPlugin, PreviewCaster, PreviewDummy};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

#[test]
fn spawn_preview_on_startup_spawns_a_caster_and_a_dummy() {
    obelisk_bevy::testkit::init_test_obelisk();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(ArenaSimPreviewPlugin);
    app.finish();
    app.cleanup();
    app.world_mut()
        .run_system_once(arena_editor::spawn_preview_on_startup)
        .ok();
    app.world_mut().flush();
    assert_eq!(
        app.world_mut()
            .query::<&PreviewCaster>()
            .iter(app.world())
            .count(),
        1,
        "one preview caster"
    );
    assert_eq!(
        app.world_mut()
            .query::<&PreviewDummy>()
            .iter(app.world())
            .count(),
        1,
        "one preview dummy"
    );
}
