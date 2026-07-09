//! Slot-based mesh selection for the unified `character.glb`.
//!
//! `assets/character.glb` is a single rig + every class outfit, face
//! variant, and hair variant the asset pack ships. To present a
//! custom-looking character we keep all those meshes loaded and
//! toggle `Visibility` per mesh based on a [`PartSelection`].
//!
//! Slots:
//!   - **Top / Bottom / Headwear** — each is a list of variants where
//!     every variant is a *set* of mesh node-names. So "Knight Helmet"
//!     picks both `F_Knight_ArmetHelmet` and `F_Knight_ArmetHelmet_Visor`.
//!     This keeps the wire small (one u8 per slot) while preserving
//!     authored-together intent.
//!   - Weapons (`F_*_Staff`, `F_*_Sword`, `F_*_Bow`, etc.) and capes
//!     (`F_*_Cape`, `F_*_Shawl`, `F_*_Scarf`) are kept loaded but
//!     hidden — gameplay drives held items via spell / inventory
//!     state, and capes were removed from the customizer after the
//!     cape physics attempt didn't pan out.
//!   - **Hair** — optional. `None` for bald, otherwise one of the
//!     working `F_hair_*` variants.
//!   - **Eyes / Eyebrows / Mouth** — pick from `F_eyes0..4` etc. Only
//!     the `*0` variant is transform-correct in the current asset
//!     pack export — extending the tables here is enough to add more
//!     once the source FBX fixes them.
//!
//! Body skin (`F_TopBody`, `F_BottomBody`) is hidden always — every
//! class top/bottom fully covers it and keeping it visible
//! double-renders limbs.
//!
//! ## Arena adaptation from wisp's `parts.rs`
//!
//! wisp gates visibility on its `LocalWizardBody` marker (local player
//! only). Arena applies to BOTH players' rigs: the ancestor check walks
//! to [`ArenaBody`] (present on every spawned arena rig — local and
//! remote alike), then to that rig's parent `NetworkedPlayer` entity.
//! Each rig is driven PER-PLAYER:
//!   - the LOCAL rig (its player carries [`LocalNetPlayer`]) reads the
//!     LOCAL [`PartSelection`] resource — the one the customizer edits;
//!   - each REMOTE rig reads its own player's replicated
//!     [`PlayerCustomization`]`.parts`.
//!
//! Per-mesh `Visibility` is set on the `SkinnedMesh` child entities, NOT
//! the body root. The local body root stays `Visibility::Hidden` (set in
//! `present.rs` + enforced by `hide_local_player_body`). `Inherited`
//! per-mesh under a `Hidden` root propagates to hidden — correct for
//! the first-person case.

use std::collections::HashSet;

use bevy::camera::visibility::Visibility;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;

use crate::client::net::LocalNetPlayer;
use crate::client::rig::ArenaBody;
use crate::net::protocol::PlayerCustomization;

pub struct PartsPlugin;

impl Plugin for PartsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PartSelection>().add_systems(
            Update,
            (
                apply_arena_part_visibility,
                refresh_arena_part_visibility_on_change,
            ),
        );
    }
}

pub use arena_sim::parts::*;

/// Marker placed on every mesh entity once we've decided its
/// visibility for the current selection. Caches the glTF `name` (so
/// `refresh_arena_part_visibility_on_change` re-evaluates without walking
/// back up to read `Name`), the owning `player` (`NetworkedPlayer` entity)
/// so the refresh can re-resolve that rig's selection, and `is_local` so
/// the refresh knows whether to read the LOCAL [`PartSelection`] resource
/// or the player's replicated [`PlayerCustomization`].
#[derive(Component)]
pub struct PartMesh {
    pub name: String,
    /// The `NetworkedPlayer` entity this rig hangs under.
    pub player: Entity,
    /// Whether `player` is the LOCAL player (drives which selection source applies).
    pub is_local: bool,
}

