# Glacier Physics Ball — Dynamic from cast, wisp-faithful, obelisk windows pinned to the ball

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** rolling_glacier's boulder becomes a REAL `RigidBody::Dynamic` sphere that exists from
the cast moment — launched flat from the hand at the charge-scaled throw speed, arcing under real
gravity, bouncing/rolling with wisp's exact physics parameters, SHOVING players, and traversing
portals — while obelisk keeps 100% authority over damage, trail painting, and the skill chain by
having its collision windows PINNED to the ball instead of self-moving.

**The normative reference is WISP** (`~/src/wisp` — the original this skill was ported from; the
user directed "the same way it works in wisp"). Verified wisp facts to port EXACTLY:
- `assets/bodies/glacier_ball.body.ron`: Dynamic `Sphere(radius: 0.32)`, **mass 6.0, friction
  0.2, restitution 0.4, linear_damping 0.05, angular_damping 0.05**; material base_color
  (0.55, 0.85, 1.0), emissive (0.5, 1.1, 1.8), roughness 0.15; **point light** (color =
  base_color, intensity 28000, range 9.0); markers `[PortalTraveler, RollingGlacier]`.
- `assets/spells/rolling_glacier.spell.ron`: launch = `Throw(forward: 9.0, up: 0.0)` — FLAT
  throw, gravity makes the arc; expiry = flat 8 s timeout that bursts WHEREVER the ball is,
  moving or not — **wisp has NO rest detector**; charge scales damage (rules-side), cooldown 0.8.
- `src/spells/ice.rs`: trail step 0.8 / tile radius 0.45 / dedup 0.25 / contact damage 28 — all
  ALREADY preserved in the arena's obelisk content; spikes are heavy (20 kg base) specifically so
  they SHOVE the 6 kg ball (spire↔ball physics is intended wisp behavior).

**Architecture (the authority flip):** avian owns the ball's trajectory; obelisk owns rules. The
arena already pins hurtboxes to bodies every tick — the same pattern pins the flight/roll
hitboxes to the ball. Phase transitions keep obelisk semantics, FIRED BY PHYSICS where wisp is
physics-driven: `report_world_hits` (arena_sim) proves the host fires obelisk's world-hit event
externally — landing = the ball's first Ground contact fires HitWorld on the flight hitbox
(→ rules chain `glacier_roll` exactly as today); roll end = the 6.5 s FUSE ONLY (wisp-faithful:
burst wherever the ball is — walls BOUNCE and bank shots keep painting; a settled ball sits as a
hazard until the fuse). NO obelisk-bevy changes.

**Deliberate divergences from wisp (state in code comments where relevant):**
- Damage stays obelisk `OncePerTarget` per window (wisp re-hits per contact edge). Rules are
  obelisk's; in 1v1 the difference is marginal. Do NOT change hit_mode.
- The flight/roll two-window chain stays (wisp has one phase); trail starts at first ground
  contact — behaviorally ≈ wisp's flat throw, which reaches ground in <1 s anyway.
- Snowball growth on foreign ice: STILL DEFERRED (the original port deferral stands; not asked).
- The expire-time wide tile + 3.5 AoE stay as the arena's `glacier_burst` content (rules-side,
  untouched).

**User decisions (2026-07-12):** real boulder physics + full player shove + ball exists from
cast + "the same way it works in wisp".

## Consequences to encode honestly

- The old cosmetic lob projectile must be retired (else two balls); one's own cast shows the
  replicated ball ~1 replication interval late — accepted; a predicted ghost-ball is a named
  polish follow-up, NOT this plan.
- Last round's roll-phase kinematic ball is SUPERSEDED: one Dynamic ball per cast, spawned at
  the flight window's open cue, RE-pinned (not respawned) at the roll cue, despawned at the roll
  end cue (fuse-driven burst).
- Full shove requires CLIENT-side collision parity (a server-only shove rubber-bands predicted
  players): the replicated ball gets a kinematic mirror collider on clients with IDENTICAL
  layers, headless included (gameplay parity, not visuals).
- The shove can push victims off the roll line — the glacier gate session may need retuning
  (params ONLY; assertions (1)-(10) stay byte-identical; document every tune).
