//! Timeline scrubbing: drag across the panel's phase strip to preview the authored cue VFX
//! WITHOUT hitting Play. The panel writes the pointer's timeline time into [`ScrubState`];
//! [`fire_scrub_cues`] fires each cue's bound lanes (as synthetic obelisk `CueEvent`s through the
//! same `on_preview_cue` observer the live sim drives) when the scrub head crosses its moment.
//!
//! Cue moments mirror where the real sim fires them (`obelisk-bevy` `src/vfx.rs`): `on_cast` at
//! cast start (t = 0), each `on_window_{id}` at its window's OPEN. `on_hit` has no authored time —
//! the sim fires it when the flying hitbox connects — so the scrubber approximates it at the first
//! window's CLOSE (else end of active). Positions are staged on the standard duel markers: cast/
//! window cues at the caster marker (muzzle height), the hit at the dummy marker. No duel entities
//! are needed — `on_preview_cue` falls back to world-space spawns when no caster exists.

use crate::model::{derive_vfx_cues, EditedSkill};
use crate::preview_controller::Playhead;
use crate::timeline_geom::{resolved_window_span, strip_span};
use arena_sim::spawn::SPAWN_MARKERS;
use bevy::prelude::*;
use obelisk_bevy::assets::CastTimeline;
use obelisk_bevy::events::{CueEvent, CueKind as ObeliskCueKind};

/// Muzzle-height offset for staged cast/window cue positions (matches the game's
/// `MUZZLE_HEIGHT_OFFSET` in `arena_game::client::cosmetics`).
const MUZZLE_OFFSET: Vec3 = Vec3::new(0.0, 1.2, 0.0);

/// How far in front of the caster marker a window cue stages its volume (the dummy is toward +X).
const WINDOW_FORWARD: Vec3 = Vec3::new(1.0, 1.2, 0.0);

/// On a fresh grab (click with no prior scrub position), fire cues within this fraction of the
/// total span behind the pointer — so clicking directly ON a cue moment plays it (e.g. near the
/// strip's left edge for the `on_cast` cue at t = 0).
const GRAB_SLOP_FRAC: f32 = 0.05;

/// The scrub head the panel writes: `time` is the pointer's timeline time while dragging the phase
/// strip (kept after release so the marker stays visible), `fired_up_to` is the high-water mark
/// [`fire_scrub_cues`] has already fired through (rewinds reset it, so scrubbing back and forth
/// replays).
#[derive(Resource, Default)]
pub struct ScrubState {
    pub time: Option<f32>,
    pub fired_up_to: Option<f32>,
}

/// One cue's staged scrub preview: its timeline moment, the cue id VALUE the lanes bind to, the
/// obelisk kind, and the world position(s) to stage the vfx at (`position_from` for two-anchor
/// beam cues).
#[derive(Debug, Clone, PartialEq)]
pub struct CueMoment {
    pub t: f32,
    pub cue_id: String,
    pub kind: ObeliskCueKind,
    pub position: Vec3,
    pub position_from: Option<Vec3>,
}

/// The scrub-preview cue schedule for `tl`, sorted by time. See the module doc for the timing and
/// staging rules. Window spans are chain-resolved (a `Chained` blast opens at its parent's close),
/// and every window gets an END moment at its close — the `on_end_{id}` cue staged at the dummy
/// marker (scheduled windows launch toward it; chained windows spawn there).
pub fn cue_moments(tl: &CastTimeline) -> Vec<CueMoment> {
    let cues = derive_vfx_cues(tl);
    let mut moments = Vec::new();
    if let Some(id) = cues.get("on_cast") {
        moments.push(CueMoment {
            t: 0.0,
            cue_id: id.clone(),
            kind: ObeliskCueKind::OnCast,
            position: SPAWN_MARKERS[0] + MUZZLE_OFFSET,
            position_from: None,
        });
    }
    let mut first_window_close: Option<f32> = None;
    for w in &tl.collision_windows {
        let (open, close) = resolved_window_span(tl, w);
        first_window_close = Some(first_window_close.map_or(close, |c: f32| c.min(close)));
        let chained = w.spawn_phase == obelisk_bevy::assets::WindowPhase::Chained;
        let beam = matches!(w.motion, obelisk_bevy::assets::VolumeMotion::Beam);
        if let Some(id) = cues.get(&format!("on_window_{}", w.id)) {
            moments.push(CueMoment {
                t: open,
                cue_id: id.clone(),
                kind: ObeliskCueKind::OnWindow,
                // Chained windows open where their parent ended (the target area); scheduled
                // ones open in front of the caster; a beam's cue lands ON its victim.
                position: if chained || beam {
                    SPAWN_MARKERS[1]
                } else {
                    SPAWN_MARKERS[0] + WINDOW_FORWARD
                },
                // Beam cues are two-anchor: stage the arc caster→dummy (a chained hop stages
                // from the first dummy's spot instead).
                position_from: beam.then(|| {
                    if chained {
                        SPAWN_MARKERS[1] + Vec3::new(0.0, 0.0, -1.5)
                    } else {
                        SPAWN_MARKERS[0] + MUZZLE_OFFSET
                    }
                }),
            });
        }
        if let Some(id) = cues.get(&format!("on_end_{}", w.id)) {
            moments.push(CueMoment {
                t: close,
                cue_id: id.clone(),
                kind: ObeliskCueKind::OnEnd,
                position: SPAWN_MARKERS[1],
                position_from: None,
            });
        }
    }
    if let Some(id) = cues.get("on_hit") {
        let d = &tl.phase_durations;
        moments.push(CueMoment {
            t: first_window_close.unwrap_or(d.windup + d.active),
            cue_id: id.clone(),
            kind: ObeliskCueKind::OnHit,
            position: SPAWN_MARKERS[1],
            position_from: None,
        });
    }
    moments.sort_by(|a, b| a.t.total_cmp(&b.t));
    moments
}

