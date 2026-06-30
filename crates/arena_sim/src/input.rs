//! Native per-tick input replicated by lightyear in the live game (`input::native::InputPlugin::<ArenaInput>`).
//!
//! Lives in `arena_sim` (transport-agnostic) so both the game and the editor preview share one
//! input type; `arena_game::net::input` re-exports it and owns the lightyear wiring.
//!
//! Mirrors `simple_box`'s `Inputs` type. The client buffers an `ArenaInput` each `FixedPreUpdate`
//! (in `InputSystems::WriteClientInputs`) onto its predicted entity's `ActionState<ArenaInput>`;
//! lightyear ships it to the server (which applies it to that client's authoritative entity) and
//! re-applies it on the client's `Predicted` entity during rollback. The shared force controller
//! (`crate::shared_controller`) consumes it on BOTH peers so they integrate in lockstep.
//!
//! Cast is NOT here — it stays a discrete reliable `CastRequestMessage` (movement prediction does
//! not depend on it). `charging` is the only cast-adjacent field: it's a pre-release telegraph the
//! server stamps into `NetworkedCastState.cast_phase` so the opponent sees a cast wind up the
//! instant the local player begins charging (Bug 4). `Default` = no input (idle).
//!
//! Trait bounds match `lightyear_inputs_native`'s `InputPlugin::<A>` requirements (verified against
//! the installed source): `Serialize + DeserializeOwned + Clone + PartialEq + Debug + Default +
//! MapEntities + Reflect/FromReflect`.

use bevy::ecs::entity::{EntityMapper, MapEntities};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-tick native input. Camera-relative movement + body yaw + jump + a charge telegraph.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default, Reflect)]
pub struct ArenaInput {
    /// Camera-relative WASD axis: x = strafe (+right), y = forward.
    pub movement: Vec2,
    /// Camera/body yaw (radians). The controller faces the body to this and builds the
    /// camera-relative movement frame from it.
    pub yaw: f32,
    /// True while the jump button (Space) is held; the controller jumps on any grounded tick.
    pub jump: bool,
    /// True while the local player is holding the cast button to charge (pre-release). Drives the
    /// opponent-facing windup telegraph in `server::mirrors::sync_cast_state`.
    pub charging: bool,
}

impl MapEntities for ArenaInput {
    fn map_entities<M: EntityMapper>(&mut self, _entity_mapper: &mut M) {
        // No entity references in the input.
    }
}
