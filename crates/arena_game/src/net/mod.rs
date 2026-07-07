//! Networking layer. Built on lightyear 0.26.4 for transport, replication, and interpolation.
//!
//! - [`protocol`]: which components replicate + which channels + which messages. Same on every peer.
//! - [`client`]: client-side plugin (transport connect).
//! - [`server`]: server-side plugin (transport listen, authority).
//!
//! Constants + the CLI arg parser below are copied from `wisp/src/net/mod.rs:25-104` (renaming the
//! `WISP_*` env prefix to `ARENA_*`), the authoritative working lightyear 0.26 reference.

pub mod client;
pub mod cue;
pub mod input;
pub mod protocol;
pub mod server;

pub use client::ClientNetPlugin;
pub use protocol::ProtocolPlugin;
pub use server::ServerNetPlugin;

// =============================================================================================
// Gameplay tuning surface — the de-facto home for cross-peer "feel" constants.
//
// Every value here is BYTE-IDENTICAL to the pre-centralization scattered magic numbers; this
// module only NAMES + DOCUMENTS them. Domain constants that MUST live next to their use site
// (rollback epsilon, replication rate, the client charge/camera/rig knobs) are cross-referenced
// from the relevant sub-section below rather than duplicated here.
// =============================================================================================

// --- movement ---
// The shared force-controller tunables live in `shared_controller`; the planar trio is re-exported
// here so it sits beside `GRAVITY` (the arc is integrated against) and `GROUND_Y` (the rest height
// the ground check reads). `shared_controller::AIR_CONTROL` scales acceleration while airborne.
pub use crate::shared_controller::{JUMP_SPEED, MAX_ACCELERATION, MAX_SPEED};

// The geometry/physics tuning constants (`GROUND_Y`, `GRAVITY`, `PLAYER_CAPSULE_RADIUS`,
// `PLAYER_CAPSULE_LENGTH`, `ARENA_EYE_HEIGHT`) now live in `arena_sim::tuning` (the single source
// of truth shared with the editor preview); re-exported here so existing `crate::net::*` use sites
// resolve unchanged. The full doc rationale lives on each const in `arena_sim/src/tuning.rs`.
pub use arena_sim::tuning::{
    ARENA_EYE_HEIGHT, GRAVITY, GROUND_Y, PLAYER_CAPSULE_LENGTH, PLAYER_CAPSULE_RADIUS,
};

// --- camera ---

// `ARENA_EYE_HEIGHT` (camera eye height / server muzzle offset) lives in `arena_sim::tuning` and is
// re-exported above with the other geometry constants.
// Mouse sensitivity (default 0.0035, env `ARENA_MOUSE_SENS`) + the pitch clamp live in
// `client::controller` next to the mouse-look system that reads them.

// --- charge ---
// `client::net::MAX_CHARGE_SECS` + `TAP_CHARGE_BYTE` + the `charge_byte_from_frac`/`charge_mult`
// helpers live in `client::net` next to the cast-charge state they tune.

// --- netcode feel ---

/// Default UDP port the server listens on. CLI flags can override.
pub const DEFAULT_PORT: u16 = 5000;

/// Fixed tick duration. 60 Hz — must match on both peers or the sims desync. The (slower) 30 Hz
/// replication send rate lives next to its use site as `server::REPLICATION_SEND_HZ`; the rollback
/// divergence threshold as `protocol::ROLLBACK_EPSILON`.
pub const TICK_HZ: u32 = 60;

// --- skills / casting (design WS2: cast intent rides the input stream) ---

/// The grantable skill roster, in SLOT ORDER. The single source of truth for: the server grant
/// loop (`server/spawn.rs`), the windowed number-key select (`skill_for_key`), and the
/// `ArenaInput.skill_slot` ↔ skill-id mapping on both peers. Index == wire slot.
pub const ARENA_SKILLS: [&str; 3] = ["firebolt", "chain_lightning", "blizzard"];

/// Slot index for a skill id (positional in [`ARENA_SKILLS`]); `None` for unknown ids.
pub fn skill_slot_for(id: &str) -> Option<u8> {
    ARENA_SKILLS.iter().position(|s| *s == id).map(|i| i as u8)
}

