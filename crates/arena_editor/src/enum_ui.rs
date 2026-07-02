//! Index<->constructor helpers for the obelisk cast enums that do NOT derive `PartialEq`
//! (they carry `f32` payloads), so an egui combo-box can round-trip a selected index to a
//! freshly-constructed variant with sensible defaults. `HitFilter`/`HitMode`/`WindowPhase`
//! derive `Eq`, so the panel drives those with `selectable_value` directly.

use obelisk_bevy::assets::{CastDelivery, CastTargeting, CollisionShape, VolumeMotion};

pub const TARGETING_LABELS: [&str; 4] = ["SelfCast", "SingleEntity", "Direction", "Cone"];
pub fn targeting_index(t: &CastTargeting) -> usize {
    match t {
        CastTargeting::SelfCast => 0,
        CastTargeting::SingleEntity { .. } => 1,
        CastTargeting::Direction { .. } => 2,
        CastTargeting::Cone { .. } => 3,
    }
}
pub fn targeting_variant(i: usize) -> CastTargeting {
    match i {
        0 => CastTargeting::SelfCast,
        2 => CastTargeting::Direction { range: 15.0 },
        3 => CastTargeting::Cone {
            angle: 90.0,
            range: 5.0,
        },
        _ => CastTargeting::SingleEntity { range: 15.0 },
    }
}

pub const DELIVERY_LABELS: [&str; 3] = ["Melee", "Instant", "Projectile"];
pub fn delivery_index(d: &CastDelivery) -> usize {
    match d {
        CastDelivery::Melee => 0,
        CastDelivery::Instant => 1,
        CastDelivery::Projectile { .. } => 2,
    }
}
pub fn delivery_variant(i: usize) -> CastDelivery {
    match i {
        0 => CastDelivery::Melee,
        2 => CastDelivery::Projectile { speed: 20.0 },
        _ => CastDelivery::Instant,
    }
}

pub const SHAPE_LABELS: [&str; 3] = ["Sphere", "Capsule", "Cone"];
pub fn shape_index(s: &CollisionShape) -> usize {
    match s {
        CollisionShape::Sphere { .. } => 0,
        CollisionShape::Capsule { .. } => 1,
        CollisionShape::Cone { .. } => 2,
    }
}
pub fn shape_variant(i: usize) -> CollisionShape {
    match i {
        1 => CollisionShape::Capsule {
            radius: 0.4,
            height: 1.0,
        },
        2 => CollisionShape::Cone {
            angle: 90.0,
            range: 3.0,
        },
        _ => CollisionShape::Sphere { radius: 0.5 },
    }
}

pub const MOTION_LABELS: [&str; 4] = ["Static", "Linear", "Ballistic", "Beam"];
pub fn motion_index(m: &VolumeMotion) -> usize {
    match m {
        VolumeMotion::Static => 0,
        VolumeMotion::Linear { .. } => 1,
        VolumeMotion::Ballistic { .. } => 2,
        VolumeMotion::Beam => 3,
    }
}
pub fn motion_variant(i: usize) -> VolumeMotion {
    match i {
        1 => VolumeMotion::Linear { speed: 20.0 },
        2 => VolumeMotion::Ballistic {
            speed: 20.0,
            gravity: 9.8,
        },
        3 => VolumeMotion::Beam,
        _ => VolumeMotion::Static,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targeting_index_and_variant_round_trip() {
        assert_eq!(targeting_index(&CastTargeting::SelfCast), 0);
        assert_eq!(
            targeting_index(&CastTargeting::Cone {
                angle: 90.0,
                range: 5.0
            }),
            3
        );
        assert!(matches!(
            targeting_variant(1),
            CastTargeting::SingleEntity { .. }
        ));
        assert_eq!(targeting_index(&targeting_variant(2)), 2);
    }

    #[test]
    fn delivery_index_and_variant() {
        assert_eq!(delivery_index(&CastDelivery::Projectile { speed: 20.0 }), 2);
        assert!(matches!(delivery_variant(0), CastDelivery::Melee));
    }

    #[test]
    fn shape_index_and_variant() {
        assert_eq!(
            shape_index(&CollisionShape::Capsule {
                radius: 0.4,
                height: 1.0
            }),
            1
        );
        assert_eq!(
            shape_index(&CollisionShape::Cone {
                angle: 90.0,
                range: 3.0
            }),
            2
        );
        assert!(matches!(shape_variant(0), CollisionShape::Sphere { .. }));
    }

    #[test]
    fn motion_index_and_variant() {
        assert_eq!(motion_index(&VolumeMotion::Linear { speed: 20.0 }), 1);
        assert!(matches!(motion_variant(0), VolumeMotion::Static));
        assert_eq!(
            motion_index(&VolumeMotion::Ballistic {
                speed: 20.0,
                gravity: 9.8
            }),
            2
        );
        assert!(matches!(
            motion_variant(2),
            VolumeMotion::Ballistic { .. }
        ));
    }
}
