//! Appearance pipeline (D6): drain each client's `CustomizeMessage`, update that player's
//! `PlayerCustomization`, then broadcast `CustomizeBroadcast` to every client on the reliable
//! `EventChannel` (mirroring the cue broadcast). Live edits ride the reliable broadcast, NOT
//! component-update replication.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{MessageReceiver, MessageSender, RemoteId};

use crate::net::protocol::{
    CustomizeBroadcast, CustomizeMessage, EventChannel, NetworkedId, NetworkedPlayer,
    PlayerCustomization,
};
use crate::trace;

use super::spawn::{peer_to_u64, ClientPlayerMap};
use serde_json::json;

/// Drain `CustomizeMessage`s from each client and propagate the new appearance (D6). For each
/// request: resolve the sender's caster entity via `ClientPlayerMap`, update its
/// `PlayerCustomization` (so late joiners get the right initial value via component replication),
/// and broadcast a `CustomizeBroadcast { player: <net id>, parts }` to EVERY client on the reliable
/// `EventChannel` — mirroring the cue broadcast. We rely on the broadcast (not component-update
/// replication, which is unreliable here) to push the live change to the opponent's rig.
pub(crate) fn drain_customize_requests(
    mut receivers: Query<(&RemoteId, &mut MessageReceiver<CustomizeMessage>), With<ClientOf>>,
    client_map: Res<ClientPlayerMap>,
    mut players: Query<(&NetworkedId, &mut PlayerCustomization), With<NetworkedPlayer>>,
    mut senders: Query<&mut MessageSender<CustomizeBroadcast>, With<ClientOf>>,
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
            let bcast = CustomizeBroadcast {
                player: net_id.0,
                parts: msg.parts,
            };
            for mut sender in &mut senders {
                sender.send::<EventChannel>(bcast);
            }
            trace::event(
                "customize_applied",
                json!({ "net_id": net_id.0, "client_id": client_id }),
            );
        }
    }
}
