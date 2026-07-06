# Arena Netcode Feel Overhaul — Design

**Goal:** make the 1v1 duel feel responsive and smooth at real internet latency (target: excellent at 40–80 ms RTT, playable at 120 ms + jitter), using lightyear 0.26.4's native feature set end-to-end — no hand-rolled networking. Emphasis: player feel — your own movement and casts must feel instant; the opponent must move smoothly and telegraph casts immediately.

**Status:** implemented on `feat/netcode-feel-overhaul` (plan + deviation notes: `docs/superpowers/plans/2026-07-05-netcode-feel-overhaul.md`).

---

## 1. Where we are (verified against source, 2026-07-05)

The post-migration arena (`crates/arena_game`) already runs lightyear-native netcode: native `ActionState<ArenaInput>` input, server-authoritative avian `Position`/`Rotation` replication, client prediction + rollback for the local player (`PredictionTarget::Single(owner)`), lightyear interpolation for the opponent (`InterpolationTarget::AllExceptSingle(owner)`), `FrameInterpolation` on the local body, rollback epsilons + linear correction registered. Combat (obelisk) is 100 % server-authoritative; casts ride a reliable `CastRequestMessage`; cues/events ride reliable messages back.

### Measured/verified feel problems

| # | Problem | Evidence | Felt as |
|---|---------|----------|---------|
| P1 | Opponent rendered **~170 ms + jitter in the past**: interpolation delay = `max(send_interval × 1.7, 5 ms)` (`lightyear_interpolation-0.26.4/src/timeline.rs:64-68`) and the server sends at 100 ms (`net/server.rs:28` `REPLICATION_SEND_HZ = 10`) | source | opponent teleport-y at 10 Hz between snapshots, reacts late, dodges after the bolt passes |
| P2 | Opponent pose updates at 10 Hz; `NetworkedHealth` / `NetworkedCastState` mirrors also at 10 Hz | `net/server.rs:85-94` | choppy remote motion; HP bar and opponent windup lag ≥100 ms |
| P3 | Own cast's bolt appears **≥ RTT late**: only the `on_cast` muzzle cue is predicted (`skills.rs::predicted_local_cast`); the bolt cosmetic waits for the server's `on_window_*` cue | `client/cosmetics.rs`, `skills.rs:264` | your firebolt visibly launches late; worse with tap casts |
| P4 | Cast requests take a reliable-message round trip and land on whatever server tick receipt happens to hit | `net/protocol.rs:91-96`, `server/cast_pipeline.rs` | cast timing jitter; lost packet = full reliable-resend delay |
| P5 | First-person camera reads the body `Transform` in `Update`, **before** `FrameInterpolationSystems::Interpolate → PhysicsSystems::Writeback` run in `PostUpdate` (`lightyear_avian3d-0.26.4/src/plugin.rs:168-179`) | `client/controller.rs:150-152,244-260` | camera is a frame stale + not frame-interpolated: micro-stutter when strafing |
| P6 | Remote spine lean uses the **local** player's `AimPitch` on the opponent's rig (placeholder) | `client/controller.rs` doc + `apply_aim_pitch_to_local_spine` | opponent's aim doesn't telegraph where they're actually aiming |
| P7 | No collider on the interpolated opponent → the local predicted body walks through them, then the server resolves the collision → rubber-band | `client/net.rs:295-321` | shove-through pops |
| P8 | Round-reset teleport rides rollback + `VisualCorrection` exponential decay (50 %/200 ms, `lightyear_prediction-0.26.4/src/correction.rs:196-204`) | `server/rounds.rs::reset_for_new_round` | body glides across the arena at round start instead of snapping |
| P9 | Sub-tick input taps can be missed: `LocalInput.jump`/`charging` are level-sampled in `FixedPreUpdate` from `Update`-written state | `client/app_windowed.rs:288-328`, `client/net.rs:204-218` | rare eaten jumps/taps at high render FPS |
| P10 | **No latency-testing tooling**: no link conditioner, so all feel judgments happen at localhost RTT ≈ 0 | grep | "works on my machine" netcode |
| P11 | Hand-rolled `CustomizeBroadcast` S→C path exists because "component updates are unreliable" — inherited wisp lore; in 0.26.4 replication writes updates directly onto the (single) Predicted/Interpolated entity, and `SinceLastAck` resends until acked | `net/protocol.rs:57-59,116-122`; `lightyear_replication-0.26.4/src/receive.rs:745-790` | dead weight + wrong mental model in the docs |

### What wisp actually is (research verdict)

