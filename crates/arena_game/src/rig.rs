//! Player character rig: loads `character.glb`, builds an `AnimationGraph`
//! from its named clips, and attaches an `AnimationPlayer` to the rig's
//! animation-target entity so the player renders as a rigged character
//! playing the idle clip (not a T-pose).
//!
//! Trimmed adaptation of wisp's `src/player/visuals.rs` (same Bevy 0.18):
//! kept just the load-graph + attach + play-idle path. Dropped wisp's
//! costume/recolor/viewmodel/render-layer/locomotion-blend logic — those
//! arrive with the third-person controller in a later task.

use bevy::mesh::skinning::SkinnedMesh;
use bevy::{gltf::Gltf, prelude::*};
use obelisk_bevy::prelude::{ActiveCast, SkillPhase};

/// Animation clip names baked into `character.glb`. Verified against the
/// glb's `gltf.named_animations` keys: `idle`, `walk_forward`,
/// `walk_backward`, `walk_left`, `walk_right`, `falling`, plus the
/// `casting_*` variants. Only `idle` is wired-to-play for now; the rest
/// are loaded into the graph so the third-person controller can blend
/// them later without rebuilding the graph.
const IDLE_CLIP: &str = "idle";
const WALK_F_CLIP: &str = "walk_forward";
const WALK_B_CLIP: &str = "walk_backward";
const WALK_L_CLIP: &str = "walk_left";
const WALK_R_CLIP: &str = "walk_right";
const FALL_CLIP: &str = "falling";
/// Casting variants — the spell-cast pose overlaid (blended) on top of the
/// matching locomotion clip when the player has an `ActiveCast`. Copied from
/// wisp's `visuals.rs:77-81`. `casting_idle` is the load-bearing one (the
/// standing cast pose); the `casting_walk_*` set is loaded so a cast issued
/// while moving keeps the directional read, and is silently skipped if absent.
const CAST_IDLE_CLIP: &str = "casting_idle";
const CAST_WALK_F_CLIP: &str = "casting_walk_forward";
const CAST_WALK_B_CLIP: &str = "casting_walk_backward";
const CAST_WALK_L_CLIP: &str = "casting_walk_left";
const CAST_WALK_R_CLIP: &str = "casting_walk_right";

/// Walk speed at which the locomotion clips play at their authored cadence
/// (weight ramps to 1). Below this the blend eases toward idle. Mirrors wisp's
/// `LOCOMOTION_REF_SPEED` (3.5), ~80% of the controller's `MOVE_SPEED` (4.0).
const LOCOMOTION_REF_SPEED: f32 = 3.5;
/// Below this planar speed the rig is treated as standing (idle / casting_idle
/// only). Mirrors wisp's `WALK_MIN_SPEED`.
const WALK_MIN_SPEED: f32 = 0.2;

/// Marker for the player's rigged body scene root (the `SceneRoot` of
/// `character.glb`). Renamed from wisp's `LocalWizardBody`. Spawned as a
/// child of the player combatant entity.
#[derive(Component)]
pub struct ArenaBody;

/// Holds the loaded character `Gltf`, the built `AnimationGraph` handle,
/// and the `AnimationNodeIndex` per named clip. Renamed from wisp's
/// `WizardAssets`. `build_graph_when_loaded` fills `graph` + the node
/// indices once the gltf finishes loading; `attach_animation_graph`
/// reads them to wire up the spawned `AnimationPlayer`.
#[derive(Resource, Default)]
pub struct RigAssets {
    gltf: Handle<Gltf>,
    pub(crate) graph: Option<Handle<AnimationGraph>>,
    pub(crate) idle: Option<AnimationNodeIndex>,
    pub(crate) walk_f: Option<AnimationNodeIndex>,
    pub(crate) walk_b: Option<AnimationNodeIndex>,
    pub(crate) walk_l: Option<AnimationNodeIndex>,
    pub(crate) walk_r: Option<AnimationNodeIndex>,
    pub(crate) falling: Option<AnimationNodeIndex>,
    pub(crate) cast_idle: Option<AnimationNodeIndex>,
    pub(crate) cast_walk_f: Option<AnimationNodeIndex>,
    pub(crate) cast_walk_b: Option<AnimationNodeIndex>,
    pub(crate) cast_walk_l: Option<AnimationNodeIndex>,
    pub(crate) cast_walk_r: Option<AnimationNodeIndex>,
}

