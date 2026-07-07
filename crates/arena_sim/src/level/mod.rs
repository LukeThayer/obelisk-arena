//! Arena level vocabulary + (in later tasks) the level catalog/loader/spawner. Levels are
//! editor-authored `.scn.ron` scenes (bevy `DynamicScene` RON); this module owns everything the
//! GAME needs to consume them and the marker types the EDITOR saves into them.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A player spawn point authored into a level scene. Match levels need slots 0 and 1 (duelist
/// spawns — faction by slot, exactly like the old `SPAWN_MARKERS`); the lobby uses any number of
/// points (players placed round-robin by sorted-id index % count). The entity's Transform
/// provides the spawn position AND facing (yaw from its forward vector). Registered in the editor
/// by the arena_editor shell (`register_custom_entity`) so it is palette-insertable and
/// round-trips through scene saves; registered in the game's level `TypeRegistry` so the loader
/// reads it back.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Component, Default)]
pub struct ArenaSpawnPoint {
    pub slot: u8,
}

use std::path::{Path, PathBuf};

use avian3d::prelude::{Collider, Position, RigidBody, Rotation};
use bevy::ecs::entity::EntityHashMap;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::scene::serde::SceneDeserializer;
use bevy::scene::DynamicScene;
use bevy_editor_game::scene_types::{
    DirectionalLightMarker, GroupMarker, Locked, PrimitiveMarker, SceneEntity, SceneLightMarker,
};
use bevy_editor_game::{MaterialDefinition, MaterialLibrary, MaterialRef};
use bevy_effect::primitive::PrimitiveShape;
use serde::de::DeserializeSeed;

/// Marker on every entity spawned from a level file — the despawn key for level switches.
#[derive(Component)]
pub struct LevelEntity;

#[derive(Debug)]
pub enum LevelError {
    Io(std::io::Error),
    Parse(String),
    /// The scene names component types outside the v1 level contract (spec §4.1). Carries the
    /// offending type paths so the authoring error is actionable.
    UnsupportedTypes(Vec<String>),
}

impl std::fmt::Display for LevelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LevelError::Io(e) => write!(f, "level io error: {e}"),
            LevelError::Parse(e) => write!(f, "level parse error: {e}"),
            LevelError::UnsupportedTypes(t) => write!(
                f,
                "level uses unsupported component types (v1 levels: primitives + groups + \
                 lights + ArenaSpawnPoint only): {t:?}"
            ),
        }
    }
}

/// The component type-paths a level may contain. Anything else fails the preflight with the
/// offending paths named — a friendly authoring error instead of SceneDeserializer's hard error.
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

/// One static (collidable, optionally visible) level object, in WORLD space.
#[derive(Debug, Clone)]
pub struct StaticDesc {
    pub name: String,
    pub shape: PrimitiveShape,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    pub material: MaterialDefinition,
}

/// A light authored into the level (client-side only; headless peers skip lights).
#[derive(Debug, Clone)]
pub enum LightDesc {
    Point {
        position: Vec3,
        color: Color,
        intensity: f32,
        range: f32,
        shadows: bool,
    },
    Directional {
        rotation: Quat,
        color: Color,
        illuminance: f32,
        shadows: bool,
    },
}

/// A player spawn slot, in WORLD space. `yaw` is the facing (radians, arena camera convention:
/// `Quat(Y, yaw) * -Z` == the marker's horizontal forward).
#[derive(Debug, Clone, Copy)]
pub struct SpawnDesc {
    pub slot: u8,
    pub position: Vec3,
    pub yaw: f32,
}

/// The plain-data contents of a loaded level scene — everything a peer needs to spawn it.
#[derive(Debug, Clone, Default)]
pub struct LevelScene {
    pub statics: Vec<StaticDesc>,
    pub lights: Vec<LightDesc>,
    pub spawns: Vec<SpawnDesc>,
}

