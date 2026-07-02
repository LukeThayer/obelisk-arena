//! The selection-driven inspector (UX spec P1, D4/D5/D11): a labeled, vertical, grouped
//! right-side panel showing the properties of whatever [`SkillSelection`] points at —
//! the skill root (aim, phases, tier-1 rules + derived readouts), a collision window
//! (shape / motion / hits / timing / on-end), or a cosmetic lane (registry-fed PICKERS for
//! anim clip, vfx effect, and socket — never type an exact name again). Inline validation
//! renders here live, not as a status string after Save.
//!
//! Windowed-only (needs a real egui context), like `panel.rs`; the pure helpers it leans on
//! (`derived`, `edits`, `fx_edits`, `timeline_geom`) are unit-tested in their modules.

use bevy_egui::egui;
use obelisk_bevy::assets::{EndReaction, HitFilter, HitMode, OnEnd, VolumeMotion, WindowPhase};

use crate::enum_ui::{
    motion_index, motion_variant, shape_index, shape_variant, targeting_index, targeting_variant,
    MOTION_LABELS, SHAPE_LABELS,
};
use crate::model::{derive_vfx_cues, EditedSkill, EditedSkillFx};
use crate::rules_model::EditedRules;
use crate::selection::SkillSelection;

/// Registry name lists the pickers feed from — collected by the panel system from the live
/// resources (empty when a registry is absent, e.g. headless).
pub struct PickerLists {
    pub vfx_effects: Vec<String>,
    pub anim_clips: Vec<String>,
    pub sockets: Vec<String>,
}

/// Draw the inspector contents for the current selection. Mutates the edited triad directly
/// and sets the dirty flags. Returns nothing — all feedback is inline.
#[allow(clippy::too_many_arguments)]
pub fn draw_inspector(
    ui: &mut egui::Ui,
    selection: &mut SkillSelection,
    edited: &mut EditedSkill,
    edited_fx: &mut EditedSkillFx,
    rules: &mut EditedRules,
    lists: &PickerLists,
) {
    // Clamp a stale window selection (deleted window) back to root.
    if let SkillSelection::Window(i) = *selection {
        if i >= edited.timeline.collision_windows.len() {
            *selection = SkillSelection::Root;
        }
    }
    // Live validation, shown wherever it bites.
    let validation = obelisk_bevy::assets::validate_timeline(&edited.timeline).err();

    match selection.clone() {
        SkillSelection::Root => draw_root(ui, edited, rules, validation.as_deref()),
        SkillSelection::Window(idx) => draw_window(ui, edited, idx, validation.as_deref()),
        SkillSelection::Lane(cue) => draw_lane(ui, edited, edited_fx, &cue, lists),
    }
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(egui::RichText::new(title).strong())
        .default_open(true)
        .show(ui, body);
    ui.add_space(4.0);
}

/// Two-column labeled row helper: `label  <widget>`.
fn row(ui: &mut egui::Ui, label: &str, widget: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([70.0, 18.0], egui::Label::new(label));
        changed = widget(ui);
    });
    changed
}

fn drag(ui: &mut egui::Ui, v: &mut f32, speed: f64, range: std::ops::RangeInclusive<f32>, suffix: &str) -> bool {
    ui.add(egui::DragValue::new(v).speed(speed).range(range).suffix(suffix))
        .changed()
}

// ---------------------------------------------------------------------------------------
// Root: the skill itself — identity, aim, phases, tier-1 rules, derived readouts.
// ---------------------------------------------------------------------------------------

