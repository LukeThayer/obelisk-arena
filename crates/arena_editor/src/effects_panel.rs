//! The Effects tab: a serde-driven egui form over `stat_core::config::EffectConfig` (buff /
//! ailment bodies). Egui shell over the tested catalogs (`stat_ui`) — compile gate + windowed use.
//! Deferred (v1): `global_conditionals` / `conditional_modifiers` — preserved on save, not shown.

use bevy_egui::egui;
use loot_core::types::EnumVariants;
use loot_core::DamageType;
use stat_core::config::effects::EffectDuration;
use stat_core::config::{ChargeConfig, EffectModConfig, StatusApplication};
use stat_core::types::{ChargeConsumption, StackingBehavior};
use stat_core::{EffectCondition, EffectTrigger};

use crate::effect_model::EditedEffect;
use crate::io::list_effect_ids_on_disk;
use crate::stat_ui::{filter_stats, stat_choices};

/// EffectTrigger prototypes for the 4-variant picker (discriminant idiom, like trigger_ui).
fn effect_trigger_prototypes() -> Vec<EffectTrigger> {
    vec![
        EffectTrigger::OnMaxStacks { consume: true },
        EffectTrigger::OnExpire,
        EffectTrigger::OnConsume,
        EffectTrigger::OnApply,
    ]
}

fn effect_trigger_label(t: &EffectTrigger) -> &'static str {
    match t {
        EffectTrigger::OnMaxStacks { .. } => "on max stacks",
        EffectTrigger::OnExpire => "on expire",
        EffectTrigger::OnConsume => "on consume",
        EffectTrigger::OnApply => "on apply",
    }
}

