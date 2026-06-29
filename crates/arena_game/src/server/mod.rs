//! Server-side arena gameplay: spawn one networked combatant per connected client + keep the
//! late-joiner replication targets fresh (netcode guide §5.1, §5.7). Later M2 tasks add the
//! movement controller, cast pipeline, egress bridge, HUD mirror, and round machine here.
//!
//! `refresh_replicate_on_connect` is copied VERBATIM from `wisp/src/net/server.rs:208-232`.
//! `sync_networked_players` is adapted from `wisp/src/net/server.rs:284-360` (obelisk combatant in
//! place of wisp's wizard rig).

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
use crate::shared_controller::{apply_arena_movement, apply_arena_yaw};
use crate::trace;
use lightyear::prelude::MessageSender;

pub struct ArenaServerPlugin;

impl Plugin for ArenaServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedIdAlloc>()
            .init_resource::<ClientPlayerMap>()
            .init_resource::<RoundState>()
            // Load the `.cast.ron` cast timelines the authoritative sim needs (firebolt). Without
            // these, obelisk's `validate_casts` rejects every cast with `TimelineMissing`. The
            // windowed client loads these itself (`client::load_cast_assets`); the headless server
            // must too — it is the combat authority.
            .init_resource::<PendingServerCastAssets>()
            .add_systems(Startup, (load_server_cast_assets, spawn_floor))
            .add_systems(Update, (poll_server_cast_assets, trace_server_net_events))
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
                    // HP mirror (Task 18): mirror each player's obelisk life → replicated
                    // NetworkedHealth so the client HUD reads server-authoritative hp.
                    sync_networked_health,
                    // Cast pipeline (Task 14): drain client cast_requests → re-validate
                    // server-side (re-acquire target) → cast_skill_at → obelisk's validate_casts
                    // gates the rest. Ordered after `sync_networked_players` so the ClientPlayerMap
                    // is populated, and after the lib's Update spatial refresh (in
                    // add_obelisk_sim_headless) so `nearest_enemy` sees a fresh pipeline.
                    drain_cast_requests,
                    // Appearance pipeline (D6): drain client CustomizeMessage → update that
                    // player's PlayerCustomization + broadcast CustomizeBroadcast to all clients
                    // (reliable), mirroring the cue broadcast. Ordered after sync_networked_players
                    // so the ClientPlayerMap is populated.
                    drain_customize_requests,
                ),
            )
            // Best-of-3 round machine (Task 19, guide §7). `detect_round_end` reads the death stream
            // and credits the winner; `run_round_machine` drives the FSM (wait → countdown → active →
            // round/match over) + resets/respawns on each new round; `broadcast_round_state` ships
            // the `RoundStateMessage` to every client on a phase/score/countdown change. Ordered after
            // `sync_networked_players` so players exist before the machine counts them; `detect_round_end`
            // before `run_round_machine` so a death this frame is consumed by the FSM the same frame.
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
            .add_observer(spawn_player_on_connect);
    }
}

/// Spawn the static arena floor collider (server-side) the Dynamic player bodies rest on.
fn spawn_floor(mut commands: Commands) {
    crate::spawn_arena_floor(&mut commands);
}

/// Lookup: connected client id → their `NetworkedPlayer` entity. Populated by
/// `sync_networked_players`; later tasks read it for cast attribution / input routing.
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
    // pass validation but resolve ZERO hits — the filter rejects same-faction targets.)
    let faction = if slot == 0 {
        Faction::Player
    } else {
        Faction::Enemy
    };
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
            Collider::capsule(0.35, 0.48),
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
    // contributing contact forces. capsule(0.35, 0.48) spans the body feet→head (origin ±0.59).
    commands.entity(player).with_children(|c| {
        c.spawn((
            Name::new("Hurtbox"),
            Hurtbox { owner: player },
            Collider::capsule(0.35, 0.48),
            Sensor,
            Transform::default(),
        ));
    });

    client_map.0.insert(client_id, player);
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
// Cast pipeline (Task 14 + Milestone B): client cast_request → server fire along aim_dir.
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
            // Use the charged variant: `charge_mult(Some(c)) = 0.5 + (c/255)*1.5`.
            // charge=85 ≈ 1.0× (instant tap), charge=255 = 2.0× (full hold).
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
// HP mirror (Task 18, guide §5.6): mirror obelisk life → replicated `NetworkedHealth`.
//
// The obelisk sim owns the authoritative life (`Attributes`/`StatBlock.current_life`); the client
// HUD must read a REPLICATED snapshot, not compute damage. Each `Update` we copy `life_of` +
// `max_life_of` (via `ObeliskRead`) into the player's `NetworkedHealth { current, max }`. Lightyear
// replicates the component change to every client (the spawn already inserted `NetworkedHealth`).
//
// We write the component every frame the value differs (not unconditionally) so lightyear only
// ships a delta on a real hp change — and so a throttled trace fires exactly on the hp drop the
// headless [H] check asserts (50 → 30 after the first firebolt hit).
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