/// Preflight: walk the RON generically and collect component type-path keys not in
/// [`SUPPORTED_TYPES`].
fn preflight_unknown_types(ron_str: &str) -> Result<(), LevelError> {
    let value: ron::Value =
        ron::from_str(ron_str).map_err(|e| LevelError::Parse(e.to_string()))?;
    let mut unknown: Vec<String> = Vec::new();
    let ron::Value::Map(top) = &value else {
        return Err(LevelError::Parse("scene root is not a map".into()));
    };
    for (key, val) in top.iter() {
        if !matches!(key, ron::Value::String(s) if s == "entities") {
            continue;
        }
        let ron::Value::Map(entities) = val else { continue };
        for (_, entity) in entities.iter() {
            let ron::Value::Map(entity) = entity else { continue };
            for (ekey, eval) in entity.iter() {
                if !matches!(ekey, ron::Value::String(s) if s == "components") {
                    continue;
                }
                let ron::Value::Map(components) = eval else { continue };
                for (ckey, _) in components.iter() {
                    if let ron::Value::String(path) = ckey {
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
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(LevelError::UnsupportedTypes(unknown))
    }
}

/// The TypeRegistry for level deserialization: exactly the supported types.
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
        r.register::<ArenaSpawnPoint>();
        r.register::<Name>();
        r.register::<Transform>();
        r.register::<RigidBody>();
        r.register::<ChildOf>();
        r.register::<bevy::ecs::hierarchy::Children>();
    }
    registry
}

/// The tolerant `.meta` sidecar parse — only the material library matters to the game (the
/// sidecar also carries editor camera state, which serde skips as unknown fields).
#[derive(serde::Deserialize, Default)]
struct MetaSidecar {
    #[serde(default)]
    material_library: MaterialLibrary,
}

/// World-space transform: walk the `ChildOf` chain multiplying parent transforms (levels use flat
/// or group-nested hierarchies).
fn world_transform(world: &bevy::ecs::world::World, entity: Entity) -> Transform {
    let mut chain: Vec<Transform> = Vec::new();
    let mut cur = Some(entity);
    while let Some(e) = cur {
        if let Some(t) = world.get::<Transform>(e) {
            chain.push(*t);
        }
        cur = world.get::<ChildOf>(e).map(|c| c.0);
    }
    let mut out = Transform::IDENTITY;
    for t in chain.iter().rev() {
        out = out * *t;
    }
    out
}

/// Yaw (radians) whose `Quat(Y, yaw) * -Z` equals the given horizontal forward.
fn yaw_from_forward(forward: Vec3) -> f32 {
    (-forward.x).atan2(-forward.z)
}

/// Load + extract a level scene from an editor-authored `.scn.ron`. Pure data out — spawning is
/// [`spawn_level`]'s job so each peer picks its own mode.
pub fn load_level_scene(path: &Path) -> Result<LevelScene, LevelError> {
    let content = std::fs::read_to_string(path).map_err(LevelError::Io)?;
    preflight_unknown_types(&content)?;

    let registry = level_type_registry();
    let scene: DynamicScene = {
        let registry_read = registry.read();
        let mut de = ron::de::Deserializer::from_str(&content)
            .map_err(|e| LevelError::Parse(e.to_string()))?;
        SceneDeserializer {
            type_registry: &registry_read,
        }
        .deserialize(&mut de)
        .map_err(|e| LevelError::Parse(format!("{e:?}")))?
    };

    // Instantiate into a scratch World to resolve hierarchy + typed components.
    let mut world = bevy::ecs::world::World::new();
    world.insert_resource(registry.clone());
    let mut entity_map = EntityHashMap::default();
    scene
        .write_to_world(&mut world, &mut entity_map)
        .map_err(|e| LevelError::Parse(format!("{e:?}")))?;

    // Optional .meta sidecar for Library material resolution.
    let meta: MetaSidecar = std::fs::read_to_string(format!("{}.meta", path.display()))
        .ok()
        .and_then(|s| ron::from_str(&s).ok())
        .unwrap_or_default();

    let fallback_material =
        || MaterialDefinition::standard(Color::srgb(0.5, 0.5, 0.55));

    let mut out = LevelScene::default();

    let mut statics_q = world.query::<(Entity, &PrimitiveMarker)>();
    let statics: Vec<(Entity, PrimitiveShape)> = statics_q
        .iter(&world)
        .map(|(e, m)| (e, m.shape.clone()))
        .collect();
    for (e, shape) in statics {
        let t = world_transform(&world, e);
        let name = world
            .get::<Name>(e)
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|| "static".into());
        let material = match world.get::<MaterialRef>(e) {
            Some(MaterialRef::Inline(def)) => def.clone(),
            Some(MaterialRef::Library(key)) => meta
                .material_library
                .materials
                .get(key)
                .cloned()
                .unwrap_or_else(|| {
                    warn!("level material '{key}' not in the .meta library — using fallback");
                    fallback_material()
                }),
            None => fallback_material(),
        };
        out.statics.push(StaticDesc {
            name,
            shape,
            translation: t.translation,
            rotation: t.rotation,
            scale: t.scale,
            material,
        });
    }

    let mut spawn_q = world.query::<(Entity, &ArenaSpawnPoint)>();
    let spawns: Vec<(Entity, u8)> = spawn_q.iter(&world).map(|(e, s)| (e, s.slot)).collect();
    for (e, slot) in spawns {
        let t = world_transform(&world, e);
        out.spawns.push(SpawnDesc {
            slot,
            position: t.translation,
            yaw: yaw_from_forward(*t.forward()),
        });
    }
    out.spawns.sort_by_key(|s| s.slot);

    let mut point_q = world.query::<(Entity, &SceneLightMarker)>();
    let points: Vec<(Entity, SceneLightMarker)> =
        point_q.iter(&world).map(|(e, l)| (e, l.clone())).collect();
    for (e, l) in points {
        let t = world_transform(&world, e);
        out.lights.push(LightDesc::Point {
            position: t.translation,
            color: l.color,
            intensity: l.intensity,
            range: l.range,
            shadows: l.shadows_enabled,
        });
    }
    let mut dir_q = world.query::<(Entity, &DirectionalLightMarker)>();
    let dirs: Vec<(Entity, DirectionalLightMarker)> =
        dir_q.iter(&world).map(|(e, l)| (e, l.clone())).collect();
    for (e, l) in dirs {
        let t = world_transform(&world, e);
        out.lights.push(LightDesc::Directional {
            rotation: t.rotation,
            color: l.color,
            illuminance: l.illuminance,
            shadows: l.shadows_enabled,
        });
    }

    Ok(out)
}

/// A collider for `shape` with the level `scale` baked into the SHAPE dimensions — collider
/// scale stays 1.0.
///
/// Why not `Collider::set_scale`: avian's `update_collider_scale` (collider backend, registered
/// even when the transform-sync plugin is disabled) resets a collider's scale to the entity's
/// `Transform` scale whenever one is present — and under lightyear's avian Position replication
/// the physics entities DO acquire an identity `Transform`, which stomped a `set_scale`d floor
/// back to a 1m cube (players fell through the world; net-test caught it). Shape-baked dimensions
/// at scale 1.0 are a fixed point of that reset, on every peer, with or without a `Transform`.
///
/// Non-uniform x/z scale on Sphere/Cylinder/Capsule can't be represented by the primitive shape;
/// the larger axis wins (levels scale those roughly uniformly in practice).
fn scaled_collider(shape: &PrimitiveShape, scale: Vec3) -> Collider {
    let s = scale.abs();
    match shape {
        // Unit shapes per `PrimitiveShape::create_collider` (bevy_effect): cube 1×1×1 (full
        // extents), sphere r=0.5, cylinder r=0.5 h=1, capsule r=0.25 l=0.5, plane 2×0.01×2.
        PrimitiveShape::Cube => Collider::cuboid(s.x, s.y, s.z),
        PrimitiveShape::Sphere => Collider::sphere(0.5 * s.x.max(s.z)),
        PrimitiveShape::Cylinder => Collider::cylinder(0.5 * s.x.max(s.z), s.y),
        PrimitiveShape::Capsule => Collider::capsule(0.25 * s.x.max(s.z), 0.5 * s.y),
        PrimitiveShape::Plane => Collider::cuboid(2.0 * s.x, 0.01, 2.0 * s.z),
    }
}

/// Spawn a loaded level on this peer. Every peer gets the PHYSICS bundle (a static collider with
/// the level scale baked into the SHAPE — see [`scaled_collider`] — plus avian
/// `Position`/`Rotation`); pass `visuals` (windowed client) to also get meshes, materials, and
/// lights. The visual mesh bakes the scale into its VERTICES so the `Transform` stays scale-1.0 —
/// a scaled `Transform` would make avian re-scale the already-baked collider. Everything is
/// tagged [`LevelEntity`] — the despawn key for level switches.
pub fn spawn_level(
    commands: &mut Commands,
    scene: &LevelScene,
    mut visuals: Option<(&mut Assets<Mesh>, &mut Assets<StandardMaterial>)>,
) -> Vec<Entity> {
    let mut spawned = Vec::with_capacity(scene.statics.len() + scene.lights.len());
    for s in &scene.statics {
        let mut ec = commands.spawn((
            LevelEntity,
            Name::new(s.name.clone()),
            RigidBody::Static,
            scaled_collider(&s.shape, s.scale),
            Position(s.translation),
            Rotation(s.rotation),
        ));
        if let Some((meshes, materials)) = visuals.as_mut() {
            ec.insert((
                Mesh3d(meshes.add(s.shape.create_mesh().scaled_by(s.scale))),
                MeshMaterial3d(materials.add(s.material.base.to_standard_material())),
                Transform {
                    translation: s.translation,
                    rotation: s.rotation,
                    scale: Vec3::ONE,
                },
                Visibility::default(),
            ));
        }
        spawned.push(ec.id());
    }
    if visuals.is_some() {
        for l in &scene.lights {
            let id = match l {
                LightDesc::Point {
                    position,
                    color,
                    intensity,
                    range,
                    shadows,
                } => commands
                    .spawn((
                        LevelEntity,
                        PointLight {
                            color: *color,
                            intensity: *intensity,
                            range: *range,
                            shadows_enabled: *shadows,
                            ..Default::default()
                        },
                        Transform::from_translation(*position),
                    ))
                    .id(),
                LightDesc::Directional {
                    rotation,
                    color,
                    illuminance,
                    shadows,
                } => commands
                    .spawn((
                        LevelEntity,
                        DirectionalLight {
                            color: *color,
                            illuminance: *illuminance,
                            shadows_enabled: *shadows,
                            ..Default::default()
                        },
                        Transform::from_rotation(*rotation),
                    ))
                    .id(),
            };
            spawned.push(id);
        }
    }
    spawned
}

/// One discovered level file.
#[derive(Debug, Clone)]
pub struct LevelInfo {
    pub id: String,
    pub path: PathBuf,
}

/// The reserved lobby level id — never listed for host selection.
pub const LOBBY_LEVEL_ID: &str = "lobby";

/// The scanned level set. Scanned once at startup on every peer (all peers ship the same files).
#[derive(Resource, Debug, Clone, Default)]
pub struct LevelCatalog {
    pub levels: Vec<LevelInfo>,
}

impl LevelCatalog {
    /// The default scan roots (CWD = the arena workspace root): shipped levels live in
    /// `assets/scenes/`; levels the user saves from the editor (whose save browser is
    /// CWD-relative, launched from `crates/arena_editor`) land in
    /// `crates/arena_editor/assets/scenes/`. First-found-by-stem wins; duplicates warn.
    pub fn scan() -> Self {
        Self::scan_roots(&[
            PathBuf::from("assets/scenes"),
            PathBuf::from("crates/arena_editor/assets/scenes"),
        ])
    }

    pub fn scan_roots(roots: &[PathBuf]) -> Self {
        let mut levels: Vec<LevelInfo> = Vec::new();
        for root in roots {
            let Ok(entries) = std::fs::read_dir(root) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(".scn.ron"))
                })
                .collect();
            found.sort();
            for path in found {
                let Some(id) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.trim_end_matches(".scn.ron").to_string())
                else {
                    continue;
                };
                if levels.iter().any(|l| l.id == id) {
                    warn!(
                        "level '{id}' found in multiple scan roots — using the first, \
                         ignoring {path:?}"
                    );
                    continue;
                }
                levels.push(LevelInfo { id, path });
            }
        }
        Self { levels }
    }

    pub fn get(&self, id: &str) -> Option<&LevelInfo> {
        self.levels.iter().find(|l| l.id == id)
    }

    /// Host-selectable levels: everything except the reserved lobby.
    pub fn selectable(&self) -> impl Iterator<Item = &LevelInfo> {
        self.levels.iter().filter(|l| l.id != LOBBY_LEVEL_ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/level/fixtures/minimal.scn.ron"
    );

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
        // The fixture's spawn faces +X (90° yaw about Y): Quat(Y, yaw)*-Z == +X.
        let yaw = scene.spawns[0].yaw;
        let fwd = Quat::from_axis_angle(Vec3::Y, yaw) * -Vec3::Z;
        assert!((fwd - Vec3::X).length() < 1e-4, "fwd={fwd:?} yaw={yaw}");
    }

    #[test]
    fn unknown_component_types_fail_loud_with_names() {
        let dir = std::env::temp_dir().join(format!("arena_level_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.scn.ron");
        std::fs::write(
            &path,
            r#"(
  resources: {},
  entities: {
    1: ( components: {
      "bevy_modal_editor::scene::SceneEntity": (),
      "bevy_modal_editor::prefabs::PrefabInstance": (prefab_name: "x", instance_id: 1),
    }),
  },
)"#,
        )
        .unwrap();
        let err = load_level_scene(&path).unwrap_err();
        match err {
            LevelError::UnsupportedTypes(types) => {
                assert!(
                    types.iter().any(|t| t.contains("PrefabInstance")),
                    "{types:?}"
                );
            }
            other => panic!("expected UnsupportedTypes, got {other:?}"),
        }
    }

    #[test]
    fn arena_flat_matches_legacy_geometry() {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/scenes/arena_flat.scn.ron"
        ));
        let scene = load_level_scene(path).expect("arena_flat loads");
        let floor = scene
            .statics
            .iter()
            .find(|s| s.name == "Floor")
            .expect("floor");
        // Top face at world 0 (legacy spawn_arena_floor contract).
        assert!((floor.translation.y + floor.scale.y / 2.0).abs() < 1e-6);
        assert_eq!(floor.scale, Vec3::new(40.0, 1.0, 40.0));
        let slots: Vec<_> = scene.spawns.iter().map(|s| (s.slot, s.position)).collect();
        assert_eq!(slots[0], (0, Vec3::new(-4.0, crate::tuning::GROUND_Y, 0.0)));
        assert_eq!(slots[1], (1, Vec3::new(4.0, crate::tuning::GROUND_Y, 0.0)));
        // Duelists face each other: slot 0 looks +X, slot 1 looks -X.
        let fwd0 = Quat::from_axis_angle(Vec3::Y, scene.spawns[0].yaw) * -Vec3::Z;
        let fwd1 = Quat::from_axis_angle(Vec3::Y, scene.spawns[1].yaw) * -Vec3::Z;
        assert!((fwd0 - Vec3::X).length() < 1e-4, "{fwd0:?}");
        assert!((fwd1 - -Vec3::X).length() < 1e-4, "{fwd1:?}");
    }

    #[test]
    fn lobby_loads_with_spawns_statics_and_lights() {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/scenes/lobby.scn.ron"
        ));
        let scene = load_level_scene(path).expect("lobby loads");
        assert!(scene.spawns.len() >= 2, "lobby needs at least 2 spawns");
        assert!(!scene.statics.is_empty());
        assert!(!scene.lights.is_empty());
    }

    #[test]
    fn catalog_reserves_lobby_and_keys_by_stem() {
        let dir = std::env::temp_dir().join(format!("arena_catalog_test_{}", std::process::id()));
        let scenes = dir.join("assets/scenes");
        std::fs::create_dir_all(&scenes).unwrap();
        std::fs::write(scenes.join("lobby.scn.ron"), "(resources: {}, entities: {})").unwrap();
        std::fs::write(
            scenes.join("arena_flat.scn.ron"),
            "(resources: {}, entities: {})",
        )
        .unwrap();
        let catalog = LevelCatalog::scan_roots(&[scenes]);
        assert!(catalog.get("lobby").is_some());
        assert!(catalog.get("arena_flat").is_some());
        let selectable: Vec<_> = catalog.selectable().map(|l| l.id.as_str()).collect();
        assert_eq!(selectable, vec!["arena_flat"]);
    }
}
