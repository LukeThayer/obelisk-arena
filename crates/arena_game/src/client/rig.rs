//! Player character rig: loads `character.glb`, builds an `AnimationGraph`
//! from its named clips, and attaches an `AnimationPlayer` to the rig's
//! animation-target entity so the player renders as a rigged character
//! playing the idle clip (not a T-pose).
//!
//! Adaptation of wisp's `src/player/visuals.rs` (same Bevy 0.18): the
//! load-graph + attach + play-idle path PLUS the per-rig locomotion+casting
//! blend ([`drive_animation`]). Costume/part selection lives in `client/parts.rs`.

use avian3d::prelude::{LinearVelocity, Rotation};
use bevy::{gltf::Gltf, prelude::*};

use super::net::{ChargeState, LocalNetPlayer};
use crate::net::protocol::NetworkedCastState;

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
    /// EVERY named clip's graph node (superset of the named fields below) — the lookup the
    /// cue-anim overlay resolves designer-authored clip names against.
    pub(crate) named: std::collections::HashMap<String, AnimationNodeIndex>,
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
    // EVERY named clip gets a node (the cue-anim overlay can play any of them); the fixed
    // locomotion/casting fields below just alias into this map.
    let mut named = std::collections::HashMap::new();
    for (name, clip) in gltf.named_animations.iter() {
        named.insert(name.to_string(), graph.add_clip(clip.clone(), 1.0, root));
    }
    let add = |name: &str| -> Option<AnimationNodeIndex> { named.get(name).copied() };

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
    rig.named = named;

    if rig.idle.is_none() {
        warn!("character.glb is missing animation \"{IDLE_CLIP}\"");
        return;
    }

    rig.graph = Some(graphs.add(graph));
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
        for node in rig.named.values().copied() {
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
/// casting variants instead of popping. Inserted on the player root by
/// `present::attach_rig_to_players`; read+written by [`drive_animation`].
#[derive(Component, Default)]
pub struct LocalAnimBlend {
    /// 0 = plain locomotion, 1 = full casting layer. Eased toward the
    /// phase-driven target each frame by [`step_casting_blend`].
    pub casting: f32,
    /// The overlay node that held the casting layer LAST frame (if any) — animation-player
    /// weights persist, so when the overlay changes/ends its old node must be re-zeroed or the
    /// stale pose keeps blending in.
    pub(crate) last_override: Option<AnimationNodeIndex>,
}

/// A cue-authored animation override for the CASTING layer of one player's rig: while present
/// (and its clip resolves in the rig's named-clip map), the casting layer plays THIS clip
/// instead of the default `casting_idle`/`casting_walk_*` set — the designer's
/// `CueBinding.anim` made real. Inserted by the charge-cue driver (looping, removed on release)
/// and by the cue cosmetics layer for one-shot `on_cast` anims (expires at `until`).
#[derive(Component, Debug, Clone)]
pub struct CueAnimOverlay {
    /// Clip name. Editor-authored names may be library-qualified (`"character::casting_idle"`)
    /// — resolution takes the last `::` segment against the glb's named clips.
    pub clip: String,
    /// `None` = hold until the component is removed (charge loops). `Some(t)` = expire once
    /// `Time::elapsed_secs()` passes `t` (one-shot cast anims) — `expire_cue_anim_overlays`.
    pub until: Option<f32>,
    /// `true` = loop the clip while the overlay holds; `false` = play it once.
    pub looping: bool,
}

/// Resolve an authored clip name (possibly `lib::clip`-qualified) against the rig's named-clip
/// nodes.
pub(crate) fn resolve_overlay_node(rig: &RigAssets, clip: &str) -> Option<AnimationNodeIndex> {
    let short = clip.rsplit("::").next().unwrap_or(clip);
    rig.named.get(short).copied()
}

/// Drop expired one-shot overlays (`until` passed). Looping overlays (`until: None`) are
/// removed by whoever inserted them (the charge-cue driver, on release).
pub fn expire_cue_anim_overlays(
    time: Res<Time>,
    q: Query<(Entity, &CueAnimOverlay)>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    for (e, overlay) in &q {
        if overlay.until.is_some_and(|t| now >= t) {
            commands.entity(e).remove::<CueAnimOverlay>();
        }
    }
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

/// Compute per-clip blend weights from world velocity + the eased casting factor,
/// pushing each `(node, weight)` pair into `sink` instead of allocating a `Vec` per
/// call. [`drive_animation`] passes a closure that `play`s the clip + sets its weight.
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
    cast_override: Option<AnimationNodeIndex>,
    sink: &mut impl FnMut(AnimationNodeIndex, f32),
) {
    let mut emit = |node: Option<AnimationNodeIndex>, w: f32| {
        if let Some(n) = node {
            sink(n, w);
        }
    };

    let casting_blend = casting_blend.clamp(0.0, 1.0);
    let cast_factor = casting_blend;
    let plain_factor = 1.0 - casting_blend;

    // A cue-authored overlay clip REPLACES the whole casting side (one clip, no directional
    // variants — the charge pose / authored cast anim).
    let cast_idle = cast_override.or(rig.cast_idle);
    let overriding = cast_override.is_some();

    let planar = Vec3::new(world_velocity.x, 0.0, world_velocity.z);
    let speed = planar.length();

    if speed < WALK_MIN_SPEED {
        // Standing: all weight on idle / casting_idle.
        emit(rig.idle, plain_factor);
        emit(cast_idle, cast_factor);
        return;
    }

    let locomotion = (speed / LOCOMOTION_REF_SPEED).clamp(0.0, 1.0);
    let idle_share = if overriding { 1.0 } else { 1.0 - locomotion };
    emit(rig.idle, (1.0 - locomotion) * plain_factor);
    emit(cast_idle, idle_share * cast_factor);

    // World velocity → local frame (character forward = -Z in the body's frame).
    let local = (Quat::from_axis_angle(Vec3::Y, -yaw) * planar) / speed;
    let forward = -local.z;
    let right = local.x;
    let f_w = locomotion * forward.max(0.0);
    let b_w = locomotion * (-forward).max(0.0);
    let r_w = locomotion * right.max(0.0);
    let l_w = locomotion * (-right).max(0.0);

    emit(rig.walk_f, f_w * plain_factor);
    emit(rig.walk_b, b_w * plain_factor);
    emit(rig.walk_r, r_w * plain_factor);
    emit(rig.walk_l, l_w * plain_factor);
    emit(rig.cast_walk_f, f_w * cast_factor);
    emit(rig.cast_walk_b, b_w * cast_factor);
    emit(rig.cast_walk_r, r_w * cast_factor);
    emit(rig.cast_walk_l, l_w * cast_factor);
}

/// Per-frame animation driver for the player rigs (the `drive_animation` shape from
/// wisp's `visuals.rs:548+`, adapted for obelisk + the NETWORKED arena client).
///
/// This is PER-PLAYER: the networked client renders TWO rigs (local + remote), each a descendant of
/// its own `NetworkedPlayer` root carrying a [`LocalAnimBlend`]. For each `AnimationPlayer` we walk
/// the `ChildOf` chain up to its owning rig root, read that root's persisted blend + replicated cast
/// state + pose, and drive that rig — so both characters animate independently instead of
/// `single_mut()` erroring on the 2-player case.
///
/// Drives two layers:
///   - **Locomotion**: the hidden LOCAL rig uses camera yaw + zero velocity; each REMOTE rig faces
///     and walks from ITS OWN interpolated avian `Rotation` (yaw) + `LinearVelocity`, so a moving
///     opponent plays the correct directional walk clip facing the right way.
///   - **Casting** from the root's `NetworkedCastState.cast_phase`: 1/2 → 1.0, 3 → 0.5, 0 → 0.0,
///     eased by [`step_casting_blend`]. For the LOCAL player only, a charge hold also drives the
///     blend to 1.0 so the caster visibly winds up while charging — the server-side cast (hence
///     `cast_phase`) only starts on release, so this pre-emptively cues the pose during the hold.
#[allow(clippy::type_complexity)]
pub fn drive_animation(
    rig: Res<RigAssets>,
    mut anim: Query<(Entity, &mut AnimationPlayer)>,
    parents: Query<&ChildOf>,
    body_marker: Query<(), With<ArenaBody>>,
    mut roots: Query<(
        Option<&NetworkedCastState>,
        Option<&LinearVelocity>,
        Option<&Rotation>,
        &mut LocalAnimBlend,
        Has<LocalNetPlayer>,
        Option<&CueAnimOverlay>,
        Option<&lightyear::prelude::input::native::ActionState<crate::net::input::ArenaInput>>,
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
        let Ok((cast_state, lin_vel, rotation, mut blend, is_local, overlay, action)) =
            roots.get_mut(root)
        else {
            continue;
        };
        let cast_phase = cast_state.map(|c| c.cast_phase).unwrap_or(0);

        // Map the cast state to a casting-layer blend target (guide §4). Both local + remote read
        // the replicated `NetworkedCastState.cast_phase` the server stamps (1 windup / 2 active →
        // 1.0, 3 recovery → 0.5, 0 none → 0.0 — this is what makes A's cast animate on B's screen).
        // A charge hold ALSO drives the blend to 1.0 as a pre-release wind-up on BOTH peers —
        // the local player from its own `ChargeState`, the REMOTE from its predicted
        // `ActionState.charging` (the input-stream charging telegraph: you see the enemy wind up
        // while they hold).
        let phase_target = match cast_phase {
            1 | 2 => 1.0,
            3 => 0.5,
            _ => 0.0,
        };
        let charging_here = if is_local {
            charge.charging
        } else {
            action.map(|a| a.0.charging).unwrap_or(false)
        };
        let casting_target = if charging_here { 1.0 } else { phase_target };
        blend.casting = step_casting_blend(blend.casting, casting_target);

        // Cue-authored casting-layer override (charge pose / authored cast anim). Weights
        // persist on the player, so a changed/ended override's old node is re-zeroed.
        let override_node = overlay.and_then(|o| resolve_overlay_node(&rig, &o.clip));
        let override_loops = overlay.map(|o| o.looping).unwrap_or(true);
        if blend.last_override != override_node {
            if let Some(old) = blend.last_override {
                player.play(old).set_weight(0.0);
            }
            blend.last_override = override_node;
        }

        // Per-rig locomotion (Bug 2): the LOCAL player uses the camera yaw + zero velocity (it's
        // first-person/hidden, so its walk clip is never seen). Each REMOTE rig uses ITS OWN yaw
        // (the interpolated avian `Rotation`) and ITS OWN planar velocity (the replicated/interpolated
        // `LinearVelocity`) so a moving opponent plays the correct directional walk clip facing the
        // right way instead of sliding while idle.
        let (rig_velocity, rig_yaw) = if is_local {
            (Vec3::ZERO, body_yaw)
        } else {
            let vel = lin_vel.map(|v| v.0).unwrap_or(Vec3::ZERO);
            let yaw = rotation.map(|r| yaw_of(r.0)).unwrap_or(0.0);
            (vel, yaw)
        };
        locomotion_blend(
            &rig,
            rig_velocity,
            rig_yaw,
            blend.casting,
            override_node,
            &mut |node, weight| {
                let anim = player.play(node);
                // One-shot overrides play through once; everything else loops.
                if override_node != Some(node) || override_loops {
                    anim.repeat();
                }
                anim.set_weight(weight);
            },
        );
    }
}

/// Extract the Y-axis rotation (yaw) from a quaternion (the body only rotates around Y).
fn yaw_of(q: Quat) -> f32 {
    q.to_euler(EulerRot::YXZ).0
}

/// Walk the `ChildOf` chain from an `AnimationPlayer` entity up to the nearest ancestor that is BOTH
/// past an [`ArenaBody`] marker (so it's an arena rig) AND carries a [`LocalAnimBlend`] (the
/// `NetworkedPlayer` root). Returns that root entity, or `None` if the anim player isn't under an
/// arena rig. Lets [`drive_animation`] drive each of the N rigs from its own root state.
#[allow(clippy::type_complexity)]
fn rig_root_of(
    anim_entity: Entity,
    parents: &Query<&ChildOf>,
    body_marker: &Query<(), With<ArenaBody>>,
    roots: &Query<(
        Option<&NetworkedCastState>,
        Option<&LinearVelocity>,
        Option<&Rotation>,
        &mut LocalAnimBlend,
        Has<LocalNetPlayer>,
        Option<&CueAnimOverlay>,
        Option<&lightyear::prelude::input::native::ActionState<crate::net::input::ArenaInput>>,
    )>,
) -> Option<Entity> {
    // Single upward pass: return the nearest ancestor carrying a `LocalAnimBlend` (the player root,
    // parent of the `ArenaBody` scene — `roots` contains exactly the player roots), but only after
    // crossing the [`ArenaBody`] marker, so an anim player not under an arena rig still yields `None`.
    // The `ArenaBody` always sits below the root, so `crossed_body` is set before the root is reached.
    let mut cur = anim_entity;
    let mut crossed_body = false;
    loop {
        if body_marker.contains(cur) {
            crossed_body = true;
        }
        if crossed_body && roots.get(cur).is_ok() {
            return Some(cur);
        }
        match parents.get(cur) {
            Ok(p) => cur = p.0,
            Err(_) => return None,
        }
    }
}