// ---------------------------------------------------------------------------------------------
// Cast-timeline asset loading (Task 13): the authoritative sim needs the `.cast.ron` timelines in
// `CastTimelineHandles`, else `validate_casts` rejects every cast with `TimelineMissing`. Mirrors
// the windowed client's `load_cast_assets`/`poll_cast_assets`, headless. One handle per registered
// obelisk skill, loaded from `assets/skills/<id>.cast.ron`.
// ---------------------------------------------------------------------------------------------

/// Cast-timeline handles being polled to finish loading (skill id → handle).
#[derive(Resource, Default)]
struct PendingServerCastAssets(Vec<(String, Handle<CastTimeline>)>);

/// Kick off loading a `.cast.ron` for every registered obelisk skill (firebolt).
fn load_server_cast_assets(
    mut pending: ResMut<PendingServerCastAssets>,
    assets: Res<AssetServer>,
    skills: Res<SkillRegistry>,
) {
    let mut ids: Vec<String> = skills.0.keys().cloned().collect();
    ids.sort();
    for id in ids {
        let handle: Handle<CastTimeline> = assets.load(format!("skills/{id}.cast.ron"));
        pending.0.push((id, handle));
    }
}

/// Poll the pending cast assets each frame; move loaded ones into `CastTimelineHandles` so
/// `validate_casts` can resolve the timeline.
fn poll_server_cast_assets(
    mut pending: ResMut<PendingServerCastAssets>,
    timelines: Res<Assets<CastTimeline>>,
    mut registry: ResMut<CastTimelineHandles>,
) {
    if pending.0.is_empty() {
        return;
    }
    pending.0.retain(|(skill, handle)| {
        if timelines.get(handle).is_some() {
            info!("server: cast timeline loaded for {skill}");
            registry.0.insert(skill.clone(), handle.clone());
            false
        } else {
            true
        }
    });
}

/// Server-side observability: trace every obelisk `NetEvent` the sim mirrors (the same stream the
/// egress bridge broadcasts). Independent `MessageReader` cursor from `egress_net_events`, so both
/// see every event. Gives the headless harness the server-authoritative `CastBegan`/`DamageResolved`
/// to compare the clients' echoed values against.
fn trace_server_net_events(mut net: MessageReader<obelisk_bevy::net::NetEvent>) {
    use obelisk_bevy::net::NetEvent;
    for ev in net.read() {
        match ev {
            NetEvent::CastBegan {
                caster,
                skill_id,
                total_duration,
            } => trace::event(
                "server_net_cast_began",
                json!({ "caster": caster, "skill_id": skill_id, "total_duration": total_duration }),
            ),
            NetEvent::DamageResolved {
                caster,
                target,
                skill_id,
                total_damage,
                is_killing_blow,
                life_after,
            } => trace::event(
                "server_net_damage_resolved",
                json!({
                    "caster": caster, "target": target, "skill_id": skill_id,
                    "total_damage": total_damage, "is_killing_blow": is_killing_blow,
                    "life_after": life_after,
                }),
            ),
            NetEvent::EntityDied { target, killer } => trace::event(
                "server_net_entity_died",
                json!({ "target": target, "killer": killer }),
            ),
            other => trace::event("server_net_event", json!({ "event": format!("{other:?}") })),
        }
    }
}

