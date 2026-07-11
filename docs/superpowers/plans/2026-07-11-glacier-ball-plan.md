# Glacier Ball — a physical rolling ice boulder for `glacier_roll`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give rolling_glacier's grounded roll a VISIBLE, physically-present ice boulder — a
replicated skill object (the frost_spire pattern) that spawns when the roll window opens, travels
in lockstep with the roll hitbox, spins like a rolling ball, and despawns exactly when the roll
ends (wall or fuse). Damage/painting stay 100% obelisk.

**Architecture:** Pure arena change (extension points 1+2 from CLAUDE.md — cue verbs + skill
objects). NO obelisk changes, NO wire schema changes (`NetworkedSkillObject` already replicates
kind+pose). Server: two new verb arms on `glacier_roll`'s window-open/end cues; the ball is a
KINEMATIC body with `LinearVelocity` = the roll hitbox's own direction × 7.0 (obelisk Linear and
avian both integrate at 60 Hz ⇒ lockstep; the end-cue despawn re-syncs any residue). Client: a
kind-keyed icy sphere with CLIENT-SIDE spin (30 Hz replicated `Rotation` would alias at ~3.5
rev/s; position-delta spin is smooth).

**Decisions (user AFK — recommended defaults applied, revisit on request):**
- **No projectile blocking (deferred):** the ball travels INSIDE its own damage hitbox — joining
  the world-hit geometry like a settled spire would burst every roll on its own boulder
  instantly. A safe "rolling shield" needs a companion-exclusion seam in
  `arena_sim::obelisk::report_world_hits`; that is a named follow-up, not this plan.
- **No player push:** a shove could push the victim out of the roll hitbox's overlap band
  mid-hit and break the glacier gate's damage/death assertions. The ball collides with NOTHING
  (`CollisionLayers` with no memberships — invisible to world-hit rays, portal passthrough,
  grounded rays, and the decal ground-snap ray, which accepts `RigidBody::Static` only anyway).

## Known data (verified against the tree)

`assets/skills/glacier_roll.cast.ron`: window id `roll`, `Sphere(radius: 0.32)` at
`anchor_offset (0, 0.35, 0)` (⇒ an r=0.32 ball at hitbox Y sits ~flush on the floor),
`Linear(speed: 7.0)`, `motion_direction: Horizontal`, `active_duration: 6.5`,
`vfx_cues: { "on_hit": "on_hit" }` — **`on_window_roll` / `on_end_roll` are NOT emitted today**
(a cue slot must be listed in `vfx_cues` for obelisk to emit the `CueEvent` the verb observes;
`cue_id == slot`). The roll direction is the INHERITED flattened impact direction — the cue's
stamped `aim_dir` is the caster's CURRENT look and is WRONG for the ball; read the direction off
the just-spawned roll `Hitbox` (it carries `aim` — the portal teleport remaps it, so the field
exists) correlated by caster + proximity to the cue position.

## Global Constraints

- obelisk-arena tree carries USER WIP (`assets/skills/blizzard.cast.ron`, `assets/vfx/*` mods,
  untracked `Particle_ *`) — **NEVER `git add -A`**; exact paths. `glacier_roll.cast.ron` is
  clean (not user WIP) and IS a task file.
- NO obelisk-bevy changes. Collision layers are part of the prediction contract (CLAUDE.md
  invariant 16) — the ball's no-membership layers must never intersect Player/Ground.
- Glacier gate: assertions (1)-(9) stay untouched; this plan ADDS (10). Tune session params only,
  never assertions. Gate PASS ×2 consecutive + firebolt gate ×1 at the end.
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

---

### Task 1 (obelisk-arena, branch `glacier-ball`): the rolling boulder, server + client + gate

**Files:**
- Modify: `assets/skills/glacier_roll.cast.ron` (vfx_cues), `crates/arena_game/src/server/skill_objects.rs`
  (kind + cap), `crates/arena_game/src/server/verbs.rs` (two arms + consts),
  `crates/arena_game/src/client/skill_objects.rs` (recipe + spin), `crates/arena_game/tests/content_wisp_ports.rs`
  (cue-pair pin), `crates/arena_game/tools/net-test/check_glacier_session.sh` (assertion (10)).

**Interfaces:** consumes `spawn_skill_object` (per-kind caps, optional `(RigidBody, Collider)`,
replication) and `skill_verbs_on_cue`'s `(skill_id, cue_id)` match; produces
`KIND_GLACIER_BALL: &str = "glacier_ball"` and client visuals keyed on it.

- [ ] **Step 1 — RED: pin the cue pairs.** In `tests/content_wisp_ports.rs` (follow its existing
  pin idiom for spire/portals), assert `glacier_roll`'s loaded `CastTimeline.vfx_cues` contains
  the `on_window_roll` AND `on_end_roll` slots. Run: FAILS (only `on_hit` exists today).