`../wisp` was explored as the reference. **Wisp does not use lightyear prediction/interpolation at all**: its avian components are registered with `ComponentReplicationConfig { disable: true }`, no entity ever gets `PredictionTarget`/`InterpolationTarget`, input is a hand-rolled per-frame `PlayerInputMessage`, remote smoothing is a hand-rolled two-sample interpolator, and the **local player is a Kinematic body whose position is copied back from the server** (movement lags a full RTT off-localhost). Its perceived responsiveness comes from (a) localhost testing, (b) fully local camera yaw/pitch, (c) instantly rendering your own ability visuals while the server applies effects. The arena is already ahead of wisp on netcode; we borrow wisp's *presentation* ideas (b) and (c) — the arena already has (b) — and take architecture from lightyear's own examples instead.

---

## 2. Goals / non-goals

**Goals**
1. Opponent motion smooth and near-present; their cast windup telegraphs the instant they start charging.
2. Own casts: muzzle + bolt + windup animation start with zero perceived latency; damage stays server-authoritative.
3. Cast intent delivery is tick-aligned, loss-tolerant, and cheat-resistant (charge computed from the input stream).
4. Player-vs-player body collision behaves under prediction.
5. Every change is lightyear-native (0.26.4 APIs that exist in the registry source); zero new hand-rolled replication.
6. Feel is validated **under artificial latency/jitter/loss** in the headless harness, not just localhost.

**Non-goals**
- Stage B (deterministic client-side obelisk combat, predicted `Hitbox` entities, `PreSpawned` projectile matching). The cosmetic layer fakes only visuals.
- Upgrading lightyear past 0.26.4 (post-tag main is a breaking rewrite: replicon-based replication, avian 0.6).
- Lag compensation (`LagCompensationPlugin`) — not needed once the opponent is *predicted* rather than interpolated (see §3); recorded as the fallback path.
- Bandwidth/priority management, interest management, WebTransport, auth — irrelevant at 2 players.
- Server main-loop pacing / CPU use (busy headless loop helps input latency; not a feel problem).

---

## 3. Approaches considered

**A. Tune-only.** Keep interpolated opponent; raise send rate to ~30–60 Hz; tune `InterpolationConfig` (delay ≈ 28–56 ms); fix camera/reset/edge-latch; add conditioner. Lowest risk, but: opponent windup still delayed by delay+mirror cadence, no body collision, hitscan (`chain_lightning`) still aims at a stale opponent → needs `LagCompensationPlugin` to be fair, and casts stay message-timed.

**B. Predict both players + casts in the input stream (chosen).** The `avian_3d_character` pattern (the file this codebase already mirrors): `PredictionTarget::to_clients(NetworkTarget::All)`, `InputConfig { rebroadcast_inputs: true }`, remote predicted players get real physics bodies and are driven by the *same* shared controller from their rebroadcast inputs. Move cast/aim/charge into `ArenaInput`. Opponent appears at your predicted tick (no interpolation delay at all); their charging telegraph is instant; bodies collide in-sim; hitscan aims at near-present state so lag comp becomes unnecessary. Costs: opponent pose is an *extrapolation* under the hood (their last-received input is held via `SameAsPrecedent`), so a sharp reversal inside your RTT shows as overshoot that `CorrectionPolicy` smooths; more rollbacks (cheap at this entity count).

**C. B's cast/presentation work + interpolated opponent + `LagCompensationPlugin`** (the projectiles-example `ClientPredictedLagComp` mode). "True past" opponent (no extrapolation artifacts) at the cost of 30–60 ms visual delay, lag-comp plumbing, no predicted body collision, and slower telegraphs. This is the **documented fallback** if B's extrapolation artifacts feel worse than delay under the conditioner — the protocol keeps `.add_linear_interpolation()` registrations so flipping back is a targeting + config change, not a rework.

**Why B:** it is the canonical lightyear answer for a small fast game (every fast example either predicts remotes or pairs interpolation with lag comp; none raises the send rate), it maximizes what the user asked for (responsiveness, full lightyear feature use), and it deletes complexity (no lag comp, no interpolation-delay engineering). Verified in registry source: `receive_remote_player_input_messages` (`lightyear_inputs-0.26.4/src/client.rs:578`) is generic over the input plugin, so native input supports rebroadcast; missing ticks fill as `SameAsPrecedent` (`input_buffer.rs:366`).

---

## 4. Design

### WS1 — Predict both players (architecture core)

