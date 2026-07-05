//! Server cue/event egress + client cue binding.
//!
//! The cue binding is split into a pure egress helper (`crate::net::cue::cue_event_to_message`) and,
//! in a later task (C3), a pure consumer that resolves a `CueBinding`. This module wires the egress
//! half to the lightyear wire:
//!
//!   - [`register_server_cue_egress`] (server): converts obelisk `CueEvent`s into serde `CueMessage`s
//!     (resolving `CueEvent.source` Entity → stable `ObeliskId` via obelisk's `ObeliskEntityIndex`)
//!     and broadcasts them as [`CueWireMessage`] on the reliable `EventChannel` to every connected
//!     client. It ALSO drives [`egress_net_events`], which broadcasts obelisk's authoritative
//!     `NetEvent` stream (CastBegan / DamageResolved / …) as [`NetEventMessage`].
//!   - [`register_client_cue_binding`] (client): consumes the replicated `CueWireMessage`s and
//!     forwards survivors to cosmetics (in `client/cosmetics.rs`) — rendering itself is stubbed
//!     until C3 (which adds `bevy_effect`).
//!
//! The server NEVER spawns cosmetics (it has no presentation); it only converts + broadcasts. The
//! client NEVER resolves combat (Stage A); it only plays cosmetics from the replicated cues +
//! events. `crate::net::cue` stays lightyear-free — this module owns the lightyear wrappers.

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
struct PendingCues(Vec<crate::net::cue::CueMessage>);

/// Observer: obelisk `CueEvent { cue_id, source: Entity, position, kind }` → serde `CueMessage`
/// (with `source_id` resolved to the stable `ObeliskId`), buffered for broadcast. If the cue's
/// source has no `ObeliskId`, skip + warn rather than emit an empty-string id (mirrors obelisk's own
/// `NetEvent` mirror invariant).
fn capture_cue_event(
    cue: On<CueEvent>,
    index: Res<ObeliskEntityIndex>,
    active_casts: Query<&ActiveCast>,
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
    // Bug 1b: carry the caster's aim direction so OBSERVERS fly the cosmetic projectile the right
    // way (without it they default to +Z, "always forward"). The on_cast cue fires during Windup,
    // so the caster's `ActiveCast` is present and holds the resolved `aim_dir`. Falls back to
    // `Vec3::ZERO` (= "unknown" → the consumer uses its local AimDirs lookup) for non-cast cues.
    let aim = active_casts
        .get(cue.source)
        .map(|c| c.aim_dir)
        .unwrap_or(Vec3::ZERO);
    pending
        .0
        .push(crate::net::cue::cue_event_to_message(cue, source_id, aim));
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
    for mut rx in &mut receivers {
        for NetEventMessage(ev) in rx.receive() {
            trace_net_event("client", &ev);
        }
    }
}

/// Emit one `<prefix>_net_*` trace line for an obelisk `NetEvent`, with the same per-variant `kind`s
/// and fields both peers use — the server passes `"server"` (`trace_server_net_events`) and the
/// client passes `"client"` (`trace_received_net_events`). Collapses the two formerly-duplicated
/// match arms into one site; the emitted trace KINDS + fields are byte-identical to the prior copies.
pub fn trace_net_event(prefix: &str, ev: &obelisk_bevy::net::NetEvent) {
    use obelisk_bevy::net::NetEvent;
    match ev {
        NetEvent::CastBegan {
            caster,
            skill_id,
            total_duration,
        } => crate::trace::event(
            &format!("{prefix}_net_cast_began"),
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
            &format!("{prefix}_net_damage_resolved"),
            serde_json::json!({ "caster": caster, "target": target, "skill_id": skill_id,
                "total_damage": total_damage, "is_killing_blow": is_killing_blow,
                "life_after": life_after }),
        ),
        NetEvent::EntityDied { target, killer } => crate::trace::event(
            &format!("{prefix}_net_entity_died"),
            serde_json::json!({ "target": target, "killer": killer }),
        ),
        other => crate::trace::event(
            &format!("{prefix}_net_event"),
            serde_json::json!({ "event": format!("{other:?}") }),
        ),
    }
}