- [ ] **Step 2 — GREEN: emit the cues.** Add to `glacier_roll.cast.ron`'s `vfx_cues` map (match
  the file's identity-mapping style): `"on_window_roll": "on_window_roll", "on_end_roll":
  "on_end_roll"`. Do NOT add `cues:` bindings (no client cosmetic — the ball is the visual).
  Content test passes. Also verify no client warn-spam from the now-broadcast unbound cues
  (`cue_unbound` is warn-once by design; confirm, note in report).
- [ ] **Step 3 — the kind.** `server/skill_objects.rs`: `pub const KIND_GLACIER_BALL: &str =
  "glacier_ball";` + cap arm `max_instances: 4` replace-oldest (roll lasts 6.5 s; the gate's 3:1
  rotation can have ~3 concurrent rolls per caster).
- [ ] **Step 4 — the verb arms.** `server/verbs.rs` in `skill_verbs_on_cue`:
  - `("glacier_roll", "on_window_roll")`: resolve the roll direction — query live obelisk
    `Hitbox`es for the one belonging to this caster nearest the cue position (≤ 1.0 m; it just
    spawned there), take its `aim` flattened to the XZ plane and normalized; fall back to the
    cue's stamped aim flattened (log a trace note) if none found. Then
    `spawn_skill_object(KIND_GLACIER_BALL, caster, cue_pos, RigidBody::Kinematic,
    Collider::sphere(GLACIER_BALL_RADIUS = 0.32), CollisionLayers::NONE-memberships,
    LinearVelocity(dir * GLACIER_BALL_SPEED = 7.0), lifetime ROLL fuse 6.5 + 0.5 margin)` —
    adapt to `spawn_skill_object`'s actual signature; if it can't carry velocity/layers, insert
    them on the returned entity. Trace rides the existing `skill_object_spawned`.
  - `("glacier_roll", "on_end_roll")`: despawn this caster's `KIND_GLACIER_BALL` object NEAREST
    the cue's end position (≤ 1.5 m; lockstep travel ⇒ it's standing right there); none found
    (evicted/expired) = no-op. This is what stops the ball AT walls — obelisk ends the roll on
    world hit, the cue fires at that position, the ball despawns there.
- [ ] **Step 5 — client visual + spin.** `client/skill_objects.rs`: recipe arm for
  `KIND_GLACIER_BALL` — `Sphere::new(0.32)` mesh, icy material tinted to match the new frost
  blue (`base_color` ≈ (0.35, 0.6, 1.0), slight emissive, perceptual_roughness low-ish), plus
  optional `skill_object_glacier_ball` VfxLibrary preset lookup (the existing kind→vfx pattern —
  no preset authored now, the lookup just no-ops). Spin: pure helper
  `roll_spin(delta: Vec3, radius: f32) -> Option<(Dir3, f32)>` returning axis
  `Vec3::Y.cross(dir)` and angle `|delta|/radius` (None for ~zero delta), applied per-frame to
  the MESH CHILD from the replicated root's position delta (root pose stays
  `mirror_skill_object_pose`'s).
  **Unit test the helper** (same file, `#[cfg(test)]`): for travel `+X`, the returned axis/angle
  must rotate the ball's TOP point (+Y) toward `+X` (forward) — i.e. rolling, not backspin:
  `Quat::from_axis_angle(axis, angle) * Vec3::Y` has positive X for a small positive delta.
  Write the test FIRST (RED against a stub returning None), then implement.
- [ ] **Step 6 — gate assertion (10).** `check_glacier_session.sh`: after (9), add
  `(10) ≥2 skill_object_spawned` with the glacier_ball kind on the server log (verify the trace's
  actual field name for kind in `spawn_skill_object` — jq accordingly), failure note
  `"no rolling boulder spawned (glacier_ball skill objects missing)"`; extend the echo with
  `balls=<n>`; update the header list to (1)-(10).
- [ ] **Step 7 — verify.** `cargo test -p arena_game` (content pin + spin helper + all existing);
  glacier gate ×2 consecutive PASS (REBUILD first run — server+client binaries changed; pkill -x
  between; ≤3 retries each; paste echo lines); firebolt gate ×1 (verbs.rs touched — insurance).
- [ ] **Step 8 — commit** (exact paths: the 6 task files):
```
feat(skills): rolling glacier gets a physical ice boulder — kinematic skill object in lockstep with the roll

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

---

## Post-plan notes (controller)

- Merge + push per the standing pattern after review.
- The EDITOR preview will NOT show the boulder (verbs are arena-server extension points — same
  parity as spires/portals); the user sees it in the game client. Say so in the final summary.
- Named follow-up (only if the user asks): "rolling shield" — projectiles bursting on the ball
  needs a companion-exclusion seam in `report_world_hits` so a roll never bursts on its own ball.
- User-visible acceptance: lob a rolling_glacier — an ice boulder rolls along the frost trail,
  spinning forward, stopping at walls/fuse-end exactly where the trail stops.
