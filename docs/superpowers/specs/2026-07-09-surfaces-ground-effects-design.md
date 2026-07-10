# Surfaces (Ground Effects) — Design Spec

**Status: APPROVED 2026-07-09** (interview-grounded; all sections approved by Luke; "surfaces
should also support vfx" folded into §6). Scope spans three repos: **obelisk-bevy** (the sim
core — the bulk of the new code), **obelisk-arena** (content, netcode mirror, rendering,
migration), **bevy_modal_editor** (authoring + preview + stage tool).

**Related:** the 2026-07-09 skill-system review
(`docs/superpowers/reviews/2026-07-09-obelisk-skill-system-and-editor-review.md`) identified the
bespoke server-verb frost-tile system as the signal of a missing sim capability; this spec is
that increment. Schema references are v2 (`obelisk-bevy/src/assets/mod.rs`).

---

## 1. Goal

Persistent, typed, paintable **ground state** as a first-class obelisk concept — replacing the
hand-coded frost-tile escape hatch and opening a content class (burning ground, oil, blessed
ground) that today requires engineering per mechanic. Requirements (Luke, 2026-07-09):

- Look natural on the ground; **merge and organically connect**.
- **Detectable when casting** — ice spire requires ice ground to cast on (exists today as
  hard-coded `validate_arena_aim`; must become authored data).
- **Spells trigger skills on contact** with a surface (fire ignites oil).
- Apply **periodic damage, single-instance damage, or statuses** to entities standing in it.
- **Editor support**: authoring, faithful preview, and testability.
- Surfaces support **VFX** (looping ambient effects — flames, mist) in addition to ground visuals.

## 2. The abstraction decision

**"Ground effects" is the right abstraction, understood as persistent typed surface state — not
a "collision area."** A collision area is just a hitbox; obelisk already has those (`EveryTick`
windows). The distinguishing features here are *identity* (a type other systems query), *state*
(persists across casts, merges, is consumable), and *reactivity* (its own payloads and
contact reactions). Two existing systems each cover half:

| | obelisk `EveryTick` windows | arena skill objects (frost tiles) |
|---|---|---|
| Deterministic, editor-previewable | ✅ | ❌ (host-only, invisible to editor/preview) |
| Persistent across casts, queryable, consumable | ❌ (anonymous, per-cast) | ✅ |
| Authored as data | ✅ | ❌ (poller + verb + aim-validator, all hand-coded) |

The feature is the union, built **sim-side (Approach 1, chosen over formalizing the host-side
objects)**: only the sim placement gives determinism, editor preview, and future AI casters for
free, and it closes the escape-hatch pattern the review flagged.

**Geometry: patch records, not a grid** (v1). A patch = circle splat entity with owner + expiry —
semantically ≈ today's 0.45 m tiles, so migration is nearly mechanical; merging is dedup +
visual fusion; a cell grid is the documented upgrade path if patch counts ever explode.

## 3. Decisions (locked in interview)

