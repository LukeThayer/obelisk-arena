# Levels & Lobby — Design

**Goal:** author playable levels in the editor (bevy_modal_editor) and tie them into the game: players spawn into a designed **lobby** level; the **first player to join is the host**; the host presses **G** to pick a level and start the PvP match there. Multiple selectable arenas are the expected future; the infrastructure must make "add an arena" = "save a new scene in the editor".

**Status:** approved (user directed "plan and build"); implementation follows the companion plan.

---

## 1. Research facts the design stands on (verified 2026-07-06)

- **Editor scene format**: a Bevy `DynamicScene` serialized to RON (`.scn.ron`) + a `.meta` RON sidecar (`EditorMetadata { camera_marks, material_library, camera_render_settings }`). Saved via the command palette (`C` → Save Scene) to CWD-relative `assets/scenes/<name>.scn.ron`. Multiple named scenes, save-as, overwrite confirmation all exist.
- **What's in the file**: markers only — `SceneEntity`, `Name`, `Transform`, `RigidBody`, `PrimitiveMarker { shape: PrimitiveShape }`, `MaterialRef` (`Library(name)` or `Inline(MaterialDefinition)`), group/light markers, hierarchy, plus **game-registered components** (`SceneComponentRegistry`). **`Collider`/`Mesh3d`/materials are NEVER serialized** — the editor regenerates them at load (`regenerate_runtime_components`). `PrimitiveShape::create_collider()` lives in `bevy_effect` and is **avian-only (no mesh/render needed)**.
- **Custom game components round-trip**: `register_custom_entity::<T>()` (bevy_editor_game) puts `T` in the save allow-list, the insert palette ("Game" category), and supports gizmo/inspector/regenerate hooks. marble_demo's `SpawnPoint`/`GoalZone` are shipped proof.
- **No headless loader exists**: the only load path is inside `EditorPlugin` (egui + render mandatory). A consuming game must bring its own slim loader.
- **Type-path coupling**: `DynamicScene` RON stores full type paths; deserialization hard-errors on types missing from the `TypeRegistry`. `MaterialRef`/`MaterialDefinition` already live in the types-only `bevy_editor_game` crate; `SceneEntity`/`PrimitiveMarker`/group/light markers live in the editor crate today.
- **CWD trap**: the editor's save/scan browser is CWD-relative; launched from `crates/arena_editor` (required for its preset loading), levels land in `crates/arena_editor/assets/scenes/`. The game runs from the workspace root. Same dual-root situation as the vfx presets (game scans `["assets/vfx", "assets/skills"]`).
- **Arena surfaces that hard-code the flat world**: `arena_sim::spawn::{SPAWN_MARKERS, spawn_arena_floor}` (3 spawn sites: server startup, headless client, windowed `setup_scene` — which also spawns a 20×20 visual plane); the controller's flat ground check (`pos_y <= GROUND_Y + 0.05`); `report_ground_hits`' flat plane (`y < 0.0`); `reset_for_new_round` teleporting to `SPAWN_MARKERS`.
- **Flow surfaces**: `RoundPhase { WaitingForPlayers, Countdown, Active, RoundOver, MatchOver(terminal) }` server FSM broadcast as `RoundStateMessage`; `RequestChannel` (C→S) carries `CustomizeMessage`; windowed panel precedent = the K-toggled customizer (`CustomizationOpen` gate); free key: **G**.
- **net-test contract**: observer-0 is `ARENA_CLIENT_ID=1` and connects first (= will be host); assertions depend on the exact flat-floor trajectory (`caster=player_1` firebolt damage 20.0 at seed 42).

## 2. Goals / non-goals

