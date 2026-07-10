# Obelisk Skill System & Editor — Critical Design Review

**Date:** 2026-07-09 · **Reviewer stance:** veteran skill-effect designer · **Scope:** the obelisk skill
sim (the *palette*), the skill designer in `bevy_modal_editor` (the *tool*), and the shipped 10‑skill
roster (the *evidence*). **Method:** read the authoritative source, not the docs — `obelisk-bevy`
@ `79882ce` (the `Cargo.lock` pin), `stat_core`/`loot_core` @ `bf9f026`, the editor's `src/skill/**`,
and every `.cast.ron` / `.toml` / `.vfx.ron` in `obelisk-arena`. Every claim below is anchored to a
file:line I verified.

This document is deliberately critical. A short "what's genuinely good" section is included so the
balance is honest, but the point of the exercise is to find what's weak, missing, or misleading.

---

## 0. TL;DR — the five things that matter

1. **Your own authoring docs describe a schema the sim deletes on load.** This is the highest‑leverage
   problem in the whole system and it is nearly free to fix. *(§1)*
2. **The sim's palette is strong and much richer than anything written down** — emitters, point‑anchored
   zones, authored acquisition with fallbacks, real cones, rules‑driven trigger cascades — but it has
   three hard ceilings that define the game's feel: **no crowd control/displacement, no damage falloff,
   and no motion beyond fixed velocity+gravity.** *(§3)*
3. **The editor matches its *approved* spec (cards, not a graph), but it's a better‑labeled wall.** You
   cannot author a chain skill in it at all, most of the rules layer is TOML‑only, and a window rename
   silently rots your VFX bindings to disk. *(§5a)*
4. **The preview is a faithful *executor* of the real sim but a poor *instrument* of it** — it runs the
   authoritative combat code and then throws away every number it computes (damage, health, death,
   crit), and it drops two determinism guarantees the shipping sim holds. *(§5b)*
5. **The content roster is mechanically coherent but narrow, and leans on a bespoke server‑verb escape
   hatch for anything novel** (portals, frost tiles, spires). Whole capability families — Cone,
   damage‑fields, the entire self/ally *support* archetype, status effects beyond `burn` — are unused. *(§4, §2.3)*

---

## 1. The linchpin: the authoring stack teaches a deleted schema

Obelisk underwent an unspec'd **schema‑v2** rework (labeled "Task 9–13" in the code, landed in the week
after the 2026‑07‑02 specs). It **deleted** the entire v1 authored‑causality vocabulary and replaced it:

| v1 (what the docs teach) | v2 (what the sim runs) |
|---|---|
| `spawn_phase: Active` / `spawn_offset` | `spawn: Scheduled(phase: Active, offset: 0.0)` |
| `spawn_phase: Chained` window | **deleted** — chained AoE is now a *separate skill* fired by a rules trigger |
| `on_end: (hit: Some(Chain("blast")))` | **deleted** — rules `[[conditions]] trigger_skill` (`always`/`on_impact`/`on_expire`) |
| `EndReaction::Retarget { … }` | **deleted** — rules `can_chain`/`chain_count` + timeline `chain_radius` (beam‑only) |
| `targeting: SingleEntity` / `delivery: Projectile` | **deleted** — `acquisition: HitscanEntity/GroundPoint/Aim/SelfPoint` |
| `assets/skills/<id>.skillfx.ron` presentation file | **deleted** — inline `cues:` / `charge_cues:` maps inside the `.cast.ron` |

This is not soft deprecation. `CastTimeline` carries `#[serde(deny_unknown_fields)]`
(`obelisk-bevy/src/assets/mod.rs:11`), and a regression test (`old_chain_schema_fails_loud`,
`assets/mod.rs:717-723`) **pins that v1 content fails to parse**. So RON authored from the current
guidance doesn't render wrong — it **hard‑fails to load**.

**What is stale, concretely:**

- **`.claude/skills/arena-skill-design`** — the project's own skill‑authoring guide. Its templates show
  `spawn_phase`, `on_end: Some(Chain(...))`, `on_end: Some(Retarget(...))`, `spawn_phase: Chained`,
  `targeting: SingleEntity`, `delivery: Projectile(...)`, and a `.skillfx.ron` presentation file. **None
  of these exist in v2.** An engineer *or an AI agent* who follows this skill to "add an ice spike spell"
  produces a file the loader rejects. (You watched me load exactly this skill at the top of the session.)
- **`obelisk-bevy/docs/superpowers/specs/2026-07-02-skill-anatomy.md`** — its §6 gap map marks `OnEnd`
  chain‑at‑position and `Retarget` as "✅ shipped," marks emitters + point‑anchored zones as "❌
  increment‑3 candidate," and says ground‑point acquisition is "still open." **All three are wrong now**
  (emitters, `WindowAnchor::CastPoint`, and `GroundPoint{range,fallback}` all ship — see §3).
