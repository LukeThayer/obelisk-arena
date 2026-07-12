//! Client visuals for replicated skill objects (portals, frost tiles, frost spires, the glacier
//! boulder — the wisp weapon ports). The server replicates `NetworkedSkillObject { kind, owner }` +
//! avian `Position`/`Rotation`; this module builds the visual per `kind`. The glacier ball + spire
//! carry their mesh on a SNAPSHOT-INTERPOLATED child (rendered ~50 ms behind the newest replicated
//! state so the boulder rolls smoothly between the 30 Hz steps instead of jumping ~23 cm per
//! snapshot); the root Transform stays pinned to the raw replicated pose (avian / `mirror_…`) and the
//! child's local transform counter-corrects it (`child_local_for`). Windowed-only.
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
                buffer_skill_object_poses,
                mirror_skill_object_pose,
                interpolate_skill_object_visuals,
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

/// Marker on the REPLICATED ROOT: the per-kind visuals have been attached (the `attach_…` run-once
/// guard). Kept OFF the mesh child so the guard tracks the root, not the visual.
#[derive(Component)]
struct SkillObjectVisualsAttached;

/// Marker on the spawned mesh CHILD (glacier ball / spire): the entity carrying the mesh + material
/// (+ the ball's light), snapshot-interpolated by [`interpolate_skill_object_visuals`] — its LOCAL
/// transform counter-corrects the stepping root.
#[derive(Component)]
struct SkillObjectVisual;

/// How many replicated pose samples to keep per skill object. At the 30 Hz send cadence 4 samples
/// span ~100 ms of history — comfortably deeper than the [`INTERP_DELAY_SECS`] (~50 ms) render lag,
/// so the delayed render time is always bracketed by two real samples.
const POSE_BUFFER_CAP: usize = 4;

/// Snapshot-interpolation state for one skill object, on the REPLICATED ROOT (despawns with it; the
/// mesh child despawns recursively with the root). Holds the mesh child it drives + the recent pose
/// history (oldest→newest).
#[derive(Component)]
struct SkillObjectInterp {
    /// The mesh-child entity whose LOCAL transform is counter-corrected each frame.
    visual: Entity,
    /// `(recv_secs, pos, rot)` history, oldest→newest, capped at [`POSE_BUFFER_CAP`].
    samples: Vec<PoseSample>,
}

impl SkillObjectInterp {
    fn new(visual: Entity) -> Self {
        Self {
            visual,
            samples: Vec::with_capacity(POSE_BUFFER_CAP),
        }
    }