/// Newly-spawned skinned meshes not yet stamped with [`PartMesh`], with their optional `Name`.
/// Aliased to keep [`apply_arena_part_visibility`]'s signature under clippy's `type_complexity` bar.
type PendingPartMeshes<'w, 's> =
    Query<'w, 's, (Entity, Option<&'static Name>), (With<SkinnedMesh>, Without<PartMesh>)>;

/// Apply per-mesh visibility to newly-spawned skinned meshes under any [`ArenaBody`]
/// (local + remote). Resolves each mesh entity to its glTF node-name (from the entity's
/// own `Name` or its nearest named ancestor) AND its owning `NetworkedPlayer` (the parent
/// of the [`ArenaBody`] root). The LOCAL rig reads the LOCAL [`PartSelection`] resource;
/// each REMOTE rig reads its player's replicated [`PlayerCustomization`]. Stamps each mesh
/// with [`PartMesh`] so it's processed exactly once.
#[allow(clippy::too_many_arguments)]
fn apply_arena_part_visibility(
    mut commands: Commands,
    local_selection: Res<PartSelection>,
    customizations: Query<&PlayerCustomization>,
    locals: Query<(), With<LocalNetPlayer>>,
    pending: PendingPartMeshes,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    body_marker: Query<(), With<ArenaBody>>,
    mut visibility: Query<&mut Visibility>,
) {
    for (entity, name) in &pending {
        // Resolve the ArenaBody scene root this mesh hangs under (local + remote rigs both
        // carry ArenaBody), then the rig's parent `NetworkedPlayer` entity.
        let Some(body_root) = ancestor_arena_body(entity, &parents, &body_marker) else {
            continue;
        };
        let Some(player) = parents.get(body_root).ok().map(|p| p.0) else {
            continue;
        };
        let is_local = locals.contains(player);
        // Resolve the glTF node-name: prefer the mesh entity's own `Name` (if
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

        let selection = selection_for(player, is_local, &local_selection, &customizations);
        let vis = if selection.is_visible(&resolved) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if let Ok(mut v) = visibility.get_mut(entity) {
            *v = vis;
        }
        commands.entity(entity).insert(PartMesh {
            name: resolved,
            player,
            is_local,
        });
    }
}

/// Re-evaluate visibility on cached [`PartMesh`] entities whenever the selection driving them
/// changes: the LOCAL [`PartSelection`] resource (customizer edits) re-evaluates LOCAL meshes;
/// a player whose replicated [`PlayerCustomization`] changed (D6 broadcast / initial replication)
/// re-evaluates that REMOTE rig's meshes. No-ops when nothing changed (cheap every frame).
fn refresh_arena_part_visibility_on_change(
    local_selection: Res<PartSelection>,
    customizations: Query<&PlayerCustomization>,
    changed_players: Query<Entity, Changed<PlayerCustomization>>,
    mut meshes: Query<(&PartMesh, &mut Visibility)>,
) {
    let local_changed = local_selection.is_changed();
    let changed: HashSet<Entity> = changed_players.iter().collect();
    if !local_changed && changed.is_empty() {
        return;
    }
    for (part, mut vis) in &mut meshes {
        let reeval = if part.is_local {
            local_changed
        } else {
            changed.contains(&part.player)
        };
        if !reeval {
            continue;
        }
        let selection = selection_for(
            part.player,
            part.is_local,
            &local_selection,
            &customizations,
        );
        *vis = if selection.is_visible(&part.name) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

/// The effective [`PartSelection`] for a rig: the LOCAL resource for the local player, else the
/// player's replicated [`PlayerCustomization`] (falling back to the default witch if it hasn't
/// replicated yet).
fn selection_for(
    player: Entity,
    is_local: bool,
    local_selection: &PartSelection,
    customizations: &Query<&PlayerCustomization>,
) -> PartSelection {
    if is_local {
        *local_selection
    } else {
        customizations
            .get(player)
            .map(|c| c.parts)
            .unwrap_or_default()
    }
}

/// Walk the `ChildOf` parent chain to find the [`ArenaBody`] scene root a mesh belongs to.
fn ancestor_arena_body(
    entity: Entity,
    parents: &Query<&ChildOf>,
    marker: &Query<(), With<ArenaBody>>,
) -> Option<Entity> {
    let mut cur = entity;
    loop {
        if marker.contains(cur) {
            return Some(cur);
        }
        match parents.get(cur) {
            Ok(p) => cur = p.0,
            Err(_) => return None,
        }
    }
}

