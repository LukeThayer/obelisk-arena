//! Client visuals for replicated skill objects (portals, frost tiles, frost spires — the wisp
//! weapon ports). The server replicates `NetworkedSkillObject { kind, owner }` + avian
//! `Position`/`Rotation`; this module builds the visual per `kind` and mirrors the replicated
//! pose into the render `Transform` (the spire visibly rises). Windowed-only.
//!
//! PORTALS are true see-through windows (wisp's `spells/portal.rs` rig, Coding-Adventure
//! style): each disc gets a [`PortalMaterial`] whose texture is rendered by a dedicated
//! off-screen camera placed at the PAIRED portal's mirrored vantage
//! (`portals_shared::portal_camera_transform`) with an OBLIQUE near-clip plane on the exit
//! disc (Lengyel) so nothing between the camera and the disc leaks into the view. The shader
//! (`assets/shaders/portal.wgsl`) samples the texture in SCREEN SPACE — the disc behaves like
//! a hole in the world, not a decal. A disc whose pair is incomplete renders rim-only (camera
//! inactive → last texture, effectively dark) exactly like wisp.
//!
//! Every other kind keeps a simple mesh recipe; if a vfx preset named `skill_object_<kind>`
//! exists in the `VfxLibrary`, it's attached too — designers can dress kinds up from the
//! editor without touching code.

use avian3d::prelude::{Position, Rotation};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::RenderTarget;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, TextureFormat};
use bevy::shader::ShaderRef;
use bevy::window::PrimaryWindow;
use bevy_vfx::VfxLibrary;

use crate::net::protocol::NetworkedSkillObject;
use crate::portals_shared::{
    portal_camera_transform, PortalPose, KIND_PORTAL_BLUE, KIND_PORTAL_ORANGE, PORTAL_RADIUS,
    PORTAL_THICKNESS,
};
// The shared physical radius (server body + client Kinematic mirror), so the icy sphere renders
// exactly the size it collides.
use crate::server::glacier_ball::GLACIER_BALL_RADIUS;

use super::controller::FollowCamera;

pub struct SkillObjectVisualsPlugin;

impl Plugin for SkillObjectVisualsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<PortalMaterial>::default());
        app.add_systems(
            Update,
            (
                attach_skill_object_visuals,
                mirror_skill_object_pose,
                update_portal_cameras,
                cleanup_orphan_portal_cameras,
            )
                .chain(),
        );
    }
}

/// The portal disc material (wisp's): a render-target texture sampled in screen space + a rim
/// tint keyed off the cap UV. `assets/shaders/portal.wgsl`.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct PortalMaterial {
    #[uniform(0)]
    pub rim_color: LinearRgba,
    #[texture(1)]
    #[sampler(2)]
    pub portal_tex: Handle<Image>,
}

impl Material for PortalMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/portal.wgsl".into()
    }
}

/// Marker: visuals already attached.
#[derive(Component)]
struct SkillObjectVisual;

/// The off-screen camera rendering the through-view for `portal` (a replicated skill-object
/// entity). Despawned when the portal goes away.
#[derive(Component)]
struct PortalCamera {
    portal: Entity,
}

/// Through-portal texture cap (wisp `PORTAL_TEXTURE_MAX_DIM`): render at window resolution,
/// downscaled to keep the longest side at most this.
const PORTAL_TEXTURE_MAX_DIM: u32 = 1280;
const PORTAL_CAMERA_FOV_DEG: f32 = 90.0;

fn portal_texture_size(window: &Window) -> (u32, u32) {
    let w = window.physical_width().max(1);
    let h = window.physical_height().max(1);
    let longest = w.max(h);
    if longest <= PORTAL_TEXTURE_MAX_DIM {
        (w, h)
    } else {
        let scale = PORTAL_TEXTURE_MAX_DIM as f32 / longest as f32;
        (
            ((w as f32) * scale).round().max(1.0) as u32,
            ((h as f32) * scale).round().max(1.0) as u32,
        )
    }
}

