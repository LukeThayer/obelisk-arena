//! Pure timeline geometry helpers for the skill-designer bottom-dock timeline.
//!
//! These map obelisk `PhaseDurations` / `CollisionWindow` timing into a normalized time axis and
//! then into pixel-space `x` coordinates. Kept pure (no egui, no ECS) so they unit-test directly.

use obelisk_bevy::assets::{CollisionWindow, PhaseDurations, WindowPhase};

/// Total authored duration of the three phases (clamping negatives to 0).
pub fn total_duration(d: &PhaseDurations) -> f32 {
    d.windup.max(0.0) + d.active.max(0.0) + d.recovery.max(0.0)
}

/// The `[windup, active, recovery]` phase spans as `(start, end)` absolute-time pairs.
pub fn phase_spans(d: &PhaseDurations) -> [(f32, f32); 3] {
    let w = d.windup.max(0.0);
    let a = d.active.max(0.0);
    let r = d.recovery.max(0.0);
    [(0.0, w), (w, w + a), (w + a, w + a + r)]
}

/// Map an absolute time `t` (over `[0, span]`) into a pixel `x` in `[left, left + width]`.
/// Degenerate `span <= 0` pins to `left`.
pub fn time_to_x(t: f32, span: f32, left: f32, width: f32) -> f32 {
    if span <= 0.0 {
        left
    } else {
        left + (t / span).clamp(0.0, 1.0) * width
    }
}

/// Absolute `(start, end)` time span of a collision window given the phase durations.
pub fn window_span(w: &CollisionWindow, d: &PhaseDurations) -> (f32, f32) {
    let phase_start = match w.spawn_phase {
        WindowPhase::Windup => 0.0,
        WindowPhase::Active => d.windup.max(0.0),
        WindowPhase::Recovery => d.windup.max(0.0) + d.active.max(0.0),
    };
    let start = phase_start + w.spawn_offset.max(0.0);
    (start, start + w.active_duration.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obelisk_bevy::assets::{CollisionShape, HitFilter, HitMode, VolumeMotion};

    fn pd(windup: f32, active: f32, recovery: f32) -> PhaseDurations {
        PhaseDurations {
            windup,
            active,
            recovery,
        }
    }

    fn window(
        spawn_phase: WindowPhase,
        spawn_offset: f32,
        active_duration: f32,
    ) -> CollisionWindow {
        CollisionWindow {
            id: "w".into(),
            spawn_phase,
            spawn_offset,
            active_duration,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Static,
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
        }
    }

    #[test]
    fn phase_spans_and_total_duration() {
        let d = pd(0.3, 0.1, 0.2);
        assert_eq!(phase_spans(&d), [(0.0, 0.3), (0.3, 0.4), (0.4, 0.6)]);
        assert_eq!(total_duration(&d), 0.6);
    }

    #[test]
    fn time_to_x_maps_and_clamps() {
        assert_eq!(time_to_x(0.0, 0.6, 10.0, 100.0), 10.0);
        assert_eq!(time_to_x(0.6, 0.6, 10.0, 100.0), 110.0);
        assert_eq!(time_to_x(0.3, 0.6, 0.0, 100.0), 50.0);
        assert_eq!(time_to_x(5.0, 0.0, 7.0, 100.0), 7.0);
    }

    #[test]
    fn window_span_offsets_from_its_phase() {
        let d = pd(0.3, 0.1, 0.2);
        assert_eq!(
            window_span(&window(WindowPhase::Active, 0.0, 0.1), &d),
            (0.3, 0.4)
        );
        // 0.3 + 0.1 + 0.05 accumulates f32 rounding, so compare within epsilon.
        assert!((window_span(&window(WindowPhase::Recovery, 0.05, 0.1), &d).0 - 0.45).abs() < 1e-5);
    }
}
