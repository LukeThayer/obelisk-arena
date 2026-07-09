//! Character part-variant tables + the visibility decision — the SHARED core of the game's
//! customization system (`arena_game::client::parts` re-exports this module and layers the
//! per-rig ECS systems on top). Lives in arena_sim so the EDITOR can deconflict the preview
//! rig with the exact same rules: `character.glb` ships EVERY variant mesh stacked together,
//! and anything rendering the raw scene must hide non-selected variants + the categorically
//! hidden meshes (body skin, weapons, capes) or the model renders as a heap of hats.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};


/// Per-slot index into the variant tables below. Wire-friendly:
/// fixed-size, all `u8`s, so it serdes cleanly inside
/// `PlayerCustomization` without growing the message meaningfully.
#[derive(Resource, Component, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartSelection {
    pub top: u8,
    pub bottom: u8,
    pub headwear: u8,
    pub hair: u8,
    pub eyes: u8,
    pub eyebrows: u8,
    pub mouth: u8,
}

impl Default for PartSelection {
    fn default() -> Self {
        Self {
            top: 0,      // Witch Top
            bottom: 0,   // Witch Bottom
            headwear: 1, // Witch Hat
            hair: 1,     // Hair 1
            eyes: 0,
            eyebrows: 0,
            mouth: 0,
        }
    }
}

/// One variant in a slot table: a display label plus the set of mesh
/// node-names that compose it. Empty mesh list = "None".
type MeshList = &'static [&'static str];

pub const TOP_VARIANTS: &[(&str, MeshList)] = &[
    ("Witch", &["F_Witch_Top"]),
    ("Archer", &["F_Archer_Top"]),
    ("Fighter", &["F_Fighter_Top"]),
    ("Hunter", &["F_Hunter_Top"]),
    // Knight pauldrons are authored as a separate node but visually
    // belong to the top — bundle them so we can't pick the Knight top
    // without the matching shoulders.
    ("Knight", &["F_Knight_Top", "F_Knight_Pauldrons"]),
    ("Mage", &["F_Mage_Top"]),
    ("Rogue", &["F_Rogue_Top"]),
    ("Sorcerer", &["F_Sorcerer_Top"]),
    ("Swordsman", &["F_Swordsman_Top"]),
];

pub const BOTTOM_VARIANTS: &[(&str, MeshList)] = &[
    ("Witch", &["F_Witch_Bottom"]),
    ("Archer", &["F_Archer_Bottom"]),
    ("Fighter", &["F_Fighter_Bottom"]),
    ("Hunter", &["F_Hunter_Bottom"]),
    ("Knight", &["F_Knight_Bottom", "F_Knight_Skirt"]),
    ("Mage", &["F_Mage_Bottom"]),
    ("Rogue", &["F_Rogue_Bottom"]),
    // Sorcerer's shoes were authored as a separate node but stay
    // with the bottom slot in every pre-baked combination.
    ("Sorcerer", &["F_Sorcerer_Bottom", "F_Sorcerer_Shoes"]),
    ("Swordsman", &["F_Swordsman_Bottom"]),
];

pub const HEADWEAR_VARIANTS: &[(&str, MeshList)] = &[
    ("None", &[]),
    ("Witch Hat", &["F_Witch_Headwear"]),
    ("Mage Hood", &["F_Mage_Headwear"]),
    ("Archer Bycocket", &["F_Archer_BycocketHat"]),
    ("Hunter Beret", &["F_Hunter_Beret"]),
    (
        "Knight Helmet",
        &["F_Knight_ArmetHelmet", "F_Knight_ArmetHelmet_Visor"],
    ),
    ("Swordsman Kettle", &["F_Swordsman_KettleHat"]),
    ("Sorcerer Circlet", &["F_Sorcerer_Circlet"]),
    ("Fighter Coif", &["F_Fighter_LeatherCoif"]),
    (
        "Rogue Mask",
        &["F_Rogue_FaceMask", "F_Rogue_FaceMask_NeckWrap"],
    ),
];

/// Every cape / shawl / scarf node-name that ships in `character.glb`.
/// Hidden categorically — no customizer slot exposes them. Lives
/// here so `is_visible` can hide them without having to know about
/// every class. (If the cape physics attempt is revisited or capes
/// become a customization slot again, remove these and add a
/// `CAPE_VARIANTS` table.)
const ALL_CAPES: &[&str] = &[
    "F_Witch_Cape",
    "F_Witch_Choker",
    "F_Archer_Cape",
    "F_Mage_Cape",
    "F_Mage_Scarf",
    "F_Sorcerer_Shawl",
];

