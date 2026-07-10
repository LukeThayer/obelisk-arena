//! Client-side present + gameplay layer.
//!
//! Two entry points (the `arena-client` bin picks via `ARENA_HEADLESS`):
//!   - [`run_windowed_client`] — the NETWORKED windowed client: DefaultPlugins +
//!     the lightyear client stack + the CLIENT obelisk subset (no authoritative ResolveHits/RNG —
//!     Stage A) + net-driven materialized players (rigged), predicted local movement, first-person
//!     camera, replicated/predicted cosmetics, and the HUD (hp bars + round banner). The duel is
//!     entirely server-authoritative + replicated.
//!   - [`run_headless_client`] — the headless scriptable cast client (`ARENA_HEADLESS=1`, the
//!     `arena-observer` bin): MinimalPlugins + the net stack, materializes players, traces the
//!     replicated combat/cue streams, and (under `ARENA_AUTOCAST`/`ARENA_AUTOMOVE`) scripts casts /
//!     movement over the wire — the net-regression vehicle.
//!
//! This module is just the submodule wiring: the two app-composition roots live in
//! [`app_windowed`]/[`app_headless`], the scene/asset setup in [`scene`], and the
//! test/verification scaffolds in [`harness`].

mod app_headless;
mod app_windowed;
pub mod charge_cues;
pub mod controller;
pub mod cosmetics;
pub mod customization;
mod harness;
pub mod level;
pub mod portals;
pub mod radial;
pub mod skill_objects;
pub(crate) mod surfaces;
pub mod weapon_select;
pub mod hud;
pub mod net;
pub mod parts;
pub mod present;
pub mod rig;
mod scene;
pub mod sockets;

pub use app_headless::run_headless_client;
pub use app_windowed::run_windowed_client;