    /// Append the newest sample, dropping the oldest once the cap is hit.
    fn push(&mut self, recv: f32, pos: Vec3, rot: Quat) {
        if self.samples.len() == POSE_BUFFER_CAP {
            self.samples.remove(0);
        }
        self.samples.push(PoseSample { recv, pos, rot });
    }
}

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
    new: Query<
        (Entity, &NetworkedSkillObject, &Position, &Rotation),
        Without<SkillObjectVisualsAttached>,
    >,
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

        // --- Portals: disc + render-to-texture camera (wisp's rig). BESPOKE — the disc mesh stays on
        // the replicated ROOT (a portal is placed once and never moves; the spawn `base_tf` fixes it,
        // and `update_portal_cameras` reads `Position`/`Rotation` directly). No mesh child / interp. ---
        if obj.kind == KIND_PORTAL_ORANGE || obj.kind == KIND_PORTAL_BLUE {
            let Some(window) = window.as_deref() else {
                commands.entity(e).insert(SkillObjectVisualsAttached);
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
                SkillObjectVisualsAttached,
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

        // --- Glacier boulder (THE AUTHORITY FLIP): the icy mesh + wisp material + point light ride a
        // spawned mesh CHILD, snapshot-interpolated (`interpolate_skill_object_visuals`) so the sphere
        // ROLLS between real 30 Hz states instead of stepping ~23 cm per snapshot. The REPLICATED ROOT
        // keeps the kinematic collider mirror (`client/net.rs`) + replicated `Position`/`Rotation`
        // (avian pins the root Transform to that raw pose; the child counter-corrects it). Wisp
        // `glacier_ball.body.ron` material + light, verbatim. ---
        if obj.kind == "glacier_ball" {
            let visual = commands
                .spawn((
                    SkillObjectVisual,
                    Name::new("Glacier-Ball-Mesh"),
                    ChildOf(e),
                    Mesh3d(meshes.add(Sphere::new(GLACIER_BALL_RADIUS))),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        // wisp: base_color (0.55, 0.85, 1.0), emissive (0.5, 1.1, 1.8) (HDR — bevy's
                        // emissive glows for values > 1), roughness 0.15.
                        base_color: Color::srgb(0.55, 0.85, 1.0),
                        emissive: LinearRgba::rgb(0.5, 1.1, 1.8),
                        perceptual_roughness: 0.15,
                        ..Default::default()
                    })),
                    // wisp's cold point light (color (0.55, 0.85, 1.0), intensity 28000, range 9),
                    // shadows off — a rolling ball of light. It rides the smooth mesh child.
                    PointLight {
                        color: Color::srgb(0.55, 0.85, 1.0),
                        intensity: 28_000.0,
                        range: 9.0,
                        shadows_enabled: false,
                        ..Default::default()
                    },
                    // Identity until the first sample lands: the child then sits AT the root pose.
                    Transform::IDENTITY,
                    Visibility::default(),
                ))
                .id();
            // Optional designer dressing rides the mesh child (the kind→vfx pattern; none authored now).
            if let Some(system) = vfx
                .as_deref()
                .and_then(|lib| lib.effects.get("skill_object_glacier_ball"))
                .cloned()
            {
                commands.entity(visual).insert(system);
            }
            commands.entity(e).insert((
                SkillObjectVisualsAttached,
                SkillObjectInterp::new(visual),
                base_tf,
                Visibility::default(),
            ));
            continue;
        }

        // --- Everything else that shares the generic mesh recipe (the frost spire): mesh + material
        // on a spawned CHILD, snapshot-interpolated exactly like the ball, so the server-side rise
        // reads smooth. The root keeps the replicated pose; `mirror_skill_object_pose` pins its
        // Transform (the spire has no client physics body — nothing else would). ---
        let Some((mesh, material)) = recipe(&obj.kind) else {
            commands.entity(e).insert(SkillObjectVisualsAttached);
            continue;
        };
        let visual = commands
            .spawn((
                SkillObjectVisual,
                Name::new(format!("{}-Mesh", obj.kind)),
                ChildOf(e),
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(materials.add(material)),
                Transform::IDENTITY,
                Visibility::default(),
            ))
            .id();
        // Optional designer dressing: a vfx preset named for the kind, on the mesh child.
        if let Some(system) = vfx
            .as_deref()
            .and_then(|lib| lib.effects.get(&format!("skill_object_{}", obj.kind)))
            .cloned()
        {
            commands.entity(visual).insert(system);
        }
        commands.entity(e).insert((
            SkillObjectVisualsAttached,
            SkillObjectInterp::new(visual),
            base_tf,
            Visibility::default(),
        ));
    }
}

/// A jump between two CONSECUTIVE replicated samples beyond this is a TELEPORT (a portal warp — the
/// glacier ball traverses portals — or a round-reset re-placement), not motion: [`sample_pose_at`]
/// SNAPS across such a pair (no blend) so the visual doesn't glide across the arena. Mirrors
/// `harness::snap_large_corrections`'s 2 m teleport threshold for predicted players (the sink-round
/// precedent).
const POSE_SNAP_DISTANCE: f32 = 2.0;

