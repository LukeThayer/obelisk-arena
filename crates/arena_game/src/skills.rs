//! Server cue/event egress + client cue binding (the M1 `register_skill_cues` split, networked).
//!
//! M2.0 split the single-process cue binding into a pure egress helper
//! (`arena_skills::cue_event_to_message`) and a pure consumer (`arena_skills::resolve_cue`). This
//! module wires both halves to the lightyear wire:
//!
//!   - [`register_server_cue_egress`] (server): converts obelisk `CueEvent`s into serde `CueMessage`s
//!     (resolving `CueEvent.source` Entity → stable `ObeliskId` via obelisk's `ObeliskEntityIndex`)
//!     and broadcasts them as [`CueWireMessage`] on the reliable `EventChannel` to every connected
//!     client. It ALSO drives [`egress_net_events`], which broadcasts obelisk's authoritative
//!     `NetEvent` stream (CastBegan / DamageResolved / …) as [`NetEventMessage`].
//!   - [`register_client_cue_binding`] (client): consumes the replicated `CueWireMessage`s →
//!     `resolve_cue` → cosmetics (Task 16, in `client/cosmetics.rs`).
//!
//! The server NEVER spawns cosmetics (it has no presentation); it only converts + broadcasts. The
//! client NEVER resolves combat (Stage A); it only plays cosmetics from the replicated cues +
//! events. `arena_skills` stays lightyear-free — this `arena_game` glue owns the lightyear wrappers.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{MessageReceiver, MessageSender};
use obelisk_bevy::prelude::*;

use crate::net::protocol::{CueWireMessage, EventChannel, NetEventMessage};

/// Register the SERVER cue + event egress (guide §5.5).
///
/// - An observer on `CueEvent` that converts each fired cue into a serde `CueMessage` (stable
///   `source_id` via `ObeliskEntityIndex`) and broadcasts it as `CueWireMessage` to every
///   `ClientOf` sender on the reliable `EventChannel`.
/// - `egress_net_events`: drains obelisk's `MessageReader<NetEvent>` and broadcasts each as a
///   `NetEventMessage` to every `ClientOf` sender on `EventChannel`.
pub fn register_server_cue_egress(app: &mut App) {
    app.init_resource::<PendingCues>();
    // The CueEvent observer can't reliably hold a `MessageSender` query (lightyear's message
    // senders are added/removed on connection lifecycle and an observer querying them was observed
    // to silently no-op). So the observer just CAPTURES each cue into a buffer; a regular Update
    // system broadcasts the buffer, exactly like `egress_net_events`.
    app.add_observer(capture_cue_event);
    app.add_systems(Update, (broadcast_cues, egress_net_events));
}

/// Cues captured this frame by [`capture_cue_event`], drained + broadcast by [`broadcast_cues`].
#[derive(Resource, Default)]
struct PendingCues(Vec<arena_skills::CueMessage>);

/// Observer: obelisk `CueEvent { cue_id, source: Entity, position, kind }` → serde `CueMessage`
/// (with `source_id` resolved to the stable `ObeliskId`), buffered for broadcast. If the cue's
/// source has no `ObeliskId`, skip + warn rather than emit an empty-string id (mirrors obelisk's own
/// `NetEvent` mirror invariant).
fn capture_cue_event(
    cue: On<CueEvent>,
    index: Res<ObeliskEntityIndex>,
    mut pending: ResMut<PendingCues>,
) {
    let cue = cue.event();
    let Some(source_id) = index.id(cue.source) else {
        warn!(
            "cue {} source {:?} has no ObeliskId — not broadcasting",
            cue.cue_id, cue.source
        );
        return;
    };
    pending.0.push(arena_skills::cue_event_to_message(
        &cue.cue_id,
        source_id,
        cue.position,
        cue.kind.into(),
    ));
}

