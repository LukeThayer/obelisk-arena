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
  increment (check whether it has landed before authoring `on_end` reactions).

**Never promise a mechanic without checking the gap map.** If the concept needs an
unimplemented edge/node (beams, retarget hops, emitters), say so and either scope the design
to what ships today or propose the sim increment first.

## 1. Decompose the concept (do this in conversation before touching files)

Fill in this skeleton with the user:
- **Fantasy + role**: one sentence. Burst? zoning? poke? finisher?
- **Cast node**: mana cost, cooldown, windup/active/recovery seconds, charged or tap?
  (charge byte scales projectile speed AND damage 0.5–2.0×).
- **Acquisition**: how aim resolves — free direction (current game default), entity target,
  ground point. (Authored acquisition/fallback is not yet data-driven — the game casts
  free-aim from the eye.)
- **Volumes/edges**: for each hit region — shape, motion (Static / Linear{speed} /
  Ballistic{speed, gravity}), lifetime (= fuse), hit_filter, hit_mode (FirstOnly = projectile,
  OncePerTarget = sweep/AoE, EveryTick+rehit_interval = damage field), and what its ending
  causes (chain a blast? nothing?).
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

**Behavior — `assets/skills/<id>.cast.ron`** (obelisk `CastTimeline`). Ballistic-lob template
(current firebolt shape):
```ron
(
  skill_id: "<id>",
  phase_durations: ( windup: 0.3, active: 0.1, recovery: 0.2 ),
  collision_windows: [
    ( id: "bolt", spawn_phase: Active, spawn_offset: 0.0, active_duration: 2.0,
      shape: Sphere( radius: 0.5 ), motion: Ballistic( speed: 20.0, gravity: 9.8 ),
      hit_filter: Enemies, hit_mode: FirstOnly ),
  ],
  targeting: SingleEntity( range: 15.0 ),
  delivery: Projectile( speed: 20.0 ),
  vfx_cues: {},   // Save in the designer derives the locked cue map
)
```
Motion picker: `Static` (melee/nova/field), `Linear(speed)` (straight bolt),
`Ballistic(speed, gravity)` (lob; gravity NOT charge-scaled — charged shots fly flatter).
Projectile hitboxes ground-stop at the floor plane (y = 0).

**Presentation — `assets/skills/<id>.skillfx.ron`** — lanes keyed by the derived cue VALUES
(`<id>_cast`, `<id>_window_<wid>`, `<id>_impact`):
```ron
(
  skill_id: "<id>",
  lanes: {
    "<id>_cast": ( lane_id: "<id>_muzzle", kind: OnCast,
      particle: Some(( count: 12, lifetime: 0.4, color: (1.0, 0.5, 0.1), speed: 4.0,
        effect: Some("Fire") )),
      projectile: Some(( speed: 20.0, gravity: 9.8, color: (1.0, 0.4, 0.05), radius: 0.2,
        effect: Some("<id>_trail") )),
      anim: Some(( state: "cast_release" )) ),
    "<id>_impact": ( lane_id: "<id>_impact", kind: OnHit,
      particle: Some(( count: 20, lifetime: 0.5, color: (1.0, 0.3, 0.05), speed: 5.0,
        effect: Some("Explosion") )) ),
  },
)
```
Hard rules:
- `projectile.speed`/`gravity` MUST equal the window's motion values (the cosmetic traces the
  authoritative hitbox).
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

- Cue lane keys are the cue VALUES (`firebolt_cast`), not the slot names (`on_cast`).
- `FirstOnly` stops the whole hitbox after one victim; `OncePerTarget` keeps sweeping.
- A window's `active_duration` IS its fuse; it can far exceed the phase total (the editor
  strip extends to the latest window close).
- bevy_vfx presets default to looping-forever — preview bounds them with lane `lifetime`
  (min 0.5 s); keep authored one-shot presets' visual length under the lane lifetime.
- Charge: `None`/tap ≈ 1.0×; the byte scales projectile speed AND damage. Don't double-dip by
  also inflating base damage for "charged" skills.
- Rules `trigger_skill` refs must exist or the whole skills dir fails to load (Save is gated).
