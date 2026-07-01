//! Filesystem helpers for the skill designer. `editor_root()` resolves the arena workspace root
//! (holding `assets/` cast timelines + `.skillfx.ron`, and `config/` skill + effect rules) so the
//! editor loads the SAME content the game does, regardless of the launch directory. Mirrors
//! `arena_game::arena_root()`: under `cargo`, `CARGO_MANIFEST_DIR` is `crates/arena_editor`, so the
//! root is two levels up; otherwise fall back to the current working directory.

use arena_skills::SkillFx;
use obelisk_bevy::assets::CastTimeline;
use std::path::{Path, PathBuf};

/// The arena workspace root (two levels up from `crates/arena_editor`).
pub fn editor_root() -> PathBuf {
    match std::env::var_os("CARGO_MANIFEST_DIR") {
        Some(dir) => PathBuf::from(dir)
            .ancestors()
            .nth(2)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

/// The canonical `.cast.ron` path for a skill id, under the workspace `assets/skills/` directory.
pub fn default_cast_path(skill_id: &str) -> PathBuf {
    editor_root().join(format!("assets/skills/{skill_id}.cast.ron"))
}

/// Serialize a `CastTimeline` to `path` as pretty RON, creating parent directories as needed.
pub fn save_cast_timeline(tl: &CastTimeline, path: &Path) -> std::io::Result<()> {
    let s = ron::ser::to_string_pretty(tl, ron::ser::PrettyConfig::new())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, s)
}

/// Parse a `CastTimeline` from a `.cast.ron` file, returning a human-readable error string on
/// read/parse failure (so callers can fall back to a blank timeline).
pub fn load_cast_timeline(path: &Path) -> Result<CastTimeline, String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    ron::de::from_str::<CastTimeline>(&s).map_err(|e| e.to_string())
}

/// The canonical `.skillfx.ron` path for a skill id, under the workspace `assets/skills/` directory
/// (alongside its `.cast.ron`). The two-file authoring pair the designer reads/writes together.
pub fn default_skillfx_path(skill_id: &str) -> PathBuf {
    editor_root().join(format!("assets/skills/{skill_id}.skillfx.ron"))
}

/// Serialize a `SkillFx` to `path` as pretty RON, creating parent directories as needed.
pub fn save_skillfx(fx: &SkillFx, path: &Path) -> std::io::Result<()> {
    let s = ron::ser::to_string_pretty(fx, ron::ser::PrettyConfig::new())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, s)
}

/// Parse a `SkillFx` from a `.skillfx.ron` file, returning a human-readable error string on
/// read/parse failure (so callers can fall back to a blank cosmetic layer).
pub fn load_skillfx(path: &Path) -> Result<SkillFx, String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    ron::de::from_str::<SkillFx>(&s).map_err(|e| e.to_string())
}
