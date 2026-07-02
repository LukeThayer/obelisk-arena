//! The designer's selection model (UX spec D4): ONE thing is selected — the skill root, a
//! collision window, or a cosmetic lane — and the right-side inspector shows ITS properties.
//! The graph canvas (P2) and viewport gizmos key off the same resource.

use bevy::prelude::*;

/// What the inspector is currently showing. `Root` = the skill itself (id, aim, phases,
/// costs, damage); `Window(i)` = `timeline.collision_windows[i]`; `Lane(cue_id)` = the
/// cosmetic lane bound to that cue VALUE (e.g. `"firebolt_end_bolt"`).
#[derive(Resource, Default, Clone, PartialEq, Eq, Debug)]
pub enum SkillSelection {
    #[default]
    Root,
    Window(usize),
    Lane(String),
}

impl SkillSelection {
    /// The selected window index, if a window is selected.
    pub fn window(&self) -> Option<usize> {
        match self {
            SkillSelection::Window(i) => Some(*i),
            _ => None,
        }
    }
}
