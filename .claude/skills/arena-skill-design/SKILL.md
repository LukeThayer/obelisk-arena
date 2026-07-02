---
name: arena-skill-design
description: Design and author a new obelisk-arena skill end-to-end — decompose the concept into the skill anatomy (nodes/edges/triggers), author the three-file triad (rules TOML, .cast.ron behavior, .skillfx.ron cosmetics + .vfx.ron presets), then verify in the skill designer (scrub + Play) and headless tests. Use when the user wants to add a new skill ("add an ice spike spell", "design chain lightning"), rework an existing one's behavior/feel, or asks what's expressible in the current sim.
---

# Designing an arena skill

## 0. Ground yourself first

Read (in the obelisk-bevy repo — sibling checkout at `../obelisk-bevy`, or the pinned cargo
checkout under `~/.cargo/git/checkouts/bevy-obelisk-*/`):
- `docs/superpowers/specs/2026-07-02-skill-anatomy.md` — what a skill IS: the three layers
  (rules / behavior graph / presentation), node kinds (Cast, Acquisition, Volume, Beam,
  Emitter), edge kinds (`At`, `OnEnd`, `OnHit`, `OnTick`, `Retarget`), and the **gap map** of
  what is expressible today vs. spec'd vs. missing.
- `docs/superpowers/specs/2026-07-02-event-driven-skill-phases.md` — the end-event + chaining
  increment (SHIPPED 2026-07-02, obelisk-bevy 6afa6ba): every hitbox ends with
  `EndReason { HitEntity, HitWorld, Fuse }` + a world position; `on_end` reactions chain
  `Chained` windows at that position; `on_end_{wid}` cues fire there.

- `docs/superpowers/specs/2026-07-02-beam-retarget-hitscan.md` — increment 2 (SHIPPED
  2026-07-02): `VolumeMotion::Beam` strikes its designated target directly;
  `EndReaction::Retarget { window, radius, max_hops }` hops to the nearest un-struck enemy
  (self-hop legal, hop-bounded); server hitscan acquisition keys on `SingleEntity` targeting
  (miss = paid fizzle); two-anchor beam cues + `beam:` lanes. Chain lightning is the proving
  case — copy its triad for beam skills.

**Never promise a mechanic without checking the gap map.** If the concept needs an
unimplemented edge/node (`OnTick` emitters, ground-point acquisition, per-hop damage
falloff), say so and either scope the design to what ships today or propose the sim
increment first.

## 1. Decompose the concept (do this in conversation before touching files)

Fill in this skeleton with the user:
- **Fantasy + role**: one sentence. Burst? zoning? poke? finisher?
- **Cast node**: mana cost, cooldown, windup/active/recovery seconds, charged or tap?
  (charge byte scales projectile speed AND damage 0.5–2.0×).
- **Acquisition**: how aim resolves. `targeting: Direction` = free aim along the crosshair
  (firebolt). `targeting: SingleEntity` = the server HITSCANS the looked-at enemy and casts
  entity-aimed (chain lightning); a miss still pays mana + cooldown and fizzles. Ground-point
  acquisition is not yet data-driven.
- **Volumes/edges**: for each hit region — shape, motion (Static / Linear{speed} /
  Ballistic{speed, gravity}), lifetime (= fuse), hit_filter, hit_mode (FirstOnly = projectile,
  OncePerTarget = sweep/AoE, EveryTick+rehit_interval = damage field), and what its ENDING
  causes per reason (enemy hit / world hit / fuse — chain a blast window? nothing?). Chained
  windows spawn at the parent's end position with the original caster/aim/charge.
- **Rules**: damage types/amounts, crit, effects applied (burn/chill — `config/effects/`),
  triggers (on-crit/on-kill secondary skills).
- **Presentation per moment**: cast (muzzle + anim), flight (trail preset), each window open,
  each impact/end. Which need world-position anchoring vs caster-socket anchoring.

## 2. Author the triad

All paths are workspace-root relative. One skill = same id across all three files.

**Rules — `config/skills/<id>.toml`** (stat_core `Skill`; loader validates `trigger_skill`
refs). Template:
```toml
id = "<id>"
name = "<Name>"
tags = ["Spell", "Fire", "Projectile"]   # drives scaling/filters
targeting = "single_enemy"                # self | single_enemy | none
delivery = "projectile"                   # melee | projectile | instant
mana_cost = 5.0
cooldown = 1.5

[damage]
base_damages = [{ damage_type = "Fire", min = 18.0, max = 22.0 }]
base_crit_chance = 0.05
```

