//! Server-side arena gameplay: the `spawn_player_on_connect` observer spawns one networked combatant
//! per connected client (`Replicate::to_clients(All)` + `PredictionTarget::Single(owner)` +
//! `InterpolationTarget::AllExceptSingle(owner)`), then the authoritative movement controller, cast
//! pipeline, egress bridge, HUD mirror, and best-of-3 round machine all run here.
//!
//! Spawning in the connect OBSERVER (not a polled system) guarantees the owner's replication sender
//! exists before the prediction/interpolation targets resolve.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::*;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{Connected, MessageReceiver, PeerId, RemoteId, Replicate};
use lightyear::prelude::{
    ControlledBy, InterpolationTarget, NetworkTarget, Predicted, PredictionTarget,
};
use obelisk_bevy::prelude::*;
use serde_json::json;
use stat_core::StatBlock;

use crate::net::input::ArenaInput;
use crate::net::protocol::{
    CastRequestMessage, CustomizeBroadcast, CustomizeMessage, EventChannel, NetworkOwner,
    NetworkedCastState, NetworkedHealth, NetworkedId, NetworkedPlayer, ObeliskNetId,
    PlayerCustomization, RoundStateMessage,
};
use crate::net::{
    COUNTDOWN_SECS, PLAYER_CAPSULE_LENGTH, PLAYER_CAPSULE_RADIUS, ROUND_OVER_SECS,
    ROUND_WINS_TO_MATCH,
};
use crate::shared_controller::{apply_arena_movement, apply_arena_yaw};
use crate::trace;
use lightyear::prelude::MessageSender;

/// Plugin: server-authoritative arena gameplay — connect-time player spawn, movement controller,
/// cast pipeline, HP/cast-state mirrors, appearance round-trip, and the best-of-3 round machine.
pub struct ArenaServerPlugin;

impl Plugin for ArenaServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedIdAlloc>()
            .init_resource::<ClientPlayerMap>()
            .init_resource::<RoundState>()
            // Load the `.cast.ron` cast timelines the authoritative sim needs (firebolt). Without
            // these, obelisk's `validate_casts` rejects every cast with `TimelineMissing`. The
            // windowed client loads these via the SAME shared `crate::cast_assets` helpers; the
            // headless server must too — it is the combat authority.
            .init_resource::<crate::cast_assets::PendingCastTimelines>()
            .add_systems(
                Startup,
                (crate::cast_assets::load_cast_timelines, spawn_floor),
            )
            .add_systems(
                Update,
                (
                    crate::cast_assets::poll_cast_timelines,
                    trace_server_net_events,
                ),
            )
            .add_systems(
                Update,
                (
                    // Throttled trace of each player's authoritative avian Position so the headless
                    // movement-replication check has the server-side ground truth to compare the
                    // observers' interpolated pose against.
                    trace_server_pose,
                    // Bug 1a: stamp each player's obelisk cast phase into the replicated cast-state
                    // so the OTHER client can animate this player's cast. Every Update (a caster
                    // stands still while casting, so this is NOT gated on Changed).
                    sync_cast_state,
                    // HP mirror: mirror each player's obelisk life → replicated
                    // NetworkedHealth so the client HUD reads server-authoritative hp.
                    sync_networked_health,
                    // Cast pipeline: drain client cast_requests → free-aim
                    // `cast_skill_dir_charged_from` (fires along the client's `aim_dir`, no
                    // re-acquire) → obelisk's validate_casts gates the rest. The ClientPlayerMap is
                    // populated by `spawn_player_on_connect`; ordered after the lib's Update spatial
                    // refresh (in add_obelisk_sim_headless) so spatial reads see a fresh pipeline.
                    drain_cast_requests,
                    // Appearance pipeline (D6): drain client CustomizeMessage → update that
                    // player's PlayerCustomization + broadcast CustomizeBroadcast to all clients
                    // (reliable), mirroring the cue broadcast. The ClientPlayerMap is populated by
                    // `spawn_player_on_connect`.
                    drain_customize_requests,
                ),
            )
            // Best-of-3 round machine (guide §7). `detect_round_end` reads the death stream
            // and credits the winner; `run_round_machine` drives the FSM (wait → countdown → active →
            // round/match over) + resets/respawns on each new round; `broadcast_round_state` ships
            // the `RoundStateMessage` to every client on a phase/score/countdown change.
            // `detect_round_end` runs before `run_round_machine` so a death this frame is consumed by
            // the FSM the same frame.
            .add_systems(
                Update,
                (detect_round_end, run_round_machine, broadcast_round_state).chain(),
            )
            // The authoritative controller runs in FixedUpdate so it ticks at the fixed 60 Hz the
            // physics group integrates on. It reads the `ActionState<ArenaInput>` lightyear keeps in
            // sync for each client's controlled entity (no manual input drain). `server_apply_yaw`
            // (writes avian `Rotation`) is a separate system from `server_apply_movement` (avian
            // `Forces`) because avian's `Forces` borrows `Rotation` internally; chained so the body
            // faces the input yaw before the movement force is applied.
            .add_systems(
                FixedUpdate,
                (server_apply_yaw, server_apply_movement).chain(),
            )
            // Spawn each player when its client connects (canonical observer-driven spawn, so the
            // owner's replication sender is ready before Replicate/PredictionTarget resolve).
            .add_observer(spawn_player_on_connect)
            // Clean up the disconnected client's lookup + score entry (and drop the FSM below 2 if
            // it lost a player), so a stale ghost id can't linger in the HUD/score or perturb the
            // reset slot ordering.
            .add_observer(cleanup_player_on_disconnect);
    }
}

/// Spawn the static arena floor collider (server-side) the Dynamic player bodies rest on.
fn spawn_floor(mut commands: Commands) {
    crate::spawn_arena_floor(&mut commands);
}

/// Lookup: connected client id → their `NetworkedPlayer` entity. Populated by
/// `spawn_player_on_connect`; read for cast attribution / input routing.
#[derive(Resource, Default)]
pub struct ClientPlayerMap(pub HashMap<u64, Entity>);