/// Which of `moments` to fire for a scrub move from `fired_up_to` to `cur`: a forward drag fires
/// everything crossed in `(prev, cur]`; a fresh grab fires within the trailing `slop`; a rewind
/// fires nothing (it just resets the high-water mark). Pure — unit-tested directly.
pub fn cues_to_fire(
    moments: &[CueMoment],
    fired_up_to: Option<f32>,
    cur: f32,
    slop: f32,
) -> Vec<&CueMoment> {
    // A fresh grab's slop window is INCLUSIVE at its lower bound (a click at exactly `slop` past a
    // cue still plays it); a forward drag is exclusive at `prev` so held/repeated positions don't
    // refire the cue already played.
    let (lo, inclusive) = match fired_up_to {
        Some(prev) if prev <= cur => (prev, false),
        Some(_) => return Vec::new(), // rewind
        None => (cur - slop, true),
    };
    moments
        .iter()
        .filter(|m| (m.t > lo || (inclusive && m.t >= lo)) && m.t <= cur)
        .collect()
}

/// Fire the lanes for every cue moment the scrub head crossed since the last run, as synthetic
/// `CueEvent`s (the source entity is a placeholder — `on_preview_cue` never dereferences it when
/// no caster exists). Skipped while a live cast plays (the real sim owns the cues then).
pub fn fire_scrub_cues(
    mut scrub: ResMut<ScrubState>,
    edited: Res<EditedSkill>,
    playhead: Res<Playhead>,
    mut commands: Commands,
) {
    let Some(cur) = scrub.time else {
        return;
    };
    if playhead.active {
        scrub.fired_up_to = Some(cur);
        return;
    }
    if scrub.fired_up_to == Some(cur) {
        return;
    }
    // Same span the strip maps pixels with (phases extended to the latest window close), so the
    // grab slop is a consistent fraction of what the user actually drags across.
    let span = strip_span(&edited.timeline).max(0.0001);
    let moments = cue_moments(&edited.timeline);
    for m in cues_to_fire(&moments, scrub.fired_up_to, cur, span * GRAB_SLOP_FRAC) {
        commands.trigger(CueEvent {
            cue_id: m.cue_id.clone(),
            source: Entity::PLACEHOLDER,
            position: m.position,
            position_from: m.position_from,
            kind: m.kind,
        });
    }
    scrub.fired_up_to = Some(cur);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::blank_cast_timeline;
    use obelisk_bevy::assets::{
        CollisionShape, CollisionWindow, HitFilter, HitMode, VolumeMotion, WindowPhase,
    };

    fn firebolt_like() -> CastTimeline {
        let mut tl = blank_cast_timeline("firebolt");
        // windup 0.3 / active 0.1 / recovery 0.2 (blank defaults)
        tl.collision_windows.push(CollisionWindow {
            id: "bolt".into(),
            spawn_phase: WindowPhase::Active,
            spawn_offset: 0.0,
            active_duration: 2.0,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Linear { speed: 20.0 },
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::FirstOnly,
            rehit_interval: None,
            on_end: Default::default(),
        });
        tl
    }

    #[test]
    fn cue_moments_schedule_cast_window_end_and_hit() {
        let m = cue_moments(&firebolt_like());
        assert_eq!(m.len(), 4, "cast + window open + window end + hit");
        assert_eq!(m[0].cue_id, "firebolt_cast");
        assert_eq!(m[0].t, 0.0);
        assert_eq!(m[1].cue_id, "firebolt_window_bolt");
        assert!((m[1].t - 0.3).abs() < 1e-6, "window opens at windup end");
        assert_eq!(m[2].cue_id, "firebolt_end_bolt");
        assert!((m[2].t - 2.3).abs() < 1e-6, "end staged at window close");
        assert_eq!(m[2].kind, ObeliskCueKind::OnEnd);
        assert_eq!(m[2].position, SPAWN_MARKERS[1]);
        assert_eq!(m[3].cue_id, "firebolt_impact");
        assert!((m[3].t - 2.3).abs() < 1e-6);
    }

    #[test]
    fn windowless_timeline_stages_the_hit_at_active_end() {
        let m = cue_moments(&blank_cast_timeline("x"));
        assert_eq!(m.len(), 2, "cast + hit only");
        assert!((m[1].t - 0.4).abs() < 1e-6, "windup 0.3 + active 0.1");
    }

    #[test]
    fn forward_drag_fires_crossed_cues_and_rewind_fires_none() {
        let moments = cue_moments(&firebolt_like());
        // Forward crossing over the window open at 0.3.
        let fired = cues_to_fire(&moments, Some(0.1), 0.5, 0.01);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].cue_id, "firebolt_window_bolt");
        // Rewind: nothing.
        assert!(cues_to_fire(&moments, Some(0.5), 0.1, 0.01).is_empty());
        // Fresh grab right on the cast start fires it via the slop window.
        let grabbed = cues_to_fire(&moments, None, 0.0, 0.01);
        assert_eq!(grabbed.len(), 1);
        assert_eq!(grabbed[0].cue_id, "firebolt_cast");
        // A grab whose slop lands EXACTLY on the cue moment still fires it (inclusive bound) —
        // clicking a few pixels into the strip must catch the t = 0 cast cue.
        let edge = cues_to_fire(&moments, None, 0.03, 0.03);
        assert_eq!(edge.len(), 1);
        assert_eq!(edge[0].cue_id, "firebolt_cast");
    }
}
