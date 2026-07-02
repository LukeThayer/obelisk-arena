//! The bottom-dock egui panel — the skill designer's main authoring surface, dispatched by the
//! editor's `dispatch_custom_panel` while in Skill mode.
//!
//! A shared header row (skill id, an open/new-skill switcher, the Timeline|Rules|Effects tab
//! strip, Save, and a status line) sits above whichever per-tab surface is active:
//!   - Timeline: the M2/M3 phase strip — targeting/delivery ComboBoxes, windup/active/recovery
//!     `DragValue`s, a painted phase-band + collision-window strip with a live playhead, the
//!     hit-windows list — plus the cosmetic lanes (anim clip / vfx effect / socket / param
//!     bindings). Edits flip `EditedSkill`/`EditedSkillFx` dirty.
//!   - Rules: the obelisk `Skill` form (M4), editing `EditedRules`.
//!   - Effects: the `EffectConfig` form (M4 stage 3), editing `EditedEffect`.
//!
//! Save is unified across all three surfaces: it always writes the `.cast.ron` (after deriving
//! the locked vfx cues) and the `.skillfx.ron` — both editor-owned files with no external
//! validation to fail. It writes the obelisk rules TOML only when `rules.dirty` and every
//! `trigger_skill` ref resolves (the obelisk loader ERRORS on unknown refs), then hot-reloads the
//! live `SkillRegistry`. It writes the effect TOML only when `effect.dirty`, then hot-swaps the
//! process-global effect registry. Gating the rules/effect writes on their own dirty flags matters
//! because `open_skill`/opening an effect is load-or-blank: a file the parser can't read opens as
//! a blank seed with `dirty == false`, and an ungated Save would silently overwrite the real
//! game-shared TOML with that blank.
//!
//! Runs only windowed (needs a real egui context); `cargo build` is the compile gate and the boot
//! test asserts the resource/registration wiring. The pure helpers it calls are unit-tested in their
//! own modules.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use obelisk_bevy::assets::{HitFilter, HitMode, WindowPhase};

use arena_skills::{CueKind, VfxBindSource, VfxParamBinding};

use crate::edits::add_collision_window;
use crate::enum_ui::{
    delivery_index, delivery_variant, motion_index, motion_variant, shape_index, shape_variant,
    targeting_index, targeting_variant, DELIVERY_LABELS, MOTION_LABELS, SHAPE_LABELS,
    TARGETING_LABELS,
};
use crate::fx_edits::{
    add_param_binding, cue_keys_for, ensure_lane, set_anim_clip, set_particle_effect,
    set_particle_socket,
};
use crate::effect_model::{sanitize_id, EditedEffect};
use crate::io::{save_cast_timeline, save_skillfx};
use crate::model::{derive_vfx_cues, EditedSkill, EditedSkillFx};
use crate::preview_controller::Playhead;
use crate::rules_model::EditedRules;
use crate::timeline_geom::{phase_spans, resolved_window_span, strip_span, time_to_x};
use obelisk_bevy::prelude::SkillRegistry;

const STRIP_H: f32 = 40.0;
const PHASE_COLORS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(60, 80, 130),
    egui::Color32::from_rgb(130, 70, 60),
    egui::Color32::from_rgb(60, 110, 80),
];

/// Which authoring surface the bottom dock shows. Timeline = the M2/M3 phase strip + cosmetic
/// lanes; Rules = the obelisk Skill form (M4); Effects = the EffectConfig form (M4 stage 3).
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
pub enum PanelTab {
    #[default]
    Timeline,
    Rules,
    Effects,
}

/// One-line save/reload status shown in the panel header (e.g. "saved; 3 skills reloaded",
/// "rules save blocked: unknown trigger_skill 'x'").
#[derive(Resource, Default)]
pub struct RulesStatus(pub String);

/// Map a locked cue-id VALUE to the `CueKind` its lane reacts to, by suffix (`_cast` → OnCast,
/// `_impact` → OnHit, otherwise a window cue → OnWindow).
fn kind_for(cue: &str) -> CueKind {
    if cue.ends_with("_cast") {
        CueKind::OnCast
    } else if cue.ends_with("_impact") {
        CueKind::OnHit
    } else {
        CueKind::OnWindow
    }
}