/// Draw the Effects form. Returns (changed, open-request): `open` is `Some(id)` when the user
/// picked a different effect (or typed a new id) — the caller swaps the `EditedEffect` resource.
pub fn draw_effects_tab(
    ui: &mut egui::Ui,
    edited: &mut EditedEffect,
    known_skill_ids: &[String],
    stat_query: &mut String,
) -> (bool, Option<String>) {
    let mut changed = false;
    let mut open: Option<String> = None;
    let e = &mut edited.config;

    // === Selector row ===
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&e.id).strong());
        egui::ComboBox::from_id_salt("fx_effect_picker")
            .selected_text("open…")
            .show_ui(ui, |ui| {
                for id in list_effect_ids_on_disk() {
                    if ui.selectable_label(e.id == id, &id).clicked() {
                        open = Some(id);
                    }
                }
            });
        if ui.button("+ New").clicked() {
            open = Some(String::new()); // caller treats empty id as "new blank"
        }
    });

    egui::ScrollArea::vertical().id_salt("fx_scroll").show(ui, |ui| {
        // === Identity / duration / stacking ===
        ui.horizontal(|ui| {
            ui.label("name");
            changed |= ui.text_edit_singleline(&mut e.name).changed();
            changed |= ui.checkbox(&mut e.is_debuff, "debuff").changed();
            let mut infinite = e.duration.is_infinite();
            if ui.checkbox(&mut infinite, "infinite").changed() {
                e.duration = if infinite {
                    EffectDuration::Infinite
                } else {
                    EffectDuration::Finite(5.0)
                };
                changed = true;
            }
            if let EffectDuration::Finite(secs) = &mut e.duration {
                changed |= ui
                    .add(egui::DragValue::new(secs).speed(0.1).range(0.0..=600.0).suffix(" s"))
                    .changed();
            }
            egui::ComboBox::from_id_salt("fx_stacking")
                .selected_text(e.stacking.variant_name())
                .show_ui(ui, |ui| {
                    for v in StackingBehavior::all_variants() {
                        if ui.selectable_value(&mut e.stacking, v.clone(), v.variant_name()).clicked()
                        {
                            changed = true;
                        }
                    }
                });
            ui.label("max stacks");
            changed |= ui.add(egui::DragValue::new(&mut e.max_stacks).range(1..=99)).changed();
        });

        // === Ailment fields ===
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("fx_dot_type")
                .selected_text(match e.damage_type {
                    Some(dt) => format!("DoT: {dt:?}"),
                    None => "DoT: none".to_string(),
                })
                .show_ui(ui, |ui| {
                    if ui.selectable_label(e.damage_type.is_none(), "none").clicked() {
                        e.damage_type = None;
                        changed = true;
                    }
                    for v in DamageType::all_variants() {
                        if ui.selectable_label(e.damage_type == Some(*v), format!("{v:?}")).clicked()
                        {
                            e.damage_type = Some(*v);
                            changed = true;
                        }
                    }
                });
            ui.label("dmg %");
            changed |= ui
                .add(egui::DragValue::new(&mut e.base_damage_percent).speed(0.01).range(0.0..=10.0))
                .changed();
            ui.label("tick");
            changed |= ui
                .add(egui::DragValue::new(&mut e.tick_rate).speed(0.05).range(0.0..=10.0).suffix(" s"))
                .changed();
            // application: Chance | Buildup{threshold} (discriminant picker — payload variant)
            let is_buildup = matches!(e.application, StatusApplication::Buildup { .. });
            let mut buildup = is_buildup;
            if ui.checkbox(&mut buildup, "buildup").changed() {
                e.application = if buildup {
                    StatusApplication::Buildup { threshold: 100.0 }
                } else {
                    StatusApplication::Chance
                };
                changed = true;
            }
            if let StatusApplication::Buildup { threshold } = &mut e.application {
                changed |= ui.add(egui::DragValue::new(threshold).range(1.0..=10_000.0)).changed();
            }
        });

        // === Charges ===
        ui.horizontal(|ui| {
            let mut has = e.charges.is_some();
            if ui.checkbox(&mut has, "charges").changed() {
                e.charges = if has {
                    Some(ChargeConfig { count: 3, consumption: ChargeConsumption::AllSkills })
                } else {
                    None
                };
                changed = true;
            }
            if let Some(c) = &mut e.charges {
                changed |= ui.add(egui::DragValue::new(&mut c.count).range(1..=99)).changed();
                egui::ComboBox::from_id_salt("fx_consumption")
                    .selected_text(c.consumption.variant_name())
                    .show_ui(ui, |ui| {
                        for v in ChargeConsumption::all_variants() {
                            let selected = c.consumption.variant_name() == v.variant_name();
                            if ui.selectable_label(selected, v.variant_name()).clicked() {
                                c.consumption = v.clone();
                                changed = true;
                            }
                        }
                    });
            }
        });

        ui.separator();

        // === Stat modifiers (the StatType picker) ===
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Modifiers").strong());
            if ui.button("+ add").clicked() {
                e.modifiers.push(EffectModConfig::default());
                changed = true;
            }
        });
        let effect_ids: Vec<String> =
            list_effect_ids_on_disk().into_iter().collect();
        let choices = stat_choices(&effect_ids);
        let mut remove_m: Option<usize> = None;
        for (i, m) in e.modifiers.iter_mut().enumerate() {
            ui.push_id(("fxmod", i), |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("fx_stat")
                        .selected_text(m.stat.to_serde_string())
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(stat_query).hint_text("search stats…"),
                            );
                            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                for (name, s) in filter_stats(&choices, stat_query) {
                                    if ui.selectable_label(&m.stat == s, name).clicked() {
                                        m.stat = s.clone();
                                        changed = true;
                                    }
                                }
                            });
                        });
                    ui.label("value");
                    changed |= ui.add(egui::DragValue::new(&mut m.value).speed(0.5)).changed();
                    changed |= ui
                        .checkbox(&mut m.is_more, "more")
                        .on_hover_text("checked: MORE multiplier; unchecked: increased")
                        .changed();
                    if ui.button("✕").clicked() {
                        remove_m = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove_m {
            e.modifiers.remove(i);
            changed = true;
        }

        ui.separator();

        // === Effect triggers (the Static-at-3-stacks → discharge cascade) ===
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Triggers").strong());
            if ui.button("+ add").clicked() {
                e.conditions.push(EffectCondition::default());
                changed = true;
            }
        });
        let mut remove_c: Option<usize> = None;
        for (i, ec) in e.conditions.iter_mut().enumerate() {
            ui.push_id(("fxcond", i), |ui| {
                ui.horizontal(|ui| {
                    let protos = effect_trigger_prototypes();
                    let mut idx = protos
                        .iter()
                        .position(|p| std::mem::discriminant(p) == std::mem::discriminant(&ec.trigger))
                        .unwrap_or(0);
                    egui::ComboBox::from_id_salt("fx_trig")
                        .selected_text(effect_trigger_label(&ec.trigger))
                        .show_ui(ui, |ui| {
                            for (j, p) in protos.iter().enumerate() {
                                if ui.selectable_value(&mut idx, j, effect_trigger_label(p)).clicked()
                                {
                                    ec.trigger = protos[j].clone();
                                    changed = true;
                                }
                            }
                        });
                    if let EffectTrigger::OnMaxStacks { consume } = &mut ec.trigger {
                        changed |= ui.checkbox(consume, "consume").changed();
                    }
                    ui.label("→ cast");
                    egui::ComboBox::from_id_salt("fx_trig_skill")
                        .selected_text(if ec.trigger_skill.is_empty() {
                            "(pick skill)".to_string()
                        } else {
                            ec.trigger_skill.clone()
                        })
                        .show_ui(ui, |ui| {
                            for id in known_skill_ids {
                                if ui
                                    .selectable_value(&mut ec.trigger_skill, id.clone(), id)
                                    .clicked()
                                {
                                    changed = true;
                                }
                            }
                        });
                    if ui.button("✕").clicked() {
                        remove_c = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove_c {
            e.conditions.remove(i);
            changed = true;
        }
    });
    (changed, open)
}
