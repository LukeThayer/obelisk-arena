//! The StatType picker's data layer. `StatType` has ~80 non-parameterized variants plus
//! per-effect parameterized expansions; `StatType::all_variants_with_effects` enumerates them and
//! `to_serde_string()` yields the exact snake_case vocabulary the TOML round-trips — so the picker
//! is a searchable combo over real values, never a hand-maintained list.

use loot_core::StatType;

/// All pickable stats: (serde-string label, value). Parameterized variants expand per effect id.
pub fn stat_choices(effect_ids: &[String]) -> Vec<(String, StatType)> {
    StatType::all_variants_with_effects(effect_ids)
        .into_iter()
        .map(|s| (s.to_serde_string(), s))
        .collect()
}

/// Case-insensitive substring filter over the labels. Empty query returns everything.
pub fn filter_stats<'a>(
    choices: &'a [(String, StatType)],
    query: &str,
) -> Vec<&'a (String, StatType)> {
    let q = query.to_lowercase();
    choices
        .iter()
        .filter(|(name, _)| name.to_lowercase().contains(&q))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_cover_the_base_variants_and_round_trip_serde_strings() {
        let choices = stat_choices(&[]);
        assert!(choices.len() >= 80, "expected the full base catalog, got {}", choices.len());
        for (name, stat) in &choices {
            let parsed = StatType::from_serde_str(name).expect("label must parse back");
            assert_eq!(&parsed, stat, "serde-string label must round-trip");
        }
    }

    #[test]
    fn effect_ids_expand_parameterized_variants() {
        let with = stat_choices(&["burn".to_string()]);
        let without = stat_choices(&[]);
        assert!(with.len() > without.len());
        assert!(with.iter().any(|(_, s)| matches!(s, StatType::EffectMagnitude(id) if id == "burn")));
    }

    #[test]
    fn filter_is_case_insensitive_substring() {
        let choices = stat_choices(&[]);
        let fire = filter_stats(&choices, "FIRE");
        assert!(!fire.is_empty());
        assert!(fire.iter().all(|(n, _)| n.to_lowercase().contains("fire")));
        assert_eq!(filter_stats(&choices, "").len(), choices.len());
    }
}
