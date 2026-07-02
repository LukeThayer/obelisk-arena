//! Derived, always-live readouts for the inspector (UX spec D11): what the authored numbers
//! MEAN — per-hit damage range, how many strikes the graph can land, full-chain totals, and
//! damage-per-mana. Pure functions over the edited triad, unit-tested here.

use obelisk_bevy::assets::{CastTimeline, EndReaction};
use stat_core::Skill;

/// The computed summary the inspector prints under the Cast/root section.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillReadout {
    /// Sum of base damage lines (min, max) for ONE strike, uncharged.
    pub per_hit: (f64, f64),
    /// Max strikes one cast can land on DISTINCT targets (1 per terminal window strike +
    /// retarget hops). Splash windows count once (their per-target reach is spatial).
    pub max_strikes: u32,
    /// per_hit × max_strikes — the full-chain range if every strike lands.
    pub full_chain: (f64, f64),
    /// full_chain / mana_cost, or `None` for free skills.
    pub per_mana: Option<(f64, f64)>,
    pub crit_chance: f64,
}

/// The maximum number of strikes the timeline's graph can land: each SCHEDULED window is one
/// strike opportunity, plus `max_hops` for every retarget reaction, plus 1 for every chained
/// window reachable via `Chain` (a blast is one more strike on a given victim). Conservative,
/// glanceable — not a combat simulator.
pub fn max_strikes(tl: &CastTimeline) -> u32 {
    use obelisk_bevy::assets::WindowPhase;
    let mut strikes = 0u32;
    // The hop counter is CHAIN-WIDE (it rides the hitbox through every hop), so the chain's
    // hop budget is the MAX max_hops any retarget reaction declares — never a sum.
    let mut hop_budget = 0u32;
    for w in &tl.collision_windows {
        if w.spawn_phase != WindowPhase::Chained {
            strikes += 1;
        }
        for r in [&w.on_end.hit, &w.on_end.world, &w.on_end.fuse]
            .into_iter()
            .flatten()
        {
            match r {
                // A chain spawns its target window once per parent end; counted below as one
                // strike per reachable Chained window (per-reason duplicates collapse).
                EndReaction::Chain(_) => {}
                EndReaction::Retarget { max_hops, .. } => {
                    hop_budget = hop_budget.max(*max_hops as u32);
                }
            }
        }
    }
    strikes += hop_budget;
    // Chained windows reachable via Chain add one strike each (spawned once per cast).
    let chained_via_chain: u32 = tl
        .collision_windows
        .iter()
        .filter(|w| w.spawn_phase == WindowPhase::Chained)
        .filter(|w| {
            tl.collision_windows.iter().any(|p| {
                [&p.on_end.hit, &p.on_end.world, &p.on_end.fuse]
                    .into_iter()
                    .flatten()
                    .any(|r| matches!(r, EndReaction::Chain(id) if *id == w.id))
            })
        })
        .count() as u32;
    (strikes + chained_via_chain).max(1)
}

/// Compute the full readout from the rules + timeline.
pub fn skill_readout(skill: &Skill, tl: &CastTimeline) -> SkillReadout {
    let (min, max) = skill
        .damage
        .base_damages
        .iter()
        .fold((0.0, 0.0), |(lo, hi), d| (lo + d.min, hi + d.max));
    let strikes = max_strikes(tl);
    let full = (min * strikes as f64, max * strikes as f64);
    let per_mana = (skill.mana_cost > 0.0).then(|| (full.0 / skill.mana_cost, full.1 / skill.mana_cost));
    SkillReadout {
        per_hit: (min, max),
        max_strikes: strikes,
        full_chain: full,
        per_mana,
        crit_chance: skill.damage.base_crit_chance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{default_rules_path, editor_root, load_cast_timeline, load_skill_rules};

    #[test]
    fn chain_lightning_readout_counts_the_hops() {
        let tl = load_cast_timeline(
            &editor_root().join("assets/skills/chain_lightning.cast.ron"),
        )
        .expect("parses");
        let skill = load_skill_rules(&default_rules_path("chain_lightning")).expect("rules");
        let r = skill_readout(&skill, &tl);
        // arc (scheduled) + 3 retarget hops = 4 strikes; hop is Chained but only reachable
        // via Retarget (already counted), not Chain.
        assert_eq!(r.max_strikes, 4);
        assert_eq!(r.per_hit, (12.0, 18.0));
        assert_eq!(r.full_chain, (48.0, 72.0));
        let (lo, hi) = r.per_mana.expect("costs mana");
        assert!((lo - 6.0).abs() < 1e-9 && (hi - 9.0).abs() < 1e-9);
    }

    #[test]
    fn firebolt_readout_counts_direct_plus_blast() {
        let tl =
            load_cast_timeline(&editor_root().join("assets/skills/firebolt.cast.ron")).expect("parses");
        let skill = load_skill_rules(&default_rules_path("firebolt")).expect("rules");
        let r = skill_readout(&skill, &tl);
        // bolt (scheduled) + blast (chained via Chain) = 2 strikes on a direct hit.
        assert_eq!(r.max_strikes, 2);
        assert_eq!(r.full_chain, (40.0, 40.0));
    }
}