**Goals**
1. Levels are authored **in the editor's native flow** (Insert primitives → place `ArenaSpawnPoint`s → palette Save Scene). No export step.
2. The same level file drives **all three peers**: headless server (colliders + spawn points), windowed client (visuals + colliders), headless observer (colliders).
3. Game flow: connect → spawn in the **lobby** level → host (first joiner) presses **G**, picks a level → countdown → the existing best-of-3 match runs in that level → MatchOver → everyone returns to the lobby.
4. Adding an arena = saving a new `.scn.ron` with ≥2 `ArenaSpawnPoint`s. It appears in the host's list automatically.
5. Movement and projectiles behave on non-flat levels (raycast ground check; world hits against real level geometry).
6. The net-test gate stays green throughout (a shipped `arena_flat` level reproduces today's exact geometry).

**Non-goals (v1)**
- GLTF/prefab/spline/splat/decal/blockout objects in arena levels (the loader supports primitives + lights + groups + arena markers; anything else fails loud with the offending type named). Blockout doesn't round-trip in the editor itself today.
- Textured level materials (inline/library PBR colors work; texture paths resolve only if the relative path exists under the arena asset root — untextured is the v1 recommendation).
- Spectators, >2 players, level voting, mid-match level change, lobby minigames.
- Editing levels live while the game runs.

## 3. Approaches considered

**A (chosen): native `.scn.ron` + a slim arena loader + a small upstream type move.** Move the serializable scene marker types (`SceneEntity`, `PrimitiveMarker`, `GroupMarker`, `SceneLightMarker`, `DirectionalLightMarker`) from the editor crate into `bevy_editor_game` (the types-only "shared vocabulary" crate — its stated charter), pinning their old `type_path`s so existing scenes keep loading; put the egui-dependent items in `bevy_editor_game` behind an on-by-default `egui` feature. The game then depends on `bevy_editor_game` (default-features=false) + `bevy_effect` (already a dep) and deserializes scenes with Bevy's `SceneDeserializer` + a registry of exactly the supported types, spawning colliders via `PrimitiveShape::create_collider()` (server) and meshes/materials via `create_mesh()` + `MaterialRef` resolution (client). Editor authoring flow untouched.

**B (rejected): the game depends on the full editor crate and reuses its load path.** Pulls egui + the editor's render stack into the game and the headless server (which cannot even run `EditorPlugin`); the server would still need a custom collider-only pass.

**C (rejected): arena-specific export format.** An "Export Level" step in the editor writing a game-native RON. Full control and no shared types, but adds a lossy export step to every iteration, diverges from the editor ecosystem (prefabs/custom entities wouldn't carry), and duplicates the scene model.

## 4. Design

### 4.1 Level content contract

- A **level** is `<name>.scn.ron` (+ `.meta`) authored in the editor. Supported object kinds: primitives (`PrimitiveMarker` + `MaterialRef` + `RigidBody` + `Transform` + `Name`), groups, point/directional lights, and the arena marker below. The loader pre-scans the RON and rejects a level naming unsupported component types with a clear error.
- **`ArenaSpawnPoint { slot: u8 }`** — new component in `arena_sim` (shared by game + editor shell; no type-path pinning needed). Registered in the editor by the `arena_editor` shell via `register_custom_entity` (palette-insertable under "Game", sphere gizmo + slot label, no collider). Semantics: **match levels** need slots 0 and 1 (duelist spawns, faction by slot as today); **the lobby** uses any number ≥1, players placed round-robin by sorted-id index modulo count. Facing: the spawn point's Transform yaw is applied to the spawned body.
- **Shipped levels** (committed): `lobby.scn.ron` (floor + perimeter walls + a few pillars + 4 spawn points) and `arena_flat.scn.ron` (exact replica of today's geometry: 40×40×1 floor cuboid, top face at y=0, spawns at (−4, GROUND_Y, 0) and (4, GROUND_Y, 0)) — hand-authored RON in the editor's exact serialization shape, then round-trip-verified in the editor.
- **Locations**: the game scans `["assets/scenes", "crates/arena_editor/assets/scenes"]` (CWD = workspace root), mirroring the established dual-root vfx-preset pattern; later-scanned duplicates by stem are ignored with a warn. The shipped levels live in `assets/scenes/` (workspace root); levels the user saves from the editor land in `crates/arena_editor/assets/scenes/` and are picked up automatically. Level id = file stem; `lobby` is reserved (never listed for selection).

### 4.2 Upstream: bevy_editor_game/bevy_modal_editor changes

1. Move `SceneEntity`, `PrimitiveMarker`, `GroupMarker`, `SceneLightMarker`, `DirectionalLightMarker` into `bevy_editor_game::scene_types`, each with `#[type_path]` pinned to its ORIGINAL path (e.g. `bevy_modal_editor::scene::primitives`) so every existing `.scn.ron` (demos included) still deserializes. The editor re-exports them from the old modules; editor test suite must stay green.
2. Gate egui-dependent items in `bevy_editor_game` (`InspectorWidgetFn`, the inspector/palette fn-pointer fields of `CustomEntityType`) behind an `egui` feature, default-on. The editor uses default features; the arena game uses `default-features = false`.
3. No other editor changes: save browser, palette, custom-entity registration are used as-is.

### 4.3 Arena loader (`arena_sim::level`)

- `LevelCatalog` (resource): scan of the two scene dirs → `Vec<LevelInfo { id, path }>`; `lobby()` accessor; selectable list = all minus `lobby`.
- `load_level_scene(path) -> Result<LevelScene, LevelError>`: read file → preflight `ron::Value` walk (collect unknown type paths → error listing them) → `SceneDeserializer` with a purpose-built `TypeRegistry` (the §4.1 types + `Transform`, `Name`, `RigidBody`, `ChildOf`, `Children`, `ArenaSpawnPoint`, `MaterialRef`) → `DynamicScene` → extract a plain-data `LevelScene { statics: Vec<StaticDesc>, lights: Vec<LightDesc>, spawns: Vec<SpawnDesc> }` (world-space transforms resolved through the hierarchy). `.meta` parsed with a tolerant serde struct `{ material_library }` for `MaterialRef::Library` resolution.
- `spawn_level(commands, &LevelScene, mode)` where `mode ∈ { Physics, PhysicsAndVisuals }`:
  - Physics (all peers): per static — `RigidBody::Static` + `PrimitiveShape::create_collider()` + avian `Position`/`Rotation` (invariant: write avian Position, never Transform) + `Name` + **`LevelEntity`** tag (the despawn key for level switches).
  - Visuals (windowed client only): additionally `Mesh3d(create_mesh())` + `MeshMaterial3d(StandardMaterial from MaterialRef)`; lights spawned from the light markers.
  - Returns `LevelSpawns { slots: Vec<(u8, Vec3, f32 /*yaw*/)> }`.
- `despawn_level(commands, Query<Entity, With<LevelEntity>>)` — switch = despawn + spawn. Level load is synchronous (local RON read) on all peers.

### 4.4 Game flow (server-authoritative)

- `RoundPhase` v2: `Lobby` replaces `WaitingForPlayers` as the initial + between-matches phase (any player count; ≥2 required to start). `MatchOver { winner, remaining }` gains a timer (`MATCH_OVER_SECS = 6.0`) and then returns everyone to the lobby (load lobby level, teleport, phase = `Lobby`). Countdown/Active/RoundOver unchanged. Mid-match disconnect below 2 players → back to `Lobby` (lobby level reloaded).
- **Host**: server tracks connect ORDER (monotonic counter per connect); host = lowest-order connected client. Host leaves → next in order. Kept in a `HostState` resource.
- **Level state**: server `CurrentLevel { id }` resource; starts as `lobby` (loaded at startup, replacing the hardcoded floor spawn). Players connecting spawn at the CURRENT level's spawn slots (round-robin), not at `SPAWN_MARKERS`.
- **Start command**: `StartMatchMessage { level: String }` (C→S, `RequestChannel`, reliable). Server accepts iff sender == host ∧ phase == `Lobby` ∧ ≥2 players ∧ level ∈ catalog ∧ level has ≥2 spawn slots (validated at load). On accept: despawn `LevelEntity`s, load the level (Physics mode), store `LevelSpawns`, reset scores, phase = `Countdown` (the existing Countdown→Active edge reset teleports players — now to the new level's slots).
- **Wire**: `RoundStateMessage` v2 gains `host: u64` (client id) and `level: String`; wire phase tag 0 becomes `Lobby` (5 = unused). Clients react to `level` changes by despawning their `LevelEntity`s and loading the named level locally (visuals+colliders windowed; colliders headless). Late joiners receive the current state via the existing dirty-broadcast (a join marks it dirty).
- `reset_for_new_round` and connect-spawn placement read `LevelSpawns` (slot-by-sorted-id for matches; round-robin for lobby) instead of `SPAWN_MARKERS`.

### 4.5 Movement & projectiles on real levels

- **Ground check** (`arena_sim::shared_controller`): replace the flat `pos_y <= GROUND_Y + 0.05` with a `SpatialQuery` raycast straight down from the capsule bottom (max distance 0.1 + skin), excluding the caster's own body + hurtbox child (same exclusion pattern as the cast raycasts). Runs identically on server and predicted clients; against static geometry the query is rollback-safe (static colliders don't move). Standing on the other player counts as grounded (server resolves those collisions anyway).
- **World hits** (`arena_sim::report_ground_hits` → `report_world_hits`): per projectile tick, raycast the movement segment (prev→current position) against non-sensor colliders excluding combatants/hurtboxes; a hit triggers `HitboxWorldHit` at the impact point (walls and floors now stop firebolts). Keep a kill-plane at `y < -10` (fell out of the world → world hit at the clamp point). The editor skill-preview stage keeps its own flat-plane copy (its stage is flat).
- The windowed `setup_scene` drops its hardcoded 20×20 plane + floor collider (levels own the world); light/camera stay.

### 4.6 Client UX (windowed)

- **Lobby HUD banner**: host sees "LOBBY — press G to choose an arena"; others "LOBBY — waiting for host". (Client knows it's host by comparing `RoundStateMessage.host` to its own client id.)
- **G panel** (mirrors the K customizer): host-only, `Lobby` phase only. Lists the catalog (client-side scan — same files); Up/Down or 1-9 to highlight, Enter/click to send `StartMatchMessage`, G/Esc closes. Movement input gated off while open (same as customizer).
- MatchOver banner shows "returning to lobby…" during the timer.

### 4.7 Harness / tests

- `ARENA_AUTOSTART_LEVEL=<id>` (headless client): while phase == `Lobby` and ≥1 remote player exists, the HOST observer sends `StartMatchMessage { level }` once per lobby visit (so full-match → lobby → restart loops keep working). run_session.sh sets `ARENA_AUTOSTART_LEVEL=arena_flat` on observer-0.
- `check_session.sh`/`summarize.py` assertions unchanged — `arena_flat` reproduces today's trajectories bit-for-bit (same floor top, same spawn slots, same seed ⇒ same 20.0 damage).
- New trace kinds: `level_loaded { id, statics, spawns }` (all peers), `host_elected { client_id }`, `match_started { level }` (server).
- Unit tests: catalog scan + reserved-lobby filtering; loader on a fixture RON (statics/spawns/world-space transforms/unknown-type rejection); host election order + re-election; `RoundPhase::Lobby` transitions incl. MatchOver→Lobby timer; spawn-slot round-robin; ground-raycast helper; segment world-hit helper. Editor-side: moved-type `type_path` pinning (old demo scene snippet still deserializes); `ArenaSpawnPoint` round-trips through save.

## 5. Risks

1. **`DynamicScene` deserialization strictness** — mitigated by the preflight unknown-type scan (clear authoring error) and the documented supported-object contract.
2. **Editor scenes' reflection RON shape vs the slim registry** (e.g. `Transform`/`RigidBody` registration needs the exact avian/bevy types) — mitigated: fixture test deserializes a REAL editor-saved scene early in the plan (fail-fast gate, like the netcode spike).
3. **Trajectory drift breaking the net-test** — `arena_flat` copies today's numbers exactly; the gate runs after every phase; the world-hit/ground-check changes are covered by the same gate (flat level ⇒ same behavior) plus dedicated unit tests.
4. **Type move breaking the editor** — `type_path` pinning + the editor's 175-test suite + demo-scene load check.
5. **Two save roots confusing users** — dual-dir scan + a `level_loaded` trace naming the winning path; documented in CLAUDE.md.

## 6. Execution order

1. Upstream: `bevy_editor_game` type move + egui feature gate; editor re-exports; suites green (fail-fast for approach A).
2. `arena_sim`: `ArenaSpawnPoint`, `level` module (catalog/loader/spawner) + fixture tests, including a REAL editor-saved fixture.
3. Shipped levels (`lobby`, `arena_flat`) + editor round-trip verification (probe screenshot).
4. Server flow: lobby phase, host election, level switching, spawns-from-level, `StartMatchMessage`; wire v2.
5. Clients: level loading on state change, windowed visuals, G panel + banners; headless autostart hook.
6. Movement/projectile generalization (ground raycast, world-hit segments).
7. Harness update + full gates (clean + conditioned) + CLAUDE.md/docs refresh.
