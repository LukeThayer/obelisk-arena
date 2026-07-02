//! The Skill designer: registers a custom **Skill** editor mode (key `K`, right-side panel) via the
//! generic `register_editor_mode` seam, and (later milestones) drives the bottom-dock timeline that
//! authors obelisk skills + the "Play the real skill" preview.

use crate::model::{blank_cast_timeline, blank_skillfx, EditedSkill, EditedSkillFx};
use bevy::prelude::*;
use bevy_modal_editor::{CustomModeDef, PanelSide, RegisterEditorModeExt};

/// The interned id of the Skill mode (matches `EditorMode::Custom(CustomModeId(SKILL_MODE_ID))`).
pub const SKILL_MODE_ID: &str = "skill";

/// Register the Skill mode with the editor's `CustomModeRegistry`: key `K`, a right-side panel, and
/// the `draw_skill_panel` system. Idempotently callable on any `App` that has a `CustomModeRegistry`
/// (the editor's `EditorStatePlugin` inits it).
pub fn register_skill_mode(app: &mut App) {
    let panel = app.register_system(crate::panel::draw_skill_panel);
    app.register_editor_mode(CustomModeDef {
        id: SKILL_MODE_ID,
        name: "SKILL",
        activation_key: KeyCode::KeyK,
        panel_side: PanelSide::Right,
        panel,
    });
}

/// Merge workspace-authored `.vfx.ron` presets into the live `VfxLibrary` (skipped headless —
/// test apps without the bevy_vfx plugin have no library resource).
fn load_workspace_vfx_presets(lib: Option<ResMut<bevy_vfx::VfxLibrary>>) {
    let Some(mut lib) = lib else {
        return;
    };
    let root = crate::io::editor_root();
    for dir in ["assets/vfx", "assets/skills"] {
        crate::io::load_vfx_presets_into(&mut lib, &root.join(dir));
    }
}

/// Plugin that registers the Skill mode. Added by the windowed binary + the headless editor app.
pub struct SkillDesignerPlugin;

impl Plugin for SkillDesignerPlugin {
    fn build(&self, app: &mut App) {
        register_skill_mode(app);
        app.init_resource::<crate::panel::PanelTab>();
        app.init_resource::<crate::selection::SkillSelection>();
        app.init_resource::<crate::panel::RulesStatus>();
        // The preview lifecycle: a persistent floor at Startup + the Play→duel handler (retires the
        // old idle-startup combatant spawn — combatants now spawn on `GameStartedEvent`).
        app.add_plugins(crate::preview_controller::PreviewControllerPlugin);
        // Draw the selected hit-window's shape as a viewport gizmo while in Skill mode.
        app.add_systems(Update, crate::gizmo::draw_window_gizmo);
        // Index named rig sockets under the preview caster so cosmetic lanes can bind to bones.
        app.init_resource::<crate::socket::RigSockets>()
            .add_systems(Update, crate::socket::index_rig_sockets);
        // Spawn the `character.glb` rig under the preview caster on Play + build/attach its anim graph.
        app.add_plugins(crate::preview_rig::PreviewRigPlugin);
        // Cosmetics: on each fired obelisk `CueEvent`, play the bound `EditedSkillFx` lanes —
        // drive the caster's anim clip + spawn `bevy_vfx` at the resolved socket with baked params.
        // Every spawned cosmetic carries a `CosmeticLifetime` (presets loop forever otherwise).
        app.init_resource::<crate::preview_cosmetics::PreviewCharge>()
            .add_observer(crate::preview_cosmetics::on_preview_cue)
            .add_systems(
                Update,
                (
                    crate::preview_cosmetics::age_preview_cosmetics,
                    crate::preview_cosmetics::fly_preview_cosmetics,
                ),
            );
        // Timeline scrubbing: dragging the panel's phase strip fires the authored cue VFX in the
        // viewport without a Play (synthetic `CueEvent`s through the same observer).
        app.init_resource::<crate::scrub::ScrubState>()
            .add_systems(Update, crate::scrub::fire_scrub_cues);
        // Merge WORKSPACE-authored vfx presets (assets/vfx + assets/skills *.vfx.ron — e.g.
        // firebolt_trail) into the `VfxLibrary`, overriding built-ins — the same dirs the game's
        // `init_vfx_library` scans, so authored-in-designer == rendered-in-game. Startup: after
        // the upstream editor's PreStartup library init.
        app.add_systems(Startup, load_workspace_vfx_presets);
        // Seed the designer with firebolt's real `.cast.ron` if it parses, else a blank timeline
        // pointed at that canonical path (load-or-blank).
        let path = crate::io::default_cast_path("firebolt");
        let timeline = crate::io::load_cast_timeline(&path)
            .unwrap_or_else(|_| blank_cast_timeline("firebolt"));
        app.insert_resource(EditedSkill::from_timeline(timeline, path));
        // Load firebolt's `.skillfx.ron` cosmetic layer if it parses, else a blank one pointed at
        // the canonical path (load-or-blank). Save writes this alongside the `.cast.ron`.
        let fx_path = crate::io::default_skillfx_path("firebolt");
        let fx = crate::io::load_skillfx(&fx_path).unwrap_or_else(|_| blank_skillfx("firebolt"));
        app.insert_resource(EditedSkillFx::from_fx(fx, fx_path));
        // Load firebolt's obelisk rules if they parse, else a blank attack seed at the canonical
        // path (load-or-blank) — the rules side of the skill triad.
        let rules_path = crate::io::default_rules_path("firebolt");
        let skill = crate::io::load_skill_rules(&rules_path)
            .unwrap_or_else(|_| crate::rules_model::blank_attack_skill("firebolt", "Firebolt"));
        app.insert_resource(crate::rules_model::EditedRules::from_skill(skill, rules_path));
        // Seed the Effects tab with the first effect on disk (burn today), else a blank.
        let first = crate::io::list_effect_ids_on_disk().into_iter().next()
            .unwrap_or_else(|| "new_effect".to_string());
        let effect_path = crate::io::default_effect_path(&first);
        let cfg = crate::io::load_effect_config(&effect_path)
            .unwrap_or_else(|_| crate::effect_model::blank_effect(&first));
        app.insert_resource(crate::effect_model::EditedEffect::from_config(cfg, effect_path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_modal_editor::{CustomModeId, CustomModeRegistry};

    /// `register_skill_mode` adds the Skill mode to the registry. Tested on a MINIMAL app (just the
    /// registry) — the full editor app can't advance a frame headlessly (egui/render), and this is
    /// what we actually own: the registration wiring.
    #[test]
    fn register_skill_mode_adds_the_skill_mode_to_the_registry() {
        let mut app = App::new();
        app.init_resource::<CustomModeRegistry>();
        register_skill_mode(&mut app);
        let reg = app.world().resource::<CustomModeRegistry>();
        assert!(
            reg.lookup(CustomModeId(SKILL_MODE_ID)).is_some(),
            "Skill mode should be registered under CustomModeId(\"skill\")"
        );
    }

    #[test]
    fn plugin_seeds_edited_rules_with_the_real_firebolt() {
        let skill = crate::io::load_skill_rules(&crate::io::default_rules_path("firebolt"))
            .expect("real firebolt.toml parses");
        assert_eq!(skill.id, "firebolt");
        assert!((skill.mana_cost - 5.0).abs() < f64::EPSILON);
    }
}
