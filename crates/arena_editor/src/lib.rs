//! `arena_editor` — a thin HOST SHELL over `bevy_modal_editor`'s built-in obelisk Skill mode.
//!
//! Phase 4 (task C8) collapsed this crate from a ~25-module bespoke skill designer (bolted onto
//! the editor via the old `register_editor_mode` custom-mode seam) down to this shell: the Skill
//! mode, its panel, its palette, and its deterministic preview stage are now BUILT IN to
//! `bevy_modal_editor` itself (enabled via its `obelisk` Cargo feature — see `Cargo.toml`). All
//! this crate does is compose `EditorPlugin` + `GamePlugin` and register the arena workspace as a
//! content root (`bevy_modal_editor::skill::RegisterObeliskContentExt::register_obelisk_content`).
//!
//! `build_editor_app()` is the HEADLESS composition (no window, no event loop) used by tests;
//! `main()` (`main.rs`) is the windowed binary. Both register the SAME content root so a headless
//! assertion on `SkillLibrary` proves what the windowed binary would load.

use bevy::prelude::*;
use bevy_editor_game::RegisterGltfLibraryExt;
use bevy_modal_editor::skill::RegisterObeliskContentExt;
use bevy_modal_editor::{EditorPlugin, EditorPluginConfig, GamePlugin};
use obelisk_bevy::prelude::ObeliskConfigExt;

pub mod io;

/// Build a HEADLESS editor `App` for tests. The editor's render-dependent sub-plugins (outliner
/// JFA, grid material, gaussian splatting, wireframe, …) require the `RenderApp` sub-app even to
/// build, so a real GPU render app is needed (`MinimalPlugins` / a null wgpu backend both fail:
/// the former has no `RenderApp`, the latter creates none). We use full `DefaultPlugins` with:
///   - **no window** (`primary_window: None`) — nothing to present to; and
///   - **`WinitPlugin` disabled** — on macOS winit's event loop must be created on the main thread,
///     but cargo runs tests on worker threads (→ panic). Tests drive the app with manual
///     `app.update()`, so the event loop isn't needed.
/// The real wgpu backend provides the `RenderApp` the editor requires.
///
/// `add_egui:true` (unlike the pre-C8 headless builder, which ran `add_egui:false`): at the
/// pinned `bevy_modal_editor` rev, `ui::material_editor::handle_material_copy_paste` is registered
/// unconditionally in `Update` and takes `EguiContexts` unconditionally (its own `EditorMode`
/// early-out reads the mode INSIDE the system body, after Bevy has already validated/fetched the
/// param) — with no `EguiPlugin` in the app, that param validation fails and Bevy's default error
/// handler panics on the very first `app.update()`, before any other system (incl. this crate's own
/// `Startup` scan) gets a chance to run. `EguiPlugin` itself works fine with zero windows (nothing
/// to attach a context to, nothing to draw), so `add_egui:true` sidesteps the gap without needing a
/// real window. `add_physics:false` still holds — the built-in Skill mode's preview stage installs
/// plain Avian itself — which is why `init_gizmo_group::<PhysicsGizmos>()` below is still required
/// (same reason `main.rs` needs it: `add_physics:false` skips Avian's `PhysicsDebugPlugin`, which
/// normally registers that gizmo config group, but `editor::state::suppress_physics_debug_in_particle_mode`
/// reads it unconditionally every `Update`). The windowed binary (`main.rs`) keeps the full
/// `DefaultPlugins` incl. winit and was never at risk of either panic (`add_egui:true` there too,
/// and it already registers the gizmo group).
///
/// Registers the SAME content root as `main.rs` (`register_gltf_library` + `add_obelisk_effects` +
/// `register_obelisk_content`) — note `register_obelisk_content` only QUEUES the root
/// (`PendingContentRoots`); it's scanned into `SkillLibrary` by a `Startup` system, so callers must
/// run at least one `app.update()` before asserting on `SkillLibrary` contents.
pub fn build_editor_app() -> App {
    let root = io::editor_root();
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                close_when_requested: false,
                ..default()
            })
            // Same workspace-assets root as the windowed binary (`main.rs`) — the default root is
            // CARGO_MANIFEST_DIR (= crates/arena_editor), which has no `character.glb`.
            .set(AssetPlugin {
                file_path: root.join("assets").to_string_lossy().into_owned(),
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>(),
    )
    .add_plugins(EditorPlugin::new(EditorPluginConfig {
        add_egui: true,
        add_physics: false,
        ..default()
    }))
    .add_plugins(GamePlugin)
    .register_gltf_library("character.glb")
    .add_obelisk_effects(&root.join("config/effects"))
    .register_obelisk_content(root)
    // See `main.rs`'s own copy of this call for why `add_physics:false` needs it.
    .init_gizmo_group::<avian3d::prelude::PhysicsGizmos>();
    app
}
