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

/// The client-local fan-out of the replicated `NetEventMessage` stream. [`drain_net_events`] is
/// the SINGLE `MessageReceiver::<NetEventMessage>` drain (`receive()` consumes — footgun 8); every
/// other consumer (trace, HUD damage numbers, predicted-cast fizzle) reads this Bevy message
/// instead. (Before this fan-out, the HUD and the trace both drained the receiver and silently
/// STOLE events from each other on the windowed client — randomly missing damage numbers.)
#[derive(Message, Clone, Debug)]
pub struct ClientNetEvent(pub obelisk_bevy::net::NetEvent);

/// Register the client-side NetEvent drain + trace + fan-out. Added by BOTH client modes.
pub fn register_client_event_trace(app: &mut App) {
    app.add_message::<ClientNetEvent>();
    app.add_systems(Update, drain_net_events);
}

/// THE single `NetEventMessage` drain: trace each event + fan it out as [`ClientNetEvent`]. The
/// damage value here is the server's authoritative number echoed verbatim (the client never
/// computes it).
fn drain_net_events(
    mut receivers: Query<&mut MessageReceiver<NetEventMessage>>,
    mut out: MessageWriter<ClientNetEvent>,
) {
    for mut rx in &mut receivers {
        for NetEventMessage(ev) in rx.receive() {
            trace_net_event("client", &ev);
            out.write(ClientNetEvent(ev));
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
    // The predicted-cue registry the de-dup reads. Empty when the predicted sim isn't registered.
    app.init_resource::<PredictedCues>();
    app.add_systems(Update, consume_replicated_cues);
}

/// Drain replicated `CueWireMessage`s, trace + de-dup, forward survivors as `LocalCue`.
#[allow(clippy::type_complexity)]
fn consume_replicated_cues(
    mut receivers: Query<&mut MessageReceiver<CueWireMessage>>,
    time: Res<Time>,
    mut registry: ResMut<PredictedCues>,
    mut out: MessageWriter<crate::client::cosmetics::LocalCue>,
) {
    let now = time.elapsed_secs_f64();
    for mut rx in &mut receivers {
        for CueWireMessage(m) in rx.receive() {
            crate::trace::event(
                "client_cue_received",
                // `cue_kind` not `kind` — avoid clobbering the trace's top-level event kind.
                serde_json::json!({ "cue_id": m.cue_id, "source_id": m.source_id,
                    "cue_kind": format!("{:?}", m.kind) }),
            );
            // De-dup (WS3): skip a replicated cue this client already PLAYED as a prediction.
            // Registry-based — exact (source_id, cue_id) pairs registered at predict time — so
            // server-only cues (Template-window shards, OnHit, OnEnd, OnEmit) always play.
            registry.0.retain(|(expiry, _, _)| *expiry > now);
            if let Some(i) = registry
                .0
                .iter()
                .position(|(_, src, cue)| *src == m.source_id && *cue == m.cue_id)
            {
                registry.0.swap_remove(i);
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

/// Register the PREDICTED local-cast presentation (design WS3).
///
/// Stage-A invariant: the client predicts the own-cast COSMETICS only — the `on_cast` muzzle NOW,
/// plus the skill's `Scheduled` collision-window cues at their AUTHORED offsets (so your own bolt
/// launches at the authored moment with zero added round-trip) — but it NEVER runs
/// `ObeliskSet::ResolveHits`, NEVER spawns an obelisk `Hitbox`, and NEVER touches `CombatRng`.
/// Hit resolution + damage stay 100% server-authoritative. Every predicted cue is recorded in
/// [`PredictedCues`] so the server's authoritative copies are skipped by
/// [`consume_replicated_cues`]; a server `CastRejected` cancels the not-yet-fired queue
/// ([`cancel_rejected_casts`] — the fizzle path).
pub fn register_predicted_sim(app: &mut App) {
    app.init_resource::<PredictedCueQueue>();
    // AimDirs is normally seeded by the windowed cosmetics init; init here too so the headless
    // client (which also registers the predicted sim) has it. Idempotent.
    app.init_resource::<crate::client::cosmetics::AimDirs>();
    app.add_systems(
        Update,
        (predicted_local_cast, tick_predicted_cues, cancel_rejected_casts),
    );
}

/// Elapsed cast-time at which a window phase begins (obelisk schedules a `Scheduled { phase,
/// offset }` window at `phase_start(phase) + offset`).
pub(crate) fn phase_start(
    d: &obelisk_bevy::assets::PhaseDurations,
    phase: obelisk_bevy::assets::WindowPhase,
) -> f32 {
    use obelisk_bevy::assets::WindowPhase;
    match phase {
        WindowPhase::Windup => 0.0,
        WindowPhase::Active => d.windup,
        WindowPhase::Recovery => d.windup + d.active,
    }
}

/// Registry of cues this client has PREDICTED (played locally) and must therefore skip when the
/// server's authoritative copy arrives: `(expires_at_secs, source_id, cue_id)`. Entries are
/// consumed on match (the server sends each fired cue once) and purged past expiry. Design WS3 —
/// replaces the old "de-dup OnCast by kind" rule so emitter-spawned Template-window cues (e.g.
/// blizzard shards, which the client does NOT predict) still play.
#[derive(Resource, Default)]
pub(crate) struct PredictedCues(Vec<(f64, String, String)>);

/// A predicted cue waiting for its authored fire time.
struct ScheduledCue {
    fire_in: Timer,
    cue: crate::net::cue::CueMessage,
    /// Cancel keys for fizzle (a rejected cast cancels its not-yet-fired cues).
    skill_id: String,
    source_id: String,
}

/// Predicted cue windows scheduled by `predicted_local_cast`, fired by `tick_predicted_cues`.
#[derive(Resource, Default)]
struct PredictedCueQueue(Vec<ScheduledCue>);

/// Fire scheduled predicted cues when their timers elapse, refreshing `position` to the caster's
/// LIVE predicted pose (they may have moved during the windup).
fn tick_predicted_cues(
    time: Res<Time>,
    mut queue: ResMut<PredictedCueQueue>,
    local: Query<
        (
            &avian3d::prelude::Position,
            &crate::net::protocol::ObeliskNetId,
            &lightyear::prelude::input::native::ActionState<crate::net::input::ArenaInput>,
        ),
        With<crate::client::net::LocalNetPlayer>,
    >,
    mut out: MessageWriter<crate::client::cosmetics::LocalCue>,
) {
    if queue.0.is_empty() {
        return;
    }
    let live: Option<(bevy::prelude::Vec3, String)> = local
        .iter()
        .next()
        // The HAND launch point — the same offset the server spawns the authoritative window
        // at (net::CAST_HAND_OFFSET), so the predicted trail leaves the hand the charge
        // gathered on, not the eye.
        .map(|(p, id, a)| (p.0 + crate::net::hand_launch_offset(a.0.yaw), id.0.clone()));
    queue.0.retain_mut(|s| {
        s.fire_in.tick(time.delta());
        if !s.fire_in.is_finished() {
            return true;
        }
        let mut cue = s.cue.clone();
        if let Some((pos, ref id)) = live {
            if *id == s.source_id {
                cue.position = pos;
            }
        }
        crate::trace::event(
            "predicted_cue",
            serde_json::json!({ "cue_id": cue.cue_id, "source_id": cue.source_id }),
        );
        out.write(crate::client::cosmetics::LocalCue(cue));
        false
    });
}

/// Fizzle (design WS3): the server rejected a cast (cooldown/mana/mid-cast) — cancel its
/// not-yet-fired predicted cues so no ghost bolt launches. The already-played on_cast muzzle is
/// acceptable (denied-cast flicker, industry standard). Reads the [`ClientNetEvent`] fan-out.
fn cancel_rejected_casts(
    mut events: MessageReader<ClientNetEvent>,
    local: Query<&crate::net::protocol::ObeliskNetId, With<crate::client::net::LocalNetPlayer>>,
    mut queue: ResMut<PredictedCueQueue>,
) {
    let Some(local_id) = local.iter().next().map(|o| o.0.clone()) else {
        return;
    };
    for ClientNetEvent(ev) in events.read() {
        if let obelisk_bevy::net::NetEvent::CastRejected {
            caster, skill_id, ..
        } = ev
        {
            if *caster == local_id {
                crate::trace::event(
                    "predicted_fizzle",
                    serde_json::json!({ "skill_id": skill_id }),
                );
                queue
                    .0
                    .retain(|s| !(s.source_id == *caster && s.skill_id == *skill_id));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::phase_start;
    use obelisk_bevy::assets::{PhaseDurations, WindowPhase};

    /// Scheduled-window fire times: Windup-phase windows fire at `offset`, Active at
    /// `windup + offset`, Recovery at `windup + active + offset` (matches obelisk's scheduler).
    #[test]
    fn phase_start_offsets_match_timeline_order() {
        let d = PhaseDurations {
            windup: 0.3,
            active: 0.1,
            recovery: 0.2,
        };
        assert_eq!(phase_start(&d, WindowPhase::Windup), 0.0);
        assert_eq!(phase_start(&d, WindowPhase::Active), 0.3);
        assert!((phase_start(&d, WindowPhase::Recovery) - 0.4).abs() < 1e-6);
    }
}

/// Consume [`PredictedCast`] → play the `on_cast` cue NOW and schedule the skill's `Scheduled`
/// collision-window cues at their authored offsets (design WS3), so the local player's bolt
/// launches at the authored moment with zero added round-trip. Registers every predicted cue in
/// [`PredictedCues`] so the server's copies are skipped. Template windows (emitter-spawned, e.g.
/// blizzard shards) are NOT predicted — their server cues play normally. Cosmetic-only (Stage A).
fn predicted_local_cast(
    mut predicted: MessageReader<crate::client::net::PredictedCast>,
    handles: Option<Res<CastTimelineHandles>>,
    timelines: Option<Res<Assets<CastTimeline>>>,
    time: Res<Time>,
    mut aim: ResMut<crate::client::cosmetics::AimDirs>,
    mut registry: ResMut<PredictedCues>,
    mut queue: ResMut<PredictedCueQueue>,
    mut out: MessageWriter<crate::client::cosmetics::LocalCue>,
) {
    // Cast-timeline infra is optional (a client mode that never loaded timelines just doesn't
    // predict cosmetics — no crash).
    let (Some(handles), Some(timelines)) = (handles, timelines) else {
        return;
    };
    let now = time.elapsed_secs_f64();
    for cast in predicted.read() {
        // If the timeline isn't loaded, skip (cosmetics-only; no crash).
        let Some(tl) = handles.0.get(&cast.skill_id).and_then(|h| timelines.get(h)) else {
            continue;
        };
        // Stash the aim so `spawn_cue_cosmetics` flies the cosmetic projectile the right way
        // (keyed by the caster's ObeliskId — matches the cue's source_id).
        aim.0.insert(cast.source_id.clone(), cast.aim_dir);
        // on_cast: fires immediately.
        if let Some(cue_id) = tl.vfx_cues.get("on_cast") {
            registry
                .0
                .push((now + 5.0, cast.source_id.clone(), cue_id.clone()));
            crate::trace::event(
                "predicted_cast",
                serde_json::json!({ "cue_id": cue_id, "source_id": cast.source_id }),
            );
            out.write(crate::client::cosmetics::LocalCue(
                crate::net::cue::CueMessage {
                    cue_id: cue_id.clone(),
                    skill_id: cast.skill_id.clone(),
                    source_id: cast.source_id.clone(),
                    position: cast.position,
                    // Carry the predicted cast's aim so the local predicted bolt flies the right
                    // way (the cue's own aim_dir is the single source of truth).
                    aim_dir: cast.aim_dir,
                    position_from: None,
                    charge: Some(cast.charge),
                    end_reason: None,
                    kind: crate::net::cue::CueKind::OnCast,
                },
            ));
        }
        // Scheduled collision windows: predict their on_window cues at the authored offsets.
        for w in &tl.collision_windows {
            let obelisk_bevy::assets::WindowSpawn::Scheduled { phase, offset } = &w.spawn else {
                continue; // Template windows are emitter-spawned — server-cued only
            };
            let Some(cue_id) = tl.vfx_cues.get(&format!("on_window_{}", w.id)) else {
                continue;
            };
            let fire_at = phase_start(&tl.phase_durations, *phase) + offset;
            registry.0.push((
                now + fire_at as f64 + 5.0,
                cast.source_id.clone(),
                cue_id.clone(),
            ));
            queue.0.push(ScheduledCue {
                fire_in: Timer::from_seconds(fire_at, TimerMode::Once),
                cue: crate::net::cue::CueMessage {
                    cue_id: cue_id.clone(),
                    skill_id: cast.skill_id.clone(),
                    source_id: cast.source_id.clone(),
                    position: cast.position, // refreshed to the live caster pose at fire time
                    aim_dir: cast.aim_dir,
                    position_from: None,
                    charge: Some(cast.charge),
                    end_reason: None,
                    kind: crate::net::cue::CueKind::OnWindow,
                },
                skill_id: cast.skill_id.clone(),
                source_id: cast.source_id.clone(),
            });
        }
    }
}
