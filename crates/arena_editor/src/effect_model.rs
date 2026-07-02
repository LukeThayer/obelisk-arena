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

/// Sanitize an id typed into an authoring field (skill id or effect id) before it's interpolated
/// into a bare TOML string (`blank_effect`) or a filesystem path (`default_effect_path` /
/// `default_rules_path`). Lowercases, then accepts only nonempty strings made entirely of
/// `[a-z0-9_-]` — this rejects both TOML-injection payloads (quotes, newlines, `[table]` syntax)
/// and path traversal (`../evil`, `/etc/passwd`).
pub fn sanitize_id(raw: &str) -> Option<String> {
    let lowered = raw.to_lowercase();
    if lowered.is_empty() {
        return None;
    }
    let ok = lowered
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    ok.then_some(lowered)
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

    #[test]
    fn sanitize_id_passes_through_a_valid_id() {
        assert_eq!(sanitize_id("haste_2-buff"), Some("haste_2-buff".to_string()));
    }

    #[test]
    fn sanitize_id_lowercases_uppercase_input() {
        assert_eq!(sanitize_id("Haste"), Some("haste".to_string()));
    }

    #[test]
    fn sanitize_id_rejects_empty() {
        assert_eq!(sanitize_id(""), None);
    }

    #[test]
    fn sanitize_id_rejects_path_traversal() {
        assert_eq!(sanitize_id("../evil"), None);
    }

    #[test]
    fn sanitize_id_rejects_quote_characters() {
        assert_eq!(sanitize_id("foo\"bar"), None);
    }
}
