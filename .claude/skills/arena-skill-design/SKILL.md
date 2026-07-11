---
name: arena-skill-design
description: Use when the user wants to add a new obelisk-arena skill or spell ("add an ice spike spell", "design chain lightning", "make a fire nova"), rework an existing skill's behavior/cost/feel, or asks what a skill can express in the current sim. Arena-specific — the obelisk `CastTimeline` v2 RON schema + stat_core rules TOML.
---

# Designing an arena skill (obelisk `CastTimeline` **v2**)

> **This skill is the authoring source of truth. The three obelisk specs it used to cite —
> `2026-07-02-skill-anatomy.md`, `-event-driven-skill-phases.md`, `-beam-retarget-hitscan.md` — describe
> a DELETED v1 schema (`on_end: Chain/Retarget`, `WindowPhase::Chained`, `spawn_phase`,
> `targeting:`/`delivery:`, separate `.skillfx.ron`). Under v2's `#[serde(deny_unknown_fields)]` that RON
> HARD-FAILS to load. Do NOT author from those specs.** The full rationale + palette ceilings + editor
> gaps are in `obelisk-arena/docs/superpowers/reviews/2026-07-09-obelisk-skill-system-and-editor-review.md`.

## 0. Ground yourself first

The one authoritative source for the `.cast.ron` schema is the code:
- **`obelisk-bevy/src/assets/mod.rs`** — the `CastTimeline` struct + every enum (`WindowSpawn`,
  `WindowAnchor`, `CollisionShape`, `VolumeMotion`, `MotionDirection`, `HitFilter`, `HitMode`,
  `Acquisition`, `AcqFallback`, `Emitter`, `CueBinding`, `ChargeCue`) and `validate_timeline`.
  `deny_unknown_fields` everywhere: a typo or a stale field fails LOUD at load.
- Current design spec (APPROVED, supersedes the v1 trio):
  `obelisk-bevy/docs/superpowers/specs/2026-07-02-skill-editor-reimplementation-design.md`.
- **Copy a shipped exemplar** (all proven to parse) rather than authoring from scratch:
  `firebolt` (projectile → rules-triggered AoE), `blizzard` (ground-point emitter storm),
  `chain_lightning` (beam + rules chain), `firebolt_explosion` (triggered sub-skill).

sibling checkout: `../obelisk-bevy`, or the pinned cargo checkout under
`~/.cargo/git/checkouts/bevy-obelisk-*/`.

## 1. What the sim can express — and its hard ceilings

**Never promise a mechanic without checking this.** The sim is a deterministic *spatiotemporal delivery +
single-target damage/effect resolver*. If a concept needs something in the "CANNOT" column, say so and
either scope to what ships or propose a sim increment first.

**Expressible today:** phases (windup/active/recovery); scheduled + emitter-spawned (`Template`) windows;
shapes **Sphere / Capsule / Cone** (all really simulate); motion **Static / Linear / Ballistic(+gravity)
/ Beam**; launch-direction override (`Inherit/Down/Horizontal`); hit filters (Caster/Allies/Enemies/All)
and modes (FirstOnly / OncePerTarget / EveryTick+rehit — the last is a damage field); **point-anchored
zones** (`WindowAnchor::CastPoint` + `anchor_offset`) and **carrier volumes** (`strikes:false`);
**emitters** (rain child windows at rate/jitter); **authored acquisition** (Aim / SelfPoint /
HitscanEntity / GroundPoint) with **fallback chains**; **persistent painted surfaces** (a window
`paints` frost/burning patches — `Trail`/`OnEnd`; a `GroundPoint` `on_surface` gate requires/snaps/
consumes one; patches carry standing payloads + skill-contact reactions); charge (byte → 0.5–2.0× speed **and** damage);
rich mitigation; **rules-driven trigger cascades** (a hit fires another skill's whole timeline at the hit
position) and **rules-driven chain** (beam-only). This covers: firebolt-style lob-and-explode, chain
lightning, blizzard/rain-of-shards, ground-targeted meteor, persistent lava field, cone flamethrower/
cleave, PBAoE nova, proximity mine — plus, with surfaces: persistent frost/burning terrain, glacier-
style paint trails, and surface-gated casts (frost spire erupts only on a frost patch).

**CANNOT today (needs a sim increment, not authoring):**
- **Any crowd control / displacement** — no knockback, pull/vortex, root, hard stun, taunt, silence,
  leap/dash/blink. The *only* control primitive is a soft **action-speed slow** as an effect; a
  movement-speed slow is positionally inert.