- **`…/2026-07-02-event-driven-skill-phases.md`** and **`…/2026-07-02-beam-retarget-hitscan.md`** — both
  stamped "SHIPPED," both document the *deleted* authoring surface. (Their *behavioral* claims still
  hold; only the authoring vocabulary is dead.)
- **`obelisk-bevy/CLAUDE.md`** — still describes the timeline as owning `targeting (SelfCast/SingleEntity/
  Direction/Cone)` + `delivery (Melee/Instant/Projectile)` (~lines 148/181/261) and omits the entire v2
  surface (acquisition, emitters, anchors, carriers, beam, cues).
- **Trailing breadcrumbs**: `crates/arena_editor/Cargo.toml:47` and
  `assets/skills/firebolt_trail.vfx.ron:2` still reference `firebolt.skillfx.ron`, a file layer that no
  longer exists.

**The one spec that *is* current** is `…/2026-07-02-skill-editor-reimplementation-design.md` (status
APPROVED). It drove schema‑v2 and explicitly retires the v1 model *and* the node‑graph‑editor vision.
Treat it as the source of truth; treat the other three specs + the skill + both CLAUDE.mds as archaeology
until regenerated.

> **Fix (P0, cheap, unblocks everything):** regenerate `arena-skill-design` and the three stale specs
> from the v2 schema in `assets/mod.rs`, and correct `obelisk-bevy/CLAUDE.md`. Add a one‑line pointer at
> the top of each retired spec: "superseded by schema‑v2; see skill-editor-reimplementation-design.md."
> Nothing else in this review will land for a future author until the map matches the territory.

---

## 2. The relationship: how skills, obelisk, and the editor fit together (v2)

### 2.1 A skill is a three‑layer triad, but the layers are not equal citizens

| Layer | File | Owner | Read by the sim? |
|---|---|---|---|
| **Rules** | `config/skills/<id>.toml` (`stat_core::Skill`) | cost, cooldown, damage math, crit, effect applications, **all causality** (`trigger_skill` conditions), chain flags | **Yes** — the authoritative "what a hit does" + "what it fires next" |
| **Behavior** | `assets/skills/<id>.cast.ron` (`CastTimeline`) | *only* where/when hit volumes exist (windows, shapes, motion, acquisition, emitters, anchors) | **Yes** — the spatial/temporal delivery |
| **Presentation** | inline in the `.cast.ron` — `cues:` + `charge_cues:` | VFX effect / attach / anim / charge params per cue | **No** (`cues` is inert to the sim, `assets/mod.rs:71-79`) — host/editor resolves it |

The mental‑model correction that trips everyone: **cross‑skill causality does not live in the behavior
graph anymore — it lives in the rules TOML.** "Projectile that explodes on impact" is now *two skills*
(`firebolt` + `firebolt_explosion`) wired by three `[[conditions]]` (`always`/`on_impact`/`on_expire`).
A triggered skill runs *its own timeline, spatially at the hit position*, mana‑free, at full charge‑scaled
damage (`combat/system.rs:315-343`, `timeline/triggered.rs:56-164`). The glacier line is a genuine
three‑stage graph: `rolling_glacier → glacier_roll → glacier_burst`.

### 2.2 The cue system is two parallel maps keyed by the same slot names — a hidden coupling

- `vfx_cues: { slot → cue_id }` — **the sim reads this**; a slot's mere presence is what makes obelisk
  *emit* a `CueEvent` (`vfx.rs:49-51`).
- `cues: { cue_id → CueBinding }` — client visuals only.

A designer must keep both in sync by hand (or trust the editor to derive them). Worse, the emitted
`CueEvent` is *also* the trigger for server verbs (below), so a cue string is doing triple duty: it names
a visual, it fires an event, and it may dispatch bespoke gameplay. Renaming a window quietly desynchronizes
all three (see finding **E3**).

### 2.3 The escape hatch: bespoke server verbs (obelisk can't express it, so the arena hand‑codes it)

`crates/arena_game/src/server/verbs.rs::skill_verbs_on_cue` matches `(skill_id, cue_id)` on every
server‑side `CueEvent` and runs hand‑written Rust:

| Skill | Cue | Bespoke mechanic (outside obelisk) |
|---|---|---|
| `portal_blue` / `portal_orange` | `on_window_portal_mark` | raycast + place a portal disc; teleport traversal (`server/portals.rs`, 10 KB) |
| `frost_spire` | `on_window_spike` | consume nearest frost tile → erupt a spire skill object; **custom aim gate** in `cast_pipeline.rs::validate_arena_aim` (must aim at a frost tile) |
| `glacier_roll` | (hitbox polled) | `drop_glacier_trail` lays frost tiles under the rolling volume |

