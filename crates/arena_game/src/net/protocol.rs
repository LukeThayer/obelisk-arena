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
        // `rebroadcast_inputs: true`: the server relays each client's inputs to the other clients,
        // which lightyear applies to the matching remote PREDICTED entity's InputBuffer
        // (`lightyear_inputs-0.26.4/src/client.rs:578 receive_remote_player_input_messages`). The
        // arena predicts ALL players (the avian_3d_character example's pattern) — the opponent's
        // body is simulated by the same shared controller from these rebroadcast inputs.
        app.add_plugins(input::native::InputPlugin::<ArenaInput> {
            config: input::InputConfig::<ArenaInput> {
                rebroadcast_inputs: true,
                ..default()
            },
        });

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

        // --- Skill objects (replicated world objects spawned by skills: portals, frost tiles,
        // spires — the wisp-port persistence layer). Plain replication (no prediction target);
        // pose rides the registered avian Position/Rotation like everything else. Clients build
        // visuals from `kind` (see client/skill_objects.rs).
        app.register_component::<NetworkedSkillObject>();

        // --- Equipped weapon (replicated per-player). Carries the obelisk item id + the skill
        // list it grants, in slot order: the input stream's `skill_slot` indexes THIS list on
        // both peers (the client's wheel/selection maps into it; the server's cast pipeline
        // resolves from its own copy). Discrete — plain registration, snapped on receive.
        app.register_component::<EquippedWeapon>();

        // --- Character appearance (replicated per-player; drives each rig's slot visibility). ---
        // Live edits are plain component updates — the receive path writes registered components
        // directly onto the (single) Predicted entity, and `SinceLastAck` resends unacked deltas.
        // (The old "component updates are unreliable" `CustomizeBroadcast` workaround was
        // inherited wisp lore — see the 2026-07-05 netcode spec.)
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
        // Rare reliable client→server requests (customization). Casts ride the input stream (WS2).
        app.add_channel::<RequestChannel>(ChannelSettings {
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
        app.register_message::<NetEventMessage>()
            .add_direction(NetworkDirection::ServerToClient); // wraps obelisk NetEvent (§5)
        app.register_message::<CueWireMessage>()
            .add_direction(NetworkDirection::ServerToClient); // wraps crate::net::cue::CueMessage (§4)
        app.register_message::<RoundStateMessage>()
            .add_direction(NetworkDirection::ServerToClient); // best-of-3 round flow (§7)

        // Live appearance change (D6): client→server request (reliable RequestChannel); the
        // server applies it to the player's `PlayerCustomization`, which replicates back to every
        // client as a plain component update.
        app.register_message::<CustomizeMessage>()
            .add_direction(NetworkDirection::ClientToServer);

        // Host's "start the match on <level>" request (reliable RequestChannel). The server
        // validates sender==host ∧ phase==Lobby ∧ 2 players ∧ level exists before acting.
        app.register_message::<StartMatchMessage>()
            .add_direction(NetworkDirection::ClientToServer);

        // Weapon equip request (reliable RequestChannel), sent from the lobby's I-key panel.
        // The server validates phase==Lobby ∧ the id exists in the weapon catalog.
        app.register_message::<EquipWeaponMessage>()
            .add_direction(NetworkDirection::ClientToServer);
    }
}

// ---------------------------------------------------------------------------------------------
// Channels (unit-struct tags; `Channel` is blanket-impl'd for any Send + Sync + 'static).
// ---------------------------------------------------------------------------------------------

/// Rare reliable client→server requests (customization). Casts ride the input stream (WS2).
pub struct RequestChannel;
/// Combat events + cues + round state, server→client, reliable.
pub struct EventChannel;

// ---------------------------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------------------------

/// Combat events on the wire — wraps obelisk's `NetEvent` so the server broadcasts it verbatim.
/// obelisk's `NetEvent` already uses STABLE STRING IDS (not `Entity`) — wire-ready.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetEventMessage(pub obelisk_bevy::net::NetEvent);

/// Cosmetic cue on the wire — wraps `crate::net::cue::CueMessage` (the serde wire type). Client
/// cue rendering is stubbed for now (C3 restores it via `bevy_effect`'s `CueBinding`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CueWireMessage(pub crate::net::cue::CueMessage);

/// Live appearance change, client→server, reliable (`RequestChannel`). Sent when the local player
/// finishes editing their costume (panel close). The server applies it to that player's
/// `PlayerCustomization` and re-broadcasts via [`CustomizeBroadcast`] (D6).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomizeMessage {
    pub parts: PartSelection,
}

/// Best-of-3 round flow (guide §7) + lobby/host state (levels-and-lobby design). The wire shape is
/// fixed here so the protocol checksum agrees on both peers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoundStateMessage {
    /// 0 Lobby, 1 Countdown, 2 Active, 3 RoundOver, 4 MatchOver.
    pub phase: u8,
    /// Seconds remaining on the phase timer (Countdown / RoundOver / MatchOver) — 0 otherwise.
    pub countdown: f32,
    /// (obelisk_id, round wins) for each of the two players.
    pub scores: [(String, u8); 2],
    /// The winner's obelisk_id for RoundOver / MatchOver, else empty.
    pub winner: String,
    /// Replicated session seed (forward-prep for Stage B; informational in Stage A).
    pub match_seed: u64,
    /// The elected host's client id (first joiner still connected) — 0 while no host exists. The
    /// matching client shows the "press G" level-select affordance.
    pub host: u64,
    /// The currently-loaded level id (catalog stem, e.g. "lobby", "arena_flat"). Clients load the
    /// matching `.scn.ron` locally on change.
    pub level: String,
}

/// Host request: start the PvP match on `level` (a `LevelCatalog` id). Client→server, reliable
/// (`RequestChannel`), sent from the G-key level-select panel. The server ignores it from anyone
/// but the elected host, outside the Lobby phase, below 2 players, or naming an unknown level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartMatchMessage {
    pub level: String,
}

/// Equip request: swap this player's weapon to `item_id` (a `WeaponCatalog` id). Client→server,
/// reliable (`RequestChannel`), sent from the lobby's I-key weapon panel. The server ignores it
/// outside the Lobby phase or for an unknown id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquipWeaponMessage {
    pub item_id: String,
}

/// The player's equipped weapon, replicated to every client: the obelisk item id (display/HUD
/// resolves the name via the local `WeaponCatalog`) + the skills it grants IN SLOT ORDER — the
/// single source of truth both peers resolve `ArenaInput.skill_slot` against. Server-written on
/// spawn (the starter weapon) and on an accepted `EquipWeaponMessage`.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EquippedWeapon {
    pub item_id: String,
    pub skills: Vec<String>,
}

/// A replicated skill-spawned WORLD OBJECT (the wisp-port persistence layer): portals, frost
/// tiles, frost spires. `kind` keys everything — server limits/behavior
/// (`server/skill_objects.rs`) and the client visual recipe (`client/skill_objects.rs`).
/// `owner` = the spawning client id (0 = none). Pose rides the replicated avian
/// `Position`/`Rotation`.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkedSkillObject {
    pub kind: String,
    pub owner: u64,
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
