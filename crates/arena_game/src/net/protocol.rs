//! Protocol: identical on every peer. Declares which components replicate, which channels carry
//! which messages, and the message payloads. Shared by server + client (netcode guide §3).
//!
//! API shapes copied from `wisp/src/net/protocol.rs` and verified against the installed lightyear
//! 0.26.4 source (`~/.cargo/registry/.../lightyear_transport-0.26.4/src/channel/builder.rs`,
//! `lightyear_messages`, `lightyear_replication`, and the canonical
//! `lightyear-0.26.4/src/protocol.rs` example). Notable divergence from the guide §3 table:
//! `ChannelMode::UnorderedReliable` takes a `ReliableSettings` argument (the guide wrote it
//! bare) — confirmed against the registry source + the canonical example.

use avian3d::prelude::{LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use core::time::Duration;
use lightyear::interpolation::registry::InterpolationRegistrationExt;
use lightyear::prediction::registry::PredictionRegistrationExt;
use lightyear::prelude::{
    AppChannelExt, AppComponentExt, AppMessageExt, ChannelMode, ChannelSettings,
    ComponentReplicationConfig, NetworkDirection, ReliableSettings,
};
use serde::{Deserialize, Serialize};

pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // --- Player identity / ownership (replicated on spawn) ---
        app.register_component::<NetworkedPlayer>();
        app.register_component::<NetworkOwner>(); // NetworkOwner(u64) == client_id (PeerId)
        app.register_component::<NetworkedId>(); // NetworkedId(u64) monotonic, cross-peer stable
        app.register_component::<ObeliskNetId>(); // wraps ObeliskId String (stable combat id)

        // --- Pose (the per-tick stream). Hand-rolled because Transform doesn't serde. ---
        // Carries obelisk cast state for remote cast animation (spec §7). The interpolation lerp
        // runs every frame and is allocation-free.
        app.register_component::<NetworkedPosition>()
            .add_interpolation_with(lerp_networked_position);

        // --- Health (replicated; HUD source of truth, server-authoritative). ---
        // No interpolation: hp is discrete and damage feedback feels best snapping.
        app.register_component::<NetworkedHealth>();

        // --- avian physics for the PREDICTED local player (Stage-A movement, M2.2). ---
        // Registered-but-DISABLED by default (`disable: true`); M2.2 opts the local player in
        // per-entity. Rollback thresholds match the avian_3d_character reference (0.01 each).
        let disable = ComponentReplicationConfig {
            disable: true,
            ..default()
        };
        app.register_component::<Position>()
            .with_replication_config(disable.clone())
            .add_prediction()
            .add_should_rollback(position_should_rollback)
            .add_linear_interpolation();
        app.register_component::<Rotation>()
            .with_replication_config(disable.clone())
            .add_prediction()
            .add_should_rollback(rotation_should_rollback)
            .add_linear_interpolation();
        // NOTE: no `.add_linear_interpolation()` on LinearVelocity — avian's `LinearVelocity`
        // doesn't impl `Ease` / `VectorSpace<Scalar = f32>`, so the linear-interp registration
        // fails to compile (verified: E0277 `LinearVelocity: Ease` not satisfied). wisp omits it
        // here for the same reason (wisp/src/net/protocol.rs:113-116). Prediction + rollback still
        // apply; velocity just isn't interpolated for remote replicas (it drives prediction only).
        app.register_component::<LinearVelocity>()
            .with_replication_config(disable)
            .add_prediction()
            .add_should_rollback(velocity_should_rollback);

        // --- Channels ---
        // Movement: latest-wins, per-tick. Unreliable.
        app.add_channel::<InputChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            send_frequency: Duration::default(),
            priority: 1.0,
        })
        .add_direction(NetworkDirection::ClientToServer);

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
        app.register_message::<PlayerInputMessage>()
            .add_direction(NetworkDirection::ClientToServer); // movement, on InputChannel
        app.register_message::<CastRequestMessage>()
            .add_direction(NetworkDirection::ClientToServer); // cast, on CastChannel
        app.register_message::<NetEventMessage>()
            .add_direction(NetworkDirection::ServerToClient); // wraps obelisk NetEvent (§5)
        app.register_message::<CueWireMessage>()
            .add_direction(NetworkDirection::ServerToClient); // wraps arena_skills CueMessage (§4)
        app.register_message::<RoundStateMessage>()
            .add_direction(NetworkDirection::ServerToClient); // best-of-3 round flow (§7)
    }
}

// ---------------------------------------------------------------------------------------------
// Channels (unit-struct tags; `Channel` is blanket-impl'd for any Send + Sync + 'static).
// ---------------------------------------------------------------------------------------------

