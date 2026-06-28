//! Player character rig: loads `character.glb`, builds an `AnimationGraph`
//! from its named clips, and attaches an `AnimationPlayer` to the rig's
//! animation-target entity so the player renders as a rigged character
//! playing the idle clip (not a T-pose).
//!
//! Trimmed adaptation of wisp's `src/player/visuals.rs` (same Bevy 0.18):
//! kept just the load-graph + attach + play-idle path. Dropped wisp's
//! costume/recolor/viewmodel/render-layer/locomotion-blend logic — those
//! arrive with the third-person controller in a later task.

use bevy::{gltf::Gltf, prelude::*};
use obelisk_bevy::prelude::{ActiveCast, SkillPhase};

use super::net::{ChargeState, LocalNetPlayer};

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
        Self { gltf, ..default() }
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

/// Walk the `ChildOf` parent chain to confirm an entity belongs to an
/// [`ArenaBody`] rig. Used by [`drive_animation`] via [`rig_root_of`].
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

/// Per-frame animation driver for the player rigs (the `drive_animation` shape from
/// wisp's `visuals.rs:548+`, adapted for obelisk + the NETWORKED arena client).
///
/// M2.5 Task 21 made this PER-PLAYER (was single-player co-located): the networked client renders
/// TWO rigs (local + remote), each a descendant of its own `NetworkedPlayer` root carrying a
/// [`LocalAnimBlend`]. For each `AnimationPlayer` we walk the `ChildOf` chain up to its owning rig
/// root, read that root's persisted blend + optional `ActiveCast`, and drive that rig — so both
/// characters animate independently instead of `single_mut()` erroring on the 2-player case.
///
/// Drives two layers:
///   - **Locomotion** from the camera yaw (the per-player world velocity is not yet threaded for
///     remote rigs — they idle/face via the interpolated `NetworkedPosition.yaw`; deriving remote
///     walk velocity from the pose-stream delta is later polish, the rig idles correctly meanwhile).
///   - **Casting** from the root's `Option<&ActiveCast>`: `Windup`/`Active` → 1.0, `Recovery` → 0.5,
///     none/`Done` → 0.0, eased by [`step_casting_blend`]. For the LOCAL player only, a charge hold
///     (C5) also drives the blend to 1.0 so the caster visibly winds up while charging — the actual
///     `ActiveCast` only starts on release, so this pre-emptively cues the pose during the hold.
#[allow(clippy::type_complexity)]
pub fn drive_animation(
    rig: Res<RigAssets>,
    mut anim: Query<(Entity, &mut AnimationPlayer)>,
    parents: Query<&ChildOf>,
    body_marker: Query<(), With<ArenaBody>>,
    mut roots: Query<(
        Option<&ActiveCast>,
        &mut LocalAnimBlend,
        Has<LocalNetPlayer>,
    )>,
    yaw: Res<super::controller::CameraYaw>,
    charge: Res<ChargeState>,
) {
    if !rig.ready() {
        return;
    }
    let body_yaw = yaw.0;

    for (anim_entity, mut player) in &mut anim {
        // Resolve THIS anim player's owning rig root: the nearest ancestor carrying a
        // `LocalAnimBlend` (the `NetworkedPlayer` root). Skip anim players not under an arena rig.
        let Some(root) = rig_root_of(anim_entity, &parents, &body_marker, &roots) else {
            continue;
        };
        let Ok((active_cast, mut blend, is_local)) = roots.get_mut(root) else {
            continue;
        };

        // Map the obelisk cast phase to a casting-layer blend target (guide §4).
        // C5: While the LOCAL player is charging (holding the cast button), drive the blend to
        // 1.0 as a wind-up so the caster visibly prepares the spell. The `ActiveCast` only
        // starts on release; this pre-emptively cues the casting pose during the hold phase.
        // Remote players are unaffected (charging is a local input state).
        let casting_target = if is_local && charge.charging {
            1.0 // wind-up: hold drives the casting pose
        } else {
            match active_cast.map(|c| c.phase) {
                Some(SkillPhase::Windup) | Some(SkillPhase::Active) => 1.0,
                Some(SkillPhase::Recovery) => 0.5,
                _ => 0.0,
            }
        };
        blend.casting = step_casting_blend(blend.casting, casting_target);

        // No per-rig world velocity threaded yet → idle/casting_idle (Vec3::ZERO speed).
        for (node, weight) in locomotion_blend(&rig, Vec3::ZERO, body_yaw, blend.casting) {
            player.play(node).repeat().set_weight(weight);
        }
    }
}

/// Walk the `ChildOf` chain from an `AnimationPlayer` entity up to the nearest ancestor that is BOTH
/// past an [`ArenaBody`] marker (so it's an arena rig) AND carries a [`LocalAnimBlend`] (the
/// `NetworkedPlayer` root). Returns that root entity, or `None` if the anim player isn't under an
/// arena rig. Lets [`drive_animation`] drive each of the N rigs from its own root state.
fn rig_root_of(
    anim_entity: Entity,
    parents: &Query<&ChildOf>,
    body_marker: &Query<(), With<ArenaBody>>,
    roots: &Query<(
        Option<&ActiveCast>,
        &mut LocalAnimBlend,
        Has<LocalNetPlayer>,
    )>,
) -> Option<Entity> {
    // First confirm it belongs to an arena rig at all.
    if !ancestor_has_body_marker(anim_entity, parents, body_marker) {
        return None;
    }
    // Then walk up to the nearest ancestor carrying a LocalAnimBlend (the player root, parent of the
    // ArenaBody scene). `roots` contains exactly the player roots.
    let mut cur = anim_entity;
    loop {
        if roots.get(cur).is_ok() {
            return Some(cur);
        }
        match parents.get(cur) {
            Ok(p) => cur = p.0,
            Err(_) => return None,
        }
    }
}
