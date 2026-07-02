//! The rules-authoring model: the `EditedRules` resource (the in-flight `stat_core::Skill` the
//! designer is editing — the obelisk RULES side of the skill triad) plus the pure seeds for new
//! skills. `Skill::default()` is a weapon-scaling attack (skill.rs:566), so the attack seed is a
//! thin rename over it; the spell seed zeroes weapon scaling and carries flat base damage.

use bevy::prelude::*;
use loot_core::types::SkillTag;
use loot_core::DamageType;
use stat_core::{BaseDamage, DamageConfig, Delivery, Skill, Targeting};
use std::path::PathBuf;

/// The obelisk rules currently open in the designer: the `Skill`, the `config/skills/<id>.toml`
/// path it saves to, and whether it has unsaved edits. Edited alongside [`crate::model::EditedSkill`]
/// (the timeline) and [`crate::model::EditedSkillFx`] (cosmetics); Save writes all three files.
#[derive(Resource)]
pub struct EditedRules {
    pub skill: Skill,
    pub path: PathBuf,
    pub dirty: bool,
}

impl EditedRules {
    /// Open `skill` for editing, saving to `path`, with no unsaved edits.
    pub fn from_skill(skill: Skill, path: PathBuf) -> Self {
        Self { skill, path, dirty: false }
    }
}

/// A fresh weapon-scaling melee attack (the `Skill::default()` shape, renamed).
pub fn blank_attack_skill(id: &str, name: &str) -> Skill {
    Skill { id: id.into(), name: name.into(), ..Skill::default() }
}

/// A fresh flat-damage projectile spell: `weapon_effectiveness = 0.0` (the default overrides it to
/// 1.0 for attacks) + one fire base-damage row + spell tag + a small mana cost.
pub fn blank_spell_skill(id: &str, name: &str) -> Skill {
    Skill {
        id: id.into(),
        name: name.into(),
        tags: vec![SkillTag::Spell],
        targeting: Targeting::SingleEnemy,
        delivery: Delivery::Projectile,
        mana_cost: 5.0,
        damage: DamageConfig {
            weapon_effectiveness: 0.0,
            base_damages: vec![BaseDamage { damage_type: DamageType::Fire, min: 10.0, max: 15.0 }],
            ..DamageConfig::default()
        },
        ..Skill::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_attack_skill_is_a_weapon_scaling_attack() {
        let s = blank_attack_skill("slam", "Slam");
        assert_eq!(s.id, "slam");
        assert!((s.damage.weapon_effectiveness - 1.0).abs() < f64::EPSILON);
        assert!(s.tags.contains(&SkillTag::Attack));
    }

    #[test]
    fn blank_spell_skill_is_flat_damage_with_no_weapon_scaling() {
        let s = blank_spell_skill("icebolt", "Icebolt");
        assert!((s.damage.weapon_effectiveness - 0.0).abs() < f64::EPSILON);
        assert_eq!(s.damage.base_damages.len(), 1);
        assert!(s.tags.contains(&SkillTag::Spell));
        assert!((s.mana_cost - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn edited_rules_from_skill_starts_clean() {
        let r = EditedRules::from_skill(blank_attack_skill("x", "X"), PathBuf::from("/tmp/x.toml"));
        assert!(!r.dirty);
        assert_eq!(r.skill.id, "x");
    }
}
