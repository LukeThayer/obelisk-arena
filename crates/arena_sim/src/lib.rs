//! Transport-agnostic arena simulation shared by arena_game + arena_editor.

pub mod ballistics;
pub mod input;
pub mod obelisk;
pub mod preview;
pub mod shared_controller;
pub mod spawn;
pub mod tuning;

pub const ARENA_SIM_TICK_HZ: u32 = 60;
