//! Client-side network plugin. Layers lightyear's `ClientPlugins` + the shared `ProtocolPlugin`,
//! spawns a netcode client entity that connects to `ConnectTo.server`, and logs the connection.
//!
//! `spawn_client` + `ConnectTo` + `on_connected` are copied from `wisp/src/net/client.rs:75-108`
//! (minus wisp's bei/customization send paths); the `ClientPlugins { tick_duration }` shape +
//! imports are verified against the installed lightyear 0.26.4 source.

use core::time::Duration;
use std::net::SocketAddr;

use bevy::prelude::*;
use lightyear::netcode::prelude::Authentication;
use lightyear::prelude::client::{
    Client, ClientPlugins, Connect, InputDelayConfig, NetcodeClient, NetcodeConfig,
};
use lightyear::prelude::{
    Connected, InputTimelineConfig, Link, LocalAddr, LocalId, PeerAddr, PredictionManager,
    ReplicationReceiver, SyncConfig, UdpIo,
};
use serde_json::json;

use crate::net::{default_server_addr, ProtocolPlugin, NETCODE_KEY, PROTOCOL_ID, TICK_HZ};
use crate::trace;

/// Plugin: client-side lightyear net stack — `ClientPlugins` + `ProtocolPlugin` + trace, and spawns
/// the netcode client entity with its `ConnectTo` target.
pub struct ClientNetPlugin;

impl Plugin for ClientNetPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ClientPlugins {
            tick_duration: Duration::from_secs_f32(1.0 / TICK_HZ as f32),
        })
        .add_plugins(ProtocolPlugin)
        .add_plugins(trace::TracePlugin)
        .insert_resource(ConnectTo {
            server: default_server_addr(),
            // `ARENA_CLIENT_ID` lets a test harness pin the id so the server-side spawn position is
            // deterministic. Real users get the wall-clock-nanos pseudo-unique id.
            client_id: std::env::var("ARENA_CLIENT_ID")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(pseudo_unique_client_id),
        })
        .add_systems(Startup, spawn_client)
        .add_observer(on_connected);
    }
}

/// Install the client net stack and point it at the configured server: add [`ClientNetPlugin`]
/// (which inserts the default [`ConnectTo`] carrying the env-derived `client_id`), then re-apply
/// `ConnectTo` with the address from `parse_addr_args` (CLI `--ip`/`--port`, defaulting to
/// `default_server_addr()`) while preserving that `client_id`. Shared by `run_windowed_client` and
/// `run_headless_client` so both peers connect identically.
pub fn connect_to_configured(app: &mut App) {
    app.add_plugins(ClientNetPlugin);
    let server = crate::net::parse_addr_args(default_server_addr());
    let client_id = app.world().resource::<ConnectTo>().client_id;
    app.insert_resource(ConnectTo { server, client_id });
}

/// The connection target. Overridable by the bin (CLI `--ip`/`--port`) before `spawn_client` runs.
#[derive(Resource, Clone)]
pub struct ConnectTo {
    pub server: SocketAddr,
    pub client_id: u64,
}

/// Derive a per-process client id from wall-clock nanoseconds so two clients on the same machine
/// don't collide. Dev/test only; production would receive its id from an auth service.
fn pseudo_unique_client_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Optional artificial-latency conditioner (design WS6) so netcode feel/regressions are tested at
/// real RTTs instead of localhost-zero: `ARENA_NET_LATENCY_MS` (one-way incoming delay),
/// `ARENA_NET_JITTER_MS`, `ARENA_NET_LOSS` (0..1 drop probability). Applied to the client's
/// RECEIVE path only — run both observers with latency L to simulate a symmetric ~2·L RTT.
/// Zero-cost when unset (`Link::new(None)`, the pre-existing shape).
fn link_conditioner_from_env() -> Option<lightyear::prelude::RecvLinkConditioner> {
    let ms = |k: &str| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
    };
    let latency = ms("ARENA_NET_LATENCY_MS");
    let jitter = ms("ARENA_NET_JITTER_MS");
    let loss = std::env::var("ARENA_NET_LOSS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok());
    if latency.is_none() && jitter.is_none() && loss.is_none() {
        return None;
    }
    Some(lightyear::prelude::RecvLinkConditioner::new(
        lightyear::prelude::LinkConditionerConfig {
            incoming_latency: Duration::from_millis(latency.unwrap_or(0)),
            incoming_jitter: Duration::from_millis(jitter.unwrap_or(0)),
            incoming_loss: loss.unwrap_or(0.0),
        },
    ))
}

