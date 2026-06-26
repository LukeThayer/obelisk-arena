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
        // Play the idle clip on a loop at full weight.
        player.play(idle).repeat().set_weight(1.0);
        commands
            .entity(entity)
            .insert(AnimationGraphHandle(graph.clone()));
    }
}
