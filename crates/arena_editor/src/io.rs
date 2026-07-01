//! Filesystem helpers for the skill designer. `editor_root()` resolves the arena workspace root
//! (holding `assets/` cast timelines + `.skillfx.ron`, and `config/` skill + effect rules) so the
//! editor loads the SAME content the game does, regardless of the launch directory. Mirrors
//! `arena_game::arena_root()`: under `cargo`, `CARGO_MANIFEST_DIR` is `crates/arena_editor`, so the
//! root is two levels up; otherwise fall back to the current working directory.

use std::path::PathBuf;

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