- `net/protocol.rs`: `input::native::InputPlugin::<ArenaInput> { config: InputConfig { rebroadcast_inputs: true, ..default() } }`. Keep `packet_redundancy` default (5), keep zero input delay (`InputTimelineConfig` default `no_input_delay`) — knob documented for later tuning, not set.
- `server/spawn.rs`: `PredictionTarget::to_clients(NetworkTarget::All)`; **delete** `InterpolationTarget`. Keep `ControlledBy`.
- `client/net.rs`: `materialize_predicted_players` already attaches the Dynamic body per `Predicted` player and gates `InputMarker`+`LocalNetPlayer` on `Controlled` (matches lightyear #1431 guidance). Extend: non-controlled predicted players must also carry `ActionState::<ArenaInput>::default()` (insert client-side if not replicated) so the rebroadcast `InputBuffer` has a component to drive. **Delete** `materialize_interpolated_players`.
- The predicted controller systems (`client_apply_yaw`/`client_apply_movement`, `With<Predicted>`) now drive both players — no query change needed. Server systems unchanged.
- `FrameInterpolate<Position/Rotation>` observer already fires per `Predicted` — now covers the opponent too (needed: remotes step at 60 Hz sim, render must interpolate).
- Keep `.add_linear_interpolation()` component registrations (fallback C stays one edit away).
- Update `LocalPlayerFilter`/`RemotePlayerFilter` aliases, rig attach filters, and traces that assumed `Interpolated` (e.g. `trace_received_remote_pose` keys off `Without<LocalNetPlayer>` — still correct).
- `net/server.rs`: `REPLICATION_SEND_HZ` 10 → **30** (33 ms). Not for interpolation (none left on pose) but for: rollback mispredict detection cadence, and the `NetworkedHealth`/`NetworkedCastState`/`PlayerCustomization` mirror latency (P2). Bandwidth at 2 entities is trivial. Keep `SendUpdatesMode::SinceLastAck`.

**Spike gate (first implementation step):** two headless observers + server; assert via traces that each observer's *remote* player entity is `Predicted`, receives fresh `ActionState` ticks (trace `remote_pose` freshness ≤ 3 ticks behind local tick), and moves under `ARENA_AUTOMOVE`. If native-input rebroadcast proves broken in 0.26.4, stop and pivot to approach C (decision recorded here; everything else in this design except WS1's targeting survives the pivot).

### WS2 — Casts ride the input stream

`ArenaInput` v2 (in `arena_sim::input`, wire-breaking is fine — single repo, protocol shared by both peers):

```rust
pub struct ArenaInput {
    pub movement: Vec2,
    pub yaw: f32,
    pub pitch: f32,        // NEW: aim pitch (drives cast ray + remote spine lean)
    pub jump: bool,
    pub charging: bool,    // existing: cast button held
    pub skill_slot: u8,    // NEW: selected skill index into ARENA_SKILLS
}
```

- **Cast = falling edge of `charging`** (true→false between consecutive ticks), detected server-side per player from `ActionState<ArenaInput>` with a tiny `PrevArenaInput` component. Edge encoding sidesteps lightyear 0.26.4's known collapsed-`just_pressed` bug (#1438) and survives packet loss: `SameAsPrecedent` fill means a lost release tick delays the cast 1–2 ticks, never loses it.
- **Charge byte computed on the server** from held-tick count: `frac = (hold_ticks / (MAX_CHARGE_SECS × TICK_HZ)).min(1)`, then the existing `charge_byte_from_frac`. The wire byte disappears; client HUD keeps computing the identical value locally from its own hold time (`ChargeState` stays, `pending_charge` plumbing dies). Cheat-resistant: charge derives from the same input stream the movement does.
- **Aim ray from yaw+pitch** in the input, reconstructed by a shared helper (`net/mod.rs`: `aim_dir(yaw, pitch) -> Vec3`, the current quat math from `send_cast_requests`) used by the client camera, the client predicted cast, and the server cast pipeline. The wire `aim_dir: [f32;3]` dies.
- **`ARENA_SKILLS: &[&str]`** shared const in `net/mod.rs` (the shared-tuning home): server grant loop, windowed key map (`skill_for_key`), and slot→id resolution all read it. `skill_slot` out of range or ungranted → cast ignored (obelisk `validate_casts` double-gates anyway).
- `server/cast_pipeline.rs`: `drain_cast_requests` (message drain) → `detect_cast_edges` (input edge system, FixedUpdate, ordered before `ObeliskSet::Validate` like today). `resolve_cast_aim`, `PendingCast`, obelisk validation: unchanged.
- Client: `send_cast_requests` no longer sends anything — the same local falling edge (from `ChargeState`) emits `PredictedCast` for cosmetics, tick-aligned in `FixedUpdate`. **Delete** `CastRequestMessage`; rename `CastChannel` → `RequestChannel` (reliable C→S), which keeps carrying `CustomizeMessage`.
- Harness: `ARENA_AUTOCAST` paths stop setting `CastIntent` and instead pulse `charging` for one fixed tick (autocast cadence unchanged, `ARENA_AUTOCAST_SKILL` maps id→slot). `CastIntent` resource dies.
- P9 fix (edge-latch): the windowed bridge accumulates `jump_pressed` / `charge released` edges into `LocalInput` with "consumed by `buffer_arena_input`" semantics, so a sub-tick tap always lands in exactly one tick's `ActionState`.