fn draw_root(
    ui: &mut egui::Ui,
    edited: &mut EditedSkill,
    rules: &mut EditedRules,
    validation: Option<&str>,
) {
    ui.label(egui::RichText::new(&edited.timeline.skill_id).heading());
    if let Some(err) = validation {
        ui.colored_label(egui::Color32::from_rgb(240, 90, 70), format!("⚠ {err}"));
    }
    ui.add_space(4.0);

    section(ui, "Aim", |ui| {
        let mut ti = targeting_index(&edited.timeline.targeting);
        // Say what the variants MEAN, not just their names (UX: hitscan is a behavior).
        const AIM_LABELS: [&str; 4] = [
            "Self cast",
            "Hitscan target (SingleEntity)",
            "Free aim (Direction)",
            "Cone",
        ];
        row(ui, "mode", |ui| {
            let mut changed = false;
            egui::ComboBox::from_id_salt("insp_targeting")
                .selected_text(AIM_LABELS[ti])
                .show_ui(ui, |ui| {
                    for (i, l) in AIM_LABELS.iter().enumerate() {
                        if ui.selectable_value(&mut ti, i, *l).clicked() {
                            edited.timeline.targeting = targeting_variant(i);
                            changed = true;
                        }
                    }
                });
            changed
        })
        .then(|| edited.dirty = true);
    });

    section(ui, "Phases", |ui| {
        let d = &mut edited.timeline.phase_durations;
        let mut c = false;
        c |= row(ui, "windup", |ui| drag(ui, &mut d.windup, 0.01, 0.0..=10.0, " s"));
        c |= row(ui, "active", |ui| drag(ui, &mut d.active, 0.01, 0.0..=10.0, " s"));
        c |= row(ui, "recovery", |ui| drag(ui, &mut d.recovery, 0.01, 0.0..=10.0, " s"));
        if c {
            edited.dirty = true;
        }
    });

    section(ui, "Cost", |ui| {
        let mut c = false;
        c |= row(ui, "mana", |ui| {
            ui.add(egui::DragValue::new(&mut rules.skill.mana_cost).speed(0.1).range(0.0..=1000.0))
                .changed()
        });
        c |= row(ui, "cooldown", |ui| {
            ui.add(
                egui::DragValue::new(&mut rules.skill.cooldown)
                    .speed(0.05)
                    .range(0.0..=120.0)
                    .suffix(" s"),
            )
            .changed()
        });
        if c {
            rules.dirty = true;
        }
    });

    section(ui, "Damage", |ui| {
        let mut c = false;
        let mut remove: Option<usize> = None;
        for (i, d) in rules.skill.damage.base_damages.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{:?}", d.damage_type));
                c |= ui
                    .add(egui::DragValue::new(&mut d.min).speed(0.5).range(0.0..=100000.0).prefix("min "))
                    .changed();
                c |= ui
                    .add(egui::DragValue::new(&mut d.max).speed(0.5).range(0.0..=100000.0).prefix("max "))
                    .changed();
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            rules.skill.damage.base_damages.remove(i);
            c = true;
        }
        if ui.button("+ damage line").clicked() {
            crate::rules_edits::add_base_damage(&mut rules.skill);
            c = true;
        }
        c |= row(ui, "crit", |ui| {
            ui.add(
                egui::DragValue::new(&mut rules.skill.damage.base_crit_chance)
                    .speed(0.005)
                    .range(0.0..=1.0),
            )
            .changed()
        });
        if c {
            rules.dirty = true;
        }
    });

    // The derived readouts (D11): what the numbers MEAN, live.
    section(ui, "Computed", |ui| {
        let r = crate::derived::skill_readout(&rules.skill, &edited.timeline);
        ui.label(format!("per hit: {:.0}–{:.0}", r.per_hit.0, r.per_hit.1));
        if r.max_strikes > 1 {
            ui.label(format!(
                "full chain ({} strikes): {:.0}–{:.0}",
                r.max_strikes, r.full_chain.0, r.full_chain.1
            ));
        }
        if let Some((lo, hi)) = r.per_mana {
            ui.label(format!("dmg per mana: {lo:.1}–{hi:.1}"));
        }
        if r.crit_chance > 0.0 {
            ui.label(format!("crit: {:.0}%", r.crit_chance * 100.0));
        }
    });

    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("Advanced rules (triggers, effects, conversions): Rules tab")
            .small()
            .weak(),
    );
}

// ---------------------------------------------------------------------------------------
// Window: shape / motion / hits / timing / on end.
// ---------------------------------------------------------------------------------------