/// Monotonic counter assigning each replicated entity a peer-stable `NetworkedId`. Starts at 1;
/// 0 is reserved for "unset".
#[derive(Resource, Default)]
pub struct NetworkedIdAlloc {
    next: u64,
}

impl NetworkedIdAlloc {
    /// Allocate the next stable id. (Named `allocate`, not `next`, to avoid clippy's
    /// `Iterator::next` confusion lint — semantics match wisp's `NetworkedIdAlloc::next`.)
    pub fn allocate(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

/// The two fixed arena spawn markers (spec §11 hard-coded geometry). Players are placed by
/// connection order: the first connected client at marker 0, the second at marker 1. Facing each
/// other across the +Z axis.
const SPAWN_MARKERS: [Vec3; 2] = [
    Vec3::new(-4.0, crate::net::GROUND_Y, 0.0),
    Vec3::new(4.0, crate::net::GROUND_Y, 0.0),
];

/// Resolve a netcode `PeerId` to its `u64` client id, matching every id-carrying variant.
fn peer_to_u64(peer: &PeerId) -> Option<u64> {
    match peer {
        PeerId::Netcode(id) | PeerId::Steam(id) | PeerId::Local(id) | PeerId::Entity(id) => {
            Some(*id)
        }
        _ => None,
    }
}

/// Map a sorted-client-id slot to its `Faction`: slot 0 → `Player`, every other slot → `Enemy`.
/// Extracted as a pure helper so the slot→faction mapping is unit-testable AND so the connect-time
/// spawn and the per-round reset assign factions IDENTICALLY (by the same sorted-id slot). Because
/// the slot comes from the sorted id list, the two duelists land on slots 0 and 1 regardless of
/// connect order, so they can NEVER share a faction — which would make firebolt's `Enemies`
/// hit-filter resolve zero hits and hang the match unwinnable (see invariant §11).
fn faction_for_slot(slot: usize) -> Faction {
    if slot == 0 {
        Faction::Player
    } else {
        Faction::Enemy
    }
}

/// Each player is a full obelisk combatant: `make_combatant(StatBlock::with_id(...))` +
/// `Faction::Player` + `grant_skill("firebolt")` + a CHILD hurtbox + the replicated networked
/// component set (`NetworkedPlayer`/`NetworkOwner`/`NetworkedId`/`ObeliskNetId`/`NetworkedHealth`/
/// `NetworkedCastState`/`PlayerCustomization`) + a Dynamic avian body driven by the shared force
/// controller. Replicated with `Replicate::to_clients(NetworkTarget::All)` +
/// `PredictionTarget::Single(owner)` + `InterpolationTarget::AllExceptSingle(owner)`: lightyear
/// auto-creates a `Predicted` entity on the owner's client and `Interpolated` entities elsewhere.
///
/// Spawn one `NetworkedPlayer` per client in the `On<Add, Connected>` OBSERVER (the canonical
/// lightyear `avian_3d_character`/`simple_box` pattern). Spawning here — NOT the old polled system —
/// guarantees the just-connected client's replication SENDER is established before `Replicate` +
/// `PredictionTarget`/`InterpolationTarget` resolve, so the OWNER reliably receives its `Predicted`
/// entity. (The polled spawn raced the per-connection sender setup: the owner that connected the
/// same frame the spawn ran was skipped by `to_clients(All)` and received NO replication at all —
/// no `Predicted` entity, hence no local player, hence no cast. The old "polled to avoid the
/// on-insert sender race" rationale was for the removed `Replicate::manual` path; the codebase
/// already drives connection setup from `On<Add, Connected>` observers, so this is consistent.)
#[allow(clippy::type_complexity)]
fn spawn_player_on_connect(
    trigger: On<Add, Connected>,
    connections: Query<(Entity, &RemoteId), (With<ClientOf>, With<Connected>)>,
    existing: Query<&NetworkOwner>,
    mut commands: Commands,
    mut id_alloc: ResMut<NetworkedIdAlloc>,
    mut client_map: ResMut<ClientPlayerMap>,
) {
    let conn_entity = trigger.entity;
    let Ok((_, RemoteId(peer_id))) = connections.get(conn_entity) else {
        return;
    };
    let Some(client_id) = peer_to_u64(peer_id) else {
        return;
    };
    let existing_ids: HashSet<u64> = existing.iter().map(|o| o.0).collect();
    if existing_ids.contains(&client_id) {
        return; // already spawned (idempotent guard)
    }

    // Stable slot by SORTED client id over all currently-connected clients (matches
    // `reset_for_new_round`'s sorted-client-id slots so the initial spawn == the reset position and
    // players don't teleport/swap at the first round reset).
    let mut all_ids: Vec<u64> = connections
        .iter()
        .filter_map(|(_, RemoteId(p))| peer_to_u64(p))
        .collect();
    all_ids.sort_unstable();
    all_ids.dedup();
    let slot = all_ids
        .iter()
        .position(|&id| id == client_id)
        .unwrap_or(0)
        .min(SPAWN_MARKERS.len() - 1);
    let spawn = SPAWN_MARKERS[slot];
    // OPPOSING factions so firebolt's `hit_filter: Enemies` (target_faction != caster_faction)
    // can resolve a hit player→player, and `nearest_enemy` acquires the opponent. With obelisk's
    // 3-faction model (Player/Enemy/Neutral), a 2-player duel puts slot 0 on Player and slot 1
    // on Enemy — they are mutual enemies. (If both shared `Faction::Player`, every cast would
    // pass validation but resolve ZERO hits — the filter rejects same-faction targets.) The same
    // sorted-id slot drives `reset_for_new_round`, which RE-asserts this each round (so a
    // connect-order race can't leave them sharing a faction once the round goes Active).
    let faction = faction_for_slot(slot);
    let net_id = id_alloc.allocate();
    // Stable obelisk id per client. `make_combatant` enforces ObeliskId == StatBlock.id.
    let obelisk_id = format!("player_{client_id}");

    info!(
        "Spawning NetworkedPlayer for client {client_id} (obelisk_id={obelisk_id}, \
             net_id={net_id})"
    );
    trace::event(
        "player_spawned",
        json!({
            "client_id": client_id,
            "net_id": net_id,
            "obelisk_id": obelisk_id,
            "pos": [spawn.x, spawn.y, spawn.z],
        }),
    );

    // Spawn the combatant root + networked + Dynamic physics components.
    let player = commands
        .spawn_empty()
        .make_combatant(StatBlock::with_id(obelisk_id.clone()))
        .insert((
            Name::new(format!("NetworkedPlayer({client_id})")),
            faction,
            NetworkedPlayer,
            NetworkOwner(client_id),
            NetworkedId(net_id),
            ObeliskNetId(obelisk_id.clone()),
            NetworkedHealth::default(),
            // Replicated appearance — default witch on spawn; live edits arrive via D6.
            PlayerCustomization::default(),
            // Replicated cast state (remote cast animation). Starts idle.
            NetworkedCastState::default(),
            // Native input — lightyear syncs this with the controlling client's input each tick.
            ActionState::<ArenaInput>::default(),
        ))
        .insert((
            // Server-authoritative Dynamic body driven by the shared force controller. Rotation
            // axes locked (the body only yaws via the controller's direct `Rotation` write);
            // zero friction so the force controller fully owns the planar velocity (avian's
            // `move_towards` recipe). The capsule (half-height 0.59) rests on the static floor
            // with feet at world 0. Position/Rotation/LinearVelocity/AngularVelocity replicate
            // (predicted on the owner, interpolated elsewhere).
            Position(spawn),
            Rotation::default(),
            LinearVelocity::default(),
            AngularVelocity::default(),
            RigidBody::Dynamic,
            Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH),
            LockedAxes::default()
                .lock_rotation_x()
                .lock_rotation_y()
                .lock_rotation_z(),
            Friction::new(0.0).with_combine_rule(CoefficientCombine::Min),
        ))
        .insert((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(*peer_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(*peer_id)),
            ControlledBy {
                owner: conn_entity,
                lifetime: Default::default(),
            },
        ))
        .id();

    commands.entity(player).grant_skill("firebolt");
    // Hurtbox so the server-side hit detection resolves firebolt hits against this player. It
    // lives on a CHILD entity (NOT the player root) so the player stays `RigidBody::Dynamic` —
    // a `RigidBody::Static` hurtbox on the same entity would conflict with the Dynamic body. The
    // child carries only `Hurtbox` + a `Collider` (no RigidBody), so avian attaches it to the
    // parent Dynamic body as a compound child collider that TRACKS the moving/jumping player and
    // stays in the SpatialQuery pipeline (obelisk's `detect_overlaps` resolves the child entity
    // to its `Hurtbox.owner` = the player). A `Sensor` so it adds a queryable volume without
    // contributing contact forces. The shared player capsule spans the body feet→head (origin ±0.59).
    commands.entity(player).with_children(|c| {
        c.spawn((
            Name::new("Hurtbox"),
            Hurtbox { owner: player },
            Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH),
            Sensor,
            Transform::default(),
        ));
    });

