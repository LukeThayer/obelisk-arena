//! Transport-agnostic arena simulation shared by arena_game + arena_editor.

pub mod ballistics;
pub mod input;
pub mod level;
pub mod obelisk;
pub mod preview;
pub mod shared_controller;
pub mod spawn;
pub mod parts;
pub mod tuning;

pub const ARENA_SIM_TICK_HZ: u32 = 60;

use avian3d::prelude::PhysicsLayer;

/// Collision layers (wisp's `GameLayer`, arena-adapted). Introduced for portal pass-through:
/// level statics are `Ground`, players are `Player`, and a player standing inside a floor
/// portal's slab drops `Ground` from its filter so it falls INTO the disc. Everything else
/// (hurtbox sensors, spires, props) stays on `Default` with ALL filters — layer-agnostic.
#[derive(PhysicsLayer, Default, Clone, Copy, Debug)]
pub enum GameLayer {
    #[default]
    Default,
    Player,
    Ground,
}
