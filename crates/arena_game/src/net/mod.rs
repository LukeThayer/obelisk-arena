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

/// Default UDP port the server listens on. CLI flags can override.
pub const DEFAULT_PORT: u16 = 5000;

/// Fixed tick duration. 60 Hz — must match on both peers or the sims desync.
pub const TICK_HZ: u32 = 60;

/// Camera eye height above the player root (world units). Shared by BOTH the first-person camera
/// placement (`client::controller::EYE_HEIGHT`) and the server muzzle offset
/// (`server::drain_cast_requests`). Because the camera sits at `origin + Y*ARENA_EYE_HEIGHT` and the
/// firebolt now spawns at `origin + Y*ARENA_EYE_HEIGHT`, the bolt originates at the eye and travels
/// along `aim_dir` (camera forward) — so the crosshair ray IS the bolt path and a shot aimed at the
/// opponent connects. Defined once here so the two values can never drift apart.
///
/// MEASURED geometry (the `character.glb` body AABB, feet-rooted): the model is ~1.18 tall, so with
/// the player origin at the BODY CENTER (see `GROUND_Y` / `present::RIG_FOOT_OFFSET`) the feet sit at
/// origin Y−0.59 and the head at origin Y+0.59. `+0.5` puts the eye just below the top of the head =
/// natural first-person eye level. The HURTBOX capsule (`server::sync_networked_players`) spans origin
/// Y±0.59 = feet→head, so the eye-height muzzle fires from inside the body span and a level shot lands.
pub const ARENA_EYE_HEIGHT: f32 = 0.5;

/// The stand height: world Y of a grounded player ORIGIN. The Dynamic body is `capsule(0.35, 0.48)`
/// (half-height 0.59) resting on the static arena floor (top at world 0), so its origin settles at
/// ≈0.59 and its feet at world 0. Used by the shared controller's ground check
/// (`shared_controller::apply_arena_movement`) — a player meaningfully above this is airborne — and
/// as the Y of `server::SPAWN_MARKERS`. With a real floor collider this is the EMERGENT rest height,
/// not a clamp.
pub const GROUND_Y: f32 = 0.59;

/// Magnitude of the arena's avian `Gravity` (m/s²), set in `add_avian_with_lightyear`. Snappier than
/// Earth for an arcade jump arc; with `shared_controller::JUMP_SPEED` (7 m/s) the apex is ≈1.22 m.
pub const GRAVITY: f32 = 20.0;

/// Netcode protocol id. Bumped whenever the wire format changes incompatibly. Bumped to 1 for the
/// lightyear-native prediction wire (native input + avian Position/Rotation replication, dropping
/// the `PlayerInputMessage`/`NetworkedPosition` streams).
pub const PROTOCOL_ID: u64 = 1;

/// Shared netcode private key. Dev/test only — a real deployment holds the key server-side and
/// hands clients a signed `ConnectToken` from a backend auth service.
pub const NETCODE_KEY: [u8; 32] = [0u8; 32];

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
