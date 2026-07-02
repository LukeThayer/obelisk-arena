//! Filesystem helpers for the skill designer. `editor_root()` resolves the arena workspace root
//! (holding `assets/` cast timelines + `.skillfx.ron`, and `config/` skill + effect rules) so the
//! editor loads the SAME content the game does, regardless of the launch directory. Mirrors
//! `arena_game::arena_root()`: under `cargo`, `CARGO_MANIFEST_DIR` is `crates/arena_editor`, so the
//! root is two levels up; otherwise fall back to the current working directory.

use arena_skills::SkillFx;
use obelisk_bevy::assets::CastTimeline;
use obelisk_bevy::prelude::SkillRegistry;
use stat_core::config::EffectConfig;
use stat_core::Skill;
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

/// The canonical obelisk rules path for a skill id, under the workspace `config/skills/`.
pub fn default_rules_path(skill_id: &str) -> PathBuf {
    editor_root().join(format!("config/skills/{skill_id}.toml"))
}

/// Serialize a `Skill` to `path` as TOML (full-rewrite: every default field is written — a
/// verbosity cost accepted for v1). Emits the bare top-level shape `load_skills_dir` accepts.
pub fn save_skill_rules(skill: &Skill, path: &Path) -> std::io::Result<()> {
    let s = toml::to_string(skill)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, s)
}

/// Parse a `Skill` from a rules TOML file, returning a human-readable error string on failure
/// (so callers can fall back to a blank seed).
pub fn load_skill_rules(path: &Path) -> Result<Skill, String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str::<Skill>(&s).map_err(|e| e.to_string())
}

/// The skill ids on disk: `config/skills/*.toml` file stems, sorted. (Arena convention is one
/// bare top-level skill per file; a `[[skills]]` array file would list under its filename stem.)
pub fn list_skill_ids() -> Vec<String> {
    let dir = editor_root().join("config/skills");
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// Re-load `config/skills` into the live `SkillRegistry` resource so the "Play the real skill"
/// preview casts with the just-saved rules. Returns the number of skills loaded. On error the
/// registry is left UNCHANGED (the loader validates trigger_skill refs and can reject the dir).
pub fn reload_skill_registry(reg: &mut SkillRegistry) -> Result<usize, String> {
    let map = stat_core::config::load_skills_dir(&editor_root().join("config/skills"))
        .map_err(|e| e.to_string())?;
    let n = map.len();
    reg.0 = map;
    Ok(n)
}

/// The canonical effect-body path for an effect id, under the workspace `config/effects/`.
pub fn default_effect_path(effect_id: &str) -> PathBuf {
    editor_root().join(format!("config/effects/{effect_id}.toml"))
}

/// Serialize an `EffectConfig` to `path` as TOML (full-rewrite, like skill rules).
pub fn save_effect_config(config: &EffectConfig, path: &Path) -> std::io::Result<()> {
    let s = toml::to_string(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, s)
}

/// Parse an `EffectConfig` from a TOML file, human-readable error on failure.
pub fn load_effect_config(path: &Path) -> Result<EffectConfig, String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str::<EffectConfig>(&s).map_err(|e| e.to_string())
}

/// The effect ids on disk: `config/effects/*.toml` stems, sorted.
pub fn list_effect_ids_on_disk() -> Vec<String> {
    let dir = editor_root().join("config/effects");
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// Re-load `config/effects` and SWAP it into the process-global obelisk registry so the preview
/// resolves the just-saved effect bodies (Task 9's stat_core API). Returns the effect count.
/// On load error the registry is left unchanged.
pub fn reload_effect_registry() -> Result<usize, String> {
    let reg = stat_core::config::load_effect_configs(&editor_root().join("config/effects"))
        .map_err(|e| e.to_string())?;
    let n = reg.all_ids().len();
    stat_core::config::swap_effect_registry(reg);
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::{default_rules_path, load_skill_rules, save_skill_rules, list_skill_ids, reload_skill_registry, default_effect_path, load_effect_config, save_effect_config, list_effect_ids_on_disk, reload_effect_registry};

    /// Full-rewrite round-trip against the REAL firebolt rules file (under cargo,
    /// `editor_root()` resolves the obelisk-arena workspace root where config/skills lives).
    #[test]
    fn skill_rules_round_trip_the_real_firebolt() {
        let loaded = load_skill_rules(&default_rules_path("firebolt")).expect("firebolt.toml parses");
        assert_eq!(loaded.id, "firebolt");
        let tmp = std::env::temp_dir().join("m4_io_test_firebolt.toml");
        save_skill_rules(&loaded, &tmp).expect("save");
        let reloaded = load_skill_rules(&tmp).expect("reparse");
        assert_eq!(loaded, reloaded, "full-rewrite serialize must round-trip losslessly");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn list_skill_ids_contains_firebolt() {
        assert!(list_skill_ids().iter().any(|s| s == "firebolt"));
    }

    #[test]
    fn reload_skill_registry_loads_the_real_dir() {
        let mut reg = obelisk_bevy::prelude::SkillRegistry::default();
        let n = reload_skill_registry(&mut reg).expect("reload");
        assert!(n >= 1);
        assert!(reg.0.contains_key("firebolt"));
    }

    #[test]
    fn effect_config_round_trips_the_real_burn() {
        let loaded = load_effect_config(&default_effect_path("burn")).expect("burn.toml parses");
        assert_eq!(loaded.id, "burn");
        let tmp = std::env::temp_dir().join("m4_io_test_burn.toml");
        save_effect_config(&loaded, &tmp).expect("save");
        let reloaded = load_effect_config(&tmp).expect("reparse");
        assert_eq!(loaded, reloaded);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn list_effect_ids_on_disk_contains_burn() {
        assert!(list_effect_ids_on_disk().iter().any(|s| s == "burn"));
    }

    #[test]
    fn reload_effect_registry_swaps_in_the_real_dir() {
        let n = reload_effect_registry().expect("swap");
        assert!(n >= 1);
        assert!(stat_core::config::effect_registry().get("burn").is_some());
    }
}
