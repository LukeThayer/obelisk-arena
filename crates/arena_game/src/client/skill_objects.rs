//! Client visuals for replicated skill objects (portals, frost tiles, frost spires — the wisp
//! weapon ports). The server replicates `NetworkedSkillObject { kind, .. }` + avian
//! `Position`/`Rotation`; this module builds a simple mesh per `kind` (wisp's shapes/colors) and
//! mirrors the replicated pose into the render `Transform` (the spire visibly rises). If a vfx
//! preset named `skill_object_<kind>` exists in the `VfxLibrary`, it's attached too — so a
//! designer can dress any object kind up from the editor without touching code. Windowed-only.

use avian3d::prelude::{Position, Rotation};
use bevy::prelude::*;
use bevy_vfx::VfxLibrary;

use crate::net::protocol::NetworkedSkillObject;

pub struct SkillObjectVisualsPlugin;

impl Plugin for SkillObjectVisualsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (attach_skill_object_visuals, mirror_skill_object_pose));
    }
}

/// Marker: visuals already attached.
#[derive(Component)]
struct SkillObjectVisual;

/// The per-kind visual recipe (wisp's shapes and rim colors).
fn recipe(kind: &str) -> Option<(Mesh, StandardMaterial)> {
    let emissive = |c: Color, strength: f32| StandardMaterial {
        base_color: c.with_alpha(0.85),
        emissive: c.to_linear() * strength,
        alpha_mode: AlphaMode::Blend,
        unlit: false,
        ..Default::default()
    };
    match kind {
        "portal_orange" => Some((
            Cylinder::new(crate::net::PORTAL_RADIUS, 0.05).into(),
            emissive(Color::srgb(1.0, 0.45, 0.1), 3.0),
        )),
        "portal_blue" => Some((
            Cylinder::new(crate::net::PORTAL_RADIUS, 0.05).into(),
            emissive(Color::srgb(0.2, 0.55, 1.0), 3.0),
        )),
        "frost_tile" => Some((
            Cylinder::new(0.45, 0.06).into(),
            StandardMaterial {
                base_color: Color::srgba(0.55, 0.85, 1.0, 0.55),
                emissive: LinearRgba::rgb(0.1, 0.35, 0.6),
                alpha_mode: AlphaMode::Blend,
                ..Default::default()
            },
        )),
        "frost_spire" => Some((
            Cuboid::new(0.55, 1.6, 0.55).into(),
            StandardMaterial {
                base_color: Color::srgb(0.75, 0.9, 1.0),
                emissive: LinearRgba::rgb(0.15, 0.4, 0.7),
                perceptual_roughness: 0.25,
                ..Default::default()
            },
        )),
        _ => None,
    }
}

/// Attach the mesh/material (+ optional authored vfx preset) to each newly replicated object.
#[allow(clippy::type_complexity)]
fn attach_skill_object_visuals(
    new: Query<
        (Entity, &NetworkedSkillObject, &Position, &Rotation),
        Without<SkillObjectVisual>,
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    vfx: Option<Res<VfxLibrary>>,
    mut commands: Commands,
) {
    for (e, obj, pos, rot) in &new {
        let Some((mesh, material)) = recipe(&obj.kind) else {
            commands.entity(e).insert(SkillObjectVisual);
            continue;
        };
        let mut ec = commands.entity(e);
        ec.insert((
            SkillObjectVisual,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(material)),
            Transform::from_translation(pos.0).with_rotation(rot.0),
            Visibility::default(),
        ));
        // Optional designer dressing: a vfx preset named for the kind.
        if let Some(system) = vfx
            .as_deref()
            .and_then(|lib| lib.effects.get(&format!("skill_object_{}", obj.kind)))
            .cloned()
        {
            ec.insert(system);
        }
    }
}

/// Mirror the replicated pose into the render Transform (spires rise; future kinds may move).
fn mirror_skill_object_pose(
    mut q: Query<(&Position, &Rotation, &mut Transform), With<NetworkedSkillObject>>,
) {
    for (pos, rot, mut tf) in &mut q {
        tf.translation = pos.0;
        tf.rotation = rot.0;
    }
}
