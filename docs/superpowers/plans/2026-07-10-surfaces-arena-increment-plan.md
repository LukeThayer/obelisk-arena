# Surfaces — Arena Increment (4–5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the obelisk surfaces core (merged upstream at `cd860e2`, pinned in this workspace's
`Cargo.lock`) into the arena: content (`config/surfaces/`), replication + traces + round-reset,
client rendering (decal + VFX), and the glacier/spire migration off the bespoke frost-tile verbs.

**Architecture:** Server runs `ObeliskSurfacesPlugin` (sim behavior, server-only — Stage A: the
client never simulates surfaces); every peer loads `config/surfaces/` via `add_obelisk_surfaces`
(the client needs `[visuals]`). Patches replicate by attaching lightyear components to the sim
entity (the proven `NetworkedSkillObject` pattern). The client renders each replicated patch as a
`ForwardDecal` (bevy 0.18, tinted `assets/textures/decal_splat.png`) + optional looping
`VfxLibrary` preset. Migration flips glacier/spire content onto `paints`/`on_surface` and deletes
the poller + aim-validator arm + tile verb-half. Spec:
`docs/superpowers/specs/2026-07-09-surfaces-ground-effects-design.md` §6–§8 + D9; increment 6
(editor) is a SEPARATE follow-up plan.

**Tech Stack:** Rust, Bevy 0.18.1, avian3d 0.5, lightyear 0.26.4, obelisk-bevy @ cd860e2
(surfaces API: `SurfacePatch{surface,owner,owner_faction,skill_id,radius,remaining,seq}`,
`SurfacePainted`/`SurfaceRemoved{reason}`/`PaintSurface` events, `add_obelisk_surfaces`,
`ObeliskSurfacesPlugin`, `assets::{PaintSpec, PaintMode, SurfaceRequirement}`), jq net-test gate.

## Global Constraints

- Repo: `~/src/obelisk-arena` (the GAME workspace — never touch `crates/arena_editor`), branch
  **`surfaces-arena`** off `master` (create in Task 1).
- **NEVER `git add -A` / `git add .`** — stage explicit paths only. The working tree carries the
  USER's pre-existing uncommitted work (`assets/skills/blizzard.cast.ron`, modified
  `assets/vfx/*.vfx.ron`, untracked `assets/vfx/Particle_ *.vfx.ron`) — do not stage, modify, or
  delete those paths.
- Stage-A invariant: the client NEVER runs surface sim systems (`ObeliskSurfacesPlugin` is added
  ONLY in the server composition branch); clients get the registry (data) + replicated mirrors.
- The net-test gate must stay green: `bash crates/arena_game/tools/net-test/run_session.sh` then
  `bash crates/arena_game/tools/net-test/check_session.sh` (jq gate; python3 not guaranteed).
  Kill strays with `pkill -x arena-server; pkill -x arena-client; pkill -x arena-observer` (exact
  names, `-x` not `-f`). The harness is wall-clock flaky — retry up to 3×; ONE `PASS` is green.
- Trace `extra` fields must never use the key `kind` (harness clobber rule) — use `surface`,
  `reason`, etc.
