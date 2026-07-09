//! The headless scriptable client (`ARENA_HEADLESS=1`, the `arena-observer` bin): MinimalPlugins +
//! the lightyear net stack, materializes players, traces the replicated combat/cue streams, and
//! (under `ARENA_AUTOCAST`/`ARENA_AUTOMOVE`/`ARENA_CUSTOMIZE`) scripts casts / movement / appearance
//! over the wire. This is the net-regression vehicle the `tools/net-test` harness drives.
//!
//! Also home to the [`autocast`] system + the local/remote player query filters it shares with the
//! windowed client (`app_windowed.rs`), since the headless `[H]` hooks here (e.g.
//! [`headless_customize_once`]) lean on the same [`LocalPlayerFilter`].

use bevy::prelude::*;
use obelisk_bevy::prelude::*;

use super::controller;
use super::harness::EnvConfig;
use super::net;
use super::parts;

use crate::{add_avian_with_lightyear, add_obelisk_sim_client, arena_root};

/// Query filter for THIS peer's local (predicted, owned) networked player.
pub(super) type LocalPlayerFilter = (
    With<crate::net::protocol::NetworkedPlayer>,
    With<net::LocalNetPlayer>,
);
/// Query filter for the REMOTE (predicted, opponent) networked players.
pub(super) type RemotePlayerFilter = (
    With<crate::net::protocol::NetworkedPlayer>,
    Without<net::LocalNetPlayer>,
);

