# Levels & Lobby Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Editor-authored `.scn.ron` levels load on all three arena peers; players spawn in a designed lobby; the first joiner (host) presses G to pick a level and start the best-of-3 match there; MatchOver returns everyone to the lobby.

**Architecture:** Spec: `docs/superpowers/specs/2026-07-06-levels-and-lobby-design.md`. Serializable scene marker types move upstream into `bevy_editor_game` (type_paths pinned; egui feature-gated) so the game can deserialize scenes without the editor crate. A slim `arena_sim::level` module (catalog → preflight → `SceneDeserializer` → plain-data `LevelScene` → per-peer spawner) drives server colliders and client visuals from the same file. The server round FSM gains `Lobby` (initial + post-match), host election by connect order, and level switching on a host-only `StartMatchMessage`.

**Tech Stack:** Rust, Bevy 0.18.1, avian3d 0.5, lightyear 0.26.4, bevy_editor_game (types) + bevy_effect (`PrimitiveShape`) from the LukeThayer/bevy_modal_editor fork.

## Global Constraints

- Never break the net-test contract (`crates/arena_game/CLAUDE.md`): existing trace kinds keep names+fields; `summarize.py` untouched; gate via `check_session.sh` after every arena task; `run_conditioned.sh` at the end. `arena_flat` must reproduce today's geometry exactly (floor cuboid 40×40×1 top at y=0; spawns (−4, GROUND_Y, 0) / (4, GROUND_Y, 0), GROUND_Y = 0.59).
- Write avian `Position`, never `Transform`, for anything physical (CLAUDE.md invariant 1). Level statics bake `Transform.scale` INTO the collider (`Collider::set_scale`) because the arena disables avian's transform sync plugins.
- Single-drain rule for lightyear `MessageReceiver`s (invariant 8). `StartMatchMessage`'s only drain is the new server system.
- `PROTOCOL_ID` bumps 2 → 3 (RoundStateMessage v2 + StartMatchMessage).
- Branch `feat/levels-and-lobby` in this checkout (no worktree — cold builds are 10+ min). The user's untracked `crates/arena_editor/assets/vfx/*.vfx.ron` files must never be staged. The ROOT `Cargo.lock` legitimately changes in Task 3 (new deps) — layer on top of the user's existing modification, never revert it.
- Cross-repo flow: edit `~/src/bevy_modal_editor`, push `lukethayer main`; edit `~/src/obelisk-bevy` if ever needed, push `origin main`; then repin consumers. Upstream work lands (and is pushed) BEFORE the arena consumes it — no path patches.
- Editor levels are saved CWD-relative: user-authored levels land in `crates/arena_editor/assets/scenes/`; shipped levels live in `assets/scenes/`. The game scans BOTH (workspace-root CWD), first-found-by-stem wins, duplicate stems warn.
- Reserved level id: `lobby` (never selectable). Supported level content: primitives + groups + point/directional lights + `ArenaSpawnPoint`; unknown component types fail the load with the offending type paths named.

---

### Task 1: Upstream — move scene marker types into bevy_editor_game (type_paths pinned) + egui feature gate

**Files (all in `~/src/bevy_modal_editor`):**
- Create: `crates/bevy_editor_game/src/scene_types.rs`
- Modify: `crates/bevy_editor_game/src/lib.rs` (module + re-exports + egui gating), `crates/bevy_editor_game/Cargo.toml` (egui optional)
- Modify: `src/scene/mod.rs` (delete `SceneEntity` def; re-export), `src/scene/primitives.rs` (delete `GroupMarker`/`Locked`/`SceneLightMarker`/`DirectionalLightMarker`/`PrimitiveMarker` defs; re-export)
- Test: `crates/bevy_editor_game/src/scene_types.rs` unit test + existing editor suite

**Interfaces:**
- Produces (consumed by arena Tasks 3-4): `bevy_editor_game::scene_types::{SceneEntity, PrimitiveMarker, GroupMarker, Locked, SceneLightMarker, DirectionalLightMarker}` — identical shapes to today's editor types, each `#[type_path]`-pinned to its ORIGINAL path so existing `.scn.ron` files (and newly saved ones) deserialize unchanged. `bevy_editor_game` builds with `default-features = false` (no egui).

- [ ] **Step 1: Write the moved types with pinned type paths**

