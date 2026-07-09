//! Transport-agnostic arena spawn primitives shared by the live game (`arena_game`) and the editor
//! preview (`arena_editor`): the static floor, the two fixed spawn markers, the `faction_for_slot`
//! slot→faction rule, and the `make_arena_combatant` recipe (an obelisk combatant + Dynamic avian
//! body + a CHILD `Hurtbox` sensor — NO networking, NO `grant_skill`). The game layers replication +
//! `grant_skill` on top; the editor preview uses the bare combatant.

use avian3d::prelude::*;
use bevy::prelude::*;
use obelisk_bevy::prelude::*;
use stat_core::StatBlock;

use crate::tuning::{GROUND_Y, PLAYER_CAPSULE_LENGTH, PLAYER_CAPSULE_RADIUS};

/// The two fixed arena spawn markers (spec §11 hard-coded geometry). Players are placed by slot:
/// slot 0 at marker 0, slot 1 at marker 1 — facing each other across the +Z axis.
pub const SPAWN_MARKERS: [Vec3; 2] = [
    Vec3::new(-4.0, GROUND_Y, 0.0),
    Vec3::new(4.0, GROUND_Y, 0.0),
];

/// Slot→faction rule: slot 0 → `Faction::Player`, every other slot → `Faction::Enemy`. With obelisk's
/// 3-faction model a 2-player duel lands slots 0/1 on mutually-opposing factions so firebolt's
/// `Enemies` hit-filter can resolve a hit (a shared faction would resolve zero hits).
pub fn faction_for_slot(slot: usize) -> Faction {
    if slot == 0 {
        Faction::Player
    } else {
        Faction::Enemy
    }
}

/// Spawn the STATIC arena floor collider on this peer. The Dynamic player bodies rest on it. A cuboid
/// spanning `±FLOOR_SIZE/2` horizontally + 1 m thick, positioned so its TOP face is at world Y = 0;
/// a capsule(0.35, 0.48) body (half-height 0.59) then rests with its origin at [`GROUND_Y`] = 0.59
/// and its feet at world 0. Spawned locally (identical on every peer — static geometry isn't
/// replicated).
pub fn spawn_arena_floor(commands: &mut Commands) {
    const FLOOR_SIZE: f32 = 40.0;
    const FLOOR_THICKNESS: f32 = 1.0;
    commands.spawn((
        Name::new("ArenaFloor"),
        RigidBody::Static,
        Collider::cuboid(FLOOR_SIZE, FLOOR_THICKNESS, FLOOR_SIZE),
        Position(Vec3::new(0.0, -FLOOR_THICKNESS / 2.0, 0.0)),
        Rotation::default(),
    ));
}

/// Build one arena combatant: a full obelisk combatant (`make_combatant(StatBlock::with_id(...))`,
/// which sets `Combatant`/`Attributes`/`ObeliskId`) + the requested `Faction` + a server-authoritative
/// Dynamic avian body (rotation axes locked, zero friction so the shared force controller owns planar
/// velocity) + a CHILD `Hurtbox` `Sensor` capsule. The hurtbox lives on a CHILD (not the root) so the
/// root stays `RigidBody::Dynamic` — avian attaches the child capsule as a compound collider that
/// tracks the moving body and stays in the `SpatialQuery` pipeline (obelisk's `detect_overlaps`
/// resolves the child to its `Hurtbox.owner`).
///
/// This is the bare, transport-agnostic recipe: it does NOT add any networked/`Replicate` components
/// and does NOT `grant_skill`. The live game ([`arena_game`]) layers those on the returned entity;
/// the editor preview uses the bare combatant.
pub fn make_arena_combatant(
    commands: &mut Commands,
    obelisk_id: &str,
    faction: Faction,
    spawn: Vec3,
) -> Entity {
    let player = commands
        .spawn_empty()
        .make_combatant(StatBlock::with_id(obelisk_id))
        .insert((
            faction,
            Position(spawn),
            Rotation::default(),
            LinearVelocity::default(),
            AngularVelocity::default(),
            RigidBody::Dynamic,
            Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH),
            // Player layer with ALL filters — the portal floor pass-through toggles the Ground
            // bit off while the body stands in a floor-portal slab (must stay identical to the
            // client's predicted body layers or prediction desyncs).
            avian3d::prelude::CollisionLayers::new(
                crate::GameLayer::Player,
                avian3d::prelude::LayerMask::ALL,
            ),
            LockedAxes::default()
                .lock_rotation_x()
                .lock_rotation_y()
                .lock_rotation_z(),
            Friction::new(0.0).with_combine_rule(CoefficientCombine::Min),
        ))
        .id();
    commands.spawn((
        Name::new("Hurtbox"),
        Hurtbox { owner: player },
        Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH),
        Sensor,
        Transform::default(),
        ChildOf(player),
    ));
    player
}

/// Re-assert every hurtbox child's AUTHORED pose each tick (before physics): identity local
/// `Transform`, identity `ColliderTransform`, and world `Position`/`Rotation` = its owner body's.
/// The hurtbox is by construction a fixed-offset (zero-offset) child sensor — nothing may ever
/// move it relative to its body.
///
/// Why this exists: the live game disables avian's `PhysicsTransformPlugin` (lightyear's avian
/// Position-mode sync replaces it), which ALSO disables the system that recomputes a child
/// collider's `ColliderTransform` from its `Transform` — so `ColliderTransform` is captured ONCE
/// at attach and then frozen. The lobby flow can teleport players within milliseconds of their
/// spawn (instant host start; level switch), and lightyear's `position_to_transform` scribbles a
/// WORLD pose into the child's local `Transform` before the capture — freezing a multi-meter
/// bogus offset into the collider. Every firebolt then flies straight through the target and no
/// damage ever resolves (the net-test caught this). Pinning `ColliderTransform` (and the rest)
/// every tick makes the authored pose the only possible steady state.
#[allow(clippy::type_complexity)]
pub fn pin_hurtboxes_to_bodies(
    bodies: Query<(&Position, &Rotation), With<RigidBody>>,
    mut hurtboxes: Query<
        (
            &Hurtbox,
            &mut Transform,
            Option<&mut ColliderTransform>,
            Option<&mut Position>,
            Option<&mut Rotation>,
        ),
        (With<Sensor>, Without<RigidBody>),
    >,
) {
    for (hurt, mut tf, ctf, pos, rot) in &mut hurtboxes {
        if *tf != Transform::IDENTITY {
            *tf = Transform::IDENTITY;
        }
        if let Some(mut ctf) = ctf {
            if ctf.translation != avian3d::math::Vector::ZERO {
                ctf.translation = avian3d::math::Vector::ZERO;
                ctf.rotation = Rotation::default();
            }
        }
        let Ok((body_pos, body_rot)) = bodies.get(hurt.owner) else {
            continue;
        };
        if let Some(mut p) = pos {
            if p.0 != body_pos.0 {
                p.0 = body_pos.0;
            }
        }
        if let Some(mut r) = rot {
            if *r != *body_rot {
                *r = *body_rot;
            }
        }
    }
}
