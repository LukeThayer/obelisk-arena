//! Viewport gizmo for the Skill designer: maps the selected hit-window's authored `CollisionShape`
//! into a render-space `GizmoShape` (`gizmo_shape`, cone degrees→half-radians) and draws it at the
//! preview caster's position while the editor is in Skill mode (`draw_window_gizmo`, mode-gated).
//! Only `gizmo_shape` is unit-tested (pure); the draw system is covered by `cargo build`.

use crate::model::EditedSkill;
use crate::skill_designer::SKILL_MODE_ID;
use arena_sim::preview::PreviewCaster;
use bevy::prelude::*;
use bevy_modal_editor::{CustomModeId, EditorMode};
use obelisk_bevy::assets::CollisionShape;

/// A hit-window shape in render-friendly form: cone angles are pre-halved into radians and radii /
/// heights pass through, so `draw_window_gizmo` never re-derives geometry.
#[derive(Debug, Clone, Copy)]
pub enum GizmoShape {
    Sphere { radius: f32 },
    Capsule { radius: f32, height: f32 },
    Cone { half_angle_rad: f32, range: f32 },
}

/// Convert an authored `CollisionShape` into its `GizmoShape`. Cone `angle` is the FULL sector in
/// degrees; the gizmo needs the half-angle in radians (`angle.to_radians() * 0.5`).
pub fn gizmo_shape(shape: &CollisionShape) -> GizmoShape {
    match *shape {
        CollisionShape::Sphere { radius } => GizmoShape::Sphere { radius },
        CollisionShape::Capsule { radius, height } => GizmoShape::Capsule { radius, height },
        CollisionShape::Cone { angle, range } => GizmoShape::Cone {
            half_angle_rad: angle.to_radians() * 0.5,
            range,
        },
    }
}

/// Draw the selected hit-window's shape at the preview caster's origin, but only in Skill mode with a
/// window selected. No-op outside Skill mode / when nothing is selected / when the index is stale.
pub fn draw_window_gizmo(
    mut gizmos: Gizmos,
    edited: Res<EditedSkill>,
    caster: Query<&Transform, With<PreviewCaster>>,
    mode: Res<State<EditorMode>>,
) {
    if *mode.get() != EditorMode::Custom(CustomModeId(SKILL_MODE_ID)) {
        return;
    }
    let Some(idx) = edited.selected_window else {
        return;
    };
    let Some(window) = edited.timeline.collision_windows.get(idx) else {
        return;
    };
    let origin = caster.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    let c = Color::srgb(1.0, 0.4, 0.1);
    match gizmo_shape(&window.shape) {
        GizmoShape::Sphere { radius } => {
            gizmos.sphere(Isometry3d::from_translation(origin), radius, c);
        }
        GizmoShape::Capsule { radius, height } => {
            let half = height * 0.5 + radius;
            gizmos.line(origin - Vec3::Y * half, origin + Vec3::Y * half, c);
            gizmos.sphere(Isometry3d::from_translation(origin), radius, c);
        }
        GizmoShape::Cone {
            half_angle_rad,
            range,
        } => {
            let e1 = Quat::from_rotation_y(half_angle_rad) * (Vec3::Z * range);
            let e2 = Quat::from_rotation_y(-half_angle_rad) * (Vec3::Z * range);
            gizmos.line(origin, origin + e1, c);
            gizmos.line(origin, origin + e2, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_4;

    #[test]
    fn gizmo_shape_maps_cone_degrees_to_half_radians() {
        let g = gizmo_shape(&CollisionShape::Cone {
            angle: 90.0,
            range: 5.0,
        });
        match g {
            GizmoShape::Cone {
                half_angle_rad,
                range,
            } => {
                assert!((half_angle_rad - FRAC_PI_4).abs() < 1e-5);
                assert_eq!(range, 5.0);
            }
            other => panic!("expected Cone, got {other:?}"),
        }
    }

    #[test]
    fn gizmo_shape_passes_through_sphere_and_capsule_dims() {
        match gizmo_shape(&CollisionShape::Sphere { radius: 0.5 }) {
            GizmoShape::Sphere { radius } => assert_eq!(radius, 0.5),
            other => panic!("expected Sphere, got {other:?}"),
        }
        match gizmo_shape(&CollisionShape::Capsule {
            radius: 0.35,
            height: 0.48,
        }) {
            GizmoShape::Capsule { radius, height } => {
                assert_eq!(radius, 0.35);
                assert_eq!(height, 0.48);
            }
            other => panic!("expected Capsule, got {other:?}"),
        }
    }
}