fn draw_window(
    ui: &mut egui::Ui,
    edited: &mut EditedSkill,
    idx: usize,
    validation: Option<&str>,
) {
    let ids: Vec<String> = edited
        .timeline
        .collision_windows
        .iter()
        .map(|w| w.id.clone())
        .collect();
    let Some(w) = edited.timeline.collision_windows.get_mut(idx) else {
        return;
    };
    let mut changed = false;

    ui.label(egui::RichText::new(format!("window: {}", w.id)).heading());
    if let Some(err) = validation {
        if err.contains(&w.id) {
            ui.colored_label(egui::Color32::from_rgb(240, 90, 70), format!("⚠ {err}"));
        }
    }
    ui.add_space(4.0);

    section(ui, "Shape", |ui| {
        let mut si = shape_index(&w.shape);
        changed |= row(ui, "shape", |ui| {
            let mut c = false;
            egui::ComboBox::from_id_salt("insp_shape")
                .selected_text(SHAPE_LABELS[si])
                .show_ui(ui, |ui| {
                    for (i, l) in SHAPE_LABELS.iter().enumerate() {
                        if ui.selectable_value(&mut si, i, *l).clicked() {
                            w.shape = shape_variant(i);
                            c = true;
                        }
                    }
                });
            c
        });
        use obelisk_bevy::assets::CollisionShape;
        match &mut w.shape {
            CollisionShape::Sphere { radius } => {
                changed |= row(ui, "radius", |ui| drag(ui, radius, 0.02, 0.05..=20.0, " m"));
            }
            CollisionShape::Capsule { radius, height } => {
                changed |= row(ui, "radius", |ui| drag(ui, radius, 0.02, 0.05..=20.0, " m"));
                changed |= row(ui, "height", |ui| drag(ui, height, 0.02, 0.05..=20.0, " m"));
            }
            CollisionShape::Cone { angle, range } => {
                changed |= row(ui, "angle", |ui| drag(ui, angle, 0.5, 1.0..=360.0, "°"));
                changed |= row(ui, "range", |ui| drag(ui, range, 0.05, 0.1..=50.0, " m"));
            }
        }
    });

    section(ui, "Motion", |ui| {
        let mut mi = motion_index(&w.motion);
        changed |= row(ui, "motion", |ui| {
            let mut c = false;
            egui::ComboBox::from_id_salt("insp_motion")
                .selected_text(MOTION_LABELS[mi])
                .show_ui(ui, |ui| {
                    for (i, l) in MOTION_LABELS.iter().enumerate() {
                        if ui.selectable_value(&mut mi, i, *l).clicked() {
                            w.motion = motion_variant(i);
                            c = true;
                        }
                    }
                });
            c
        });
        match &mut w.motion {
            VolumeMotion::Static => {
                ui.label(egui::RichText::new("anchored where it spawns").small().weak());
            }
            VolumeMotion::Linear { speed } => {
                changed |= row(ui, "speed", |ui| drag(ui, speed, 0.1, 0.5..=100.0, " m/s"));
            }
            VolumeMotion::Ballistic { speed, gravity } => {
                changed |= row(ui, "speed", |ui| drag(ui, speed, 0.1, 0.5..=100.0, " m/s"));
                changed |= row(ui, "gravity", |ui| drag(ui, gravity, 0.1, 0.0..=100.0, " m/s²"));
            }
            VolumeMotion::Beam => {
                ui.label(
                    egui::RichText::new("strikes the aimed/retargeted victim instantly")
                        .small()
                        .weak(),
                );
            }
        }
    });

    section(ui, "Hits", |ui| {
        changed |= row(ui, "filter", |ui| {
            let mut c = false;
            egui::ComboBox::from_id_salt("insp_filter")
                .selected_text(format!("{:?}", w.hit_filter))
                .show_ui(ui, |ui| {
                    for o in [HitFilter::Caster, HitFilter::Allies, HitFilter::Enemies, HitFilter::All] {
                        if ui.selectable_value(&mut w.hit_filter, o, format!("{o:?}")).clicked() {
                            c = true;
                        }
                    }
                });
            c
        });
        changed |= row(ui, "mode", |ui| {
            let mut c = false;
            egui::ComboBox::from_id_salt("insp_mode")
                .selected_text(match w.hit_mode {
                    HitMode::OncePerTarget => "Once per target",
                    HitMode::FirstOnly => "First hit only",
                    HitMode::EveryTick => "Every tick",
                })
                .show_ui(ui, |ui| {
                    for (o, l) in [
                        (HitMode::OncePerTarget, "Once per target"),
                        (HitMode::FirstOnly, "First hit only"),
                        (HitMode::EveryTick, "Every tick"),
                    ] {
                        if ui.selectable_value(&mut w.hit_mode, o, l).clicked() {
                            c = true;
                        }
                    }
                });
            c
        });
        let mut rehit = w.rehit_interval.unwrap_or(0.0);
        if row(ui, "rehit", |ui| drag(ui, &mut rehit, 0.02, 0.0..=10.0, " s")) {
            w.rehit_interval = (rehit > 0.0).then_some(rehit);
            changed = true;
        }
    });

    section(ui, "Timing", |ui| {
        if w.spawn_phase == WindowPhase::Chained {
            ui.label(
                egui::RichText::new("Chained: spawns where its parent window ends")
                    .small()
                    .weak(),
            );
        } else {
            changed |= row(ui, "phase", |ui| {
                let mut c = false;
                egui::ComboBox::from_id_salt("insp_phase")
                    .selected_text(format!("{:?}", w.spawn_phase))
                    .show_ui(ui, |ui| {
                        for o in [
                            WindowPhase::Windup,
                            WindowPhase::Active,
                            WindowPhase::Recovery,
                            WindowPhase::Chained,
                        ] {
                            if ui.selectable_value(&mut w.spawn_phase, o, format!("{o:?}")).clicked() {
                                c = true;
                            }
                        }
                    });
                c
            });
            changed |= row(ui, "offset", |ui| drag(ui, &mut w.spawn_offset, 0.01, 0.0..=10.0, " s"));
        }
        changed |= row(ui, "fuse", |ui| drag(ui, &mut w.active_duration, 0.01, 0.01..=30.0, " s"));
        if w.spawn_phase == WindowPhase::Chained {
            // Offer the way back to a scheduled phase.
            if ui.small_button("make scheduled (Active)").clicked() {
                w.spawn_phase = WindowPhase::Active;
                changed = true;
            }
        }
    });

    section(ui, "On end", |ui| {
        let self_id = w.id.clone();
        let current = match &w.on_end.hit {
            Some(EndReaction::Chain(id)) => format!("chain → {id}"),
            Some(EndReaction::Retarget { window, .. }) => format!("hop → {window}"),
            None => "nothing".into(),
        };
        changed |= row(ui, "ending", |ui| {
            let mut c = false;
            egui::ComboBox::from_id_salt("insp_on_end")
                .selected_text(&current)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current == "nothing", "nothing").clicked() {
                        w.on_end = OnEnd::default();
                        c = true;
                    }
                    for id in ids.iter().filter(|i| **i != self_id) {
                        let l = format!("chain → {id}");
                        if ui.selectable_label(current == l, &l).clicked() {
                            let chain = Some(EndReaction::Chain(id.clone()));
                            w.on_end = OnEnd { hit: chain.clone(), world: chain.clone(), fuse: chain };
                            c = true;
                        }
                    }
                    for id in ids.iter() {
                        let l = format!("hop → {id}");
                        if ui.selectable_label(current == l, &l).clicked() {
                            w.on_end = OnEnd {
                                hit: Some(EndReaction::Retarget {
                                    window: id.clone(),
                                    radius: 6.0,
                                    max_hops: 3,
                                }),
                                world: None,
                                fuse: None,
                            };
                            c = true;
                        }
                    }
                });
            c
        });
        if let Some(EndReaction::Retarget { radius, max_hops, .. }) = &mut w.on_end.hit {
            changed |= row(ui, "hop radius", |ui| drag(ui, radius, 0.1, 0.5..=30.0, " m"));
            let mut hops = *max_hops as u32;
            if row(ui, "max hops", |ui| {
                ui.add(egui::DragValue::new(&mut hops).range(1..=16)).changed()
            }) {
                *max_hops = hops as u8;
                changed = true;
            }
        }
        ui.label(
            egui::RichText::new("chain: spawn that window where this one ends\nhop: beam it onto the nearest un-struck enemy")
                .small()
                .weak(),
        );
    });

    if changed {
        edited.dirty = true;
    }
}

