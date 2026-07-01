//! Rig socket index: scans `Name`d descendants of the preview caster rig so the cosmetics layer can
//! bind lane effects to named bones (e.g. `"chest_joint"`, `"right_hand"`). Attaching to a socket
//! that doesn't exist falls back to the rig root, so authoring against a missing bone never panics.

use arena_sim::preview::PreviewCaster;
use bevy::prelude::*;
use std::collections::HashMap;

/// Index of the preview rig's named sockets (bones). Populated by [`index_rig_sockets`] as the rig's
/// `Name`d entities spawn; consumed by [`resolve_socket`] to map an authored socket name to an entity.
#[derive(Resource, Default)]
pub struct RigSockets {
    /// Socket names in first-seen order (stable for UI listing).
    pub names: Vec<String>,
    /// Name → entity lookup.
    pub by_name: HashMap<String, Entity>,
}

/// Record newly-added `Name`d entities that are descendants of a [`PreviewCaster`] into [`RigSockets`].
/// Walks the `ChildOf` chain to the root; ignores names that aren't under the preview caster (e.g. UI).
pub fn index_rig_sockets(
    q: Query<(Entity, &Name), Added<Name>>,
    parents: Query<&ChildOf>,
    caster: Query<Entity, With<PreviewCaster>>,
    mut sockets: ResMut<RigSockets>,
) {
    for (entity, name) in &q {
        let mut cur = entity;
        let mut under = caster.get(cur).is_ok();
        while let Ok(p) = parents.get(cur) {
            cur = p.0;
            if caster.get(cur).is_ok() {
                under = true;
                break;
            }
        }
        if !under {
            continue;
        }
        let n = name.as_str().to_string();
        if sockets.by_name.insert(n.clone(), entity).is_none() {
            sockets.names.push(n);
        }
    }
}

/// Resolve an authored socket name to an entity, falling back to `root` when the name is `None` or
/// unknown. This is the pure attach-point lookup the cosmetics layer uses when spawning lane effects.
pub fn resolve_socket(s: &RigSockets, name: Option<&str>, root: Entity) -> Entity {
    name.and_then(|n| s.by_name.get(n)).copied().unwrap_or(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_socket_hits_known_name_and_falls_back_to_root() {
        let root = Entity::from_raw_u32(1).unwrap();
        let bone = Entity::from_raw_u32(2).unwrap();
        let mut sockets = RigSockets::default();
        sockets.by_name.insert("chest_joint".into(), bone);
        sockets.names.push("chest_joint".into());
        assert_eq!(resolve_socket(&sockets, Some("chest_joint"), root), bone);
        assert_eq!(resolve_socket(&sockets, Some("missing"), root), root);
        assert_eq!(resolve_socket(&sockets, None, root), root);
    }

    #[test]
    fn index_rig_sockets_records_names_under_the_caster_and_ignores_others() {
        let mut app = App::new();
        app.init_resource::<RigSockets>();
        app.add_systems(Update, index_rig_sockets);

        // A preview caster rig root with a named bone child.
        let caster = app.world_mut().spawn(PreviewCaster).id();
        app.world_mut()
            .spawn((Name::new("chest_joint"), ChildOf(caster)));
        // An unrelated named entity NOT under the caster (e.g. a UI label).
        app.world_mut().spawn(Name::new("ui_label"));

        app.update();

        let sockets = app.world().resource::<RigSockets>();
        assert!(
            sockets.by_name.contains_key("chest_joint"),
            "the bone under the preview caster should be indexed"
        );
        assert!(
            !sockets.by_name.contains_key("ui_label"),
            "a name not under the preview caster should be ignored"
        );
        assert_eq!(sockets.names, vec!["chest_joint".to_string()]);
    }
}
