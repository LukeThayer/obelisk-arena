//! `arena_editor` — the in-editor obelisk-arena skill designer, built on `bevy_modal_editor`.
//!
//! This crate embeds the generic modal editor (`EditorPlugin`) + its game lifecycle (`GamePlugin`)
//! and adds a custom **Skill** mode (registered via the upstream `register_editor_mode` seam) whose
//! bottom-dock timeline authors obelisk skills and previews them through the real `arena_sim`
//! simulation. See `docs/superpowers/specs/2026-06-30-skill-designer-design.md` in obelisk-bevy.
//!
//! `build_editor_app()` is the HEADLESS composition (no window, no event loop) used by tests;
//! `main()` is the windowed binary.

use bevy::prelude::*;
use bevy_modal_editor::{EditorPlugin, EditorPluginConfig, GamePlugin};

pub mod edits;
pub mod enum_ui;
pub mod fx_edits;
pub mod gizmo;
pub mod io;
pub mod model;
pub mod rules_model;
pub mod panel;
pub mod preview_controller;
pub mod preview_cosmetics;
pub mod preview_rig;
pub mod sim_config;
pub mod skill_designer;
pub mod socket;
pub mod timeline_geom;
pub mod vfx_bind;
pub use model::{blank_cast_timeline, blank_skillfx, derive_vfx_cues, EditedSkill, EditedSkillFx};
pub use preview_controller::PreviewControllerPlugin;
pub use preview_cosmetics::{PreviewCharge, PreviewCosmetic};
pub use sim_config::PreviewSimConfigPlugin;
pub use skill_designer::{register_skill_mode, SkillDesignerPlugin, SKILL_MODE_ID};

/// Build a HEADLESS editor `App` for tests. The editor's render-dependent sub-plugins (outliner
/// JFA, grid material, gaussian splatting, wireframe, …) require the `RenderApp` sub-app even to
/// build, so a real GPU render app is needed (`MinimalPlugins` / a null wgpu backend both fail:
/// the former has no `RenderApp`, the latter creates none). We use full `DefaultPlugins` with:
///   - **no window** (`primary_window: None`) — nothing to present to; and
///   - **`WinitPlugin` disabled** — on macOS winit's event loop must be created on the main thread,
///     but cargo runs tests on worker threads (→ panic). Tests drive the app with manual
///     `app.update()`, so the event loop isn't needed.
/// The real wgpu backend (Metal on macOS) provides the `RenderApp` the editor requires. The editor
/// runs with `add_egui:false` + `add_physics:false` (the preview installs plain Avian itself). The
/// windowed binary (`main.rs`) keeps the full `DefaultPlugins` incl. winit.
pub fn build_editor_app() -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                close_when_requested: false,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>(),
    )
    .add_plugins(EditorPlugin::new(EditorPluginConfig {
        add_egui: false,
        add_physics: false,
        ..default()
    }))
    .add_plugins(GamePlugin);
    app
}