/// Every weapon node-name that ships in `character.glb`. Listed here
/// so `is_visible` can hide them all categorically — no customizer
/// slot exposes a weapon choice. If gameplay (spells / inventory)
/// later wants to *show* a specific weapon, it'll override visibility
/// on the matching mesh entity directly.
const ALL_WEAPONS: &[&str] = &[
    "F_Witch_Staff",
    "F_Mage_Staff",
    "F_Sorcerer_Staff",
    "F_Archer_Bow",
    "F_Archer_Arrow",
    "F_Archer_ArrowQuiver",
    "F_Hunter_Bow",
    "F_Hunter_Arrow",
    "F_Hunter_ArrowQuiver",
    "F_Fighter_Sword",
    "F_Fighter_SwordScabbard",
    "F_Knight_Sword",
    "F_Knight_SwordScabbard",
    "F_Knight_Dagger",
    "F_Knight_DaggerScabbard",
    "F_Swordsman_Sword",
    "F_Swordsman_SwordScabbard",
    "F_Rogue_Dagger",
    "F_Rogue_DaggerScabbard",
];

pub const HAIR_VARIANTS: &[(&str, Option<&str>)] = &[
    ("None", None),
    ("Hair 1", Some("F_hair_1")),
    ("Hair 1b", Some("F_hair_1b")),
    ("Hair 14b Bun", Some("F_hair_14b_Bun")),
];

pub const EYES_VARIANTS: &[(&str, &str)] = &[("Eyes 0", "F_eyes0")];
pub const EYEBROWS_VARIANTS: &[(&str, &str)] = &[("Eyebrows 0", "F_eyebrows0")];
pub const MOUTH_VARIANTS: &[(&str, &str)] = &[("Mouth 0", "F_mouth0")];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    Top,
    Bottom,
    Headwear,
    Hair,
    Eyes,
    Eyebrows,
    Mouth,
}

/// Iteration order used by the customizer UI.
pub const SLOTS: &[Slot] = &[
    Slot::Top,
    Slot::Bottom,
    Slot::Headwear,
    Slot::Hair,
    Slot::Eyes,
    Slot::Eyebrows,
    Slot::Mouth,
];

impl Slot {
    pub fn display(self) -> &'static str {
        match self {
            Slot::Top => "Top",
            Slot::Bottom => "Bottom",
            Slot::Headwear => "Headwear",
            Slot::Hair => "Hair",
            Slot::Eyes => "Eyes",
            Slot::Eyebrows => "Eyebrows",
            Slot::Mouth => "Mouth",
        }
    }

    fn outfit_table(self) -> Option<&'static [(&'static str, MeshList)]> {
        match self {
            Slot::Top => Some(TOP_VARIANTS),
            Slot::Bottom => Some(BOTTOM_VARIANTS),
            Slot::Headwear => Some(HEADWEAR_VARIANTS),
            _ => None,
        }
    }
}

impl PartSelection {
    pub fn index_for(&self, slot: Slot) -> u8 {
        match slot {
            Slot::Top => self.top,
            Slot::Bottom => self.bottom,
            Slot::Headwear => self.headwear,
            Slot::Hair => self.hair,
            Slot::Eyes => self.eyes,
            Slot::Eyebrows => self.eyebrows,
            Slot::Mouth => self.mouth,
        }
    }

    fn index_mut(&mut self, slot: Slot) -> &mut u8 {
        match slot {
            Slot::Top => &mut self.top,
            Slot::Bottom => &mut self.bottom,
            Slot::Headwear => &mut self.headwear,
            Slot::Hair => &mut self.hair,
            Slot::Eyes => &mut self.eyes,
            Slot::Eyebrows => &mut self.eyebrows,
            Slot::Mouth => &mut self.mouth,
        }
    }

    pub fn variant_count(slot: Slot) -> usize {
        match slot {
            Slot::Top => TOP_VARIANTS.len(),
            Slot::Bottom => BOTTOM_VARIANTS.len(),
            Slot::Headwear => HEADWEAR_VARIANTS.len(),
            Slot::Hair => HAIR_VARIANTS.len(),
            Slot::Eyes => EYES_VARIANTS.len(),
            Slot::Eyebrows => EYEBROWS_VARIANTS.len(),
            Slot::Mouth => MOUTH_VARIANTS.len(),
        }
    }

    /// Wrapping advance/back through the slot's variant table.
    pub fn advance(&mut self, slot: Slot, dir: i32) {
        let len = Self::variant_count(slot);
        if len == 0 {
            return;
        }
        let current = self.index_mut(slot);
        let next = (*current as i32 + dir).rem_euclid(len as i32);
        *current = next as u8;
    }

