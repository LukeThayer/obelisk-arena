//! Shared cast-timeline asset loader, used IDENTICALLY by both peers.
//!
//! The authoritative server AND the windowed client both need the `.cast.ron` cast timelines in
//! `CastTimelineHandles` (one handle per registered obelisk skill, loaded from
//! `assets/skills/<id>.cast.ron`). Without them obelisk's `validate_casts` rejects every cast with
//! `TimelineMissing`. The server needs them because it is the combat authority; the windowed client
//! needs them for the predicted-cast cue lookup (`skills::predicted_local_cast`) + cosmetics.
//!
//! Both peers `init_resource::<PendingCastTimelines>()`, run [`load_cast_timelines`] at `Startup`,
//! and [`poll_cast_timelines`] each `Update` — relocated here so the single copy is shared rather
//! than duplicated across `server/mod.rs` and `client/mod.rs`.

use bevy::prelude::*;
use obelisk_bevy::prelude::{CastTimeline, CastTimelineHandles, SkillRegistry};

/// Cast-timeline handles being polled to finish loading (skill id → handle). Drained into
/// `CastTimelineHandles` by [`poll_cast_timelines`] as each asset finishes loading.
#[derive(Resource, Default)]
pub struct PendingCastTimelines(pub Vec<(String, Handle<CastTimeline>)>);

/// Kick off loading a `.cast.ron` for every registered obelisk skill (firebolt), in sorted-id order
/// for determinism. Pushes each handle onto [`PendingCastTimelines`] for [`poll_cast_timelines`].
pub fn load_cast_timelines(
    mut pending: ResMut<PendingCastTimelines>,
    assets: Res<AssetServer>,
    skills: Res<SkillRegistry>,
) {
    let mut ids: Vec<String> = skills.0.keys().cloned().collect();
    ids.sort();
    for id in ids {
        let handle: Handle<CastTimeline> = assets.load(format!("skills/{id}.cast.ron"));
        pending.0.push((id, handle));
    }
}

/// Poll the pending cast assets each frame; move loaded ones into `CastTimelineHandles` so
/// `validate_casts` (server) / the predicted-cast cue lookup (client) can resolve the timeline.
pub fn poll_cast_timelines(
    mut pending: ResMut<PendingCastTimelines>,
    timelines: Res<Assets<CastTimeline>>,
    mut registry: ResMut<CastTimelineHandles>,
) {
    if pending.0.is_empty() {
        return;
    }
    pending.0.retain(|(skill, handle)| {
        if timelines.get(handle).is_some() {
            info!("cast timeline loaded for {skill}");
            registry.0.insert(skill.clone(), handle.clone());
            false
        } else {
            true
        }
    });
}
