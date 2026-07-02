//! The bottom-dock phase-timeline egui panel — the skill designer's main authoring surface.
//!
//! `draw_skill_panel` is dispatched by the editor's `dispatch_custom_panel` while in Skill mode. It
//! is an egui shell over the pure helpers (`timeline_geom` / `edits` / `enum_ui` / `model` / `io`):
//!   - a header row: skill id + targeting/delivery ComboBoxes + Save;
//!   - windup/active/recovery `DragValue`s;
//!   - a painted strip: phase bands (top half) + collision-window bars (bottom half) + a live
//!     playhead line driven by `Playhead`;
//!   - a hit-windows list (id/shape/motion/filter/mode/phase/offset/duration + select) + Add-Window.
//! All edits flip `dirty`; Save derives the locked vfx cues and writes the `.cast.ron`.
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
use crate::io::{save_cast_timeline, save_skillfx};
use crate::model::{derive_vfx_cues, EditedSkill, EditedSkillFx};
use crate::preview_controller::Playhead;
use crate::rules_model::EditedRules;
use crate::timeline_geom::{phase_spans, time_to_x, total_duration, window_span};
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
    registry: Option<ResMut<SkillRegistry>>,
    mut tab: ResMut<PanelTab>,
    mut status: ResMut<RulesStatus>,
    playhead: Res<Playhead>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut changed = false;
    let mut fx_changed = false;
    let mut save_clicked = false;
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
                if !status.0.is_empty() {
                    ui.label(egui::RichText::new(&status.0).small().weak());
                }
            });
            match *tab {
                PanelTab::Timeline => {
                    let (c, fc) = draw_timeline_tab(ui, &mut edited, &mut edited_fx, &playhead);
                    changed |= c;
                    fx_changed |= fc;
                }
                PanelTab::Rules => {
                    let ids = effect_id_list();
                    if crate::rules_panel::draw_rules_tab(ui, &mut rules.skill, &ids) {
                        rules.dirty = true;
                    }
                }
                PanelTab::Effects => {
                    ui.label("Effect authoring lands in Stage 3.");
                }
            }
        });
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

/// The M2/M3 timeline surface: targeting/delivery combos, phase DragValues, the painted
/// phase/window strip + playhead, the hit-windows list, and the cosmetic lanes.
/// Returns (timeline_changed, fx_changed).
fn draw_timeline_tab(
    ui: &mut egui::Ui,
    edited: &mut EditedSkill,
    edited_fx: &mut EditedSkillFx,
    playhead: &Playhead,
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
    let span = total_duration(&edited.timeline.phase_durations).max(0.0001);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), STRIP_H),
        egui::Sense::hover(),
    );
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
        let (ws, we) = window_span(w, &edited.timeline.phase_durations);
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