/// Movement input, client→server, unreliable (latest wins).
pub struct InputChannel;
/// Cast requests, client→server, reliable (a dropped cast is unacceptable).
pub struct CastChannel;
/// Combat events + cues + round state, server→client, reliable.
pub struct EventChannel;

// ---------------------------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------------------------

/// Per-tick movement input from a client to the server. Server runs its authoritative controller
/// against this and ships the resulting pose back via `NetworkedPosition` (Stage-A option b).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerInputMessage {
    /// Camera-relative WASD axis.
    pub movement: [f32; 2],
    /// Camera yaw (radians).
    pub yaw: f32,
    /// Aim pitch (spine lean; cosmetic).
    pub pitch: f32,
    pub jump: bool,
}

/// Cast request — replaces M1's direct `cast_skill_at` on the local entity (spec §5.2). The server
/// fires along `aim_dir` (the client's camera forward vector) via `cast_skill_dir_charged` — free
/// aim, no auto-acquire. The server re-validates caster validity + already-casting; obelisk's
/// `validate_casts` gates mana/cooldown/already-casting.
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

/// Cosmetic cue on the wire — wraps `arena_skills::CueMessage` (the serde wire type from M2.0). The
/// client consumer re-looks-up the `LaneEvent`s via the `SkillFxRegistry` (§4.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CueWireMessage(pub arena_skills::CueMessage);

/// Best-of-3 round flow (§7). Filled out by M2.4 Task 19; the wire shape is fixed here so the
/// protocol checksum agrees on both peers from M2.1.
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

/// Serializable world pose + obelisk cast state. Bevy's `Transform` doesn't serde, so the protocol
/// carries this wrapper and a client system copies it onto Transform. Player bodies rotate only
/// around Y, so a single yaw scalar suffices. Cast fields drive remote cast animation (spec §7).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkedPosition {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub airborne: bool,
    /// 0 none, 1 windup, 2 active, 3 recovery.
    pub cast_phase: u8,
    pub cast_elapsed: f32,
    /// Interned skill index (skill_id table) or 0 = none.
    pub cast_skill: u16,
}

impl NetworkedPosition {
    pub fn from_vec3(v: Vec3) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
            ..default()
        }
    }
    pub fn to_vec3(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

// ---------------------------------------------------------------------------------------------
// Interpolation + rollback helpers (every-frame, allocation-free).
// ---------------------------------------------------------------------------------------------

/// Linear interpolation between two `NetworkedPosition` snapshots. Yaw uses shortest-arc wrap so a
/// player turning +179° → -179° interpolates +2°, not -358°. Discrete cast phase snaps at t>=0.5.
/// `t` is clamped to `[0, 1]` (late packets can push it past 1 — guide trap).
fn lerp_networked_position(
    start: NetworkedPosition,
    end: NetworkedPosition,
    t: f32,
) -> NetworkedPosition {
    use core::f32::consts::PI;
    let t = t.clamp(0.0, 1.0);
    let mut dyaw = end.yaw - start.yaw;
    if dyaw > PI {
        dyaw -= 2.0 * PI;
    } else if dyaw < -PI {
        dyaw += 2.0 * PI;
    }
    NetworkedPosition {
        x: start.x + (end.x - start.x) * t,
        y: start.y + (end.y - start.y) * t,
        z: start.z + (end.z - start.z) * t,
        yaw: start.yaw + dyaw * t,
        pitch: start.pitch + (end.pitch - start.pitch) * t,
        airborne: if t >= 0.5 {
            end.airborne
        } else {
            start.airborne
        },
        // Discrete cast phase snaps at the midpoint; elapsed lerps.
        cast_phase: if t >= 0.5 {
            end.cast_phase
        } else {
            start.cast_phase
        },
        cast_elapsed: start.cast_elapsed + (end.cast_elapsed - start.cast_elapsed) * t,
        cast_skill: if t >= 0.5 {
            end.cast_skill
        } else {
            start.cast_skill
        },
    }
}

/// Rollback thresholds for the avian physics components, matched to the avian_3d_character
/// reference example (0.01 epsilons). Below these we let prediction stand (avoids float-noise
/// jitter). NOT reflexive: comparing a value to itself returns false (no rollback), which is
/// correct — only a real divergence >= 0.01 triggers a rollback (guide §1.2 trap).
fn position_should_rollback(this: &Position, that: &Position) -> bool {
    (this.0 - that.0).length() >= 0.01
}

fn rotation_should_rollback(this: &Rotation, that: &Rotation) -> bool {
    this.angle_between(*that) >= 0.01
}

fn velocity_should_rollback(this: &LinearVelocity, that: &LinearVelocity) -> bool {
    (this.0 - that.0).length() >= 0.01
}