- **Any damage falloff** — chain hops + AoE deal FULL damage; no radial/per-hop/range taper.
  (`pierce_count` is declarable in rules but wired to no geometry.)
- **Motion beyond the 4 fixed kinds** — no homing/seeking, orbit, boomerang-return, or curve-to-point.
- **Non-round shapes** — no box/wall/line (Cone is a circular sector around the aim axis, not a flat fan).
- **Following/sweeping volumes** — volumes are world-frozen at spawn; no channeled cone that sweeps as you
  turn, no aura that trails the caster (point-anchored zones stay put, which is fine).
- **Bespoke mechanics** (portals, teleport, spawned skill objects) are **arena
  server verbs** (`arena_game/src/server/verbs.rs`, keyed on `(skill_id, cue_id)`), NOT authorable in RON
  and invisible to the editor/preview. If the design needs one, that's an engineering task.

## 2. Decompose the concept (in conversation, before touching files)

- **Fantasy + role**: one sentence — burst / zoning / poke / finisher?
- **Cast**: mana, cooldown, windup/active/recovery seconds, `chargeable` + `max_hold`?
- **Acquisition** (how aim resolves): `Aim` (free-aim along the crosshair), `SelfPoint` (centered on the
  caster), `HitscanEntity` (server raycasts the looked-at enemy; a miss fizzles), `GroundPoint` (a point
  on the floor; the host produces the point via a camera→ground ray) — each fallible one carries a
  `fallback` (`Fizzle` = paid rejection, or `Then(<other acquisition>)`).
- **Windows** (hit regions): for each — shape, motion(+params), `motion_direction`, `active_duration`
  (= the fuse), `hit_filter`, `hit_mode`, `anchor`(+offset), `strikes`, `emitter?`.
- **Causality is RULES-side** (§3a): "on hit/impact/fuse, do X" = a `[[conditions]] trigger_skill` in the
  TOML that fires ANOTHER skill's timeline at the hit position. Chaining = `can_chain`/`chain_count`
  (beam-only).
- **Rules**: damage types/amounts, crit, `effect_applications` (burn — `config/effects/`).
- **Presentation per moment**: cast (muzzle + charge glow), flight (trail), each impact/end.

## 3. Author the triad

One skill = same `id` across all three files. **Two mental-model shifts from v1:** (a) cross-skill
causality lives in the **rules TOML** (trigger conditions), not the behavior graph; (b) presentation is
**inline in the `.cast.ron`** (`cues`/`charge_cues`), not a separate `.skillfx.ron`.

### 3a. Rules — `config/skills/<id>.toml` (`stat_core::Skill`)

```toml
id = "firebolt"
name = "Firebolt"
tags = ["spell", "fire"]                 # drives scaling/filters
targeting = "single_enemy"               # VESTIGIAL in v2 — the real targeting is the timeline
delivery  = "projectile"                 # acquisition. single_enemy/projectile is the safe default.
mana_cost = 5.0
cooldown  = 1.5

[damage]
base_damages = [{ type = "fire", min = 20.0, max = 20.0 }]   # type: physical|fire|cold|lightning|chaos
base_crit_chance = 0.05
# can_chain = true   # beam-only chaining; pair with chain_count and a Beam window (see chain_lightning)
# chain_count = 3

# Apply an ailment on hit (effect def lives in config/effects/<id>.toml — today only `burn` exists):
[[effect_applications]]
effect_id = "burn"
target = "target"
apply_chance = "always"
[effect_applications.scaling.damage_driven]
conversions = { fire = 0.5 }

# CAUSALITY: fire a SEPARATE skill's timeline when this skill's hitbox ends. One condition per ending:
#   always -> entity hit (fires only when a damage packet exists) · on_impact -> world hit · on_expire -> fuse
# `additional = true` is REQUIRED for a timeline-target condition (fires ALONGSIDE this skill's own hit).
[[conditions]]
trigger_skill = "firebolt_explosion"
type = "always"
additional = true
[[conditions]]
trigger_skill = "firebolt_explosion"
type = "on_impact"
additional = true
[[conditions]]
trigger_skill = "firebolt_explosion"
type = "on_expire"
additional = true
```