- A direct enemy hit in flight still ends the flight window (HitEntity → chain at the victim);
  the ball keeps flying and the roll re-pins to it — trail resumes where the ball actually is.
- Spires are Static terrain: the ball bounces off them (and a rising spire can launch it) — this
  is wisp-intended, leave it emergent.

## Global Constraints

- obelisk-arena tree carries USER WIP (`assets/skills/blizzard.cast.ron`, `assets/vfx/*` mods,
  untracked `Particle_ *`) — **NEVER `git add -A`**; exact paths. `rolling_glacier.cast.ron` +
  `glacier_roll.cast.ron` are task files (NOT user WIP).
- NO obelisk-bevy changes. Collision layers are part of the prediction contract (CLAUDE.md
  invariant 16): server ball layers ≡ client mirror layers, on BOTH peers, or shoves desync.
- Glacier gate assertions (1)-(10) byte-untouched; firebolt gate ×1; **`run_conditioned.sh` ×1**
  (prediction contract touched — CLAUDE.md mandates it); glacier ×2 consecutive PASS.
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

---

### Task 1 (obelisk-arena, branch `glacier-physics-ball`): the whole flip

**Files:**
- Modify: `assets/skills/rolling_glacier.cast.ron` (flight window → `motion: Static`, add
  `on_window_<flight-id>` to vfx_cues, REMOVE/rebind the cosmetic flight projectile lane),
  `assets/skills/glacier_roll.cast.ron` (roll window → `motion: Static`),
  `crates/arena_game/src/server/verbs.rs` (rework the glacier arms),
  `crates/arena_game/src/server/skill_objects.rs` (ball cap → effectively 1 live ball per caster;
  keep replace-oldest), NEW `crates/arena_game/src/server/glacier_ball.rs` (pin system + landing
  detector; registered from `server/mod.rs`), `crates/arena_game/src/server/portals.rs` (ball as
  portal traveler — see Step 6), `crates/arena_game/src/client/skill_objects.rs` (mirror
  collider + wisp material/light; retire the cosmetic spin if replicated rotation suffices),
  `crates/arena_game/tests/content_wisp_ports.rs` (pin the new flight cue slot),
  `crates/arena_game/tools/net-test/run_glacier_session.sh` (retunes, documented),
  `crates/arena_game/tools/net-test/check_glacier_session.sh` (echo field only if needed —
  assertions untouched).

**Step order:**

- [ ] **Step 0 — read the seams before writing** (findings in the report): (a)
  `arena_sim::obelisk::report_world_hits` — the EXACT event fired into obelisk (name/fields) so
  the landing detector fires the identical one; (b) avian 0.5 collision events server-side under
  the game's composition (`CollisionStarted` reader or contact pairs); (c) `GameLayer`'s home +
  members (likely arena_sim — editable; if it were obelisk-bevy, STOP); (d)
  `pin_hurtboxes_to_bodies` ordering — the ball pin must write hitbox Transforms BEFORE obelisk
  overlap detection + trail painting each FixedUpdate; (e) rolling_glacier's cosmetic flight
  lane binding (what to remove); (f) avian 0.5 mass API (explicit `Mass` vs `ColliderDensity`)
  to hit wisp's mass 6.0 exactly; (g) `portal_teleport`'s traveler queries (players +
  projectiles today) and `portals_shared`'s virtual-transform/velocity_rotation helpers.
- [ ] **Step 1 — content (RED→GREEN).** Content test requires rolling_glacier's flight-window
  `on_window_<id>` slot in vfx_cues (RED) → add it + flip BOTH windows' motion to `Static` +
  remove the cosmetic projectile lane (GREEN). Damage/chain/paints TOML untouched.
- [ ] **Step 2 — the ball (wisp body).** Verb arm `("rolling_glacier", "on_window_<flight-id>")`:
  spawn ONE `KIND_GLACIER_BALL` at the cue position (the hand), `RigidBody::Dynamic`,
  `Collider::sphere(0.32)`, **wisp physics verbatim**: mass 6.0 (Step 0f API), `Friction::new(0.2)`,
  `Restitution::new(0.4)`, `LinearDamping(0.05)`, `AngularDamping(0.05)`; launch
  `LinearVelocity(flat(aim) * 9.0 * charge_mult(ev.charge))` where `flat(aim)` is the aim
  flattened to XZ + normalized (wisp throws FLAT, `up: 0.0` — the hand height provides the drop;
  do NOT launch along the pitched eye ray). Layers: member `<ball layer per Step 0c>`, filters
  `Ground | Player`. Verify and STATE in the report that `report_world_hits`' ray cannot see the
  ball (else the flight hitbox bursts on its own body). Link the flight Hitbox via a
  `PinnedBall`-style component (same caster+skill+proximity correlation as the existing arm).
