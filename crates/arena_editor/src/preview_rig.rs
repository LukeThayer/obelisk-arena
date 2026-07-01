//! Preview character rig for the Skill designer: loads `character.glb` (registered as a GLTF library
//! via `register_gltf_library` in `main.rs`, so its clips land in `bevy_editor_game`'s
//! `AnimationLibrary` keyed `"character::<clip>"`), builds one `AnimationGraph` from the preview
//! clips, and attaches it to the `AnimationPlayer` the scene loader spawns under the rig.
//!
//! Adapted from `arena_game/src/client/rig.rs` (same Bevy 0.18 load-graph → attach → play path), but
//! the clip source is the editor's `AnimationLibrary` (populated once the GLTF library indexes) rather
//! than a per-entity `RigAssets.gltf` handle, and the rig hangs under the preview caster spawned by
//! `arena_sim`. The per-frame locomotion/casting blend is NOT ported here — the preview cosmetics
//! observer (Task 31) drives clip weights on cue.

use arena_sim::preview::PreviewCaster;
use bevy::prelude::*;
use bevy_editor_game::{AnimationLibrary, GameStartedEvent};
use std::collections::HashMap;

/// The clips the preview graph pulls from `character.glb`: the locomotion set + their casting
/// variants. Matches `arena_game/src/client/rig.rs`'s clip constants.
pub const PREVIEW_CLIPS: [&str; 11] = [
    "idle",
    "walk_forward",
    "walk_backward",
    "walk_left",
    "walk_right",
    "falling",
    "casting_idle",
    "casting_walk_forward",
    "casting_walk_backward",
    "casting_walk_left",
    "casting_walk_right",
];

/// Look up an animation clip in the `AnimationLibrary` by SHORT name (e.g. `casting_idle`). The
/// library keys are `"{gltf_name}::{clip}"` (see `bevy_modal_editor`'s `asset_libraries::index_gltf`),
/// so we match either the `::<short_name>` suffix or an exact (unqualified) key.
pub fn clip_node_for(lib: &AnimationLibrary, short_name: &str) -> Option<Handle<AnimationClip>> {
    let suffix = format!("::{short_name}");
    lib.clips
        .iter()
        .find(|(k, _)| k.ends_with(&suffix) || k.as_str() == short_name)
        .map(|(_, h)| h.clone())
}

/// The built preview `AnimationGraph` handle + the per-clip node indices, keyed by short clip name.
#[derive(Resource, Default)]
pub struct PreviewAnimGraph {
    pub graph: Option<Handle<AnimationGraph>>,
    pub nodes: HashMap<String, AnimationNodeIndex>,
}

impl PreviewAnimGraph {
    /// Whether the graph has been built (all its clips resolved + the asset added).
    pub fn ready(&self) -> bool {
        self.graph.is_some()
    }
}

/// Once the GLTF library has indexed `character.glb`'s clips, build a single `AnimationGraph` adding
/// every resolvable preview clip under the root. Idempotent: returns early once `ready()`.
pub fn build_preview_anim_graph(
    lib: Res<AnimationLibrary>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut state: ResMut<PreviewAnimGraph>,
) {
    if state.ready() || lib.clips.is_empty() {
        return;
    }
    let mut graph = AnimationGraph::new();
    let root = graph.root;
    for name in PREVIEW_CLIPS {
        if let Some(clip) = clip_node_for(&lib, name) {
            let node = graph.add_clip(clip, 1.0, root);
            state.nodes.insert(name.to_string(), node);
        }
    }
    if state.nodes.is_empty() {
        return;
    }
    state.graph = Some(graphs.add(graph));
}

/// Attach the built graph to any freshly-spawned `AnimationPlayer` (the GLTF scene loader creates one
/// inside the rig tree). Seeds every clip looping muted-at-rest so the cue-driven blend (Task 31) can
/// set weights without first having to `play` each one.
pub fn attach_preview_anim_graph(
    mut commands: Commands,
    state: Res<PreviewAnimGraph>,
    pending: Query<Entity, (With<AnimationPlayer>, Without<AnimationGraphHandle>)>,
    mut players: Query<&mut AnimationPlayer>,
) {
    let Some(graph) = state.graph.clone() else {
        return;
    };
    for entity in &pending {
        let Ok(mut player) = players.get_mut(entity) else {
            continue;
        };
        for node in state.nodes.values() {
            player.play(*node).repeat().set_weight(0.0);
        }
        commands
            .entity(entity)
            .insert(AnimationGraphHandle(graph.clone()));
    }
}

/// Drive one clip node to `weight` (looping). The cosmetic observer calls this per cue-bound anim lane.
pub fn drive_anim_clip(player: &mut AnimationPlayer, node: AnimationNodeIndex, weight: f32) {
    player.play(node).repeat().set_weight(weight);
}

/// On the `GameStartedEvent` (Play), hang the `character.glb` scene under the preview caster so the
/// duel renders as the real rigged character. The scene loader will spawn the `AnimationPlayer` that
/// `attach_preview_anim_graph` binds the graph to.
pub fn spawn_preview_rig(
    mut started: MessageReader<GameStartedEvent>,
    caster: Query<Entity, With<PreviewCaster>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    if started.read().next().is_none() {
        return;
    }
    let Ok(caster) = caster.single() else {
        return;
    };
    let scene: Handle<Scene> =
        asset_server.load(GltfAssetLabel::Scene(0).from_asset("character.glb"));
    commands.entity(caster).with_children(|c| {
        c.spawn((SceneRoot(scene), Transform::default()));
    });
}

/// Wires the preview rig: the anim-graph state resource + the build/attach/spawn systems.
pub struct PreviewRigPlugin;

impl Plugin for PreviewRigPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PreviewAnimGraph>().add_systems(
            Update,
            (
                spawn_preview_rig,
                build_preview_anim_graph,
                attach_preview_anim_graph,
            ),
        );
    }
}
