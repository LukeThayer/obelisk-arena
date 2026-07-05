//! The engine-neutral cue wire type (moved here from the deleted `arena_skills`). `arena_game`
//! owns the lightyear `CueWireMessage`/`LocalCue` wrappers around it.
use bevy::prelude::Vec3;
use obelisk_bevy::events::{CueEvent, CueKind as ObeliskCueKind, EndReason as ObeliskEndReason};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CueKind { OnCast, OnWindow, OnHit, OnEnd, OnEmit }

impl From<ObeliskCueKind> for CueKind {
    fn from(k: ObeliskCueKind) -> Self {
        match k {
            ObeliskCueKind::OnCast => CueKind::OnCast,
            ObeliskCueKind::OnWindow => CueKind::OnWindow,
            ObeliskCueKind::OnHit => CueKind::OnHit,
            ObeliskCueKind::OnEnd => CueKind::OnEnd,
            ObeliskCueKind::OnEmit => CueKind::OnEmit,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndReasonWire { HitEntity, HitWorld, Fuse }
impl From<ObeliskEndReason> for EndReasonWire {
    fn from(r: ObeliskEndReason) -> Self {
        match r {
            ObeliskEndReason::HitEntity => EndReasonWire::HitEntity,
            ObeliskEndReason::HitWorld => EndReasonWire::HitWorld,
            ObeliskEndReason::Fuse => EndReasonWire::Fuse,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CueMessage {
    /// The fired cue id == its slot (`on_cast`/`on_window_{id}`/`on_hit`/`on_end_{id}`/`emit_{id}`).
    pub cue_id: String,
    /// The originating skill id — the client indexes `timelines[skill_id].cues[cue_id]`.
    pub skill_id: String,
    /// Stable `ObeliskId` of the cue's source (caster for OnCast/OnWindow, target for OnHit).
    pub source_id: String,
    pub position: Vec3,
    /// Caster's normalized aim when the cue fired (observers fly Follow proxies the right way).
    pub aim_dir: Vec3,
    #[serde(default)]
    pub position_from: Option<Vec3>,
    /// The cast's charge, forwarded on every slot (drives `ParamSource::Charge` bindings).
    #[serde(default)]
    pub charge: Option<u8>,
    /// Set only on OnEnd cues (from `HitboxEnded.reason`).
    #[serde(default)]
    pub end_reason: Option<EndReasonWire>,
    pub kind: CueKind,
}

/// Pure egress: build the wire message from a fired `CueEvent` + the resolved stable source id +
/// the caster aim. `arena_game` supplies `source_id` (via `ObeliskEntityIndex`) and `aim_dir`.
pub fn cue_event_to_message(ev: &CueEvent, source_id: &str, aim_dir: Vec3) -> CueMessage {
    CueMessage {
        cue_id: ev.cue_id.clone(),
        skill_id: ev.skill_id.clone(),
        source_id: source_id.to_string(),
        position: ev.position,
        aim_dir,
        position_from: ev.position_from,
        charge: ev.charge,
        end_reason: ev.end_reason.map(Into::into),
        kind: ev.kind.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Vec3;
    #[test]
    fn wire_roundtrips_with_skill_charge_endreason() {
        let m = CueMessage {
            cue_id: "on_end_bolt".into(), skill_id: "firebolt".into(),
            source_id: "player_1".into(), position: Vec3::new(1.0, 2.0, 3.0),
            aim_dir: Vec3::NEG_Z, position_from: None, charge: Some(200),
            end_reason: Some(EndReasonWire::HitWorld), kind: CueKind::OnEnd,
        };
        let bytes = serde_json::to_vec(&m).unwrap();
        let back: CueMessage = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(m, back);
    }
}
