//! Appearance pipeline (D6): drain each client's `CustomizeMessage` and update that player's
//! `PlayerCustomization`. The change reaches every client as a plain component UPDATE — the 0.26.4
//! receive path writes registered components directly onto the (single) Predicted entity, and
//! `SendUpdatesMode::SinceLastAck` resends unacked deltas until delivery. (The old
//! `CustomizeBroadcast` message mirror was inherited wisp lore — see the 2026-07-05 netcode spec.)

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{MessageReceiver, RemoteId};

use crate::net::protocol::{CustomizeMessage, NetworkedId, NetworkedPlayer, PlayerCustomization};
use crate::trace;

use super::spawn::{peer_to_u64, ClientPlayerMap};
use serde_json::json;

/// Drain `CustomizeMessage`s from each client and apply the new appearance (D6). For each request:
/// resolve the sender's player entity via `ClientPlayerMap` and update its `PlayerCustomization`;
/// component replication carries the update to every client (and to late joiners as the initial
/// value).
pub(crate) fn drain_customize_requests(
    mut receivers: Query<(&RemoteId, &mut MessageReceiver<CustomizeMessage>), With<ClientOf>>,
    client_map: Res<ClientPlayerMap>,
    mut players: Query<(&NetworkedId, &mut PlayerCustomization), With<NetworkedPlayer>>,
) {
    for (RemoteId(peer_id), mut receiver) in &mut receivers {
        let Some(client_id) = peer_to_u64(peer_id) else {
            continue;
        };
        for msg in receiver.receive() {
            let Some(&player) = client_map.0.get(&client_id) else {
                continue;
            };
            let Ok((net_id, mut cust)) = players.get_mut(player) else {
                continue;
            };
            cust.parts = msg.parts;
            trace::event(
                "customize_applied",
                json!({ "net_id": net_id.0, "client_id": client_id }),
            );
        }
    }
}