`crates/bevy_editor_game/src/scene_types.rs` (shapes copied verbatim from `src/scene/mod.rs:37-40` and `src/scene/primitives.rs:19-61,270-276`; only the derives' imports and the `#[type_path]` pins are new):

```rust
//! Serializable scene marker components — the shared vocabulary between the editor's scene
//! serialization and any GAME that loads editor-authored `.scn.ron` scenes (e.g. obelisk-arena's
//! level loader). Moved here from `bevy_modal_editor::scene`; every type's `type_path` is PINNED
//! to its original module path because `DynamicScene` RON stores full type paths — existing
//! saved scenes must keep deserializing, and consumers register these types under those paths.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use bevy_effect::primitive::PrimitiveShape;

/// Marker component for entities that are part of the editable scene.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene"]
pub struct SceneEntity;

/// Component to track what primitive shape an entity is.
#[derive(Component, Serialize, Deserialize, Clone, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct PrimitiveMarker {
    pub shape: PrimitiveShape,
}

/// Marker component for group entities (containers for nesting).
#[derive(Component, Serialize, Deserialize, Clone, Default, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct GroupMarker;

/// Marker component for locked entities (prevents editing).
#[derive(Component, Serialize, Deserialize, Clone, Default, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct Locked;

/// Marker component for point lights.
#[derive(Component, Serialize, Deserialize, Clone, Reflect)]
#[reflect(Component, Default)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct SceneLightMarker {
    pub color: Color,
    pub intensity: f32,
    pub range: f32,
    pub shadows_enabled: bool,
    #[serde(default)]
    pub radius: f32,
}

/// Marker component for directional lights (sun).
#[derive(Component, Serialize, Deserialize, Clone, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct DirectionalLightMarker {
    pub color: Color,
    pub illuminance: f32,
    pub shadows_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    /// The pins ARE the contract: existing `.scn.ron` files reference these exact paths.
    #[test]
    fn type_paths_are_pinned_to_original_editor_modules() {
        assert_eq!(SceneEntity::type_path(), "bevy_modal_editor::scene::SceneEntity");
        assert_eq!(
            PrimitiveMarker::type_path(),
            "bevy_modal_editor::scene::primitives::PrimitiveMarker"
        );
        assert_eq!(
            SceneLightMarker::type_path(),
            "bevy_modal_editor::scene::primitives::SceneLightMarker"
        );
        assert_eq!(
            DirectionalLightMarker::type_path(),
            "bevy_modal_editor::scene::primitives::DirectionalLightMarker"
        );
        assert_eq!(GroupMarker::type_path(), "bevy_modal_editor::scene::primitives::GroupMarker");
    }
}
```

Copy the exact `Default` impls for `SceneLightMarker`/`DirectionalLightMarker` from `src/scene/primitives.rs:41-70` verbatim.

- [ ] **Step 2: Wire the module + move `bevy_effect` dep; gate egui**

- `crates/bevy_editor_game/Cargo.toml`: add `bevy_effect = { path = "../bevy_effect" }` (for `PrimitiveShape`); change `bevy_egui.workspace = true` to `bevy_egui = { workspace = true, optional = true }`; add `[features] default = ["egui"]` / `egui = ["dep:bevy_egui"]`.
- `crates/bevy_editor_game/src/lib.rs`: `pub mod scene_types;` + `pub use scene_types::*;`; wrap every egui-dependent item (`InspectorWidgetFn` at lib.rs:280 and the `CustomEntityType` fields/aliases that reference `egui::Ui`, plus their uses) in `#[cfg(feature = "egui")]`. If `CustomEntityType` mixes egui and non-egui fields, gate the egui FIELDS (`draw_inspector` etc.) rather than the whole type so `register_custom_entity` still exists without egui.
- Editor side: in `src/scene/mod.rs` delete the `SceneEntity` definition and add `pub use bevy_editor_game::scene_types::SceneEntity;`; in `src/scene/primitives.rs` delete the five moved definitions and add `pub use bevy_editor_game::scene_types::{DirectionalLightMarker, GroupMarker, Locked, PrimitiveMarker, SceneLightMarker};`. Fix any `PrimitiveShape` import that pointed at the marker's old module.

- [ ] **Step 3: Run the editor suites**

```bash
cd ~/src/bevy_modal_editor && cargo test -p bevy_editor_game && cargo test --features obelisk 2>&1 | grep -E "test result:" | awk '{s+=$4; f+=$6} END {print "passed:", s, "failed:", f}'
cargo check -p bevy_editor_game --no-default-features
```

Expected: all pass (175+ before this task, plus the new type-path test); the no-default-features check compiles (proves the egui gate).

- [ ] **Step 4: Prove an existing saved scene still loads (type-path pin verification)**

```bash
cd ~/src/bevy_modal_editor && cargo test --features obelisk --test skill_preview 2>&1 | grep "test result"
grep -c "bevy_modal_editor::scene::primitives::PrimitiveMarker" crates/marble_demo/assets/levels/the_descent.scn.ron
```

Expected: tests pass; grep ≥ 1 (the path in real files matches the pin). Additionally run the marble demo's level loading headlessly if a test exists; otherwise the arena fixture test in Task 3 covers deserialization end-to-end.

- [ ] **Step 5: Commit + push upstream**

```bash
cd ~/src/bevy_modal_editor && git add -A && git commit -m "refactor(editor_game): shared scene marker types with pinned type_paths; egui behind a feature

Games loading editor-authored .scn.ron scenes need SceneEntity/PrimitiveMarker/
light markers in their TypeRegistry without pulling the whole editor (egui +
render). Move them to bevy_editor_game (the types-only shared-vocabulary crate),
pin each type_path to its original module so existing scenes keep loading, and
make bevy_egui optional (default-on) so headless consumers build without it." && git push lukethayer main
```

---

### Task 2: Editor shell — `ArenaSpawnPoint` registered as a custom entity

**Files:**
- Create: `crates/arena_sim/src/level/mod.rs` (module skeleton + `ArenaSpawnPoint` only, this task)
- Modify: `crates/arena_sim/src/lib.rs` (add `pub mod level;`)
- Modify: `crates/arena_editor/src/main.rs` (register the custom entity)
- Modify: `crates/arena_editor/Cargo.toml` if `arena_sim` isn't already a dep (it is — verify)
- Test: extend `crates/arena_editor/tests/smoke.rs`

**Interfaces:**
- Produces: `arena_sim::level::ArenaSpawnPoint { pub slot: u8 }` (`Component + Reflect + Serialize + Deserialize`, `#[reflect(Component, Default)]`) — the marker BOTH the editor saves and the game loader reads. Editor palette gains "Arena Spawn Point" under Game.

- [ ] **Step 1: Define the component**

`crates/arena_sim/src/level/mod.rs`:

```rust
//! Arena level vocabulary + (Task 3) the level catalog/loader/spawner. Levels are editor-authored
//! `.scn.ron` scenes (bevy `DynamicScene` RON); this module owns everything the GAME needs to
//! consume them and the marker types the EDITOR saves into them.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A player spawn point authored into a level scene. Match levels need slots 0 and 1 (duelist
/// spawns — faction by slot, as with the old SPAWN_MARKERS); the lobby uses any number of points
/// (players placed round-robin by sorted-id index % count). The entity's Transform provides the
/// spawn position AND facing (yaw). Registered in the editor by the arena_editor shell
/// (`register_custom_entity`) so it is palette-insertable and round-trips through scene saves;
/// registered in the game's level TypeRegistry so the loader reads it back.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Component, Default)]
pub struct ArenaSpawnPoint {
    pub slot: u8,
}
```

- [ ] **Step 2: Register it in the editor shell**

In `crates/arena_editor/src/main.rs`, after `.register_obelisk_content(root.clone())`:

```rust
        // Arena level vocabulary: spawn points are palette-insertable ("Game" category), draw a
        // gizmo, and round-trip through scene saves (register_custom_entity adds the type to the
        // scene-save allow-list). The GAME's level loader reads them back (arena_sim::level).
        .register_custom_entity::<arena_sim::level::ArenaSpawnPoint>(
            bevy_editor_game::CustomEntityType {
                name: "Arena Spawn Point".to_string(),
                spawn: |commands, position| {
                    commands
                        .spawn((
                            arena_sim::level::ArenaSpawnPoint::default(),
                            Transform::from_translation(position),
                        ))
                        .id()
                },
                draw_gizmo: Some(|gizmos, transform, _entity| {
                    gizmos.sphere(transform.translation(), 0.4, bevy::color::palettes::css::LIME);
                    gizmos.arrow(
                        transform.translation(),
                        transform.translation() + transform.forward() * 1.0,
                        bevy::color::palettes::css::LIME,
                    );
                }),
                ..Default::default()
            },
        )
```

NOTE: `CustomEntityType`'s exact field set must be read from `crates/bevy_editor_game/src/lib.rs:384-404` + the marble registration (`crates/marble_demo/src/main.rs:59-105`) at implementation time and this call adapted to the REAL fields (the marble demo registration is the template; `..Default::default()` covers inspector hooks). Any egui-typed fields are behind the Task-1 feature (the shell builds with default features, so they exist here).

- [ ] **Step 3: Round-trip test in the shell**

Append to `crates/arena_editor/tests/smoke.rs`:

```rust
/// ArenaSpawnPoint must survive an editor scene save (it's in the SceneComponentRegistry via
/// register_custom_entity) — the GAME's level loader depends on reading it back.
#[test]
fn arena_spawn_point_round_trips_through_scene_save() {
    use arena_sim::level::ArenaSpawnPoint;
    use bevy_modal_editor::scene::{build_editor_scene, SceneEntity};

    let mut app = arena_editor::build_editor_app();
    app.finish();
    app.cleanup();
    app.update();

    let e = app
        .world_mut()
        .spawn((
            SceneEntity,
            bevy::prelude::Name::new("spawn_0"),
            bevy::prelude::Transform::from_xyz(1.0, 0.6, 2.0),
            ArenaSpawnPoint { slot: 1 },
        ))
        .id();
    let scene = build_editor_scene(app.world_mut(), vec![e]);
    let registry = app.world().resource::<bevy::ecs::reflect::AppTypeRegistry>().clone();
    let ron = scene.serialize(&registry.read()).expect("serialize");
    assert!(
        ron.contains("ArenaSpawnPoint"),
        "spawn point must be in the saved scene; got:\n{ron}"
    );
    assert!(ron.contains("slot: 1"));
}
```

(`build_editor_scene`'s exact signature is `src/scene/mod.rs:51` — adapt the call if it takes `&mut World` + ids slice.)

- [ ] **Step 4: Build, test, commit (arena repo), and note NO push needed yet**

```bash
cd /home/luke/src/obelisk-arena/crates/arena_editor && cargo metadata --format-version 1 >/dev/null && cargo update -p "https://github.com/LukeThayer/bevy_modal_editor#bevy_modal_editor" && cargo test 2>&1 | grep "test result"
cd /home/luke/src/obelisk-arena && git add crates/arena_sim/src/level/mod.rs crates/arena_sim/src/lib.rs crates/arena_editor/src/main.rs crates/arena_editor/Cargo.lock crates/arena_editor/tests/smoke.rs && git commit -m "feat(editor): ArenaSpawnPoint — palette-insertable, save-round-tripping level spawn marker"
```

Expected: smoke tests pass (boot + skills + the new round-trip).

---

### Task 3: `arena_sim::level` — catalog, loader, spawner (+ real-fixture gate)

**Files:**
- Modify: `crates/arena_sim/src/level/mod.rs` (grow), `crates/arena_sim/Cargo.toml` (add deps), root `Cargo.toml` `[workspace.dependencies]` (add `bevy_editor_game` no-default-features + `ron`; `bevy_effect` already present)
- Create: `crates/arena_sim/src/level/fixtures/minimal.scn.ron` (test fixture, editor-shaped)
- Test: unit tests in `level/mod.rs`

**Interfaces:**
- Produces (consumed by Tasks 4-6):
  - `LevelCatalog { pub levels: Vec<LevelInfo> }` resource; `LevelInfo { pub id: String, pub path: PathBuf }`; `LevelCatalog::scan() -> Self` (scans `["assets/scenes", "crates/arena_editor/assets/scenes"]`); `fn selectable(&self) -> impl Iterator<Item = &LevelInfo>` (excludes `lobby`); `fn get(&self, id: &str) -> Option<&LevelInfo>`.
  - `load_level_scene(path: &Path) -> Result<LevelScene, LevelError>`; `LevelScene { pub statics: Vec<StaticDesc>, pub lights: Vec<LightDesc>, pub spawns: Vec<SpawnDesc> }`; `StaticDesc { pub name: String, pub shape: PrimitiveShape, pub translation: Vec3, pub rotation: Quat, pub scale: Vec3, pub material: MaterialDesc }`; `MaterialDesc = bevy_editor_game::MaterialDefinition` (Library refs resolved through the `.meta` at load); `LightDesc { Point { pos: Vec3, color: Color, intensity: f32, range: f32, shadows: bool }, Directional { rotation: Quat, color: Color, illuminance: f32, shadows: bool } }`; `SpawnDesc { pub slot: u8, pub position: Vec3, pub yaw: f32 }`.
  - `LevelEntity` marker component (despawn key).
  - `spawn_level_physics(commands: &mut Commands, scene: &LevelScene)` — every peer: statics as `LevelEntity + Name + RigidBody::Static + Collider (scale-baked) + Position + Rotation`.
  - `spawn_level_visuals(commands, scene, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>)` — windowed client add-on: `Mesh3d`/`MeshMaterial3d`/`Transform` (with scale) + lights.
  - `LevelError::UnsupportedTypes(Vec<String>) | Io(...) | Parse(String) | NoSpawns`.

- [ ] **Step 1: Deps**

Root `Cargo.toml` `[workspace.dependencies]`:

```toml
bevy_editor_game = { git = "https://github.com/LukeThayer/bevy_modal_editor", branch = "main", default-features = false }
```

(`ron` and `bevy_effect` already exist in the workspace deps — verify; add `ron` to arena_sim's `[dependencies]` along with `bevy_editor_game.workspace = true`, `bevy_effect.workspace = true`.)

- [ ] **Step 2: Write the fixture (editor-shaped RON)**

`crates/arena_sim/src/level/fixtures/minimal.scn.ron` — hand-authored to the EXACT shape the editor writes (mirror `~/src/bevy_modal_editor/crates/marble_demo/assets/levels/the_descent.scn.ron`): a `(resources: {}, entities: { ... })` DynamicScene with two entities — a floor (`RigidBody`: `Static`, `Name`: "Floor", `PrimitiveMarker(shape: Cube)`, inline `MaterialRef`, `Transform` translation (0, −0.5, 0) scale (40, 1, 40), `SceneEntity`) and one spawn point (`Name`: "spawn_0", `Transform` translation (−4, 0.59, 0) with a 90° yaw rotation, `ArenaSpawnPoint(slot: 0)`, `SceneEntity`). Copy field spellings/enum forms from the real marble file verbatim (e.g. `Srgba((red: …))`, full type-path keys like `"bevy_editor_game::MaterialRef"`, `"arena_sim::level::ArenaSpawnPoint"`).

- [ ] **Step 3: Failing tests**

In `level/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/level/fixtures/minimal.scn.ron");

    #[test]
    fn loads_statics_spawns_and_scale_from_editor_shaped_ron() {
        let scene = load_level_scene(Path::new(FIXTURE)).expect("fixture loads");
        assert_eq!(scene.statics.len(), 1);
        let floor = &scene.statics[0];
        assert_eq!(floor.name, "Floor");
        assert_eq!(floor.scale, Vec3::new(40.0, 1.0, 40.0));
        assert!((floor.translation.y - -0.5).abs() < 1e-6);
        assert_eq!(scene.spawns.len(), 1);
        assert_eq!(scene.spawns[0].slot, 0);
        assert!((scene.spawns[0].position.x - -4.0).abs() < 1e-6);
    }

    #[test]
    fn unknown_component_types_fail_loud_with_names() {
        let dir = std::env::temp_dir().join(format!("arena_level_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.scn.ron");
        std::fs::write(&path, r#"(
  resources: {},
  entities: {
    1: ( components: {
      "bevy_modal_editor::scene::SceneEntity": (),
      "bevy_modal_editor::prefabs::PrefabInstance": (prefab_name: "x", instance_id: 1),
    }),
  },
)"#).unwrap();
        let err = load_level_scene(&path).unwrap_err();
        match err {
            LevelError::UnsupportedTypes(types) => {
                assert!(types.iter().any(|t| t.contains("PrefabInstance")), "{types:?}");
            }
            other => panic!("expected UnsupportedTypes, got {other:?}"),
        }
    }

    #[test]
    fn catalog_reserves_lobby_and_keys_by_stem() {
        let dir = std::env::temp_dir().join(format!("arena_catalog_test_{}", std::process::id()));
        let scenes = dir.join("assets/scenes");
        std::fs::create_dir_all(&scenes).unwrap();
        std::fs::write(scenes.join("lobby.scn.ron"), "(resources: {}, entities: {})").unwrap();
        std::fs::write(scenes.join("arena_flat.scn.ron"), "(resources: {}, entities: {})").unwrap();
        let catalog = LevelCatalog::scan_roots(&[dir.join("assets/scenes")]);
        assert!(catalog.get("lobby").is_some());
        assert!(catalog.get("arena_flat").is_some());
        let selectable: Vec<_> = catalog.selectable().map(|l| l.id.as_str()).collect();
        assert_eq!(selectable, vec!["arena_flat"]);
    }
}
```

Run: `cargo test -p arena_sim level` → expected: compile FAIL (types/fns missing).

- [ ] **Step 4: Implement**

Core shape (complete except mechanical match arms):

```rust
use std::path::{Path, PathBuf};

use avian3d::prelude::{Collider, Position, RigidBody, Rotation};
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::scene::serde::SceneDeserializer;
use bevy::scene::DynamicScene;
use bevy_editor_game::scene_types::{
    DirectionalLightMarker, GroupMarker, Locked, PrimitiveMarker, SceneEntity, SceneLightMarker,
};
use bevy_editor_game::{MaterialDefinition, MaterialRef};
use bevy_effect::primitive::PrimitiveShape;
use serde::de::DeserializeSeed;

/// Marker on every entity spawned from a level file — the despawn key for level switches.
#[derive(Component)]
pub struct LevelEntity;

#[derive(Debug)]
pub enum LevelError {
    Io(std::io::Error),
    Parse(String),
    UnsupportedTypes(Vec<String>),
    NoSpawns,
}

/// The component type-paths a level may contain. Anything else fails the preflight with the
/// offending paths named (spec §4.1: primitives + groups + lights + arena markers only in v1).
const SUPPORTED_TYPES: &[&str] = &[
    "bevy_modal_editor::scene::SceneEntity",
    "bevy_modal_editor::scene::primitives::PrimitiveMarker",
    "bevy_modal_editor::scene::primitives::GroupMarker",
    "bevy_modal_editor::scene::primitives::Locked",
    "bevy_modal_editor::scene::primitives::SceneLightMarker",
    "bevy_modal_editor::scene::primitives::DirectionalLightMarker",
    "bevy_editor_game::MaterialRef",
    "arena_sim::level::ArenaSpawnPoint",
    "bevy_ecs::name::Name",
    "bevy_transform::components::transform::Transform",
    "avian3d::dynamics::rigid_body::RigidBody",
    "bevy_ecs::hierarchy::ChildOf",
    "bevy_ecs::hierarchy::Children",
];

/// Preflight: walk the RON generically and collect component type-path keys not in
/// [`SUPPORTED_TYPES`] — a friendly authoring error instead of SceneDeserializer's hard error.
fn preflight_unknown_types(ron_str: &str) -> Result<(), LevelError> {
    let value: ron::Value = ron::from_str(ron_str).map_err(|e| LevelError::Parse(e.to_string()))?;
    // entities: Map< id, ( components: Map<String, _> ) > — walk defensively.
    let mut unknown = Vec::new();
    if let ron::Value::Map(top) = &value {
        if let Some(ron::Value::Map(entities)) = top.get(&ron::Value::String("entities".into())) {
            for (_, entity) in entities.iter() {
                if let ron::Value::Map(entity) = entity {
                    if let Some(ron::Value::Map(components)) =
                        entity.get(&ron::Value::String("components".into()))
                    {
                        for (key, _) in components.iter() {
                            if let ron::Value::String(path) = key {
                                if !SUPPORTED_TYPES.contains(&path.as_str())
                                    && !unknown.contains(path)
                                {
                                    unknown.push(path.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if unknown.is_empty() { Ok(()) } else { Err(LevelError::UnsupportedTypes(unknown)) }
}

/// Build the TypeRegistry for level deserialization: exactly the supported types.
fn level_type_registry() -> AppTypeRegistry {
    let registry = AppTypeRegistry::default();
    {
        let mut r = registry.write();
        r.register::<SceneEntity>();
        r.register::<PrimitiveMarker>();
        r.register::<GroupMarker>();
        r.register::<Locked>();
        r.register::<SceneLightMarker>();
        r.register::<DirectionalLightMarker>();
        r.register::<MaterialRef>();
        r.register::<super::level::ArenaSpawnPoint>();
        r.register::<Name>();
        r.register::<Transform>();
        r.register::<RigidBody>();
        r.register::<bevy::ecs::hierarchy::ChildOf>();
        r.register::<bevy::ecs::hierarchy::Children>();
    }
    registry
}

pub fn load_level_scene(path: &Path) -> Result<LevelScene, LevelError> {
    let content = std::fs::read_to_string(path).map_err(LevelError::Io)?;
    preflight_unknown_types(&content)?;
    let registry = level_type_registry();
    let registry_read = registry.read();
    let mut de = ron::de::Deserializer::from_str(&content)
        .map_err(|e| LevelError::Parse(e.to_string()))?;
    let scene: DynamicScene = SceneDeserializer { type_registry: &registry_read }
        .deserialize(&mut de)
        .map_err(|e| LevelError::Parse(format!("{e:?}")))?;
    // Extract into plain data. Instantiate into a scratch World (write_to_world needs the same
    // registry as a resource) and walk hierarchies for world-space transforms.
    ...
    // .meta sidecar (optional): tolerant serde parse for Library material resolution.
    #[derive(serde::Deserialize)]
    struct MetaSidecar {
        #[serde(default)]
        material_library: bevy_editor_game::MaterialLibrary,
    }
    ...
}
```

Extraction implementation detail (write in full at implementation): spawn a scratch `World`, insert the registry as `AppTypeRegistry`, `scene.write_to_world(&mut world, &mut EntityHashMap::default())`, then:
- statics: query `(&PrimitiveMarker, &Transform, Option<&Name>, Option<&MaterialRef>, Option<&ChildOf>)`; compute WORLD transform by walking `ChildOf` chains (levels use flat or one-deep group nesting; multiply parent `Transform`s); material = Inline verbatim, Library resolved via the sidecar (missing name → `MaterialDefinition::standard(Color::srgb(0.5, 0.5, 0.55))` + warn).
- spawns: query `(&ArenaSpawnPoint, &Transform)` → world position + yaw from the transform's forward.
- lights: the two marker queries.
`spawn_level_physics`: per static, `let mut collider = desc.shape.create_collider(); collider.set_scale(desc.scale, 8);` + `Position(desc.translation)` + `Rotation(desc.rotation)` + `RigidBody::Static` + `Name` + `LevelEntity`. `spawn_level_visuals`: `Mesh3d(meshes.add(desc.shape.create_mesh()))` + `MeshMaterial3d(materials.add(desc.material.base.to_standard_material()))` + `Transform { translation, rotation, scale }` + `LevelEntity` on the SAME entity as physics — combine: visuals fn takes the already-spawned physics entities? Simpler: ONE spawner `spawn_level(commands, scene, Option<(&mut Assets<Mesh>, &mut Assets<StandardMaterial>)>)` that inserts the visual bundle when the assets are provided. Produce THAT signature and use it in Tasks 4-6:

```rust
pub fn spawn_level(
    commands: &mut Commands,
    scene: &LevelScene,
    visuals: Option<(&mut Assets<Mesh>, &mut Assets<StandardMaterial>)>,
) -> Vec<Entity>
```

- [ ] **Step 5: Green + commit**

```bash
cargo test -p arena_sim level 2>&1 | grep "test result"
git add -A crates/arena_sim Cargo.toml Cargo.lock && git commit -m "feat(sim): level catalog + editor-scene loader + per-peer spawner"
```

---

### Task 4: Shipped levels (`lobby`, `arena_flat`) + geometry-equivalence test

**Files:**
- Create: `assets/scenes/arena_flat.scn.ron`, `assets/scenes/lobby.scn.ron`
- Test: `crates/arena_sim/src/level/mod.rs` (append)

**Interfaces:**
- Produces: two loadable levels. `arena_flat`: floor cube scaled (40, 1, 40) at (0, −0.5, 0) (top at y=0) + `ArenaSpawnPoint{slot:0}` at (−4, 0.59, 0) facing +X and `{slot:1}` at (4, 0.59, 0) facing −X. `lobby`: 30×30 floor (top at y=0), four perimeter walls (scaled cubes), two pillars, one directional light marker + two point lights, four spawn points slots 0-3 near the center.

- [ ] **Step 1: Author both files** in the exact editor RON shape (as the Task 3 fixture; distinct entity ids; every entity carries `SceneEntity` + `Name`; statics carry `RigidBody: Static` + `PrimitiveMarker` + inline `MaterialRef` colors — floor grey, walls darker, pillars accent).

- [ ] **Step 2: Equivalence test** (append to level tests):

```rust
    #[test]
    fn arena_flat_matches_legacy_geometry() {
        let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/scenes/arena_flat.scn.ron"));
        let scene = load_level_scene(path).expect("arena_flat loads");
        let floor = scene.statics.iter().find(|s| s.name == "Floor").expect("floor");
        // Top face at world 0 (legacy spawn_arena_floor contract).
        assert!((floor.translation.y + floor.scale.y / 2.0).abs() < 1e-6);
        assert_eq!(floor.scale, Vec3::new(40.0, 1.0, 40.0));
        let mut slots: Vec<_> = scene.spawns.iter().map(|s| (s.slot, s.position)).collect();
        slots.sort_by_key(|(s, _)| *s);
        assert_eq!(slots[0].1, Vec3::new(-4.0, crate::tuning::GROUND_Y, 0.0));
        assert_eq!(slots[1].1, Vec3::new(4.0, crate::tuning::GROUND_Y, 0.0));
    }

    #[test]
    fn lobby_loads_with_at_least_one_spawn() {
        let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/scenes/lobby.scn.ron"));
        let scene = load_level_scene(path).expect("lobby loads");
        assert!(!scene.spawns.is_empty());
        assert!(!scene.statics.is_empty());
    }
```

- [ ] **Step 3: Green + commit** (`cargo test -p arena_sim level`; `git add assets/scenes crates/arena_sim && git commit -m "content(levels): shipped lobby + arena_flat (legacy-geometry equivalent)"`).

---

### Task 5: Server flow — lobby phase, host election, level switching, wire v2

**Files:**
- Modify: `crates/arena_game/src/net/protocol.rs` (RoundStateMessage v2 + StartMatchMessage), `crates/arena_game/src/net/mod.rs` (`PROTOCOL_ID = 3`, `MATCH_OVER_SECS = 6.0`)
- Modify: `crates/arena_game/src/server/rounds.rs` (RoundPhase v2 + FSM + spawns-from-level), `crates/arena_game/src/server/spawn.rs` (spawn at current level slots; host order), `crates/arena_game/src/server/mod.rs` (wiring + startup lobby load + `LevelCatalog`/`CurrentLevel`/`LevelSpawns`/`HostState` resources + `drain_start_match`), delete the `spawn_floor` startup
- Test: unit tests in `rounds.rs` + `spawn.rs`

**Interfaces:**
- Produces:
  - `RoundStateMessage` v2: existing fields + `pub host: u64` + `pub level: String` (wire tag 0 now means Lobby; tags 1-4 unchanged).
  - `StartMatchMessage { pub level: String }` C→S on `RequestChannel`.
  - Server resources: `CurrentLevel { pub id: String }`, `LevelSpawns { pub slots: Vec<SpawnDesc> }` (from `arena_sim::level`), `HostState { pub order: Vec<u64>, pub host: Option<u64> }` with `fn on_connect(&mut self, id: u64)` / `fn on_disconnect(&mut self, id: u64)` (host = first of `order` still present).
  - `RoundPhase::Lobby` initial; `MatchOver { winner, remaining }` timed → lobby reload.
  - Trace kinds: `level_loaded { id, statics, spawns }`, `host_elected { client_id }`, `match_started { level }`.
- Consumes: `LevelCatalog::scan`, `load_level_scene`, `spawn_level`, `LevelEntity`, `SpawnDesc` (Task 3); `arena_flat`/`lobby` (Task 4).

- [ ] **Step 1: Failing unit tests** — host election order + re-election; Lobby→Countdown requires an explicit start (no auto-start at 2 players); MatchOver timer returns to Lobby; round-robin lobby placement:

```rust
    #[test]
    fn host_is_first_joiner_and_reelects_in_join_order() {
        let mut h = HostState::default();
        h.on_connect(7);
        h.on_connect(3);
        assert_eq!(h.host, Some(7));
        h.on_disconnect(7);
        assert_eq!(h.host, Some(3));
        h.on_connect(9);
        assert_eq!(h.host, Some(3));
    }

    #[test]
    fn lobby_does_not_autostart_with_two_players() {
        // run_round_machine's Lobby arm only transitions on RoundState.start_requested —
        // exercised as a pure helper: lobby_should_start(players, start_requested).
        assert!(!lobby_should_start(2, false));
        assert!(!lobby_should_start(1, true));
        assert!(lobby_should_start(2, true));
    }

    #[test]
    fn lobby_slot_assignment_is_round_robin() {
        // 4 lobby spawn points, 2 players by sorted-id index.
        assert_eq!(lobby_spawn_index(0, 4), 0);
        assert_eq!(lobby_spawn_index(1, 4), 1);
        assert_eq!(lobby_spawn_index(5, 4), 1);
    }
```

- [ ] **Step 2: Wire changes** (`protocol.rs`): add fields to `RoundStateMessage` (`host: u64`, `level: String`); register `StartMatchMessage { level: String }` `ClientToServer`; update the phase-tag doc (0 = Lobby). `net/mod.rs`: `PROTOCOL_ID = 3`, `pub const MATCH_OVER_SECS: f32 = 6.0;`.

- [ ] **Step 3: Server implementation** (`rounds.rs`, `spawn.rs`, `mod.rs`):
  - `RoundPhase::WaitingForPlayers` → renamed `Lobby` (wire tag 0). `RoundState` gains `start_requested: Option<String>` (set by `drain_start_match`, consumed by the FSM).
  - Startup: `LevelCatalog::scan()` resource; load + `spawn_level(physics-only)` the LOBBY; insert `CurrentLevel { id: "lobby" }` + `LevelSpawns`; REMOVE the `spawn_floor` Startup system.
  - `spawn_player_on_connect`: position from `LevelSpawns` — match phase Lobby: `lobby_spawn_index(sorted_index, slots.len())`; also `HostState::on_connect` + `host_elected` trace on change; `cleanup_player_on_disconnect`: `HostState::on_disconnect`; phase falls back to `Lobby` (not WaitingForPlayers) below 2 players mid-match AND reloads the lobby level.
  - `drain_start_match` (Update, the ONLY `MessageReceiver<StartMatchMessage>` drain): sender RemoteId → client id; accept iff `Some(id) == host.host` ∧ phase == Lobby ∧ `client_map.len() >= 2` ∧ catalog has the level; on accept load the level scene (validate ≥2 match slots 0/1 → else reject with a warn), despawn `LevelEntity`s, `spawn_level` physics, replace `LevelSpawns`/`CurrentLevel`, reset scores to 0, set `start_requested`, trace `match_started`.
  - FSM `Lobby` arm: if `start_requested.take()` and ≥2 players → `Countdown(COUNTDOWN_SECS)`. `MatchOver` arm: tick `remaining`; at 0 → reload lobby (despawn + spawn + resources), teleport everyone round-robin, phase = `Lobby`, scores cleared.
  - `reset_for_new_round`: teleport by MATCH slot (`slot 0/1` from `LevelSpawns`, sorted-id order as today) and apply spawn yaw to `Rotation`.
  - `broadcast_round_state`: include `host` (0 if none) + `level`.

- [ ] **Step 4: Green + commit** (unit tests + `cargo build`): net-test is EXPECTED RED until Task 6 wires the client autostart — do NOT run the gate yet; commit `feat(server): lobby phase, host election, level switching (wire v3)`.

---

### Task 6: Clients — level loading, lobby UX, G panel, autostart hook

**Files:**
- Modify: `crates/arena_game/src/client/app_headless.rs` (level physics on state change; `ARENA_AUTOSTART_LEVEL`; remove floor spawn), `crates/arena_game/src/client/app_windowed.rs` (register level-visual systems; remove plane+floor from `setup_scene` in `client/scene.rs`), `crates/arena_game/src/client/hud.rs` (lobby/matchover banners + host detection), `crates/arena_game/src/client/net.rs` or new `crates/arena_game/src/client/level.rs` (client level sync + G panel)
- Test: net-test gate (this task turns it green again)

**Interfaces:**
- Produces: `client/level.rs`: `ClientLevelPlugin` — holds `ClientLevel { current: Option<String> }`; a system reading the round-state (via the existing single-drain fan-outs: the headless tracer and the windowed HUD forward `RoundStateMessage` into a new local Bevy message `RoundStateChanged(RoundStateMessage)` — ONE forwarding site per app, keeping the single-drain rule) that on `level` change despawns `LevelEntity`s and `spawn_level`s (visuals iff windowed — plugin flag); `LevelSelectOpen` resource + G-toggle panel (host-only, Lobby only) sending `StartMatchMessage`; headless: `autostart_level` system.
- The K-customizer pattern (`client/customization.rs`) is the template for the panel; the panel lists `LevelCatalog::scan().selectable()` done once at startup.

- [ ] **Step 1: RoundState fan-out** — in `hud.rs` (windowed drain) and `app_headless.rs` (`trace_replicated_round_state`), after their existing handling, `out.write(RoundStateChanged(msg.clone()))` (message registered by `ClientLevelPlugin`). The drains stay the single `MessageReceiver` readers.
- [ ] **Step 2: Level sync system** — on `RoundStateChanged` with `msg.level != client_level.current`: despawn `LevelEntity`s, catalog lookup, `load_level_scene`, `spawn_level(..., visuals_if_windowed)`, set current, trace `level_loaded`. Remove the hardcoded plane + `spawn_arena_floor` from `client/scene.rs::setup_scene` and the headless Startup closure.
- [ ] **Step 3: Banners** — `hud.rs` phase 0 renders "LOBBY — press G to choose an arena" when `msg.host == my client id` (client id: from `ConnectTo` resource) else "LOBBY — waiting for host"; MatchOver shows "returning to lobby…" with the countdown.
- [ ] **Step 4: G panel** — `LevelSelectOpen { open: bool, highlighted: usize }`; toggle on G (`just_pressed`, host + phase Lobby only); Up/Down/number keys move highlight; Enter sends `StartMatchMessage { level }` via `MessageSender<StartMatchMessage>` on `RequestChannel` and closes; movement input bridge gates on it like `CustomizationOpen`. Render with the same bevy_ui style as the customizer panel.
- [ ] **Step 5: Headless autostart** —

```rust
/// [H] ARENA_AUTOSTART_LEVEL=<id>: the HOST observer starts a match whenever the lobby is ready
/// (once per lobby visit) — the harness's stand-in for pressing G. Non-hosts no-op.
fn autostart_level(
    state: Res<ClientLevel>, // carries last RoundStateChanged snapshot (phase, host)
    me: Res<crate::net::client::ConnectTo>,
    remotes: Query<(), RemotePlayerFilter>,
    sender: Option<Single<&mut MessageSender<StartMatchMessage>>>,
    mut sent_this_lobby: Local<bool>,
) { ... }
```

Reset `sent_this_lobby` when phase leaves Lobby. Register under `if let Ok(level) = std::env::var("ARENA_AUTOSTART_LEVEL")`.
- [ ] **Step 6: Harness env** — `run_session.sh` observer-0 gains `ARENA_AUTOSTART_LEVEL=arena_flat`. Then the full gate:

```bash
cargo build -p arena_game --bin arena-server --bin arena-client --bin arena-observer
ARENA_SKIP_BUILD=1 bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-net-levels; true
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-net-levels
grep -c '"kind":"level_loaded"' /tmp/arena-net-levels/server.jsonl
```

Expected: `PASS` (same damage 20.0) + level_loaded ≥ 2 (lobby + arena_flat). Commit `feat(client): level sync, lobby UX, host level-select (G), autostart hook`.

---

### Task 7: Movement + projectiles on real geometry

**Files:**
- Modify: `crates/arena_sim/src/shared_controller.rs` (raycast ground check), `crates/arena_sim/src/obelisk.rs` (`report_ground_hits` → `report_world_hits` + kill plane)
- Modify: callers of `apply_arena_movement` (`crates/arena_game/src/server/controller.rs`, `crates/arena_game/src/client/net.rs`) — signature gains the spatial query + self-exclusion
- Test: unit tests for the pure helpers; net-test gate

**Interfaces:**
- Produces: `pub fn grounded_by_ray(spatial: &SpatialQuery, origin: Vec3, exclude: &[Entity]) -> bool` (ray straight down from the capsule bottom, `max_distance = 0.15`, filter excludes `exclude`); `apply_arena_movement(mass, dt, input, forces, grounded: bool)` (grounded computed by the caller — keeps the controller pure and rollback-friendly); `report_world_hits` casting each projectile's movement segment (`prev_pos` tracked via a `LastHitboxPos(Vec3)` component inserted on window spawn... inserted by the system itself on first sight) against non-sensor colliders excluding combatant bodies + hurtboxes, triggering `HitboxWorldHit` at the impact; kill plane `y < -10.0`.
- Controller callers pass `grounded` from `grounded_by_ray` (server + predicted client identically); exclusion = the body + its hurtbox child (children query).

- [ ] Steps: failing unit tests for both helpers (grounded true on a floor fixture via a scratch avian world is heavy — test the FILTER/threshold math pure parts + rely on the gate for integration); implement; run `cargo test -p arena_sim -p arena_game`; full net-test gate (flat level ⇒ unchanged trajectories) + conditioned gate; commit `feat(sim): raycast ground check + world-geometry projectile hits`.

---

### Task 8: Docs + final gates

**Files:** `crates/arena_game/CLAUDE.md`, spec status, memory.

- [ ] CLAUDE.md: new §Levels (format, dual scan roots, supported content, ArenaSpawnPoint, LevelEntity switching); flow section rewrite (Lobby/host/G/StartMatch, MatchOver→Lobby); invariants: level statics bake scale into colliders; PROTOCOL_ID 3; net-test env `ARENA_AUTOSTART_LEVEL`; grounded check is now a raycast (flat-floor invariant 12 retired).
- [ ] Full suite: `cargo test -p arena_game -p arena_sim`, clean + conditioned gates, `crates/arena_editor` smoke suite.
- [ ] Spec `**Status:**` → implemented; commit docs.

## Execution deviation notes

(append during execution)

## Self-review notes (already applied)

- Spec §4.1→Tasks 2-4, §4.2→Task 1, §4.3→Task 3, §4.4→Task 5, §4.5→Task 7, §4.6→Task 6, §4.7→Tasks 6/8.
- Known judgment points called out in-place: `CustomEntityType` exact fields (Task 2 reads the marble template), `LevelScene` extraction internals (Task 3 Step 4 sketch + full signature contract), heavy avian-world unit tests avoided in Task 7 (gate covers integration).
- Type consistency: `SpawnDesc`/`LevelSpawns`/`spawn_level`/`LevelEntity`/`RoundStateChanged`/`StartMatchMessage` names used identically across Tasks 3-7.