The loader validates `trigger_skill` refs — a dangling ref fails the whole skills dir. Triggered skills
run their own timeline **at the hit position, mana-free, at full charge-scaled damage** (budget totals
accordingly: a 20 bolt + 15 explosion = 35 on a direct hit). Other `type` values exist (on-crit/on-kill,
etc. — see `loot_core` `TriggerCondition`); `always`/`on_impact`/`on_expire` are the causality set.

### 3b. Behavior — `assets/skills/<id>.cast.ron` (`CastTimeline`, v2)

**`CollisionWindow` field reference** (defaults let you omit fields; `deny_unknown_fields` rejects typos):

| Field | Values | Notes |
|---|---|---|
| `id` | string | referenced by cues (`on_window_{id}`…) + emitters |
| `spawn` | `Scheduled( phase: Windup\|Active\|Recovery, offset: 0.0 )` · `Template` | `Template` = never self-schedules; only an emitter spawns it |
| `anchor` | `Caster` (default) · `CastPoint` | `CastPoint` = the acquired point / the trigger's payload position |
| `anchor_offset` | `(x, y, z)` | world-axis offset (e.g. `(0,8,0)` hangs a cloud 8 up) |
| `strikes` | `true` (default) · `false` | `false` = carrier: flies/ends/emits but never hits |
| `active_duration` | seconds | **this IS the fuse** (can far exceed the phase total) |
| `shape` | `Sphere( radius )` · `Capsule( radius, height )` · `Cone( angle, range )` | round only |
| `motion` | `Static` (default) · `Linear( speed )` · `Ballistic( speed, gravity )` · `Beam` | fixed velocity |
| `motion_direction` | `Inherit` (default) · `Down` · `Horizontal` | overrides launch dir (falling shards / ground-rollers) |
| `hit_filter` | `Caster` · `Allies` · `Enemies` · `All` | |
| `hit_mode` | `FirstOnly` · `OncePerTarget` · `EveryTick` | FirstOnly=projectile (ends on 1st hit); EveryTick(+`rehit_interval`)=damage field |
| `rehit_interval` | `Some(secs)` · `None` | |
| `emitter` | `Some(( rate, jitter, window ))` · `None` | rains the named `Template` window at `rate`/s, xz-jitter `jitter` |
| `paints` | `Some(( surface, radius, mode: Trail( step )\|OnEnd, lifetime: Some(secs)? ))` · `None` | paints a `config/surfaces/` patch (§3d) while alive (`Trail` = every `step` m of travel + once at spawn) or at end (`OnEnd`); `surface` must be a loaded id |

Top-level: `acquisition` (§2), `chain_radius: 6.0` (beam chain search radius), `chargeable: bool`,
`max_hold: secs`, plus the presentation maps (§3c).

**Acquisition surface gate** (on `GroundPoint` only): `GroundPoint( range, fallback, on_surface: Some((
surface, snap, consume )) )` requires the aimed point to land on a `surface` patch — `snap: true`
(default) recenters the cast point on the matched patch, `consume: true` removes the patch at cast-accept
(spent even if the cast is later interrupted). A point off any patch runs the normal `fallback` (paid
`Fizzle`). See Archetype 5 + §3d.

**Archetype 1 — projectile that triggers an AoE** (firebolt: the bolt is here, the boom is a *separate*
skill fired by the rules conditions in §3a):
```ron
(
  skill_id: "firebolt",
  phase_durations: ( windup: 0.3, active: 0.1, recovery: 0.2 ),
  collision_windows: [
    ( id: "bolt", spawn: Scheduled( phase: Active, offset: 0.0 ), active_duration: 2.0,
      shape: Sphere( radius: 0.5 ), motion: Ballistic( speed: 20.0, gravity: 9.8 ),
      hit_filter: Enemies, hit_mode: FirstOnly ),
  ],
  acquisition: Aim,
  chargeable: true,
  max_hold: 1.5,
  vfx_cues: { "on_cast": "on_cast", "on_window_bolt": "on_window_bolt", "on_hit": "on_hit", "on_end_bolt": "on_end_bolt" },
  cues: {
    "on_cast":        ( effect: Some("Fire"), attach: Bone( socket: "R_wrist_joint", offset: (0.0, 0.05, 0.0) ), anim: None, params: [ ( param: "scale", source: Charge ) ] ),
    "on_window_bolt": ( effect: Some("firebolt_trail"), attach: Follow, anim: None, params: [] ),
    "on_hit":         ( effect: Some("Sparks"), attach: World, anim: None, params: [] ),
  },
  charge_cues: [
    ( threshold: 0.0, cue: ( effect: Some("charge_sparks"), attach: Bone( socket: "R_wrist_joint", offset: (0.0, 0.05, 0.0) ), anim: Some("casting_idle"), params: [ ( param: "scale", source: Charge ) ] ) ),
    ( threshold: 0.6, cue: ( effect: Some("charge_storm"),  attach: Bone( socket: "R_wrist_joint", offset: (0.0, 0.05, 0.0) ), anim: Some("casting_idle"), params: [ ( param: "scale", source: Charge ) ] ) ),
  ],
)
```
> `on_end_bolt` is in `vfx_cues` with NO `cues` binding on purpose: it's the teardown trigger that
> despawns the `Follow` trail when the bolt ends. A `Follow` cue needs no matching `on_end` binding to
> clean up — the window's end event does it.
> The AoE this triggers can itself leave a **persistent surface**: firebolt_explosion's blast
> `paints: OnEnd` a `burning` scorch (full line in Archetype 2; §3d), whose standing tick then hits
> enemies in it. Painting is a window property, so it rides the trigger cascade for free — no extra
> skill, no mana.