    client_map.0.insert(client_id, player);
}

/// Tear down per-client server state when a client's `Connected` is removed (disconnect). Removes
/// the client from `ClientPlayerMap` and drops its score entry from `RoundState` so a stale ghost
/// id can't linger in the HUD/score or perturb the sorted-slot ordering the reset depends on. If
/// this drops the match below 2 players, fall any in-progress phase back to `WaitingForPlayers` so
/// the survivor isn't hung in 'FIGHT!'/countdown forever. (lightyear despawns the disconnected
/// player's replicated entity via its `ControlledBy` lifetime; this only owns the server-side
/// resources keyed off the client.)
fn cleanup_player_on_disconnect(
    trigger: On<Remove, Connected>,
    connections: Query<&RemoteId, With<ClientOf>>,
    mut client_map: ResMut<ClientPlayerMap>,
    mut round: ResMut<RoundState>,
) {
    let conn_entity = trigger.entity;
    let Ok(RemoteId(peer_id)) = connections.get(conn_entity) else {
        return;
    };
    let Some(client_id) = peer_to_u64(peer_id) else {
        return;
    };
    if client_map.0.remove(&client_id).is_none() {
        return; // not a spawned duelist (idempotent guard)
    }
    // Score is keyed by obelisk_id (`make_combatant` enforces it == "player_{client_id}").
    let obelisk_id = format!("player_{client_id}");
    round.scores.remove(&obelisk_id);

    // Below 2 players: bail any in-progress (non-terminal) phase back to waiting so the survivor
    // isn't stuck. MatchOver is terminal (hold the banner); WaitingForPlayers is already there.
    if client_map.0.len() < 2 {
        match round.phase {
            RoundPhase::Countdown(_) | RoundPhase::Active | RoundPhase::RoundOver { .. } => {
                round.phase = RoundPhase::WaitingForPlayers;
                round.dirty = true;
            }
            RoundPhase::WaitingForPlayers | RoundPhase::MatchOver { .. } => {}
        }
    }
    trace::event(
        "player_disconnected",
        json!({ "client_id": client_id, "obelisk_id": obelisk_id }),
    );
}

// ---------------------------------------------------------------------------------------------
// Movement: lightyear-native server controller (the shared force controller over the replicated
// `ActionState<ArenaInput>` lightyear keeps in sync for each client's controlled entity).
//
// `Without<Predicted>` is a host-server safety guard (the server has no Predicted entities on a
// dedicated build, so it's a no-op there) mirroring `simple_box`/`avian_3d_character`.
// ---------------------------------------------------------------------------------------------