impl RigAssets {
    /// Construct from a `character.glb` handle (`AssetServer::load`).
    pub fn new(gltf: Handle<Gltf>) -> Self {
        Self {
            gltf,
            ..default()
        }
    }

    /// The graph is built and the idle clip resolved.
    pub(crate) fn ready(&self) -> bool {
        self.graph.is_some() && self.idle.is_some()
    }
}

/// Once the character `Gltf` finishes loading, build an `AnimationGraph`
/// from its named clips and store the per-clip node indices on
/// `RigAssets`. Idempotent: returns early once `ready()`.
pub fn build_graph_when_loaded(
    mut rig: ResMut<RigAssets>,
    gltfs: Res<Assets<Gltf>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    if rig.ready() {
        return;
    }
    let Some(gltf) = gltfs.get(&rig.gltf) else {
        return;
    };

    // Log the available clip names once so a wrong constant is obvious in
    // the log rather than a silent missing-animation T-pose.
    let mut keys: Vec<&str> = gltf.named_animations.keys().map(|k| k.as_ref()).collect();
    keys.sort();
    info!("rig: character.glb named_animations = {:?}", keys);

    let mut graph = AnimationGraph::new();
    let root = graph.root;
    let mut add = |name: &str| -> Option<AnimationNodeIndex> {
        gltf.named_animations
            .get(name)
            .map(|clip| graph.add_clip(clip.clone(), 1.0, root))
    };

    rig.idle = add(IDLE_CLIP);
    rig.walk_f = add(WALK_F_CLIP);
    rig.walk_b = add(WALK_B_CLIP);
    rig.walk_l = add(WALK_L_CLIP);
    rig.walk_r = add(WALK_R_CLIP);
    rig.falling = add(FALL_CLIP);
    rig.cast_idle = add(CAST_IDLE_CLIP);
    rig.cast_walk_f = add(CAST_WALK_F_CLIP);
    rig.cast_walk_b = add(CAST_WALK_B_CLIP);
    rig.cast_walk_l = add(CAST_WALK_L_CLIP);
    rig.cast_walk_r = add(CAST_WALK_R_CLIP);

    if rig.idle.is_none() {
        warn!("character.glb is missing animation \"{IDLE_CLIP}\"");
        return;
    }

    rig.graph = Some(graphs.add(graph));
}

/// The single coherent outfit we keep visible: the **Witch** set, which reads
/// as a wizard (the M0/M1 fantasy). One top + one bottom + the matching hat,
/// plus one hair variant and the transform-correct `*0` face features. Every
/// other mesh in `character.glb` (the eight other class outfits, all weapons,
/// capes, body skin, and the dozens of extra hair/face variants) is hidden.
///
/// Mesh node-names verified against `character.glb` (they match wisp's
/// `parts.rs` tables exactly). A minimal allowlist — not a loadout system; the
/// full slot-based customizer (`PartSelection`) lands later.
const KEEP_MESHES: &[&str] = &[
    "F_Witch_Top",
    "F_Witch_Bottom",
    "F_Witch_Headwear",
    "F_hair_1",
    "F_eyes0",
    "F_eyebrows0",
    "F_mouth0",
];

/// Marker placed on every rig mesh once we've decided its costume visibility,
/// so we don't re-evaluate it every frame.
#[derive(Component)]
pub(crate) struct CostumeCulled;

