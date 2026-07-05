//! Protocol: identical on every peer. Declares which components replicate, which channels carry
//! which messages, and the message payloads. Shared by server + client (netcode guide §3).
//!
//! API shapes copied from `wisp/src/net/protocol.rs` and verified against the installed lightyear
//! 0.26.4 source (`~/.cargo/registry/.../lightyear_transport-0.26.4/src/channel/builder.rs`,
//! `lightyear_messages`, `lightyear_replication`, and the canonical
//! `lightyear-0.26.4/src/protocol.rs` example). Notable divergence from the guide §3 table:
//! `ChannelMode::UnorderedReliable` takes a `ReliableSettings` argument (the guide wrote it
//! bare) — confirmed against the registry source + the canonical example.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use core::time::Duration;

use crate::client::parts::PartSelection;
use crate::net::input::ArenaInput;
use lightyear::interpolation::registry::InterpolationRegistrationExt;
use lightyear::prediction::registry::PredictionRegistrationExt;
use lightyear::prelude::{
    input, AppChannelExt, AppComponentExt, AppMessageExt, ChannelMode, ChannelSettings,
    NetworkDirection, ReliableSettings,
};
use serde::{Deserialize, Serialize};

/// Plugin: the shared lightyear protocol — native `ArenaInput`, the replicated component set with
/// prediction/interpolation/rollback registration, and the channels + messages. Added by both peers.
pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // --- Native input (lightyear ships `ActionState<ArenaInput>` per tick) ---
        // `rebroadcast_inputs` stays default (false): only the predicting OWNER needs its own input
        // locally; remote players are INTERPOLATED, not simulated, so the server never has to relay
        // inputs to the other clients (the avian_3d_character example sets rebroadcast because it
        // predicts ALL characters — the arena predicts only the owner). Mirrors `simple_box`.
        app.add_plugins(input::native::InputPlugin::<ArenaInput>::default());

        // --- Player identity / ownership (replicated on spawn) ---
        app.register_component::<NetworkedPlayer>();
        app.register_component::<NetworkOwner>(); // NetworkOwner(u64) == client_id (PeerId)
        app.register_component::<NetworkedId>(); // NetworkedId(u64) monotonic, cross-peer stable
        app.register_component::<ObeliskNetId>(); // wraps ObeliskId String (stable combat id)

        // --- Cast state (replicated; drives remote cast animation). ---
        // Pose is now avian `Position`/`Rotation` (predicted+interpolated below); this carries ONLY
        // the obelisk cast phase + skill index that the pose stream can't express. Plain
        // registration (no interpolation): lightyear writes it to the Predicted/Interpolated entity
        // and keeps it updated — discrete state, so snapping is correct (spec §7).
        app.register_component::<NetworkedCastState>();

        // --- Health (replicated; HUD source of truth, server-authoritative). ---
        // No interpolation: hp is discrete and damage feedback feels best snapping.
        app.register_component::<NetworkedHealth>();

        // --- Character appearance (replicated per-player; drives each rig's slot visibility). ---
        // Initial-value replication is reliable in this lightyear setup — that's what the spawn
        // relies on (each player spawns with `PlayerCustomization::default()`). Live appearance
        // changes are pushed via the reliable `CustomizeBroadcast` message path (D6), not by
        // trusting component-update replication (which is unreliable here — see CLAUDE notes).
        app.register_component::<PlayerCustomization>();

        // --- avian physics: lightyear-native prediction (rollback) + interpolation. ---
        // The server spawns each player a
        // Dynamic avian body with `PredictionTarget::Single(owner)` + `InterpolationTarget::
        // AllExceptSingle(owner)`; lightyear predicts Position/Rotation/Velocity on the owner's
        // client (re-simulating the shared controller during rollback) and interpolates Position/
        // Rotation on the others. Rollback thresholds + the linear correction fn mirror the
        // canonical `avian_3d_character` example exactly.
        app.register_component::<Position>()
            .add_prediction()
            .add_should_rollback(position_should_rollback)
            .add_linear_correction_fn()
            .add_linear_interpolation();
        app.register_component::<Rotation>()
            .add_prediction()
            .add_should_rollback(rotation_should_rollback)
            .add_linear_correction_fn()
            .add_linear_interpolation();
        // Velocity components are predicted + rolled back but NOT interpolated (avian's
        // `LinearVelocity`/`AngularVelocity` don't impl `Ease`, and they aren't visual). They drive
        // prediction only; the interpolated remote replicas snap velocity at the send rate.
        app.register_component::<LinearVelocity>()
            .add_prediction()
            .add_should_rollback(velocity_should_rollback);
        app.register_component::<AngularVelocity>()
            .add_prediction()
            .add_should_rollback(angular_velocity_should_rollback);

        // --- Channels ---
        // cast_request: never drop. Reliable.
        app.add_channel::<CastChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            send_frequency: Duration::default(),
            priority: 1.0,
        })
        .add_direction(NetworkDirection::ClientToServer);

        // Combat events + cues + round state: reliable, server→client.
        app.add_channel::<EventChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            send_frequency: Duration::default(),
            priority: 1.0,
        })
        .add_direction(NetworkDirection::ServerToClient);

        // --- Messages ---
        app.register_message::<CastRequestMessage>()
            .add_direction(NetworkDirection::ClientToServer); // cast, on CastChannel
        app.register_message::<NetEventMessage>()
            .add_direction(NetworkDirection::ServerToClient); // wraps obelisk NetEvent (§5)
        app.register_message::<CueWireMessage>()
            .add_direction(NetworkDirection::ServerToClient); // wraps crate::net::cue::CueMessage (§4)
        app.register_message::<RoundStateMessage>()
            .add_direction(NetworkDirection::ServerToClient); // best-of-3 round flow (§7)

        // Live appearance change (D6): client→server request (reused reliable CastChannel) + the
        // server→client broadcast (reused reliable EventChannel). Mirrors the cue broadcast pattern
        // so a live edit propagates reliably (component UPDATES are unreliable in this setup).
        app.register_message::<CustomizeMessage>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<CustomizeBroadcast>()
            .add_direction(NetworkDirection::ServerToClient);
    }
}