fn spawn_client(mut commands: Commands, target: Res<ConnectTo>) {
    let auth = Authentication::Manual {
        server_addr: target.server,
        client_id: target.client_id,
        private_key: NETCODE_KEY,
        protocol_id: PROTOCOL_ID,
    };
    let client = match NetcodeClient::new(auth, NetcodeConfig::default()) {
        Ok(c) => c,
        Err(err) => {
            error!("Failed to build NetcodeClient: {err:?}");
            return;
        }
    };
    info!(
        "Connecting to server {} as client_id={}…",
        target.server, target.client_id
    );
    trace::event(
        "connecting",
        json!({ "server": target.server.to_string(), "client_id": target.client_id }),
    );
    // Bind to 0.0.0.0:0 so the OS picks an ephemeral local port — required by lightyear's UdpIo
    // link even client-side. Two clients on the same machine therefore get distinct local ports.
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    let local_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
    let entity = commands
        .spawn((
            // `Client` + `Link` + `PredictionManager` are required by the canonical lightyear client
            // setup (simple_setup/common client.rs): `PredictionManager` is what creates the local
            // `Predicted` entity for an owned, prediction-targeted player.
            Client::default(),
            client,
            UdpIo::default(),
            LocalAddr(local_addr),
            PeerAddr(target.server),
            Link::new(link_conditioner_from_env()),
            ReplicationReceiver::default(),
            // Explicit (== default) policies, named so the tuning surface is visible: rollback on
            // confirmed-state mismatch per the protocol's 0.01-epsilon comparators; smooth the
            // post-rollback visual error exponentially (50% per 200ms; teleport-scale errors are
            // snapped by `client::harness::snap_large_corrections`). Tune `correction_policy`'s
            // decay downward if remote-input mispredicts feel too floaty under the conditioner.
            PredictionManager {
                rollback_policy: lightyear::prelude::RollbackPolicy::default(),
                correction_policy: lightyear::prediction::correction::CorrectionPolicy::default(),
                ..default()
            },
            // Input-timeline ahead-margin. The sync objective is `server + rtt/2 + jitter_margin -
            // input_delay` (lightyear_sync input.rs::sync_objective); the default 5ms margin leaves
            // the client only ~⅓ tick ahead at LAN/localhost RTT, so input messages for tick T can
            // arrive AFTER the server simulated T — and the server-side buffer read (`get(tick)`,
            // no fallback in 0.26.4) then NEVER applies them (observed: player frozen with
            // `rebroadcast_inputs` enabled). 25ms keeps the client ≥1.5 ticks ahead. This is NOT
            // local input delay (input_delay stays 0) — the client just simulates further ahead;
            // own-input feel is unchanged.
            InputTimelineConfig::new(
                SyncConfig {
                    jitter_margin: Duration::from_millis(25),
                    ..Default::default()
                },
                InputDelayConfig::no_input_delay(),
            ),
        ))
        .id();
    commands.trigger(Connect { entity });
}

fn on_connected(
    trigger: On<Add, Connected>,
    local_ids: Query<&LocalId, Without<lightyear::prelude::server::ClientOf>>,
) {
    if let Ok(LocalId(peer_id)) = local_ids.get(trigger.entity) {
        info!("Connected to server: client peer_id={:?}", peer_id);
        trace::event("connected", json!({ "peer_id": format!("{:?}", peer_id) }));
    }
}