- Wire bump: `PROTOCOL_ID` 5 → 6 (old peers can't mix — fine, all local).
- Run tests from the workspace root: `cargo test -p arena_game` (+ the full suite in Task 5).
- Every commit message ends with:
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

## File Structure

- Create: `config/surfaces/frost.toml`, `config/surfaces/burning.toml`,
  `config/skills/burning_ground_tick.toml`, `assets/skills/burning_ground_tick.cast.ron`,
  `crates/arena_game/src/server/surfaces.rs` (replication attach + trace observers),
  `crates/arena_game/src/client/surfaces.rs` (trace system Task 2; visuals plugin Task 3),
  `crates/arena_game/tests/content_surfaces.rs`.
- Modify: `crates/arena_sim/src/obelisk.rs` (plugin in the server branch),
  `crates/arena_game/src/bin/server.rs` + `src/client/app_headless.rs` + `src/client/app_windowed.rs`
  (registry load + registrations), `src/net/protocol.rs` + `src/net/mod.rs` (wire),
  `src/server/mod.rs` (wiring), `src/server/rounds.rs` (reset clears patches),
  `src/client/scene.rs` (DepthPrepass), `assets/skills/{firebolt_explosion,glacier_roll,frost_spire}.cast.ron`,
  `src/server/verbs.rs` + `src/server/cast_pipeline.rs` + `src/server/skill_objects.rs` +
  `src/client/skill_objects.rs` (deletions), `tests/content_wisp_ports.rs`,
  `tools/net-test/check_session.sh` + `tools/net-test/summarize.py`, `crates/arena_game/CLAUDE.md`.

---

### Task 1: Surface content + registry/plugin composition

**Files:**
- Create: `config/surfaces/frost.toml`, `config/surfaces/burning.toml`,
  `config/skills/burning_ground_tick.toml`, `assets/skills/burning_ground_tick.cast.ron`
- Modify: `assets/skills/firebolt_explosion.cast.ron`, `crates/arena_sim/src/obelisk.rs`,
  `crates/arena_game/src/bin/server.rs`, `crates/arena_game/src/client/app_headless.rs`,
  `crates/arena_game/src/client/app_windowed.rs`
- Test: `crates/arena_game/tests/content_surfaces.rs`

**Interfaces:**
- Consumes: obelisk `load_surfaces_dir(dir, Option<&SkillRegistry>)`, `add_obelisk_surfaces` (on
  `ObeliskConfigExt`), `ObeliskSurfacesPlugin`, `PaintSpec`/`PaintMode`.
- Produces: surface ids `"frost"`/`"burning"` and skill `"burning_ground_tick"` that Tasks 2–5
  reference; every peer has `SurfaceRegistry`; the SERVER sim paints when firebolt_explosion ends.

- [ ] **Step 0: Branch**

```bash
cd ~/src/obelisk-arena && git checkout -b surfaces-arena
```

- [ ] **Step 1: Write the failing test** (create `crates/arena_game/tests/content_surfaces.rs`)

```rust
//! Content well-formedness for the surfaces arena increment (spec §6-§8): the surface-type
//! TOMLs load + validate against the real skills registry, and the painting/gating content
//! carries the authored fields the sim consumes.
use bevy::prelude::App;
use obelisk_bevy::assets::{Acquisition, CastTimeline, PaintMode};
use obelisk_bevy::prelude::{ObeliskConfigExt, SkillRegistry, SkillSource};
use obelisk_bevy::surfaces::load_surfaces_dir;

fn read(rel: &str) -> String {
    std::fs::read_to_string(arena_game::arena_root().join(rel)).expect(rel)
}

fn timeline(id: &str) -> CastTimeline {
    let tl: CastTimeline = ron::from_str(&read(&format!("assets/skills/{id}.cast.ron")))
        .unwrap_or_else(|e| panic!("{id}.cast.ron parses: {e}"));
    assert_eq!(tl.skill_id, id);
    tl
}

#[test]
fn surface_types_load_and_validate_against_the_real_registries() {
    let mut app = App::new();
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&arena_game::arena_root().join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(
        arena_game::arena_root().join("config/skills"),
    ));
    let reg = app.world().resource::<SkillRegistry>();
    let map = load_surfaces_dir(&arena_game::arena_root().join("config/surfaces"), Some(reg))
        .expect("config/surfaces loads + validates");
    // frost: pure spire fuel, tile-parity numbers (verbs.rs consts it replaces).
    let frost = &map["frost"];
    assert_eq!(frost.lifetime, 180.0);
    assert_eq!(frost.patch_radius, 0.45);
    assert_eq!(frost.max_patches, 64);
    assert!(frost.standing.is_none(), "frost is fuel, no standing payload (v1)");
    // burning: standing tick via the triggered-only skill.
    let burning = &map["burning"];
    let standing = burning.standing.as_ref().expect("burning has standing");
    assert_eq!(standing.tick_skill.as_deref(), Some("burning_ground_tick"));
    assert!(reg.0.contains_key("burning_ground_tick"), "tick skill registered");
    // visuals present for the client renderer.
    assert!(frost.visuals.as_ref().is_some_and(|v| v.decal.is_some()));
    assert!(burning.visuals.as_ref().is_some_and(|v| v.decal.is_some()));
}

#[test]
fn firebolt_explosion_paints_burning_on_end() {
    let tl = timeline("firebolt_explosion");
    let paints = tl.collision_windows[0]
        .paints
        .as_ref()
        .expect("blast paints");
    assert_eq!(paints.surface, "burning");
    assert!(matches!(paints.mode, PaintMode::OnEnd));
    assert!(paints.lifetime.is_some(), "short scorch override, not burning's default");
}

#[test]
fn burning_ground_tick_is_a_triggered_only_castpoint_blast() {
    let tl = timeline("burning_ground_tick");
    assert!(matches!(tl.acquisition, Acquisition::SelfPoint));
    assert!(matches!(
        tl.collision_windows[0].anchor,
        obelisk_bevy::assets::WindowAnchor::CastPoint
    ));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p arena_game --test content_surfaces 2>&1 | tail -5`
Expected: FAIL — `config/surfaces` missing / `firebolt_explosion` has no `paints`.

- [ ] **Step 3: Write the content**

`config/surfaces/frost.toml` (numbers = the `verbs.rs` consts this replaces: radius 0.45,
lifetime 180, cap 64; merge 0.25 = the poller's dedup):
```toml
id = "frost"
lifetime = 180.0
merge_radius = 0.25
max_patches = 64
patch_radius = 0.45

[visuals]
decal = "textures/decal_splat.png"
color = [0.55, 0.85, 1.0, 0.85]
```

`config/surfaces/burning.toml`:
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

`config/skills/burning_ground_tick.toml`:
```toml
id = "burning_ground_tick"
name = "Burning Ground"
tags = ["spell", "fire"]
targeting = "single_enemy"
delivery = "projectile"
mana_cost = 0.0

# TRIGGERED-ONLY (never on a weapon): the burning surface's standing tick — executed AT each
# victim standing in the scorch (obelisk apply_standing_payloads), attributed to the painter.
[damage]
base_damages = [{ type = "fire", min = 4.0, max = 6.0 }]
```

`assets/skills/burning_ground_tick.cast.ron`:
```ron
// The burning surface's standing tick (spec §4): a triggered-only instant blast at the victim's
// position — the firebolt_explosion pattern. SelfPoint + CastPoint = "detonate at the payload
// position" (the sim passes the victim's position as the execution payload).
( skill_id: "burning_ground_tick",
  phase_durations: ( windup: 0.0, active: 0.05, recovery: 0.0 ),
  collision_windows: [
    ( id: "tick", spawn: Scheduled( phase: Active, offset: 0.0 ), anchor: CastPoint,
      active_duration: 0.05, shape: Sphere( radius: 0.6 ), motion: Static,
      hit_filter: Enemies, hit_mode: OncePerTarget ),
  ],
  acquisition: SelfPoint,
)
```

`assets/skills/firebolt_explosion.cast.ron` — add ONE field to the `blast` window (after
`hit_mode: OncePerTarget`, keeping everything else byte-identical):
```ron
      hit_filter: Enemies, hit_mode: OncePerTarget,
      paints: Some(( surface: "burning", radius: 1.2, mode: OnEnd, lifetime: Some(4.0) )) ),
```
and extend the file's header comment with one line: the blast now scorches the ground for 4 s
(burning ticks `burning_ground_tick` at enemies standing in it).

- [ ] **Step 4: Wire the compositions**

`crates/arena_sim/src/obelisk.rs` — in `add_obelisk_sim(app, resolve_hits)`, inside the existing
`if resolve_hits { ... }` branch (beside `ObeliskCombatPlugin`, ~line 61):
```rust
        // Surfaces (ground effects) BEHAVIOR is server-only like combat (Stage A): paint /
        // standing / contact / decay run beside the resolve funnel. Clients only load the
        // registry (add_obelisk_surfaces — data, [visuals]) and render replicated mirrors.
        app.add_plugins(obelisk_bevy::surfaces::ObeliskSurfacesPlugin);
```

All three compositions gain the registry load AFTER their `add_obelisk_skills` line (validation
order — the loader checks `tick_skill`/`trigger_skill` refs against `SkillRegistry`):
- `crates/arena_game/src/bin/server.rs` (~line 49):
- `crates/arena_game/src/client/app_headless.rs` (~line 68):
- `crates/arena_game/src/client/app_windowed.rs` (~line 68):
```rust
    app.add_obelisk_surfaces(&root.join("config/surfaces"));
```
(`ObeliskConfigExt` is already in scope at all three sites — it provides the neighboring
`add_obelisk_effects`/`add_obelisk_skills`.)

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p arena_game --test content_surfaces 2>&1 | tail -5` → `3 passed`.
Then: `cargo test -p arena_game 2>&1 | tail -5` (all green — content-only + additive wiring)
and `cargo check -p arena-server 2>/dev/null || cargo check -p arena_game --bins 2>&1 | tail -3`.

- [ ] **Step 6: Commit**

```bash
git add config/surfaces config/skills/burning_ground_tick.toml \
  assets/skills/burning_ground_tick.cast.ron assets/skills/firebolt_explosion.cast.ron \
  crates/arena_sim/src/obelisk.rs crates/arena_game/src/bin/server.rs \
  crates/arena_game/src/client/app_headless.rs crates/arena_game/src/client/app_windowed.rs \
  crates/arena_game/tests/content_surfaces.rs Cargo.lock
git commit -m "feat(surfaces): arena content + registry/plugin composition

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```
(`Cargo.lock` only if the earlier `cargo update -p obelisk-bevy` left it dirty — check
`git status --short Cargo.lock` first; it was updated pre-plan and may already be staged-clean.)

---

### Task 2: Replication, traces, round-reset clear

**Files:**
- Modify: `crates/arena_game/src/net/protocol.rs`, `crates/arena_game/src/net/mod.rs`
- Create: `crates/arena_game/src/server/surfaces.rs`, `crates/arena_game/src/client/surfaces.rs`
- Modify: `crates/arena_game/src/server/mod.rs`, `crates/arena_game/src/server/rounds.rs`,
  `crates/arena_game/src/client/mod.rs`, `src/client/app_headless.rs`, `src/client/app_windowed.rs`

**Interfaces:**
- Consumes: obelisk `SurfacePatch` (fields `surface`, `owner: Entity`, `radius`),
  `SurfacePainted { patch, surface, position, owner }`,
  `SurfaceRemoved { patch, surface, position, reason }` (reason is `Debug`, not serde),
  `NetworkOwner`, `Replicate`, `trace::event`.
- Produces (Tasks 3+5 rely on): `net::protocol::NetworkedSurfacePatch { surface: String, owner: u64, radius: f32 }`
  replicated to all clients; server trace kinds `surface_painted { surface, pos, owner }` /
  `surface_removed { surface, reason }`; client trace kind `replicated_surface_patch { surface }`
  (BOTH windowed and headless); round reset despawns every patch.

- [ ] **Step 1: Wire types + registration**

`crates/arena_game/src/net/protocol.rs` — after the `NetworkedSkillObject` struct (~line 238):
```rust
/// A replicated surface PATCH (obelisk ground effects, spec §7): one painted splat. `surface`
/// keys the client visual recipe (SurfaceRegistry `[visuals]`); `owner` = painting client id
/// (0 = none); `radius` sizes the decal. Pose rides the replicated avian `Position` (static —
/// set once at attach). The sim entity IS the replicated entity (the skill-object pattern).
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkedSurfacePatch {
    pub surface: String,
    pub owner: u64,
    pub radius: f32,
}
```
and register it right after the `NetworkedSkillObject` registration (~line 65):
```rust
        // --- Surface patches (replicated ground effects — spec §7). Discrete + static:
        // plain registration, no prediction (server-authoritative; visuals ≤ ~50ms late is
        // fine for ground state, and the client never simulates surfaces — Stage A).
        app.register_component::<NetworkedSurfacePatch>();
```

`crates/arena_game/src/net/mod.rs` (~lines 122-124) — bump and re-document:
```rust
/// ... 6 = surfaces wire (NetworkedSurfacePatch replicated ground effects); 5 was the
/// wisp-weapon-ports wire (NetworkedSkillObject replicated world objects). 4 was weapons, 3 ...
pub const PROTOCOL_ID: u64 = 6;
```
(Adapt the existing comment's phrasing — keep its history chain intact.)

- [ ] **Step 2: Server attach + trace observers** (create `crates/arena_game/src/server/surfaces.rs`)

```rust
//! Server-side surfaces bridge (spec §7): attach replication to every sim-spawned
//! [`SurfacePatch`] (the skill-object pattern — the sim entity IS the replicated entity;
//! lightyear despawn-replication handles every removal path: decay, consume, evict, round
//! reset), and trace the paint/remove stream for the net-test harness.
use avian3d::prelude::{Position, Rotation};
use bevy::prelude::*;
use lightyear::prelude::{NetworkTarget, Replicate};
use obelisk_bevy::surfaces::{SurfacePainted, SurfacePatch, SurfaceRemoved};
use serde_json::json;

use crate::net::protocol::{NetworkOwner, NetworkedSurfacePatch};
use crate::trace;

/// Attach replication to freshly-painted patches. Runs in Update (the sim paints in
/// FixedUpdate; `Added` is observed on the next Update pass — same cadence as the rig/visual
/// attach systems). The patch is STATIC: `Position` is set once from the sim `Transform`.
pub(crate) fn attach_patch_replication(
    q: Query<(Entity, &SurfacePatch, &Transform), Added<SurfacePatch>>,
    owners: Query<&NetworkOwner>,
    mut commands: Commands,
) {
    for (e, p, tf) in &q {
        let owner = owners.get(p.owner).map(|o| o.0).unwrap_or(0);
        commands.entity(e).insert((
            Name::new(format!("SurfacePatch({})", p.surface)),
            NetworkedSurfacePatch {
                surface: p.surface.clone(),
                owner,
                radius: p.radius,
            },
            Position(tf.translation),
            Rotation::default(),
            Replicate::to_clients(NetworkTarget::All),
        ));
    }
}

/// Trace observers (the harness substrate — `kind` key reserved, use `surface`/`reason`).
pub(crate) fn trace_surface_painted(ev: On<SurfacePainted>) {
    let e = ev.event();
    trace::event(
        "surface_painted",
        json!({ "surface": e.surface,
                "pos": [e.position.x, e.position.y, e.position.z] }),
    );
}

pub(crate) fn trace_surface_removed(ev: On<SurfaceRemoved>) {
    let e = ev.event();
    trace::event(
        "surface_removed",
        json!({ "surface": e.surface, "reason": format!("{:?}", e.reason) }),
    );
}
```

`crates/arena_game/src/server/mod.rs`: add `pub(crate) mod surfaces;` beside the other module
decls; register `surfaces::attach_patch_replication` in the SAME Update `add_systems` tuple as
the skill-object housekeeping (the tuple containing `skill_objects::reap_skill_objects` at
~line 90); add the two observers beside the existing `.add_observer(verbs::skill_verbs_on_cue)`:
```rust
            .add_observer(surfaces::trace_surface_painted)
            .add_observer(surfaces::trace_surface_removed)
```

- [ ] **Step 3: Round reset clears patches (spec D9)**

`crates/arena_game/src/server/rounds.rs`:
- Import: `use obelisk_bevy::surfaces::SurfacePatch;`
- `run_round_machine` gains a parameter `surface_patches: Query<Entity, With<SurfacePatch>>`
  (append after its existing query params; the system is registered by name in `server/mod.rs`
  so no call-site change beyond the signature).
- At the `reset_for_new_round(...)` call site (~line 398), immediately BEFORE the call, add:
```rust
                // Spec D9: a new round starts on clean ground — despawn every surface patch
                // (replication mirrors the despawn to clients; frost fuel + scorches all reset).
                for patch in &surface_patches {
                    commands.entity(patch).despawn();
                }
```
- If `rounds.rs` has OTHER `reset_for_new_round` call sites (grep first — there is one at ~398;
  the disconnect-fallback path returns to Lobby via the level switch, which does not need the
  clear), apply the same block at each Countdown→Active reset site only.

- [ ] **Step 4: Client-side replicated-patch trace** (create `crates/arena_game/src/client/surfaces.rs`)

```rust
//! Client-side surfaces (spec §6-§7). This module carries the HEADLESS-SAFE trace system
//! (both client roots register it — the net-test asserts replication reached every observer);
//! Task 3 adds the windowed visuals plugin alongside.
use bevy::prelude::*;
use serde_json::json;

use crate::net::protocol::NetworkedSurfacePatch;
use crate::trace;

/// Trace every replicated patch as it materializes (headless + windowed — the harness signal).
pub(crate) fn trace_replicated_patches(
    q: Query<&NetworkedSurfacePatch, Added<NetworkedSurfacePatch>>,
) {
    for p in &q {
        trace::event("replicated_surface_patch", json!({ "surface": p.surface }));
    }
}
```

`crates/arena_game/src/client/mod.rs`: add `pub(crate) mod surfaces;`.
`src/client/app_headless.rs` + `src/client/app_windowed.rs`: register the system in each root's
existing Update `add_systems` block (any tuple that runs unconditionally, e.g. beside the
level/round trace systems):
```rust
    app.add_systems(Update, crate::client::surfaces::trace_replicated_patches);
```

- [ ] **Step 5: Verify (compile + unit) and commit**

Run: `cargo check -p arena_game --all-targets 2>&1 | tail -3` then
`cargo test -p arena_game 2>&1 | tail -4` (existing suites green — this task is wiring; the
behavioral proof is the Task 5 net-test).

```bash
git add crates/arena_game/src/net/protocol.rs crates/arena_game/src/net/mod.rs \
  crates/arena_game/src/server/surfaces.rs crates/arena_game/src/server/mod.rs \
  crates/arena_game/src/server/rounds.rs crates/arena_game/src/client/surfaces.rs \
  crates/arena_game/src/client/mod.rs crates/arena_game/src/client/app_headless.rs \
  crates/arena_game/src/client/app_windowed.rs
git commit -m "feat(surfaces): patch replication (wire v6), paint/remove traces, round-reset clear

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Client rendering — decal + VFX (windowed)

**Files:**
- Modify: `crates/arena_game/src/client/surfaces.rs` (extend), `src/client/scene.rs`,
  `src/client/app_windowed.rs`

**Interfaces:**
- Consumes: `NetworkedSurfacePatch` (Task 2), obelisk `SurfaceRegistry` (loaded Task 1 —
  `[visuals]` decal/color/vfx), bevy `ForwardDecal` + `ForwardDecalMaterial<StandardMaterial>`
  (`bevy::pbr::decal::{ForwardDecal, ForwardDecalMaterial, ForwardDecalMaterialExt}`),
  `DepthPrepass` (`bevy::core_pipeline::prepass::DepthPrepass`), `bevy_vfx` `VfxLibrary` spawn
  (mirror `client/cosmetics.rs`'s library-spawn call + its `ParticleLifetime` drain pattern).
- Produces: every replicated patch renders as a tinted ground decal (+ looping vfx when
  authored); despawn cleans up (children die with the replicated entity).

- [ ] **Step 1: DepthPrepass on the main camera**

`crates/arena_game/src/client/scene.rs` — the `setup_scene` camera spawn tuple (~line 23,
`Camera3d::default(), ... FollowCamera`) gains one component:
```rust
        // ForwardDecal (surface patches) samples the depth prepass to project onto whatever
        // geometry is under the decal volume. Main camera only — the portal render cameras
        // don't get it, so patches don't show through portals (accepted v1; noted in spec §6).
        bevy::core_pipeline::prepass::DepthPrepass,
```

- [ ] **Step 2: Extend `client/surfaces.rs` with the visuals plugin**

Append:
```rust
use bevy::pbr::decal::{ForwardDecal, ForwardDecalMaterial, ForwardDecalMaterialExt};
use bevy::pbr::{ExtendedMaterial, MaterialPlugin, StandardMaterial};
use obelisk_bevy::surfaces::SurfaceRegistry;

/// Windowed-only visuals: one tinted [`ForwardDecal`] (projected `decal_splat.png`) per
/// replicated patch, plus the surface's optional looping vfx preset. Everything spawns as a
/// CHILD of the replicated entity — lightyear's despawn replication (decay/consume/evict/round
/// reset) recursively removes the visuals with it.
pub struct SurfaceVisualsPlugin;
impl Plugin for SurfaceVisualsPlugin {
    fn build(&self, app: &mut App) {
        // StandardMaterial's decal extension needs its MaterialPlugin registered explicitly
        // (guard: PbrPlugin may already register it in this bevy version — the add is
        // idempotent-checked below to avoid the duplicate-plugin panic).
        if !app.is_plugin_added::<MaterialPlugin<ForwardDecalMaterial<StandardMaterial>>>() {
            app.add_plugins(MaterialPlugin::<ForwardDecalMaterial<StandardMaterial>>::default());
        }
        app.add_systems(Update, attach_surface_visuals);
    }
}

fn attach_surface_visuals(
    q: Query<(Entity, &NetworkedSurfacePatch, &avian3d::prelude::Position), Added<NetworkedSurfacePatch>>,
    registry: Option<Res<SurfaceRegistry>>,
    asset_server: Res<AssetServer>,
    mut decal_materials: ResMut<Assets<ForwardDecalMaterial<StandardMaterial>>>,
    vfx: Option<Res<bevy_vfx::VfxLibrary>>,
    mut commands: Commands,
) {
    for (e, p, pos) in &q {
        let visuals = registry
            .as_ref()
            .and_then(|r| r.0.get(&p.surface))
            .and_then(|s| s.visuals.clone())
            .unwrap_or_default();
        let color = visuals
            .color
            .map(|c| Color::srgba(c[0], c[1], c[2], c[3]))
            .unwrap_or(Color::srgba(1.0, 1.0, 1.0, 0.8));
        let texture = visuals
            .decal
            .as_deref()
            .unwrap_or("textures/decal_splat.png")
            .to_string();
        // The replicated patch entity has Position (replicated) but no render Transform —
        // patches are STATIC, so stamp the Transform once and hang children under it.
        commands.entity(e).insert((
            Transform::from_translation(pos.0),
            Visibility::default(),
        ));
        let decal = commands
            .spawn((
                Name::new(format!("SurfaceDecal({})", p.surface)),
                ForwardDecal,
                MeshMaterial3d(decal_materials.add(ForwardDecalMaterial {
                    base: StandardMaterial {
                        base_color: color,
                        base_color_texture: Some(asset_server.load(&texture)),
                        alpha_mode: AlphaMode::Blend,
                        perceptual_roughness: 1.0,
                        ..Default::default()
                    },
                    extension: ForwardDecalMaterialExt {
                        depth_fade_factor: 1.0,
                    },
                })),
                // ForwardDecal's unit mesh projects within its scaled box: XZ = diameter,
                // Y = projection depth (enough to catch gentle slopes/spire bases).
                Transform::from_scale(Vec3::new(p.radius * 2.0, 1.0, p.radius * 2.0)),
            ))
            .id();
        commands.entity(e).add_child(decal);
        if let (Some(vfx_name), Some(vfx_lib)) = (visuals.vfx.as_deref(), vfx.as_ref()) {
            // Mirror client/cosmetics.rs's VfxLibrary spawn (same call + ParticleLifetime
            // bound) — the preset loops at the patch center and dies with the parent.
            // IMPLEMENTATION NOTE: reuse the exact spawn helper cosmetics.rs uses for cue
            // effects (resolve by name from vfx_lib, spawn at Vec3::ZERO relative, parent
            // under `e`); if that helper is private, hoist it to `pub(crate)` rather than
            // duplicating it.
            let _ = (vfx_name, vfx_lib); // replaced by the real spawn in implementation
        }
    }
}
```
The vfx block is the one intentionally-open integration point: read
`crates/arena_game/src/client/cosmetics.rs`'s cue-effect spawn (the `VfxLibrary` tier of
`spawn_cue_cosmetics`) and reuse its spawn call, parenting the spawned effect entity under the
patch entity. Do NOT bound it with `ParticleLifetime` (it loops for the patch's life —
despawn-with-parent is the cleanup); if the helper is private, make it `pub(crate)`.

`src/client/app_windowed.rs` — beside `skill_objects::SkillObjectVisualsPlugin` (~line 132):
```rust
    app.add_plugins(crate::client::surfaces::SurfaceVisualsPlugin);
```

- [ ] **Step 3: Compile + visual verification**

Run: `cargo check -p arena_game --all-targets 2>&1 | tail -3`.
Visual probe (windowed, ~20 s): with a second headless observer as the target, autocast
firebolt and screenshot after the explosion paints:
```bash
pkill -x arena-server; pkill -x arena-client; pkill -x arena-observer; sleep 1
cargo run --bin arena-server &
sleep 3
ARENA_CLIENT_ID=2 ARENA_HEADLESS=1 ARENA_AUTOSTART_LEVEL=arena_flat cargo run --bin arena-client &
sleep 3
ARENA_CLIENT_ID=1 ARENA_AUTOSTART_LEVEL=arena_flat ARENA_AUTOCAST=1 ARENA_AUTOMOVE=1 \
  ARENA_CAM_YAW=-1.5707963 ARENA_SHOT=/tmp/surfaces-decal.png ARENA_SHOT_FRAME=600 \
  ARENA_SMOKE_FRAMES=700 cargo run --bin arena-client
pkill -x arena-server; pkill -x arena-client
```
Read `/tmp/surfaces-decal.png` (the Read tool renders images): an orange-tinted splat must be
visible on the floor where bolts landed. If the decal is invisible/artifacted (DepthPrepass
conflict with the portal cameras' render graph), fall back per spec §6: replace the
`ForwardDecal` child with a soft alpha quad — `Mesh3d(meshes.add(Plane3d::default().mesh().size(p.radius * 2.0, p.radius * 2.0)))`
+ `MeshMaterial3d<StandardMaterial>` (same tint/texture, `alpha_mode: Blend`,
`Transform::from_xyz(0.0, 0.02, 0.0)`) — and note the swap in your report. The patch→visual
seam (this fn) is identical either way.

- [ ] **Step 4: Commit**

```bash
git add crates/arena_game/src/client/surfaces.rs crates/arena_game/src/client/scene.rs \
  crates/arena_game/src/client/app_windowed.rs
git commit -m "feat(surfaces): client decal + vfx rendering for replicated patches

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Migration — glacier trail + spire gate onto surfaces; delete the bespoke layer

**Files:**
- Modify: `assets/skills/glacier_roll.cast.ron`, `assets/skills/frost_spire.cast.ron`,
  `crates/arena_game/src/server/verbs.rs`, `src/server/cast_pipeline.rs`,
  `src/server/skill_objects.rs`, `src/server/mod.rs`, `src/client/skill_objects.rs`,
  `tests/content_wisp_ports.rs`

**Interfaces:**
- Consumes: Tasks 1–3 (frost surface id, replication, rendering); obelisk `on_surface`
  acquisition + `PaintMode::Trail`.
- Produces: the glacier chain runs 100% on surfaces; `KIND_FROST_TILE`, the poller, the
  aim-validator frost arm, and the verb's tile-consume half are GONE.

- [ ] **Step 1: Update the content-pin tests FIRST (they define the migration)**

`tests/content_wisp_ports.rs`:
- In `glacier_timelines_parse_and_chain_hugs_the_ground`, REPLACE the `w.emitter.is_none()`
  assertion block (lines ~38-42) with:
```rust
    // The tile trail is now the authored surfaces painter (spec §8) — the old ARENA poller
    // (drop_glacier_trail) is deleted; painting is a window PROPERTY, so it composes without
    // the Template-lifecycle trap that forced the poller.
    let paints = w.paints.as_ref().expect("roll paints the frost trail");
    assert_eq!(paints.surface, "frost");
    assert!(matches!(
        paints.mode,
        obelisk_bevy::assets::PaintMode::Trail { step } if step == 0.8
    ));
```
- In `spire_and_portals_keep_the_verb_cue_slots`, after the `GroundPoint { .. }` match assert
  (~line 60), add:
```rust
    let Acquisition::GroundPoint { on_surface, .. } = &spire.acquisition else {
        unreachable!()
    };
    let req = on_surface.as_ref().expect("spire gates on frost (spec §5.1)");
    assert_eq!(req.surface, "frost");
    assert!(req.snap && req.consume, "snap to the patch center; consume the fuel at accept");
```

Run: `cargo test -p arena_game --test content_wisp_ports 2>&1 | tail -5` → FAIL (content not
yet flipped).

- [ ] **Step 2: Flip the content**

`assets/skills/glacier_roll.cast.ron` — the `roll` window gains (after `hit_mode: OncePerTarget`):
```ron
      hit_filter: Enemies, hit_mode: OncePerTarget,
      paints: Some(( surface: "frost", radius: 0.45, mode: Trail( step: 0.8 ) )) ),
```
and update the header comment: the frost-tile trail is painted by the window itself now
(surfaces core); the server poller is gone.

`assets/skills/frost_spire.cast.ron` — the acquisition line becomes:
```ron
  acquisition: GroundPoint( range: 60.0, fallback: Fizzle,
    on_surface: Some(( surface: "frost", snap: true, consume: true )) ),
```
and update the header comment: the "must aim at frost" gate + snap + fuel-consume are authored
data now (obelisk `on_surface`); `validate_arena_aim` no longer has a frost arm; the eruption
verb erupts at the cue position (already the snapped patch center) without consuming anything.

- [ ] **Step 3: Delete the bespoke layer**

- `src/server/verbs.rs`:
  - DELETE `drop_glacier_trail` (whole fn, ~lines 244-314) and `TrailMemory` (~lines 64-66).
  - DELETE the consts `FROST_TILE_RADIUS`, `TRAIL_STEP`, `FROST_TILE_LIFETIME`,
    `SPIRE_MATCH_RANGE` (~lines 32-40) — `config/surfaces/frost.toml` + obelisk
    `SURFACE_MATCH_SLACK` own these numbers now.
  - In the `("frost_spire", "on_window_spike")` verb arm (~lines 159-209): delete the
    nearest-tile search + consume (the `nearest_tile` binding, the fizzle-return, and the
    tile-despawn block); the eruption anchors at `ev.position` directly:
```rust
        // --- Frost spire: erupt at the cue position. The position IS the consumed frost
        // patch's center — obelisk's on_surface acquisition (snap: true, consume: true)
        // gated the cast, snapped the cast point, and spent the fuel at cast-accept; this
        // verb only spawns the PHYSICAL spire (a collider-bearing world object stays host
        // territory — spec §8).
        ("frost_spire", "on_window_spike") => {
            let rest = ev.position + Vec3::Y * (SPIRE_HEIGHT * 0.5 - 0.08);
```
    (keep everything from `let start = rest - ...` onward unchanged, including `SpireRise` +
    `settle_spires` + the `spire_erupted` trace; DELETE the `spire_fizzled_no_tile` trace path.)
  - Update the module-header verb table: remove the `glacier_roll (window)` poller row; note
    frost_spire's row now reads "erupt spire at the (pre-snapped) cue position".
- `src/server/cast_pipeline.rs`: in `validate_arena_aim` (~lines 234-264), delete the
  `"frost_spire"` arm so the match is just `_ => Some(aim)`. KEEP the fn + call site as the
  documented seam, and replace the arm with a comment:
```rust
        // (frost_spire's tile gate moved into authored data — obelisk `on_surface` on its
        // GroundPoint acquisition. This seam stays for future gestures obelisk can't express.)
```
  Also update the fn's doc comment and delete the now-unused `skill_objects`/`positions` params
  IF nothing else uses them (compile will tell you — if they become unused, drop them from the
  signature and the call site).
- `src/server/skill_objects.rs`: delete `KIND_FROST_TILE` (~line 33) and its `max_instances`
  arm (~line 41).
- `src/client/skill_objects.rs`: delete the `"frost_tile"` recipe arm (~lines 106-...; the
  translucent cyan puck).
- `src/server/mod.rs`: remove `verbs::drop_glacier_trail,` from the Update tuple (~line 91).

- [ ] **Step 4: Verify + commit**

Run: `cargo test -p arena_game 2>&1 | tail -5` (content_wisp_ports + content_surfaces + all
unit tests green; compile catches any missed reference to the deleted items).

```bash
git add assets/skills/glacier_roll.cast.ron assets/skills/frost_spire.cast.ron \
  crates/arena_game/src/server/verbs.rs crates/arena_game/src/server/cast_pipeline.rs \
  crates/arena_game/src/server/skill_objects.rs crates/arena_game/src/server/mod.rs \
  crates/arena_game/src/client/skill_objects.rs crates/arena_game/tests/content_wisp_ports.rs
git commit -m "feat(surfaces): migrate glacier trail + spire gate onto surfaces; delete tile verbs

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Harness assertions, end-to-end net-test, docs sweep

**Files:**
- Modify: `crates/arena_game/tools/net-test/check_session.sh`,
  `crates/arena_game/tools/net-test/summarize.py`, `crates/arena_game/CLAUDE.md`

**Interfaces:**
- Consumes: Task 2's trace kinds (`surface_painted`, `replicated_surface_patch`); the existing
  session script UNCHANGED (firebolt → firebolt_explosion now paints burning automatically).
- Produces: the gate asserts surfaces replicate end-to-end; docs match reality.

- [ ] **Step 1: Extend the jq gate**

`tools/net-test/check_session.sh` — after the firebolt_explosion server check (~line 28), add:
```bash
# (7) surfaces: the explosion scorches the ground (server paints burning)...
n=$(jq -s '[.[] | select(.kind=="surface_painted" and .surface=="burning")] | length' "$server")
[[ "$n" -ge 1 ]] || note "server painted no burning surface (firebolt_explosion paints OnEnd)"
```
and inside the per-observer loop (after the explosion check, ~line 41), add:
```bash
    # (8) ...and the patch replicated to this observer.
    n=$(jq -s '[.[] | select(.kind=="replicated_surface_patch" and .surface=="burning")] | length' "$f")
    [[ "$n" -ge 1 ]] || note "$name received no replicated burning surface patch"
```

- [ ] **Step 2: Mirror in summarize.py**

Add the same two assertions following the file's existing per-kind counting pattern (read the
M2 assertion block ~lines 60-145 and mirror one existing check verbatim for each: server kind
`surface_painted` with `surface == "burning"` ≥ 1; per-observer kind `replicated_surface_patch`
with `surface == "burning"` ≥ 1, failure strings matching the shell gate's phrasing).

- [ ] **Step 3: Run the gate**

```bash
pkill -x arena-server; pkill -x arena-client; pkill -x arena-observer; sleep 1
bash crates/arena_game/tools/net-test/run_session.sh /tmp/arena-surfaces-test
bash crates/arena_game/tools/net-test/check_session.sh /tmp/arena-surfaces-test
```
Expected: `PASS` (retry ≤3 on the known wall-clock flake; one PASS is green). Then the full
workspace: `cargo test 2>&1 | tail -4` (root workspace only — NOT the editor).

- [ ] **Step 4: Docs sweep** (`crates/arena_game/CLAUDE.md`)

Surgical edits, matching the file's voice:
- Verb table (module responsibility row for `server/verbs.rs`): drop the poller sentence;
  frost_spire row: "erupt at the pre-snapped cue position (fuel consumed by obelisk
  `on_surface` at cast-accept)".
- `server/skill_objects.rs` row: tiles removed from the kind list (portals/spires remain).
- Replication list: add `NetworkedSurfacePatch { surface, owner, radius }` (discrete, static
  pose, wire v6) + one line on `server/surfaces.rs` / `client/surfaces.rs`.
- Trace-kinds list: add `surface_painted`, `surface_removed`, `replicated_surface_patch`;
  remove `glacier_tile_drop`, `spire_fizzled_no_tile`.
- `PROTOCOL_ID` mention: 5 → 6.
- The "Complex spells" section: add surfaces as extension point #0 (check surfaces BEFORE
  reaching for cue verbs/skill objects/aim validators), and delete the poller rationale
  sentence ("deliberately not an emitter...") — painting is a window property now.

- [ ] **Step 5: Commit**

```bash
git add crates/arena_game/tools/net-test/check_session.sh \
  crates/arena_game/tools/net-test/summarize.py crates/arena_game/CLAUDE.md
git commit -m "test(surfaces): net-test asserts paint+replication; docs sweep

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Post-plan notes (for the coordinating session, not a task)

- Increment 6 (editor: authoring UI, preview rendering, stage paint tool) is the NEXT plan —
  includes `cargo update -p obelisk-bevy` in `crates/arena_editor` (its own workspace) and the
  editor-scrub note from the final core review (staged `PaintSurface` paints must re-trigger
  each re-sim).
- Also queued from the core's final review: the spec-§11 `.trace` golden once surfaces enter
  obelisk's `feature_matrix()`; the cross-observer burst-evict residual; `arena-skill-design`
  skill gains paints/on_surface authoring docs.
- Known-good baseline if the net-test flakes persistently: re-run on `master` first to confirm
  the flake predates this branch.
