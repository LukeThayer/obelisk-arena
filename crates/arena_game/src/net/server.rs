//! Server-side network plugin. Layers lightyear's `ServerPlugins` + the shared `ProtocolPlugin`,
//! spawns a netcode server entity bound to `ServerBind`, attaches a `ReplicationSender` to each new
//! client link, and logs connections.
//!
//! `spawn_server` + `on_new_link` are adapted from `wisp/src/net/server.rs:154-176` (diverges:
//! triggers `Start` not `LinkStart`, and attaches an explicit
//! `ReplicationSender::new(100ms, SinceLastAck, false)`); the imports + `ServerPlugins { tick_duration }`
//! shape are verified against the installed lightyear 0.26.4 source (`lightyear-0.26.4/src/server.rs`).

use core::time::Duration;
use std::net::SocketAddr;

use bevy::prelude::*;
use lightyear::prelude::server::{
    ClientOf, NetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};
use lightyear::prelude::{
    Connected, LinkOf, LocalAddr, PeerId, RemoteId, ReplicationSender, SendUpdatesMode,
};
use serde_json::json;

use crate::net::{default_server_addr, ProtocolPlugin, NETCODE_KEY, PROTOCOL_ID, TICK_HZ};
use crate::trace;

/// Plugin: server-side lightyear net stack — `ServerPlugins` + `ProtocolPlugin` + trace, spawns the
/// netcode server entity bound to `ServerBind`, and attaches a `ReplicationSender` per client link.
pub struct ServerNetPlugin;

impl Plugin for ServerNetPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ServerPlugins {
            tick_duration: Duration::from_secs_f32(1.0 / TICK_HZ as f32),
        })
        .add_plugins(ProtocolPlugin)
        .add_plugins(trace::TracePlugin)
        .insert_resource(ServerBind {
            addr: default_server_addr(),
        })
        .add_systems(Startup, spawn_server)
        .add_observer(on_new_link)
        .add_observer(on_client_connected);
    }
}

/// The address the server binds to. Overridable by the bin via CLI (`--ip`/`--port`) before
/// `spawn_server` runs at Startup.
#[derive(Resource, Clone)]
pub struct ServerBind {
    pub addr: SocketAddr,
}

fn spawn_server(mut commands: Commands, bind: Res<ServerBind>) {
    let config = NetcodeConfig::default()
        .with_protocol_id(PROTOCOL_ID)
        .with_key(NETCODE_KEY);
    info!("arena server listening on {}", bind.addr);
    trace::event("server_listening", json!({ "addr": bind.addr.to_string() }));
    let entity = commands
        .spawn((
            NetcodeServer::new(config),
            ServerUdpIo::default(),
            LocalAddr(bind.addr),
        ))
        .id();
    // `Start` (NOT `LinkStart`): `Start` triggers `LinkStart` for us AND adds the `Started`
    // component to the server entity. `Replicate::on_insert` HARD-REQUIRES `With<Started>` (verified
    // in lightyear_replication components.rs:888) to register a newly-spawned entity to ALREADY-
    // connected clients; with only `LinkStart` that path bails, so an entity reaches only clients
    // that connect AFTER it spawns — the first player never replicates to its own (first) client.
    // (The old `LinkStart` was copied from wisp's hand-rolled setup; the canonical lightyear examples
    // all trigger `Start`. Proven in a minimal repro: `Start` makes both clients receive their own
    // Predicted player + the opponent Interpolated.)
    commands.trigger(Start { entity });
}

/// Each new client connection gets its own `LinkOf` entity on the server. Attach a
/// `ReplicationSender` (canonical `new(SEND_INTERVAL, SinceLastAck, false)` — every example uses this,
/// NOT `default()`, which sends every frame and ships a 0-tick send-interval the client uses for
/// interpolation timing) so replication actually flows.
fn on_new_link(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert((
        Name::new("ClientLink"),
        ReplicationSender::new(
            Duration::from_millis(100),
            SendUpdatesMode::SinceLastAck,
            false,
        ),
    ));
}

/// Log + trace each handshake completion. Adapted from `wisp/src/net/server.rs:178-193`.
fn on_client_connected(trigger: On<Add, Connected>, clients: Query<&RemoteId, With<ClientOf>>) {
    let Ok(RemoteId(peer_id)) = clients.get(trigger.entity) else {
        return;
    };
    info!("Client connected: {:?}", peer_id);
    let id = match peer_id {
        PeerId::Netcode(id) | PeerId::Steam(id) | PeerId::Local(id) | PeerId::Entity(id) => {
            Some(*id)
        }
        _ => None,
    };
    trace::event("client_connected", json!({ "client_id": id }));
}
