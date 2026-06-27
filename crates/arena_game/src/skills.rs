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

/// Register the CLIENT-side trace of the replicated combat events (guide §5.5/§8). Drains the
/// replicated `NetEventMessage` stream (CastBegan / DamageResolved / …) → one trace line per event,
/// so the headless harness can assert BOTH clients receive the server-authoritative combat events
/// and that the echoed damage matches the server's.
///
/// The CUE stream (`CueWireMessage`) is drained separately by [`register_client_cue_binding`] (the
/// single drain point — `MessageReceiver::receive()` drains, so only one consumer may read it).
/// Added by both client modes.
pub fn register_client_event_trace(app: &mut App) {
    app.add_systems(Update, trace_received_net_events);
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

/// Register the CLIENT cue consumer (Task 16, guide §6.5). The SINGLE drain point for the replicated
/// `CueWireMessage` stream: it traces each received cue (`client_cue_received`), de-dups against the
/// local player's own predicted cues, and feeds the surviving cues into the cosmetics consumer via
/// the existing [`LocalCue`] channel (so `client::cosmetics::spawn_cue_cosmetics` plays them).
///
/// De-dup (guide §6.5): the LOCAL predicting player fires its OWN `on_cast`/projectile cues locally
/// (Task 17) for zero latency, so a replicated cue whose `source_id == local player's ObeliskId` AND
/// whose kind is a predicted kind (`OnCast`) is SKIPPED — the predicted copy already played.
/// Resolution-dependent cues (`OnHit`/impact) always come from the server and are never de-duped.
///
/// Headless mode adds this too (so the [H] `client_cue_received` + `cue_dispatch` traces appear and
/// the single-drain invariant holds); it just has no `spawn_cue_cosmetics` reader, so the emitted
/// `LocalCue`s harmlessly clear.
pub fn register_client_cue_binding(app: &mut App) {
    // Ensure the LocalCue channel exists even on the headless client (the windowed client already
    // adds it in `register_cue_egress`; `add_message` is idempotent-safe via Bevy's dedup).
    app.add_message::<crate::client::cosmetics::LocalCue>();
    app.add_systems(Update, consume_replicated_cues);
}

/// Drain replicated `CueWireMessage`s, trace + de-dup, forward survivors as `LocalCue`.
#[allow(clippy::type_complexity)]
fn consume_replicated_cues(
    mut receivers: Query<&mut MessageReceiver<CueWireMessage>>,
    local: Query<
        &crate::net::protocol::ObeliskNetId,
        (
            With<crate::net::protocol::NetworkedPlayer>,
            With<crate::client::net::LocalNetPlayer>,
        ),
    >,
    mut out: MessageWriter<crate::client::cosmetics::LocalCue>,
) {
    let local_id: Option<String> = local.iter().next().map(|o| o.0.clone());
    for mut rx in &mut receivers {
        for CueWireMessage(m) in rx.receive() {
            crate::trace::event(
                "client_cue_received",
                // `cue_kind` not `kind` — avoid clobbering the trace's top-level event kind.
                serde_json::json!({ "cue_id": m.cue_id, "source_id": m.source_id,
                    "cue_kind": format!("{:?}", m.kind) }),
            );
            // De-dup: skip a replicated predicted-kind cue for our own player (already played
            // locally by the predicted sim). OnHit/impact always plays (server-authoritative).
            let is_own = local_id.as_deref() == Some(m.source_id.as_str());
            let is_predicted_kind = m.kind == arena_skills::CueKind::OnCast;
            if is_own && is_predicted_kind {
                crate::trace::event(
                    "cue_deduped",
                    serde_json::json!({ "cue_id": m.cue_id, "source_id": m.source_id }),
                );
                continue;
            }
            crate::trace::event(
                "cue_dispatch",
                serde_json::json!({ "cue_id": m.cue_id, "source_id": m.source_id,
                    "cue_kind": format!("{:?}", m.kind) }),
            );
            out.write(crate::client::cosmetics::LocalCue(m));
        }
    }
}
