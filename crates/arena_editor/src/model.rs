//! The authoring model: the `EditedSkill` resource (the in-flight `CastTimeline` the designer is
//! editing) plus the pure helpers that seed it. `blank_cast_timeline` builds a minimal valid
//! timeline for a fresh skill, and `derive_vfx_cues` computes the LOCKED cue-key → cue-id map the
//! designer offers as VFX-bind targets (M3 binds cosmetic lanes to these VALUES).

use arena_skills::SkillFx;
use bevy::prelude::*;
use obelisk_bevy::assets::{
    CastDelivery, CastTargeting, CastTimeline, CollisionWindow, PhaseDurations,
};
use std::collections::HashMap;
use std::path::PathBuf;

/// The skill currently open in the designer: its `CastTimeline`, the `.cast.ron` path it saves to,
/// whether it has unsaved edits, and which collision window (if any) is selected on the timeline.
#[derive(Resource)]
pub struct EditedSkill {
    pub timeline: CastTimeline,
    pub path: PathBuf,
    pub dirty: bool,
    pub selected_window: Option<usize>,
}

impl EditedSkill {
    /// Open `timeline` for editing, saving to `path`, with no unsaved edits and nothing selected.
    pub fn from_timeline(timeline: CastTimeline, path: PathBuf) -> Self {
        Self {
            timeline,
            path,
            dirty: false,
            selected_window: None,
        }
    }
}

/// A minimal valid `CastTimeline` for a new skill: a short windup/active/recovery, no collision
/// windows yet, single-target instant delivery, no cues.
pub fn blank_cast_timeline(skill_id: impl Into<String>) -> CastTimeline {
    CastTimeline {
        skill_id: skill_id.into(),
        phase_durations: PhaseDurations {
            windup: 0.3,
            active: 0.1,
            recovery: 0.2,
        },
        collision_windows: Vec::<CollisionWindow>::new(),
        targeting: CastTargeting::SingleEntity { range: 15.0 },
        delivery: CastDelivery::Instant,
        vfx_cues: HashMap::new(),
    }
}

/// The cosmetic layer currently open in the designer: its `SkillFx` (the `.skillfx.ron` lanes bound
/// to this skill's cues), the path it saves to, and whether it has unsaved edits. Edited alongside
/// [`EditedSkill`]; Save writes BOTH files.
#[derive(Resource)]
pub struct EditedSkillFx {
    pub fx: SkillFx,
    pub path: PathBuf,
    pub dirty: bool,
}

impl EditedSkillFx {
    /// Open `fx` for editing, saving to `path`, with no unsaved edits.
    pub fn from_fx(fx: SkillFx, path: PathBuf) -> Self {
        Self {
            fx,
            path,
            dirty: false,
        }
    }
}

/// A fresh cosmetic layer for `skill_id` with no lanes yet (the designer adds lanes per cue as the
/// author binds them). The load-or-blank fallback when a skill has no `.skillfx.ron` on disk.
pub fn blank_skillfx(skill_id: impl Into<String>) -> SkillFx {
    SkillFx {
        skill_id: skill_id.into(),
        lanes: HashMap::new(),
    }
}

/// The LOCKED cue-key → cue-id map the designer exposes as VFX-bind targets: always `on_cast`
/// and `on_hit`, plus one `on_window_{wid}` (open) and one `on_end_{wid}` (termination, fired at
/// the END POSITION — hit / world / fuse) per collision window. The VALUES (`{id}_cast`,
/// `{id}_window_{wid}`, `{id}_end_{wid}`, `{id}_impact`) are the cue ids the preview cosmetics
/// bind lanes to.
pub fn derive_vfx_cues(tl: &CastTimeline) -> HashMap<String, String> {
    let id = &tl.skill_id;
    let mut cues = HashMap::new();
    cues.insert("on_cast".to_string(), format!("{id}_cast"));
    for w in &tl.collision_windows {
        cues.insert(
            format!("on_window_{}", w.id),
            format!("{id}_window_{}", w.id),
        );
        cues.insert(format!("on_end_{}", w.id), format!("{id}_end_{}", w.id));
    }
    cues.insert("on_hit".to_string(), format!("{id}_impact"));
    cues
}

#[cfg(test)]
mod tests {
    use super::*;
    use obelisk_bevy::assets::{CollisionShape, HitFilter, HitMode, VolumeMotion, WindowPhase};

    fn firebolt_window() -> CollisionWindow {
        CollisionWindow {
            id: "bolt".into(),
            spawn_phase: WindowPhase::Active,
            spawn_offset: 0.0,
            active_duration: 0.1,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Static,
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            on_end: Default::default(),
        }
    }

    #[test]
    fn blank_cast_timeline_is_a_minimal_valid_instant_skill() {
        let tl = blank_cast_timeline("newskill");
        assert_eq!(tl.skill_id, "newskill");
        assert_eq!(tl.phase_durations.windup, 0.3);
        assert!(tl.collision_windows.is_empty());
        assert_eq!(
            format!("{:?}", tl.delivery),
            format!("{:?}", CastDelivery::Instant)
        );
    }

    #[test]
    fn derive_vfx_cues_for_a_one_window_firebolt_yields_the_locked_keys() {
        let mut tl = blank_cast_timeline("firebolt");
        tl.collision_windows.push(firebolt_window());
        let cues = derive_vfx_cues(&tl);
        assert_eq!(cues.len(), 4, "cast + window open + window end + hit");
        assert_eq!(
            cues.get("on_cast").map(String::as_str),
            Some("firebolt_cast")
        );
        assert_eq!(
            cues.get("on_window_bolt").map(String::as_str),
            Some("firebolt_window_bolt")
        );
        assert_eq!(
            cues.get("on_end_bolt").map(String::as_str),
            Some("firebolt_end_bolt")
        );
        assert_eq!(
            cues.get("on_hit").map(String::as_str),
            Some("firebolt_impact")
        );
    }
}