**Archetype 2 — triggered sub-skill** (firebolt_explosion: the AoE that Archetype 1 fires). `SelfPoint`
acquisition + a `CastPoint`-anchored window = "detonate at the position the trigger fired at". Never
granted to a weapon; `mana_cost = 0`. The blast also `paints` a lingering `burning` scorch on end (§3d):
```ron
(
  skill_id: "firebolt_explosion",
  phase_durations: ( windup: 0.0, active: 0.05, recovery: 0.0 ),
  collision_windows: [
    ( id: "blast", spawn: Scheduled( phase: Active, offset: 0.0 ), anchor: CastPoint,
      active_duration: 0.05, shape: Sphere( radius: 1.5 ), motion: Static,
      hit_filter: Enemies, hit_mode: OncePerTarget,
      paints: Some(( surface: "burning", radius: 1.2, mode: OnEnd, lifetime: Some(4.0) )) ),
  ],
  acquisition: SelfPoint,
  vfx_cues: { "on_window_blast": "on_window_blast" },
  cues: { "on_window_blast": ( effect: Some("Explosion"), attach: World, anim: None, params: [] ) },
)
```

**Archetype 3 — ground-targeted emitter storm** (blizzard: a carrier `storm` cloud rains `shard`
`Template` windows). Note `GroundPoint → Then(SelfPoint)` fallback and `motion_direction: Down`:
```ron
(
  skill_id: "blizzard",
  phase_durations: ( windup: 0.4, active: 0.1, recovery: 0.3 ),
  collision_windows: [
    ( id: "storm", spawn: Scheduled( phase: Active, offset: 0.0 ), anchor: CastPoint, anchor_offset: (0.0, 8.0, 0.0),
      strikes: false, active_duration: 4.0, shape: Sphere( radius: 3.0 ), motion: Static,
      hit_filter: Enemies, hit_mode: OncePerTarget,
      emitter: Some(( rate: 6.0, jitter: 3.0, window: "shard" )) ),
    ( id: "shard", spawn: Template, active_duration: 2.0, shape: Sphere( radius: 0.5 ),
      motion: Linear( speed: 12.0 ), motion_direction: Down, hit_filter: Enemies, hit_mode: OncePerTarget ),
  ],
  acquisition: GroundPoint( range: 30.0, fallback: Then(SelfPoint) ),
  chargeable: true,
  max_hold: 1.0,
  vfx_cues: { "on_cast": "on_cast", "on_hit": "on_hit", "emit_shard": "emit_shard" },
  cues: {
    "on_cast":    ( effect: Some("blizzard_frost"), attach: Bone( socket: "R_wrist_joint", offset: (0.0, 0.0, 0.0) ), anim: None, params: [ ( param: "scale", source: Charge ) ], duration: Some(0.5) ),
    "emit_shard": ( effect: Some("blizzard_frost"), attach: World, anim: None, params: [], duration: Some(0.35) ),
    "on_hit":     ( effect: Some("blizzard_frost"), attach: World, anim: None, params: [], duration: Some(0.35) ),
  },
)
```