    pub fn label(&self, slot: Slot) -> &'static str {
        let idx = self.index_for(slot) as usize;
        match slot {
            Slot::Top => TOP_VARIANTS.get(idx).map(|(l, _)| *l).unwrap_or("?"),
            Slot::Bottom => BOTTOM_VARIANTS.get(idx).map(|(l, _)| *l).unwrap_or("?"),
            Slot::Headwear => HEADWEAR_VARIANTS.get(idx).map(|(l, _)| *l).unwrap_or("?"),
            Slot::Hair => HAIR_VARIANTS.get(idx).map(|(l, _)| *l).unwrap_or("?"),
            Slot::Eyes => EYES_VARIANTS.get(idx).map(|(l, _)| *l).unwrap_or("?"),
            Slot::Eyebrows => EYEBROWS_VARIANTS.get(idx).map(|(l, _)| *l).unwrap_or("?"),
            Slot::Mouth => MOUTH_VARIANTS.get(idx).map(|(l, _)| *l).unwrap_or("?"),
        }
    }

    /// Returns whether a given mesh node-name should be visible under
    /// this selection. The decision walks the slot tables and uses
    /// per-slot ownership: a mesh that's listed in any outfit slot's
    /// table is shown iff it's in the **selected** variant of that
    /// slot. Meshes not listed anywhere fall through to
    /// face / hair / always-on rules.
    pub fn is_visible(&self, mesh_name: &str) -> bool {
        // Body skin always hidden.
        if mesh_name == "F_BottomBody" || mesh_name == "F_TopBody" {
            return false;
        }
        // Weapons + capes always hidden — no customizer slots expose
        // them. Weapons are reserved for gameplay; capes were
        // dropped after the cape-physics experiment didn't land.
        if ALL_WEAPONS.contains(&mesh_name) || ALL_CAPES.contains(&mesh_name) {
            return false;
        }
        // Face features and hair are decided by their own tables.
        if let Some(rest) = mesh_name.strip_prefix("F_eyes") {
            return is_indexed_match(rest, self.eyes, EYES_VARIANTS.len());
        }
        if let Some(rest) = mesh_name.strip_prefix("F_eyebrows") {
            return is_indexed_match(rest, self.eyebrows, EYEBROWS_VARIANTS.len());
        }
        if let Some(rest) = mesh_name.strip_prefix("F_mouth") {
            return is_indexed_match(rest, self.mouth, MOUTH_VARIANTS.len());
        }
        if mesh_name.starts_with("F_hair") {
            let target = HAIR_VARIANTS.get(self.hair as usize).and_then(|(_, n)| *n);
            return target.is_some_and(|t| t == mesh_name);
        }
        // Outfit slots: if mesh appears in any slot's table, show iff
        // in that slot's selected variant.
        for &slot in SLOTS {
            let Some(table) = slot.outfit_table() else {
                continue;
            };
            if let Some(visible) = decide_outfit_visibility(mesh_name, table, self.index_for(slot))
            {
                return visible;
            }
        }
        // Unrecognized mesh — default to visible.
        true
    }
}

fn decide_outfit_visibility(
    mesh_name: &str,
    table: &[(&str, MeshList)],
    selected: u8,
) -> Option<bool> {
    let mut found_in_table = false;
    for (i, (_, meshes)) in table.iter().enumerate() {
        if meshes.contains(&mesh_name) {
            found_in_table = true;
            if i == selected as usize {
                return Some(true);
            }
        }
    }
    if found_in_table {
        Some(false)
    } else {
        None
    }
}

fn is_indexed_match(rest: &str, selected: u8, len: usize) -> bool {
    if let Ok(n) = rest.parse::<u8>() {
        return (n as usize) < len && n == selected;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify `is_visible` under the DEFAULT `PartSelection` (default witch):
    ///   top=0 (Witch), bottom=0 (Witch), headwear=1 (Witch Hat), hair=1,
    ///   eyes=0, eyebrows=0, mouth=0.
    #[test]
    fn is_visible_default_selection() {
        let sel = PartSelection::default();

        // --- Selected outfit parts are visible ---
        assert!(sel.is_visible("F_Witch_Top"), "Witch Top should be visible");
        assert!(
            sel.is_visible("F_Witch_Bottom"),
            "Witch Bottom should be visible"
        );
        assert!(
            sel.is_visible("F_Witch_Headwear"),
            "Witch Hat should be visible (headwear=1)"
        );

        // --- Non-selected outfit parts are hidden ---
        assert!(
            !sel.is_visible("F_Knight_Top"),
            "Knight Top should be hidden"
        );
        assert!(!sel.is_visible("F_Mage_Top"), "Mage Top should be hidden");

        // --- Face features: only index 0 variants exist and are selected ---
        assert!(
            sel.is_visible("F_eyes0"),
            "F_eyes0 should be visible (eyes=0)"
        );
        assert!(
            !sel.is_visible("F_eyes1"),
            "F_eyes1 should be hidden (only index 0 exists)"
        );
        assert!(sel.is_visible("F_mouth0"), "F_mouth0 should be visible");
        assert!(
            sel.is_visible("F_eyebrows0"),
            "F_eyebrows0 should be visible"
        );

        // --- Weapons always hidden ---
        assert!(
            !sel.is_visible("F_Witch_Staff"),
            "Witch Staff (weapon) should be hidden"
        );
        assert!(
            !sel.is_visible("F_Mage_Staff"),
            "Mage Staff (weapon) should be hidden"
        );

        // --- Capes always hidden ---
        assert!(
            !sel.is_visible("F_Witch_Cape"),
            "Witch Cape should be hidden"
        );

        // --- Body skin always hidden ---
        assert!(!sel.is_visible("F_TopBody"), "F_TopBody should be hidden");
        assert!(
            !sel.is_visible("F_BottomBody"),
            "F_BottomBody should be hidden"
        );
    }
}