// ---------------------------------------------------------------------------------------------
// Channels (unit-struct tags; `Channel` is blanket-impl'd for any Send + Sync + 'static).
// ---------------------------------------------------------------------------------------------

/// Cast requests, client→server, reliable (a dropped cast is unacceptable).
pub struct CastChannel;
/// Combat events + cues + round state, server→client, reliable.
pub struct EventChannel;

// ---------------------------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------------------------

/// Cast request — replaces M1's direct `cast_skill_at` on the local entity (spec §5.2). The client
/// sends its camera-forward `aim_dir` + charge; the SERVER resolves a candidate `CastAim` from the
/// skill's authored `Acquisition` (hitscan-entity / ground-point raycast, else direction) and
/// obelisk's `validate_casts` gates mana/cooldown/already-casting + does the authoritative
/// range/filter/fallback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CastRequestMessage {
    pub skill_id: String,
    /// Camera-forward unit vector in world space (yaw + pitch applied). The server fires the
    /// projectile along this direction; the client uses it for predicted own-cast cosmetics.
    pub aim_dir: [f32; 3],
    /// Hold-to-charge level (0–255). Mapped from hold duration via the formula
    /// `85 + frac * 170` where `frac ∈ [0, 1]`: 85 ≈ instant tap (≈1.0× via `charge_mult`),
    /// 255 = max hold (2.0×). The server passes this to `cast_skill_dir_charged`, scaling both
    /// damage and projectile speed. Formula: `charge_mult(Some(c)) = 0.5 + (c/255) * 1.5`.
    pub charge: u8,
}

/// Combat events on the wire — wraps obelisk's `NetEvent` so the server broadcasts it verbatim.
/// obelisk's `NetEvent` already uses STABLE STRING IDS (not `Entity`) — wire-ready.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetEventMessage(pub obelisk_bevy::net::NetEvent);

/// Cosmetic cue on the wire — wraps `crate::net::cue::CueMessage` (the serde wire type). Client
/// cue rendering is stubbed for now (C3 restores it via `bevy_effect`'s `CueBinding`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CueWireMessage(pub crate::net::cue::CueMessage);

/// Live appearance change, client→server, reliable (`CastChannel`). Sent when the local player
/// finishes editing their costume (panel close). The server applies it to that player's
/// `PlayerCustomization` and re-broadcasts via [`CustomizeBroadcast`] (D6).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomizeMessage {
    pub parts: PartSelection,
}

