//! Pure edit operations for the skill-designer timeline.
//!
//! These mutate obelisk `PhaseDurations` / `CollisionWindow` / `CastTimeline` in response to the
//! bottom-dock timeline's drag/add interactions. Kept pure (no egui, no ECS) so they unit-test
//! directly; the panel wires them to egui drag deltas and buttons.

use obelisk_bevy::assets::{
    CastTimeline, CollisionShape, CollisionWindow, HitFilter, HitMode, PhaseDurations,
    VolumeMotion, WindowPhase,
};

/// Drag a phase boundary to absolute time `t`, keeping the total duration and the neighboring
/// phase's far edge fixed. `boundary` 0 = windup/active split, 1 = active/recovery split.
pub fn set_phase_boundary(d: &mut PhaseDurations, boundary: usize, t: f32) {
    match boundary {
        0 => {
            let cap = d.windup + d.active;
            let nb = t.clamp(0.0, cap);
            let delta = nb - d.windup;
            d.windup = nb;
            d.active = (d.active - delta).max(0.0);
        }
        1 => {
            let lo = d.windup;
            let cap = lo + d.active + d.recovery;
            let nb = t.clamp(lo, cap);
            let na = nb - lo;
            let delta = na - d.active;
            d.active = na;
            d.recovery = (d.recovery - delta).max(0.0);
        }
        _ => {}
    }
}

/// A sensible default collision window (an Active-phase 0.1s enemies-only sphere).
pub fn default_window(id: impl Into<String>) -> CollisionWindow {
    CollisionWindow {
        id: id.into(),
        spawn_phase: WindowPhase::Active,
        spawn_offset: 0.0,
        active_duration: 0.1,
        shape: CollisionShape::Sphere { radius: 0.5 },
        motion: VolumeMotion::Static,
        hit_filter: HitFilter::Enemies,
        hit_mode: HitMode::OncePerTarget,
        rehit_interval: None,
    }
}

/// Append a new default collision window with an auto-assigned `window_<n>` id.
pub fn add_collision_window(tl: &mut CastTimeline) {
    let id = format!("window_{}", tl.collision_windows.len());
    tl.collision_windows.push(default_window(id));
}

/// Set a window's start to absolute time `new_start_abs`, converting to a phase-relative
/// `spawn_offset` (clamped so a start before the phase begins pins the offset to 0).
pub fn set_window_start(w: &mut CollisionWindow, d: &PhaseDurations, new_start_abs: f32) {
    let phase_start = match w.spawn_phase {
        WindowPhase::Windup => 0.0,
        WindowPhase::Active => d.windup.max(0.0),
        WindowPhase::Recovery => d.windup.max(0.0) + d.active.max(0.0),
    };
    w.spawn_offset = (new_start_abs - phase_start).max(0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use obelisk_bevy::assets::{CollisionShape, PhaseDurations};

    fn durations() -> PhaseDurations {
        PhaseDurations {
            windup: 0.3,
            active: 0.1,
            recovery: 0.2,
        }
    }

    #[test]
    fn set_phase_boundary_moves_windup_active_split() {
        let mut d = durations();
        set_phase_boundary(&mut d, 0, 0.35);
        assert!((d.windup - 0.35).abs() < 1e-6);
        assert!((d.active - 0.05).abs() < 1e-6);
        assert!((d.recovery - 0.2).abs() < 1e-6);
    }

    #[test]
    fn set_phase_boundary_clamps_at_cap() {
        let mut d = durations();
        set_phase_boundary(&mut d, 0, 5.0);
        assert!((d.windup - 0.4).abs() < 1e-6);
        assert!((d.active - 0.0).abs() < 1e-6);
        assert!((d.recovery - 0.2).abs() < 1e-6);
    }

    #[test]
    fn add_collision_window_appends_auto_ided_sphere_windows() {
        let mut tl = obelisk_bevy::assets::CastTimeline {
            skill_id: "test".into(),
            phase_durations: durations(),
            collision_windows: Vec::new(),
            targeting: obelisk_bevy::assets::CastTargeting::SelfCast,
            delivery: obelisk_bevy::assets::CastDelivery::Instant,
            vfx_cues: std::collections::HashMap::new(),
        };
        add_collision_window(&mut tl);
        add_collision_window(&mut tl);
        assert_eq!(tl.collision_windows.len(), 2);
        assert_eq!(tl.collision_windows[0].id, "window_0");
        assert_eq!(tl.collision_windows[1].id, "window_1");
        assert!(matches!(
            tl.collision_windows[0].shape,
            CollisionShape::Sphere { .. }
        ));
    }

    #[test]
    fn set_window_start_converts_absolute_to_offset() {
        let d = durations();
        let mut w = default_window("a");
        set_window_start(&mut w, &d, 0.32);
        assert!((w.spawn_offset - 0.02).abs() < 1e-6);
    }

    #[test]
    fn set_window_start_clamps_before_phase_start() {
        let d = durations();
        let mut w = default_window("a");
        set_window_start(&mut w, &d, 0.1);
        assert!((w.spawn_offset - 0.0).abs() < 1e-6);
    }
}