/// Face each authoritative character to its input yaw (writes avian `Rotation`).
#[allow(clippy::type_complexity)]
fn server_apply_yaw(
    mut q: Query<
        (&ActionState<ArenaInput>, &mut Rotation),
        (With<NetworkedPlayer>, Without<Predicted>),
    >,
) {
    for (action, mut rot) in &mut q {
        apply_arena_yaw(&action.0, &mut rot);
    }
}

/// Apply the planar movement force + jump impulse for each authoritative character.
#[allow(clippy::type_complexity)]
fn server_apply_movement(
    time: Res<Time>,
    mut q: Query<
        (&ComputedMass, &ActionState<ArenaInput>, Forces),
        (With<NetworkedPlayer>, Without<Predicted>),
    >,
) {
    let dt = time.delta_secs();
    for (mass, action, forces) in &mut q {
        apply_arena_movement(mass, dt, &action.0, forces);
    }
}

/// Throttled trace of each player's authoritative avian `Position` so the headless
/// movement-replication check can confirm the server's ground-truth pose changes for a moving
/// player. Keyed by `NetworkOwner`; gated on `Changed<Position>` so an idle player is silent.
#[allow(clippy::type_complexity)]
fn trace_server_pose(
    q: Query<(Entity, &Position, &NetworkOwner), (With<NetworkedPlayer>, Changed<Position>)>,
    mut throttle: Local<HashMap<Entity, u32>>,
) {
    for (entity, position, owner) in &q {
        let n = throttle.entry(entity).or_insert(0);
        *n += 1;
        if *n % 30 == 1 {
            trace::event(
                "server_pose",
                json!({
                    "owner": owner.0,
                    "pos": [position.0.x, position.0.y, position.0.z],
                }),
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Cast pipeline: client cast_request → server fire along aim_dir.
//
// The client sends a `CastRequestMessage` on the reliable `CastChannel` (it NEVER validates or
// resolves — Stage A). The server maps the sender's `RemoteId` → caster entity via the
// `ClientPlayerMap` and fires along the client's `aim_dir` (camera forward, full 3D) via
// `cast_skill_dir` — free aim, no auto-acquire. obelisk's `validate_casts` (FixedUpdate) gates
// mana/cooldown/already-casting and emits `CastBegan` or `CastRejected`. The projectile can miss
// if the client was not aimed at the target — this is intentional (free-aim design).
// ---------------------------------------------------------------------------------------------

/// Drain `CastRequestMessage`s from each connected client and fire along the client's aim direction.
///
/// Fires the caster's skill via `cast_skill_dir` with the `aim_dir` from the message (the client's
/// camera forward vector). No server-side target re-acquisition — the bolt goes where the client
/// aimed (free aim). Skips a caster already mid-cast (`AlreadyCasting` avoidance). The caster
/// entity must exist in the `ClientPlayerMap`; otherwise the request is silently dropped.
fn drain_cast_requests(
    mut receivers: Query<(&RemoteId, &mut MessageReceiver<CastRequestMessage>), With<ClientOf>>,
    client_map: Res<ClientPlayerMap>,
    casters: Query<&ObeliskId, With<NetworkedPlayer>>,
    active: Query<(), With<ActiveCast>>,
    mut commands: Commands,
) {
    for (RemoteId(peer_id), mut receiver) in &mut receivers {
        let Some(client_id) = peer_to_u64(peer_id) else {
            continue;
        };
        for req in receiver.receive() {
            let Some(&caster) = client_map.0.get(&client_id) else {
                continue;
            };
            if active.get(caster).is_ok() {
                // Already casting; obelisk would reject. Drop silently.
                continue;
            }
            let Ok(caster_id) = casters.get(caster) else {
                continue;
            };
            // Fire along the client's camera-forward direction. Fall back to -Z (straight forward)
            // if the vector is degenerate (shouldn't happen from a well-formed client).
            let dir = Dir3::new(Vec3::from(req.aim_dir)).unwrap_or(Dir3::NEG_Z);
            trace::event(
                "cast_request_accepted",
                json!({ "caster": caster_id.0, "skill_id": req.skill_id,
                        "aim_dir": req.aim_dir, "charge": req.charge }),
            );
            // Use the charged variant; the byte's gameplay meaning is documented client-side by
            // `client::net::charge_mult` (`0.5 + (c/255)*1.5`) and produced by `charge_byte_from_frac`:
            // charge=85 (`TAP_CHARGE_BYTE`) ≈ 1.0× (instant tap), charge=255 = 2.0× (full hold).
            // `u8` is inherently bounded [0, 255] — no extra clamp needed.
            //
            // Fire from the caster's EYE (`origin + Y*ARENA_EYE_HEIGHT`), the same height the client
            // camera sits at. The client's `aim_dir` is the camera-forward ray FROM that eye, so a
            // muzzle at the eye makes the bolt travel along the crosshair ray — a shot aimed at the
            // opponent lands (Bug 1). Without the offset the bolt spawns at the feet (Y=1.0) and
            // undershoots a crosshair-aimed shot.
            let muzzle_offset = Vec3::Y * crate::net::ARENA_EYE_HEIGHT;
            commands.entity(caster).cast_skill_dir_charged_from(
                req.skill_id.clone(),
                dir,
                req.charge,
                muzzle_offset,
            );
        }
    }
}

/// Drain `CustomizeMessage`s from each client and propagate the new appearance (D6). For each
/// request: resolve the sender's caster entity via `ClientPlayerMap`, update its
/// `PlayerCustomization` (so late joiners get the right initial value via component replication),
/// and broadcast a `CustomizeBroadcast { player: <net id>, parts }` to EVERY client on the reliable
/// `EventChannel` — mirroring the cue broadcast. We rely on the broadcast (not component-update
/// replication, which is unreliable here) to push the live change to the opponent's rig.
fn drain_customize_requests(
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

/// Map an optional obelisk `SkillPhase` to the replicated `NetworkedCastState.cast_phase` byte:
/// `None` → 0 (not casting), `Windup` → 1, `Active` → 2, `Recovery` → 3. `SkillPhase::Done` is the
/// terminal phase obelisk removes the `ActiveCast` on, so it maps to 0 (no cast) too. Pure helper so
/// the byte mapping is unit-testable without booting an app.
fn cast_phase_byte(phase: Option<SkillPhase>) -> u8 {
    match phase {
        Some(SkillPhase::Windup) => 1,
        Some(SkillPhase::Active) => 2,
        Some(SkillPhase::Recovery) => 3,
        Some(SkillPhase::Done) | None => 0,
    }
}

/// Stamp each player's obelisk cast state into its replicated `NetworkedCastState` so the OTHER
/// client can drive a cast animation on this player's remote rig (Bug 1a). Runs every `Update` (a
/// caster usually stands still while casting, so this is NOT gated on `Changed`). The `ActiveCast`
/// (the real obelisk cast, post-release) takes precedence; before release, a player CHARGING shows
/// Windup (1) so the opponent sees the cast wind up the instant charging begins (Bug 4). The
/// charging signal rides the replicated `ActionState<ArenaInput>` lightyear maintains.
///
/// Writes `cast_phase`/`cast_skill` ONLY when they change so a delta is shipped, not every frame.
#[allow(clippy::type_complexity)]
fn sync_cast_state(
    mut q: Query<
        (
            Option<&ActiveCast>,
            Option<&ActionState<ArenaInput>>,
            &mut NetworkedCastState,
        ),
        With<NetworkedPlayer>,
    >,
) {
    for (active, action, mut cast) in &mut q {
        let active_phase = cast_phase_byte(active.map(|c| c.phase));
        let charging = action.map(|a| a.0.charging).unwrap_or(false);
        let phase = if active_phase != 0 {
            active_phase
        } else if charging {
            1
        } else {
            0
        };
        // A simple "is casting" marker is enough here (the client only needs phase to animate).
        let skill = if phase == 0 { 0 } else { 1 };
        if cast.cast_phase != phase {
            cast.cast_phase = phase;
        }
        if cast.cast_skill != skill {
            cast.cast_skill = skill;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// HP mirror (guide §5.6): mirror obelisk life → replicated `NetworkedHealth`.
//
// The obelisk sim owns the authoritative life (`Attributes`/`StatBlock.current_life`); the client
// HUD must read a REPLICATED snapshot, not compute damage. Each `Update` we copy `life_of` +
// `max_life_of` (via `ObeliskRead`) into the player's `NetworkedHealth { current, max }`. Lightyear
// replicates the component change to every client (the spawn already inserted `NetworkedHealth`).
//
// We write the component every frame the value differs (not unconditionally) so lightyear only
// ships a delta on a real hp change — and so a throttled trace fires exactly on the hp drop the
// headless net-test asserts (50 → 30 after the first firebolt hit).
// ---------------------------------------------------------------------------------------------

/// Mirror each networked player's obelisk life into its replicated `NetworkedHealth`. Reads
/// `ObeliskRead` (the authoritative life facade); writes the component only when it changes so
/// lightyear ships a delta and the trace fires on the real drop.
fn sync_networked_health(
    read: ObeliskRead,
    mut players: Query<(Entity, &ObeliskNetId, &mut NetworkedHealth), With<NetworkedPlayer>>,
) {
    for (entity, net_id, mut health) in &mut players {
        let Some(current) = read.life_of(entity) else {
            continue;
        };
        let max = read.max_life_of(entity).unwrap_or(current);
        // Only write (and trace) on an actual change so replication ships a delta, not every tick.
        if (health.current - current).abs() > f64::EPSILON
            || (health.max - max).abs() > f64::EPSILON
        {
            health.current = current;
            health.max = max;
            trace::event(
                "hp",
                json!({ "obelisk_id": net_id.0, "current": current, "max": max }),
            );
        }
    }
}

/// Server-side observability: trace every obelisk `NetEvent` the sim mirrors (the same stream the
/// egress bridge broadcasts). Independent `MessageReader` cursor from `egress_net_events`, so both
/// see every event. Gives the headless harness the server-authoritative `CastBegan`/`DamageResolved`
/// to compare the clients' echoed values against.
fn trace_server_net_events(mut net: MessageReader<obelisk_bevy::net::NetEvent>) {
    for ev in net.read() {
        crate::skills::trace_net_event("server", ev);
    }
}

// =============================================================================================
// Best-of-3 round state machine (guide §7).
//
// The server owns the match flow and broadcasts it as a `RoundStateMessage` on the reliable
// `EventChannel`. Flow:
//   WaitingForPlayers  — until 2 ClientOf players exist.
//   Countdown(t)       — ~3s pre-round; reset hp/effects + respawn both at markers on ENTRY.
//   Active             — the duel; a round ends when a player's obelisk dies (NetEvent::EntityDied).
//   RoundOver{winner}  — brief pause crediting the SURVIVOR; first to 2 wins → MatchOver.
//   MatchOver{winner}  — terminal; the banner stays.
//
// Damage stays 100% server-authoritative (obelisk resolves it); this machine only reads the death
// stream + resets state between rounds. The reset heals to max + clears effects (so a leftover burn
// DoT doesn't pre-damage the next round) + interrupts any in-flight cast + teleports both back to
// their spawn markers.
// =============================================================================================
//
// The match-pacing constants (`ROUND_WINS_TO_MATCH`, `COUNTDOWN_SECS`, `ROUND_OVER_SECS`) live in
// the `net` tuning surface (the "match pacing" sub-section) and are imported above.

/// The match phase. Mirrors `RoundStateMessage.phase` (0..=4) but carries the live timer/winner.
#[derive(Clone, Debug, PartialEq)]
pub enum RoundPhase {
    WaitingForPlayers,
    Countdown(f32),
    Active,
    RoundOver { winner: String, remaining: f32 },
    MatchOver { winner: String },
}

impl RoundPhase {
    /// The wire phase tag (matches the `RoundStateMessage` docstring: 0 wait, 1 countdown, 2 active,
    /// 3 round-over, 4 match-over).
    fn wire_tag(&self) -> u8 {
        match self {
            RoundPhase::WaitingForPlayers => 0,
            RoundPhase::Countdown(_) => 1,
            RoundPhase::Active => 2,
            RoundPhase::RoundOver { .. } => 3,
            RoundPhase::MatchOver { .. } => 4,
        }
    }
    fn countdown_secs(&self) -> f32 {
        match self {
            RoundPhase::Countdown(t) => *t,
            RoundPhase::RoundOver { remaining, .. } => *remaining,
            _ => 0.0,
        }
    }
    fn winner(&self) -> String {
        match self {
            RoundPhase::RoundOver { winner, .. } | RoundPhase::MatchOver { winner } => {
                winner.clone()
            }
            _ => String::new(),
        }
    }
}

/// Server-owned best-of-3 match state. `scores` is keyed by obelisk_id; `entered_active_for` guards
/// the per-round reset so it runs exactly once on the Countdown→Active transition.
#[derive(Resource)]
pub struct RoundState {
    phase: RoundPhase,
    /// Round wins per obelisk_id. Populated when 2 players first appear.
    scores: HashMap<String, u8>,
    /// True when the phase/score changed and a `RoundStateMessage` must be (re)broadcast.
    dirty: bool,
    /// Set true on entering `Active`; the reset (heal/respawn) runs on the rising edge.
    needs_round_reset: bool,
}

impl Default for RoundState {
    fn default() -> Self {
        Self {
            phase: RoundPhase::WaitingForPlayers,
            scores: HashMap::new(),
            dirty: true, // broadcast the initial WaitingForPlayers once a client can receive it
            needs_round_reset: false,
        }
    }
}

impl RoundState {
    /// The two players' (obelisk_id, wins) in a stable order for the wire `scores` array. Falls back
    /// to empty entries until both players are known.
    fn wire_scores(&self) -> [(String, u8); 2] {
        let mut ids: Vec<(&String, &u8)> = self.scores.iter().collect();
        ids.sort_by(|a, b| a.0.cmp(b.0));
        let mut out = [(String::new(), 0u8), (String::new(), 0u8)];
        for (i, (id, wins)) in ids.into_iter().take(2).enumerate() {
            out[i] = (id.clone(), *wins);
        }
        out
    }
}

/// The outcome of the death(s) seen during an `Active` tick. Pure result of `round_outcome` so the
/// winner-selection (and the double-KO draw) is unit-testable without booting an app.
#[derive(Debug, PartialEq)]
enum RoundOutcome {
    /// No CURRENT player died this tick (a stray/non-player death) — the round continues.
    Continue,
    /// BOTH current players died the same tick — a draw: credit no one, replay the round.
    Draw,
    /// Exactly one current player survived — they win the round.
    Winner(String),
}

/// Decide the round outcome from the current players (`all_ids`) and the obelisk ids that died THIS
/// tick (`died_this_tick`). A double-KO (both current players in `died_this_tick`) is a `Draw`
/// rather than arbitrarily crediting whichever corpse the death stream happened to yield first. A
/// death of something that isn't a current player leaves the round running (`Continue`). Pure.
fn round_outcome(all_ids: &[String], died_this_tick: &[String]) -> RoundOutcome {
    let any_current_died = all_ids.iter().any(|id| died_this_tick.contains(id));
    if !any_current_died {
        return RoundOutcome::Continue;
    }
    let mut survivors = all_ids.iter().filter(|id| !died_this_tick.contains(id));
    match (survivors.next(), survivors.next()) {
        // Exactly one survivor among the current players → they win.
        (Some(winner), None) => RoundOutcome::Winner(winner.clone()),
        // Zero survivors (both died) → draw. (More than one survivor is impossible here: a current
        // player died, so a 1v1 leaves at most one survivor.)
        _ => RoundOutcome::Draw,
    }
}

/// Detect a round ending: while `Active`, read obelisk's `EntityDied` stream. Collect ALL deaths
/// this tick (don't break on the first) so a simultaneous double-KO is a DRAW (replay, credit no
/// one) instead of arbitrarily crediting whichever corpse arrived first and maybe handing the
/// match. Otherwise the SURVIVOR wins the round; their score increments and the phase transitions
/// to `RoundOver` (or `MatchOver` at the win threshold). Reads obelisk's `NetEvent` (stable string
/// ids) via an independent cursor from the egress/trace readers.
fn detect_round_end(
    mut net: MessageReader<obelisk_bevy::net::NetEvent>,
    mut round: ResMut<RoundState>,
    players: Query<&ObeliskNetId, With<NetworkedPlayer>>,
) {
    use obelisk_bevy::net::NetEvent;
    // Only deaths during the live round count.
    if round.phase != RoundPhase::Active {
        // Still drain the stream so a death during countdown/reset isn't mis-attributed next round.
        for _ in net.read() {}
        return;
    }
    // Collect every death this tick (not just the first) so a double-KO can be told apart.
    let died_this_tick: Vec<String> = net
        .read()
        .filter_map(|ev| match ev {
            NetEvent::EntityDied { target, .. } => Some(target.clone()),
            _ => None,
        })
        .collect();
    if died_this_tick.is_empty() {
        return;
    }
    // Build the current-player id list only now that a relevant death occurred (not every Active
    // frame).
    let all_ids: Vec<String> = players.iter().map(|o| o.0.clone()).collect();
    match round_outcome(&all_ids, &died_this_tick) {
        RoundOutcome::Continue => {}
        RoundOutcome::Draw => {
            // Replay the round (same pause, no score change). RoundOver with an empty winner.
            trace::event("round_draw", json!({ "died": died_this_tick }));
            round.phase = RoundPhase::RoundOver {
                winner: String::new(),
                remaining: ROUND_OVER_SECS,
            };
            round.dirty = true;
        }
        RoundOutcome::Winner(winner) => {
            let loser = died_this_tick.first().cloned().unwrap_or_default();
            let wins = {
                let w = round.scores.entry(winner.clone()).or_insert(0);
                *w += 1;
                *w
            };
            trace::event(
                "round_won",
                json!({ "winner": winner, "loser": loser, "wins": wins }),
            );
            if wins >= ROUND_WINS_TO_MATCH {
                round.phase = RoundPhase::MatchOver {
                    winner: winner.clone(),
                };
                trace::event("match_over", json!({ "winner": winner, "wins": wins }));
            } else {
                round.phase = RoundPhase::RoundOver {
                    winner,
                    remaining: ROUND_OVER_SECS,
                };
            }
            round.dirty = true;
        }
    }
}

/// Drive the round FSM by wall/real time each `Update`. Handles: waiting → countdown (once 2 players
/// exist) → active (with the per-round reset on entry) → round-over pause → next countdown. The reset
/// (heal/clear-effects/respawn) runs here on the Countdown→Active edge via `reset_for_new_round`.
#[allow(clippy::type_complexity)]
fn run_round_machine(
    time: Res<Time>,
    mut round: ResMut<RoundState>,
    mut players: Query<
        (
            Entity,
            &ObeliskNetId,
            &mut Attributes,
            &mut Position,
            &mut LinearVelocity,
            &NetworkOwner,
            &mut Faction,
        ),
        With<NetworkedPlayer>,
    >,
    mut commands: Commands,
    client_map: Res<ClientPlayerMap>,
) {
    let dt = time.delta_secs();
    let player_count = players.iter().count();

    // Lazily register both players in `scores` (0 wins) once they exist, so the wire `scores` array
    // carries both obelisk_ids from the first broadcast.
    if player_count >= 2 {
        for (_, net_id, ..) in &players {
            if !round.scores.contains_key(&net_id.0) {
                round.scores.insert(net_id.0.clone(), 0);
                round.dirty = true;
            }
        }
    }

    match round.phase.clone() {
        RoundPhase::WaitingForPlayers => {
            if player_count >= 2 {
                round.phase = RoundPhase::Countdown(COUNTDOWN_SECS);
                round.dirty = true;
                trace::event("round_phase", json!({ "phase": "countdown" }));
            }
        }
        RoundPhase::Countdown(t) => {
            // If a player vanished mid-countdown, fall back to waiting.
            if player_count < 2 {
                round.phase = RoundPhase::WaitingForPlayers;
                round.dirty = true;
                return;
            }
            let nt = t - dt;
            if nt <= 0.0 {
                round.phase = RoundPhase::Active;
                round.needs_round_reset = true;
                round.dirty = true;
                trace::event("round_phase", json!({ "phase": "active" }));
            } else {
                // Re-broadcast only when the displayed (ceil'd) second changes, not every frame, so
                // the wire carries ~1 countdown update/sec instead of 60.
                round.dirty |= t.ceil() != nt.ceil();
                round.phase = RoundPhase::Countdown(nt);
            }
        }
        RoundPhase::Active => {
            // If a player vanished mid-duel, fall back to waiting (mirrors the Countdown arm) so a
            // disconnect can't hang the survivor in 'FIGHT!' forever.
            if player_count < 2 {
                round.phase = RoundPhase::WaitingForPlayers;
                round.dirty = true;
                return;
            }
            // On the rising edge into Active, reset + respawn both players for the new round.
            if round.needs_round_reset {
                round.needs_round_reset = false;
                reset_for_new_round(&mut players, &mut commands, &client_map);
                trace::event("round_reset", json!({ "players": player_count }));
            }
        }
        RoundPhase::RoundOver { winner, remaining } => {
            let nr = remaining - dt;
            if nr <= 0.0 {
                // Next round: back to countdown (the reset happens on the Countdown→Active edge).
                round.phase = RoundPhase::Countdown(COUNTDOWN_SECS);
                round.dirty = true;
                trace::event("round_phase", json!({ "phase": "countdown" }));
            } else {
                // Throttle the round-over countdown re-broadcast to ~1/sec (see Countdown above).
                round.dirty |= remaining.ceil() != nr.ceil();
                round.phase = RoundPhase::RoundOver {
                    winner,
                    remaining: nr,
                };
            }
        }
        RoundPhase::MatchOver { .. } => { /* terminal — hold the banner */ }
    }
}

/// Per-round reset (runs on the Countdown→Active edge): heal every player to full, clear effects
/// (drops a leftover burn DoT), interrupt any in-flight cast, RE-assert each player's `Faction` by
/// sorted-id slot (so a connect-order race can never leave the two duelists sharing a faction —
/// which would make every firebolt resolve zero hits), and teleport both back to their fixed spawn
/// markers (avian `Position`, zeroing `LinearVelocity` so a falling/jumping body lands clean).
/// lightyear replicates the Position reset; the predicted owner rolls back to it. Slot is by the
/// player's connection order in `ClientPlayerMap` so the two land at the two markers consistently.
#[allow(clippy::type_complexity)]
fn reset_for_new_round(
    players: &mut Query<
        (
            Entity,
            &ObeliskNetId,
            &mut Attributes,
            &mut Position,
            &mut LinearVelocity,
            &NetworkOwner,
            &mut Faction,
        ),
        With<NetworkedPlayer>,
    >,
    commands: &mut Commands,
    client_map: &ClientPlayerMap,
) {
    // Stable slot assignment: order client ids the same way `spawn_player_on_connect` did (insertion
    // order isn't stable across a HashMap, so sort by client id for determinism).
    let mut ordered: Vec<(u64, Entity)> = client_map.0.iter().map(|(k, v)| (*k, *v)).collect();
    ordered.sort_by_key(|(cid, _)| *cid);
    let slot_of: HashMap<Entity, usize> = ordered
        .iter()
        .enumerate()
        .map(|(i, (_, e))| (*e, i))
        .collect();

    for (entity, net_id, mut attrs, mut position, mut lin_vel, _owner, mut faction) in
        players.iter_mut()
    {
        // Heal to full + restore mana + clear effects (drop any lingering DoT/buff).
        let max_life = attrs.0.computed_max_life();
        let max_mana = attrs.0.computed_max_mana();
        attrs.0.current_life = max_life;
        attrs.0.current_mana = max_mana;
        attrs.0.effects.clear();

        // Interrupt any in-flight cast so the new round starts clean.
        commands.entity(entity).interrupt_cast();

        // Respawn at the fixed marker for this player's slot; zero velocity so the Dynamic body
        // doesn't carry momentum (or a fall) into the new round. Re-assert the slot's faction too
        // (same sorted-id slot), so the two are guaranteed OPPOSING factions before the round goes
        // Active regardless of connect order — firebolt's `Enemies` filter can resolve hits.
        let slot = slot_of
            .get(&entity)
            .copied()
            .unwrap_or(0)
            .min(SPAWN_MARKERS.len() - 1);
        let spawn = SPAWN_MARKERS[slot];
        position.0 = spawn;
        lin_vel.0 = Vec3::ZERO;
        *faction = faction_for_slot(slot);

        trace::event(
            "player_respawn",
            json!({ "obelisk_id": net_id.0, "pos": [spawn.x, spawn.y, spawn.z],
                    "life": max_life }),
        );
    }
}

/// Broadcast the current `RoundStateMessage` to every connected client on the reliable `EventChannel`
/// whenever the round state is `dirty` (phase/score/countdown changed). Clears the flag after sending.
/// `match_seed` is the replicated session seed (forward-prep for Stage B; informational in Stage A).
fn broadcast_round_state(
    mut round: ResMut<RoundState>,
    mut senders: Query<&mut MessageSender<RoundStateMessage>, With<ClientOf>>,
) {
    if !round.dirty {
        return;
    }
    // Don't clear `dirty` until at least one sender exists, else the initial states are lost before
    // a client connects (the reliable channel only delivers to currently-connected senders).
    let mut sent = false;
    let msg = RoundStateMessage {
        phase: round.phase.wire_tag(),
        countdown: round.phase.countdown_secs(),
        scores: round.wire_scores(),
        winner: round.phase.winner(),
        match_seed: crate::net::session_seed(),
    };
    for mut sender in &mut senders {
        sender.send::<EventChannel>(msg.clone());
        sent = true;
    }
    if sent {
        round.dirty = false;
        trace::event(
            "round_state",
            json!({ "phase": msg.phase, "countdown": msg.countdown,
                    "scores": msg.scores, "winner": msg.winner }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{cast_phase_byte, faction_for_slot, round_outcome, RoundOutcome};
    use obelisk_bevy::prelude::{Faction, SkillPhase};

    /// The phase→byte mapping the client decodes (`NetworkedCastState.cast_phase`): no cast → 0,
    /// Windup → 1, Active → 2, Recovery → 3; the terminal `Done` (obelisk removes ActiveCast on it)
    /// collapses to 0. Pins the cast-state wire contract.
    #[test]
    fn cast_phase_byte_maps_each_phase() {
        assert_eq!(cast_phase_byte(None), 0);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Windup)), 1);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Active)), 2);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Recovery)), 3);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Done)), 0);
    }

    /// Slot 0 → Player, slot 1 → Enemy, and the two are always OPPOSING — so the two duelists can
    /// never share a faction (which would make firebolt's `Enemies` filter resolve zero hits and
    /// hang the match). The net-test uses ascending ids so it never exercises this; pin it here.
    #[test]
    fn faction_for_slot_assigns_opposing_factions() {
        assert_eq!(faction_for_slot(0), Faction::Player);
        assert_eq!(faction_for_slot(1), Faction::Enemy);
        assert_ne!(faction_for_slot(0), faction_for_slot(1));
    }

    /// The slot→faction mapping is CONNECT-ORDER-INDEPENDENT: whichever client connected first,
    /// each client keeps the SAME faction (derived from its position in the sorted id list), and
    /// the two are always opposing. This is the property the faction-order fix guarantees.
    #[test]
    fn faction_slotting_is_connect_order_independent() {
        // Map a pair of (client_id) given in CONNECT order → each client's faction (by sorted slot).
        let factions_for = |connect_order: [u64; 2]| -> [Faction; 2] {
            let mut sorted = connect_order.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            connect_order.map(|id| {
                let slot = sorted.iter().position(|&s| s == id).unwrap();
                faction_for_slot(slot)
            })
        };
        let ab = factions_for([7, 42]); // client 7 connected first
        let ba = factions_for([42, 7]); // client 42 connected first
                                        // Client 7 keeps its faction regardless of who connected first; likewise client 42.
        assert_eq!(ab[0], ba[1]); // client 7
        assert_eq!(ab[1], ba[0]); // client 42
                                  // ...and the two are always opposing.
        assert_ne!(ab[0], ab[1]);
        assert_ne!(ba[0], ba[1]);
    }

    /// A simultaneous double-KO (BOTH current players in the same tick's death set) is a `Draw`
    /// (credit no one, replay) — NOT an arbitrary winner taken from whichever corpse arrived first.
    #[test]
    fn round_outcome_double_ko_is_draw() {
        let players = vec!["player_1".to_string(), "player_2".to_string()];
        // Both orderings of the death set must still be a draw (order-independence is the point).
        for died in [
            vec!["player_1".to_string(), "player_2".to_string()],
            vec!["player_2".to_string(), "player_1".to_string()],
        ] {
            assert_eq!(round_outcome(&players, &died), RoundOutcome::Draw);
        }
    }

    /// One death credits the SURVIVOR; a death of something that isn't a current player leaves the
    /// round running (`Continue`).
    #[test]
    fn round_outcome_single_death_and_stray() {
        let players = vec!["player_1".to_string(), "player_2".to_string()];
        assert_eq!(
            round_outcome(&players, &["player_1".to_string()]),
            RoundOutcome::Winner("player_2".to_string())
        );
        assert_eq!(
            round_outcome(&players, &["minion_99".to_string()]),
            RoundOutcome::Continue
        );
    }
}