/// Effect ids for the pickers, from the live obelisk registry (empty if uninitialized —
/// minimal test apps don't init it; the windowed editor does via PreviewSimConfigPlugin).
fn effect_id_list() -> Vec<String> {
    if stat_core::config::effect_registry_initialized() {
        let mut ids: Vec<String> = stat_core::config::effect_registry()
            .all_ids()
            .into_iter()
            .map(str::to_owned)
            .collect();
        ids.sort();
        ids
    } else {
        Vec::new()
    }
}

/// Draw the bottom-dock timeline panel and apply its edits to `EditedSkill`.
pub fn draw_skill_panel(
    mut contexts: EguiContexts,
    mut edited: ResMut<EditedSkill>,
    mut edited_fx: ResMut<EditedSkillFx>,
    mut rules: ResMut<EditedRules>,
    mut effect: ResMut<EditedEffect>,
    registry: Option<ResMut<SkillRegistry>>,
    mut tab: ResMut<PanelTab>,
    mut status: ResMut<RulesStatus>,
    playhead: Res<Playhead>,
    mut scrub: ResMut<crate::scrub::ScrubState>,
    mut new_id: Local<String>,
    mut stat_query: Local<String>,
    mut new_effect_id: Local<String>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut changed = false;
    let mut fx_changed = false;
    let mut save_clicked = false;
    let mut switch_to: Option<(EditedSkill, EditedSkillFx, EditedRules)> = None;
    egui::TopBottomPanel::bottom("skill_timeline")
        .resizable(true)
        .min_height(180.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&edited.timeline.skill_id).strong());
                ui.selectable_value(&mut *tab, PanelTab::Timeline, "Timeline");
                ui.selectable_value(&mut *tab, PanelTab::Rules, "Rules");
                ui.selectable_value(&mut *tab, PanelTab::Effects, "Effects");
                if ui.button("Save").clicked() {
                    save_clicked = true;
                }
                egui::ComboBox::from_id_salt("skill_picker")
                    .selected_text("open…")
                    .show_ui(ui, |ui| {
                        for id in crate::io::list_skill_ids() {
                            if ui.selectable_label(edited.timeline.skill_id == id, &id).clicked() {
                                switch_to = Some(crate::rules_model::open_skill(&id));
                            }
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut *new_id)
                        .hint_text("new id")
                        .desired_width(80.0),
                );
                for (lab, seed) in [
                    (
                        "+Attack",
                        crate::rules_model::blank_attack_skill as fn(&str, &str) -> stat_core::Skill,
                    ),
                    (
                        "+Spell",
                        crate::rules_model::blank_spell_skill as fn(&str, &str) -> stat_core::Skill,
                    ),
                ] {
                    if ui.button(lab).clicked() {
                        // Sanitize before seeding: the id becomes a filesystem path stem
                        // (config/skills/<id>.toml) and, downstream, a bare TOML string.
                        if let Some(id) = sanitize_id(&new_id) {
                            let (mut c, mut f, mut r) = crate::rules_model::open_skill(&id);
                            r.skill = seed(&id, &id);
                            c.dirty = true;
                            f.dirty = true;
                            r.dirty = true;
                            switch_to = Some((c, f, r));
                            new_id.clear();
                        }
                    }
                }
                if !status.0.is_empty() {
                    ui.label(egui::RichText::new(&status.0).small().weak());
                }
            });
            match *tab {
                PanelTab::Timeline => {
                    let (c, fc) =
                        draw_timeline_tab(ui, &mut edited, &mut edited_fx, &playhead, &mut scrub);
                    changed |= c;
                    fx_changed |= fc;
                }
                PanelTab::Rules => {
                    let ids = effect_id_list();
                    let known = crate::io::list_skill_ids();
                    if crate::rules_panel::draw_rules_tab(ui, &mut rules.skill, &ids, &known) {
                        rules.dirty = true;
                    }
                }
                PanelTab::Effects => {
                    let known = crate::io::list_skill_ids();
                    let (c, open) = crate::effects_panel::draw_effects_tab(
                        ui,
                        &mut effect,
                        &known,
                        &mut stat_query,
                        &mut new_effect_id,
                    );
                    if c {
                        effect.dirty = true;
                    }
                    if let Some(id) = open {
                        let path = crate::io::default_effect_path(&id);
                        let cfg = crate::io::load_effect_config(&path)
                            .unwrap_or_else(|_| crate::effect_model::blank_effect(&id));
                        *effect = crate::effect_model::EditedEffect::from_config(cfg, path);
                    }
                }
            }
        });
    if let Some((c, f, r)) = switch_to {
        *edited = c;
        *edited_fx = f;
        *rules = r;
        status.0 = format!("opened {}", edited.timeline.skill_id);
    }
    if changed {
        edited.dirty = true;
    }
    if fx_changed {
        edited_fx.dirty = true;
    }
    if save_clicked {
        edited.timeline.vfx_cues = derive_vfx_cues(&edited.timeline);
        let path = edited.path.clone();
        if save_cast_timeline(&edited.timeline, &path).is_ok() {
            edited.dirty = false;
        }
        // Save writes BOTH files: the `.cast.ron` above + the `.skillfx.ron` cosmetic layer here.
        if save_skillfx(&edited_fx.fx, &edited_fx.path).is_ok() {
            edited_fx.dirty = false;
        }
        // ...plus the obelisk rules TOML, then hot-reload the live SkillRegistry so the "Play the
        // real skill" preview casts with the just-saved rules.
        //
        // Gated on rules.dirty: `open_skill` is load-or-blank, so a rules file the single-skill
        // parser can't read (or one that was simply never opened this session) opens/starts as a
        // blank seed with dirty == false. An ungated write here would silently overwrite the real
        // game-shared TOML with that blank seed on every Save — a data-destruction vector. The
        // `.cast.ron`/`.skillfx.ron` writes above are independent, editor-owned surfaces and stay
        // unconditional.
        if rules.dirty {
            // Referential-integrity gate: the obelisk loader ERRORS on unknown trigger_skill refs
            // (config/skills.rs:77), so Save must refuse to write the rules file when any exist —
            // the `.cast.ron`/`.skillfx.ron` writes above are independent surfaces and still happen.
            let known: std::collections::HashSet<String> =
                crate::io::list_skill_ids().into_iter().collect();
            let bad = crate::trigger_ui::invalid_trigger_refs(&rules.skill, &known);
            if !bad.is_empty() {
                status.0 = format!("rules save blocked: unknown trigger_skill {bad:?}");
            } else {
                match crate::io::save_skill_rules(&rules.skill, &rules.path) {
                    Ok(()) => {
                        rules.dirty = false;
                        if let Some(mut reg) = registry {
                            match crate::io::reload_skill_registry(&mut reg) {
                                Ok(n) => status.0 = format!("saved; {n} skills reloaded"),
                                Err(e) => status.0 = format!("saved, but skill reload failed: {e}"),
                            }
                        } else {
                            status.0 = "saved (no SkillRegistry to reload)".into();
                        }
                    }
                    Err(e) => status.0 = format!("rules save failed: {e}"),
                }
            }
        }
        if effect.dirty {
            match crate::io::save_effect_config(&effect.config, &effect.path) {
                Ok(()) => {
                    effect.dirty = false;
                    match crate::io::reload_effect_registry() {
                        Ok(n) => {
                            status.0 = format!("{} | {n} effects swapped", status.0);
                        }
                        Err(e2) => status.0 = format!("{} | effect swap failed: {e2}", status.0),
                    }
                }
                Err(e2) => status.0 = format!("{} | effect save failed: {e2}", status.0),
            }
        }
    }
}