/// Run the headless connectivity/movement client: MinimalPlugins + LogPlugin (no window, no
/// rendering, no HUD) + the lightyear client net stack + the avian-lightyear physics + the
/// net-driven player layer (materialize bodies + send input). Gated by `ARENA_HEADLESS=1` so two
/// clients can be brought up under the net-test harness to verify connection + replication +
/// movement without windows.
///
/// It materializes a body for every replicated `NetworkedPlayer` (tracing `materialized_player`
/// with the owner's client_id + local flag so the late-joiner check still passes) and — under
/// `ARENA_AUTOMOVE=1` — feeds a constant forward input so the movement-replication check can drive
/// the server controller headlessly.
pub fn run_headless_client() {
    let root = arena_root();
    let mut app = App::new();

    app.add_plugins((
        MinimalPlugins,
        bevy::log::LogPlugin::default(),
        AssetPlugin {
            file_path: root.join("assets").to_string_lossy().into_owned(),
            ..default()
        },
        TransformPlugin,
        bevy::mesh::MeshPlugin,
        bevy::scene::ScenePlugin,
    ));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    crate::net::client::connect_to_configured(&mut app);
    add_avian_with_lightyear(&mut app);

    // The CLIENT obelisk subset — same composition as the windowed client (asset/config infra, NO
    // ResolveHits/CombatRng — Stage A). Gives the headless observer the SkillRegistry + CastTimeline
    // asset infra the predicted-cast presentation (WS3) schedules from, so the net-test exercises
    // the real predicted path.
    add_obelisk_sim_client(&mut app);
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&root.join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(root.join("config/skills")));
    // Client-side RNG is seeded but NEVER drawn (Stage A) — same as the windowed client.
    app.seed_combat_rng(1);

    // Net-driven player layer: attach a body to each materialized NetworkedPlayer + stage input
    // (lightyear drives prediction + remote interpolation). Also traces replicated/materialized
    // players for the late-joiner check.
    app.add_plugins(net::ClientNetPlayerPlugin);

    // Seed CameraYaw + AimPitch from env vars so `send_cast_requests` has an aim direction.
    // `ARENA_CAM_YAW` (radians) steers the cast; `ARENA_TEST_PITCH` seeds the pitch (both
    // default to 0.0 if unset). These mirror the env-var seeding in `ArenaControllerPlugin`
    // (windowed client) via the SHARED `EnvConfig` parser, giving the headless harness the same
    // knob to aim the caster.
    let env = EnvConfig::from_env();
    app.insert_resource(controller::CameraYaw(env.cam_yaw))
        .insert_resource(controller::AimPitch(env.test_pitch));

    // Level sync (physics-only: `windowed: false`): mirrors the server's level id from the
    // round-state fan-out — the LOBBY level's floor replaces the old hard-coded floor collider, so
    // the predicted Dynamic body rests on real level geometry here too.
    app.add_plugins(super::level::ClientLevelPlugin { windowed: false });
    // [H] Trace the replicated combat events (NetEventMessage), and consume the replicated cues
    // (CueWireMessage → trace + de-dup + dispatch). Headless has no cosmetics reader, so the
    // dispatched LocalCues clear harmlessly; the trace lines are what the net-test asserts on.
    crate::skills::register_client_event_trace(&mut app);
    crate::skills::register_client_cue_binding(&mut app);
    // The predicted own-cast presentation (WS3) — headless has no cosmetics reader, so the
    // predicted LocalCues clear harmlessly, but the `predicted_cast`/`predicted_cue`/`cue_deduped`
    // traces + the de-dup registry run exactly as in the windowed client, so the net-test
    // exercises the real predicted path. The cast timelines it schedules from load here the same
    // way the (equally headless) server loads them.
    app.init_resource::<crate::cast_assets::PendingCastTimelines>();
    app.add_systems(Startup, crate::cast_assets::load_cast_timelines);
    app.add_systems(Update, crate::cast_assets::poll_cast_timelines);
    crate::skills::register_predicted_sim(&mut app);
    app.add_systems(
        Update,
        (
            trace_replicated_players,
            trace_replicated_health,
            trace_replicated_round_state,
        ),
    );

    // [H] AUTOMOVE hook: feed a constant forward movement input so the headless movement-replication
    // check can drive the server controller without a keyboard. Off unless ARENA_AUTOMOVE=1.
    if std::env::var("ARENA_AUTOMOVE").ok().as_deref() == Some("1") {
        app.add_systems(Update, automove_input);
    }

    // [H] CUSTOMIZE hook (D6 verification): if `ARENA_CUSTOMIZE=<top_index>` is set, change the
    // local PartSelection's Top slot once (after we own a local player) and mark it dirty so
    // `send_customization` ships a `CustomizeMessage`. The server applies + broadcasts it, and the
    // OTHER observer's `drain_customize_broadcasts` emits a `customize_received` trace — proving the
    // live appearance change propagates to the opponent over the wire.
    if let Some(top) = std::env::var("ARENA_CUSTOMIZE")
        .ok()
        .and_then(|v| v.parse::<u8>().ok())
    {
        app.insert_resource(HeadlessCustomize(top));
        app.add_systems(Update, headless_customize_once);
    }

    // [H] AUTOEQUIP hook (ARENA_AUTOEQUIP=<weapon_id>): request the named weapon once per lobby
    // visit as soon as we own a local player — the harness's stand-in for the I panel. Pairs
    // with ARENA_AUTOCAST_SKILL to script casts from a non-starter weapon (e.g.
    // ARENA_AUTOEQUIP=storm_staff ARENA_AUTOCAST_SKILL=blizzard).
    if std::env::var("ARENA_AUTOEQUIP").is_ok() {
        app.add_systems(Update, autoequip_weapon);
    }

    // [H] AUTOCAST hook: once we own a local player AND an opponent is replicated, set the cast
    // intent once so `net::send_cast_requests` fires a `CastRequestMessage`. This is the
    // headless verification vehicle — a server `CastBegan` (and downstream the egress of
    // damage + cues) follows. Off unless ARENA_AUTOCAST=1.
    if std::env::var("ARENA_AUTOCAST").ok().as_deref() == Some("1") {
        app.add_systems(Update, autocast);
    }

    app.run();
}