- [ ] **Step 3 — the pin system** (glacier_ball.rs, FixedUpdate BEFORE obelisk overlap/trail):
  write each pinned ball's avian `Position` into its hitbox's Transform (windows are `Static`;
  the pin is the sole mover — trail painting + overlap detection read pinned positions and Just
  Work). Unit-test extractable pure helpers.
- [ ] **Step 4 — landing detector + re-pin.** First ball↔Ground contact while the FLIGHT hitbox
  is live → fire the Step-0a world-hit event on the flight hitbox at the contact point → obelisk
  chains glacier_roll (TOML untouched). The `("glacier_roll", "on_window_roll")` arm becomes
  RE-PIN (retarget the link to the new roll Hitbox; keep the fallback trace). No new ball. Roll
  END is the 6.5 s fuse ONLY (wisp parity — no rest detector); the `on_end_roll` despawn arm is
  unchanged except clearing the pin link.
- [ ] **Step 5 — client mirror + wisp visuals.** Glacier-ball recipe: ADD `RigidBody::Kinematic`
  + `Collider::sphere(0.32)` + IDENTICAL layers on the replicated entity (headless too —
  gameplay parity so predicted players collide with the mirrored ball and shoves don't
  rubber-band); material → wisp's (base_color (0.55, 0.85, 1.0), emissive (0.5, 1.1, 1.8) ×
  reasonable bevy emissive scaling, roughness 0.15) + a `PointLight` (28000 lm, range 9.0,
  shadows off) — windowed-only for the light/mesh, ALWAYS for the collider mirror. Replicated
  rotation now carries real rolling: delete the cosmetic spin system + helper + test if
  redundant.
- [ ] **Step 6 — portal traversal (wisp `PortalTraveler`).** Extend `server/portals.rs::
  portal_teleport`'s traveler handling to `KIND_GLACIER_BALL` skill objects: crossing detection
  via `portals_shared::projectile_crossing` on the ball's segment, remap Transform/Position via
  the virtual transform + `velocity_rotation` on `LinearVelocity` (no `Hitbox.aim` — the pin
  drags the obelisk hitbox through, so the trail continues on the far side). Keep it MINIMAL —
  reuse the shared math; if this step fights the portal state machinery for more than a bounded
  effort, DROP it to a named follow-up and say so (the rest of the plan must not be hostage).
- [ ] **Step 7 — verify + retune.** `cargo test -p arena_game`; glacier gate ×2 consecutive PASS
  (REBUILD; the shove will move the target — expect pitch/duration/position retunes, document
  each; assertions byte-untouched); firebolt ×1; `run_conditioned.sh` ×1. Paste all echo lines.
- [ ] **Step 8 — commit** (exact paths of every touched file):
```
feat(skills): rolling glacier is a real Dynamic boulder from cast — wisp physics, obelisk windows pinned

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

**STOP rules:** if `GameLayer` lives in obelisk-bevy, or the Step-0a event cannot be fired
externally without obelisk changes, or pinned-Static-window trail/overlap demonstrably doesn't
fire — STOP, DONE_WITH_CONCERNS, full diagnosis.

---

## Post-plan notes (controller)

- Merge + push per the standing pattern after review.
- User-visible acceptance (game): the boulder leaves the hand the moment the cast fires, arcs
  flat-thrown under gravity, bounces/rolls with wisp's exact feel (heavy, low-friction, lively
  restitution), shoves players, glows (emissive + light), paints frost wherever it ACTUALLY
  rolls (bank shots included), goes through portals, and bursts at the fuse wherever it is.
  Editor preview still shows no boulder (arena-server machinery).
- Named follow-ups (only if asked): predicted ghost-ball; snowball growth on foreign ice
  (wisp has it, port deferral stands); rolling-shield (world-hit companion exclusion).