/// The M2/M3 timeline surface: targeting/delivery combos, phase DragValues, the painted
/// phase/window strip + playhead, the hit-windows list, and the cosmetic lanes.
/// Returns (timeline_changed, fx_changed).
fn draw_timeline_tab(
    ui: &mut egui::Ui,
    edited: &mut EditedSkill,
    edited_fx: &mut EditedSkillFx,
    playhead: &Playhead,
    scrub: &mut crate::scrub::ScrubState,
) -> (bool, bool) {
    let mut changed = false;
    let mut fx_changed = false;
    ui.horizontal(|ui| {
        let mut ti = targeting_index(&edited.timeline.targeting);
        egui::ComboBox::from_id_salt("targeting")
            .selected_text(TARGETING_LABELS[ti])
            .show_ui(ui, |ui| {
                for (i, l) in TARGETING_LABELS.iter().enumerate() {
                    if ui.selectable_value(&mut ti, i, *l).clicked() {
                        edited.timeline.targeting = targeting_variant(i);
                        changed = true;
                    }
                }
            });
        let mut di = delivery_index(&edited.timeline.delivery);
        egui::ComboBox::from_id_salt("delivery")
            .selected_text(DELIVERY_LABELS[di])
            .show_ui(ui, |ui| {
                for (i, l) in DELIVERY_LABELS.iter().enumerate() {
                    if ui.selectable_value(&mut di, i, *l).clicked() {
                        edited.timeline.delivery = delivery_variant(i);
                        changed = true;
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        let d = &mut edited.timeline.phase_durations;
        for (lab, val) in [
            ("windup", &mut d.windup),
            ("active", &mut d.active),
            ("recovery", &mut d.recovery),
        ] {
            ui.label(lab);
            if ui
                .add(
                    egui::DragValue::new(val)
                        .speed(0.01)
                        .range(0.0..=10.0)
                        .suffix(" s"),
                )
                .changed()
            {
                changed = true;
            }
        }
    });
    // The strip spans the phases EXTENDED to the latest window close (a projectile window
    // routinely outlives the phases — firebolt's bolt flies 2 s past a 0.6 s cast), so the window
    // bar fits and the scrub head can reach the impact moment at the window close.
    let span = strip_span(&edited.timeline).max(0.0001);
    // click_and_drag: the strip doubles as the scrubber — dragging across it fires the authored
    // cue VFX in the viewport (see `crate::scrub`), no Play needed.
    let (rect, strip_resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), STRIP_H),
        egui::Sense::click_and_drag(),
    );
    if strip_resp.clicked() || strip_resp.dragged() {
        if strip_resp.clicked() {
            // A plain click re-arms the grab window so clicking directly ON a cue moment plays
            // it (otherwise a click left of the last scrub position reads as a silent rewind).
            scrub.fired_up_to = None;
        }
        if let Some(pos) = strip_resp.interact_pointer_pos() {
            let t = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) * span;
            scrub.time = Some(t);
        }
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, egui::Color32::from_rgb(24, 24, 28));
    for (i, (s, e)) in phase_spans(&edited.timeline.phase_durations)
        .iter()
        .enumerate()
    {
        let x0 = time_to_x(*s, span, rect.left(), rect.width());
        let x1 = time_to_x(*e, span, rect.left(), rect.width());
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.center().y)),
            0.0,
            PHASE_COLORS[i],
        );
    }
    for w in &edited.timeline.collision_windows {
        let (ws, we) = resolved_window_span(&edited.timeline, w);
        let x0 = time_to_x(ws, span, rect.left(), rect.width());
        let x1 = time_to_x(we, span, rect.left(), rect.width());
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x0, rect.center().y + 2.0),
                egui::pos2(x1.max(x0 + 2.0), rect.bottom()),
            ),
            2.0,
            egui::Color32::from_rgb(220, 180, 60),
        );
    }
    if playhead.active && playhead.total > 0.0 {
        let x = time_to_x(playhead.elapsed, span, rect.left(), rect.width());
        p.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(230, 70, 70)),
        );
    } else if let Some(t) = scrub.time {
        // The scrub head (idle only — the live playhead wins while a cast plays).
        let x = time_to_x(t, span, rect.left(), rect.width());
        p.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(240, 160, 50)),
        );
    }
    ui.horizontal(|ui| {
        ui.label("Hit Windows");
        if ui.button("+ Add").clicked() {
            add_collision_window(&mut edited.timeline);
            changed = true;
        }
    });
    let len = edited.timeline.collision_windows.len();
    for idx in 0..len {
        let selected = edited.selected_window == Some(idx);
        ui.push_id(idx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(selected, &edited.timeline.collision_windows[idx].id)
                    .clicked()
                {
                    edited.selected_window = Some(idx);
                }
                let mut si = shape_index(&edited.timeline.collision_windows[idx].shape);
                egui::ComboBox::from_id_salt("shape")
                    .selected_text(SHAPE_LABELS[si])
                    .show_ui(ui, |ui| {
                        for (i, l) in SHAPE_LABELS.iter().enumerate() {
                            if ui.selectable_value(&mut si, i, *l).clicked() {
                                edited.timeline.collision_windows[idx].shape = shape_variant(i);
                                changed = true;
                            }
                        }
                    });
                let mut mi = motion_index(&edited.timeline.collision_windows[idx].motion);
                egui::ComboBox::from_id_salt("motion")
                    .selected_text(MOTION_LABELS[mi])
                    .show_ui(ui, |ui| {
                        for (i, l) in MOTION_LABELS.iter().enumerate() {
                            if ui.selectable_value(&mut mi, i, *l).clicked() {
                                edited.timeline.collision_windows[idx].motion = motion_variant(i);
                                changed = true;
                            }
                        }
                    });
                // The motion variant's params, editable in place.
                match &mut edited.timeline.collision_windows[idx].motion {
                    obelisk_bevy::assets::VolumeMotion::Static => {}
                    obelisk_bevy::assets::VolumeMotion::Linear { speed } => {
                        if ui
                            .add(egui::DragValue::new(speed).speed(0.1).range(0.0..=100.0).prefix("spd "))
                            .changed()
                        {
                            changed = true;
                        }
                    }
                    obelisk_bevy::assets::VolumeMotion::Ballistic { speed, gravity } => {
                        if ui
                            .add(egui::DragValue::new(speed).speed(0.1).range(0.0..=100.0).prefix("spd "))
                            .changed()
                        {
                            changed = true;
                        }
                        if ui
                            .add(egui::DragValue::new(gravity).speed(0.1).range(0.0..=100.0).prefix("grav "))
                            .changed()
                        {
                            changed = true;
                        }
                    }
                    // Beam: instantaneous strike on the designated target — no motion params.
                    obelisk_bevy::assets::VolumeMotion::Beam => {}
                }
                let f = &mut edited.timeline.collision_windows[idx].hit_filter;
                egui::ComboBox::from_id_salt("filter")
                    .selected_text(format!("{f:?}"))
                    .show_ui(ui, |ui| {
                        for o in [
                            HitFilter::Caster,
                            HitFilter::Allies,
                            HitFilter::Enemies,
                            HitFilter::All,
                        ] {
                            if ui.selectable_value(f, o, format!("{o:?}")).clicked() {
                                changed = true;
                            }
                        }
                    });
                let m = &mut edited.timeline.collision_windows[idx].hit_mode;
                egui::ComboBox::from_id_salt("mode")
                    .selected_text(format!("{m:?}"))
                    .show_ui(ui, |ui| {
                        for o in [
                            HitMode::OncePerTarget,
                            HitMode::FirstOnly,
                            HitMode::EveryTick,
                        ] {
                            if ui.selectable_value(m, o, format!("{o:?}")).clicked() {
                                changed = true;
                            }
                        }
                    });
                let ph = &mut edited.timeline.collision_windows[idx].spawn_phase;
                egui::ComboBox::from_id_salt("phase")
                    .selected_text(format!("{ph:?}"))
                    .show_ui(ui, |ui| {
                        for o in [
                            WindowPhase::Windup,
                            WindowPhase::Active,
                            WindowPhase::Recovery,
                            // Chained: never scheduled — spawns at a parent window's end
                            // position via that parent's `on_end` chain.
                            WindowPhase::Chained,
                        ] {
                            if ui.selectable_value(ph, o, format!("{o:?}")).clicked() {
                                changed = true;
                            }
                        }
                    });
                let w = &mut edited.timeline.collision_windows[idx];
                if ui
                    .add(
                        egui::DragValue::new(&mut w.spawn_offset)
                            .speed(0.01)
                            .range(0.0..=10.0)
                            .prefix("off "),
                    )
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .add(
                        egui::DragValue::new(&mut w.active_duration)
                            .speed(0.01)
                            .range(0.0..=10.0)
                            .prefix("dur "),
                    )
                    .changed()
                {
                    changed = true;
                }
                // The window's on_end reaction: what its termination spawns at the end position
                // — chain a window there, or RETARGET (seek the nearest un-struck enemy and
                // beam the window onto it; hit-reason only, hop-bounded). v1 UI sets the chain
                // on ALL THREE reasons and retarget on hit — the schema stays per-reason.
                use obelisk_bevy::assets::{EndReaction, OnEnd};
                let other_ids: Vec<String> = edited
                    .timeline
                    .collision_windows
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != idx)
                    .map(|(_, w)| w.id.clone())
                    .collect();
                let self_id = edited.timeline.collision_windows[idx].id.clone();
                let w = &mut edited.timeline.collision_windows[idx];
                let current = match &w.on_end.hit {
                    Some(EndReaction::Chain(id)) => format!("chain {id}"),
                    Some(EndReaction::Retarget { window, .. }) => format!("hop {window}"),
                    None => "(none)".to_string(),
                };
                egui::ComboBox::from_id_salt("on_end")
                    .selected_text(format!("end→{current}"))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current == "(none)", "(none)").clicked() {
                            w.on_end = Default::default();
                            changed = true;
                        }
                        for id in &other_ids {
                            let label = format!("chain {id}");
                            if ui.selectable_label(current == label, &label).clicked() {
                                let chain = Some(EndReaction::Chain(id.clone()));
                                w.on_end = OnEnd {
                                    hit: chain.clone(),
                                    world: chain.clone(),
                                    fuse: chain,
                                };
                                changed = true;
                            }
                        }
                        // Retarget may target any window INCLUDING this one (self-hop, the
                        // chain-lightning shape) — the hop counter bounds the cycle.
                        for id in other_ids.iter().chain([&self_id]) {
                            let label = format!("hop {id}");
                            if ui.selectable_label(current == label, &label).clicked() {
                                w.on_end = OnEnd {
                                    hit: Some(EndReaction::Retarget {
                                        window: id.clone(),
                                        radius: 6.0,
                                        max_hops: 3,
                                    }),
                                    world: None,
                                    fuse: None,
                                };
                                changed = true;
                            }
                        }
                    });
                if let Some(EndReaction::Retarget {
                    radius, max_hops, ..
                }) = &mut w.on_end.hit
                {
                    if ui
                        .add(egui::DragValue::new(radius).speed(0.1).range(0.5..=30.0).prefix("r "))
                        .changed()
                    {
                        changed = true;
                    }
                    let mut hops = *max_hops as u32;
                    if ui
                        .add(egui::DragValue::new(&mut hops).range(1..=16).prefix("hops "))
                        .changed()
                    {
                        *max_hops = hops as u8;
                        changed = true;
                    }
                }
            });
        });
    }

    ui.separator();
    ui.label("Cosmetic Lanes");
    for cue in cue_keys_for(&edited.timeline) {
        let kind = kind_for(&cue);
        ui.push_id(&cue, |ui| {
            ui.horizontal(|ui| {
                ui.label(&cue);
                let lane = ensure_lane(&mut edited_fx.fx, &cue, kind);

                // Anim clip text field (Task 29 upgrades this to a clip ComboBox).
                let mut clip = lane
                    .anim
                    .as_ref()
                    .and_then(|a| a.clip.clone())
                    .unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut clip).hint_text("anim clip"))
                    .changed()
                {
                    let next = if clip.is_empty() { None } else { Some(clip) };
                    set_anim_clip(lane, next, 0, 1.0);
                    fx_changed = true;
                }

                // Particle effect name text field.
                let mut effect = lane
                    .particle
                    .as_ref()
                    .and_then(|p| p.effect.clone())
                    .unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut effect).hint_text("vfx effect"))
                    .changed()
                {
                    set_particle_effect(
                        lane,
                        if effect.is_empty() {
                            None
                        } else {
                            Some(effect)
                        },
                    );
                    fx_changed = true;
                }

                // Socket text field (Task 28 upgrades this to a RigSockets ComboBox).
                let mut socket = lane
                    .particle
                    .as_ref()
                    .and_then(|p| p.socket.clone())
                    .unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut socket).hint_text("(root)"))
                    .changed()
                {
                    set_particle_socket(
                        lane,
                        if socket.is_empty() {
                            None
                        } else {
                            Some(socket)
                        },
                    );
                    fx_changed = true;
                }

                if ui.button("+ charge→scale").clicked() {
                    add_param_binding(
                        lane,
                        VfxParamBinding {
                            param: "scale".into(),
                            source: VfxBindSource::Charge,
                            min: 0.5,
                            max: 2.0,
                        },
                    );
                    fx_changed = true;
                }
            });
        });
    }
    (changed, fx_changed)
}