/// The camera-forward aim ray both peers reconstruct from the input's `yaw`+`pitch`:
/// `Quat(Y,yaw) * Quat(X,pitch) * -Z`, normalized. Matches the windowed camera
/// (`controller::follow_local_net_player`) exactly, so the bolt flies where the crosshair looks.
pub fn aim_dir(yaw: f32, pitch: f32) -> bevy::prelude::Vec3 {
    use bevy::prelude::{Quat, Vec3};
    let rot = Quat::from_axis_angle(Vec3::Y, yaw) * Quat::from_axis_angle(Vec3::X, pitch);
    (rot * -Vec3::Z).normalize()
}

// --- charge (moved here from `client::net` — the server now computes the byte too) ---

/// Maximum hold time for the charge mechanic. A full hold of this duration maps to charge=255
/// (2.0× multiplier); an instant tap maps to [`TAP_CHARGE_BYTE`] (≈1.0×).
pub const MAX_CHARGE_SECS: f32 = 1.5;

/// Charge byte for an instant tap: ≈ one-third up the 0-255 range, which `charge_mult` maps to
/// ≈1.0×. A full hold sends 255 (→2.0×).
pub const TAP_CHARGE_BYTE: u8 = 85;

/// Map a hold fraction `[0, 1]` to a charge byte: tap → [`TAP_CHARGE_BYTE`], full hold → 255.
pub fn charge_byte_from_frac(frac: f32) -> u8 {
    let span = 255.0 - TAP_CHARGE_BYTE as f32;
    (TAP_CHARGE_BYTE as f32 + frac * span)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// The charge → damage/speed multiplier mapping (reference; obelisk owns the authoritative
/// scaling): `0.5 + (c/255) * 1.5`, so tap ≈ 1.0× and 255 = 2.0×.
pub fn charge_mult(byte: u8) -> f32 {
    0.5 + (byte as f32 / 255.0) * 1.5
}

/// Server-side charge derivation (design WS2): held-tick count → charge byte, via the same
/// `charge_byte_from_frac` the client HUD uses on its local hold time — both peers derive the
/// identical value from the identical input stream. Saturates at MAX_CHARGE_SECS worth of ticks.
pub fn charge_byte_from_hold_ticks(hold_ticks: u32) -> u8 {
    let full = MAX_CHARGE_SECS * TICK_HZ as f32;
    charge_byte_from_frac(((hold_ticks as f32 - 1.0) / (full - 1.0)).clamp(0.0, 1.0))
}

/// Netcode protocol id. Bumped whenever the wire format changes incompatibly. Bumped to 3 for the
/// levels-and-lobby wire (RoundStateMessage v2 with host+level, StartMatchMessage, phase tag 0 now
/// Lobby). 2 was the input-carried cast wire (ArenaInput v2 with pitch/skill_slot).
pub const PROTOCOL_ID: u64 = 3;

/// Shared netcode private key. Dev/test only — a real deployment holds the key server-side and
/// hands clients a signed `ConnectToken` from a backend auth service.
pub const NETCODE_KEY: [u8; 32] = [0u8; 32];

// --- visual offsets ---
// `present::RIG_FOOT_OFFSET`, `cosmetics::MUZZLE_HEIGHT_OFFSET`, and `rig::LOCOMOTION_REF_SPEED`
// are client-presentation-only; they live next to the rig/cosmetic systems that consume them.

// --- match pacing ---

/// Rounds needed to win the match (best-of-3 ⇒ first to 2).
pub const ROUND_WINS_TO_MATCH: u8 = 2;
/// Pre-round countdown length (seconds).
pub const COUNTDOWN_SECS: f32 = 3.0;
/// Pause between a round ending and the next countdown (seconds), so the result is readable.
pub const ROUND_OVER_SECS: f32 = 2.0;
/// How long the MatchOver banner holds before everyone returns to the lobby (seconds).
pub const MATCH_OVER_SECS: f32 = 6.0;

/// Default address the server binds to and clients connect to.
pub fn default_server_addr() -> std::net::SocketAddr {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_PORT)
}

