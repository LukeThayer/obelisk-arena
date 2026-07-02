//! The skill designer's egui surfaces, dispatched by the editor's `dispatch_custom_panel`
//! while in Skill mode (UX spec P1):
//!
//! - RIGHT: the selection-driven INSPECTOR (`inspector.rs`) — labeled, grouped properties for
//!   whatever the [`crate::selection::SkillSelection`] points at (skill root / window / lane),
//!   with registry-fed pickers and live validation.
//! - BOTTOM: structure + time — the painted phase/window strip with the scrubber + playhead,
//!   window chips and moment chips that drive the selection, template-based "+ Add", the
//!   open/new-skill header, and Save. (The Rules/Effects tabs survive for advanced editing
//!   until the graph canvas lands in P2.)
//!
//! Save is unified: it always writes the `.cast.ron` (after deriving the locked vfx cues) and
//! the `.skillfx.ron`; the obelisk rules TOML only when `rules.dirty` and every trigger ref
//! resolves (hot-reloading the live `SkillRegistry`); the effect TOML only when `effect.dirty`
//! (hot-swapping the process-global effect registry). The dirty gating matters because opening
//! is load-or-blank — an ungated Save would overwrite a real game-shared TOML with a blank.
//!
//! Runs only windowed (needs a real egui context); `cargo build` is the compile gate and the
//! boot test asserts the resource/registration wiring. The pure helpers live in `derived`,
//! `edits`, `fx_edits`, `timeline_geom` — unit-tested there.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use obelisk_bevy::assets::WindowPhase;

use arena_skills::CueKind;

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
/// `_impact` → OnHit, `_end_*` → OnEnd, otherwise a window cue → OnWindow).
pub(crate) fn kind_for(cue: &str) -> CueKind {
    if cue.ends_with("_cast") {
        CueKind::OnCast
    } else if cue.ends_with("_impact") {
        CueKind::OnHit
    } else if cue.contains("_end_") {
        CueKind::OnEnd
    } else {
        CueKind::OnWindow
    }
}

/// The live registries the inspector's pickers list from, bundled as one `SystemParam` (Bevy
/// caps systems at 16 params). Each is optional — headless test apps lack them.
#[derive(bevy::ecs::system::SystemParam)]
pub struct PanelRegistries<'w> {
    sockets: Option<Res<'w, crate::socket::RigSockets>>,
    anim_lib: Option<Res<'w, bevy_editor_game::AnimationLibrary>>,
    vfx_lib: Option<Res<'w, bevy_vfx::VfxLibrary>>,
}

impl PanelRegistries<'_> {
    fn picker_lists(&self) -> crate::inspector::PickerLists {
        crate::inspector::PickerLists {
            vfx_effects: self
                .vfx_lib
                .as_ref()
                .map(|l| {
                    let mut v: Vec<String> = l.effects.keys().cloned().collect();
                    v.sort();
                    v
                })
                .unwrap_or_default(),
            anim_clips: self
                .anim_lib
                .as_ref()
                .map(|l| {
                    let mut v: Vec<String> = l
                        .clips
                        .keys()
                        .map(|k| k.rsplit("::").next().unwrap_or(k).to_string())
                        .collect();
                    v.sort();
                    v.dedup();
                    v
                })
                .unwrap_or_default(),
            sockets: self
                .sockets
                .as_ref()
                .map(|s| s.names.clone())
                .unwrap_or_default(),
        }
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
    mut scrub: ResMut<crate::scrub::ScrubSim>,
    mut selection: ResMut<crate::selection::SkillSelection>,
    registries: PanelRegistries,
    mut new_id: Local<String>,
    mut stat_query: Local<String>,
    mut new_effect_id: Local<String>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut changed = false;
    let mut fx_changed = false;

    // The selection-driven inspector (UX spec P1): a labeled right panel for whatever is
    // selected — the properties surface; the bottom dock is structure + time only.
    let lists = registries.picker_lists();
    egui::SidePanel::right("skill_inspector")
        .resizable(true)
        .default_width(240.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                crate::inspector::draw_inspector(
                    ui,
                    &mut selection,
                    &mut edited,
                    &mut edited_fx,
                    &mut rules,
                    &lists,
                );
            });
        });
    // Keep the legacy selected_window (gizmo reads it) in sync with the selection model.
    edited.selected_window = selection.window();
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
                    let (c, fc) = draw_timeline_tab(
                        ui,
                        &mut edited,
                        &mut edited_fx,
                        &playhead,
                        &mut scrub,
                        &mut selection,
                    );
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