### WS3 — Predicted own-cast presentation (zero-latency casts)

Today only `on_cast` is predicted. Extend `skills.rs`:

- `predicted_local_cast` grows into a small **local cue scheduler**: on the local cast edge, read the skill's loaded `CastTimeline` and schedule its cue windows (`on_cast` now; `on_window_open`/`emit_*` at their authored tick offsets, charge-scaled exactly like the server's cue payloads — reuse `charge_mult`). Emitted as the same `LocalCue` messages the server path uses, so `spawn_cue_cosmetics`/`CosmeticProjectile` rendering is untouched.
- `consume_replicated_cues` de-dup extends from "local `OnCast` only" to **all cue kinds sourced from the local caster within the predicted window**, EXCEPT `OnEnd`/impact cues — endings stay server-authoritative (they carry `end_reason` and real impact position). The predicted cosmetic projectile is keyed so the server's `OnEnd` cue tears it down (this matches the existing `on_end_bolt` teardown path).
- **Fizzle cleanup:** if the server never echoes a matching cast (rejected: mana/cooldown/mid-cast), predicted cosmetics for that cast despawn after a timeout (≈ 0.5 s) — presentation-only, no gameplay divergence. (The client cannot fully predict obelisk validation in Stage A; a mispredicted windup that fizzles is acceptable and rare — cooldown/mana denials.)
- **Opponent windup telegraph:** remote cast/charging animation keys off the remote predicted entity's `ActionState<ArenaInput>.charging` (instant, from rebroadcast inputs); `NetworkedCastState` remains the authoritative phase source for active/recovery blends and the headless traces. P6 fix rides WS2's pitch: `apply_aim_pitch_to_local_spine` reads the **remote's replicated input pitch** instead of the local `AimPitch` resource.

### WS4 — Presentation correctness & polish

- **Camera (P5):** move `follow_local_net_player` to `PostUpdate`, `.after(PhysicsSystems::Writeback).before(TransformSystems::Propagate)` (the chain lightyear_avian configures — `plugin.rs:168-179`). Mouse yaw/pitch stay resource-driven (instant); only the translation source changes to the frame-interpolated, correction-applied Transform.
- **Round-reset snap (P8):** on the round-reset edge (client sees `RoundStateMessage` phase → Countdown), remove `VisualCorrection<Position>`/`VisualCorrection<Rotation>` from predicted players so the teleport snaps instead of gliding. Server side already writes `Position` correctly.
- **Correction tuning:** insert `PredictionManager { correction_policy, ..default() }` explicitly with the default 200 ms/0.5 decay as the starting point; tune under the conditioner (this is the knob that hides remote-input mispredicts in approach B).
- `pseudo_unique_client_id` and connection plumbing unchanged.

### WS5 — Protocol hygiene (de-hand-rolling)

- **Delete `CustomizeBroadcast`** (P11): live `PlayerCustomization` edits flow as plain component updates (single-entity model writes them directly; `SinceLastAck` resends until acked). `CustomizeMessage` (C→S, reliable) stays — that's idiomatic lightyear messaging. `drain_customize_broadcasts` becomes a `Changed<PlayerCustomization>` rig refresh (which `parts.rs` already implements — the broadcast applier just dies). Headless net-test asserts a live edit propagates.
- `EventChannel` + `NetEventMessage`/`CueWireMessage`/`RoundStateMessage`: unchanged (reliable server→client events are the right lightyear-native tool).
- CLAUDE.md netcode sections rewritten at the end (the "component updates are unreliable" lore dies; new §Movement/§Cast pipeline reflect input-driven casts + all-predicted players).

### WS6 — Latency-conditioned verification (the honesty layer)

