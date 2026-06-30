# `arena_game` — architecture (post lightyear-native migration)

`arena_game` is a 1v1 online wizard duel: **Bevy 0.18.1 + Avian3d 0.5 + lightyear 0.26.4**, with combat owned by `obelisk-bevy`. It is **server-authoritative** with **lightyear-native client prediction + interpolation**. Movement is a Dynamic-body FORCE controller (`shared_controller.rs`) driven by native `ActionState<ArenaInput>`; each player replicates as a lightyear `Predicted` entity (the owner) + `Interpolated` entities (everyone else).

> The pre-migration hand-rolled netcode is GONE: `NetworkedPosition` pose stream, `PlayerInputMessage`/`InputChannel`, the kinematic `run_player_controller`/`sync_player_positions`/`drain_player_inputs`, `prediction.rs`, `replication.rs`, `materialize_replicated_players`, and `LinkStart`. If you find an in-code comment naming any of these, it is stale — fix it.

## Crate + binary layout

- `crates/arena_game/src/` — the shared lib (`lib.rs` re-exports `client`, `net`, `server`, `shared_controller`, `skills`, `trace`).
- `crates/arena_game/src/bin/` — `arena-server` (headless authority), `arena-client` (windowed by default; `ARENA_HEADLESS=1` → headless), `arena-observer` (thin alias to `client::run_headless_client`).
- `crates/arena_skills/src/lib.rs` — the lightyear-free `.skillfx.ron` cosmetic-binding format + registry (`SkillFx`, `SkillFxRegistry`, serde `CueMessage`, pure `cue_event_to_message`/`resolve_cue`, `ArenaSkillsPlugin`). `arena_game` owns every lightyear wrapper around it.

## Load-bearing plugin composition order

Identical on server (`bin/server.rs:20-58`) and windowed client (`client/mod.rs:88-216`); get it wrong and you get a `PhysicsSchedulePlugin already added` panic or replication that silently never flows.