/// Render skill-object visuals this far behind the newest replicated state — classic snapshot
/// interpolation. `1.5 × 1/REPLICATION_SEND_HZ` (30 Hz → 33 ms ⇒ ~50 ms): far enough behind that the
/// render time is always bracketed by two REAL samples, so we interpolate and NEVER extrapolate. A
/// constant ~50 ms visual lag is imperceptible on a rolling boulder, and players are unaffected —
/// they are predicted (`client/net.rs`), only these cosmetic skill-object meshes lag.
const INTERP_DELAY_SECS: f32 = 1.5 / crate::net::server::REPLICATION_SEND_HZ as f32;

/// One buffered replicated pose: its receive time (`Time::elapsed_secs` seconds — the interp clock)
/// and the raw avian pose. Snapshot-interpolation history for the smooth visual.
#[derive(Clone, Copy)]
struct PoseSample {
    recv: f32,
    pos: Vec3,
    rot: Quat,
}

/// Sample the buffered pose history (oldest→newest) at render time `t` (seconds, same clock as the
/// stored `recv`). Binary-searches the bracketing sample pair and lerps/slerps by the pair-local
/// fraction. CLAMPS to the oldest/newest sample outside the buffered span — it NEVER extrapolates
/// (past the newest sample it HOLDS the newest, so a stalled stream freezes rather than flying off).
/// A pair whose endpoints are more than [`POSE_SNAP_DISTANCE`] apart is a TELEPORT: SNAP to the newer
/// endpoint (no blend) so a portal warp / round-reset re-placement doesn't glide across the arena
/// (the sink-round precedent — `harness::snap_large_corrections`). `None` iff the buffer is empty.
fn sample_pose_at(samples: &[PoseSample], t: f32) -> Option<(Vec3, Quat)> {
    match samples {
        [] => None,
        [only] => Some((only.pos, only.rot)),
        [oldest, .., newest] => {
            if t >= newest.recv {
                return Some((newest.pos, newest.rot)); // clamp — NEVER extrapolate forward
            }
            if t <= oldest.recv {
                return Some((oldest.pos, oldest.rot)); // clamp to the oldest
            }
            // Binary-search the bracketing pair [a, b] with `a.recv <= t < b.recv`.
            let hi = samples.partition_point(|sm| sm.recv <= t);
            let a = &samples[hi - 1];
            let b = &samples[hi];
            if a.pos.distance(b.pos) > POSE_SNAP_DISTANCE {
                return Some((b.pos, b.rot)); // teleport — reveal the destination, do NOT glide
            }
            let span = (b.recv - a.recv).max(f32::EPSILON);
            let frac = ((t - a.recv) / span).clamp(0.0, 1.0);
            Some((a.pos.lerp(b.pos, frac), a.rot.slerp(b.rot, frac)))
        }
    }
}

/// The mesh child's LOCAL transform such that, parented to a root at (`parent_pos`, `parent_rot`),
/// its WORLD pose is exactly (`target_pos`, `target_rot`). Counter-corrects the stepping root: avian
/// pins a physics-body root's Transform to the raw, 30Hz-stepped replicated Position, so the smoothly
/// interpolated visual is expressed RELATIVE to that root —
/// `local_pos = parent_rot⁻¹·(target_pos − parent_pos)`, `local_rot = parent_rot⁻¹·target_rot`.
fn child_local_for(
    parent_pos: Vec3,
    parent_rot: Quat,
    target_pos: Vec3,
    target_rot: Quat,
) -> Transform {
    let inv = parent_rot.inverse();
    Transform {
        translation: inv * (target_pos - parent_pos),
        rotation: inv * target_rot,
        scale: Vec3::ONE,
    }
}

