//! The Rules tab: a hand-built serde-driven egui form over `stat_core::Skill` (stat_core has no
//! Reflect — by design; see the M4 research doc §1). An egui shell over `rules_edits`; compile
//! gate + windowed use, like `panel.rs`.

use bevy_egui::egui;
use loot_core::types::{EnumVariants, SkillTag};
use loot_core::DamageType;
use stat_core::skill::{ApplicationScaling, ApplicationTarget, ApplyChance};
use stat_core::{Delivery, Skill, Targeting};
use std::collections::HashMap;

use crate::rules_edits::{
    add_base_damage, add_effect_application, remove_base_damage, remove_effect_application,
    set_opt_text, toggle_tag,
};

/// Draw the Rules form. `effect_ids` populates the effect-application picker (from the live
/// obelisk effect registry). Returns true if any field changed.
pub fn draw_rules_tab(ui: &mut egui::Ui, skill: &mut Skill, effect_ids: &[String]) -> bool {
    let mut changed = false;
    egui::ScrollArea::vertical().show(ui, |ui| {
        // === Identity ===
        ui.horizontal(|ui| {
            ui.label("name");
            changed |= ui.text_edit_singleline(&mut skill.name).changed();
            ui.label("description");
            changed |= ui.text_edit_singleline(&mut skill.description).changed();
        });

        // === Tags ===
        ui.horizontal_wrapped(|ui| {
            ui.label("tags");
            for tag in SkillTag::all_variants() {
                let mut has = skill.tags.contains(tag);
                if ui.checkbox(&mut has, tag.variant_name()).changed() {
                    toggle_tag(skill, *tag);
                    changed = true;
                }
            }
        });

        // === Targeting / delivery / cost ===
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("rules_targeting")
                .selected_text(skill.targeting.variant_name())
                .show_ui(ui, |ui| {
                    for v in Targeting::all_variants() {
                        if ui.selectable_value(&mut skill.targeting, *v, v.variant_name()).clicked() {
                            changed = true;
                        }
                    }
                });
            egui::ComboBox::from_id_salt("rules_delivery")
                .selected_text(skill.delivery.variant_name())
                .show_ui(ui, |ui| {
                    for v in Delivery::all_variants() {
                        if ui.selectable_value(&mut skill.delivery, *v, v.variant_name()).clicked() {
                            changed = true;
                        }
                    }
                });
            for (lab, val) in [
                ("mana", &mut skill.mana_cost),
                ("cooldown", &mut skill.cooldown),
                ("speed×", &mut skill.attack_speed_modifier),
            ] {
                ui.label(lab);
                changed |= ui
                    .add(egui::DragValue::new(val).speed(0.1).range(0.0..=1000.0))
                    .changed();
            }
            ui.label("elude");
            changed |= ui
                .add(egui::DragValue::new(&mut skill.grants_elude_stacks).range(0..=20))
                .changed();
        });

        // === UI-hint strings (empty ⇒ None) ===
        ui.horizontal(|ui| {
            for (lab, slot) in [
                ("use_message", &mut skill.use_message),
                ("hint", &mut skill.hint),
                ("hint_effect", &mut skill.hint_effect),
            ] {
                ui.label(lab);
                let mut text = slot.clone().unwrap_or_default();
                if ui.add(egui::TextEdit::singleline(&mut text).desired_width(110.0)).changed() {
                    set_opt_text(slot, text);
                    changed = true;
                }
            }
        });

        ui.separator();

        // === Damage ===
        ui.label(egui::RichText::new("Damage").strong());
        ui.horizontal(|ui| {
            for (lab, val) in [
                ("weapon eff", &mut skill.damage.weapon_effectiveness),
                ("damage eff", &mut skill.damage.damage_effectiveness),
                ("crit %", &mut skill.damage.base_crit_chance),
                ("crit multi+", &mut skill.damage.crit_multiplier_bonus),
            ] {
                ui.label(lab);
                changed |= ui.add(egui::DragValue::new(val).speed(0.05)).changed();
            }
            changed |= ui.checkbox(&mut skill.damage.guaranteed_crit, "guaranteed crit").changed();
            ui.label("hits");
            changed |= ui
                .add(egui::DragValue::new(&mut skill.damage.hits_per_attack).range(1..=20))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("base damages");
            if ui.button("+ add").clicked() {
                add_base_damage(skill);
                changed = true;
            }
        });
        let mut remove_bd: Option<usize> = None;
        for (i, bd) in skill.damage.base_damages.iter_mut().enumerate() {
            ui.push_id(("bd", i), |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("bd_type")
                        .selected_text(format!("{:?}", bd.damage_type))
                        .show_ui(ui, |ui| {
                            for v in DamageType::all_variants() {
                                if ui
                                    .selectable_value(&mut bd.damage_type, *v, format!("{v:?}"))
                                    .clicked()
                                {
                                    changed = true;
                                }
                            }
                        });
                    ui.label("min");
                    changed |= ui.add(egui::DragValue::new(&mut bd.min).speed(0.5)).changed();
                    ui.label("max");
                    changed |= ui.add(egui::DragValue::new(&mut bd.max).speed(0.5)).changed();
                    if ui.button("✕").clicked() {
                        remove_bd = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove_bd {
            remove_base_damage(skill, i);
            changed = true;
        }

        ui.separator();

        // === Effect applications ===
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Applies Effects").strong());
            if ui.button("+ add").clicked() {
                add_effect_application(skill);
                changed = true;
            }
        });
        let mut remove_ea: Option<usize> = None;
        for (i, ea) in skill.effect_applications.iter_mut().enumerate() {
            ui.push_id(("ea", i), |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("ea_effect")
                        .selected_text(if ea.effect_id.is_empty() {
                            "(pick effect)".to_string()
                        } else {
                            ea.effect_id.clone()
                        })
                        .show_ui(ui, |ui| {
                            for id in effect_ids {
                                if ui.selectable_value(&mut ea.effect_id, id.clone(), id).clicked() {
                                    changed = true;
                                }
                            }
                        });
                    egui::ComboBox::from_id_salt("ea_target")
                        .selected_text(ea.target.variant_name())
                        .show_ui(ui, |ui| {
                            for v in ApplicationTarget::all_variants() {
                                if ui.selectable_value(&mut ea.target, *v, v.variant_name()).clicked()
                                {
                                    changed = true;
                                }
                            }
                        });
                    let mut driven = matches!(ea.scaling, ApplicationScaling::DamageDriven { .. });
                    if ui.checkbox(&mut driven, "damage-driven").changed() {
                        ea.scaling = if driven {
                            ApplicationScaling::DamageDriven {
                                conversions: HashMap::from([(DamageType::Fire, 0.5)]),
                            }
                        } else {
                            ApplicationScaling::Direct
                        };
                        changed = true;
                    }
                    let mut scaled = matches!(ea.apply_chance, ApplyChance::DamageScaled { .. });
                    if ui.checkbox(&mut scaled, "damage-scaled chance").changed() {
                        ea.apply_chance = if scaled {
                            ApplyChance::DamageScaled { bonus: 0.0 }
                        } else {
                            ApplyChance::Always
                        };
                        changed = true;
                    }
                    if let ApplyChance::DamageScaled { bonus } = &mut ea.apply_chance {
                        ui.label("bonus");
                        changed |= ui.add(egui::DragValue::new(bonus).speed(0.05)).changed();
                    }
                    if ui.button("✕").clicked() {
                        remove_ea = Some(i);
                    }
                });
                if let ApplicationScaling::DamageDriven { conversions } = &mut ea.scaling {
                    ui.horizontal(|ui| {
                        ui.label("   conversions");
                        for dt in DamageType::all_variants() {
                            let mut v = conversions.get(dt).copied().unwrap_or(0.0);
                            ui.label(format!("{dt:?}"));
                            if ui
                                .add(egui::DragValue::new(&mut v).speed(0.05).range(0.0..=1.0))
                                .changed()
                            {
                                if v > 0.0 {
                                    conversions.insert(*dt, v);
                                } else {
                                    conversions.remove(dt);
                                }
                                changed = true;
                            }
                        }
                    });
                }
            });
        }
        if let Some(i) = remove_ea {
            remove_effect_application(skill, i);
            changed = true;
        }
    });
    changed
}
