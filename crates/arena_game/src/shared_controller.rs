//! Shared force-based character controller, driven by native [`ArenaInput`].
//!
//! The implementation now lives in `arena_sim::shared_controller` (transport-agnostic, shared with
//! the editor preview); this module re-exports it so existing `crate::shared_controller::*` use
//! sites (the controllers on both peers + `crate::net`'s planar-const re-export) resolve unchanged.
//! See `arena_sim/src/shared_controller.rs` for the full rationale + the controller's unit test.

pub use arena_sim::shared_controller::*;