**Behavior — `assets/skills/<id>.cast.ron`** (obelisk `CastTimeline`). Ballistic
lob-and-explode template (current firebolt v2 shape — projectile chains a blast at wherever
it ends):
```ron
(
  skill_id: "<id>",
  phase_durations: ( windup: 0.3, active: 0.1, recovery: 0.2 ),
  collision_windows: [
    ( id: "bolt", spawn_phase: Active, spawn_offset: 0.0, active_duration: 2.0,
      shape: Sphere( radius: 0.5 ), motion: Ballistic( speed: 20.0, gravity: 9.8 ),
      hit_filter: Enemies, hit_mode: FirstOnly,
      on_end: ( hit: Some(Chain("blast")), world: Some(Chain("blast")), fuse: Some(Chain("blast")) ) ),
    // Chained: never scheduled — spawns only via a parent's on_end, AT the end position.
    ( id: "blast", spawn_phase: Chained, spawn_offset: 0.0, active_duration: 0.05,
      shape: Sphere( radius: 1.5 ), motion: Static,
      hit_filter: Enemies, hit_mode: OncePerTarget ),
  ],
  targeting: SingleEntity( range: 15.0 ),
  delivery: Projectile( speed: 20.0 ),
  vfx_cues: {},   // Save in the designer derives the locked cue map
)
```
Beam/chain template (chain lightning shape — hitscan target, then N hops):
```ron
    ( id: "arc", spawn_phase: Active, spawn_offset: 0.0, active_duration: 0.15,
      shape: Sphere( radius: 0.3 ), motion: Beam,
      hit_filter: Enemies, hit_mode: FirstOnly,
      on_end: ( hit: Some(Retarget( window: "hop", radius: 6.0, max_hops: 3 )) ) ),
    ( id: "hop", spawn_phase: Chained, spawn_offset: 0.0, active_duration: 0.15,
      shape: Sphere( radius: 0.3 ), motion: Beam,
      hit_filter: Enemies, hit_mode: FirstOnly,
      on_end: ( hit: Some(Retarget( window: "hop", radius: 6.0, max_hops: 3 )) ) ),
    // targeting: SingleEntity( range: 15.0 ) — opts into server hitscan acquisition.
```
Motion picker: `Static` (melee/nova/field), `Linear(speed)` (straight bolt),
`Ballistic(speed, gravity)` (lob; gravity NOT charge-scaled — charged shots fly flatter),
`Beam` (instant strike on the designated target; no target = paid fizzle). `Retarget` may
self-reference (hop→hop) — the hop counter bounds it; the chain never strikes the same enemy
twice; hops keep the original caster + charge at FULL damage.
Projectile hitboxes hitting the floor plane (y = 0) end with `HitWorld` at the impact point.
`on_end` is per-reason (`hit`/`world`/`fuse`, each `Option<Chain("id")>`) — the fuse IS
`active_duration`, so "explode after N seconds wherever it is" = `active_duration: N` +
`fuse: Some(Chain(...))`. The loader validates chains: targets must exist, must be
`Chained`, and the graph must be acyclic (Save fails otherwise). In the designer, the
`end→<window>` combo on a window row sets all three reasons to one target.

**Presentation — `assets/skills/<id>.skillfx.ron`** — lanes keyed by the derived cue VALUES
(`<id>_cast`, `<id>_window_<wid>` at open, `<id>_end_<wid>` at the END POSITION, `<id>_impact`
victim-anchored):
```ron
(
  skill_id: "<id>",
  lanes: {
    "<id>_cast": ( lane_id: "<id>_muzzle", kind: OnCast,
      particle: Some(( count: 12, lifetime: 0.4, color: (1.0, 0.5, 0.1), speed: 4.0,
        effect: Some("Fire") )),
      anim: Some(( state: "cast_release" )) ),
    // Flight visuals bind to the WINDOW-OPEN cue (the hitbox exists NOW, here) — an on_cast
    // projectile launches a whole windup early and visually overshoots the real bolt.
    "<id>_window_bolt": ( lane_id: "<id>_flight", kind: OnWindow,
      projectile: Some(( speed: 20.0, gravity: 9.8, color: (1.0, 0.4, 0.05), radius: 0.2,
        effect: Some("<id>_trail"), end_cue: Some("<id>_end_bolt") )) ),
    // The BIG payoff renders at the end position — enemy, ground, or mid-air fuse.
    "<id>_end_bolt": ( lane_id: "<id>_blast", kind: OnEnd,
      particle: Some(( count: 20, lifetime: 0.5, color: (1.0, 0.3, 0.05), speed: 5.0,
        effect: Some("Explosion") )) ),
    // Optional small victim-anchored flash on the direct hit.
    "<id>_impact": ( lane_id: "<id>_impact", kind: OnHit,
      particle: Some(( count: 8, lifetime: 0.3, color: (1.0, 0.6, 0.2), speed: 3.0 )) ),
  },
)
```
Beam lanes (chain lightning): bind a `beam:` spec to each beam window's OPEN cue — the cue
carries BOTH anchors (origin→victim) and the lane renders sampled bursts along the arc:
```ron
    "<id>_window_arc": ( lane_id: "<id>_arc", kind: OnWindow,
      beam: Some(( effect: Some("Sparks"), color: (0.5, 0.7, 1.0), segments: 10, lifetime: 0.25 )) ),
```
Hard rules:
- `projectile.speed`/`gravity` MUST equal the window's motion values (the cosmetic traces the
  authoritative hitbox), and `end_cue:` MUST name the window's end-cue value
  (`<id>_end_<wid>`) — the sim's end cue is what terminates the flight, on every peer, so the
  visual can't outfly or undershoot the hitbox.