/// Mirror each replicated skill-object pose onto its ROOT render Transform — RAW, unsmoothed. The old
/// exponential smoothing here was a proven no-op (avian's `position_to_transform` re-pins the root to
/// the raw `Position` every frame; the smoothing lerp got clobbered — divergence 0.000000 over 2926
/// samples in the sink investigation), so the smoothness now lives on the interpolated mesh CHILD
/// ([`interpolate_skill_object_visuals`]). This just keeps the root Transform EQUAL to the live
/// replicated `Position`/`Rotation`, which is exactly the parent pose the child's counter-transform
/// ([`child_local_for`]) is computed against.
///
/// REQUIRED for a kind with NO client physics body — the frost spire rises server-side and replicates
/// only `Position`, and nothing else moves its Transform (without this it stays buried). REDUNDANT but
/// harmless for the glacier ball (avian pins the root to this same `Position` — same value, last write
/// wins) and portals (placed once, static — the spawn `base_tf` already suffices). RENDER-ONLY:
/// collision + the shove read the replicated `Position` on the ROOT directly (the ball's Kinematic
/// mirror + the subfloor witness, `client/net.rs`) — never this Transform. Portals and unknown kinds
/// keep the mesh on the root; the ball and spire carry it on a child that this pose parents.
fn mirror_skill_object_pose(
    mut q: Query<(&Position, &Rotation, &mut Transform), With<NetworkedSkillObject>>,
) {
    for (pos, rot, mut tf) in &mut q {
        tf.translation = pos.0;
        tf.rotation = rot.0;
    }
}

/// Record each newly-replicated skill-object pose (with its receive time) into the interp buffer.
/// `Changed<Position>` fires when a replication snapshot lands — and ONLY then: the ball's velocity is
/// per-entity excluded (`server/verbs.rs`) so its Kinematic mirror carries no `LinearVelocity` for
/// avian to integrate, and the spire has no client body at all, so nothing re-marks `Position` between
/// snapshots. One sample per 30 Hz snapshot at its true arrival time — exactly the history snapshot
/// interpolation needs. Only ball/spire roots carry a `SkillObjectInterp` (portals/unknowns don't).
fn buffer_skill_object_poses(
    time: Res<Time>,
    mut q: Query<(&Position, &Rotation, &mut SkillObjectInterp), Changed<Position>>,
) {
    let now = time.elapsed_secs();
    for (pos, rot, mut interp) in &mut q {
        interp.push(now, pos.0, rot.0);
    }
}

