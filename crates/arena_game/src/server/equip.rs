//! Server-side weapon equipping (protocol v4): the [`WeaponCatalog`] resource + the ONLY
//! `MessageReceiver<EquipWeaponMessage>` drain (single-drain rule, invariant §8) + the equip
//! primitive shared with the connect-time spawn.
//!
//! Equipping is obelisk-native: the weapon is a `loot_core::Item`, and `StatBlock::equip`
//! (stat_core) folds its stats into the combatant's computed stats. The one bridge obelisk
//! leaves to the consumer is the SKILL half — the player's `SkillSlots` is REWRITTEN to exactly
//! the weapon's `granted_skills` (obelisk has no revoke API; the vec is the truth), and the
//! replicated [`EquippedWeapon`] component carries the same list in slot order for the cast
//! pipeline + every client's wheel/panel.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{MessageReceiver, RemoteId};
use obelisk_bevy::prelude::*;
use serde_json::json;
use stat_core::EquipmentSlot;

use crate::net::protocol::{EquipWeaponMessage, EquippedWeapon};
use crate::net::weapons::{WeaponCatalog, WeaponDef};
use crate::trace;

use super::rounds::{RoundPhase, RoundState};
use super::spawn::{peer_to_u64, ClientPlayerMap};

/// Queue the full equip onto `player` (works pre-flush from the spawn observer AND on live
/// entities from the drain): `StatBlock::equip(MainHand, item)` (stats + rebuild), `SkillSlots`
/// rewritten to the weapon's skills, current life/mana restored to the (possibly changed)
/// computed maxima — equips happen in the LOBBY, so a fresh loadout starts topped up — and the
/// replicated [`EquippedWeapon`] inserted/updated.
pub(crate) fn queue_equip(commands: &mut Commands, player: Entity, weapon: &WeaponDef) {
    let item = weapon.item.clone();
    let skills = weapon.skills.clone();
    let equipped = EquippedWeapon {
        item_id: weapon.id.clone(),
        skills: skills.clone(),
    };
    commands.entity(player).queue(move |mut entity: EntityWorldMut| {
        if let Some(mut attrs) = entity.get_mut::<Attributes>() {
            attrs.0.equip(EquipmentSlot::MainHand, item);
            let max_life = attrs.0.computed_max_life();
            let max_mana = attrs.0.computed_max_mana();
            attrs.0.current_life = max_life;
            attrs.0.current_mana = max_mana;
        }
        if let Some(mut slots) = entity.get_mut::<SkillSlots>() {
            slots.0 = skills;
        } else {
            entity.insert(SkillSlots(skills));
        }
        entity.insert(equipped);
    });
}

/// THE `EquipWeaponMessage` drain: accept iff the round phase is Lobby (loadouts lock once a
/// match starts) and the id exists in the catalog. Anything else warns and is dropped.
pub(crate) fn drain_equip_weapon(
    mut receivers: Query<(&RemoteId, &mut MessageReceiver<EquipWeaponMessage>), With<ClientOf>>,
    catalog: Res<WeaponCatalog>,
    client_map: Res<ClientPlayerMap>,
    round: Res<RoundState>,
    mut commands: Commands,
) {
    for (RemoteId(peer_id), mut receiver) in &mut receivers {
        let Some(client_id) = peer_to_u64(peer_id) else {
            continue;
        };
        for msg in receiver.receive() {
            if round.phase != RoundPhase::Lobby {
                warn!("equip_weapon outside Lobby (client {client_id}) — ignored");
                continue;
            }
            let Some(weapon) = catalog.get(&msg.item_id) else {
                warn!("equip_weapon: unknown weapon '{}' — ignored", msg.item_id);
                continue;
            };
            let Some(&player) = client_map.0.get(&client_id) else {
                continue;
            };
            queue_equip(&mut commands, player, weapon);
            trace::event(
                "weapon_equipped",
                json!({ "client_id": client_id, "item_id": weapon.id,
                        "skills": weapon.skills }),
            );
        }
    }
}
