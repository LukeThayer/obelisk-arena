//! Geometry + physics tuning constants shared by the live game and the editor preview.
//!
//! Every value here is BYTE-IDENTICAL to the pre-extraction `arena_game::net` constants; this
//! module is now the single source of truth and `arena_game` re-exports them.

/// The stand height: world Y of a grounded player ORIGIN. The Dynamic body is `capsule(0.35, 0.48)`
/// (half-height 0.59) resting on the static arena floor (top at world 0), so its origin settles at
/// ≈0.59 and its feet at world 0. Used by the shared controller's ground check
/// (`shared_controller::apply_arena_movement`) — a player meaningfully above this is airborne — and
/// as the Y of `spawn::SPAWN_MARKERS`. With a real floor collider this is the EMERGENT rest height,
/// not a clamp.
pub const GROUND_Y: f32 = 0.59;

/// Magnitude of the arena's avian `Gravity` (m/s²), set in `add_avian_with_lightyear`. Snappier than
/// Earth for an arcade jump arc; with `shared_controller::JUMP_SPEED` (7 m/s) the apex is ≈1.22 m
/// (pinned by `jump_apex_matches_documented_height`).
pub const GRAVITY: f32 = 20.0;

/// Camera eye height above the player root (world units). Shared by BOTH the first-person camera
/// placement (`client::controller::EYE_HEIGHT`) and the server muzzle offset
/// (`server::cast_pipeline::drain_cast_requests`). Because the camera sits at `origin + Y*ARENA_EYE_HEIGHT` and the
/// firebolt now spawns at `origin + Y*ARENA_EYE_HEIGHT`, the bolt originates at the eye and travels
/// along `aim_dir` (camera forward) — so the crosshair ray IS the bolt path and a shot aimed at the
/// opponent connects. Defined once here so the two values can never drift apart.
///
/// MEASURED geometry (the `character.glb` body AABB, feet-rooted): the model is ~1.18 tall, so with
/// the player origin at the BODY CENTER (see `GROUND_Y` / `present::RIG_FOOT_OFFSET`) the feet sit at
/// origin Y−0.59 and the head at origin Y+0.59. `+0.5` puts the eye just below the top of the head =
/// natural first-person eye level. The HURTBOX capsule (`server::spawn::spawn_player_on_connect`) spans origin
/// Y±0.59 = feet→head, so the eye-height muzzle fires from inside the body span and a level shot lands.
pub const ARENA_EYE_HEIGHT: f32 = 0.5;

/// The CASTING-HAND launch offset, in the caster's LOCAL aim frame (+X = right, +Y = up from
/// the body center, -Z = forward): where projectiles/windows SPAWN — matching the rig socket
/// (`R_wrist_joint`) the charge tiers + on_cast cues anchor to. Rotated by the caster's yaw and
/// passed as obelisk's `PendingCast.muzzle_offset` (consumed only at window spawn). AIM
/// acquisition keeps the camera-accurate EYE origin; the launch carries the standard FPS muzzle
/// parallax. Lateral 0.32 stays inside the bolt-radius + hurtbox-radius overlap band so a
/// centered flat aim still lands (the net-test's firebolt depends on it). Shared here so the
/// EDITOR's preview casts launch from the identical point.
pub const CAST_HAND_OFFSET: bevy::math::Vec3 = bevy::math::Vec3::new(0.32, 0.55, -0.25);

/// Player body capsule dimensions, used IDENTICALLY by all three spawns — the server authoritative
/// body, the server hurtbox child, and the client predicted body — so they can never desync (a
/// mismatch breaks prediction/hurtbox alignment). `capsule(radius, length)` ⇒ half-height =
/// length/2 + radius = 0.24 + 0.35 = 0.59 = [`GROUND_Y`]. Player mass is implicit (avian default
/// density) and intentionally cancels in the controller (`force = accel*mass`, `impulse = dv*mass`).
pub const PLAYER_CAPSULE_RADIUS: f32 = 0.35;
pub const PLAYER_CAPSULE_LENGTH: f32 = 0.48;