/// Snapshot-interpolate each skill object's mesh CHILD ~[`INTERP_DELAY_SECS`] behind the newest
/// replicated state — the boulder ROLLS between real states instead of stepping ~23 cm per 30 Hz
/// snapshot. The root Transform is pinned to the raw stepped `Position` (avian for the ball,
/// [`mirror_skill_object_pose`] for the bodyless spire), so we express the smooth pose on the child,
/// counter-transformed against that stepping root ([`child_local_for`]). RENDER-ONLY: collision +
/// shove keep reading the raw root `Position`. Before any sample lands the child stays at its spawn
/// identity (== the root pose). The two queries touch disjoint entities/components (root reads
/// pose+interp; child writes Transform), so they never alias.
fn interpolate_skill_object_visuals(
    time: Res<Time>,
    roots: Query<(&Position, &Rotation, &SkillObjectInterp)>,
    mut visuals: Query<&mut Transform, With<SkillObjectVisual>>,
) {
    let render_at = time.elapsed_secs() - INTERP_DELAY_SECS;
    for (pos, rot, interp) in &roots {
        let Some((target_pos, target_rot)) = sample_pose_at(&interp.samples, render_at) else {
            continue; // no samples yet — leave the child at its spawn identity (root pose)
        };
        if let Ok(mut tf) = visuals.get_mut(interp.visual) {
            *tf = child_local_for(pos.0, rot.0, target_pos, target_rot);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(recv: f32, pos: Vec3) -> PoseSample {
        PoseSample {
            recv,
            pos,
            rot: Quat::IDENTITY,
        }
    }

    #[test]
    fn sample_lerps_at_the_exact_midpoint() {
        let buf = [s(0.0, Vec3::ZERO), s(1.0, Vec3::new(2.0, 0.0, 0.0))];
        let (p, _) = sample_pose_at(&buf, 0.5).unwrap();
        assert!((p - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-6, "got {p:?}");
    }

    #[test]
    fn sample_clamps_before_two_samples() {
        // Empty buffer → None (the caller leaves the child at identity).
        assert!(sample_pose_at(&[], 0.0).is_none());
        // A single sample clamps to itself at ANY t — never extrapolates off one point.
        let one = [s(5.0, Vec3::new(3.0, 0.0, 0.0))];
        assert_eq!(sample_pose_at(&one, 0.0).unwrap().0, Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(sample_pose_at(&one, 9.0).unwrap().0, Vec3::new(3.0, 0.0, 0.0));
    }

    #[test]
    fn sample_clamps_past_newest_never_extrapolates() {
        let buf = [s(0.0, Vec3::ZERO), s(1.0, Vec3::new(2.0, 0.0, 0.0))];
        // Well past the newest sample: HOLD the newest — do NOT run the line out to (4,0,0).
        assert_eq!(sample_pose_at(&buf, 100.0).unwrap().0, Vec3::new(2.0, 0.0, 0.0));
        // Before the oldest: hold the oldest.
        assert_eq!(sample_pose_at(&buf, -100.0).unwrap().0, Vec3::ZERO);
    }

    #[test]
    fn sample_snaps_across_a_teleport_pair() {
        // Endpoints > POSE_SNAP_DISTANCE (2 m) apart = a portal warp: must NOT glide the midpoint.
        let buf = [s(0.0, Vec3::ZERO), s(1.0, Vec3::new(10.0, 0.0, 0.0))];
        let (p, _) = sample_pose_at(&buf, 0.5).unwrap();
        assert_eq!(
            p,
            Vec3::new(10.0, 0.0, 0.0),
            "a teleport pair must snap to the destination, not lerp to (5,0,0)"
        );
    }

    #[test]
    fn sample_slerps_shortest_path() {
        // `b` is the SAME orientation as +90° yaw but the negated quaternion representation. Slerp
        // must take the SHORT way (≈ +45° at the midpoint), not the long 270° arc.
        let a = Quat::IDENTITY;
        let b = -Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let buf = [
            PoseSample { recv: 0.0, pos: Vec3::ZERO, rot: a },
            PoseSample { recv: 1.0, pos: Vec3::ZERO, rot: b },
        ];
        let (_, q) = sample_pose_at(&buf, 0.5).unwrap();
        let expected = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        assert!(
            (q * Vec3::NEG_Z).dot(expected * Vec3::NEG_Z) > 0.999,
            "slerp took the long way: {q:?}"
        );
    }

    #[test]
    fn child_local_composes_back_to_target_under_rotated_parent() {
        let parent_pos = Vec3::new(1.0, 2.0, 3.0);
        let parent_rot = Quat::from_rotation_y(0.7)
            * Quat::from_rotation_x(-0.3)
            * Quat::from_rotation_z(1.1);
        let target_pos = Vec3::new(-4.0, 5.0, 0.5);
        let target_rot = Quat::from_rotation_y(-1.2)
            * Quat::from_rotation_x(0.4)
            * Quat::from_rotation_z(0.2);
        let local = child_local_for(parent_pos, parent_rot, target_pos, target_rot);
        // Parent × child == target world pose (the counter-transform is exact).
        let world = Transform::from_translation(parent_pos)
            .with_rotation(parent_rot)
            .mul_transform(local);
        assert!(
            (world.translation - target_pos).length() < 1e-5,
            "pos {:?}",
            world.translation
        );
        assert!(
            world.rotation.dot(target_rot).abs() > 1.0 - 1e-5,
            "rot {:?}",
            world.rotation
        );
    }

    #[test]
    fn child_local_is_identity_when_parent_equals_target() {
        let p = Vec3::new(2.0, 0.0, -1.0);
        let r = Quat::from_rotation_y(0.9);
        let local = child_local_for(p, r, p, r);
        assert!(local.translation.length() < 1e-6, "{:?}", local.translation);
        assert!(local.rotation.dot(Quat::IDENTITY).abs() > 1.0 - 1e-6);
    }
}