// =============================================================================================
// Best-of-3 round state machine (Task 19, guide §7).
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

/// Rounds needed to win the match (best-of-3 ⇒ first to 2).
const ROUND_WINS_TO_MATCH: u8 = 2;
/// Pre-round countdown length (seconds).
const COUNTDOWN_SECS: f32 = 3.0;
/// Pause between a round ending and the next countdown (seconds), so the result is readable.
const ROUND_OVER_SECS: f32 = 2.0;

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

/// Detect a round ending: while `Active`, read obelisk's `EntityDied` stream. The SURVIVOR (the
/// living player other than the one who died) wins the round; their score increments. Transition to
/// `RoundOver` (or `MatchOver` if they reached the win threshold). Reads obelisk's `NetEvent` (stable
/// string ids) via an independent cursor from the egress/trace readers.
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
    let all_ids: Vec<String> = players.iter().map(|o| o.0.clone()).collect();
    for ev in net.read() {
        let NetEvent::EntityDied { target, .. } = ev else {
            continue;
        };
        // The winner is the OTHER player (the survivor). Robust against whether `killer` is set
        // (a burn-DoT death may have no killer attribution).
        let Some(winner) = all_ids.iter().find(|id| *id != target).cloned() else {
            continue;
        };
        let wins = {
            let w = round.scores.entry(winner.clone()).or_insert(0);
            *w += 1;
            *w
        };
        trace::event(
            "round_won",
            json!({ "winner": winner, "loser": target, "wins": wins }),
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
        break; // one death ends the round; ignore any same-frame extras
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
/// (drops a leftover burn DoT), interrupt any in-flight cast, and teleport both back to their fixed
/// spawn markers (avian `Position`, zeroing `LinearVelocity` so a falling/jumping body lands clean).
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
        ),
        With<NetworkedPlayer>,
    >,
    commands: &mut Commands,
    client_map: &ClientPlayerMap,
) {
    // Stable slot assignment: order client ids the same way `sync_networked_players` did (insertion
    // order isn't stable across a HashMap, so sort by client id for determinism).
    let mut ordered: Vec<(u64, Entity)> = client_map.0.iter().map(|(k, v)| (*k, *v)).collect();
    ordered.sort_by_key(|(cid, _)| *cid);
    let slot_of: HashMap<Entity, usize> = ordered
        .iter()
        .enumerate()
        .map(|(i, (_, e))| (*e, i))
        .collect();

    for (entity, net_id, mut attrs, mut position, mut lin_vel, _owner) in players.iter_mut() {
        // Heal to full + restore mana + clear effects (drop any lingering DoT/buff).
        let max_life = attrs.0.computed_max_life();
        let max_mana = attrs.0.computed_max_mana();
        attrs.0.current_life = max_life;
        attrs.0.current_mana = max_mana;
        attrs.0.effects.clear();

        // Interrupt any in-flight cast so the new round starts clean.
        commands.entity(entity).interrupt_cast();

        // Respawn at the fixed marker for this player's slot; zero velocity so the Dynamic body
        // doesn't carry momentum (or a fall) into the new round.
        let slot = slot_of
            .get(&entity)
            .copied()
            .unwrap_or(0)
            .min(SPAWN_MARKERS.len() - 1);
        let spawn = SPAWN_MARKERS[slot];
        position.0 = spawn;
        lin_vel.0 = Vec3::ZERO;

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
    use super::cast_phase_byte;
    use obelisk_bevy::prelude::SkillPhase;

    /// The phase→byte mapping the client decodes (`NetworkedPosition.cast_phase`): no cast → 0,
    /// Windup → 1, Active → 2, Recovery → 3; the terminal `Done` (obelisk removes ActiveCast on it)
    /// collapses to 0. Pins Bug 1a's wire contract.
    #[test]
    fn cast_phase_byte_maps_each_phase() {
        assert_eq!(cast_phase_byte(None), 0);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Windup)), 1);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Active)), 2);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Recovery)), 3);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Done)), 0);
    }
}