| # | Decision |
|---|---|
| D1 | v1 content: **frost (migrated) + burning ground**; oil + blessed validate the schema, ship later. |
| D2 | Frost tiles **migrate** (replace, not coexist): poller, aim-validator frost arm, tile verb-half, `KIND_FROST_TILE` all die. |
| D3 | Same-type paint **merges** (dedup radius + visual fusion). Skill-contact triggers ship v1 (oil-ignite is the proving case); a general type-vs-type mixing table is a follow-up. |
| D4 | Payloads v1 = damage (periodic / on-enter-once) + obelisk effects, faction-filtered vs the painter. **No movement modifiers** (client-prediction contract; see §10). |
| D5 | Editor: paint/require authoring, native preview, **stage paint tool** (pre-paint patches to test gated casts). |
| D6 | Surfaces are **sim ECS entities**; the arena replicates them like skill objects (late-join free). |
| D7 | **Consume on cast-accept** (with mana), not at window spawn — an interrupted spire still spends the tile ("paid" philosophy; small delta vs today). |
| D8 | Contact triggers attribute to the **contacting caster** (you ignited it), not the painter. Standing payloads attribute to the **painter**. |
| D9 | **Round reset clears all patches** (today's tiles persisting across rounds is treated as a bug). |
| D10 | Visuals = **decal + optional looping VFX preset** per surface type, authored in the surface TOML's `[visuals]` block (sim-inert data, host-rendered). |

## 4. Content: surface types — `config/surfaces/<id>.toml`

Loaded by a new host API `add_obelisk_surfaces(dir)` (mirrors `add_obelisk_effects`) into a
`SurfaceRegistry` resource — registered by **every peer**: arena server (mechanics), windowed
client (needs `[visuals]`), headless client (harmless no-op consumer), and both editor shells.
Loader validation fails loud (like `trigger_skill` refs): unknown `effect` / `tick_skill` /
`trigger_skill` ids reject the directory.

> The example below is *schema illustration*. v1's shipped `frost.toml` migrates faithfully with
> **no `[standing]` block** (today's tiles are pure spire fuel, and `chill` doesn't exist yet —
> `burn` is the only effect); `burning.toml` is the first standing-payload user.

```toml
id = "frost"                      # burning.toml, oil.toml, blessed.toml — same shape
lifetime = 180.0                  # default patch lifetime (secs)
merge_radius = 0.25               # dedup: skip a paint this close to an existing same-type patch
max_patches = 64                  # per-type cap, replace-oldest

[standing]                        # payload for entities INSIDE the surface (attributed to PAINTER)
filter = "enemies"                # relative to painter: enemies | allies | all
effect = "chill"                  # apply/refresh an obelisk effect while standing
# tick_skill = "burning_ground_tick"  # AND/OR execute a triggered-only skill AT the victim
# rehit_interval = 0.5                #   every N secs (periodic damage via the FULL combat path)
# on_enter_only = true                #   or once per (patch, entity) visit — single-instance

[[on_skill_contact]]              # a HITBOX touching this surface
tags_any = ["fire"]               # match the contacting skill's rules tags
trigger_skill = "oil_ignite"      # execute that skill's timeline AT the contact point
consume = true                    # the contacted patch is consumed

[visuals]                         # sim-inert (like `cues`) — host/editor render these
decal = "surface_frost"           # ground decal texture key
color = [0.6, 0.9, 1.0, 0.8]
vfx = "frost_mist"                # optional looping VfxLibrary preset anchored per patch
```

Two structural properties:
- **A surface never deals damage itself.** It applies effects, or *casts a triggered-only skill
  on you* (`tick_skill`, the `firebolt_explosion` pattern) — crits, mitigation, attribution, and
  golden coverage all ride the existing combat path.
- **Reactions live on the surface, not the skill.** "Oil ignites on fire contact" is a property
  of oil. No `stat_core`/`loot_core` (vothuul/obelisk) changes are needed — contact reactions
  resolve entirely in obelisk-bevy via the existing triggered-execution machinery.

## 5. Sim (obelisk-bevy): schema + systems

### 5.1 Schema additions (all serde-defaulted — every existing `.cast.ron` parses unchanged)

**Paint — new per-window field:**
```ron
paints: Some(( surface: "frost", radius: 0.45, mode: Trail( step: 0.8 ) )),   // glacier_roll
paints: Some(( surface: "burning", radius: 1.5, mode: OnEnd )),               // firebolt_explosion
```
- `Trail(step)`: paint every `step` meters of actual hitbox travel — replaces the arena poller
  AND avoids the Template-lifecycle trap that forced it (Template windows share the parent's
  lifecycle triggers; painting is a window *property*, not a child window).
- `OnEnd`: paint once at the end position (hooks the existing `HitboxEnded` funnel — shards,
  blasts). Patch lifetime defaults from the surface type; optional per-paint override.

**Require/consume — acquisition extension:**
```ron
acquisition: GroundPoint( range: 60.0, fallback: Fizzle,
  on_surface: Some(( surface: "frost", snap: true, consume: true )) ),
```
Checked in `validate_casts` after the point resolves: nearest matching patch within match slack
(patch radius + 0.3, today's `SPIRE_MATCH_RANGE` feel); `snap: true` recenters the cast point on
the patch center; failure runs the normal fallback chain (paid fizzle); `consume: true` removes
the matched patch at cast-accept (D7). Loader validation: `on_surface` on a non-point-producing
acquisition is rejected (same class as the `CastPoint` reachability check).

**Patch entity:** `SurfacePatch { id: u64, surface: String, owner: Entity, skill_id: String,
pos: Vec3, radius: f32, expires_at: Tick }` — plain sim entity, deterministic id counter.

### 5.2 Systems (FixedUpdate, inside the `ObeliskSet` chain)

| System | Placement | Behavior |
|---|---|---|
| `paint_surfaces` | after `Projectiles`, before `ResolveHits` | Trail-step + OnEnd painting; per-type `merge_radius` dedup; per-type `max_patches` replace-oldest evict. Positions from sim state, **no RNG**. |
| `decay_surfaces` | with the expiry systems | fixed-tick lifetime expiry + consume removals. |
| `apply_standing_payloads` | with `ResolveHits` (server-only, like combat) | per-combatant overlap test (XZ disc + ~1.5 m Y tolerance — surfaces are 2.5D in-sim); faction filter vs painter; applies/refreshes `standing.effect`; runs `tick_skill` as a triggered execution at the victim. **Clocks are per (entity, surface-type)** — standing in 3 overlapping burning patches ticks once. `on_enter_only` fires once per (patch, entity). |
| `surface_contact_triggers` | with `ResolveHits` | hitbox-vs-patch overlap matched against `on_skill_contact` by the contacting skill's tags → triggered execution at the contact point, attributed to the contacting caster (D8). **Once per (hitbox, surface-type)** — first contact only; consumption removes the contacted patch. Fire *propagation* along an oil trail is explicitly deferred. |

**Determinism:** patch ids from a counter, positions from sim state, timers on the fixed tick,
zero RNG. A new **golden scenario** locks a trail-paint + standing-tick + oil-ignite chain;
existing goldens stay byte-identical (all fields defaulted).

**2.5D + world geometry:** the sim never learns the ground mesh — patches carry the painting
hitbox's position; queries tolerate ~1.5 m of Y. Visual grounding is the render layer's job
(decals project onto real geometry, §6). If multi-level arenas arrive, a host ground-snap
trigger (the `HitboxWorldHit` pattern) is the documented extension — not v1.

## 6. Rendering (arena, windowed client + editor preview)

`client/surfaces.rs`: for each replicated patch —
- **Decal layer:** a `bevy::pbr::decal::ForwardDecal` (bevy 0.18) projecting the surface's
  `[visuals].decal` texture (tinted `color`) onto whatever geometry is under it — patches of the
  same type visually fuse where they overlap = the "merge and organically connect" requirement,
  with zero sim-side geometry. *Impl caveat:* `ForwardDecal` requires `DepthPrepass` on the
  camera; if that fights the portal render cameras, the documented fallback is soft-edged
  alpha-blended quads — the patch→visual seam is unchanged either way.
- **VFX layer (D10):** if `[visuals].vfx` names a preset, spawn it looping at the patch center;
  on patch removal, stop emission and let live particles drain (the `ParticleLifetime` pattern
  charge-cues use). Density knob deferred until patch counts demand it.

Headless clients render nothing (both visual layers are windowed-only, existing pattern).

## 7. Netcode (arena)

- A thin server system on `Added<SurfacePatch>` attaches replication to **the sim entity itself**
  (the player/skill-object pattern): `Replicate::to_clients(All)` + new
  `NetworkedSurfacePatch { surface: String, owner: u64, radius: f32 }` + avian `Position`.
  Despawn (expiry/consume/evict) replicates automatically; **late-joiners sync for free**.
- **No prediction**: server-authoritative state; visuals arrive ≤ ~50 ms late — acceptable for
  ground state. The client never simulates surfaces (Stage A preserved: no combat, no
  `CombatRng` draws client-side). Wire protocol bumps v5 → v6.
- **Round reset** (D9): `reset_for_new_round` despawns all patches.
- New trace kinds: `surface_painted { surface, pos, owner }`, `surface_removed { reason }`,
  `surface_consumed { skill }`. `glacier_tile_drop` / `spire_fizzled_no_tile` retire **in the
  same change** as a coordinated harness/docs sweep (net-test scripts + `CLAUDE.md` trace list).

## 8. Arena migration (D2)

| Dies | Stays | Content changes |
|---|---|---|
| `drop_glacier_trail` poller + `TrailMemory` | spire verb's *spawn the physical rising spire* half (a collider-bearing world object is genuinely host territory) | `glacier_roll.cast.ron`: `paints: Trail(0.8)` frost |
| `validate_arena_aim` frost arm | portals, all other verbs | `frost_spire.cast.ron`: `on_surface` require+snap+consume |
| frost-consume half of the spire verb | `skill_objects.rs` (portals/spires still use it) | `firebolt_explosion.cast.ron`: `paints: OnEnd` burning |
| `KIND_FROST_TILE` + client puck rendering | | new `config/surfaces/{frost,burning}.toml`; new triggered-only `burning_ground_tick` skill; (later: `oil.toml` + `oil_ignite`, `blessed.toml`) |

`validate_arena_aim` may well die entirely (frost was its only arm) — keep the seam documented
for future gestures the sim genuinely can't express.

## 9. Editor (bevy_modal_editor Skill mode)

- **Authoring:** window card gains a *Paints* section (surface picker fed by `SurfaceRegistry`,
  radius drag, Trail/OnEnd combo + step field); acquisition card gains *Require surface*
  (picker, snap, consume checkboxes). Surface TOMLs are hand-authored v1 (like effects); the
  editor validates all refs — unknown surface/effect/skill ids are **blocking** validation
  errors (same class as emitter-target checks).
- **Preview:** the stage composes the surfaces module automatically (it builds the real sim),
  so patches exist natively — rendered with the same decal+VFX recipes (editor owns a
  `VfxLibrary`). **Scrub is correct for free**: the prefix re-sim deterministically repaints at
  time *t*. Reset clears patches.
- **Stage paint tool (D5):** a command-palette action pre-paints patches into the preview stage
  (session staging resource, never written to `.cast.ron`) — making `frost_spire`'s gate
  **testable in the editor for the first time**.

## 10. Explicitly deferred (so nothing is silently dropped)

- **Movement modifiers** (slippery ice, walk-slow tar): they enter the client-prediction
  contract (both players' movement is client-predicted) — needs client-predicted surface state.
  Design sketch when wanted: replicate patches into the predicted timeline + evaluate in the
  shared controller; until then, "slows" are obelisk action-speed effects (server-side, safe).
- **Type-vs-type mixing table** beyond oil-ignite (fire melts frost, water quenches burning):
  data-driven follow-up on the same patch model.
- **Fire propagation** along a trail (chain-ignite), **grid geometry** (if patch counts explode),
  **multi-level ground snap** (host trigger), **oil/blessed content** (schema-validated, ship
  after v1), **VFX density scaling**.

## 11. Verification

- **obelisk-bevy:** unit tests — trail spacing determinism, cap/evict order, merge dedup,
  standing enter/exit + per-(entity,type) clocks, contact-trigger once-per-(hitbox,type),
  consume-on-accept, loader validation failures. New golden scenario (paint → stand → ignite);
  existing goldens byte-identical.
- **arena:** content tests pinning the migrated glacier/spire triad (the `content_wisp_ports`
  pattern); net-test session extended with `surface_painted`/`surface_consumed` assertions and
  the glacier flow staying green under the conditioned run.
- **editor:** headless preview test — a skill with `paints` produces patches in the stage;
  stage-paint → `frost_spire` cast succeeds; scrub reproduces patches at *t*.

## 12. Build order (increments, each independently green)

1. **obelisk core** — registry + schema + paint/decay/query + goldens (no consumers yet).
2. **standing payloads + contact triggers** — the combat-path integrations.
3. **acquisition `on_surface`** — require/snap/consume.
4. **arena wiring** — replication mirror, rendering (decal+VFX), round-reset clear, traces.
5. **migration** — glacier/spire/firebolt content flips over; poller/validator/verb-half die;
   harness sweep.
6. **editor** — authoring UI, preview rendering, stage paint tool, validation.
