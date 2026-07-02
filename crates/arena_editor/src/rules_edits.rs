//! Pure row-edit helpers for the Rules form (the egui panel stays a thin shell, the M2 idiom).

use loot_core::types::SkillTag;
use loot_core::DamageType;
use stat_core::skill::{ApplicationScaling, ApplicationTarget, ApplyChance, EffectApplication};
use stat_core::{BaseDamage, Skill};

/// Add `tag` if absent, remove it if present.
pub fn toggle_tag(skill: &mut Skill, tag: SkillTag) {
    if skill.tags.contains(&tag) {
        skill.tags.retain(|t| *t != tag);
    } else {
        skill.tags.push(tag);
    }
}

/// Append a default physical base-damage row.
pub fn add_base_damage(skill: &mut Skill) {
    skill.damage.base_damages.push(BaseDamage {
        damage_type: DamageType::Physical,
        min: 1.0,
        max: 2.0,
    });
}

pub fn remove_base_damage(skill: &mut Skill, idx: usize) {
    if idx < skill.damage.base_damages.len() {
        skill.damage.base_damages.remove(idx);
    }
}

/// Append a blank effect application (target-directed, direct scaling, always applies).
pub fn add_effect_application(skill: &mut Skill) {
    skill.effect_applications.push(EffectApplication {
        effect_id: String::new(),
        target: ApplicationTarget::Target,
        scaling: ApplicationScaling::Direct,
        apply_chance: ApplyChance::Always,
    });
}

pub fn remove_effect_application(skill: &mut Skill, idx: usize) {
    if idx < skill.effect_applications.len() {
        skill.effect_applications.remove(idx);
    }
}

/// Empty text ⇒ `None` (the Option<String> fields: use_message / hint / hint_effect).
pub fn set_opt_text(slot: &mut Option<String>, text: String) {
    *slot = if text.is_empty() { None } else { Some(text) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_model::blank_attack_skill;

    #[test]
    fn toggle_tag_adds_then_removes() {
        let mut s = blank_attack_skill("x", "X");
        assert!(!s.tags.contains(&SkillTag::Fire));
        toggle_tag(&mut s, SkillTag::Fire);
        assert!(s.tags.contains(&SkillTag::Fire));
        toggle_tag(&mut s, SkillTag::Fire);
        assert!(!s.tags.contains(&SkillTag::Fire));
    }

    #[test]
    fn base_damage_rows_add_and_remove() {
        let mut s = blank_attack_skill("x", "X");
        add_base_damage(&mut s);
        add_base_damage(&mut s);
        assert_eq!(s.damage.base_damages.len(), 2);
        remove_base_damage(&mut s, 0);
        assert_eq!(s.damage.base_damages.len(), 1);
        remove_base_damage(&mut s, 5); // out of range is a no-op
        assert_eq!(s.damage.base_damages.len(), 1);
    }

    #[test]
    fn effect_application_rows_add_and_remove() {
        let mut s = blank_attack_skill("x", "X");
        add_effect_application(&mut s);
        assert_eq!(s.effect_applications.len(), 1);
        assert!(matches!(s.effect_applications[0].apply_chance, ApplyChance::Always));
        remove_effect_application(&mut s, 0);
        assert!(s.effect_applications.is_empty());
    }

    #[test]
    fn set_opt_text_maps_empty_to_none() {
        let mut slot = Some("hi".to_string());
        set_opt_text(&mut slot, String::new());
        assert!(slot.is_none());
        set_opt_text(&mut slot, "msg".into());
        assert_eq!(slot.as_deref(), Some("msg"));
    }
}
