//! Native per-tick input replicated by lightyear (`input::native::InputPlugin::<ArenaInput>`).
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

pub use arena_sim::input::ArenaInput;