**Four of ten skills depend on this.** It's a clean, documented dispatcher — but it is the seam where the
data‑driven, editor‑previewable, deterministic model *ends*. A designer cannot author, tune, or preview
any of it: portals, the frost‑tile economy, and spires are invisible to the editor and to the preview
sim. Anything genuinely new in this game is currently an engineering task, not a design task. That's the
real answer to "what's the relationship between the skills and obelisk": **the interesting half of the
frost/utility content lives *beside* obelisk, keyed on cue strings, not inside it.**

---

## 3. What obelisk gives you — the corrected palette

Status: **YES** = expressible today · **RULES‑ONLY** = declarable in `stat_core` but no sim geometry ·
**NO** = missing entirely.

| Capability | Status | Evidence |
|---|---|---|
| Phases (windup/active/recovery), speed‑scaled | YES | `timeline/state.rs:52-82`, `advance.rs:446-457` |
| Scheduled windows (`phase` + `offset`) | YES | `assets/mod.rs:301-313` — `offset` is the *only* intra‑phase delay |
| Shapes: **Sphere, Capsule, Cone** (all really simulate) | YES | `assets/mod.rs:335-340`; cone is a true sector, `spatial/cone.rs:5-17` |
| Motion: Static, Linear, Ballistic(+gravity), Beam | YES | `assets/mod.rs:342-362`; fixed velocity only, `projectile.rs:19-26` |
| `MotionDirection` Inherit/Down/Horizontal | YES | `assets/mod.rs:384-401` (falling shards, ground‑rollers) |
| Hit filter Caster/Allies/Enemies/All; mode FirstOnly/OncePerTarget/EveryTick(+rehit) | YES | `assets/mod.rs:403-416`, `spatial/boxes.rs:73-92` |
| Persistent damage field (lava pool) | YES | long Static sphere + EveryTick; circular, does not move/expand |
| Charge byte → 0.5–2.0× speed **and** damage | YES | `timeline/cast.rs:24-26`, `combat/resolve.rs:143-150` |
| **Authored acquisition** Aim/SelfPoint/HitscanEntity/GroundPoint **+ fallback chains** | YES | `assets/mod.rs:212-247` — *ground point is data‑driven now* |
| **Point‑anchored volumes** (`WindowAnchor::CastPoint` + `anchor_offset`) | YES | `assets/mod.rs:319-326` (meteor/storm over a point) |
| **Emitters** (rain child `Template` windows at rate/jitter, deterministic `SpawnRng`) | YES | `assets/mod.rs:371-379`, `advance.rs:575-640` |
| Carrier volumes (`strikes:false`) | YES | `assets/mod.rs:270-273` |
| Rules‑driven chain / hop | YES, **beam‑only** | `advance.rs:723-773`; non‑beam `can_chain` silently never chains |
| Triggered secondary skill runs its own timeline spatially | YES | `combat/system.rs:315-343`; free, full damage, charge inherited |
| Crit / mitigation (armour/resist/barrier/block/DR/elude/leech) | RULES/YES | `combat/resolve.rs:75-94` — rich defensive model |
| Effects/ailments as data (stat‑mods + DoT + charges + stacking) | RULES | `stat_core/types.rs:27,203`; **ailments are string IDs, no fixed enum** |
| ~36 trigger conditions across 5 phases (OnCrit/OnKill/OnImpact/OnExpire/…) | RULES | `loot_core/types.rs:1270`; Lifecycle at `combat/system.rs:66` |
| Damage types: Physical/Fire/Cold/Lightning/Chaos (5) | RULES | `loot_core/types.rs:100` |
| **Pierce** (`pierce_chance`/`pierce_count`) | **NO geometry** | declarable in `stat_core/skill.rs:387-390`, wired nowhere in the sim |

### The ceilings — ranked by how much they constrain the game

1. **No crowd control or displacement — the defining hole.** The sim resolves damage/effects/DoT and
   *never moves a combatant.* Impossible today: knockback nova, black‑hole pull/vortex, root/snare, hard
   stun, freeze‑lock, taunt, silence, leap/charge/dash/blink, pull‑then‑slam combos. The *only* control
   primitive is a soft **action‑speed** slow debuff (`stat_block/computed.rs:56`); even a *movement*‑speed
   slow is positionally inert. For an arena duel game this is the biggest single limiter on skill design
   space.
2. **No damage falloff of any kind.** `resolve.rs:178-211` sums packets; chain hops re‑strike at **full**
   damage. Impossible: diminishing chain lightning, edge‑of‑blast taper, range ramp/falloff, "secondary
   targets take less."
