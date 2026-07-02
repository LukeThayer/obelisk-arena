//! Ballistic aim helpers shared by the editor preview (loft the demo cast onto the dummy, draw
//! the trajectory gizmo) and any future AI aiming. Pure math — no ECS.

use bevy::prelude::*;

/// The launch direction (unit vector) that lands a ballistic projectile fired at `speed` under
/// `gravity` from `from` onto `to` — the LOW-arc solution (flattest hit), which is what a player
/// leading with free look would shoot. Falls back to:
/// - the straight line for non-ballistic inputs (`gravity <= 0`) or a (near-)vertical shot,
/// - a 45° lob toward the target when it is out of range (no exact solution).
pub fn ballistic_launch_dir(from: Vec3, to: Vec3, speed: f32, gravity: f32) -> Vec3 {
    let delta = to - from;
    let flat = Vec3::new(delta.x, 0.0, delta.z);
    let d = flat.length();
    if gravity <= 0.0 || d < 1e-4 || speed <= 0.0 {
        return delta.normalize_or(Vec3::X);
    }
    let h = delta.y;
    let s2 = speed * speed;
    // Standard projectile-aim quadratic in tan(theta):
    //   tan(theta) = (s^2 ± sqrt(s^4 - g(g d^2 + 2 h s^2))) / (g d)
    let disc = s2 * s2 - gravity * (gravity * d * d + 2.0 * h * s2);
    if disc < 0.0 {
        // Out of range: best-effort 45° lob toward the target.
        return (flat / d + Vec3::Y).normalize();
    }
    let tan_theta = (s2 - disc.sqrt()) / (gravity * d);
    (flat / d + Vec3::Y * tan_theta).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Analytically fly the arc and measure the height error at the target's horizontal distance.
    fn miss_at_target(from: Vec3, to: Vec3, speed: f32, gravity: f32) -> f32 {
        let dir = ballistic_launch_dir(from, to, speed, gravity);
        let v = dir * speed;
        let flat = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
        let d = flat.length();
        let v_flat = Vec3::new(v.x, 0.0, v.z).length();
        let t = d / v_flat;
        let y = from.y + v.y * t - 0.5 * gravity * t * t;
        (y - to.y).abs()
    }

    #[test]
    fn lands_on_a_level_target() {
        // The preview duel: caster center (-4, 0.59, 0) → dummy center (4, 0.59, 0), firebolt 20/9.8.
        let from = Vec3::new(-4.0, 0.59, 0.0);
        let to = Vec3::new(4.0, 0.59, 0.0);
        assert!(miss_at_target(from, to, 20.0, 9.8) < 1e-3);
        let dir = ballistic_launch_dir(from, to, 20.0, 9.8);
        assert!(dir.y > 0.05 && dir.y < 0.2, "small upward loft: {}", dir.y);
        assert!((dir.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn lands_on_raised_and_lowered_targets() {
        let from = Vec3::new(0.0, 1.0, 0.0);
        assert!(miss_at_target(from, Vec3::new(6.0, 3.0, 2.0), 20.0, 9.8) < 1e-3);
        assert!(miss_at_target(from, Vec3::new(-5.0, 0.0, 4.0), 20.0, 9.8) < 1e-3);
    }

    #[test]
    fn zero_gravity_is_the_straight_line() {
        let dir = ballistic_launch_dir(Vec3::ZERO, Vec3::new(3.0, 4.0, 0.0), 20.0, 0.0);
        assert!((dir - Vec3::new(0.6, 0.8, 0.0)).length() < 1e-5);
    }

    #[test]
    fn out_of_range_falls_back_to_a_45_degree_lob() {
        // Max range at 10 u/s, g 9.8 is ~10.2 m; ask for 100 m.
        let dir = ballistic_launch_dir(Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0), 10.0, 9.8);
        assert!((dir.y - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    }
}