**Archetype 4 — beam + rules chain** (chain_lightning: hitscan the target, one `Beam` window; rules
`can_chain=true`/`chain_count=3` + `chain_radius` auto-hop to the nearest un-struck enemy). A miss (no
entity aimed) is a paid fizzle:
```ron
(
  skill_id: "chain_lightning",
  phase_durations: ( windup: 0.25, active: 0.05, recovery: 0.2 ),
  collision_windows: [
    ( id: "arc", spawn: Scheduled( phase: Active, offset: 0.0 ), active_duration: 0.15,
      shape: Sphere( radius: 0.3 ), motion: Beam, hit_filter: Enemies, hit_mode: FirstOnly ),
  ],
  acquisition: HitscanEntity( range: 15.0, filter: Enemies, fallback: Fizzle ),
  chain_radius: 6.0,
  vfx_cues: { "on_window_arc": "on_window_arc", "on_hit": "on_hit" },
  cues: {
    "on_window_arc": ( effect: Some("Sparks"), attach: World, anim: None, params: [] ),
    "on_hit":        ( effect: Some("Sparks"), attach: World, anim: None, params: [] ),
  },
)
```

**Archetype 5 — surface-gated consumer** (frost_spire: erupt a spire only on a `frost` patch the
glacier painted). The `GroundPoint` `on_surface` gate requires the aimed point to land on a `frost`
patch, SNAPS the cast to that patch's center, and CONSUMES it at cast-accept; a point off any patch is a
paid `Fizzle`. The damage window is a `CastPoint`-anchored capsule the size of the rising spire (the
physical spire itself is an arena server verb on the `on_window_spike` cue — §1):
```ron
(
  skill_id: "frost_spire",
  phase_durations: ( windup: 0.25, active: 0.1, recovery: 0.25 ),
  collision_windows: [
    ( id: "spike", spawn: Scheduled( phase: Active, offset: 0.0 ), anchor: CastPoint,
      anchor_offset: (0.0, 0.8, 0.0), active_duration: 0.25,
      shape: Capsule( radius: 0.45, height: 1.6 ), motion: Static,
      hit_filter: Enemies, hit_mode: OncePerTarget ),
  ],
  acquisition: GroundPoint( range: 60.0, fallback: Fizzle,
    on_surface: Some(( surface: "frost", snap: true, consume: true )) ),
  chargeable: true,
  max_hold: 1.5,
)
```
> `consume: true` spends the patch even if the cast is interrupted after accept. The gate only resolves
> when a `frost` patch already exists under the aim — the glacier paints them in-game; in the designer,
> stage one first (§4) or the cast always fizzles.

Motion picker: `Static` (melee/nova/field), `Linear(speed)` (straight bolt/shard),
`Ballistic(speed, gravity)` (lob — gravity NOT charge-scaled, so charged shots fly flatter), `Beam`
(instant strike on the designated target; no target = paid fizzle). For a lob-and-explode use
`Ballistic` + a rules trigger to a `SelfPoint`/`CastPoint` sub-skill (Archetypes 1+2) — there is no
inline "chained window" in v2.

### 3c. Presentation — inline `cues` / `charge_cues` (the sim never reads these)

Two maps keyed by the same **slot names**:
- `vfx_cues: { slot -> cue_id }` — **the sim READS this**; a slot's presence makes obelisk *emit* the
  `CueEvent` (which also drives server verbs). Keep it in sync with `cues`.
- `cues: { cue_id -> CueBinding }` — client visuals only; inert to the sim (a typo'd effect renders
  **nothing, no error**).

**Cue slots** (`{id}` = a `CollisionWindow.id`): `on_cast` (cast begins) · `on_window_{id}` (a scheduled
window spawns) · `on_hit` (each hit) · `on_end_{id}` (a window ends, at the end position) ·
`emit_{id}` (an emitter instantiates a Template — fires this, never `on_window_{id}`).

**`CueBinding`**: `( effect: Some("Key"), attach: World|Follow|Bone( socket, offset ), anim: Some("clip"),
params: [ ( param: "scale", source: Charge ) ], duration: Some(secs) )`.
- `attach`: `World` (fixed position — the only legal option on `on_hit`/`on_end_*`) · `Follow` (host flies
  a proxy along the window's motion; only on `on_window_*`/`emit_*`) · `Bone( socket, offset )` (a named
  rig joint — `on_cast` + charge tiers; the casting hand is `R_wrist_joint`). `anim` is only meaningful
  on `on_cast`. `source: Charge` is the ONLY dynamic param source.
