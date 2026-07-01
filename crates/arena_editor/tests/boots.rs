//! Boot test for the skill-designer editor app (Task 11): the headless editor app comes up with the
//! modal `EditorMode` state registered — proving `EditorPlugin` + `GamePlugin` compose in the
//! `arena_editor` binary.
//!
//! NB: we assert against the freshly-BUILT app (no `update()`). The full editor cannot advance a
//! frame headlessly — its UI systems expect an egui context (we build `add_egui:false` for headless)
//! and its render systems assume a window/surface. That's fine: the windowed binary (`main.rs`,
//! full `DefaultPlugins`) runs the real editor, and the skill-designer LOGIC (mode registration, the
//! preview simulation) is tested in isolation (minimal apps / the `arena_sim` preview harness),
//! where the real correctness risk lives — not by driving the whole editor headlessly.

use bevy::prelude::State;
use bevy_modal_editor::EditorMode;

#[test]
fn editor_app_registers_the_editor_mode_state() {
    let app = arena_editor::build_editor_app();
    assert!(app.world().contains_resource::<State<EditorMode>>());
}