/// AUTOCAST (`ARENA_AUTOCAST=1`), shared by the windowed and headless clients: pulse the cast
/// charge latch on a cadence once the local player + an opponent are materialized, so the input
/// stream carries a press→release edge and BOTH peers' edge detectors fire (server: authoritative
/// cast via `detect_cast_edges`; client: predicted cosmetics via `local_cast_edge`) — design WS2.
/// `ARENA_AUTOCAST_SKILL` picks the skill (default firebolt) via [`net::SelectedSkill`] — a
/// comma-separated list rotates one entry per pulse (e.g. `portal_orange,portal_blue` places the
/// pair); `ARENA_AUTOCAST_PERIOD` the cadence (default 0.8s, comfortably above firebolt's ~0.6s
/// cast time so edges don't pile up behind an in-flight cast — the server skips a caster mid-cast).
///
/// Repeating (not one-shot) so an AUTOCAST client drives a FULL best-of-3 match across rounds.
/// The server is authoritative for whether a given cast lands; firing continuously is safe — a
/// stray cast during a countdown/round-over window is harmless.
pub(super) fn autocast(
    time: Res<Time>,
    mut accum: Local<f32>,
    mut pulses: Local<usize>,
    mut input: ResMut<net::LocalInput>,
    yaw: Res<controller::CameraYaw>,
    pitch: Res<controller::AimPitch>,
    mut charge: ResMut<net::ChargeState>,
    mut selected: ResMut<net::SelectedSkill>,
    local: Query<(), LocalPlayerFilter>,
    remotes: Query<(), RemotePlayerFilter>,
) {
    // Need our own player + an opponent before the cast has a useful target.
    if local.iter().next().is_none() || remotes.iter().next().is_none() {
        return;
    }
    // Stamp the configured aim onto the input stream — WITHOUT this, a headless caster that
    // isn't also AUTOMOVE-ing (the only other yaw/pitch writer) casts along the identity aim
    // (-Z, flat) no matter what ARENA_CAM_YAW/ARENA_TEST_PITCH say. Windowed, CameraYaw/AimPitch
    // are the live camera so this matches what the input bridge writes anyway.
    input.yaw = yaw.0;
    input.pitch = pitch.0;
    let period = std::env::var("ARENA_AUTOCAST_PERIOD")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.8);
    *accum += time.delta_secs();
    if *accum >= period {
        *accum = 0.0;
        if let Ok(skills) = std::env::var("ARENA_AUTOCAST_SKILL") {
            let list: Vec<&str> = skills.split(',').filter(|s| !s.is_empty()).collect();
            if !list.is_empty() {
                selected.0 = list[*pulses % list.len()].to_string();
            }
        }
        *pulses += 1;
        charge.tap_latch = true; // one charging tick, then release → edge on both peers
    }
}

/// [H] `ARENA_AUTOEQUIP=<weapon_id>`: send `EquipWeaponMessage` once per lobby visit, as soon as
/// the local player exists — the headless stand-in for the lobby's I panel (mirrors
/// `autostart_level`'s once-per-lobby re-arm so soaks re-equip after each match).
pub(super) fn autoequip_weapon(
    state: Res<super::level::ClientLevel>,
    local: Query<(), LocalPlayerFilter>,
    sender: Option<
        Single<&mut lightyear::prelude::MessageSender<crate::net::protocol::EquipWeaponMessage>>,
    >,
    mut sent_this_lobby: Local<bool>,
) {
    if state.phase != 0 {
        *sent_this_lobby = false;
        return;
    }
    if *sent_this_lobby || local.iter().next().is_none() {
        return;
    }
    let Ok(item_id) = std::env::var("ARENA_AUTOEQUIP") else {
        return;
    };
    let Some(mut sender) = sender else { return };
    sender.send::<crate::net::protocol::RequestChannel>(crate::net::protocol::EquipWeaponMessage {
        item_id: item_id.clone(),
    });
    *sent_this_lobby = true;
    crate::trace::event("equip_weapon_sent", serde_json::json!({ "item_id": item_id }));
}

/// The Top-slot index a headless observer applies once under `ARENA_CUSTOMIZE` (D6 verification).
#[derive(Resource)]
struct HeadlessCustomize(u8);

/// [H] CUSTOMIZE (D6): once we own a local player, set its Top slot to the configured index and
/// flag `CustomizeDirty` so `net::send_customization` ships the change. One-shot via the `done`
/// local. Lets the net-test confirm a non-default selection propagates to the other observer.
fn headless_customize_once(
    cfg: Res<HeadlessCustomize>,
    mut selection: ResMut<parts::PartSelection>,
    mut dirty: ResMut<net::CustomizeDirty>,
    local: Query<(), LocalPlayerFilter>,
    mut done: Local<bool>,
) {
    if *done || local.iter().next().is_none() {
        return;
    }
    selection.top = cfg.0;
    dirty.0 = true;
    *done = true;
}

