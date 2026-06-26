//! Networking layer. Built on lightyear 0.26.4 for transport, replication, and interpolation.
//!
//! - [`protocol`]: which components replicate + which channels + which messages. Same on every peer.
//! - [`client`]: client-side plugin (transport connect).
//! - [`server`]: server-side plugin (transport listen, authority).
//!
//! Constants + the CLI arg parser below are copied from `wisp/src/net/mod.rs:25-104` (renaming the
//! `WISP_*` env prefix to `ARENA_*`), the authoritative working lightyear 0.26 reference.

pub mod client;
pub mod protocol;
pub mod server;

pub use client::ClientNetPlugin;
pub use protocol::ProtocolPlugin;
pub use server::ServerNetPlugin;

/// Default UDP port the server listens on. CLI flags can override.
pub const DEFAULT_PORT: u16 = 5000;

/// Fixed tick duration. 60 Hz — must match on both peers or the sims desync.
pub const TICK_HZ: u32 = 60;

/// Netcode protocol id. Bumped whenever the wire format changes incompatibly. Dev: 0.
pub const PROTOCOL_ID: u64 = 0;

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
