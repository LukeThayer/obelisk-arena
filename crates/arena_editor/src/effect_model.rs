//! The effect-authoring model: the `EditedEffect` resource (the in-flight `EffectConfig` open in
//! the Effects tab). `EffectConfig` has no `Default` derive — `blank_effect` builds a minimal
//! instance through serde (every field except id/name is `#[serde(default)]`).

use bevy::prelude::*;
use stat_core::config::EffectConfig;
use std::path::PathBuf;

/// The effect body currently open in the designer: its `EffectConfig`, the
/// `config/effects/<id>.toml` path it saves to, and whether it has unsaved edits.
#[derive(Resource)]
pub struct EditedEffect {
    pub config: EffectConfig,
    pub path: PathBuf,
    pub dirty: bool,
}

impl EditedEffect {
    /// Open `config` for editing, saving to `path`, with no unsaved edits.
    pub fn from_config(config: EffectConfig, path: PathBuf) -> Self {
        Self { config, path, dirty: false }
    }
}

/// A minimal fresh effect: 5s buff-shaped defaults, no modifiers/conditions yet.
pub fn blank_effect(id: &str) -> EffectConfig {
    let mut c: EffectConfig =
        toml::from_str(&format!("id = \"{id}\"\nname = \"{id}\"")).expect("minimal EffectConfig");
    c.duration = stat_core::config::effects::EffectDuration::Finite(5.0);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_effect_is_a_minimal_finite_buff() {
        let e = blank_effect("haste");
        assert_eq!(e.id, "haste");
        assert!(!e.is_debuff);
        assert!(e.modifiers.is_empty());
        assert!((e.duration.as_seconds() - 5.0).abs() < f64::EPSILON);
        assert_eq!(e.max_stacks, 1);
    }
}