/// Register the CLIENT cue consumer (guide §6.5). The SINGLE drain point for the replicated
/// `CueWireMessage` stream: it traces each received cue (`client_cue_received`), de-dups against the
/// local player's own predicted cues, and feeds the surviving cues into the cosmetics consumer via
/// the existing [`LocalCue`] channel (so `client::cosmetics::spawn_cue_cosmetics` plays them).
///
/// De-dup (guide §6.5): the LOCAL predicting player fires its OWN `on_cast`/projectile cues locally
/// for zero latency, so a replicated cue whose `source_id == local player's ObeliskId` AND
/// whose kind is a predicted kind (`OnCast`) is SKIPPED — the predicted copy already played.
/// Resolution-dependent cues (`OnHit`/impact) always come from the server and are never de-duped.
///
/// Headless mode adds this too (so the [H] `client_cue_received` + `cue_dispatch` traces appear and
/// the single-drain invariant holds); it just has no `spawn_cue_cosmetics` reader, so the emitted
/// `LocalCue`s harmlessly clear.
pub fn register_client_cue_binding(app: &mut App) {
    // Ensure the LocalCue channel exists even on the headless client (`add_message` is
    // idempotent-safe via Bevy's dedup if another registration already added it).
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
            let is_predicted_kind = m.kind == crate::net::cue::CueKind::OnCast;
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

/// Register the PREDICTED local-cast presentation (guide §6.4).
///
/// Stage-A invariant (guide risk #2): the client predicts the own-cast COSMETICS only — it fires the
/// local `on_cast` muzzle + cosmetic projectile the instant the cast_request goes out (zero latency),
/// but it NEVER runs `ObeliskSet::ResolveHits`, NEVER spawns an obelisk `Hitbox`, and NEVER touches
/// `CombatRng`. Hit resolution + damage stay 100% server-authoritative (they arrive via the
/// replicated `NetEventMessage(DamageResolved)` a few ms later). This system is the predicted half;
/// it consumes [`PredictedCast`] (emitted by `client::net::send_cast_requests`), looks up the skill's
/// `on_cast` cue id from the loaded cast timeline, stashes the aim direction, and emits a `LocalCue`
/// so `client::cosmetics::spawn_cue_cosmetics` plays the windup + projectile cosmetics immediately.
///
/// Only added by the windowed client (the headless client has no cosmetics). The replicated OnCast
/// cue for this same local player is de-duped by [`consume_replicated_cues`], so the cast plays
/// exactly once (predicted), not twice.
pub fn register_predicted_sim(app: &mut App) {
    app.add_systems(Update, predicted_local_cast);
}

/// Consume [`PredictedCast`] → play the predicted own-cast cosmetics immediately (no resolution).
fn predicted_local_cast(
    mut predicted: MessageReader<crate::client::net::PredictedCast>,
    handles: Res<CastTimelineHandles>,
    timelines: Res<Assets<CastTimeline>>,
    mut aim: ResMut<crate::client::cosmetics::AimDirs>,
    mut out: MessageWriter<crate::client::cosmetics::LocalCue>,
) {
    for cast in predicted.read() {
        // Resolve the skill's `on_cast` cue id from its loaded cast timeline (e.g.
        // firebolt → "firebolt_cast"). If the timeline isn't loaded, skip (cosmetics-only; no crash).
        let Some(cue_id) = handles
            .0
            .get(&cast.skill_id)
            .and_then(|h| timelines.get(h))
            .and_then(|t| t.vfx_cues.get("on_cast").cloned())
        else {
            continue;
        };
        // Stash the aim so `spawn_cue_cosmetics` flies the cosmetic projectile the right way
        // (keyed by the caster's ObeliskId — matches the cue's source_id).
        aim.0.insert(cast.source_id.clone(), cast.aim_dir);
        crate::trace::event(
            "predicted_cast",
            serde_json::json!({ "cue_id": cue_id, "source_id": cast.source_id }),
        );
        out.write(crate::client::cosmetics::LocalCue(
            crate::net::cue::CueMessage {
                cue_id,
                skill_id: cast.skill_id.clone(),
                source_id: cast.source_id.clone(),
                position: cast.position,
                // Bug 1b: carry the predicted cast's aim so the local predicted bolt flies the
                // right way (and so the cue's own aim_dir is the single source of truth).
                aim_dir: cast.aim_dir,
                position_from: None,
                // `PredictedCast` carries no charge byte (see `client/net.rs`) — the predicted
                // local cue's charge only affects a cosmetic scale that isn't rendered until C3,
                // so there is nothing to thread yet.
                charge: None,
                end_reason: None,
                kind: crate::net::cue::CueKind::OnCast,
            },
        ));
    }
}
