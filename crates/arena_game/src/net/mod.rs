//! Networking layer. Built on lightyear 0.26.4 for transport, replication, and interpolation.
//!
//! - [`protocol`]: which components replicate + which channels + which messages. Same on every peer.
//! - [`client`]: client-side plugin (transport connect).
//! - [`server`]: server-side plugin (transport listen, authority).
//!
//! Constants + the CLI arg parser below are copied from `wisp/src/net/mod.rs:25-104` (renaming the
//! `WISP_*` env prefix to `ARENA_*`), the authoritative working lightyear 0.26 reference.

pub mod client;
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

/// Fixed tick duration. 60 Hz — must match on both peers or the sims desync. The (slower) 10 Hz
/// replication send rate lives next to its use site as `server::REPLICATION_SEND_HZ`; the rollback
/// divergence threshold as `protocol::ROLLBACK_EPSILON`.
pub const TICK_HZ: u32 = 60;

/// Netcode protocol id. Bumped whenever the wire format changes incompatibly. Bumped to 1 for the
/// lightyear-native prediction wire (native input + avian Position/Rotation replication, dropping
/// the `PlayerInputMessage`/`NetworkedPosition` streams).
pub const PROTOCOL_ID: u64 = 1;

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
}