- **`charge_cues`**: `[ ( threshold: 0.0..=1.0, cue: <CueBinding> ) ]` — hold-to-charge tiers, ascending
  thresholds, `World`/`Bone` only (no `Follow`), `duration` ignored (tiers loop). Pure host-side.

**Effect names** are case-sensitive `VfxLibrary` keys: built-ins are capitalized (`"Fire"`, `"Explosion"`,
`"Sparks"`, `"Portal"`); workspace presets load by file stem from `assets/vfx/*.vfx.ron` (which WINS over
`assets/skills/*.vfx.ron` on a name collision — author presets in `assets/vfx/`). For a new in-flight
look, author `assets/vfx/<id>_trail.vfx.ron` (copy `firebolt_trail.vfx.ron`).

### 3d. Surfaces — `config/surfaces/<id>.toml` (`obelisk_bevy::surfaces::SurfaceType`)

A **surface** is persistent painted ground: a window `paints` patches of it (§3b), a `GroundPoint`
`on_surface` gate requires/consumes them, and each patch runs a payload on whoever stands in it. One TOML
per surface id (the arena root ships `frost` + `burning`); the editor loads them into `SurfaceRegistry`
the same way it loads skills. Copy `burning.toml`:

```toml
id = "burning"
lifetime = 8.0
merge_radius = 0.3
max_patches = 32
patch_radius = 1.2

[standing]
filter = "enemies"
tick_skill = "burning_ground_tick"
rehit_interval = 0.5

[visuals]
decal = "textures/decal_splat.png"
color = [1.0, 0.45, 0.1, 0.85]
vfx = "Embers"
```