// ---------------------------------------------------------------------------------------
// Lane: pickers for anim clip / vfx effect / socket — no exact-name typing.
// ---------------------------------------------------------------------------------------

/// A `(none) + entries` picker over registry names. Returns Some(new_value) on change, where
/// `None` inside means "cleared".
#[allow(clippy::option_option)]
fn name_picker(
    ui: &mut egui::Ui,
    salt: &str,
    current: Option<&str>,
    entries: &[String],
) -> Option<Option<String>> {
    let mut picked: Option<Option<String>> = None;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(current.unwrap_or("(none)"))
        .show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), "(none)").clicked() {
                picked = Some(None);
            }
            for e in entries {
                if ui.selectable_label(current == Some(e.as_str()), e).clicked() {
                    picked = Some(Some(e.clone()));
                }
            }
        });
    picked
}

fn draw_lane(
    ui: &mut egui::Ui,
    edited: &mut EditedSkill,
    edited_fx: &mut EditedSkillFx,
    cue: &str,
    lists: &PickerLists,
) {
    ui.label(egui::RichText::new(format!("moment: {cue}")).heading());
    // Human-readable description of WHEN this fires.
    let desc = if cue.ends_with("_cast") {
        "fires when the cast begins (windup start) — muzzle flash, cast anim"
    } else if cue.contains("_window_") {
        "fires when the hit volume spawns — flight visuals, beam arcs"
    } else if cue.contains("_end_") {
        "fires WHERE the volume ends (enemy / ground / fuse) — explosions, payoffs"
    } else {
        "fires on each confirmed hit — victim-anchored flash"
    };
    ui.label(egui::RichText::new(desc).small().weak());
    ui.add_space(4.0);

    let kind = crate::panel::kind_for(cue);
    let lane = crate::fx_edits::ensure_lane(&mut edited_fx.fx, cue, kind);
    let mut changed = false;

    section(ui, "Animation", |ui| {
        let current = lane.anim.as_ref().and_then(|a| a.clip.clone());
        if let Some(next) = row_picker(ui, "clip", "insp_clip", current.as_deref(), &lists.anim_clips) {
            crate::fx_edits::set_anim_clip(lane, next, 0, 1.0);
            changed = true;
        }
    });

    section(ui, "Particles", |ui| {
        let current = lane.particle.as_ref().and_then(|p| p.effect.clone());
        if let Some(next) = row_picker(ui, "effect", "insp_vfx", current.as_deref(), &lists.vfx_effects) {
            crate::fx_edits::set_particle_effect(lane, next);
            changed = true;
        }
        let socket = lane.particle.as_ref().and_then(|p| p.socket.clone());
        if let Some(next) = row_picker(ui, "socket", "insp_socket", socket.as_deref(), &lists.sockets) {
            crate::fx_edits::set_particle_socket(lane, next);
            changed = true;
        }
        if ui.small_button("+ charge → scale binding").clicked() {
            crate::fx_edits::add_param_binding(
                lane,
                arena_skills::VfxParamBinding {
                    param: "scale".into(),
                    source: arena_skills::VfxBindSource::Charge,
                    min: 0.5,
                    max: 2.0,
                },
            );
            changed = true;
        }
    });

    // Projectile / beam sections only where they make sense for the moment.
    if cue.contains("_window_") {
        section(ui, "Projectile (flies the window's motion)", |ui| {
            let has = lane.projectile.is_some();
            let mut want = has;
            ui.checkbox(&mut want, "cosmetic projectile");
            if want != has {
                lane.projectile = want.then_some(arena_skills::ProjectileCosmetic {
                    speed: 20.0,
                    gravity: 0.0,
                    color: [1.0, 0.6, 0.2],
                    radius: 0.2,
                    effect: None,
                    socket: None,
                    end_cue: None,
                });
                changed = true;
            }
            if let Some(p) = &mut lane.projectile {
                if let Some(next) = row_picker(ui, "effect", "insp_proj_vfx", p.effect.as_deref(), &lists.vfx_effects) {
                    p.effect = next;
                    changed = true;
                }
                ui.label(
                    egui::RichText::new("speed/gravity/end-cue sync from the window on Save")
                        .small()
                        .weak(),
                );
            }
            let has_beam = lane.beam.is_some();
            let mut want_beam = has_beam;
            ui.checkbox(&mut want_beam, "beam arc (two-anchor)");
            if want_beam != has_beam {
                lane.beam = want_beam.then_some(arena_skills::BeamSpec {
                    effect: None,
                    color: [0.6, 0.8, 1.0],
                    segments: 8,
                    lifetime: 0.25,
                });
                changed = true;
            }
            if let Some(b) = &mut lane.beam {
                if let Some(next) = row_picker(ui, "effect", "insp_beam_vfx", b.effect.as_deref(), &lists.vfx_effects) {
                    b.effect = next;
                    changed = true;
                }
                let mut seg = b.segments as u32;
                if row(ui, "segments", |ui| {
                    ui.add(egui::DragValue::new(&mut seg).range(2..=32)).changed()
                }) {
                    b.segments = seg as u8;
                    changed = true;
                }
            }
        });
    }

    if changed {
        edited_fx.dirty = true;
        edited.dirty = true; // vfx_cues may need re-deriving on save
    }
    let _ = derive_vfx_cues(&edited.timeline);
}

/// Labeled picker row; returns the pick if one happened.
#[allow(clippy::option_option)]
fn row_picker(
    ui: &mut egui::Ui,
    label: &str,
    salt: &str,
    current: Option<&str>,
    entries: &[String],
) -> Option<Option<String>> {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.add_sized([70.0, 18.0], egui::Label::new(label));
        out = name_picker(ui, salt, current, entries);
    });
    out
}

