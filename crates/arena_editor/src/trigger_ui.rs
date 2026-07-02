//! The TriggerCondition catalog: one default-payload prototype per variant, grouped by pipeline
//! phase, so the Rules panel can drive a single ComboBox over all 34 variants (`TriggerCondition`
//! has no EnumVariants impl — it carries payloads). Labels come from the enum's own `Display`.
//! Also referential-integrity validation for `SkillCondition.trigger_skill` — the obelisk loader
//! ERRORS on unknown refs (config/skills.rs:77), so Save must block them.

use loot_core::DamageType;
use stat_core::{Skill, TriggerCondition};
use std::collections::HashSet;

/// Every `TriggerCondition` variant as (pipeline-group label, default-payload prototype), in
/// pipeline order. The single source of truth for the condition picker.
pub fn trigger_prototypes() -> Vec<(&'static str, TriggerCondition)> {
    use TriggerCondition::*;
    vec![
        ("Unconditional", Always),
        ("Pre-calculation", PlayerFullLife),
        ("Pre-calculation", PlayerLowLife { threshold: 0.35 }),
        ("Pre-calculation", TargetFullLife),
        ("Pre-calculation", TargetLowLife { threshold: 0.35 }),
        ("Pre-calculation", TargetHasEffect { id: String::new() }),
        ("Pre-calculation", TargetEffectStacks { id: String::new(), min_stacks: 1 }),
        ("Pre-calculation", SelfHasEffect { id: String::new() }),
        ("Pre-calculation", EveryNthHit { n: 3 }),
        ("Pre-calculation", PlayerLowMana { threshold: 0.35 }),
        ("Pre-calculation", PlayerFullMana),
        ("Pre-calculation", PlayerHasBarrier),
        ("Pre-calculation", PlayerNoBarrier),
        ("Pre-calculation", TargetHasBarrier),
        ("Pre-calculation", TargetNoBarrier),
        ("Pre-calculation", SelfEffectStacks { id: String::new(), min_stacks: 1 }),
        ("Pre-calculation", TargetNoEffect { id: String::new() }),
        ("Post-calculation", OnCrit),
        ("Post-calculation", DamageTypeDealt { damage_type: DamageType::Fire }),
        ("Post-calculation", OnNonCrit),
        ("Post-calculation", DamageOverThreshold { threshold: 0.0 }),
        ("Post-calculation", MultipleDamageTypes),
        ("Post-resolution", OnKill),
        ("Post-resolution", OnBarrierBroken),
        ("Post-resolution", OnOverkill { threshold: 0.0 }),
        ("Defensive", OnDamageTaken),
        ("Defensive", OnDamageTakenOfType { damage_type: DamageType::Fire }),
        ("Defensive", OnEffectConsumed { id: String::new() }),
        ("Defensive", OnEffectChargeUsed { id: String::new() }),
        ("Defensive", OnDodge),
        ("Defensive", OnEvasionCap),
        ("Defensive", OnHitTaken),
        ("Defensive", OnBarrierDepleted),
        ("Defensive", OnLowLifeReached { threshold: 0.35 }),
    ]
}

/// Index of `c`'s VARIANT in `trigger_prototypes()` (payload-insensitive, via discriminant).
pub fn trigger_index(c: &TriggerCondition) -> usize {
    trigger_prototypes()
        .iter()
        .position(|(_, p)| std::mem::discriminant(p) == std::mem::discriminant(c))
        .unwrap_or(0)
}

/// The `conditions[].trigger_skill` ids that are NOT in `known_ids` (and not the skill itself —
/// self-reference is valid: the loader validates against the post-insert map). Non-empty ⇒ the
/// obelisk loader would reject the whole skills dir; Save must refuse to write.
pub fn invalid_trigger_refs(skill: &Skill, known_ids: &HashSet<String>) -> Vec<String> {
    skill
        .conditions
        .iter()
        .map(|c| c.trigger_skill.clone())
        .filter(|id| id != &skill.id && !known_ids.contains(id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_model::blank_attack_skill;
    use stat_core::SkillCondition;

    #[test]
    fn catalog_covers_all_34_variants_uniquely() {
        let protos = trigger_prototypes();
        assert_eq!(protos.len(), 34);
        for (i, (_, p)) in protos.iter().enumerate() {
            assert_eq!(trigger_index(p), i, "prototype {i} must index back to itself");
        }
    }

    #[test]
    fn trigger_index_is_payload_insensitive() {
        let c = TriggerCondition::PlayerLowLife { threshold: 0.1 };
        let proto = TriggerCondition::PlayerLowLife { threshold: 0.35 };
        assert_eq!(trigger_index(&c), trigger_index(&proto));
        assert_eq!(trigger_index(&TriggerCondition::Always), 0);
    }

    #[test]
    fn invalid_trigger_refs_flags_unknown_and_allows_known_and_self() {
        let mut s = blank_attack_skill("zap", "Zap");
        s.conditions.push(SkillCondition {
            trigger_skill: "discharge".into(),
            additional: true,
            condition: TriggerCondition::OnCrit,
        });
        s.conditions.push(SkillCondition {
            trigger_skill: "zap".into(), // self-reference: valid
            additional: false,
            condition: TriggerCondition::OnKill,
        });
        s.conditions.push(SkillCondition {
            trigger_skill: "ghost".into(),
            additional: true,
            condition: TriggerCondition::OnCrit,
        });
        let known: HashSet<String> = ["discharge".to_string()].into();
        assert_eq!(invalid_trigger_refs(&s, &known), vec!["ghost".to_string()]);
    }
}
