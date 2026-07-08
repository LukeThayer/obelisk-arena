//! The arena WEAPON catalog — obelisk items (`loot_core`) as equippable, skill-granting weapons.
//!
//! Weapons are defined PURELY as obelisk item TOML (`config/items/base_types/*.toml`,
//! `BaseTypeConfig`): stats (implicit affix / weapon damage / defenses) plus `granted_skills`.
//! No visuals. Every peer scans the same files at startup (the level-catalog pattern — only ids
//! travel on the wire), and every `[[base_types]]` entry appears in the lobby's I-key panel
//! automatically, so adding a weapon is purely a content change.
//!
//! Equip semantics live server-side (`server/equip.rs`): obelisk's own `StatBlock::equip`
//! applies the stats; the skill half is the one bridge obelisk leaves to the consumer — the
//! equipping player's `SkillSlots` is REWRITTEN to the weapon's `granted_skills`, and the cast
//! pipeline resolves cast slots against the replicated [`crate::net::protocol::EquippedWeapon`]
//! list (which is also what makes un-equipped skills uncastable).

use std::path::Path;

use bevy::prelude::*;

/// One equippable weapon: the realized obelisk item + the presentation fields every UI needs.
#[derive(Debug, Clone)]
pub struct WeaponDef {
    /// The `BaseTypeConfig.id` — the wire identity (`EquipWeaponMessage.item_id`).
    pub id: String,
    /// Display name (`BaseTypeConfig.name`).
    pub name: String,
    /// The skills this weapon grants, in authored order — slot i of the input stream's
    /// `skill_slot` means `skills[i]` OF THE EQUIPPED WEAPON on both peers.
    pub skills: Vec<String>,
    /// The realized item (fixed seed ⇒ deterministic roll on every peer) handed to
    /// `StatBlock::equip`.
    pub item: loot_core::Item,
}

/// The scanned weapon set, ordered alphabetically by id (deterministic across peers — display
/// order only; the wire carries ids, never indices into this list).
#[derive(Resource, Debug, Default)]
pub struct WeaponCatalog {
    pub weapons: Vec<WeaponDef>,
}

/// The fixed generation seed: weapons are CONTENT, not drops — every peer must realize the
/// identical item (implicit rolls draw from a seeded rng).
const WEAPON_SEED: u64 = 0;

impl WeaponCatalog {
    /// Load every base type under `dir` (a `loot_core::Config` tree — in the arena,
    /// `config/items/`) into realized items. A missing dir is an EMPTY catalog (warn, not
    /// panic): the game degrades to no-weapon spawns rather than refusing to boot.
    pub fn load(dir: &Path) -> Self {
        let config = match loot_core::Config::load_from_dir(dir) {
            Ok(c) => c,
            Err(e) => {
                warn!("weapon catalog failed to load from {dir:?}: {e:?}");
                return Self::default();
            }
        };
        let generator = loot_core::Generator::new(config.clone());
        let mut ids: Vec<&String> = config.base_types.keys().collect();
        ids.sort();
        let mut weapons = Vec::with_capacity(ids.len());
        for id in ids {
            let base = &config.base_types[id];
            match generator.generate(id, WEAPON_SEED) {
                Ok(item) => weapons.push(WeaponDef {
                    id: id.clone(),
                    name: base.name.clone(),
                    skills: item.all_skills().iter().map(|s| s.to_string()).collect(),
                    item,
                }),
                Err(e) => warn!("weapon '{id}' failed to generate: {e:?}"),
            }
        }
        info!(
            "weapon catalog: {:?}",
            weapons.iter().map(|w| &w.id).collect::<Vec<_>>()
        );
        Self { weapons }
    }

    pub fn get(&self, id: &str) -> Option<&WeaponDef> {
        self.weapons.iter().find(|w| w.id == id)
    }

    /// The spawn-default weapon: the one tagged `"starter"`, else the alphabetically first.
    pub fn default_weapon(&self) -> Option<&WeaponDef> {
        self.weapons
            .iter()
            .find(|w| w.item.tags.iter().any(|t| t == "starter"))
            .or_else(|| self.weapons.first())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arena_items_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/items")
    }

    /// The shipped catalog: all four weapons load with their authored skills, in authored order
    /// (slot 0 of storm_staff is blizzard, slot 1 chain_lightning — the wheel + cast slots
    /// depend on this order), and ember_wand is the starter.
    #[test]
    fn shipped_weapons_load_with_authored_skills() {
        let catalog = WeaponCatalog::load(&arena_items_dir());
        assert_eq!(
            catalog.weapons.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            vec!["ember_wand", "needle_and_thread", "potted_spring", "storm_staff"],
        );
        assert_eq!(catalog.get("ember_wand").unwrap().skills, vec!["firebolt"]);
        assert_eq!(
            catalog.get("storm_staff").unwrap().skills,
            vec!["blizzard", "chain_lightning"],
        );
        assert_eq!(
            catalog.get("potted_spring").unwrap().skills,
            vec!["rolling_glacier", "frost_spire"],
        );
        assert_eq!(
            catalog.get("needle_and_thread").unwrap().skills,
            vec!["portal_orange", "portal_blue"],
        );
        assert_eq!(catalog.default_weapon().unwrap().id, "ember_wand");
        assert_eq!(catalog.get("ember_wand").unwrap().name, "Ember Wand");
    }

    /// Item realization is deterministic (fixed seed): two loads produce identical implicit
    /// rolls — every peer equips the SAME item.
    #[test]
    fn weapon_items_are_deterministic() {
        let a = WeaponCatalog::load(&arena_items_dir());
        let b = WeaponCatalog::load(&arena_items_dir());
        let (wa, wb) = (a.get("ember_wand").unwrap(), b.get("ember_wand").unwrap());
        assert_eq!(
            wa.item.implicit.as_ref().map(|m| m.value),
            wb.item.implicit.as_ref().map(|m| m.value),
        );
    }

    /// A missing directory degrades to an empty catalog, never a panic.
    #[test]
    fn missing_dir_is_empty_catalog() {
        let catalog = WeaponCatalog::load(Path::new("/nonexistent-items-xyz"));
        assert!(catalog.weapons.is_empty());
        assert!(catalog.default_weapon().is_none());
    }
}
