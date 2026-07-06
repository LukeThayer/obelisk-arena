//! Task C8 smoke test: boots the thin shell headlessly for a few frames and asserts that
//! `bevy_modal_editor`'s built-in `SkillLibrary` (the Skill mode's content registry) picked up the
//! arena's skills — proving `register_obelisk_content(editor_root())` actually wires this
//! workspace's `config/skills/` + `assets/skills/` into the built-in Skill mode, not just that the
//! shell compiles.
//!
//! `register_obelisk_content` only QUEUES the root (`PendingContentRoots`); the real scan runs in
//! a `Startup` system (`scan_registered_content_roots`), so at least one `app.update()` is required
//! before `SkillLibrary` is populated. A few more frames beyond that prove the shell doesn't panic
//! once ticking either (the built-in preview stage's own `Startup`/`Update` systems run too).
//!
//! `app.finish()` + `app.cleanup()` before the update loop: `App::run()` normally calls both just
//! before handing off to its runner (which is what installs `RenderDevice`/etc. via
//! `RenderPlugin::finish()`), but this test drives `update()` manually instead of calling `run()`,
//! so it has to call them itself — otherwise the first `update()` panics deep in `bevy_pbr`
//! (`Res<RenderDevice>` doesn't exist yet).

use bevy_modal_editor::skill::SkillLibrary;

#[test]
fn shell_boots_and_loads_the_arenas_skills() {
    let mut app = arena_editor::build_editor_app();
    app.finish();
    app.cleanup();
    for _ in 0..10 {
        app.update();
    }
    let library = app.world().resource::<SkillLibrary>();
    for id in ["firebolt", "firebolt_explosion", "chain_lightning", "blizzard"] {
        assert!(
            library.skills.contains_key(id),
            "expected skill '{id}' to be loaded from config/skills into SkillLibrary; got {:?}",
            library.skills.keys().collect::<Vec<_>>()
        );
    }
}