3. **Motion is 4 fixed kinds — no steering.** Velocity frozen at spawn. Impossible: homing seeker,
   orbiting blade/spinning shield, boomerang/returning glaive, arc‑that‑curves‑to‑a‑point.
4. **Volumes are world‑frozen at spawn; they don't follow the caster.** Impossible: a channeled flamethrower
   cone that sweeps as you turn, a damaging aura that trails you, a beam you steer while firing.
   (Point‑anchored zones are fine — they're meant to stay put.)
5. **Only round shapes — no box/wall/line.** No wall of fire, rectangular cleave, laser rectangle,
   advancing wave‑front bar. (Cone covers flamethrowers/melee arcs, but it's a *circular* cone around the
   aim axis and also extends vertically — not a flat ground fan; Capsule orientation is coupled to the
   `Z→aim` arc rotation, `advance.rs:508`, so a capsule "line" won't cleanly align to travel.)
6. **Chaining is narrow:** beam‑only, count‑bounded, no forking/branching, one retarget rule per skill.
7. **Structural caps:** 3 factions with binary friend/foe filtering (no FFA/many‑team); `MAX_TRIGGER_DEPTH
   = 8`; a single `offset` is the only intra‑phase timing knob (no general "wait N seconds then spawn X").

**Worth telling designers these ARE possible** (they read as impossible from the stale docs): a
ground‑targeted meteor, a persistent lava field, a rain‑of‑shards storm (shipping as blizzard), a cone
flamethrower/cleave, a PBAoE nova, a proximity mine, and chain lightning. Budget note: all
chained/emitted/triggered hits are **mana‑free** and deal **full** charge‑scaled damage — a "20‑damage"
firebolt that also triggers a 15 explosion is 35 on a direct hit.

---

## 4. What the content actually uses — a breadth audit

The roster is **10 skills / perfect 1:1 TOML↔cast.ron pairing / all trigger refs resolve / all 8
referenced VFX names resolve / verb cue‑slots present and test‑pinned.** Mechanically it is clean. It is
also *narrow* — it exercises a thin slice of the palette above:

| Heavily used | Used by exactly ONE skill (single point of coverage) | **Dormant (zero skills)** |
|---|---|---|
| Sphere · Static · OncePerTarget · Enemies filter · rules‑trigger causality · `chargeable` | Beam (`chain_lightning`) · HitscanEntity (`chain_lightning`) · Emitter/Template + multi‑window (`blizzard`) · `can_chain` (`chain_lightning`) · Capsule (`frost_spire`) · multi‑tier charge (`firebolt`) · GroundPoint · fallback chain | **Cone** · **EveryTick damage‑field / DoT‑volume** · **Caster/Allies/All filters → the entire self/ally *support* archetype (no heals, buffs, shields)** |

Design‑level observations:

- **There is no support archetype at all.** Every window filters `Enemies`. No skill targets the caster or
  an ally. Heals, shields, buffs, cleanses — the whole cooperative/defensive half of a kit — are not just
  unbuilt, they're unrepresented in the content, even though the sim supports the filters.
- **The "schools" are cosmetic, not mechanical.** Six skills are tagged `cold` and one `lightning`, but
  **only `burn` exists as an effect** (`config/effects/` has one file), and **only `firebolt` applies
  it.** Frost has no chill/slow/freeze; lightning has no shock/stun. Cold and lightning are a damage
  number and a particle color — no identity. This is the highest‑value *content* gap: a status‑effect
  layer would give five of your skills a reason to exist beyond raw damage.
- **Single points of coverage are onboarding + regression risk.** Emitters, beams, chaining, and cones
  each rest on one skill (or zero). Break blizzard and you've lost the only worked example of the emitter
  path; there is no second reference to copy.

---

## 5. The editor, eagle‑eye

The editor is a thin arena shell (`crates/arena_editor` = 3 files) over the *built‑in* obelisk Skill mode
in `bevy_modal_editor` (`src/skill/**`). It faithfully implements its **approved** spec
(`skill-editor-reimplementation-design.md`), which deliberately **walked back** the earlier "the graph is
the editor" vision (`skill-designer-ux-graph.md`, now superseded). So the shipped UI is a single
vertically‑scrolled stack of egui cards — Rules → Behavior → Presentation — with causality shown as
one‑line "→ target" text chips. It is a *better‑labeled* version of the "unusable wall of unlabeled combos"
the earlier spec set out to kill — but structurally it is still a flat wall. A skill *is* a causality
graph; nothing in the tool draws it as one.

### 5a. Authoring surface (`src/skill/panel/**`, `edits.rs`, `save.rs`, `validation.rs`)

| # | Finding | Sev | Evidence |
|---|---|---|---|
| **E1** | **You cannot author a chain skill in the tool.** `can_chain`/`chain_count` are *read* to gate the chain‑radius field and draw the "↺ ×N" chip, but **no widget assigns them** — chain lightning (one of the three reference skills) requires hand‑editing the TOML. The `chain_radius` field even shows "inert — turn on Can Chain," pointing at a toggle that doesn't exist. | HIGH | `panel/behavior.rs:106,163`, `chips.rs:76-79`; no assignment anywhere in `src/skill/` |
| **E2** | **Duplicate / rename / delete are unimplemented in the UI.** The functions (with back‑reference checks) exist and are tested in `library.rs:329-464` but are wired to nothing. A skill's `id` is frozen at creation; you cannot fork an existing skill to iterate, rename a mistake, or delete one — all are drop‑to‑filesystem, and a manual rename desyncs `id`/`skill_id`/trigger refs. | HIGH | `library.rs:329-464` unused; palette only offers New/Rescan/Open (`command_palette/skill_preset.rs:106-129`) |
| **E3** | **Renaming or removing a window silently orphans its cue bindings — permanently, to disk.** `rename_window` repoints `emitter.window` refs but never touches the `cues` map. Orphaned `on_window_<old>`/`on_end_<old>`/`emit_<old>` slots vanish from the UI (Presentation iterates *derived* slots), validation never checks slot↔window correspondence, and Save re‑serializes them anyway. Your authored trail/impact VFX disappears with zero feedback. | HIGH | `edits.rs:317-341` (never touches `cues`), `cue_slots.rs:55-99`, `validation.rs:198-220`, `save.rs:176-182` |
| **E4** | **Most of the rules layer is TOML‑only.** `OWNED_RULES_FIELDS` is 7 keys (`id, name, mana_cost, cooldown, damage, conditions, effect_applications`). Everything else — `tags, targeting, delivery, attack_speed, use_conditions, conditional_modifiers, grants_elude, …` — has no control; the Advanced drawer literally says "Edit the TOML directly." Even inside owned areas, `effect_applications` exposes only `effect_id`+`target` (not `scaling`/`apply_chance`), and `name` has no field, so a template skill shows "New Projectile" forever. | MED‑HIGH | `save.rs:49`, `rules.rs:245-329,741-755`; `stat_core/skill.rs:464-543` |
| **E5** | **Effects are bound blind.** The picker is a flat ComboBox of `EffectLibrary` + `VfxLibrary` names with a "(vfx)" suffix — no hover‑preview, no thumbnail, no spawn‑to‑viewport. D9's whole promise ("hovering an entry spawns the preset at the lane's anchor") is absent; "→ Effect mode" pins the panel but doesn't even select the preset. | MED | `panel/presentation.rs:254-270,425-462`; `mod.rs:569-572` |
| **E6** | **Validation gaps let broken states save green.** Charge‑param name typos are a presentation‑only warning that does *not* block Save; orphan cue slots (E3) are never checked; `can_chain`/`chain_radius` coherence is unchecked; the "cycle" check is a depth‑8 cap, not a real cycle detector (a legit 9‑deep chain and a true A→B→A loop are indistinguishable). | MED | `presentation.rs:41-45,782-789`, `validation.rs:198-220,339-352` |
| **E7** | **Save destroys hand‑authored comments.** `.cast.ron` is fully re‑serialized (RON has no format‑preserving writer), so *all* behavior‑file comments are lost; TOML comments *inside* an owned subtree (`[[conditions]]`, `damage`, `effect_applications`) are destroyed too. The designer who annotates content — exactly your audience — loses it on first Save, no warning. | MED | `save.rs:1-9,41-48,176-182` |
| **E8** | **The timeline strip looks editable but is scrub‑only, and hides emitter output.** Windows paint as draggable yellow bars but a drag only seeks the scrub head — no timing edit (D7 unimplemented). Worse, `Template`/emitted windows have no span and **aren't drawn at all**, so a blizzard's shard rain is invisible on the one surface meant to show time. | MED | `panel/strip.rs:80-91,130-133,184-189,232-246` |
| **E9** | **Emitter/Template authoring is an order‑dependent dance with a reachable Save‑blocking dead‑end.** Guards refuse deletes/flips in the "wrong" order with messages the designer must decode, and `remove_emitter` deliberately leaves an orphaned `Template` that then **blocks Save** until manually cleaned. The tool enforces validity but pushes the bookkeeping onto the designer instead of cascading the fix. | MED | `edits.rs:177-380,543-565`, `validation.rs:321-330` |
| **E10** | **Silent blind‑typing fallbacks.** When the effect registry or preview rig isn't indexed yet (bare session, scene still loading), the effect‑id and Bone‑socket pickers degrade to raw `TextEdit` with no autocomplete and no indication — the exact "exact‑name typing" pain the redesign set out to kill. | LOW‑MED | `rules.rs:252,267-270`, `presentation.rs:589-593` |
| **E11** | Minor: no window reorder (append‑only); no undo; "→ Effect mode" round‑trip half‑built; the chain chip renders like the navigable trigger chips but does nothing on click. | LOW | `edits.rs:158-162`, `chips.rs:116-123` |

### 5b. Preview & feedback loop (`src/skill/preview/**`, `readouts.rs`)

The preview genuinely composes the real obelisk sub‑plugins and resolves real damage — it is not a
re‑implementation. The problem is that it is **a faithful executor but a poor instrument**: it runs the
authoritative sim and then shows you cosmetics and four abstract tick‑marks while discarding every number
the sim produced.

| # | Finding | Sev | Evidence |
|---|---|---|---|
| **P1** | **No results readout — the loop shows visuals, not outcomes.** `DamageResolved{total, life_after, is_crit}` and `EntityDied` fire in‑sim but nothing records or displays them; the "Hit" marker's label is the *window id*, not a number; no health bar; the dummy stays standing at 0 life. A designer's core question — "did that hit, for how much, did it kill?" — is unanswerable from the editor. | HIGH | markers limited to 4 kinds `preview/scrub.rs:179-231`; `combat/system.rs:279`, `events.rs:134` unhandled; only heal reads life `stage.rs:716` |
| **P2** | **Determinism divergence: the FixedUpdate executor is not pinned single‑threaded.** The real `ObeliskSimPlugin` pins `SingleThreaded` to close an entity‑ID‑allocation‑order divergence; the editor composes the sub‑plugins manually and never re‑adds the pin. Order‑sensitive content (chain‑hop tie‑breaks sort by `owner.index()`) can disagree with the game *and with itself* run‑to‑run — directly undercutting the scrub's "same seed → identical" promise. | HIGH | editor has no `set_executor_kind`; real pin `obelisk-bevy/src/lib.rs:129-131`; tie‑break `advance.rs:875` |
| **P3** | **`SpawnRng` is never reset, so emitter/blizzard scrub is non‑deterministic.** Stage reset reseeds `CombatRng` but not `SpawnRng`, and emitter jitter draws from it. Re‑scrubbing a blizzard shows **different shard positions every time** — false exactly for the emitter skills the spec calls a must‑author acceptance case. | HIGH | `stage.rs:673-724` (no `SpawnRng`), `advance.rs:619-623` |
| **P4** | **The real moving hit volume is invisible; the proxy gizmo misleads.** Obelisk `Hitbox` entities carry no avian `Collider`, so physics debug can't draw them; the only shape viz is a *selected‑window, static‑at‑spawn* gizmo that sits at the muzzle while the real hitbox flies 8 m downrange. "Does this window actually overlap the target?" must be inferred from a marker, never seen. | MED‑HIGH | `advance.rs:509-534`, `proxies.rs:119-146,238-255` |
| **P5** | **Readouts are authored‑static, not the live sim's numbers.** The prominent per‑hit / full‑range readout is computed over base TOML damage with a heuristic strike count — it ignores charge, crit, mitigation, and how many strikes actually landed. Balancing off the readout is balancing off a different number than the sim below it deals (especially under the charge slider, which the readout ignores). | MED | `panel/rules.rs:78-125`, `readouts.rs:62,92-153` |
| **P6** | **Unresolved effect names fail silently in the viewport** — a typo'd/renamed effect renders nothing and only warns once to stdout; no in‑editor signal distinguishes "intentionally no visual" from "broken binding." (Pairs with E3/E6.) | MED | `cosmetics.rs:443-449` |
| **P7** | **Cosmetic `Follow` ignores `motion_direction`.** Flight velocity uses `aim_dir`; the authoritative hitbox honors `Down`/`Horizontal`. A falling shard or ground‑roller's trail visibly desyncs from where hits resolve. | MED | `cosmetics.rs:362-368` vs `advance.rs:499-506` |
| **P8** | **The charge slider applies to non‑chargeable skills** — it scales speed+damage the shipping game will never produce, with no hint the slider is inert for this skill. | MED | `stage.rs:883`, `scrub.rs:335` (no `chargeable` gate) |
| **P9** | Structural, documented: flat‑floor `y<0` `OnImpact` plane and scripted aim‑at‑dummy acquisition — a skill that behaves against the scripted dummy line may not against real geometry/targeting. | LOW | `stage.rs:214-231,770-796` |
| **P10** | The headless faithfulness tests run avian in `FixedUpdate`, but the real editor runs it in `FixedPostUpdate` — any lag‑sensitive interaction (fast projectile grazing a moved target) is validated on a different schedule than ships. | LOW | `stage.rs:264-273` vs `plugin.rs:205` |

### 5c. What's genuinely good (so the balance is honest)

- **The scrub is excellent:** synchronous, frame‑accurate, deterministic *prefix re‑sim* with an
  empirically‑grown trailing region, so triggered sub‑casts and chain hops resolve correctly at a frozen
  seek time (`scrub.rs:250-316`). Cross‑skill causality (firebolt→explosion, chain hops, emitters) is
  *real* in preview, not faked.
- **Iteration latency is tight:** the stage is persistent, so cast→see is one frame and tweak→re‑Play is
  instant. The bottleneck is *signal*, not speed.
- **Cue anchoring and projectile integration are faithful** — `on_window`/`on_hit`/`on_end` land where the
  sim resolves them, and the flight proxy uses the sim's exact integrator.
- **Real guardrails exist:** live per‑frame validation with a Save button gated on it; a stale‑disk guard
  with reload/overwrite and a refusal to torn‑write across the two files; format‑preserving TOML for owned
  fields; the `additional` flag auto‑locked for timeline triggers; and honest live readouts with an "≈"
  marker where the math is approximate. The *bones* of a safe tool are here — the gaps are in **reach**
  (what you can author at all) far more than in **safety**.

---

## 6. Content hygiene findings (small, real, cheap)

| # | Finding | Sev | Evidence |
|---|---|---|---|
| **C1** | **Two git‑tracked VFX presets are silently shadowed.** `assets/skills/blizzard_frost.vfx.ron` and `assets/skills/firebolt_trail.vfx.ron` are tracked, but the loader loads `assets/skills` then `assets/vfx` and `.insert()`‑overwrites, so the `assets/vfx/` copies always win — and the pairs **differ** (skills‑dir copies are older). Edit the obvious file next to the skill, see zero in‑game change. Delete the shadowed copies. | MED‑HIGH | `cosmetics.rs:138-159`; `diff` both pairs = differ |
| **C2** | **`blizzard` anchors its cast/charge FX to a fingertip joint** (`R_thumbFinger_joint3_end`) while every other skill and the projectile launch point use `R_wrist_joint` (7× wrist vs 2× thumb). Visible parallax with the wrist‑launched bolt; and if the joint name is wrong it silently falls back to the rig root, never erroring. | MED | `blizzard.cast.ron:83,102`; convention `arena_sim/tuning.rs:35` (`CAST_HAND_OFFSET`) |
| **C3** | **Vestigial rules `targeting`/`delivery`** are `single_enemy`/`projectile` on all 10 skills and are descriptively wrong for 6 (blizzard=ground AoE, portals=self‑utility, chain_lightning=hitscan, frost_spire=ground‑point). The real targeting is the timeline `acquisition`; these fields lie to anyone reading rules first. | MED | every `*.toml`; admitted in `blizzard.toml:9-12` |
| **C4** | **Untracked byte‑identical `Explosion` duplicates** (`assets/vfx/Particle_ Explosion 1.vfx.ron`, `…Particle_ Explosion 1 1.vfx.ron`) are editor Save artifacts that load as orphan library keys nothing references. Repo clutter; delete. | LOW‑MED | `diff -q` vs `Explosion.vfx.ron` = identical; git status untracked |
| **C5** | **`type = "always"` is a correct‑but‑subtle footgun.** firebolt stacks `always`+`on_impact`+`on_expire`; it's safe *only* because `Always` is `PreCalculation` (gated behind an entity hit) and the bolt is `FirstOnly`, so exactly one fires. Copy this pattern onto a `OncePerTarget` skill or add a fourth condition and it misbehaves. The label reads like "on every ending." | LOW‑MED | `firebolt.toml:22-35`; `loot_core/types.rs:1418,1456` |
| **C6** | Thin presentation that reads as silent: `chain_lightning` has no `on_cast` telegraph and reuses `Sparks` for both cast‑arc and hit; `glacier_roll`'s 6.5 s rolling body has no cosmetic of its own (its whole visual is the replicated frost tiles + terminal burst); `blizzard`'s charge tier lacks the `anim`/`duration` its three sibling chargeables carry. | LOW | `chain_lightning.cast.ron:17-18`, `glacier_roll.cast.ron:18-22`, `blizzard.cast.ron:96-115` |

---

## 7. Recommendations, prioritized

**P0 — Fix the map before anything else (hours, not days).** Regenerate `arena-skill-design` and the three
stale specs to the v2 schema; correct `obelisk-bevy/CLAUDE.md`; add "superseded" banners to the retired
specs; drop the dangling `.skillfx.ron` breadcrumbs (`Cargo.toml:47`, `firebolt_trail.vfx.ron:2`). Until
this lands, every future author (human or AI) is led to write RON that hard‑fails. *(§1)*

**P1 — Close the editor's reach gaps (this is where the tool fails its user).** In rough ROI order:
1. Make `can_chain`/`chain_count` editable (E1) — a named reference skill is currently un‑authorable.
2. Cascade cue‑slot keys on window rename/remove, and validate slot↔window correspondence (E3, E6) — this
   is silent data loss.
3. Wire the existing duplicate/rename/delete functions to the palette (E2) — iteration is crippled without
   fork/rename.
4. Add a `name` field and `effect_applications` `scaling`/`apply_chance` (E4).
5. Warn before Save destroys `.cast.ron` comments, or move authored notes to a preserved sidecar (E7).

**P1 — Turn the preview into an instrument.** Surface the numbers the sim already computes: floating damage
on the Hit marker, a dummy health bar + death state, crit/kill/cast‑rejected indicators (P1); make the
readout reflect the *live* charged/crit/mitigated resolution (P5). Then restore the two dropped determinism
guarantees — pin the FixedUpdate executor single‑threaded (P2) and reset `SpawnRng` on stage reset (P3) —
so the scrub's promise is true for emitter skills. Draw the live moving hit volume (P4).

**P2 — Grow content breadth where the palette is already capable.** The cheapest high‑impact content win is
a **status‑effect layer**: give cold a `chill` (action‑speed slow — the one CC primitive the sim *does*
have) and lightning a `shock`, so five skills gain mechanical identity beyond a color (§4). Build one
**support** skill (a self/ally shield or heal) to prove the `Caster`/`Allies` filters and open the whole
dormant archetype. Author one **Cone** and one **EveryTick damage‑field** skill so those paths have a
worked reference. Clean the hygiene items (C1–C4) in an afternoon.

**P2/P3 — Sim increments (design‑gated, not authoring).** If the game wants the feel it currently can't
express, these are engine work, in priority for an arena duel game: **(1) displacement/CC** — even a single
knockback‑impulse primitive would unlock novas, launchers, and peel; **(2) damage falloff** — a per‑hop /
radial multiplier makes chain and AoE tunable; **(3) `pierce_count` geometry** — the rules field already
exists, wired to nothing. Homing/steering and non‑round shapes are lower priority. Each is a sim spec, not
a content task — and each should ship *with* its editor authoring surface and preview support so it doesn't
become the next server‑verb escape hatch.

**A note on the escape hatch (§2.3).** Portals, frost tiles, and spires living in `server/verbs.rs` is a
pragmatic, well‑contained pattern — but every mechanic that goes there is invisible to the designer, the
editor, and the preview, and forks gameplay logic away from the deterministic sim. Treat "does this need a
server verb?" as the signal that a *sim* capability is missing, and let it drive the increment backlog
above rather than accreting silently.

---

## Appendix — the roster at a glance

| Skill | Castable? | Acquisition | Windows / motion | Causality (rules) | Notable |
|---|---|---|---|---|---|
| firebolt | ✅ ember_wand | Aim | 1 · Ballistic | →firebolt_explosion (always/impact/expire); applies burn | only effect‑applier; 2‑tier charge |
| firebolt_explosion | ⛔ triggered | SelfPoint | 1 · Static @CastPoint | — | owns the boom |
| chain_lightning | ✅ storm_staff | HitscanEntity+Fizzle | 1 · **Beam** | rules chain ×3 | only Beam/Hitscan/chain; no telegraph |
| blizzard | ✅ storm_staff | **GroundPoint+Then(SelfPoint)** | 2 · storm(carrier)→**emitter**→shard(Down) | — | only emitter/multi‑window; thumb‑joint FX |
| frost_spire | ✅ potted_spring | GroundPoint+Fizzle | 1 · **Capsule** @CastPoint | — | only Capsule; server verb + frost‑tile aim gate |
| glacier_burst | ⛔ triggered | SelfPoint | 1 · Static r3.5 @CastPoint | — | glacier terminal nova |
| glacier_roll | ⛔ triggered | SelfPoint | 1 · Linear **Horizontal** 6.5 s | →glacier_burst (impact/expire) | server drops frost trail; invisible body |
| rolling_glacier | ✅ potted_spring | Aim | 1 · Ballistic | →glacier_roll(impact)/glacier_burst(expire) | stage‑1 of the glacier graph |
| portal_blue / _orange | ✅ needle_and_thread | Aim | 1 · `strikes:false` | — (zero damage) | pure server‑verb utility |

**Verified against:** obelisk-bevy @ `79882ce`, stat_core/loot_core @ `bf9f026`, editor `bevy_modal_editor`
`src/skill/**`, arena content @ working tree 2026‑07‑09.