/// Appearance broadcast, server→client, reliable (`EventChannel`). The server relays a player's
/// new costume to every client; each client applies it to the matching player's rig (keyed by the
/// replicated [`NetworkedId`]). Mirrors the cue broadcast (`CueWireMessage`) — the proven reliable
/// S→C path — because component UPDATES don't propagate reliably in this lightyear setup (only
/// initial inserts do).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomizeBroadcast {
    /// The target player's replicated [`NetworkedId`] (stable cross-peer key).
    pub player: u64,
    pub parts: PartSelection,
}

/// Best-of-3 round flow (guide §7). The wire shape is fixed here so the protocol checksum agrees on
/// both peers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoundStateMessage {
    /// 0 WaitingForPlayers, 1 Countdown, 2 Active, 3 RoundOver, 4 MatchOver.
    pub phase: u8,
    /// Countdown seconds remaining (phase 1) — 0 otherwise.
    pub countdown: f32,
    /// (obelisk_id, round wins) for each of the two players.
    pub scores: [(String, u8); 2],
    /// The winner's obelisk_id for RoundOver / MatchOver, else empty.
    pub winner: String,
    /// Replicated session seed (forward-prep for Stage B; informational in Stage A).
    pub match_seed: u64,
}

// ---------------------------------------------------------------------------------------------
// Replicated components
// ---------------------------------------------------------------------------------------------

/// Server-spawned per-client player marker. Clients materialize a body for each on receive.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkedPlayer;

/// Which connected client owns a replicated entity. Carries the netcode `client_id` (`u64`).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetworkOwner(pub u64);

/// Server-assigned stable cross-peer id (local Bevy `Entity` differs per peer). The harness keys
/// trace correlation on this.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetworkedId(pub u64);

/// The replicated obelisk `ObeliskId` (stable combat id string). The cue de-dup + damage-target
/// lookups key on this on the client side.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObeliskNetId(pub String);

/// Replicated health snapshot. Mirrors obelisk life on the server; clients read it for the HUD.
/// `f64` to match obelisk's `current_life`/`max_life`.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkedHealth {
    pub current: f64,
    pub max: f64,
}

/// Replicated per-player character appearance: the slot-based [`PartSelection`] that drives which
/// `character.glb` meshes are visible on that player's rig. Replicated on spawn (initial value is
/// reliable here) so every client materializes both players with a coherent costume; live edits are
/// pushed via the reliable `CustomizeBroadcast` path (D6). `Default` is the default witch (via
/// `PartSelection::default`).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerCustomization {
    pub parts: PartSelection,
}

/// Replicated obelisk cast state for remote cast animation (spec §7). Pose is now avian
/// `Position`/`Rotation`; this carries ONLY what the pose stream can't: the cast phase byte (so the
/// opponent's rig can play the casting blend) + the skill marker. Discrete — snapped on receive
/// (no interpolation registration).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkedCastState {
    /// 0 none, 1 windup, 2 active, 3 recovery.
    pub cast_phase: u8,
    /// Interned skill marker (1 = casting, 0 = none).
    pub cast_skill: u16,
}

// ---------------------------------------------------------------------------------------------
// Rollback helpers (every-frame, allocation-free).
// ---------------------------------------------------------------------------------------------

/// Per-component rollback threshold, matched to the avian_3d_character reference example. Below this
/// divergence we let prediction stand (avoids float-noise jitter); at or above it the predicted
/// history is corrected. Shared by all four `*_should_rollback` fns so they can never drift apart.
/// The comparisons are NOT reflexive: comparing a value to itself returns false (no rollback), which
/// is correct — only a real divergence `>= ROLLBACK_EPSILON` triggers a rollback (guide §1.2 trap).
const ROLLBACK_EPSILON: f32 = 0.01;

fn position_should_rollback(this: &Position, that: &Position) -> bool {
    (this.0 - that.0).length() >= ROLLBACK_EPSILON
}

fn rotation_should_rollback(this: &Rotation, that: &Rotation) -> bool {
    this.angle_between(*that) >= ROLLBACK_EPSILON
}

fn velocity_should_rollback(this: &LinearVelocity, that: &LinearVelocity) -> bool {
    (this.0 - that.0).length() >= ROLLBACK_EPSILON
}

fn angular_velocity_should_rollback(this: &AngularVelocity, that: &AngularVelocity) -> bool {
    (this.0 - that.0).length() >= ROLLBACK_EPSILON
}