/// Polling tracer: emit `replicated_player` once per replicated
/// `NetworkedPlayer` (keyed by `NetworkOwner.client_id`) so the late-joiner check has its signal.
fn trace_replicated_players(
    players: Query<
        (
            Entity,
            &crate::net::protocol::NetworkOwner,
            Option<&crate::net::protocol::ObeliskNetId>,
        ),
        With<crate::net::protocol::NetworkedPlayer>,
    >,
    mut seen: Local<std::collections::HashSet<u64>>,
) {
    for (entity, owner, obelisk_id) in &players {
        if seen.insert(owner.0) {
            let obelisk_id = obelisk_id.map(|o| o.0.clone());
            bevy::log::info!(
                "client received replicated NetworkedPlayer: entity={entity:?} owner={} obelisk_id={obelisk_id:?}",
                owner.0
            );
            crate::trace::event(
                "replicated_player",
                serde_json::json!({ "owner": owner.0, "obelisk_id": obelisk_id }),
            );
        }
    }
}

/// [H] Trace the replicated `NetworkedHealth` whenever it CHANGES on this client — proves the
/// server-authoritative hp mirror replicates over the wire and drops on a hit (50 → 30).
/// `Changed<NetworkedHealth>` fires only on a real delta, so the trace mirrors the server's `hp`
/// stream from the receiving end. Keyed by the replicated `ObeliskNetId`.
#[allow(clippy::type_complexity)]
fn trace_replicated_health(
    changed: Query<
        (
            &crate::net::protocol::ObeliskNetId,
            &crate::net::protocol::NetworkedHealth,
        ),
        (
            With<crate::net::protocol::NetworkedPlayer>,
            Changed<crate::net::protocol::NetworkedHealth>,
        ),
    >,
) {
    for (net_id, health) in &changed {
        crate::trace::event(
            "client_hp",
            serde_json::json!({ "obelisk_id": net_id.0, "current": health.current,
                "max": health.max }),
        );
    }
}

/// [H] THE headless `RoundStateMessage` drain (single-drain rule, invariant §8): trace each
/// replicated round state so the round-machine check can assert the best-of-3 flow replicates over
/// the wire, AND fan every message out as [`super::level::RoundStateChanged`] for the level sync +
/// autostart consumers. The TRACE dedups consecutive identical (phase, scores, winner) so it
/// carries transitions, not the ~1/sec countdown re-broadcasts — but the fan-out forwards
/// EVERYTHING (the level sync must see every change).
#[allow(clippy::type_complexity)]
fn trace_replicated_round_state(
    mut receivers: Query<
        &mut lightyear::prelude::MessageReceiver<crate::net::protocol::RoundStateMessage>,
    >,
    mut out: MessageWriter<super::level::RoundStateChanged>,
    mut last: Local<Option<(u8, [(String, u8); 2], String)>>,
) {
    for mut rx in &mut receivers {
        for msg in rx.receive() {
            let key = (msg.phase, msg.scores.clone(), msg.winner.clone());
            let fresh = last.as_ref() != Some(&key);
            if fresh {
                *last = Some(key);
                crate::trace::event(
                    "client_round_state",
                    serde_json::json!({ "phase": msg.phase, "countdown": msg.countdown,
                        "scores": msg.scores, "winner": msg.winner,
                        "host": msg.host, "level": msg.level }),
                );
            }
            out.write(super::level::RoundStateChanged(msg));
        }
    }
}

/// [H] AUTOMOVE: write a constant forward movement into [`net::LocalInput`] so the headless client
/// drives the shared controller (predicted locally + authoritative on the server). `movement.y = 1`
/// (full forward) in the `CameraYaw` frame — so the mover walks ALONG its look/aim axis (the cast
/// fires along the same axis, keeping a moving caster on the firing line). The resulting avian
/// `Position` change is what the movement-replication check asserts on (server pose changes + the
/// OTHER client observes the interpolated pose move).
pub(super) fn automove_input(
    mut input: ResMut<net::LocalInput>,
    yaw: Res<controller::CameraYaw>,
    pitch: Res<controller::AimPitch>,
) {
    input.movement = Vec2::new(0.0, 1.0);
    input.yaw = yaw.0;
    input.pitch = pitch.0;
    input.jump = false;
}
