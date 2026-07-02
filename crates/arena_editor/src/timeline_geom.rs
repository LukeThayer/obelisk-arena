//! Pure timeline geometry helpers for the skill-designer bottom-dock timeline.
//!
//! These map obelisk `PhaseDurations` / `CollisionWindow` timing into a normalized time axis and
//! then into pixel-space `x` coordinates. Kept pure (no egui, no ECS) so they unit-test directly.

use obelisk_bevy::assets::{CastTimeline, CollisionWindow, PhaseDurations, WindowPhase};

/// Total authored duration of the three phases (clamping negatives to 0).
pub fn total_duration(d: &PhaseDurations) -> f32 {
    d.windup.max(0.0) + d.active.max(0.0) + d.recovery.max(0.0)
}

/// The full time span the timeline strip (and its scrubber) covers: the phase total, extended to
/// the LATEST collision-window close (chained windows resolved after their parent). A projectile
/// window routinely outlives the cast phases (firebolt: phases 0.6 s, bolt flies 2 s) — without
/// the extension, the window bar runs off the strip and the scrub head can never reach the end
/// moments staged at window closes.
pub fn strip_span(tl: &CastTimeline) -> f32 {
    tl.collision_windows
        .iter()
        .map(|w| resolved_window_span(tl, w).1)
        .fold(total_duration(&tl.phase_durations), f32::max)
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

/// Absolute `(start, end)` time span of a SCHEDULED collision window given the phase durations.
/// `Chained` windows have no schedule of their own — this returns `(0, duration)` for them; use
/// [`resolved_window_span`] to place them after their parent's close.
pub fn window_span(w: &CollisionWindow, d: &PhaseDurations) -> (f32, f32) {
    let phase_start = match w.spawn_phase {
        WindowPhase::Windup => 0.0,
        WindowPhase::Active => d.windup.max(0.0),
        WindowPhase::Recovery => d.windup.max(0.0) + d.active.max(0.0),
        WindowPhase::Chained => return (0.0, w.active_duration.max(0.0)),
    };
    let start = phase_start + w.spawn_offset.max(0.0);
    (start, start + w.active_duration.max(0.0))
}

/// The window ids `w`'s `on_end` reactions chain to.
fn chain_targets(w: &CollisionWindow) -> impl Iterator<Item = &str> {
    [&w.on_end.hit, &w.on_end.world, &w.on_end.fuse]
        .into_iter()
        .flatten()
        .map(|obelisk_bevy::assets::EndReaction::Chain(id)| id.as_str())
}

/// Absolute `(start, end)` of a window on the STRIP axis, resolving `Chained` windows to start
/// at their (earliest) parent's close — recursively, depth-capped (the loader rejects cycles,
/// but the editor draws unsaved/invalid states too).
pub fn resolved_window_span(tl: &CastTimeline, w: &CollisionWindow) -> (f32, f32) {
    fn inner(tl: &CastTimeline, w: &CollisionWindow, depth: u8) -> (f32, f32) {
        if w.spawn_phase != WindowPhase::Chained || depth > 8 {
            return window_span(w, &tl.phase_durations);
        }
        let parent_close = tl
            .collision_windows
            .iter()
            .filter(|p| p.id != w.id && chain_targets(p).any(|t| t == w.id))
            .map(|p| inner(tl, p, depth + 1).1)
            .fold(None, |acc: Option<f32>, c| Some(acc.map_or(c, |a| a.min(c))));
        let start = parent_close.unwrap_or(0.0);
        (start, start + w.active_duration.max(0.0))
    }
    inner(tl, w, 0)
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
            on_end: Default::default(),
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
    fn strip_span_extends_to_the_latest_window_close() {
        use obelisk_bevy::assets::{CastDelivery, CastTargeting};
        let mut tl = CastTimeline {
            skill_id: "x".into(),
            phase_durations: pd(0.3, 0.1, 0.2),
            collision_windows: vec![],
            targeting: CastTargeting::SingleEntity { range: 15.0 },
            delivery: CastDelivery::Instant,
            vfx_cues: Default::default(),
        };
        assert_eq!(strip_span(&tl), 0.6, "no windows: phase total");
        tl.collision_windows
            .push(window(WindowPhase::Active, 0.0, 2.0));
        assert!(
            (strip_span(&tl) - 2.3).abs() < 1e-6,
            "long window extends the strip to its close"
        );
    }

    #[test]
    fn chained_window_resolves_after_its_parent_close() {
        use obelisk_bevy::assets::{CastDelivery, CastTargeting, EndReaction, OnEnd};
        let mut bolt = window(WindowPhase::Active, 0.0, 2.0);
        bolt.id = "bolt".into();
        bolt.on_end = OnEnd {
            hit: Some(EndReaction::Chain("blast".into())),
            world: Some(EndReaction::Chain("blast".into())),
            fuse: Some(EndReaction::Chain("blast".into())),
        };
        let mut blast = window(WindowPhase::Chained, 0.0, 0.05);
        blast.id = "blast".into();
        let tl = CastTimeline {
            skill_id: "firebolt".into(),
            phase_durations: pd(0.3, 0.1, 0.2),
            collision_windows: vec![bolt, blast],
            targeting: CastTargeting::SingleEntity { range: 15.0 },
            delivery: CastDelivery::Projectile { speed: 20.0 },
            vfx_cues: Default::default(),
        };
        let (s, e) = resolved_window_span(&tl, &tl.collision_windows[1]);
        assert!((s - 2.3).abs() < 1e-5, "blast starts at the bolt's close: {s}");
        assert!((e - 2.35).abs() < 1e-5);
        assert!((strip_span(&tl) - 2.35).abs() < 1e-5, "strip extends past the chain");
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