/// Whether a rig mesh node-name belongs to the single kept (Witch) outfit.
/// Default-hide: anything not in [`KEEP_MESHES`] is culled. Meshes inside the
/// glTF that aren't outfit nodes at all (the head/eyes mesh containers named
/// `F_Head`, `Mesh.NNN`, etc.) are resolved to their outfit node-name by the
/// caller before this is asked; an unrecognized name falls through to hidden,
/// which is the safe default for a "show ONE outfit" cull.
fn keep_mesh(name: &str) -> bool {
    KEEP_MESHES.contains(&name)
}

/// Costume-cull: once the rig scene spawns, hide every mesh that isn't part of
/// the single kept (Witch) outfit so the unified `character.glb` renders as ONE
/// readable character instead of all nine class outfits stacked at once.
///
/// Walks newly-spawned skinned meshes under an [`ArenaBody`], resolves each to
/// its outfit node-name (the `Name` on the mesh entity or its nearest named
/// ancestor — glTF nests the visible mesh under a node carrying the `F_*` name),
/// and sets `Visibility::Hidden` on anything not in [`KEEP_MESHES`]. Each mesh
/// is stamped [`CostumeCulled`] so it's processed exactly once.
pub fn cull_costume(
    mut commands: Commands,
    pending: Query<(Entity, Option<&Name>), (With<SkinnedMesh>, Without<CostumeCulled>)>,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    body_marker: Query<(), With<ArenaBody>>,
    mut visibility: Query<&mut Visibility>,
) {
    for (entity, name) in &pending {
        if !ancestor_has_body_marker(entity, &parents, &body_marker) {
            continue;
        }
        // Resolve the outfit node-name: prefer the mesh entity's own `Name` (if
        // it's not an auto-generated "Mesh.NNN"), else walk up to the nearest
        // named ancestor. glTF imports the visible primitive under a node that
        // carries the authored `F_*` name.
        let own = name
            .map(|n| n.as_str().to_string())
            .filter(|s| !s.starts_with("Mesh"));
        let resolved = own
            .or_else(|| {
                let mut cur = entity;
                loop {
                    match parents.get(cur) {
                        Ok(p) => {
                            cur = p.0;
                            if let Ok(n) = names.get(cur) {
                                let s = n.as_str();
                                if !s.starts_with("Mesh") {
                                    break Some(s.to_string());
                                }
                            }
                        }
                        Err(_) => break None,
                    }
                }
            })
            .unwrap_or_default();

        if let Ok(mut v) = visibility.get_mut(entity) {
            *v = if keep_mesh(&resolved) {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        }
        commands.entity(entity).insert(CostumeCulled);
    }
}

/// Walk the `ChildOf` parent chain to confirm a mesh belongs to an
/// [`ArenaBody`] before culling it. Mirrors the controller's
/// `ancestor_has_body_marker`.
fn ancestor_has_body_marker(
    entity: Entity,
    parents: &Query<&ChildOf>,
    marker: &Query<(), With<ArenaBody>>,
) -> bool {
    let mut cur = entity;
    loop {
        if marker.contains(cur) {
            return true;
        }
        match parents.get(cur) {
            Ok(p) => cur = p.0,
            Err(_) => return false,
        }
    }
}

/// The glTF loader spawns an `AnimationPlayer` entity inside the scene
/// tree once the `SceneRoot` resolves. Attach our graph + an
/// `AnimationPlayer` playing the idle clip when that entity appears.
///
/// Seeds idle at full weight so the character isn't a T-pose for the
/// single frame before any blend driver runs.
pub fn attach_animation_graph(
    mut commands: Commands,
    rig: Res<RigAssets>,
    pending: Query<Entity, (With<AnimationPlayer>, Without<AnimationGraphHandle>)>,
    mut players: Query<&mut AnimationPlayer>,
) {
    if !rig.ready() {
        return;
    }
    let Some(graph) = rig.graph.clone() else {
        return;
    };
    let Some(idle) = rig.idle else {
        return;
    };
    for entity in &pending {
        let Ok(mut player) = players.get_mut(entity) else {
            continue;
        };
        // Start every clip looping muted-at-rest so the per-frame `drive_animation`
        // blend can set weights without first having to `play` each one. `play` is
        // idempotent (entry-or-default), so every clip is started exactly once here.
        for node in [
            rig.idle,
            rig.walk_f,
            rig.walk_b,
            rig.walk_l,
            rig.walk_r,
            rig.falling,
            rig.cast_idle,
            rig.cast_walk_f,
            rig.cast_walk_b,
            rig.cast_walk_l,
            rig.cast_walk_r,
        ]
        .into_iter()
        .flatten()
        {
            player.play(node).repeat().set_weight(0.0);
        }
        // Seed idle at full weight so the character isn't a T-pose for the single
        // frame before any blend driver runs.
        player.play(idle).set_weight(1.0);
        commands
            .entity(entity)
            .insert(AnimationGraphHandle(graph.clone()));
    }
}

/// Smoothly-eased casting factor for the player rig, persisted across frames so the
/// cast wind-up/recovery cross-fades between the plain locomotion clips and the
/// casting variants instead of popping. Inserted on the player combatant root in
/// `spawn_combatants`; read+written by [`drive_animation`].
#[derive(Component, Default)]
pub struct LocalAnimBlend {
    /// 0 = plain locomotion, 1 = full casting layer. Eased toward the
    /// phase-driven target each frame by [`step_casting_blend`].
    pub casting: f32,
}

/// Per-frame exponential follow toward the target casting blend. `ALPHA` 0.2
/// reaches ~95% in ~14 frames (~230ms at 60Hz) — fast enough to read with the
/// cast but slow enough that the cross-fade doesn't pop. Copied from wisp's
/// `step_casting_blend` (`visuals.rs:521`), generalized to follow a continuous
/// target (not just a 0/1 bool) so `SkillPhase::Recovery` can ease toward 0.5.
fn step_casting_blend(current: f32, target: f32) -> f32 {
    const ALPHA: f32 = 0.2;
    current + (target - current) * ALPHA
}

/// Compute per-clip blend weights from world velocity + the eased casting factor.
///
/// `casting_blend` (0..1) cross-fades the locomotion side between the plain clips
/// (idle / walk_*) and the casting variants (casting_idle / casting_walk_*):
/// 0 = plain, 1 = casting. A missing casting variant silently drops that direction's
/// casting weight (the plain clip stays visible), so the rig degrades gracefully if
/// only `casting_idle` is authored. Adapted from wisp's `locomotion_blend`
/// (`visuals.rs:438+`), dropping the airborne/`falling` term (the arena is grounded).
fn locomotion_blend(
    rig: &RigAssets,
    world_velocity: Vec3,
    yaw: f32,
    casting_blend: f32,
) -> Vec<(AnimationNodeIndex, f32)> {
    let mut out: Vec<(AnimationNodeIndex, f32)> = Vec::with_capacity(10);
    let push = |out: &mut Vec<_>, node: Option<AnimationNodeIndex>, w: f32| {
        if let Some(n) = node {
            out.push((n, w));
        }
    };

    let casting_blend = casting_blend.clamp(0.0, 1.0);
    let cast_factor = casting_blend;
    let plain_factor = 1.0 - casting_blend;

    let planar = Vec3::new(world_velocity.x, 0.0, world_velocity.z);
    let speed = planar.length();

    if speed < WALK_MIN_SPEED {
        // Standing: all weight on idle / casting_idle.
        push(&mut out, rig.idle, plain_factor);
        push(&mut out, rig.cast_idle, cast_factor);
        return out;
    }

    let locomotion = (speed / LOCOMOTION_REF_SPEED).clamp(0.0, 1.0);
    let idle_share = 1.0 - locomotion;
    push(&mut out, rig.idle, idle_share * plain_factor);
    push(&mut out, rig.cast_idle, idle_share * cast_factor);

    // World velocity → local frame (character forward = -Z in the body's frame).
    let local = (Quat::from_axis_angle(Vec3::Y, -yaw) * planar) / speed;
    let forward = -local.z;
    let right = local.x;
    let f_w = locomotion * forward.max(0.0);
    let b_w = locomotion * (-forward).max(0.0);
    let r_w = locomotion * right.max(0.0);
    let l_w = locomotion * (-right).max(0.0);

    push(&mut out, rig.walk_f, f_w * plain_factor);
    push(&mut out, rig.walk_b, b_w * plain_factor);
    push(&mut out, rig.walk_r, r_w * plain_factor);
    push(&mut out, rig.walk_l, l_w * plain_factor);
    push(&mut out, rig.cast_walk_f, f_w * cast_factor);
    push(&mut out, rig.cast_walk_b, b_w * cast_factor);
    push(&mut out, rig.cast_walk_r, r_w * cast_factor);
    push(&mut out, rig.cast_walk_l, l_w * cast_factor);
    out
}

/// Per-frame animation driver for the player rig (the `drive_animation` shape from
/// wisp's `visuals.rs:548+`, adapted for obelisk + the arena controller).
///
/// Drives two layers:
///   - **Locomotion** from [`PlayerVelocity`] (Task 11) + the camera yaw, blending
///     idle / directional-walk clips.
///   - **Casting** from the player's `Option<&ActiveCast>` (obelisk's cast state
///     machine): the `ActiveCast.phase` maps to a casting blend target —
///     `Windup`/`Active` → 1.0, `Recovery` → 0.5, none/`Done` → 0.0 — eased by
///     [`step_casting_blend`] so the pose doesn't pop. The casting clips are
///     overlaid on the locomotion layer underneath.
///
/// `world_velocity` is in world space and `yaw` is the camera/body yaw, so the
/// directional-walk split reads the same regardless of which way the camera faces.
/// The `AnimationPlayer` lives on an entity DEEP inside the rig scene tree, so we
/// walk the `ChildOf` chain up to the `PlayerController` root to find the player's
/// `ActiveCast` + `LocalAnimBlend`.
pub fn drive_animation(
    rig: Res<RigAssets>,
    mut anim: Query<(Entity, &mut AnimationPlayer)>,
    parents: Query<&ChildOf>,
    body_marker: Query<(), With<ArenaBody>>,
    mut player_state: Query<(Option<&ActiveCast>, &mut LocalAnimBlend)>,
    velocity: Res<crate::controller::PlayerVelocity>,
    yaw: Res<crate::controller::CameraYaw>,
) {
    if !rig.ready() {
        return;
    }

    // Find the player's cast phase + the persisted blend state. The
    // `LocalAnimBlend` + `ActiveCast` live on the `PlayerController` root; there is
    // exactly one player, so take the single matching entity.
    let Ok((active_cast, mut blend)) = player_state.single_mut() else {
        return;
    };

    // Map the obelisk cast phase to a casting-layer blend target (guide §4).
    let casting_target = match active_cast.map(|c| c.phase) {
        Some(SkillPhase::Windup) => 1.0,
        Some(SkillPhase::Active) => 1.0,
        Some(SkillPhase::Recovery) => 0.5,
        // No cast, or it has finished (`Done`) → fade the casting layer out.
        _ => 0.0,
    };
    blend.casting = step_casting_blend(blend.casting, casting_target);

    let world_velocity = velocity.0;
    let body_yaw = yaw.0;

    for (anim_entity, mut player) in &mut anim {
        // Only drive `AnimationPlayer`s that belong to the player's rig (a
        // descendant of an `ArenaBody`); ignore any other rigs.
        if !ancestor_has_body_marker(anim_entity, &parents, &body_marker) {
            continue;
        }
        for (node, weight) in locomotion_blend(&rig, world_velocity, body_yaw, blend.casting) {
            player.play(node).repeat().set_weight(weight);
        }
    }
}