- **Conditioner knobs:** `ARENA_NET_LATENCY_MS` / `ARENA_NET_JITTER_MS` / `ARENA_NET_LOSS` env vars on the client build the lightyear-native `LinkConditionerConfig` and attach it to the client `Link` (`Link::new(Some(RecvLinkConditioner::new(config)))` — `lightyear_link-0.26.4/src/conditioner.rs`). Zero-cost when unset.
- **jq summarizer:** `summarize.py` needs python3 (absent in this dev shell); add `summarize.jq`/`check_session.sh` implementing the same assertions so the gate runs locally AND in CI. `run_session.sh` + `summarize.py` stay untouched for the user's CI.
- **New assertions** (merged JSONL traces, wall-clock `ts` already shared):
  1. Existing: cast_began + damage_resolved echoed on both observers with matching totals (unchanged — the regression gate).
  2. Remote freshness: each observer's `remote_pose` for the moving opponent advances every ≤ 3 ticks worth of wall-clock under automove (the current 1-in-30 trace throttle is replaced by a per-emit tick stamp so the summarizer can measure cadence).
  3. Cast latency: observer-0's `predicted_cast` fires within 1 tick of its cast edge; the server's `server_net_cast_began` lands within (one-way + 2 ticks); opponent's `remote_cast_phase`/charging telegraph within (one-way + 2 ticks).
  4. Conditioned run (100 ms RTT, 20 ms jitter, 2 % loss): assertions 1–3 still hold with widened bounds; no panic, no desync (`client_hp` eventually equals server hp).
- **Unit tests:** cast-edge detection (incl. loss-fill edge), hold-ticks→charge mapping anchors, slot↔skill mapping, existing round-outcome tests keep passing.

---

## 5. Wire/protocol delta summary

| Item | Before | After |
|---|---|---|
| `ArenaInput` | movement, yaw, jump, charging | + pitch, + skill_slot |
| Cast delivery | `CastRequestMessage { skill_id, aim_dir[3], charge }` on reliable `CastChannel` | falling edge of `charging` in the input stream; charge/aim derived server-side |
| `CastChannel` | reliable C→S channel carrying casts + customize | renamed `RequestChannel`, carries only `CustomizeMessage` |
| Customization S→C | `CustomizeBroadcast` message | plain `PlayerCustomization` component updates |
| Player targeting | `PredictionTarget::Single(owner)` + `InterpolationTarget::AllExceptSingle(owner)` | `PredictionTarget::to_clients(All)` |
| Inputs | owner-only | `rebroadcast_inputs: true` (opponent's inputs drive their predicted body + telegraphs) |
| Send interval | 100 ms | 33 ms |
| Cues/events/rounds | reliable messages | unchanged |

Compatibility: single-repo lockstep protocol; no cross-version concerns. The net-test trace *kinds* all survive; `cast_request_sent` is renamed/re-pointed to the edge site (`cast_edge_sent`) with the summarizer updated in the same commit.

## 6. Risks & mitigations

1. **Native-input rebroadcast is example-unproven** (examples use leafwing/BEI). Mitigated: source-verified generic path; WS1 spike gate is implementation step 1; documented pivot to approach C.
2. **Extrapolation overshoot on the opponent** (inherent to predicting a human). Mitigated: `CorrectionPolicy` smoothing, 33 ms confirm cadence, conditioner A/B vs approach C before accepting.
3. **0.26.4 input bugs (#1438 etc.).** Mitigated: edge-encoded casts (no `just_pressed` reliance), `SameAsPrecedent` fill semantics, redundancy 5.
4. **Predicted-cast fizzle mispredicts** (client shows a windup the server rejects). Bounded: cosmetic-only, timeout cleanup, mana/cooldown denials are the only sources.
5. **net-test breakage** (the gate CLAUDE.md forbids breaking). Mitigated: gate runs (via jq summarizer) after every phase; trace kinds preserved; autocast harness reworked in the same commit as the input change.
6. **Round-reset under all-predicted players**: both bodies now roll back on reset on both clients. Covered by the WS4 snap fix + a conditioned round-transition assertion.

## 7. Execution order (phases = commits/gates)

0. Baseline: jq summarizer + green baseline net-test run (pre-change reference numbers recorded).
1. WS1 spike: rebroadcast + predict-all targeting + remote materialization; **gate** on remote-input freshness traces.
2. WS1 complete (send interval, filters, frame-interp coverage, trace/doc touch-ups) + net-test green.
3. WS2: input v2 + server edge pipeline + harness rework + delete message path + unit tests + net-test green.
4. WS3: predicted cue scheduler + de-dup extension + fizzle cleanup + telegraph-from-input + net-test cast-latency assertions.
5. WS4: camera PostUpdate, reset snap, explicit correction policy.
6. WS5: customize de-hand-rolling + protocol cleanup.
7. WS6: conditioner + conditioned gate + tuning pass (correction decay, send interval) + CLAUDE.md rewrite.

Each phase ends with: `cargo build` + `cargo test -p arena_game` + headless net-test (jq gate) green.
