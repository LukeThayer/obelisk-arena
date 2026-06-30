//! The editor-preview host: a plain-Avian (no lightyear) physics + obelisk composition that runs the
//! EXACT shared `arena_sim` simulation the live game runs, so "Play the real skill" in the Skill
//! Designer is byte-for-byte the obelisk the game plays.
//!
//! [`ArenaSimPreviewPlugin`] is the editor-side counterpart to the game's `add_avian_with_lightyear`:
//! it owns the sole `PhysicsPlugins` registrant (plain `FixedUpdate` avian, no lightyear transport),
//! installs `Gravity`, and composes obelisk via [`add_obelisk_sim`] with `resolve_hits = true` (the
//! preview is server-authoritative over itself — it owns a local seeded `CombatRng` and resolves its
//! own hits, exactly like the headless server). [`spawn_preview_world`] lays out the standard duel:
//! the static floor + a Player [`PreviewCaster`] at marker 0 + an Enemy [`PreviewDummy`] at marker 1.

use avian3d::prelude::*;
use bevy::prelude::*;
use obelisk_bevy::prelude::Faction;

use crate::obelisk::add_obelisk_sim;
use crate::spawn::{make_arena_combatant, spawn_arena_floor, SPAWN_MARKERS};
use crate::tuning::GRAVITY;

/// Marks the preview's player-controlled caster (Player faction, marker 0).
#[derive(Component)]
pub struct PreviewCaster;

/// Marks the preview's enemy target dummy (Enemy faction, marker 1).
#[derive(Component)]
pub struct PreviewDummy;

/// The editor-preview physics + obelisk host. Owns the sole `PhysicsPlugins` registrant for the
/// preview world (plain `FixedUpdate` avian, no lightyear), installs `Gravity`, and composes the
/// shared obelisk sim with hit resolution ON — the preview resolves its own deterministic combat.
pub struct ArenaSimPreviewPlugin;

impl Plugin for ArenaSimPreviewPlugin {
    fn build(&self, app: &mut App) {
        // Guard against a host that already registered the physics group. `PhysicsPlugins` is a
        // `PluginGroup`, not a `Plugin`, so it can't be passed to `is_plugin_added`; we probe its
        // first sub-plugin (`PhysicsSchedulePlugin`, always added by the group) as the sentinel.
        if !app.is_plugin_added::<PhysicsSchedulePlugin>() {
            app.add_plugins(PhysicsPlugins::new(FixedUpdate));
        }
        app.insert_resource(Gravity(Vec3::new(0.0, -GRAVITY, 0.0)));
        // resolve_hits = true: the preview is authoritative over its own world (local seeded
        // CombatRng + the detect_overlaps funnel), matching the headless server's composition.
        add_obelisk_sim(app, true);
    }
}

/// Lay out the standard preview duel: the static arena floor + a Player [`PreviewCaster`] at spawn
/// marker 0 + an Enemy [`PreviewDummy`] at spawn marker 1 (mutually-opposing factions so the
/// caster's `Enemies`-filtered skills can resolve against the dummy). Bare combatants — NO
/// networking, NO granted skills (the designer drives the cast).
pub fn spawn_preview_world(commands: &mut Commands) {
    spawn_arena_floor(commands);
    let caster = make_arena_combatant(
        commands,
        "preview_caster",
        Faction::Player,
        SPAWN_MARKERS[0],
    );
    let dummy = make_arena_combatant(commands, "preview_dummy", Faction::Enemy, SPAWN_MARKERS[1]);
    commands.entity(caster).insert(PreviewCaster);
    commands.entity(dummy).insert(PreviewDummy);
}