/// Broadcast every buffered `CueMessage` as a `CueWireMessage` on the reliable `EventChannel` to all
/// connected clients (one `send` per `ClientOf` sender). Drains the buffer.
fn broadcast_cues(
    mut pending: ResMut<PendingCues>,
    mut senders: Query<&mut MessageSender<CueWireMessage>, With<ClientOf>>,
) {
    if pending.0.is_empty() {
        return;
    }
    for m in pending.0.drain(..) {
        // NOTE: emit the cue's kind as `cue_kind`, NOT `kind` — the trace harness merges `extra`
        // over the base object, so a `"kind"` field here would clobber the top-level
        // `"kind":"cue_egress"` (the same footgun documented in `client::cosmetics`).
        crate::trace::event(
            "cue_egress",
            serde_json::json!({ "cue_id": m.cue_id, "source_id": m.source_id,
                "cue_kind": format!("{:?}", m.kind) }),
        );
        let wire = CueWireMessage(m);
        for mut sender in &mut senders {
            sender.send::<EventChannel>(wire.clone());
        }
    }
}

/// Drain obelisk's authoritative `NetEvent` stream and broadcast each as a `NetEventMessage` on the
/// reliable `EventChannel` to every connected client (guide §5.5a). obelisk's `NetEvent` already
/// uses stable string ids — wire-ready, broadcast verbatim. Independent `MessageReader` cursor from
/// the server's `trace_server_net_events`, so both see every event.
fn egress_net_events(
    mut net: MessageReader<obelisk_bevy::net::NetEvent>,
    mut senders: Query<&mut MessageSender<NetEventMessage>, With<ClientOf>>,
) {
    for ev in net.read() {
        let wire = NetEventMessage(ev.clone());
        for mut sender in &mut senders {
            sender.send::<EventChannel>(wire.clone());
        }
    }
}

/// Register the CLIENT-side trace of the replicated combat events + cues (guide §5.5/§8). Drains
/// the replicated `NetEventMessage` (CastBegan / DamageResolved / …) and `CueWireMessage` streams
/// and emits a trace line per item, so the headless harness can assert that BOTH clients receive
/// the server-authoritative combat events + a cue, and that the echoed damage matches the server's.
///
/// This is the OBSERVABILITY half; the actual cosmetics dispatch (+ de-dup) is
/// `client::cosmetics` (Task 16). Added by both client modes (windowed + headless).
pub fn register_client_event_trace(app: &mut App) {
    app.add_systems(Update, (trace_received_net_events, trace_received_cues));
}

/// Drain the replicated `NetEventMessage` stream → one trace line per event. The damage value here
/// is the server's authoritative number echoed verbatim (the client never computes it).
fn trace_received_net_events(mut receivers: Query<&mut MessageReceiver<NetEventMessage>>) {
    use obelisk_bevy::net::NetEvent;
    for mut rx in &mut receivers {
        for NetEventMessage(ev) in rx.receive() {
            match ev {
                NetEvent::CastBegan {
                    caster,
                    skill_id,
                    total_duration,
                } => crate::trace::event(
                    "client_net_cast_began",
                    serde_json::json!({ "caster": caster, "skill_id": skill_id,
                        "total_duration": total_duration }),
                ),
                NetEvent::DamageResolved {
                    caster,
                    target,
                    skill_id,
                    total_damage,
                    is_killing_blow,
                    life_after,
                } => crate::trace::event(
                    "client_net_damage_resolved",
                    serde_json::json!({ "caster": caster, "target": target, "skill_id": skill_id,
                        "total_damage": total_damage, "is_killing_blow": is_killing_blow,
                        "life_after": life_after }),
                ),
                NetEvent::EntityDied { target, killer } => crate::trace::event(
                    "client_net_entity_died",
                    serde_json::json!({ "target": target, "killer": killer }),
                ),
                other => crate::trace::event(
                    "client_net_event",
                    serde_json::json!({ "event": format!("{other:?}") }),
                ),
            }
        }
    }
}

/// Drain the replicated `CueWireMessage` stream → one trace line per cue received.
fn trace_received_cues(mut receivers: Query<&mut MessageReceiver<CueWireMessage>>) {
    for mut rx in &mut receivers {
        for CueWireMessage(m) in rx.receive() {
            crate::trace::event(
                "client_cue_received",
                // `cue_kind` not `kind` — avoid clobbering the trace's top-level event kind.
                serde_json::json!({ "cue_id": m.cue_id, "source_id": m.source_id,
                    "cue_kind": format!("{:?}", m.kind) }),
            );
        }
    }
}
