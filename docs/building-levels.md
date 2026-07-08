# Building levels for obelisk-arena

How to author a level (an arena or the lobby) in the editor and play it in the game. Levels are
bevy `DynamicScene` RON files (`.scn.ron`); every peer ships the same files and only the level *id*
travels over the network.

## TL;DR

```bash
cd crates/arena_editor && cargo run          # launch the editor
# build the level (below), place spawn points, then: C → "save" → type a name → Enter
# the file lands in crates/arena_editor/assets/scenes/<name>.scn.ron

cd ../..                                     # back to the repo root
cargo run --bin arena-server                 # terminal 1
cargo run --bin arena-client                 # terminal 2 (first to connect = host)
cargo run --bin arena-client                 # terminal 3
# in the host's window: G → arrows to pick your level → Enter. Fight!
```

New levels appear in the host's `G` menu automatically — the catalog is scanned at startup, so
**restart the server + clients after saving a new level** (there is no hot reload).

## 1. Launch the editor

```bash
cd crates/arena_editor
cargo run
```

The editor is keyboard-first and modal (vim-style). The two keys to remember:

- `C` — command palette (fuzzy-searchable access to everything, including save/load)
- `?` — hotkey cheat-sheet for the current mode

## 2. Build the geometry

Insert primitives with `I` (Insert mode opens the palette automatically): type `cube`, pick it,
move the preview into place, click to place (Shift+Click places several). Select an object and
press `E` for Edit mode: `Q` translate / `W` rotate / `E` scale, constrain with `A`/`S`/`D` (X/Y/Z),
click to confirm. `Ctrl+D` duplicates; arrow keys nudge on the XZ plane. `M` edits the selected
object's material (PBR color/metallic/roughness, or `F` for the material-library presets).

Rules that matter to the game:

- **Stick to plain primitives** (cube, sphere, cylinder, capsule, plane) plus **lights** and
  **groups**. That's the v1 level contract — anything else in the file (gltf models, splines,
  splats, prefabs, blockout stairs/ramps/arches) makes the game refuse the level with an error
  naming the offending type. Inserted primitives are already `RigidBody::Static`, which is what
  the game expects; colliders are regenerated at load, never stored.
- **Scale freely.** A cube scaled to (40, 1, 40) is a floor. The game bakes the scale into the
  collider shape and mesh at load — you don't have to think about it.
- **Floor convention:** put your main floor's TOP face at world y = 0 (e.g. a 1-thick cube at
  y = −0.5). Not mandatory — the ground check is a raycast, so platforms and ramps at any height
  work — but it keeps spawn-point heights consistent across levels.
- **Stay above y ≈ −9.** Anything below y = −10 is the projectile kill plane, and a player who
  falls out of the world just falls (there's no respawn-on-fall yet).
- **Lights are level data.** Add a directional light ("sun") and any point lights in the editor —
  the game renders exactly what you author (the client no longer has a built-in light). No lights
  = a very dark arena.

## 3. Place spawn points

Press `I` and type `spawn` — insert an **Arena Spawn Point** (it lives in the palette's "Game"
category). It renders as a lime sphere with a forward arrow.

- **Position:** keep the marker ~0.6 m above the floor surface (the palette default height). The
  marker's position is where the player's body *center* is placed; a capsule is 0.59 half-height,
  so ~0.6 above the floor puts feet on the ground.
- **Facing:** the arrow is the direction the player looks on spawn. Rotate the marker (Edit mode,
  `W`) to aim it — e.g. the two duel spawns should face each other.
- **Slot:** select the marker, press `O` (Inspector mode), and edit the `ArenaSpawnPoint`
  component's `slot` field.
  - **Arena (match) levels need exactly slots `0` and `1`** — the two duelists. The server
    refuses to start a match on a level that lacks either.
  - **The lobby can have any number of points** (players are placed round-robin), any slots.

## 4. Save

`C` → type `save` → Enter → type a filename → Enter. The file is written to
`crates/arena_editor/assets/scenes/<name>.scn.ron` (plus a `.meta` sidecar carrying the material
library — keep the two files together if you ever move a level by hand).

**The filename stem is the level id** — `frost_pit.scn.ron` shows up as `frost_pit` in the host's
G menu. Two names are special:

- `lobby` — the level everyone hangs out in between matches. Reserved: never offered in the G menu.
- `arena_flat` — the shipped default duel arena.

## 5. Play it

The game scans two directories at startup, in order:

1. `assets/scenes/` (repo root) — the shipped levels (`lobby`, `arena_flat`)
2. `crates/arena_editor/assets/scenes/` — your editor saves

First root wins on a name collision. So:

- **A new arena** needs nothing extra — save it in the editor, restart server + clients, and the
  host sees it under `G`.
- **Editing a shipped level (e.g. redesigning the lobby):** the shipped copy shadows an editor
  save of the same name, and the editor's load browser only sees its own directory. Copy it in,
  edit, copy it back:

  ```bash
  cp assets/scenes/lobby.scn.ron crates/arena_editor/assets/scenes/
  # edit in the editor (C → "load" → lobby), save, then ship it:
  cp "crates/arena_editor/assets/scenes/lobby.scn.ron" assets/scenes/
  cp "crates/arena_editor/assets/scenes/lobby.scn.ron.meta" assets/scenes/ 2>/dev/null || true
  ```

The in-game flow: everyone connects into the **lobby**; the first player to join is the **host**
(the lobby banner tells the host to press `G`, everyone else waits). `G` opens the level list —
arrows to highlight, Enter to start (Escape/G closes). Match is best-of-3; after MATCH OVER the
banner counts down ~6 s and everyone returns to the lobby for the next pick.

### Headless / scripted testing

`ARENA_AUTOSTART_LEVEL=<id>` makes a peer request that level automatically whenever it's the
elected host and both players stand in the lobby (once per lobby visit) — the harness's stand-in
for pressing G. The net-test uses it (`crates/arena_game/tools/net-test/run_session.sh` sets
`arena_flat` on both observers); it's equally handy for soak-testing your own level:

```bash
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/mylevel-session
bash crates/arena_game/tools/net-test/check_session.sh /tmp/mylevel-session
```

(Edit the `ARENA_AUTOSTART_LEVEL` value in `run_session.sh`, or export it, to point at your level;
the gate's damage assertions assume `arena_flat`'s spawn layout, so expect `PASS` only there.)

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| Level missing from the G menu | Server/clients not restarted after saving; or the file isn't in one of the two scan roots; or it's named `lobby` (reserved). |
| Server log: `level uses unsupported component types (…)` | The scene contains non-v1 content (gltf/spline/splat/prefab/blockout tiles). Rebuild those parts from plain primitives. |
| Server log: `start_match: level '…' lacks match spawn slots 0+1` | Add two Arena Spawn Points and set their `slot` fields to 0 and 1 (Inspector mode, `O`). |
| Players spawn inside the floor / fall forever | Spawn markers placed at floor level or below — raise them to ~0.6 m above the surface. |
| Arena renders pitch black | No lights authored in the level — add a directional light in the editor. |
| G does nothing | Only the elected host, only during the lobby phase. The banner says which you are. |
