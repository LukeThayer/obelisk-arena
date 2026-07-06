# Arena Netcode Feel Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the 1v1 duel responsive at real internet latency: predict BOTH players (avian_3d_character pattern), move casts into the native input stream, predict own-cast presentation, and verify under an artificial-latency conditioner — all lightyear-0.26.4-native.

**Architecture:** Spec at `docs/superpowers/specs/2026-07-05-netcode-feel-overhaul-design.md`. Both players become `Predicted` on every client (`PredictionTarget::to_clients(All)` + `InputConfig { rebroadcast_inputs: true }`); the opponent's body is simulated by the same shared force controller from their rebroadcast inputs. Casts become the falling edge of `charging` in `ArenaInput` (charge computed server-side from held ticks); the reliable cast message dies. The predicted-cast cosmetic layer schedules the authored timeline cues locally so your own bolt launches with zero added latency; damage stays 100 % obelisk/server-authoritative.

**Tech Stack:** Rust, Bevy 0.18.1, avian3d 0.5, lightyear 0.26.4 (`netcode, udp, avian3d, interpolation, input_native, prediction`), obelisk-bevy. Verify API claims against `~/.cargo/registry/src/*/lightyear_*-0.26.4/` (NOT the `~/src/lightyear` working tree — it is 45 commits ahead with an incompatible rewrite; use `git -C ~/src/lightyear show 0.26.4:<path>` for examples).

## Global Constraints