- **Top level**: `id`; `lifetime` (patch seconds — a paint's own `lifetime` override wins); `merge_radius`
  (a paint this close to a same-type patch is skipped); `max_patches` (per-type cap — exceeding it despawns
  the OLDEST, the arena's replace-oldest tile feel); `patch_radius` (default radius for a paint that authors
  none).
- **`[standing]`** — payload for entities standing INSIDE a patch, attributed to the painter: `filter`
  (`enemies`/`allies`/`all`, relative to the painter); `effect` (an obelisk effect id applied/refreshed
  while standing — chill/burn); `tick_skill` (a triggered skill run AT the victim on the clock —
  full-combat periodic damage, the `firebolt_explosion` pattern); `rehit_interval` (clock period);
  `on_enter_only` (fire once per patch×visit instead of on the clock).
- **`[[on_skill_contact]]`** (repeatable) — a reaction to a HITBOX touching the patch ("fire ignites oil"):
  if the contacting skill's rules `tags` intersect `tags_any`, run `trigger_skill` at the contact point
  (attributed to the contacting caster), optionally `consume` the patch.
- **`[visuals]`** — sim-inert host render hints (like `cues`): `decal` texture key, `color` RGBA tint,
  `vfx` looping preset. The sim never reads them.

**Loader fails LOUD** (`load_surfaces_dir`, crate convention): numeric sanity (`lifetime > 0`,
`max_patches >= 1`, `patch_radius > 0`, `rehit_interval > 0`), duplicate/empty ids, an `on_skill_contact`
tag that isn't a real `SkillTag`, and any dangling `tick_skill` / `on_skill_contact.trigger_skill` /
`standing.effect` ref each reject the WHOLE `config/surfaces/` directory. (The effect/skill checks fire
only once `add_obelisk_effects` + the skills scan have run — the editor's content scan orders them for you.)

**Editor**: the surface pickers in the behavior panel (the window `paints` row + the `on_surface` gate) are
registry-fed — you can only pick ids loaded from `config/surfaces/` (an empty registry shows a muted "no
surfaces loaded" note). To test a gated cast, stage a patch first: command palette → **"Stage: paint frost
patch"** (a staged patch survives stage reset + scrub; **"Stage: clear staged paints"** removes them).

## 4. Verify — designer first, then headless

1. `cd crates/arena_editor && cargo run --bin arena-editor` (own cargo workspace — never `-p
   arena_editor` from root). Press `K` for Skill mode, open the skill via the picker.
2. **Scrub** the phase strip: each bound cue's vfx fires at its moment (no Play needed). Select a window
   to see its shape gizmo.
3. **Play**: the caster casts at the dummy. Watch flight/impact/timing.
   - **Surface-gated skills** (`on_surface`) fizzle on bare ground — first stage a patch: command palette
     → **"Stage: paint frost patch"** (survives reset + scrub), then Play the gated cast (§3d).
4. **Save** writes the `.cast.ron` (+ TOML when its tab is dirty) and hot-reloads.
5. Headless: `cd crates/arena_editor && cargo test`. Game-side changes: the net-test
   (`pkill -f arena-server; pkill -f arena-client; sleep 1; bash crates/arena_game/tools/net-test/run_session.sh`).

**Editor reality — what the tool will NOT do for you** (hand-edit the file for these):
- **Chaining is not authorable in the UI** — `can_chain`/`chain_count` have no widget; set them in the TOML.
- **Most rules fields are TOML-only** — `tags`, `targeting`/`delivery`, `name`, and
  `effect_applications` `scaling`/`apply_chance` have no editor control.
- **Renaming a window in the UI orphans its cue bindings** (the `cues` map isn't repointed) — after a
  rename, fix the `on_window_{id}`/`on_end_{id}`/`emit_{id}` keys by hand, or edit ids in the file.
- **Save re-serializes the `.cast.ron` and strips comments** — keep authoritative comments in git and
  expect the editor to drop them.
- **The preview shows no damage/health/death numbers** — judge damage from the rules readout + headless
  tests, not the viewport. Emitter (blizzard) scrub is **not** frame-deterministic (shard positions vary
  per scrub). The charge slider applies even to non-`chargeable` skills — ignore it unless `chargeable: true`.

## 5. Pitfalls checklist (v2)

- **Stale v1 syntax fails LOUD.** `spawn_phase`/`spawn_offset`, `on_end: Chain/Retarget`,
  `WindowPhase::Chained`, `targeting:`/`delivery:` in the `.cast.ron`, and `.skillfx.ron` files no longer
  exist — `deny_unknown_fields` rejects the file. Copy a shipped v2 exemplar, don't reconstruct from memory.
- **Causality is rules-side.** A projectile→AoE is TWO skills wired by a `[[conditions]] trigger_skill`;
  there is no inline chained window. `additional = true` is required on timeline-target conditions.
- **`type = "always"` is entity-hit only** (it needs a damage packet). Pair it with `on_impact` +
  `on_expire` for full coverage — and it only stays "fires once" because a `FirstOnly` bolt ends once. On
  a `OncePerTarget` skill, `always` fires per victim.
- **Emitter rules** (`validate_timeline` enforces): the `emitter.window` must exist and be `Template`;
  every `Template` must be referenced by an emitter; a `Template` can't carry its own emitter; `rate > 0`,
  `jitter >= 0`.
- **`CastPoint` needs a point-producing acquisition** — a `CastPoint`-anchored window on an `Aim`/
  `HitscanEntity`(without a `Then` point) timeline fails validation. Use `GroundPoint`/`SelfPoint` (directly
  or via a `Then` fallback).
- **`active_duration` IS the fuse.** "Explode after N seconds wherever it is" = `active_duration: N` + an
  `on_expire` rules trigger.
- **`FirstOnly` ends the whole hitbox after one victim; `OncePerTarget` keeps hitting until the fuse;
  `EveryTick` + `rehit_interval` is a damage field.**
- **Chaining is beam-only.** `can_chain` on a non-`Beam` skill silently never chains.
- **Charge scales speed AND damage** (0.5–2.0×). Don't also inflate base damage for "charged" skills.
- **Chained/emitted/triggered/surface hits are mana-free and full-damage** — a `standing.tick_skill`, a
  contact `trigger_skill`, and every chained/emitted/triggered hit cost the caster nothing; budget the
  totals on one victim.
- **Surface ids must be loaded.** A `paints`/`on_surface` `surface` absent from `config/surfaces/`: the
  editor's picker only offers loaded ids (you can't select a missing one), but a hand-edited unknown id
  warns-and-skips the paint at runtime, and an unknown `on_surface` id silently gates the cast (no patch
  ever matches → it always fizzles). Keep them in `config/surfaces/` (§3d).
- **`on_skill_contact` tags must be real `SkillTag`s.** The surface loader REJECTS the whole
  `config/surfaces/` dir on an unknown contact tag (it would otherwise silently never match) — the same
  fail-loud contract as a dangling skill ref.
- **Cue lane keys are the slot names** (`on_window_bolt`, `on_end_bolt`, `emit_shard`), and a typo'd
  `effect` name renders nothing with no error — verify names against the `VfxLibrary` in the designer.