1. Base plugins + `Time::<Fixed>::from_hz(60.0)`. Server/headless: `MinimalPlugins` + `LogPlugin` + `AssetPlugin` + `TransformPlugin` + `MeshPlugin` + `ScenePlugin` (avian's collider cache needs the asset/mesh/scene plugins even with no rendering). Windowed: `DefaultPlugins` with `AssetPlugin.file_path = <root>/assets`.
2. **lightyear net stack FIRST** — `ServerNetPlugin`/`ClientNetPlugin`, which add `ProtocolPlugin` + `TracePlugin`. Then reapply `ConnectTo`/`ServerBind` from `parse_addr_args(default)`.
3. `add_avian_with_lightyear(&mut app)` **after** the net stack — the *sole* avian `PhysicsPlugins` registrant (`lib.rs:54`).
4. obelisk sim: `add_obelisk_sim_headless` (server) / `add_obelisk_sim_client` (client) — both **omit `ObeliskSpatialPlugin`** (else the physics group double-adds and panics). The client variant additionally omits `ObeliskCombatPlugin` + `detect_overlaps` (Stage A).
5. obelisk config/effects/skills + `seed_combat_rng(session_seed())` (server) / `seed_combat_rng(1)` (client, never drawn).
6. Server: `skills::register_server_cue_egress` + `ArenaServerPlugin`. Client: `ArenaSkillsPlugin`, controller/present/parts/customization/hud plugins, `ClientNetPlayerPlugin`, frame-interpolation, cue-binding + predicted-cast + event-trace, input bridges.

## Module responsibility table

### `crates/arena_game/src/`
| File | Responsibility (now) |
|---|---|
| `lib.rs` | `arena_root()`; `add_avian_with_lightyear` (sole `PhysicsPlugins` registrant, `AvianReplicationMode::Position`, `PhysicsPlugins::new(FixedUpdate)` with Transform/Interpolation/Island plugins disabled, arena `Gravity`); `spawn_arena_floor` (static cuboid, top face at world 0); `add_obelisk_sim_headless` vs `add_obelisk_sim_client` (latter omits `ObeliskCombatPlugin`/ResolveHits — Stage A); the `refresh_spatial_pipeline`/`refresh_spatial_pipeline_pre_detect` systems required under `LightyearAvianPlugin`. |
| `shared_controller.rs` | The shared force controller, run on BOTH peers: `apply_arena_movement` (planar `move_towards` force `* mass` + grounded jump impulse) + `apply_arena_yaw` (writes avian `Rotation`). Consts `MAX_SPEED=4`, `MAX_ACCELERATION=30`, `JUMP_SPEED=7`. Ground check = body world-Y `<= GROUND_Y + 0.05` (no raycast). |
| `skills.rs` | Server cue/event egress (`register_server_cue_egress`: `capture_cue_event` observer → `PendingCues` → `broadcast_cues` → `CueWireMessage`; `egress_net_events` → `NetEventMessage`); client event trace (`register_client_event_trace`/`trace_received_net_events`); client cue binding (`register_client_cue_binding`/`consume_replicated_cues` — the SINGLE `CueWireMessage` drain + de-dup of the local OnCast); predicted local cast (`register_predicted_sim`/`predicted_local_cast`). |
| `trace.rs` | `ARENA_TRACE_FILE` JSONL structured tracing (`event(kind, extra)`, `src` from `ARENA_TRACE_SRC`), the cross-process test substrate. `TracePlugin` emits a `start` sentinel. |
| `net/mod.rs` | Cross-peer constants: `TICK_HZ=60`, `GROUND_Y=0.59`, `ARENA_EYE_HEIGHT=0.5`, `GRAVITY=20`, `PROTOCOL_ID=1`, `NETCODE_KEY`; `parse_addr_args`, `default_server_addr`, `session_seed`. The de-facto shared-tuning home. |
| `net/protocol.rs` | `ProtocolPlugin`: native `InputPlugin::<ArenaInput>`; the replicated component set + prediction/interpolation/rollback registration; channels (`CastChannel` C→S, `EventChannel` S→C); messages; the four `*_should_rollback` threshold fns (0.01 epsilons). |
| `net/input.rs` | `ArenaInput { movement: Vec2, yaw: f32, jump: bool, charging: bool }` — the native per-tick input. |
| `net/client.rs` | `ClientNetPlugin`: `ClientPlugins{1/60}` + `ProtocolPlugin`; spawns the netcode client entity (`Client`+`Link`+`PredictionManager`+`ReplicationReceiver`+…); `ConnectTo` (`ARENA_CLIENT_ID` or wall-clock-nanos id). |
| `net/server.rs` | `ServerNetPlugin`: `ServerPlugins{1/60}` + `ProtocolPlugin`; `spawn_server` triggers **`Start`** (NOT `LinkStart`); `on_new_link` attaches `ReplicationSender::new(100ms, SinceLastAck, false)` per `LinkOf`; `ServerBind`. |
| `server/mod.rs` | `ArenaServerPlugin`: `spawn_player_on_connect` observer; authoritative `server_apply_yaw`/`server_apply_movement` (FixedUpdate, `With<NetworkedPlayer>, Without<Predicted>`); `drain_cast_requests`; `sync_cast_state`; `sync_networked_health`; `drain_customize_requests`; cast-timeline load/poll; `trace_server_pose`/`trace_server_net_events`; the best-of-3 round machine. Resources `ClientPlayerMap`, `NetworkedIdAlloc`, `RoundState`. |
| `bin/{server,client,observer}.rs` | The three process entry points. |

### `crates/arena_game/src/client/`
| File | Responsibility (now) |
|---|---|
| `mod.rs` | `run_windowed_client` + `run_headless_client`; input bridges (`bridge_windowed_input_to_local_input`, `bridge_windowed_cast_hold`, `release_keys_on_focus_loss`); headless `[H]` hooks (`automove_input`, `headless_autocast`, `headless_customize_once`, `trace_replicated_*`); `add_frame_interpolation_to_predicted`; scene/rig/cast-asset setup; screenshot + smoke harness; ~15 `ARENA_*` env hooks. |
| `net.rs` | `ClientNetPlayerPlugin`: `materialize_predicted_players` (local Dynamic body + `InputMarker`/`ActionState` + `LocalNetPlayer`) vs `materialize_interpolated_players` (no body, waits for `Position`+`Rotation`); `buffer_arena_input` (FixedPreUpdate `WriteClientInputs`); predicted `client_apply_yaw`/`client_apply_movement` (`With<Predicted>`); `send_cast_requests` (+`PredictedCast`); `send_customization`; `drain_customize_broadcasts`; remote pose/cast-phase traces. Resources: `LocalInput`, `CastIntent`, `ChargeState` (`MAX_CHARGE_SECS=1.5`, `pending_charge` default 85), `CustomizeDirty`. |
| `controller.rs` | `ArenaControllerPlugin`: mouse-look → `CameraYaw`/`AimPitch` (`MOUSE_SENSITIVITY=0.0035`, `PITCH_LIMIT=85°`); `follow_local_net_player` (first-person camera at `EYE_HEIGHT = ARENA_EYE_HEIGHT` on the `LocalNetPlayer`); `apply_aim_pitch_to_local_spine` (REMOTE-only `chest_joint` lean). **It no longer moves a Transform** — prediction owns the body. |
| `present.rs` | `attach_rig_to_players`: hang the `character.glb` `ArenaBody` rig under each materialized player (`RIG_FOOT_OFFSET = -0.62`, π gltf-yaw) + insert `LocalAnimBlend`; tag the local body `LocalPlayerBody` + `Visibility::Hidden`; `hide_local_player_body` enforces it. |
| `rig.rs` | `RigAssets` + `build_graph_when_loaded` (one `AnimationGraph` from named glb clips) + `attach_animation_graph`; per-player `drive_animation` (locomotion from `LinearVelocity`+`Rotation` for remotes / camera-yaw+zero-vel for the hidden local; casting from `NetworkedCastState.cast_phase`, local pre-empts on charge). `LOCOMOTION_REF_SPEED=3.5`, `WALK_MIN_SPEED=0.2`. |
| `parts.rs` | `PartSelection` (7 `u8` slots) + variant tables; `apply_arena_part_visibility` toggles per-mesh `Visibility` (local rig reads the local `PartSelection` resource, each remote rig reads its replicated `PlayerCustomization`); `PartMesh` cache + `refresh_arena_part_visibility_on_change`. |
| `cosmetics.rs` | `LocalCue` → emissive billboards + flying `CosmeticProjectile` (NON-authoritative); `AimDirs`; `MUZZLE_HEIGHT_OFFSET=Vec3(0,1.2,0)`; `fly_cosmetic_projectiles` + `age_lifetimes`. |
| `customization.rs` | `K`-toggled customizer panel + third-person orbit preview (`CustomizationOpen`, `ORBIT_SPEED=2.2`); sets `CustomizeDirty` on close. |
| `hud.rs` | HP bars (from `NetworkedHealth`), floating damage + hit flash (from `DamageResolved`), round banner (from `RoundStateMessage`), charge bar (from `ChargeState`), crosshair. Windowed-only. |

## Netcode: connection → spawn → replication

1. Server boots; `spawn_server` triggers **`Start`** (`net/server.rs:70`). `Start` (not `LinkStart`) adds the `Started` component that `Replicate::on_insert` hard-requires to register newly-spawned entities to already-connected clients; with only `LinkStart`, the first player never replicates to its own client.
2. Each new client link (`On<Add, LinkOf>`) gets `ReplicationSender::new(100ms, SinceLastAck, false)` (`net/server.rs:77-86`) — NOT `default()`.
3. On `On<Add, Connected>`, `spawn_player_on_connect` (`server/mod.rs:162`) spawns ONE obelisk combatant per client: `make_combatant(StatBlock::with_id("player_{id}"))` + `Faction` (slot 0 → `Player`, else `Enemy`) + `grant_skill("firebolt")` + a CHILD `Hurtbox` sensor + the networked component set + `RigidBody::Dynamic` `Collider::capsule(0.35, 0.48)` (rotation locked, friction 0) + `Replicate::to_clients(All)` + `PredictionTarget::Single(owner)` + `InterpolationTarget::AllExceptSingle(owner)` + `ControlledBy`. Spawning in the OBSERVER (not a polled system) guarantees the owner's replication sender exists before the targets resolve. The slot comes from this client's position in the SORTED list of currently-connected ids (matches `reset_for_new_round`).
4. lightyear materializes a **`Predicted`** entity on the owner's client and **`Interpolated`** entities elsewhere. `client/net.rs` attaches a Dynamic body + input marker to the Predicted one and nothing physical to the Interpolated ones (lightyear drives their `Position`/`Rotation`).

### What replicates (`net/protocol.rs:37-85`)
- Identity: `NetworkedPlayer`, `NetworkOwner(client_id u64)`, `NetworkedId(u64)` (stable cross-peer key, the harness correlation key), `ObeliskNetId(String)` (stable combat id).
- Pose: avian `Position` + `Rotation` — predicted (rollback) AND interpolated AND linearly corrected.
- Velocity: `LinearVelocity` + `AngularVelocity` — predicted + rollback only, **not** interpolated (they don't impl `Ease`).
- `NetworkedCastState { cast_phase, cast_skill }` — discrete, snapped (no interpolation). Drives the opponent's cast animation.
- `NetworkedHealth { current: f64, max: f64 }` — discrete, snapped. The HUD source of truth.
- `PlayerCustomization { parts }` — initial-value replication is reliable; **live edits do NOT trust component-update replication**, they ride the reliable `CustomizeBroadcast`.

### Channels + messages (`net/protocol.rs:87-120`)
- `CastChannel` (C→S, reliable): `CastRequestMessage { skill_id, aim_dir:[f32;3], charge:u8 }`, `CustomizeMessage { parts }`.
- `EventChannel` (S→C, reliable): `NetEventMessage` (wraps obelisk `NetEvent`), `CueWireMessage` (wraps `arena_skills::CueMessage`), `RoundStateMessage`, `CustomizeBroadcast { player, parts }`.

### Rollback (`net/protocol.rs:260-274`)
Per-component `*_should_rollback` with 0.01 epsilons (matches the canonical `avian_3d_character` example). Not reflexive — comparing a value to itself returns false, so only a real `>= 0.01` divergence triggers a rollback.

## Movement: prediction + rollback

A Dynamic-body force controller shared verbatim by both peers and re-run by lightyear during rollback.

1. Input source → `LocalInput` resource: windowed `bridge_windowed_input_to_local_input` (`client/mod.rs:329`, WASD + `CameraYaw`/`AimPitch`, camera-relative: forward = -Z, strafe = +X in the yaw frame) or headless `automove_input` (`client/mod.rs:645`).
2. `buffer_arena_input` (`client/net.rs:171`) copies `LocalInput` + `ChargeState.charging` onto the Predicted entity's `ActionState<ArenaInput>` in `FixedPreUpdate`/`InputSystems::WriteClientInputs` — lightyear samples it there and ships it.
3. Controller execution (FixedUpdate, chained yaw-then-movement on both peers): server `server_apply_yaw`/`server_apply_movement` (`With<NetworkedPlayer>, Without<Predicted>`); client `client_apply_yaw`/`client_apply_movement` (`With<Predicted>`). `apply_arena_movement` accelerates planar velocity toward `move_dir * MAX_SPEED` via avian's `move_towards`, applying `required_acceleration * mass`; a grounded jump applies an upward impulse to reach `JUMP_SPEED`. Yaw is a **separate** system because avian's `Forces` borrows `Rotation` internally.
4. Render smoothing: the local Predicted player gets `FrameInterpolate<Position/Rotation>` (`client/mod.rs:679`); Interpolated remotes are already smooth via lightyear.

Note: movement force is applied **unconditionally of ground state** (full-strength air control); only the jump is ground-gated. Friction is 0 so the controller fully owns planar velocity (releasing input decelerates at the full `MAX_ACCELERATION`).

## Cast pipeline

Combat is 100% server-authoritative (obelisk). The client only *requests* casts and *predicts cosmetics*.

1. `bridge_windowed_cast_hold` (`client/mod.rs:272`): LMB-hold accumulates `ChargeState.secs` (clamped to `MAX_CHARGE_SECS`); on release `pending_charge = (85 + frac*170)` (85 ≈ tap ≈1.0×, 255 = full hold 2.0×) and sets `CastIntent`.
2. `send_cast_requests` (`client/net.rs:301`): ships `CastRequestMessage` on `CastChannel`, where `aim_dir` is the camera-forward vector `Quat(Y,yaw)*Quat(X,pitch) * -Z`. It also emits a `PredictedCast` for zero-latency own-cast cosmetics, then clears the intent.
3. `drain_cast_requests` (`server/mod.rs:372`): resolves sender `RemoteId` → caster via `ClientPlayerMap`, skips a caster mid-`ActiveCast`, and fires `cast_skill_dir_charged_from(skill, dir, charge, Vec3::Y*ARENA_EYE_HEIGHT)`. Free aim from the eye, no auto-acquire — it can miss.
4. obelisk FixedUpdate sets resolve the rest: `validate_casts` (mana/cooldown/already-casting gate) → `advance_casts` → `move_projectiles` → `detect_overlaps` (hit → `DamageResolved`). Only the server runs `ObeliskSet::ResolveHits`.
5. Egress (`skills.rs`): `capture_cue_event` buffers obelisk `CueEvent`s (resolving `source` Entity → `ObeliskId`, stamping the caster `aim_dir`), `broadcast_cues` ships them as `CueWireMessage`; `egress_net_events` broadcasts `NetEvent` as `NetEventMessage`.
6. Client consume: `consume_replicated_cues` (`skills.rs:199`) is the SINGLE `CueWireMessage` drain — it de-dups the local player's own `OnCast` cue and forwards survivors as `LocalCue`. `predicted_local_cast` (`skills.rs:259`) plays the local on_cast windup + cosmetic projectile immediately (never a `Hitbox`, never `CombatRng`). `spawn_cue_cosmetics` (`cosmetics.rs:76`) turns each `LocalCue` into billboards + a flying `CosmeticProjectile`. Damage numbers come from the replicated `DamageResolved` in `hud.rs`.

## Rig / animation + customization

- **Attach** (`present.rs`): `attach_rig_to_players` hangs the `character.glb` scene (`ArenaBody`, π gltf-yaw, `RIG_FOOT_OFFSET=-0.62`) under every materialized `NetworkedPlayer` + inserts `LocalAnimBlend`. The local body is `LocalPlayerBody` + `Visibility::Hidden` (first-person).
- **Animate** (`rig.rs`): `build_graph_when_loaded` builds one `AnimationGraph` from the glb's named clips. `drive_animation` is **per-player** — it walks the `ChildOf` chain from each `AnimationPlayer` to its owning rig root and drives a locomotion layer (remotes blend from their replicated `LinearVelocity`+`Rotation` yaw; the hidden local rig uses camera yaw + zero velocity) + a casting layer (from `NetworkedCastState.cast_phase`: 1/2 → 1.0, 3 → 0.5, 0 → 0.0, eased; the local player pre-empts to 1.0 while charging).
- **Parts** (`parts.rs`): per-mesh `Visibility` from `PartSelection`. The local rig reads the local `PartSelection` resource (the customizer's edits); each remote rig reads its player's replicated `PlayerCustomization.parts`. Body skin / weapons / capes are categorically hidden.
- **Customization round-trip (D6)**: `K` opens the customizer; edits mutate the local `PartSelection`; on close `CustomizeDirty` → `send_customization` → server `drain_customize_requests` updates `PlayerCustomization` + broadcasts `CustomizeBroadcast` → every client's `drain_customize_broadcasts` applies it to the rig keyed by `NetworkedId`. Live edits use the broadcast, NOT component-update replication.

## Best-of-3 round machine (`server/mod.rs:639-975`)

Server-owned FSM (`RoundState`/`RoundPhase`), broadcast as `RoundStateMessage` on the reliable `EventChannel`:

`WaitingForPlayers` (until 2 players) → `Countdown(3s)` → `Active` → `RoundOver(2s, winner)` → next `Countdown`, or `MatchOver` once a player reaches `ROUND_WINS_TO_MATCH = 2`.

- `detect_round_end` reads obelisk's `EntityDied` stream **only during `Active`**, collecting **all** deaths that tick (pure `round_outcome` helper): a single death credits the *survivor*; a same-tick **double-KO is a draw** (replay the round, credit no one) instead of crediting an arbitrary corpse; a stray non-player death leaves the round running. It still drains the stream in other phases so a stale death isn't mis-attributed.
- `run_round_machine` advances by real time and runs `reset_for_new_round` on the Countdown→Active rising edge: heal to max life/mana, clear effects, `interrupt_cast`, **re-assert each player's `Faction` by sorted client-id slot** (`faction_for_slot`), teleport both to `SPAWN_MARKERS` by the same slot, zero `LinearVelocity`. The Position reset replicates; the predicted owner rolls back to it. A mid-duel disconnect (player_count < 2) in `Active` or `Countdown` falls back to `WaitingForPlayers`. The `cleanup_player_on_disconnect` observer (`On<Remove, Connected>`) drops the client from `ClientPlayerMap` + its score entry and re-evaluates the phase.
- `broadcast_round_state` (`:946`) sends only when `dirty`, and won't clear `dirty` until at least one sender exists (so the initial state isn't lost pre-connect). Countdown re-broadcasts are throttled to ~1/sec.

Damage stays 100% obelisk-authoritative; this machine only reads deaths and resets state. The HUD renders it via `hud::update_round_label`.

## Key invariants + footguns

1. **Write avian `Position`, never `Transform`.** Spawn/reset set `Position` (`server/mod.rs:251,932`). `LightyearAvianPlugin` (`AvianReplicationMode::Position`) owns the `Position`↔`Transform` sync; a `Transform` write gets clobbered and won't replicate.
2. **Trigger `Start`, not `LinkStart`** (`net/server.rs:62-71`). `LinkStart` alone → the first player never replicates to its own client.
3. **`add_avian_with_lightyear` is the ONLY `PhysicsPlugins` registrant.** Both obelisk composers omit `ObeliskSpatialPlugin`; adding it (or `ObeliskSimPlugin`) double-adds the physics group and panics.
4. **Stage A: the client never resolves combat.** `add_obelisk_sim_client` omits `ObeliskCombatPlugin` and `detect_overlaps` — the client must never draw `CombatRng` or spawn a `Hitbox`. Damage is server-only.
5. **Hurtbox lives on a CHILD entity** (`server/mod.rs:283-291`) so the player root stays `RigidBody::Dynamic`. The child is a `Sensor` capsule with NO `RigidBody`, attached as a compound child collider that tracks the moving body.
6. **Force-refresh the `SpatialQueryPipeline`** (`lib.rs:260`, ordered before `ObeliskSet::Validate` and before `ResolveHits`). Under `LightyearAvianPlugin` the physics-set reshuffle leaves obelisk's spatial reads seeing an EMPTY pipeline — without this, firebolts fly straight through hurtboxes and never resolve.
7. **Trace `extra` fields must not use the key `kind`** — use `cue_kind` etc. The harness merges `extra` over a base object carrying the top-level `kind`, so a `kind` field silently clobbers the event type.
8. **Single-drain rule:** `MessageReceiver::receive()` drains; only `consume_replicated_cues` (`skills.rs:199`) may drain `CueWireMessage`. A second reader steals cues.
9. **obelisk is the authority** for casts/damage/death; identity correlation across peers uses `NetworkedId`/`ObeliskNetId`, never local `Entity`.
10. **`Collider::capsule(0.35, 0.48)` is duplicated in 3 spawns** (server body `:256`, server hurtbox `:287`, client predicted body `net.rs:231`) and MUST stay identical or prediction/hurtbox desync. Player mass is implicit (avian default density) and intentionally cancels in the controller (`force = accel*mass`, `impulse = dv*mass`) — any future knockback/external impulse must account for this.
11. **Faction is assigned by sorted-client-id slot** (`faction_for_slot`: slot 0 → Player, else Enemy) at connect AND **re-asserted every round** in `reset_for_new_round`. Because the slot comes from the *sorted* id list (not connect order), the two duelists land on slots 0/1 and can NEVER share a faction — a shared faction would make firebolt's `Enemies` filter resolve zero hits and hang the match. Any change to the slot derivation must keep spawn and reset using the *same* sorted-id slot.
12. **Flat-floor ground check.** `grounded = pos_y <= GROUND_Y + 0.05` assumes a single flat floor; stacked bodies / knock-up / ramps break it silently.

## Net-test harness (`crates/arena_game/tools/net-test/`) — do not break

The objective headless regression gate. `run_session.sh` launches `arena-server` + two `arena-observer` headless clients (the `arena-client` `ARENA_HEADLESS=1` path), each with distinct `ARENA_TRACE_FILE`/`ARENA_TRACE_SRC`. Observer-0 (`ARENA_CLIENT_ID=1`, `ARENA_AUTOCAST=1`, `ARENA_AUTOMOVE=1`, `ARENA_CAM_YAW=-1.5707963`) scripts firebolts at observer-1 (`ARENA_CLIENT_ID=2`); `summarize.py` asserts over the merged per-process JSONL that:
- the server emits `server_net_cast_began(caster=player_1)` + `server_net_damage_resolved(caster=player_1, target=player_2)`,
- BOTH observers emit a matching `client_net_cast_began` AND `client_net_damage_resolved`,
- the echoed `total_damage` matches the server's authoritative number.

It resolves obelisk ids from the server's `player_spawned` events (keyed by `client_id`). Knobs: `ARENA_NET_TEST_DURATION` (default 8), `ARENA_MATCH_SEED` (default 42 → deterministic damage), `ARENA_SKIP_BUILD=1`.

**The gate depends on** (a) the headless env hooks in `client/mod.rs` (`ARENA_AUTOCAST`/`ARENA_AUTOMOVE`/`ARENA_CUSTOMIZE`/`ARENA_CAM_YAW`/`ARENA_TEST_PITCH`/`ARENA_CLIENT_ID`), (b) the `trace::event` kinds emitted across `server/mod.rs`, `client/net.rs`, and `skills.rs` (`player_spawned`, `server_net_cast_began`, `server_net_damage_resolved`, `client_net_cast_began`, `client_net_damage_resolved`, `materialized_player`, `replicated_player`, `remote_pose`, `client_hp`, `client_round_state`, …), and (c) the `arena-observer` alias staying in sync with `run_headless_client`. Renaming/removing any trace kind or env hook, or moving combat resolution to the client, fails the gate. Run with `bash crates/arena_game/tools/net-test/run_session.sh`.