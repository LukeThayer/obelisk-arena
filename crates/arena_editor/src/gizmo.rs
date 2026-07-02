//! Viewport gizmo for the Skill designer: maps the selected hit-window's authored `CollisionShape`
//! into a render-space `GizmoShape` (`gizmo_shape`, cone degrees→half-radians) and draws it at the
//! preview caster's position while the editor is in Skill mode (`draw_window_gizmo`, mode-gated).
//! Only `gizmo_shape` is unit-tested (pure); the draw system is covered by `cargo build`.

use crate::model::EditedSkill;
use crate::skill_designer::SKILL_MODE_ID;
use arena_sim::preview::PreviewCaster;
use arena_sim::spawn::SPAWN_MARKERS;
use bevy::prelude::*;
use bevy_modal_editor::{CustomModeId, EditorMode};
use obelisk_bevy::assets::{CollisionShape, VolumeMotion};

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

/// Sample the window's flight path over its `active_duration` for the trajectory gizmo (analytic
/// ballistic `p = o + v·t − ½·g·t²·ŷ` — visually indistinguishable from the sim's fixed-step
/// Euler). `Static` motion yields no points.
pub fn trajectory_points(
    motion: &VolumeMotion,
    duration: f32,
    origin: Vec3,
    dir: Vec3,
) -> Vec<Vec3> {
    let (speed, gravity) = match *motion {
        VolumeMotion::Static | VolumeMotion::Beam => return Vec::new(),
        VolumeMotion::Linear { speed } => (speed, 0.0),
        VolumeMotion::Ballistic { speed, gravity } => (speed, gravity),
    };
    let v = dir * speed;
    const SAMPLES: usize = 32;
    let mut points = Vec::with_capacity(SAMPLES + 1);
    for i in 0..=SAMPLES {
        let t = duration * i as f32 / SAMPLES as f32;
        let p = origin + v * t - Vec3::Y * (0.5 * gravity * t * t);
        points.push(p);
        // The sim ground-stops projectile hitboxes at the floor plane — clip the guide the same.
        if p.y < 0.0 {
            break;
        }
    }
    points
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
    // The caster's position while a duel is up; the caster spawn marker while just editing — so
    // the trajectory is visible (and tunable) without hitting Play.
    let origin = caster
        .single()
        .map(|t| t.translation)
        .unwrap_or(SPAWN_MARKERS[0]);
    let c = Color::srgb(1.0, 0.4, 0.1);
    // Flight-path polyline toward the dummy marker for moving windows: Linear flies flat,
    // Ballistic gets the same LOFTED launch the preview cast solves (land the arc on the dummy).
    let target = SPAWN_MARKERS[1];
    let dir = match window.motion {
        VolumeMotion::Ballistic { speed, gravity } => {
            arena_sim::ballistics::ballistic_launch_dir(origin, target, speed, gravity)
        }
        _ => (target - origin).normalize_or(Vec3::X),
    };
    let path = trajectory_points(&window.motion, window.active_duration, origin, dir);
    if path.len() > 1 {
        gizmos.linestrip(path, Color::srgb(1.0, 0.8, 0.2));
    }
    // Beam windows: the arc line to the staged victim, plus the retarget hop radius around it.
    if matches!(window.motion, VolumeMotion::Beam) {
        let beam_c = Color::srgb(0.5, 0.8, 1.0);
        gizmos.line(origin + Vec3::Y * 1.2, target, beam_c);
        if let Some(obelisk_bevy::assets::EndReaction::Retarget { radius, .. }) =
            &window.on_end.hit
        {
            gizmos.circle(
                Isometry3d::new(target, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                *radius,
                beam_c,
            );
        }
    }
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
    fn trajectory_points_arc_and_straight() {
        // Static: nothing to draw.
        assert!(trajectory_points(&VolumeMotion::Static, 1.0, Vec3::ZERO, Vec3::X).is_empty());
        // Linear at ground level: flat along the axis, full length, endpoint at speed * duration.
        let flat = trajectory_points(
            &VolumeMotion::Linear { speed: 20.0 },
            2.0,
            Vec3::ZERO,
            Vec3::X,
        );
        assert_eq!(flat.len(), 33);
        let end = *flat.last().unwrap();
        assert!((end.x - 40.0).abs() < 1e-4 && end.y == 0.0);
        // Ballistic from a height: arcs down and CLIPS at the floor plane (the sim ground-stops
        // there) instead of sampling the full duration underground.
        let arc = trajectory_points(
            &VolumeMotion::Ballistic {
                speed: 20.0,
                gravity: 9.8,
            },
            2.0,
            Vec3::Y * 2.0,
            Vec3::X,
        );
        assert!(arc.len() < 33, "clipped at the ground: {} points", arc.len());
        let end = *arc.last().unwrap();
        let before_end = arc[arc.len() - 2];
        assert!(end.y < 0.0 && before_end.y >= 0.0, "stops at the first below-ground sample");
    }

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