- Anchoring by cue kind: `OnCast`/`OnWindow` lanes are caster/socket-anchored; `OnHit` renders
  at the victim; `OnEnd` renders at the carried world position. Put area payoffs (explosions,
  ground fields) on `OnEnd`, never `OnHit` — an `OnHit` explosion silently vanishes when the
  bolt hits the ground or fuses out.
- Timing by cue kind: muzzle flash + cast anim on `OnCast` (windup start); the PROJECTILE on
  `OnWindow` (windup end — when the hitbox spawns); the payoff on `OnEnd`. The window cue slot
  (`on_window_<wid>`) must be present in the `.cast.ron` `vfx_cues` map (designer Save derives
  it; hand-edited files must include it or the lane never fires).
- `effect:` names are **case-sensitive `VfxLibrary` keys**: built-ins are capitalized
  (`"Fire"`, `"Explosion"`, `"Sparks"`, …); workspace-authored presets load from
  `assets/skills/<name>.vfx.ron` and `assets/vfx/` by file stem (both the game and the
  designer scan these). Preset textures live in `assets/textures/particles/`.
- For a new in-flight look, author `assets/skills/<id>_trail.vfx.ron` (copy
  `firebolt_trail.vfx.ron`): core emitter `sim_space: Local` rides the bolt; trail/spark
  emitters `sim_space: World` linger along the arc.

## 3. Verify — designer first, then headless

1. `cd crates/arena_editor && cargo run --bin arena-editor` (own cargo workspace — never
   `-p arena_editor` from root). Press `K` for Skill mode, open the skill via the picker.
2. **Scrub** the phase strip: each bound cue's vfx fires in the viewport at its moment —
   no Play needed. Select a window to see its shape + trajectory gizmo (arc shows the
   landing point).
3. **Play**: the real sim duel — caster rig casts at the dummy from your current camera view;
   check flight vfx, impact position, playhead. Escape pauses, Reset clears.
4. Save in the designer (writes `.cast.ron` + `.skillfx.ron`; rules TOML only when its tab is
   dirty — hot-reloads the registry).
5. Headless: `cd crates/arena_editor && cargo test` (preview harness resolves real damage);
   for game-side changes run the net-test:
   `pkill -f arena-server; pkill -f arena-client; sleep 1; bash crates/arena_game/tools/net-test/run_session.sh`
   (needs python3; flaky on wall clock — retry ≤3×, one `session PASS` is green).

## Pitfalls checklist

- Cue lane keys are the cue VALUES (`firebolt_cast`, `firebolt_end_bolt`), not the slot names
  (`on_cast`, `on_end_bolt`).
- `FirstOnly` stops the whole hitbox after one victim AND ends it immediately (`HitEntity`);
  `OncePerTarget` keeps sweeping until the fuse.
- A window's `active_duration` IS its fuse; it can far exceed the phase total (the editor
  strip extends to the latest window close, chained windows drawn after their parent's close).
- Chained windows are never scheduled: a `Chained` window nobody chains to simply never
  spawns (the loader doesn't reject orphans — check the `end→` combos).
- Chained damage keeps the ORIGINAL caster + charge — a blast is the same skill's damage roll
  hitting again; budget totals accordingly (direct + splash on the same victim).
- bevy_vfx presets default to looping-forever — preview bounds them with lane `lifetime`
  (min 0.5 s); keep authored one-shot presets' visual length under the lane lifetime.
- Charge: `None`/tap ≈ 1.0×; the byte scales projectile speed AND damage. Don't double-dip by
  also inflating base damage for "charged" skills.
- Rules `trigger_skill` refs must exist or the whole skills dir fails to load (Save is gated).