/// The per-kind visual recipe for NON-portal kinds (portals take the material+camera path).
fn recipe(kind: &str) -> Option<(Mesh, StandardMaterial)> {
    match kind {
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

/// Attach visuals to each newly replicated object: portals get the see-through disc + their
/// dedicated render camera; other kinds get their mesh recipe (+ optional authored vfx preset).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn attach_skill_object_visuals(
    new: Query<(Entity, &NetworkedSkillObject, &Position, &Rotation), Without<SkillObjectVisual>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut portal_materials: ResMut<Assets<PortalMaterial>>,
    mut images: ResMut<Assets<Image>>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    vfx: Option<Res<VfxLibrary>>,
    mut commands: Commands,
) {
    for (e, obj, pos, rot) in &new {
        let base_tf = Transform::from_translation(pos.0).with_rotation(rot.0);

        // --- Portals: disc + render-to-texture camera (wisp's rig). ---
        if obj.kind == KIND_PORTAL_ORANGE || obj.kind == KIND_PORTAL_BLUE {
            let Some(window) = window.as_deref() else {
                commands.entity(e).insert(SkillObjectVisual);
                continue;
            };
            let (rim_color, name) = if obj.kind == KIND_PORTAL_ORANGE {
                (LinearRgba::rgb(1.5, 0.55, 0.12), "Portal-Orange")
            } else {
                (LinearRgba::rgb(0.25, 0.85, 2.2), "Portal-Blue")
            };
            let (tex_w, tex_h) = portal_texture_size(window);
            let render_image = images.add(Image::new_target_texture(
                tex_w,
                tex_h,
                TextureFormat::Bgra8UnormSrgb,
                None,
            ));
            commands.entity(e).insert((
                SkillObjectVisual,
                Name::new(name),
                Mesh3d(meshes.add(Cylinder::new(PORTAL_RADIUS, PORTAL_THICKNESS))),
                MeshMaterial3d(portal_materials.add(PortalMaterial {
                    rim_color,
                    portal_tex: render_image.clone(),
                })),
                base_tf,
                Visibility::default(),
            ));
            commands.spawn((
                Name::new(format!("{name}-Camera")),
                PortalCamera { portal: e },
                Camera3d::default(),
                Camera {
                    order: -1,
                    is_active: false,
                    ..default()
                },
                RenderTarget::Image(render_image.into()),
                Projection::Perspective(PerspectiveProjection {
                    fov: PORTAL_CAMERA_FOV_DEG.to_radians(),
                    ..default()
                }),
                Transform::default(),
                // Include SELF_BODY_LAYER: the local player's own (first-person-hidden) body
                // renders through portals — you can see yourself (wisp behavior).
                RenderLayers::from_layers(&[0, super::present::SELF_BODY_LAYER]),
            ));
            continue;
        }

        // --- Glacier boulder (THE AUTHORITY FLIP): the icy mesh + wisp material + point light live
        // on the REPLICATED ROOT. The server's ball is a real Dynamic body, so its `Rotation`
        // replicates real rolling — `mirror_skill_object_pose` (and the Kinematic mirror's avian
        // sync) drive the root Transform, so the sphere visibly rolls with NO cosmetic spin helper.
        // Wisp `glacier_ball.body.ron` material + light, verbatim. ---
        if obj.kind == "glacier_ball" {
            let mut ec = commands.entity(e);
            ec.insert((
                SkillObjectVisual,
                Mesh3d(meshes.add(Sphere::new(GLACIER_BALL_RADIUS))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    // wisp: base_color (0.55, 0.85, 1.0), emissive (0.5, 1.1, 1.8) (HDR — bevy's
                    // emissive glows for values > 1), roughness 0.15.
                    base_color: Color::srgb(0.55, 0.85, 1.0),
                    emissive: LinearRgba::rgb(0.5, 1.1, 1.8),
                    perceptual_roughness: 0.15,
                    ..Default::default()
                })),
                base_tf,
                Visibility::default(),
            ));
            // wisp's cold point light (color (0.55, 0.85, 1.0), intensity 28000, range 9), shadows
            // off — a rolling ball of light.
            ec.insert(PointLight {
                color: Color::srgb(0.55, 0.85, 1.0),
                intensity: 28_000.0,
                range: 9.0,
                shadows_enabled: false,
                ..Default::default()
            });
            // Optional designer dressing on the root (the kind→vfx pattern; no preset authored now).
            if let Some(system) = vfx
                .as_deref()
                .and_then(|lib| lib.effects.get("skill_object_glacier_ball"))
                .cloned()
            {
                ec.insert(system);
            }
            continue;
        }

        // --- Everything else: the simple recipe. ---
        let Some((mesh, material)) = recipe(&obj.kind) else {
            commands.entity(e).insert(SkillObjectVisual);
            continue;
        };
        let mut ec = commands.entity(e);
        ec.insert((
            SkillObjectVisual,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(material)),
            base_tf,
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

/// Visual pose-smoothing rate: the per-frame lerp factor is `1 - exp(-dt * POSE_SMOOTH_RATE)`, an
/// exponential approach that converges ~90% in ~115 ms at 60 fps. This hides the 30 Hz replication
/// ALIASING of the glacier ball's landing micro-bounce — the Step-0 evidence showed NO solver
/// penetration (per-tick landing minimum center Y 0.3178, settle band 0.3124–0.3476 around the 0.32
/// rest = a 35 mm restitution bounce), so raw snapping of those 30 Hz samples was the whole sink-pop.
/// Steady-state lag for a constant-velocity target is `v / POSE_SMOOTH_RATE` (~5 cm at the ~1 m/s
/// settle where it matters; a mild uniform lag during fast flight, which reads as smooth, not a pop).
/// Tune here if the flight lag wants trimming.
const POSE_SMOOTH_RATE: f32 = 20.0;

/// A replicated-pose jump beyond this is a TELEPORT (a portal warp — the glacier ball traverses
/// portals — or a round-reset re-placement), not motion: snap so the visual doesn't glide across the
/// arena. Mirrors `harness::snap_large_corrections`'s 2 m teleport threshold for predicted players.
const POSE_SNAP_DISTANCE: f32 = 2.0;

/// Mirror the replicated pose into the render Transform, SMOOTHED. Each rendered frame the visual
/// Transform lerps (position) / slerps (rotation) toward the latest replicated `Position`/`Rotation`
/// instead of snapping, so the 30 Hz-replicated glacier-ball landing bounce reads as a smooth settle
/// (the reported sink-pop) and the spire's rise is smoother too. Two cases still SNAP:
///   * the very first pose — `attach_skill_object_visuals` (chained before this) seeds the Transform
///     to the spawn pose, so on the add frame `tf ≈ pos`, the lerp is a no-op, and the > 2 m guard is
///     also false: an effective spawn snap with no extra per-entity state;
///   * a teleport (> `POSE_SNAP_DISTANCE` position jump: a portal warp / round-reset re-placement) —
///     snap position AND rotation so the ball doesn't glide the width of the arena.
/// RENDER-ONLY: collision reads the replicated avian `Position` directly (the ball's Kinematic mirror,
/// `client/net.rs`), so the physical shove stays in prediction lockstep — this smoothing never
/// touches it. Unknown-kind objects carry no `Transform` (see `attach_…`), so the query skips them.
fn mirror_skill_object_pose(
    time: Res<Time>,
    mut q: Query<(&Position, &Rotation, &mut Transform), With<NetworkedSkillObject>>,
) {
    let alpha = 1.0 - (-time.delta_secs() * POSE_SMOOTH_RATE).exp();
    for (pos, rot, mut tf) in &mut q {
        if tf.translation.distance(pos.0) > POSE_SNAP_DISTANCE {
            tf.translation = pos.0;
            tf.rotation = rot.0;
        } else {
            tf.translation = tf.translation.lerp(pos.0, alpha);
            tf.rotation = tf.rotation.slerp(rot.0, alpha);
        }
    }
}

/// Drive every portal camera from the LOCAL VIEWER's camera: entry = the camera's own disc,
/// exit = the same owner's other disc. Inactive until the pair is complete. Oblique near-clip
/// plane = the exit disc plane (Lengyel 2005) so the wall back / entry disc / anything between
/// the virtual camera and the exit is clipped out of the through-view.
#[allow(clippy::type_complexity)]
fn update_portal_cameras(
    viewer: Option<Single<&GlobalTransform, (With<FollowCamera>, Without<PortalCamera>)>>,
    portals: Query<(Entity, &NetworkedSkillObject, &Position, &Rotation)>,
    mut cameras: Query<
        (&PortalCamera, &mut Transform, &mut Camera, &mut Projection),
        Without<NetworkedSkillObject>,
    >,
) {
    let Some(viewer) = viewer else { return };
    let viewer_tf = viewer.compute_transform();

    for (portal_cam, mut cam_tf, mut camera, mut projection) in &mut cameras {
        // Resolve this camera's own disc, then its pair mate (same owner, other color).
        let Ok((_, me, my_pos, my_rot)) = portals.get(portal_cam.portal) else {
            continue; // orphan — cleanup system despawns it
        };
        let mate_kind = if me.kind == KIND_PORTAL_ORANGE {
            KIND_PORTAL_BLUE
        } else {
            KIND_PORTAL_ORANGE
        };
        let mate = portals
            .iter()
            .find(|(_, o, ..)| o.owner == me.owner && o.kind == mate_kind);
        let Some((_, _, mate_pos, mate_rot)) = mate else {
            if camera.is_active {
                camera.is_active = false;
            }
            continue;
        };

        // Looking AT this disc, you see the world at the OTHER disc: entry = this disc,
        // exit = the mate.
        let entry = PortalPose::new(my_pos.0, my_rot.0);
        let exit = PortalPose::new(mate_pos.0, mate_rot.0);
        *cam_tf = portal_camera_transform(viewer_tf, &entry, &exit);
        if !camera.is_active {
            camera.is_active = true;
        }

        if let Projection::Perspective(p) = &mut *projection {
            let cam_rot_inv = cam_tf.rotation.inverse();
            let normal_view = (cam_rot_inv * exit.normal).normalize();
            let q_view = cam_rot_inv * (exit.position - cam_tf.translation);
            let w = -normal_view.dot(q_view);
            p.near_clip_plane = normal_view.extend(w);
        }
    }
}

/// Despawn cameras whose portal entity is gone (the server re-placed or expired the disc — the
/// replicated entity despawns and the camera would otherwise leak).
fn cleanup_orphan_portal_cameras(
    cameras: Query<(Entity, &PortalCamera)>,
    portals: Query<(), With<NetworkedSkillObject>>,
    mut commands: Commands,
) {
    for (cam_entity, portal_cam) in &cameras {
        if portals.get(portal_cam.portal).is_err() {
            commands.entity(cam_entity).despawn();
        }
    }
}

