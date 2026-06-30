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
use lightyear::prelude::client::{Client, ClientPlugins, Connect, NetcodeClient, NetcodeConfig};
use lightyear::prelude::{
    Connected, Link, LocalAddr, LocalId, PeerAddr, PredictionManager, ReplicationReceiver, UdpIo,
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
            Link::new(None),
            ReplicationReceiver::default(),
            PredictionManager::default(),
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