/// Parse `--ip <addr>` and `--port <num>` from `std::env::args()`, returning the resulting
/// `SocketAddr`. Either flag is optional; missing fields fall back to `default`. Invalid values are
/// logged to stderr and ignored (keeps the default for that field). Server bin uses the result as
/// its bind addr; client bin uses it as the connect target.
pub fn parse_addr_args(default: std::net::SocketAddr) -> std::net::SocketAddr {
    let mut ip = default.ip();
    let mut port = default.port();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--ip" => {
                if let Some(v) = args.get(i + 1) {
                    match v.parse() {
                        Ok(parsed) => ip = parsed,
                        Err(e) => eprintln!("--ip {v:?}: {e}; keeping {ip}"),
                    }
                    i += 2;
                    continue;
                } else {
                    eprintln!("--ip requires a value");
                }
            }
            "--port" => {
                if let Some(v) = args.get(i + 1) {
                    match v.parse() {
                        Ok(parsed) => port = parsed,
                        Err(e) => eprintln!("--port {v:?}: {e}; keeping {port}"),
                    }
                    i += 2;
                    continue;
                } else {
                    eprintln!("--port requires a value");
                }
            }
            _ => {}
        }
        i += 1;
    }
    std::net::SocketAddr::new(ip, port)
}

/// The session/match seed used to seed the server's `CombatRng`. Env `ARENA_MATCH_SEED` pins it for
/// deterministic tests; otherwise wall-clock nanos. Replicated to clients (M2.4 round state) so a
/// future Stage-B (M3) predicted-damage path can reproduce rolls; in Stage A the server is the sole
/// RNG consumer and this is forward-prep only.
pub fn session_seed() -> u64 {
    std::env::var("ARENA_MATCH_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the jump-arc apex RELATIONSHIP `JUMP_SPEED² / (2·GRAVITY)` (not a value): 7² / (2·20) =
    /// 49/40 = 1.225 m, the ≈1.22 m the `GRAVITY` doc cites. Documents WHY the two values pair as
    /// they do without changing either. (`GRAVITY` is the magnitude; the avian `Gravity` is −GRAVITY·Y.)
    #[test]
    fn jump_apex_matches_documented_height() {
        let apex = JUMP_SPEED * JUMP_SPEED / (2.0 * GRAVITY);
        assert!((apex - 1.225).abs() < 1e-4, "apex={apex}");
    }

    /// `aim_dir` must equal the camera math the windowed client uses:
    /// `Quat(Y,yaw) * Quat(X,pitch) * -Z`, normalized.
    #[test]
    fn aim_dir_matches_camera_forward() {
        use bevy::prelude::{Quat, Vec3};
        for (yaw, pitch) in [(0.0f32, 0.0f32), (-1.5707963, 0.2), (2.5, -0.7)] {
            let rot = Quat::from_axis_angle(Vec3::Y, yaw) * Quat::from_axis_angle(Vec3::X, pitch);
            let expect = (rot * -Vec3::Z).normalize();
            assert!((aim_dir(yaw, pitch) - expect).length() < 1e-5);
        }
    }

    /// Slot mapping is the positional index into ARENA_SKILLS; unknown ids have no slot.
    #[test]
    fn skill_slots_are_positional() {
        assert_eq!(skill_slot_for("firebolt"), Some(0));
        assert_eq!(skill_slot_for("chain_lightning"), Some(1));
        assert_eq!(skill_slot_for("blizzard"), Some(2));
        assert_eq!(skill_slot_for("nope"), None);
        assert_eq!(ARENA_SKILLS.len(), 3);
    }

    /// Hold-tick charge anchors: an instant tap (1 tick) ≈ TAP_CHARGE_BYTE (≈1.0×); holding
    /// MAX_CHARGE_SECS worth of ticks (or longer) = 255 (2.0×).
    #[test]
    fn charge_from_hold_ticks_anchors() {
        let full = (MAX_CHARGE_SECS * TICK_HZ as f32).ceil() as u32;
        assert!(charge_byte_from_hold_ticks(1).abs_diff(TAP_CHARGE_BYTE) <= 2);
        assert_eq!(charge_byte_from_hold_ticks(full), 255);
        assert_eq!(charge_byte_from_hold_ticks(full * 3), 255);
    }
}