/// The slim structure-and-time surface (UX spec P1): the painted phase/window strip with the
/// scrubber + playhead, a window CHIP row and a lane CHIP row that drive the [`SkillSelection`]
/// inspector, and a template-based "+ Add" menu. All property editing lives in the inspector.
/// Returns (timeline_changed, fx_changed).
fn draw_timeline_tab(
    ui: &mut egui::Ui,
    edited: &mut EditedSkill,
    edited_fx: &mut EditedSkillFx,
    playhead: &Playhead,
    scrub: &mut crate::scrub::ScrubSim,
    selection: &mut crate::selection::SkillSelection,
) -> (bool, bool) {
    let mut changed = false;

    // The strip spans the phases EXTENDED to the latest window close (chained windows resolved
    // after their parent), so every end moment is reachable by the scrub head.
    let span = strip_span(&edited.timeline).max(0.0001);
    let (rect, strip_resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), STRIP_H),
        egui::Sense::click_and_drag(),
    );
    if strip_resp.clicked() || strip_resp.dragged() {
        if let Some(pos) = strip_resp.interact_pointer_pos() {
            let t = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) * span;
            scrub.target = Some(t);
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
    for (i, w) in edited.timeline.collision_windows.iter().enumerate() {
        let (ws, we) = resolved_window_span(&edited.timeline, w);
        let x0 = time_to_x(ws, span, rect.left(), rect.width());
        let x1 = time_to_x(we, span, rect.left(), rect.width());
        let selected = *selection == crate::selection::SkillSelection::Window(i);
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x0, rect.center().y + 2.0),
                egui::pos2(x1.max(x0 + 2.0), rect.bottom()),
            ),
            2.0,
            if selected {
                egui::Color32::from_rgb(255, 220, 120)
            } else {
                egui::Color32::from_rgb(220, 180, 60)
            },
        );
    }
    if playhead.active && playhead.total > 0.0 {
        let x = time_to_x(playhead.elapsed, span, rect.left(), rect.width());
        p.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(230, 70, 70)),
        );
    } else if scrub.mode != crate::scrub::ScrubMode::Idle {
        // The scrub head shows the SIM's actual clock (the frozen truth), not the request.
        let x = time_to_x(scrub.clock, span, rect.left(), rect.width());
        p.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(240, 160, 50)),
        );
    }
    ui.horizontal(|ui| {
        if ui
            .button("⟳ replay")
            .on_hover_text("re-run the cast at 1× from the start (deterministic)")
            .clicked()
        {
            scrub.replay_requested = true;
        }
        match scrub.mode {
            crate::scrub::ScrubMode::Frozen => {
                ui.label(
                    egui::RichText::new(format!("frozen at {:.2}s — drag to seek", scrub.clock))
                        .small(),
                );
            }
            crate::scrub::ScrubMode::Replaying | crate::scrub::ScrubMode::Seeking => {
                ui.label(egui::RichText::new(format!("{:.2}s", scrub.clock)).small());
            }
            crate::scrub::ScrubMode::Idle => {
                ui.label(
                    egui::RichText::new("drag the strip to scrub the REAL sim; ⟳ replays")
                        .small()
                        .weak(),
                );
            }
        }
        ui.separator();
        ui.label("charge");
        let mut charge = scrub.charge as u32;
        if ui
            .add(egui::Slider::new(&mut charge, 0..=255).show_value(false))
            .on_hover_text(
                "cast charge: 85 ≈ tap (1.0×), 255 = full hold (2.0×) — scales speed AND damage",
            )
            .changed()
        {
            scrub.charge = charge as u8;
        }
        ui.label(
            egui::RichText::new(format!("{:.2}×", 0.5 + (scrub.charge as f32 / 255.0) * 1.5))
                .small(),
        );
    });

    // Window chips — click to inspect. "+ Add" offers the archetype templates.
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Windows").strong());
        let sel_root = *selection == crate::selection::SkillSelection::Root;
        if ui.selectable_label(sel_root, "⚙ skill").clicked() {
            *selection = crate::selection::SkillSelection::Root;
        }
        for (i, w) in edited.timeline.collision_windows.iter().enumerate() {
            let sel = *selection == crate::selection::SkillSelection::Window(i);
            let chained = w.spawn_phase == WindowPhase::Chained;
            let label = if chained {
                format!("» {}", w.id)
            } else {
                w.id.clone()
            };
            if ui.selectable_label(sel, label).clicked() {
                *selection = crate::selection::SkillSelection::Window(i);
            }
        }
        egui::ComboBox::from_id_salt("add_window")
            .selected_text("+ Add")
            .show_ui(ui, |ui| {
                for t in crate::edits::WindowTemplate::ALL {
                    if ui.button(t.label()).clicked() {
                        let idx =
                            crate::edits::add_window_from_template(&mut edited.timeline, t);
                        *selection = crate::selection::SkillSelection::Window(idx);
                        changed = true;
                    }
                }
            });
        // Delete the selected window (chips are the list; deletion lives with them).
        if let crate::selection::SkillSelection::Window(i) = *selection {
            if ui.small_button("❌ delete").clicked()
                && i < edited.timeline.collision_windows.len()
            {
                edited.timeline.collision_windows.remove(i);
                *selection = crate::selection::SkillSelection::Root;
                changed = true;
            }
        }
    });

    // Moment (lane) chips — one per locked cue, in TIMELINE order (cast, then each window's
    // open/end in authored order, then hit), with human labels. ● = has authored visuals.
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Moments").strong());
        let cues = derive_vfx_cues(&edited.timeline);
        let mut moments: Vec<(String, String)> = Vec::new();
        if let Some(v) = cues.get("on_cast") {
            moments.push(("cast".into(), v.clone()));
        }
        for w in &edited.timeline.collision_windows {
            if let Some(v) = cues.get(&format!("on_window_{}", w.id)) {
                moments.push((format!("{} opens", w.id), v.clone()));
            }
            if let Some(v) = cues.get(&format!("on_end_{}", w.id)) {
                moments.push((format!("{} ends", w.id), v.clone()));
            }
        }
        if let Some(v) = cues.get("on_hit") {
            moments.push(("hit".into(), v.clone()));
        }
        for (label, cue_value) in moments {
            let sel = *selection == crate::selection::SkillSelection::Lane(cue_value.clone());
            let has_visuals = edited_fx.fx.lanes.get(&cue_value).is_some_and(|l| {
                l.particle.is_some() || l.projectile.is_some() || l.beam.is_some() || l.anim.is_some()
            });
            // Authored moments read solid; empty ones read as faint opportunities.
            let text = if has_visuals {
                egui::RichText::new(label).strong()
            } else {
                egui::RichText::new(label).weak()
            };
            if ui
                .selectable_label(sel, text)
                .on_hover_text(if has_visuals {
                    "has visuals — click to edit"
                } else {
                    "no visuals yet — click to author"
                })
                .clicked()
            {
                *selection = crate::selection::SkillSelection::Lane(cue_value);
            }
        }
    });

    (changed, false)
}