- **Never break the net-test harness contract** (`crates/arena_game/CLAUDE.md` §net-test): trace kinds `player_spawned`, `server_net_cast_began`, `server_net_damage_resolved`, `client_net_cast_began`, `client_net_damage_resolved`, `materialized_player`, `replicated_player`, `remote_pose`, `client_hp`, `client_round_state` must keep their names + fields. `summarize.py` stays untouched (CI uses it); local runs use the new jq gate.
- **Write avian `Position`, never `Transform`** (CLAUDE.md invariant 1).
- **Client never resolves combat** (Stage A): no `Hitbox`, no `CombatRng`, no `ResolveHits` on the client. Predicted casts are cosmetic-only.
- **Single-drain rule:** one `MessageReceiver::<M>::receive()` call site per message type per app. (Task 7 fixes the existing NetEventMessage double-drain.)
- **`Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH)` must stay identical at every spawn site** (server body, server hurtbox, client predicted body).
- 60 Hz fixed tick (`TICK_HZ = 60`) on both peers; `PROTOCOL_ID` bumps to 2 with the wire change (Task 4).
- Work on branch `feat/netcode-feel-overhaul` in this checkout (no worktree: cold bevy/lightyear builds cost 10+ min; the user's WIP — modified `Cargo.lock`, untracked `crates/arena_editor/assets/vfx/*.vfx.ron` — must never be staged or reverted).
- Run every net-test with `ARENA_SKIP_BUILD=1` after an explicit `cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer` so build failures surface in the task, not inside the harness.
- python3 is absent in this shell; `jq` is available. `bash crates/arena_game/tools/net-test/run_session.sh` will FAIL at the summarize step even when the session is good — always gate with `check_session.sh` (Task 1) instead.

---

### Task 1: Branch + jq gate (baseline)

**Files:**
- Create: `crates/arena_game/tools/net-test/check_session.sh`
- Modify: none

**Interfaces:**
- Produces: `bash crates/arena_game/tools/net-test/check_session.sh <session_dir>` → exit 0 + `PASS` iff the M2 gate assertions hold (the jq mirror of `summarize.py`). Later tasks call it after every session run.

- [ ] **Step 1: Create the branch**

```bash
cd /home/luke/src/obelisk-arena
git checkout -b feat/netcode-feel-overhaul
```

- [ ] **Step 2: Write the jq gate**

Create `crates/arena_game/tools/net-test/check_session.sh`:

```bash
#!/usr/bin/env bash
# jq mirror of summarize.py (python3 is unavailable in some dev shells). Asserts the M2 gate over
# a finished session dir produced by run_session.sh. Usage: check_session.sh [session_dir]
set -uo pipefail
session="${1:-/tmp/arena-net-test}"
fail=0
note() { echo "  - $*"; fail=1; }

server="$session/server.jsonl"; obs0="$session/observer-0.jsonl"; obs1="$session/observer-1.jsonl"
for f in "$server" "$obs0" "$obs1"; do
    [[ -s "$f" ]] || { echo "missing/empty trace: $f"; echo FAIL; exit 1; }
done

caster=$(jq -rs '[.[] | select(.kind=="player_spawned" and .client_id==1)][0].obelisk_id // empty' "$server")
target=$(jq -rs '[.[] | select(.kind=="player_spawned" and .client_id==2)][0].obelisk_id // empty' "$server")
[[ -n "$caster" && -n "$target" ]] || note "server did not spawn both players (caster='$caster' target='$target')"

# (1) server CastBegan(caster, firebolt)
n=$(jq -s --arg c "$caster" '[.[] | select(.kind=="server_net_cast_began" and .skill_id=="firebolt" and .caster==$c)] | length' "$server")
[[ "$n" -ge 1 ]] || note "server emitted no CastBegan(caster=$caster, firebolt)"

# (2) server DamageResolved(caster->target, firebolt) + capture the authoritative value
dmg=$(jq -rs --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="server_net_damage_resolved" and .skill_id=="firebolt" and .caster==$c and .target==$t)][0].total_damage // empty' "$server")
[[ -n "$dmg" ]] || note "server emitted no DamageResolved(caster=$caster, target=$target, firebolt)"

# (5) firebolt_explosion trigger fired end-to-end on the server
n=$(jq -s --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="server_net_damage_resolved" and .skill_id=="firebolt_explosion" and .caster==$c and .target==$t)] | length' "$server")
[[ "$n" -ge 1 ]] || note "server emitted no DamageResolved(firebolt_explosion)"

# (3)+(4)+(6) per observer: echoed cast + damage (matching value) + explosion
for name in observer-0 observer-1; do
    f="$session/$name.jsonl"
    n=$(jq -s --arg c "$caster" '[.[] | select(.kind=="client_net_cast_began" and .skill_id=="firebolt" and .caster==$c)] | length' "$f")
    [[ "$n" -ge 1 ]] || note "$name received no CastBegan"
    echoed=$(jq -rs --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="client_net_damage_resolved" and .skill_id=="firebolt" and .caster==$c and .target==$t)][0].total_damage // empty' "$f")
    [[ -n "$echoed" ]] || note "$name received no DamageResolved"
    if [[ -n "$echoed" && -n "$dmg" && "$echoed" != "$dmg" ]]; then
        note "$name echoed damage $echoed != server's $dmg"
    fi
    n=$(jq -s --arg c "$caster" --arg t "$target" '[.[] | select(.kind=="client_net_damage_resolved" and .skill_id=="firebolt_explosion" and .caster==$c and .target==$t)] | length' "$f")
    [[ "$n" -ge 1 ]] || note "$name received no DamageResolved(firebolt_explosion)"
done

echo "caster=$caster target=$target server_damage=$dmg"
if [[ "$fail" -ne 0 ]]; then echo FAIL; exit 1; fi
echo PASS
```

```bash
chmod +x crates/arena_game/tools/net-test/check_session.sh
```

- [ ] **Step 3: Run the baseline session and gate it**

```bash
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-baseline; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-baseline
```

Expected: `run_session.sh` itself exits non-zero (its python3 summarize step can't run) — ignore that; `check_session.sh` must print `PASS`. If it prints FAIL, STOP: the baseline is broken before any change — investigate before proceeding.

- [ ] **Step 4: Commit**

```bash
git add crates/arena_game/tools/net-test/check_session.sh
git commit -m "test(net): jq session gate — python3-free mirror of summarize.py"
```

---

### Task 2: Predict both players (WS1 spike)

**Files:**
- Modify: `crates/arena_game/src/net/protocol.rs:30-36` (input plugin config)
- Modify: `crates/arena_game/src/server/spawn.rs:176-184` (targeting)
- Modify: `crates/arena_game/src/client/net.rs` (materialization + spike trace)

**Interfaces:**
- Consumes: `ArenaInput` (unchanged this task), `ActionState<ArenaInput>` from `lightyear::prelude::input::native`.
- Produces: every player entity on every client is `Predicted` with a Dynamic body; non-controlled ones carry `ActionState::<ArenaInput>` fed by rebroadcast inputs. New trace kind `remote_input_seen { owner, movement }` (the spike gate). `materialize_interpolated_players` is GONE.

- [ ] **Step 1: Enable input rebroadcast in the shared protocol**

In `crates/arena_game/src/net/protocol.rs` replace the `InputPlugin` registration (lines 30-36):

```rust
        // --- Native input (lightyear ships `ActionState<ArenaInput>` per tick) ---
        // `rebroadcast_inputs: true`: the server relays each client's inputs to the other clients,
        // which lightyear applies to the matching remote PREDICTED entity's InputBuffer
        // (`lightyear_inputs-0.26.4/src/client.rs:578 receive_remote_player_input_messages`). The
        // arena predicts ALL players (the avian_3d_character pattern) — the opponent's body is
        // simulated by the same shared controller from these rebroadcast inputs.
        app.add_plugins(input::native::InputPlugin::<ArenaInput> {
            config: input::InputConfig::<ArenaInput> {
                rebroadcast_inputs: true,
                ..default()
            },
        });
```

- [ ] **Step 2: Target prediction at ALL clients on the server spawn**

In `crates/arena_game/src/server/spawn.rs` replace the targeting insert (lines 176-184):

```rust
        .insert((
            Replicate::to_clients(NetworkTarget::All),
            // Predict on EVERY client (avian_3d_character pattern): the owner predicts from its own
            // inputs; the opponent's client predicts this body from the server-rebroadcast inputs.
            // No InterpolationTarget — nothing is interpolated any more (design WS1; approach C in
            // the spec flips this back if extrapolation feel loses to delay under the conditioner).
            PredictionTarget::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: conn_entity,
                lifetime: Default::default(),
            },
        ))
```

Remove `InterpolationTarget` from the `use lightyear::prelude::{...}` list at line 14.

- [ ] **Step 3: Materialize remote predicted players**

In `crates/arena_game/src/client/net.rs`:

(a) In `materialize_predicted_players`, after the `if is_controlled { ... }` block (line 271-277), add the remote branch:

```rust
        } else {
            // Remote predicted player: no InputMarker (lightyear #1431 — marker only on the
            // authority entity), but it needs an ActionState for the rebroadcast InputBuffer to
            // drive (ActionState is not a replicated component; insert it locally).
            commands
                .entity(entity)
                .insert(ActionState::<ArenaInput>::default());
        }
```

(so the existing `if is_controlled { commands.entity(entity).insert((LocalNetPlayer, InputMarker..., ActionState...)); }` gains an `else`).

(b) Delete the whole `materialize_interpolated_players` system (lines 289-321) and its registration in `ClientNetPlayerPlugin` (line 190). Remove the now-unused `Interpolated` import (line 22).

(c) Add the spike-gate trace system and register it in `ClientNetPlayerPlugin`'s `Update` tuple:

```rust
/// One-shot spike gate (design WS1): trace the first time a REMOTE predicted player's
/// `ActionState<ArenaInput>` carries non-zero movement — proof that native-input rebroadcast
/// (server → this client) reaches the remote entity's input buffer. Cheap after latching.
fn trace_remote_input_once(
    remotes: Query<
        (&NetworkOwner, &ActionState<ArenaInput>),
        (With<Predicted>, Without<LocalNetPlayer>),
    >,
    mut seen: Local<bool>,
) {
    if *seen {
        return;
    }
    for (owner, action) in &remotes {
        if action.0.movement != Vec2::ZERO {
            *seen = true;
            crate::trace::event(
                "remote_input_seen",
                serde_json::json!({ "owner": owner.0,
                    "movement": [action.0.movement.x, action.0.movement.y] }),
            );
        }
    }
}
```

(d) Update the module doc comment (lines 1-16): remote players are now Predicted with bodies driven by rebroadcast inputs, not Interpolated.

- [ ] **Step 4: Build + run the spike gate**

```bash
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer 2>&1 | tail -5
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-spike; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-spike
grep -c remote_input_seen /tmp/arena-net-spike/observer-1.jsonl
```

Expected: `PASS`, and the grep prints ≥ 1 (observer-1 saw observer-0's rebroadcast movement on the remote predicted entity).

**DECISION GATE:** if `remote_input_seen` never fires on observer-1 while `check_session.sh` otherwise passes, native-input rebroadcast is broken in 0.26.4 → STOP this plan and pivot to spec approach C (interpolated opponent + faster send + `LagCompensationPlugin`); everything from Task 4 onward still applies after the pivot.

- [ ] **Step 5: Commit**

```bash
git add crates/arena_game/src/net/protocol.rs crates/arena_game/src/server/spawn.rs crates/arena_game/src/client/net.rs
git commit -m "feat(net): predict BOTH players — PredictionTarget::All + input rebroadcast"
```

---

### Task 3: 30 Hz replication + stale-filter cleanup (WS1 finish)

**Files:**
- Modify: `crates/arena_game/src/net/server.rs:25-28`
- Modify: `crates/arena_game/src/client/app_headless.rs:24-28` (filter docs), `crates/arena_game/src/client/net.rs:475-503` (remote pose trace doc)

**Interfaces:**
- Produces: `REPLICATION_SEND_HZ = 30` (33 ms confirm cadence). No API changes.

- [ ] **Step 1: Raise the send rate**

In `crates/arena_game/src/net/server.rs` replace lines 25-28:

```rust
/// Replication send rate (Hz). The per-client `ReplicationSender` flushes component updates at this
/// cadence. 30 Hz (33 ms): nothing is interpolated any more (both players are PREDICTED — design
/// WS1), so this no longer sets a visual delay; it sets (a) how fast a mispredict is detected +
/// rolled back and (b) the staleness bound on the `NetworkedHealth`/`NetworkedCastState`/
/// `PlayerCustomization` mirrors. Bandwidth at 2 players is trivial. (lightyear's own examples use
/// 100 ms — a demo-bandwidth default, not a feel choice.)
const REPLICATION_SEND_HZ: u32 = 30;
```

- [ ] **Step 2: Sweep stale "interpolated" language at the two filter sites**

In `crates/arena_game/src/client/app_headless.rs` lines 24-28, reword the `RemotePlayerFilter` doc to `/// Query filter for the REMOTE (predicted, opponent) networked players.` In `crates/arena_game/src/client/net.rs`, in `trace_received_remote_pose`'s doc (lines 477-483), replace "interpolated pose" wording with "predicted remote pose (driven by rebroadcast inputs)". Do NOT rename either filter or the trace kind.

- [ ] **Step 3: Build + gate**

```bash
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer 2>&1 | tail -3
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-t3; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-t3
```

Expected: `PASS`.

- [ ] **Step 4: Commit**

```bash
git add crates/arena_game/src/net/server.rs crates/arena_game/src/client/app_headless.rs crates/arena_game/src/client/net.rs
git commit -m "feat(net): 30Hz replication cadence; predicted-remote doc sweep"
```

---

### Task 4: ArenaInput v2 + shared skill/aim/charge helpers (WS2 foundation)

**Files:**
- Modify: `crates/arena_sim/src/input.rs` (add `pitch`, `skill_slot`)
- Modify: `crates/arena_game/src/net/mod.rs` (add `ARENA_SKILLS`, `aim_dir`, `skill_slot_for`, charge consts + helpers, bump `PROTOCOL_ID`)
- Modify: `crates/arena_game/src/client/net.rs` (move charge consts out, re-export; extend `LocalInput` + `buffer_arena_input`)
- Modify: `crates/arena_game/src/client/app_windowed.rs` (`skill_for_key` reads `ARENA_SKILLS`; bridge writes pitch)
- Modify: `crates/arena_game/src/client/app_headless.rs` (`automove_input` writes pitch)
- Modify: `crates/arena_game/src/server/spawn.rs:185-187` (grant loop reads `ARENA_SKILLS`)
- Test: `cargo test -p arena_game net::tests` (new unit tests in `net/mod.rs`)

**Interfaces:**
- Produces (used by Tasks 5, 6, 8):
  - `ArenaInput { movement: Vec2, yaw: f32, pitch: f32, jump: bool, charging: bool, skill_slot: u8 }`
  - `crate::net::ARENA_SKILLS: [&str; 3]` = `["firebolt", "chain_lightning", "blizzard"]`
  - `crate::net::aim_dir(yaw: f32, pitch: f32) -> Vec3` (unit camera-forward)
  - `crate::net::skill_slot_for(id: &str) -> Option<u8>`
  - `crate::net::{MAX_CHARGE_SECS, TAP_CHARGE_BYTE, charge_byte_from_frac, charge_mult, charge_byte_from_hold_ticks}` (moved from `client::net`, which re-exports them so existing call sites + tests keep compiling)

- [ ] **Step 1: Write the failing unit tests**

Append to the `tests` module in `crates/arena_game/src/net/mod.rs`:

```rust
    /// `aim_dir` must equal the camera math the windowed client uses:
    /// `Quat(Y,yaw) * Quat(X,pitch) * -Z`, normalized.
    #[test]
    fn aim_dir_matches_camera_forward() {
        use bevy::prelude::{Quat, Vec3};
        for (yaw, pitch) in [(0.0f32, 0.0f32), (-1.5707963, 0.2), (2.5, -0.7)] {
            let rot = Quat::from_axis_angle(Vec3::Y, yaw) * Quat::from_axis_angle(Vec3::X, pitch);
            let expect = (rot * -Vec3::Z).normalize();
            assert!((aim_dir(yaw, pitch) - expect).length() < 1e-5);
        }
    }

    /// Slot mapping is the positional index into ARENA_SKILLS; unknown ids have no slot.
    #[test]
    fn skill_slots_are_positional() {
        assert_eq!(skill_slot_for("firebolt"), Some(0));
        assert_eq!(skill_slot_for("chain_lightning"), Some(1));
        assert_eq!(skill_slot_for("blizzard"), Some(2));
        assert_eq!(skill_slot_for("nope"), None);
        assert_eq!(ARENA_SKILLS.len(), 3);
    }

    /// Hold-tick charge anchors: an instant tap (1 tick) ≈ TAP_CHARGE_BYTE (≈1.0×); holding
    /// MAX_CHARGE_SECS worth of ticks (or longer) = 255 (2.0×).
    #[test]
    fn charge_from_hold_ticks_anchors() {
        let full = (MAX_CHARGE_SECS * TICK_HZ as f32).ceil() as u32;
        assert!(charge_byte_from_hold_ticks(1).abs_diff(TAP_CHARGE_BYTE) <= 2);
        assert_eq!(charge_byte_from_hold_ticks(full), 255);
        assert_eq!(charge_byte_from_hold_ticks(full * 3), 255);
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test -p arena_game net::tests 2>&1 | tail -5
```

Expected: compile FAILURE (`aim_dir`/`skill_slot_for`/`charge_byte_from_hold_ticks`/`ARENA_SKILLS` not found).

- [ ] **Step 3: Extend `ArenaInput`**

In `crates/arena_sim/src/input.rs` replace the struct (lines 25-38) with:

```rust
/// Per-tick native input. Camera-relative movement + body yaw + aim pitch + jump + the cast-charge
/// telegraph + the selected skill slot. CAST intent is carried IN this stream (design WS2): a cast
/// is the falling edge of `charging` (true→false across consecutive ticks); the server counts the
/// held ticks for the charge byte and reconstructs the aim ray from `yaw`+`pitch` — no cast
/// message, no client-supplied charge. Missing ticks fill as SameAsPrecedent server-side, so a
/// lost release packet DELAYS a cast 1-2 ticks, never loses it.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default, Reflect)]
pub struct ArenaInput {
    /// Camera-relative WASD axis: x = strafe (+right), y = forward.
    pub movement: Vec2,
    /// Camera/body yaw (radians). The controller faces the body to this and builds the
    /// camera-relative movement frame from it.
    pub yaw: f32,
    /// Aim pitch (radians, +up). With `yaw` it defines the cast ray (`net::aim_dir`) and drives
    /// the opponent-side spine lean.
    pub pitch: f32,
    /// True while the jump button (Space) is held; the controller jumps on any grounded tick.
    pub jump: bool,
    /// True while the cast button is held to charge. Falling edge = cast (server-side edge detect);
    /// also drives the opponent-facing windup telegraph.
    pub charging: bool,
    /// Index into `arena_game::net::ARENA_SKILLS` of the currently-selected skill.
    pub skill_slot: u8,
}
```

- [ ] **Step 4: Add the shared helpers to `net/mod.rs`**

In `crates/arena_game/src/net/mod.rs`, after the `TICK_HZ` const (line 62), insert:

```rust
/// The grantable skill roster, in SLOT ORDER. The single source of truth for: the server grant
/// loop (`server/spawn.rs`), the windowed number-key select (`skill_for_key`), and the
/// `ArenaInput.skill_slot` ↔ skill-id mapping on both peers. Index == wire slot.
pub const ARENA_SKILLS: [&str; 3] = ["firebolt", "chain_lightning", "blizzard"];

/// Slot index for a skill id (positional in [`ARENA_SKILLS`]); `None` for unknown ids.
pub fn skill_slot_for(id: &str) -> Option<u8> {
    ARENA_SKILLS.iter().position(|s| *s == id).map(|i| i as u8)
}

/// The camera-forward aim ray both peers reconstruct from the input's `yaw`+`pitch`:
/// `Quat(Y,yaw) * Quat(X,pitch) * -Z`, normalized. Matches the windowed camera
/// (`controller::follow_local_net_player`) exactly, so the bolt flies where the crosshair looks.
pub fn aim_dir(yaw: f32, pitch: f32) -> bevy::prelude::Vec3 {
    use bevy::prelude::{Quat, Vec3};
    let rot = Quat::from_axis_angle(Vec3::Y, yaw) * Quat::from_axis_angle(Vec3::X, pitch);
    (rot * -Vec3::Z).normalize()
}

// --- charge (moved here from `client::net` — the server now computes the byte too) ---

/// Maximum hold time for the charge mechanic. A full hold of this duration maps to charge=255
/// (2.0× multiplier); an instant tap maps to [`TAP_CHARGE_BYTE`] (≈1.0×).
pub const MAX_CHARGE_SECS: f32 = 1.5;

/// Charge byte for an instant tap: ≈ one-third up the 0-255 range, which `charge_mult` maps to
/// ≈1.0×. A full hold sends 255 (→2.0×).
pub const TAP_CHARGE_BYTE: u8 = 85;

/// Map a hold fraction `[0, 1]` to a charge byte: tap → [`TAP_CHARGE_BYTE`], full hold → 255.
pub fn charge_byte_from_frac(frac: f32) -> u8 {
    let span = 255.0 - TAP_CHARGE_BYTE as f32;
    (TAP_CHARGE_BYTE as f32 + frac * span).round().clamp(0.0, 255.0) as u8
}

/// The charge → damage/speed multiplier mapping (reference; obelisk owns the authoritative
/// scaling): `0.5 + (c/255) * 1.5`, so tap ≈ 1.0× and 255 = 2.0×.
pub fn charge_mult(byte: u8) -> f32 {
    0.5 + (byte as f32 / 255.0) * 1.5
}

/// Server-side charge derivation (design WS2): held-tick count → charge byte, via the same
/// `charge_byte_from_frac` the client HUD uses on its local hold time — both peers derive the
/// identical value from the identical input stream. Saturates at MAX_CHARGE_SECS worth of ticks.
pub fn charge_byte_from_hold_ticks(hold_ticks: u32) -> u8 {
    let full = MAX_CHARGE_SECS * TICK_HZ as f32;
    charge_byte_from_frac(((hold_ticks as f32 - 1.0) / (full - 1.0)).clamp(0.0, 1.0))
}
```

Also bump the protocol id (line 67): `pub const PROTOCOL_ID: u64 = 2;` and update its doc comment (`Bumped to 2 for the input-carried cast wire (ArenaInput v2, CastRequestMessage removed)`).

- [ ] **Step 5: Re-point `client::net`'s charge items + extend `LocalInput` and the bridges**

In `crates/arena_game/src/client/net.rs`:
- Delete the local `MAX_CHARGE_SECS`/`TAP_CHARGE_BYTE`/`charge_byte_from_frac`/`charge_mult` definitions (lines 63-87) and replace with re-exports so every existing call site + the `tests` module (lines 505-523) compile unchanged:

```rust
pub use crate::net::{charge_byte_from_frac, charge_mult, MAX_CHARGE_SECS, TAP_CHARGE_BYTE};
```

- Extend `LocalInput` (lines 39-44) with pitch:

```rust
#[derive(Resource, Default, Clone, Copy)]
pub struct LocalInput {
    pub movement: Vec2,
    pub yaw: f32,
    pub pitch: f32,
    pub jump: bool,
}
```

- Extend `buffer_arena_input` (lines 204-218) to fill the new fields (SelectedSkill moves into this plugin's shared state; it is already `init_resource`'d by the windowed build — init it here too so headless has it):

```rust
fn buffer_arena_input(
    input: Res<LocalInput>,
    charge: Res<ChargeState>,
    selected: Res<SelectedSkill>,
    mut query: Query<&mut ActionState<ArenaInput>, With<InputMarker<ArenaInput>>>,
) {
    let Ok(mut action_state) = query.single_mut() else {
        return;
    };
    action_state.0 = ArenaInput {
        movement: input.movement,
        yaw: input.yaw,
        pitch: input.pitch,
        jump: input.jump,
        charging: charge.charging,
        skill_slot: crate::net::skill_slot_for(&selected.0).unwrap_or(0),
    };
}
```

and add `.init_resource::<SelectedSkill>()` to `ClientNetPlayerPlugin::build` (before the systems), keeping any existing windowed init (Bevy's `init_resource` is idempotent).

- In `crates/arena_game/src/client/app_windowed.rs`, `bridge_windowed_input_to_local_input` gains a pitch param + write: add `pitch: Res<controller::AimPitch>` to the system params and `local_input.pitch = pitch.0;` next to the existing `local_input.yaw = yaw.0;` (line 325). Replace `skill_for_key`'s hardcoded ids (lines ~262-268):

```rust
fn skill_for_key(key: KeyCode) -> Option<&'static str> {
    let idx = match key {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        _ => return None,
    };
    crate::net::ARENA_SKILLS.get(idx).copied()
}
```

- In `crates/arena_game/src/client/app_headless.rs`, `automove_input` (lines 271-275) gains pitch: add `pitch: Res<controller::AimPitch>` and `input.pitch = pitch.0;`.
- In `crates/arena_game/src/server/spawn.rs`, replace the grant chain (lines 185-187):

```rust
    {
        let mut ec = commands.entity(player);
        for skill in crate::net::ARENA_SKILLS {
            ec.grant_skill(skill);
        }
    }
```

(keep the `client_map.0.insert(...)` after it).

- [ ] **Step 6: Run the tests + build**

```bash
cargo test -p arena_game 2>&1 | tail -8
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer 2>&1 | tail -3
```

Expected: all tests PASS (including the three new ones and the preserved `charge_mult_anchors`/`charge_byte_endpoints`); build clean.

- [ ] **Step 7: Commit**

```bash
git add crates/arena_sim/src/input.rs crates/arena_game/src/net/mod.rs crates/arena_game/src/client/net.rs crates/arena_game/src/client/app_windowed.rs crates/arena_game/src/client/app_headless.rs crates/arena_game/src/server/spawn.rs
git commit -m "feat(net): ArenaInput v2 (pitch + skill_slot) + shared ARENA_SKILLS/aim/charge helpers"
```

---

### Task 5: Server cast-edge pipeline (WS2 server half)

**Files:**
- Modify: `crates/arena_game/src/server/cast_pipeline.rs` (replace `drain_cast_requests` with `detect_cast_edges` + `PrevCastInput`)
- Modify: `crates/arena_game/src/server/spawn.rs` (insert `PrevCastInput::default()` in the networked component set)
- Modify: `crates/arena_game/src/server/mod.rs` (schedule change)
- Test: unit tests inside `cast_pipeline.rs`

**Interfaces:**
- Consumes: `ArenaInput` v2, `charge_byte_from_hold_ticks`, `ARENA_SKILLS`, `aim_dir` (Task 4); `resolve_cast_aim` (existing, unchanged).
- Produces: `pub(crate) struct PrevCastInput { charging: bool, hold_ticks: u32, slot: u8 }` component on every server player; system `detect_cast_edges` (FixedUpdate, `.before(ObeliskSet::Validate)`), trace kind `cast_edge { caster, skill_id, charge }`. `drain_cast_requests` is GONE.

- [ ] **Step 1: Write the failing edge-detection unit test**

Append to the `acq_tests` module in `crates/arena_game/src/server/cast_pipeline.rs` (rename module to `tests`):

```rust
    /// The falling-edge detector: a cast fires exactly when charging goes true→false, with the
    /// charge byte derived from the number of held ticks.
    #[test]
    fn cast_edge_fires_on_release_with_held_charge() {
        let mut prev = PrevCastInput::default();
        // idle → no cast
        assert_eq!(step_cast_edge(&mut prev, false, 0), None);
        // press + hold 3 ticks → no cast while held
        assert_eq!(step_cast_edge(&mut prev, true, 0), None);
        assert_eq!(step_cast_edge(&mut prev, true, 0), None);
        assert_eq!(step_cast_edge(&mut prev, true, 0), None);
        // release → cast with 3 held ticks' charge, slot latched from the charged ticks
        let fired = step_cast_edge(&mut prev, false, 0);
        assert_eq!(fired, Some((0, crate::net::charge_byte_from_hold_ticks(3))));
        // staying released → nothing
        assert_eq!(step_cast_edge(&mut prev, false, 0), None);
    }

    /// The slot recorded at release is the slot held during the charge (latched at press),
    /// so a selection change mid-charge can't retarget an in-flight charge.
    #[test]
    fn cast_edge_latches_slot_at_press() {
        let mut prev = PrevCastInput::default();
        assert_eq!(step_cast_edge(&mut prev, true, 2), None);
        let fired = step_cast_edge(&mut prev, false, 1);
        assert_eq!(fired, Some((2, crate::net::charge_byte_from_hold_ticks(1))));
    }
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p arena_game cast_edge 2>&1 | tail -5
```

Expected: compile FAILURE (`PrevCastInput`/`step_cast_edge` not found).

- [ ] **Step 3: Implement the edge core + system; delete the message drain**

In `crates/arena_game/src/server/cast_pipeline.rs`:

(a) Replace the module doc (lines 1-11) with:

```rust
//! Cast pipeline: input-stream cast edges → server-resolved `CastAim` → obelisk cast.
//!
//! Casts ride the native input (design WS2): a cast is the FALLING EDGE of `ArenaInput.charging`
//! (true→false across consecutive ticks) in the per-tick `ActionState<ArenaInput>` lightyear
//! maintains for each player. The server counts held ticks for the charge byte
//! (`net::charge_byte_from_hold_ticks`), latches the skill slot at press, reconstructs the aim ray
//! from the input's yaw+pitch (`net::aim_dir` — the same math as the client camera), resolves a
//! CANDIDATE `CastAim` from the skill's authored `Acquisition` (`resolve_cast_aim`), and inserts a
//! `PendingCast`; obelisk's `validate_casts` does the AUTHORITATIVE range/filter/fallback +
//! mana/cooldown gate. Packet loss fills missing ticks as SameAsPrecedent, so a lost release
//! DELAYS the edge 1-2 ticks rather than dropping the cast.
```

(b) Remove the `MessageReceiver`/`RemoteId`/`ClientOf`/`CastRequestMessage` imports; add:

```rust
use lightyear::prelude::input::native::ActionState;
use crate::net::input::ArenaInput;
```

(c) Add the component + pure edge core:

```rust
/// Per-player cast-edge memory: last tick's `charging`, how many ticks it has been held, and the
/// skill slot latched at press. Server-only (inserted by `spawn_player_on_connect`).
#[derive(Component, Default, Debug, PartialEq)]
pub(crate) struct PrevCastInput {
    charging: bool,
    hold_ticks: u32,
    slot: u8,
}

/// Advance the per-tick edge state machine. Returns `Some((slot, charge_byte))` exactly on the
/// falling edge of `charging`. Pure — unit-testable without an app.
fn step_cast_edge(prev: &mut PrevCastInput, charging: bool, slot: u8) -> Option<(u8, u8)> {
    let fired = match (prev.charging, charging) {
        (true, false) => Some((
            prev.slot,
            crate::net::charge_byte_from_hold_ticks(prev.hold_ticks),
        )),
        _ => None,
    };
    if charging {
        if !prev.charging {
            prev.slot = slot; // latch selection at press
            prev.hold_ticks = 0;
        }
        prev.hold_ticks = prev.hold_ticks.saturating_add(1);
    } else {
        prev.hold_ticks = 0;
    }
    prev.charging = charging;
    fired
}
```

(d) Replace `drain_cast_requests` with:

```rust
/// FixedUpdate: detect each player's cast edge from its per-tick `ActionState<ArenaInput>` and
/// insert a `PendingCast` (obelisk validates + executes). Runs `.before(ObeliskSet::Validate)` so
/// the cast lands the SAME tick as its input edge. Skips a caster already mid-`ActiveCast`
/// (obelisk would reject; skipping keeps the edge state clean).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn detect_cast_edges(
    mut players: Query<
        (
            Entity,
            &ObeliskId,
            &ActionState<ArenaInput>,
            &mut PrevCastInput,
        ),
        With<NetworkedPlayer>,
    >,
    active: Query<(), With<ActiveCast>>,
    handles: Res<CastTimelineHandles>,
    timelines: Res<Assets<CastTimeline>>,
    transforms: Query<&bevy::prelude::Transform>,
    hurtboxes: Query<(Entity, &Hurtbox)>,
    spatial: avian3d::prelude::SpatialQuery,
    mut commands: Commands,
) {
    for (caster, caster_id, action, mut prev) in &mut players {
        let Some((slot, charge)) = step_cast_edge(&mut prev, action.0.charging, action.0.skill_slot)
        else {
            continue;
        };
        if active.get(caster).is_ok() {
            continue; // mid-cast; obelisk would reject anyway
        }
        let Some(skill_id) = crate::net::ARENA_SKILLS.get(slot as usize).map(|s| s.to_string())
        else {
            continue; // out-of-range slot from a bad client: ignore
        };
        let dir = Dir3::new(crate::net::aim_dir(action.0.yaw, action.0.pitch)).unwrap_or(Dir3::NEG_Z);
        trace::event(
            "cast_edge",
            json!({ "caster": caster_id.0, "skill_id": skill_id, "charge": charge }),
        );
        let muzzle_offset = Vec3::Y * crate::net::ARENA_EYE_HEIGHT;
        let acq = timelines
            .get(handles.0.get(&skill_id).map(|h| h.id()).unwrap_or_default())
            .map(|tl| tl.acquisition.clone())
            .unwrap_or(Acquisition::Aim);
        let cast_entity = |range: f32| -> Option<RayHit> {
            let origin = transforms.get(caster).ok()?.translation + muzzle_offset;
            let own: Vec<Entity> = hurtboxes
                .iter()
                .filter(|(_, h)| h.owner == caster)
                .map(|(e, _)| e)
                .chain([caster])
                .collect();
            let filter = avian3d::prelude::SpatialQueryFilter::default().with_excluded_entities(own);
            let hit = spatial.cast_ray(origin, dir, range, true, &filter)?;
            hurtboxes
                .get(hit.entity)
                .map(|(_, h)| h.owner)
                .ok()
                .or_else(|| players.contains(hit.entity).then_some(hit.entity))
                .map(RayHit::Entity)
        };
        let cast_ground = |range: f32| -> Option<Vec3> {
            let origin = transforms.get(caster).ok()?.translation + muzzle_offset;
            let own: Vec<Entity> = hurtboxes
                .iter()
                .filter(|(_, h)| h.owner == caster)
                .map(|(e, _)| e)
                .chain([caster])
                .collect();
            let filter = avian3d::prelude::SpatialQueryFilter::default().with_excluded_entities(own);
            let hit = spatial.cast_ray(origin, dir, range, true, &filter)?;
            Some(origin + *dir * hit.distance)
        };
        let aim = resolve_cast_aim(&acq, dir, cast_entity, cast_ground);
        let aim_shape = match &aim {
            CastAim::Entity(_) => "Entity",
            CastAim::Point(_) => "Point",
            CastAim::Direction(_) => "Direction",
        };
        trace::event(
            "cast_acquired",
            json!({ "caster": caster_id.0, "skill_id": skill_id, "aim": aim_shape }),
        );
        commands.entity(caster).insert(PendingCast {
            skill_id,
            aim,
            charge: Some(charge),
            muzzle_offset,
        });
    }
}
```

NOTE: `players.contains(hit.entity)` replaces the old `casters.get(hit.entity).is_ok()` body-collider fallback (the query itself now proves "is a networked player"). If the borrow checker rejects using `players` inside the closures while iterating it, hoist a `HashSet<Entity>` of player entities + a `HashMap<Entity, Vec3>` of positions before the loop and use those in the closures instead — preserve behavior exactly.

(e) Delete the old `ClientPlayerMap` import if now unused.

- [ ] **Step 4: Insert `PrevCastInput` at spawn; rewire the schedule**

In `crates/arena_game/src/server/spawn.rs`, add to the first `.insert((...))` tuple (after `ActionState::<ArenaInput>::default(),` line 174):

```rust
            // Server-side cast-edge memory (design WS2). Not replicated.
            super::cast_pipeline::PrevCastInput::default(),
```

In `crates/arena_game/src/server/mod.rs`:
- `use cast_pipeline::detect_cast_edges;` (replacing `drain_cast_requests`)
- Remove `drain_cast_requests` from the `Update` tuple (line 84 area).
- Add to the FixedUpdate registration (replacing lines 108-111):

```rust
            .add_systems(
                FixedUpdate,
                (
                    (server_apply_yaw, server_apply_movement).chain(),
                    // Cast edges from the per-tick input stream, before obelisk validates this
                    // tick's PendingCasts (commands flush at the ordering edge, so the cast lands
                    // the SAME tick as its input edge).
                    detect_cast_edges.before(obelisk_bevy::prelude::ObeliskSet::Validate),
                ),
            )
```

- [ ] **Step 5: Run the unit tests**

```bash
cargo test -p arena_game -p arena_sim 2>&1 | tail -8
```

Expected: PASS including `cast_edge_fires_on_release_with_held_charge` and `cast_edge_latches_slot_at_press`. (The client still sends `CastRequestMessage` — dead on the server now, deleted next task; net-test would FAIL at this commit, which is why Tasks 5+6 land back-to-back before the next gate.)

- [ ] **Step 6: Commit**

```bash
git add crates/arena_game/src/server/cast_pipeline.rs crates/arena_game/src/server/spawn.rs crates/arena_game/src/server/mod.rs
git commit -m "feat(server): input-edge cast pipeline — detect_cast_edges replaces the cast message drain"
```

---

### Task 6: Client cast edge + harness rework; delete the cast message (WS2 client half)

**Files:**
- Modify: `crates/arena_game/src/client/net.rs` (tap/jump latches, `local_cast_edge`, delete `CastIntent`+`send_cast_requests`)
- Modify: `crates/arena_game/src/client/app_windowed.rs` (`bridge_windowed_cast_hold` sets latch; autocast import)
- Modify: `crates/arena_game/src/client/app_headless.rs` (autocast pulses the latch)
- Modify: `crates/arena_game/src/net/protocol.rs` (delete `CastRequestMessage`; rename `CastChannel`→`RequestChannel`)

**Interfaces:**
- Consumes: `aim_dir`, `ARENA_SKILLS`, `skill_slot_for` (Task 4); `PredictedCast` (existing message, gains `charge: u8` field).
- Produces: `ChargeState.tap_latch: bool` (press latch, guarantees ≥1 charging tick per tap); `LocalInput.jump_latch: bool`; system `local_cast_edge` (FixedUpdate, emits `PredictedCast` on the local falling edge); trace kind `cast_edge_sent { skill_id, charge }` replacing `cast_request_sent`. `CastIntent`, `send_cast_requests`, `CastRequestMessage` are GONE; `RequestChannel` carries only `CustomizeMessage`.

- [ ] **Step 1: Protocol — delete the cast message, rename the channel**

In `crates/arena_game/src/net/protocol.rs`:
- Delete the `CastRequestMessage` struct (lines 144-155) and its `register_message` (lines 107-108).
- Rename `pub struct CastChannel;` → `pub struct RequestChannel;` (line 131) with doc `/// Rare reliable client→server requests (customization). Casts ride the input stream (WS2).`
- Update the channel registration comment + type (lines 90-96) to `app.add_channel::<RequestChannel>(...)` (same settings).
- `CustomizeMessage`'s doc (line 167-169): now "reliable (`RequestChannel`)".

- [ ] **Step 2: Client — latches + the local edge system; delete the message path**

In `crates/arena_game/src/client/net.rs`:

(a) `LocalInput` gains a jump latch; `ChargeState` gains a tap latch:

```rust
#[derive(Resource, Default, Clone, Copy)]
pub struct LocalInput {
    pub movement: Vec2,
    pub yaw: f32,
    pub pitch: f32,
    pub jump: bool,
    /// Set by the input bridge on a jump PRESS; OR-ed into the next sampled tick then cleared, so
    /// a sub-tick tap (press+release between FixedPreUpdates) still lands in exactly one tick.
    pub jump_latch: bool,
}
```

```rust
pub struct ChargeState {
    pub secs: f32,
    pub charging: bool,
    /// Set on cast-button PRESS; guarantees the input stream carries ≥1 `charging=true` tick even
    /// for a sub-tick tap, so the server's falling-edge detector always sees press-then-release.
    /// Cleared by `buffer_arena_input` after it samples a released tick.
    pub tap_latch: bool,
}
```

(`pending_charge` and its `Default` init DIE — remove them; `Default` now derives all-false/0.0. Delete the `pending_charge` doc lines. Keep `frac()`.)

(b) `buffer_arena_input` consumes the latches:

```rust
fn buffer_arena_input(
    mut input: ResMut<LocalInput>,
    mut charge: ResMut<ChargeState>,
    selected: Res<SelectedSkill>,
    mut query: Query<&mut ActionState<ArenaInput>, With<InputMarker<ArenaInput>>>,
) {
    let Ok(mut action_state) = query.single_mut() else {
        return;
    };
    let charging = charge.charging || charge.tap_latch;
    action_state.0 = ArenaInput {
        movement: input.movement,
        yaw: input.yaw,
        pitch: input.pitch,
        jump: input.jump || input.jump_latch,
        charging,
        skill_slot: crate::net::skill_slot_for(&selected.0).unwrap_or(0),
    };
    input.jump_latch = false;
    if !charge.charging {
        // We just sampled the release (or the latch-extended tap tick); the NEXT tick samples
        // false and produces the falling edge on both peers.
        charge.tap_latch = false;
    }
}
```

(c) Delete `CastIntent` (lines 46-51) and `send_cast_requests` (lines 323-395) and their plugin registrations. `PredictedCast` gains a charge field (after `aim_dir`):

```rust
    pub aim_dir: Vec3,
    /// Charge byte derived from the local hold (same `charge_byte_from_hold_ticks` the server
    /// uses), threaded into the predicted cues' `ParamSource::Charge` bindings.
    pub charge: u8,
```

(d) Add the local mirror of the server's edge detector (same helper semantics, one entity):

```rust
/// FixedUpdate: mirror the server's cast-edge detection on the LOCAL predicted entity's own
/// per-tick `ActionState<ArenaInput>` (the identical stream lightyear ships), emitting a
/// [`PredictedCast`] on the falling edge for zero-latency own-cast presentation (design WS3).
/// The server detects the same edge in the same stream and runs the authoritative cast.
#[allow(clippy::type_complexity)]
fn local_cast_edge(
    mut prev: Local<(bool, u32, u8)>, // (charging, hold_ticks, latched slot)
    local: Query<
        (&Position, &ObeliskNetId, &ActionState<ArenaInput>),
        (With<NetworkedPlayer>, With<LocalNetPlayer>),
    >,
    mut predicted: MessageWriter<PredictedCast>,
) {
    let Ok((pos, obelisk_id, action)) = local.single() else {
        return;
    };
    let a = &action.0;
    let fired = prev.0 && !a.charging;
    if a.charging {
        if !prev.0 {
            prev.2 = a.skill_slot;
            prev.1 = 0;
        }
        prev.1 = prev.1.saturating_add(1);
    }
    if fired {
        let charge = crate::net::charge_byte_from_hold_ticks(prev.1);
        if let Some(skill_id) = crate::net::ARENA_SKILLS.get(prev.2 as usize) {
            crate::trace::event(
                "cast_edge_sent",
                serde_json::json!({ "skill_id": skill_id, "charge": charge }),
            );
            predicted.write(PredictedCast {
                skill_id: skill_id.to_string(),
                source_id: obelisk_id.0.clone(),
                position: pos.0,
                aim_dir: crate::net::aim_dir(a.yaw, a.pitch),
                charge,
            });
        }
        prev.1 = 0;
    }
    prev.0 = a.charging;
}
```

Add the needed imports (`ObeliskNetId` is already imported; add `Position` from avian's prelude — already glob-imported at line 18).

(e) Register it in `ClientNetPlayerPlugin` FixedUpdate, after the controller pair:

```rust
            .add_systems(
                FixedUpdate,
                (client_apply_yaw, client_apply_movement, local_cast_edge).chain(),
            )
```

(f) `send_customization` (line 415): `sender.send::<CastChannel>(...)` → `sender.send::<RequestChannel>(...)`; fix the import.

- [ ] **Step 3: Windowed bridge sets the latch; headless autocast pulses it**

In `crates/arena_game/src/client/app_windowed.rs`, in `bridge_windowed_cast_hold`: where the press is first detected (the `just_pressed` / hold-start branch), add `charge.tap_latch = true;`. Where release currently computed `pending_charge` + set `CastIntent`, delete both — release now only sets `charge.charging = false` (+ resets `secs`). The charge byte is derived from the input stream on both peers; `SelectedSkill` is already what `buffer_arena_input` samples.

In `crates/arena_game/src/client/app_headless.rs`, replace `autocast` (lines 136-160):

```rust
/// AUTOCAST (`ARENA_AUTOCAST=1`), shared by the windowed and headless clients: pulse the cast
/// charge latch on a cadence once the local player + an opponent are materialized, so the input
/// stream carries a press→release edge and BOTH peers' edge detectors fire (server: authoritative
/// cast; client: predicted cosmetics). `ARENA_AUTOCAST_SKILL` picks the skill (default firebolt)
/// via `SelectedSkill`; `ARENA_AUTOCAST_PERIOD` the cadence (default 0.8s).
pub(super) fn autocast(
    time: Res<Time>,
    mut accum: Local<f32>,
    mut charge: ResMut<net::ChargeState>,
    mut selected: ResMut<net::SelectedSkill>,
    local: Query<(), LocalPlayerFilter>,
    remotes: Query<(), RemotePlayerFilter>,
) {
    if local.iter().next().is_none() || remotes.iter().next().is_none() {
        return;
    }
    let period = std::env::var("ARENA_AUTOCAST_PERIOD")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.8);
    *accum += time.delta_secs();
    if *accum >= period {
        *accum = 0.0;
        if let Ok(skill) = std::env::var("ARENA_AUTOCAST_SKILL") {
            selected.0 = skill;
        }
        charge.tap_latch = true; // one charging tick, then release → edge
    }
}
```

(`ChargeState`/`SelectedSkill` need `pub` visibility from `client::net` — they already are `pub`.)

- [ ] **Step 4: Build, unit tests, END-TO-END gate**

```bash
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer 2>&1 | tail -3
cargo test -p arena_game 2>&1 | tail -5
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-t6; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-t6
grep -c cast_edge /tmp/arena-net-t6/server.jsonl
```

Expected: tests PASS; gate `PASS` (this proves the whole input-carried cast pipeline end-to-end: autocast latch → input edge → server `detect_cast_edges` → obelisk cast → damage on both observers); server grep ≥ 1.

- [ ] **Step 5: Commit**

```bash
git add crates/arena_game/src/net/protocol.rs crates/arena_game/src/client/net.rs crates/arena_game/src/client/app_windowed.rs crates/arena_game/src/client/app_headless.rs
git commit -m "feat(net): casts ride the input stream — client edge + latches; CastRequestMessage deleted"
```

---

### Task 7: Client NetEventMessage fan-out (fixes a live double-drain bug)

**Files:**
- Modify: `crates/arena_game/src/skills.rs` (single drain → local `ClientNetEvent` fan-out)
- Modify: `crates/arena_game/src/client/hud.rs:346-360` (consume the fan-out, not the receiver)

**Interfaces:**
- Produces: `#[derive(Message)] pub struct ClientNetEvent(pub obelisk_bevy::net::NetEvent);` in `skills.rs` — THE way any client system consumes replicated combat events from now on (Task 8's fizzle-cancel uses it).

Background: `skills.rs::trace_received_net_events` AND `hud.rs` (line 346) both call `MessageReceiver::<NetEventMessage>::receive()` on the windowed client — each steals events from the other (violates CLAUDE.md footgun 8; symptom: randomly missing damage numbers/hit flashes).

- [ ] **Step 1: Make the trace drain the single drain + fan out**

In `crates/arena_game/src/skills.rs`, replace `register_client_event_trace` + `trace_received_net_events` (lines 128-140):

```rust
/// The client-local fan-out of the replicated `NetEventMessage` stream. `drain_net_events` is the
/// SINGLE `MessageReceiver::<NetEventMessage>` drain (footgun 8); every other consumer (trace, HUD
/// damage numbers, predicted-cast fizzle) reads this Bevy message instead.
#[derive(Message, Clone, Debug)]
pub struct ClientNetEvent(pub obelisk_bevy::net::NetEvent);

/// Register the client-side NetEvent drain + trace. Added by BOTH client modes.
pub fn register_client_event_trace(app: &mut App) {
    app.add_message::<ClientNetEvent>();
    app.add_systems(Update, drain_net_events);
}

/// THE single `NetEventMessage` drain: trace each event + fan it out as [`ClientNetEvent`].
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
```

- [ ] **Step 2: Re-point the HUD**

In `crates/arena_game/src/client/hud.rs`, change the damage-consuming system's parameter from `mut receivers: Query<&mut MessageReceiver<NetEventMessage>>` (line 346) to `mut events: MessageReader<crate::skills::ClientNetEvent>`, and its loop from `for mut rx in &mut receivers { for NetEventMessage(ev) in rx.receive() {` to `for crate::skills::ClientNetEvent(ev) in events.read() {` (adjusting the closing braces; the match body over `ev` is unchanged, though `ev` is now a reference — add `let ev = ev.clone();` if the body needs ownership). Remove the now-unused `MessageReceiver`/`NetEventMessage` imports.

- [ ] **Step 3: Build + gate**

```bash
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer 2>&1 | tail -3
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-t7; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-t7
```

Expected: `PASS` (the `client_net_*` trace kinds are emitted by the same `trace_net_event` from the new single drain).

- [ ] **Step 4: Commit**

```bash
git add crates/arena_game/src/skills.rs crates/arena_game/src/client/hud.rs
git commit -m "fix(client): single NetEventMessage drain + ClientNetEvent fan-out (double-drain stole HUD events)"
```

---

### Task 8: Predicted cast presentation (WS3)

**Files:**
- Modify: `crates/arena_game/src/skills.rs` (cue scheduler + `PredictedCues` de-dup registry + fizzle cancel)
- Test: unit test for the window-time helper in `skills.rs`

**Interfaces:**
- Consumes: `PredictedCast` (with `charge`, Task 6), `ClientNetEvent` (Task 7), `CastTimeline { phase_durations, collision_windows, vfx_cues }`, `WindowSpawn::Scheduled { phase, offset }` from `obelisk_bevy::assets`, `CueMessage`/`CueKind` from `net::cue`.
- Produces: `PredictedCues` resource (the de-dup registry `Vec<(f64 expiry, String source_id, String cue_id)>`), `PredictedCueQueue` resource, trace kinds `predicted_cue { cue_id }`, `predicted_fizzle { skill_id }`. De-dup in `consume_replicated_cues` becomes registry-based (kind-based rule dies).

- [ ] **Step 1: Write the failing window-time unit test**

Add a `tests` module to `crates/arena_game/src/skills.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::phase_start;
    use obelisk_bevy::assets::PhaseDurations;
    use obelisk_bevy::prelude::SkillPhase;

    /// Scheduled-window fire times: Windup-phase windows fire at `offset`, Active at
    /// `windup + offset`, Recovery at `windup + active + offset` (matches obelisk's scheduler).
    #[test]
    fn phase_start_offsets_match_timeline_order() {
        let d = PhaseDurations { windup: 0.3, active: 0.1, recovery: 0.2 };
        assert_eq!(phase_start(&d, SkillPhase::Windup), 0.0);
        assert_eq!(phase_start(&d, SkillPhase::Active), 0.3);
        assert!((phase_start(&d, SkillPhase::Recovery) - 0.4).abs() < 1e-6);
        assert!((phase_start(&d, SkillPhase::Done) - 0.6).abs() < 1e-6);
    }
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p arena_game phase_start 2>&1 | tail -4
```

Expected: compile FAILURE (`phase_start` not found).

- [ ] **Step 3: Implement scheduler + registry + fizzle**

In `crates/arena_game/src/skills.rs`:

(a) Helpers + resources:

```rust
/// Elapsed cast-time at which a phase begins (obelisk schedules a `Scheduled { phase, offset }`
/// window at `phase_start(phase) + offset`).
pub(crate) fn phase_start(d: &obelisk_bevy::assets::PhaseDurations, phase: SkillPhase) -> f32 {
    match phase {
        SkillPhase::Windup => 0.0,
        SkillPhase::Active => d.windup,
        SkillPhase::Recovery => d.windup + d.active,
        SkillPhase::Done => d.windup + d.active + d.recovery,
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
    /// Cancel key for fizzle (a rejected cast cancels its not-yet-fired cues).
    skill_id: String,
    source_id: String,
}

/// Predicted cue windows scheduled by `predicted_local_cast`, fired by `tick_predicted_cues`.
#[derive(Resource, Default)]
struct PredictedCueQueue(Vec<ScheduledCue>);
```

(b) Extend `register_predicted_sim`:

```rust
pub fn register_predicted_sim(app: &mut App) {
    app.init_resource::<PredictedCueQueue>();
    // AimDirs is normally seeded by the windowed cosmetics init; init here too so the headless
    // client (which now also registers the predicted sim, see Step 4) has it. Idempotent.
    app.init_resource::<crate::client::cosmetics::AimDirs>();
    app.add_systems(Update, (predicted_local_cast, tick_predicted_cues, cancel_rejected_casts));
}
```

and add `app.init_resource::<PredictedCues>();` inside `register_client_cue_binding` (both modes need the resource to exist for `consume_replicated_cues`).

(c) Rewrite `predicted_local_cast` to schedule the full timeline (it currently emits only `on_cast`):

```rust
/// Consume [`PredictedCast`] → play the on_cast cue NOW and schedule the skill's `Scheduled`
/// collision-window cues at their authored offsets (design WS3), so the local player's bolt
/// launches at the authored moment with zero added round-trip. Registers every predicted cue in
/// [`PredictedCues`] so the server's copies are skipped. Template windows (emitter-spawned, e.g.
/// blizzard shards) are NOT predicted — their server cues play normally. Cosmetic-only (Stage A).
fn predicted_local_cast(
    mut predicted: MessageReader<crate::client::net::PredictedCast>,
    handles: Res<CastTimelineHandles>,
    timelines: Res<Assets<CastTimeline>>,
    time: Res<Time>,
    mut aim: ResMut<crate::client::cosmetics::AimDirs>,
    mut registry: ResMut<PredictedCues>,
    mut queue: ResMut<PredictedCueQueue>,
    mut out: MessageWriter<crate::client::cosmetics::LocalCue>,
) {
    let now = time.elapsed_secs_f64();
    for cast in predicted.read() {
        let Some(tl) = handles.0.get(&cast.skill_id).and_then(|h| timelines.get(h)) else {
            continue; // timeline not loaded: cosmetics-only, skip
        };
        aim.0.insert(cast.source_id.clone(), cast.aim_dir);
        let mut register = |cue_id: &str, fire_at: f32| {
            registry.0.push((
                now + fire_at as f64 + 5.0,
                cast.source_id.clone(),
                cue_id.to_string(),
            ));
        };
        // on_cast: fires immediately.
        if let Some(cue_id) = tl.vfx_cues.get("on_cast") {
            register(cue_id, 0.0);
            crate::trace::event(
                "predicted_cast",
                serde_json::json!({ "cue_id": cue_id, "source_id": cast.source_id }),
            );
            out.write(crate::client::cosmetics::LocalCue(crate::net::cue::CueMessage {
                cue_id: cue_id.clone(),
                skill_id: cast.skill_id.clone(),
                source_id: cast.source_id.clone(),
                position: cast.position,
                aim_dir: cast.aim_dir,
                position_from: None,
                charge: Some(cast.charge),
                end_reason: None,
                kind: crate::net::cue::CueKind::OnCast,
            }));
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
            register(cue_id, fire_at);
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

/// Fire scheduled predicted cues when their timers elapse, refreshing `position` to the caster's
/// LIVE predicted pose (they may have moved during the windup).
fn tick_predicted_cues(
    time: Res<Time>,
    mut queue: ResMut<PredictedCueQueue>,
    local: Query<
        (&avian3d::prelude::Position, &crate::net::protocol::ObeliskNetId),
        With<crate::client::net::LocalNetPlayer>,
    >,
    mut out: MessageWriter<crate::client::cosmetics::LocalCue>,
) {
    let live: Option<(bevy::prelude::Vec3, String)> =
        local.iter().next().map(|(p, id)| (p.0, id.0.clone()));
    queue.0.retain_mut(|s| {
        s.fire_in.tick(time.delta());
        if !s.fire_in.is_finished() {
            return true;
        }
        let mut cue = s.cue.clone();
        if let Some((pos, ref id)) = live {
            if *id == s.source_id {
                cue.position = pos + bevy::prelude::Vec3::Y * crate::net::ARENA_EYE_HEIGHT;
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
/// acceptable (denied-cast flicker, industry standard). Reads the Task-7 fan-out.
fn cancel_rejected_casts(
    mut events: MessageReader<ClientNetEvent>,
    local: Query<&crate::net::protocol::ObeliskNetId, With<crate::client::net::LocalNetPlayer>>,
    mut queue: ResMut<PredictedCueQueue>,
) {
    let Some(local_id) = local.iter().next().map(|o| o.0.clone()) else {
        return;
    };
    for ClientNetEvent(ev) in events.read() {
        if let obelisk_bevy::net::NetEvent::CastRejected { caster, skill_id, .. } = ev {
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
```

(Add `SkillPhase` to the obelisk prelude import if not already glob-imported — `use obelisk_bevy::prelude::*;` at line 23 covers it.)

(d) Replace the de-dup block in `consume_replicated_cues` (lines 225-234) with the registry check, and its system params gain `time: Res<Time>, mut registry: ResMut<PredictedCues>`:

```rust
            // De-dup (WS3): skip a replicated cue this client already PLAYED as a prediction.
            // Registry-based — exact (source_id, cue_id) pairs registered at predict time — so
            // server-only cues (Template-window shards, OnHit, OnEnd, OnEmit) always play.
            let now = time.elapsed_secs_f64();
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
```

(Delete the old `local:`/`local_id` query param + `is_own`/`is_predicted_kind` logic from this system.)

- [ ] **Step 4: Tests + gate**

```bash
cargo test -p arena_game 2>&1 | tail -5
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer 2>&1 | tail -3
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-t8; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-t8
grep -c '"kind":"predicted_cast"' /tmp/arena-net-t8/observer-0.jsonl
```

Expected: tests PASS; gate PASS. NOTE the grep: `register_predicted_sim` is windowed-only today — the headless observer emits `cast_edge_sent` but NOT `predicted_cast`. Add `crate::skills::register_predicted_sim(&mut app);` to `run_headless_client` (after `register_client_cue_binding`) so the predicted path is exercised headlessly (it emits LocalCues into the void — cosmetics::LocalCue has no headless reader, which is the existing harmless pattern). Then the grep prints ≥ 1.

- [ ] **Step 5: Commit**

```bash
git add crates/arena_game/src/skills.rs crates/arena_game/src/client/app_headless.rs
git commit -m "feat(client): predicted cast-cue timeline + registry de-dup + fizzle cancel"
```

---

### Task 9: Presentation polish — camera schedule, teleport snap, explicit policies, remote pitch (WS4)

**Files:**
- Modify: `crates/arena_game/src/client/controller.rs` (camera follow → PostUpdate; remote lean from remote input)
- Modify: `crates/arena_game/src/client/net.rs` or new system site: teleport snap (put it in `client/harness.rs` next to the frame-interp observer)
- Modify: `crates/arena_game/src/net/client.rs` (explicit `PredictionManager` policies)

**Interfaces:**
- Consumes: `VisualCorrection<Position>`/`VisualCorrection<Rotation>` (`lightyear::prediction::correction::VisualCorrection`, field `pub error`; for avian3d `Diffable<Self>` so `D = Position`/`Rotation`); `PhysicsSystems::Writeback` (avian) + `TransformSystems::Propagate` ordering from `lightyear_avian3d-0.26.4/src/plugin.rs:168-179`.
- Produces: const `CORRECTION_SNAP_DISTANCE: f32 = 2.0` + system `snap_large_corrections`.

- [ ] **Step 1: Camera follow to PostUpdate**

In `crates/arena_game/src/client/controller.rs`, split the Update tuple (lines 149-155): `cursor_grab` + `accumulate_mouse_look` stay in `Update` (chained); move `follow_local_net_player` to:

```rust
            .add_systems(
                PostUpdate,
                // AFTER lightyear's frame-interpolation + correction have been written back to
                // Transform (PhysicsSystems::Writeback — the chain lightyear_avian configures:
                // FrameInterpolate → VisualCorrection → Writeback → Propagate), BEFORE propagation
                // folds it into GlobalTransform. In Update the camera read a frame-stale,
                // non-interpolated Transform → micro-stutter when strafing (design P5).
                follow_local_net_player
                    .after(avian3d::prelude::PhysicsSystems::Writeback)
                    .before(bevy::transform::TransformSystems::Propagate),
            )
```

(add `use avian3d::prelude::PhysicsSystems;` — if the item lives elsewhere in avian 0.5, rustc's suggestion will name the right path, e.g. `avian3d::schedule::PhysicsSystems`; keep the ordering pair exactly as written.)

- [ ] **Step 2: Teleport snap for large corrections**

In `crates/arena_game/src/client/harness.rs`, add and register in both app roots' `Update` (windowed `app_windowed.rs` next to the frame-interp plugin adds; headless skips it — no rendering):

```rust
/// Round-reset teleports produce a huge rollback error that `CorrectionPolicy` would otherwise
/// GLIDE across the arena over ~1s (design P8). Any correction error beyond this is a teleport,
/// not a mispredict — snap it (remove the visual correction).
const CORRECTION_SNAP_DISTANCE: f32 = 2.0;

/// Remove `VisualCorrection` when its error is teleport-sized so resets snap instead of gliding.
pub(super) fn snap_large_corrections(
    q: Query<(Entity, &VisualCorrection<Position>), With<Predicted>>,
    mut commands: Commands,
) {
    for (e, corr) in &q {
        if corr.error.0.length() > CORRECTION_SNAP_DISTANCE {
            commands
                .entity(e)
                .remove::<(VisualCorrection<Position>, VisualCorrection<Rotation>)>();
        }
    }
}
```

with imports `use lightyear::prediction::correction::VisualCorrection;`, `use lightyear::prelude::Predicted;`, `use avian3d::prelude::{Position, Rotation};`. Register in `app_windowed.rs`: `app.add_systems(PostUpdate, snap_large_corrections.before(lightyear::prelude::RollbackSystems::VisualCorrection));` — if `RollbackSystems` isn't exported at that path, use `lightyear::prediction::rollback::RollbackSystems`.

- [ ] **Step 3: Explicit prediction policies**

In `crates/arena_game/src/net/client.rs`, replace `PredictionManager::default()` (line 114):

```rust
            // Explicit (== default) policies, named so the tuning surface is visible: rollback on
            // confirmed-state mismatch per the protocol's 0.01-epsilon comparators; smooth the
            // post-rollback visual error exponentially (50% per 200ms). Tune decay_period downward
            // if remote-input mispredicts feel too floaty under the Task-11 conditioner.
            PredictionManager {
                rollback_policy: lightyear::prelude::RollbackPolicy::default(),
                correction_policy: lightyear::prediction::correction::CorrectionPolicy::default(),
                ..default()
            },
```

(if `RollbackPolicy` isn't in the prelude, `lightyear::prediction::manager::RollbackPolicy`.)

- [ ] **Step 4: Remote spine lean from the remote's OWN pitch**

In `crates/arena_game/src/client/controller.rs`, `apply_aim_pitch_to_local_spine` currently leans REMOTE rigs with the LOCAL `AimPitch` resource (design P6). Change its signature/body: instead of `pitch: Res<AimPitch>`, walk from the bone to the owning rig root (the system already resolves the `ArenaBody` ancestor chain) and read that player's `ActionState<ArenaInput>` pitch:

- Add system param: `players: Query<&ActionState<ArenaInput>, With<crate::net::protocol::NetworkedPlayer>>` and, on the rig-root resolution path that identifies the owning player entity, use `players.get(player_entity).map(|a| a.0.pitch).unwrap_or(0.0)` as the lean angle instead of `pitch.0`. Keep the local-body skip exactly as-is. Add imports `use lightyear::prelude::input::native::ActionState; use crate::net::input::ArenaInput;`.
- If the current implementation does NOT resolve the owning player entity (only the rig root), extend the ancestry walk one hop: the rig root's `ChildOf` target IS the `NetworkedPlayer` entity (present.rs hangs the rig under the player).

- [ ] **Step 5: Build + gate + commit**

```bash
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer 2>&1 | tail -3
cargo test -p arena_game 2>&1 | tail -4
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-t9; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-t9
git add crates/arena_game/src/client/controller.rs crates/arena_game/src/client/harness.rs crates/arena_game/src/client/app_windowed.rs crates/arena_game/src/net/client.rs
git commit -m "feat(client): PostUpdate camera follow, teleport snap, explicit prediction policies, remote-pitch lean"
```

---

### Task 10: Customize de-hand-rolling (WS5)

**Files:**
- Modify: `crates/arena_game/src/net/protocol.rs` (delete `CustomizeBroadcast`)
- Modify: `crates/arena_game/src/server/customize.rs` (stop broadcasting)
- Modify: `crates/arena_game/src/client/net.rs` (replace `drain_customize_broadcasts` with a `Changed` tracer)

**Interfaces:**
- Produces: live `PlayerCustomization` edits propagate as plain component updates (single-entity model writes them directly; `SinceLastAck` resends until acked). Trace kind `customize_received { player }` survives, now sourced from `Changed<PlayerCustomization>`.

- [ ] **Step 1: Delete the broadcast**

- `protocol.rs`: delete the `CustomizeBroadcast` struct (lines 180-185), its `register_message` (lines 121-122), and rewrite the `PlayerCustomization` registration comment (lines 55-60) to: `// Live edits are plain component updates — the receive path writes registered components directly onto the (single) Predicted entity, and SinceLastAck resends unacked deltas. (The old "component updates are unreliable" CustomizeBroadcast workaround was inherited wisp lore — see the 2026-07-05 netcode spec.)`
- `server/customize.rs`: `drain_customize_requests` keeps updating the player's `PlayerCustomization` component but deletes the `CustomizeBroadcast` send + its `MessageSender` param.
- `client/net.rs`: replace `drain_customize_broadcasts` (lines 426-443) with:

```rust
/// Trace a remote player's live appearance change (replicated `PlayerCustomization` update). The
/// rig re-skin itself is `Changed`-driven in `client::parts`; this keeps the D6 harness signal.
fn trace_customize_updates(
    changed: Query<
        &NetworkedId,
        (
            With<NetworkedPlayer>,
            Changed<PlayerCustomization>,
            Without<LocalNetPlayer>,
        ),
    >,
) {
    for net_id in &changed {
        crate::trace::event(
            "customize_received",
            serde_json::json!({ "player": net_id.0 }),
        );
    }
}
```

swapping it into the plugin's system tuple. Remove the `CustomizeBroadcast` import.

- [ ] **Step 2: Verify the D6 round-trip headlessly**

```bash
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer 2>&1 | tail -3
session=/tmp/arena-net-t10
rm -rf $session && mkdir -p $session
target/debug/arena-server & sleep 2
ARENA_HEADLESS=1 ARENA_CLIENT_ID=1 ARENA_CUSTOMIZE=3 ARENA_TRACE_FILE=$session/observer-0.jsonl ARENA_TRACE_SRC=observer-0 target/debug/arena-observer & 
ARENA_HEADLESS=1 ARENA_CLIENT_ID=2 ARENA_TRACE_FILE=$session/observer-1.jsonl ARENA_TRACE_SRC=observer-1 target/debug/arena-observer &
sleep 6; kill %1 %2 %3 2>/dev/null
grep -c '"kind":"customize_received"' $session/observer-1.jsonl
```

Expected: ≥ 1 (observer-1 saw observer-0's live edit arrive as a component update). Then the standard gate:

```bash
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-t10b; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-t10b
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/arena_game/src/net/protocol.rs crates/arena_game/src/server/customize.rs crates/arena_game/src/client/net.rs
git commit -m "refactor(net): customization rides component replication — CustomizeBroadcast deleted"
```

---

### Task 11: Link conditioner + conditioned gate (WS6)

**Files:**
- Modify: `crates/arena_game/src/net/client.rs` (env-gated conditioner on the client link)
- Modify: `crates/arena_game/src/client/net.rs` (`trace_received_remote_pose` throttle 30→10)
- Create: `crates/arena_game/tools/net-test/run_conditioned.sh`

**Interfaces:**
- Consumes: `LinkConditionerConfig { incoming_latency, incoming_jitter, incoming_loss }` + `RecvLinkConditioner` (`lightyear_link-0.26.4/src/conditioner.rs`); `Link::new(Option<RecvLinkConditioner>)` (the arena already passes `Link::new(None)` at `net/client.rs:112`).
- Produces: env knobs `ARENA_NET_LATENCY_MS` / `ARENA_NET_JITTER_MS` / `ARENA_NET_LOSS` (client-side, incoming half of the RTT); a conditioned session script asserting the gate holds at 100 ms RTT.

- [ ] **Step 1: Conditioner from env**

In `crates/arena_game/src/net/client.rs`, add:

```rust
/// Optional artificial-latency conditioner (design WS6) so netcode feel/regressions are tested at
/// real RTTs instead of localhost-zero: `ARENA_NET_LATENCY_MS` (one-way incoming delay),
/// `ARENA_NET_JITTER_MS`, `ARENA_NET_LOSS` (0..1). Applied to the client's RECEIVE path only —
/// run both observers with latency L to simulate a symmetric 2·L RTT. Zero-cost when unset.
fn link_conditioner_from_env() -> Option<lightyear::link::RecvLinkConditioner> {
    let ms = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let latency = ms("ARENA_NET_LATENCY_MS");
    let jitter = ms("ARENA_NET_JITTER_MS");
    let loss = std::env::var("ARENA_NET_LOSS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok());
    if latency.is_none() && jitter.is_none() && loss.is_none() {
        return None;
    }
    Some(lightyear::link::RecvLinkConditioner::new(
        lightyear::link::LinkConditionerConfig {
            incoming_latency: core::time::Duration::from_millis(latency.unwrap_or(0)),
            incoming_jitter: core::time::Duration::from_millis(jitter.unwrap_or(0)),
            incoming_loss: loss.unwrap_or(0.0),
        },
    ))
}
```

and change `Link::new(None)` (line 112) to `Link::new(link_conditioner_from_env())`. (If the types live elsewhere, rustc will point at the re-export — `lightyear::prelude::{LinkConditionerConfig, RecvLinkConditioner}` is the likely alternative; keep the construction identical.)

- [ ] **Step 2: Denser remote-pose trace**

In `crates/arena_game/src/client/net.rs`, `trace_received_remote_pose` (line 493): `if *throttle % 30 == 1` → `if *throttle % 10 == 1`, and update its doc line to say "every 10th change" (the `ts` field each trace line already carries is what freshness assertions diff).

- [ ] **Step 3: Conditioned session script**

Create `crates/arena_game/tools/net-test/run_conditioned.sh`:

```bash
#!/usr/bin/env bash
# Conditioned net gate (design WS6): the standard session under ~100ms RTT + jitter + loss.
# Both observers get 50ms incoming latency (≈100ms RTT), 10ms jitter, 2% loss. The M2 gate
# assertions must still hold — casts ride the redundant input stream, events ride reliable
# channels, so nothing may be lost (only delayed).
set -uo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
session="${1:-/tmp/arena-net-conditioned}"
export ARENA_NET_LATENCY_MS="${ARENA_NET_LATENCY_MS:-50}"
export ARENA_NET_JITTER_MS="${ARENA_NET_JITTER_MS:-10}"
export ARENA_NET_LOSS="${ARENA_NET_LOSS:-0.02}"
export ARENA_NET_TEST_DURATION="${ARENA_NET_TEST_DURATION:-12}"
bash "$script_dir/run_session.sh" "$session" || true
bash "$script_dir/check_session.sh" "$session"
```

```bash
chmod +x crates/arena_game/tools/net-test/run_conditioned.sh
```

`run_session.sh` launches the observers with the parent environment, so the exported `ARENA_NET_*` vars reach them (verify: `grep -n "ARENA_HEADLESS=1" crates/arena_game/tools/net-test/run_session.sh` — the per-process env prefixes ADD vars, they don't sanitize). The longer 12 s duration absorbs the added RTT.

- [ ] **Step 4: Run both gates**

```bash
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer 2>&1 | tail -3
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-t11; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-t11
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_conditioned.sh /tmp/arena-net-cond
```

Expected: both print `PASS`. If the conditioned run fails on missing casts, first suspect the autocast latch riding `tap_latch` across a lossy release — check `cast_edge` counts on the server vs `cast_edge_sent` on observer-0 (they should match within one).

- [ ] **Step 5: Commit**

```bash
git add crates/arena_game/src/net/client.rs crates/arena_game/src/client/net.rs crates/arena_game/tools/net-test/run_conditioned.sh
git commit -m "test(net): env-gated link conditioner + conditioned session gate (100ms RTT/jitter/loss)"
```

---

### Task 12: Docs — CLAUDE.md netcode rewrite + spec cross-link

**Files:**
- Modify: `crates/arena_game/CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-07-05-netcode-feel-overhaul-design.md` (status line)

- [ ] **Step 1: Rewrite the stale netcode sections**

In `crates/arena_game/CLAUDE.md` update (keeping the file's voice + table format):
- Header (§1): "server-authoritative with lightyear-native client prediction for BOTH players (PredictionTarget::All + rebroadcast inputs); nothing is interpolated."
- §Netcode connection→spawn→replication step 3/4: `PredictionTarget::to_clients(All)`, no InterpolationTarget; both clients materialize both players as `Predicted` (`ActionState` inserted client-side on the non-controlled one).
- §What replicates: note `ArenaInput` v2 fields; `CastRequestMessage`/`CastChannel`/`CustomizeBroadcast` GONE — casts are the `charging` falling edge (server `detect_cast_edges`, `PrevCastInput`), charge from held ticks, aim from yaw+pitch (`net::aim_dir`), skills by `ARENA_SKILLS` slot; `RequestChannel` carries `CustomizeMessage` only; customization updates ride component replication.
- §Cast pipeline: input-edge flow + predicted cue scheduler (`PredictedCues` registry de-dup, fizzle on `CastRejected`), single `NetEventMessage` drain → `ClientNetEvent` fan-out.
- §Movement: unchanged controller; remotes now predicted from rebroadcast inputs; camera follow lives in PostUpdate after `PhysicsSystems::Writeback`; `snap_large_corrections` teleport snap; `REPLICATION_SEND_HZ = 30`.
- §Key invariants: update footgun 8 (NetEventMessage single drain = `skills::drain_net_events`); add "InputMarker only on the Controlled entity"; add the conditioner env knobs + `check_session.sh`/`run_conditioned.sh` to the net-test section, noting `summarize.py` remains the CI gate.
- Fix any other line the migration falsified (grep the file for `Interpolated`, `CastRequestMessage`, `CustomizeBroadcast`, `CastChannel`, `pending_charge`, `cast_request_sent`, `100ms`).

- [ ] **Step 2: Mark the spec implemented**

In the spec's `**Status:**` line: `implemented on feat/netcode-feel-overhaul (see docs/superpowers/plans/2026-07-05-netcode-feel-overhaul.md)`.

- [ ] **Step 3: Final full gate + commit**

```bash
cargo test -p arena_game -p arena_sim 2>&1 | tail -4
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-final; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-final
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_conditioned.sh /tmp/arena-net-final-cond
git add crates/arena_game/CLAUDE.md docs/superpowers/specs/2026-07-05-netcode-feel-overhaul-design.md
git commit -m "docs: netcode CLAUDE.md rewrite for the feel overhaul (predict-both, input casts)"
```

Expected: everything PASS.

---

## Execution deviation notes (what reality added)

1. **Task 2 discovered a blocker + fix:** enabling `rebroadcast_inputs` left the client's input timeline only ~⅓ tick ahead at LAN RTT (sync objective = `server + rtt/2 + 5ms default jitter_margin`), and 0.26.4's server-side `InputBuffer::get(tick)` has NO last-value fallback — inputs arrived one tick late forever and the player froze server-side. Fix: `InputTimelineConfig` with a **25ms jitter_margin** on the client entity (`net/client.rs`) — ≥1.5 ticks of ahead-slack, zero local input delay. Root-caused via trace-level probes; bisect confirmed the flag (not the targeting) as the trigger.
2. **Task 8 needed the headless client to compose the obelisk CLIENT subset** (`add_obelisk_sim_client` + config/effects/skills + cast-timeline loading, mirroring the windowed root) — `predicted_local_cast` reads `CastTimelineHandles`/`SkillRegistry`, which the observer never had. Side benefit: the net-test now exercises the real predicted path (9 `predicted_cast` + 9 `predicted_cue` + de-dups per session). `predicted_local_cast`'s timeline params are also `Option`-wrapped (defensive).
3. **Task 11's conditioned gate immediately caught a real bug:** `local_cast_edge` re-fired during rollback re-simulation (31 client edges for 8 real casts → duplicate predicted cosmetics under loss). Fixed with a `With<Rollback>` guard — recorded as CLAUDE.md invariant 14.
4. `phase_start` takes obelisk's `WindowPhase` (Windup/Active/Recovery), not `SkillPhase` — the plan's test was adjusted accordingly.

## Self-review notes (already applied)

- Spec WS1-WS6 → Tasks 2-3 / 4-6 / 7-8 / 9 / 10 / 11; Task 1 is the spec's phase 0; Task 12 the doc phase. The spec's "InputTimelineConfig knob" is deliberately NOT set (documented in Task 2 step 1 comment) — no task needed.
- Type consistency: `PrevCastInput`/`step_cast_edge` (Task 5) mirror `local_cast_edge`'s `Local<(bool,u32,u8)>` (Task 6); `charge_byte_from_hold_ticks` defined Task 4, used Tasks 5/6; `ClientNetEvent` defined Task 7, consumed Task 8; `PredictedCast.charge` added Task 6, consumed Task 8; `RequestChannel` renamed Task 6, used by `send_customization` same task.
- Two known compile-risk points carry explicit fallbacks in-place: the borrow of `players` inside Task 5's closures (hoist a HashSet/HashMap) and the `PhysicsSystems`/`RollbackSystems`/conditioner import paths (rustc-suggested re-export; behavior pinned).
