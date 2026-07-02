//! Round-trip test for the serde `CueMessage` wire type.
//!
//! `CueMessage` is a plain serde wire shape `{ cue_id, source_id, position, kind }`. This proves it
//! survives a `serde_json` round-trip (the precondition for putting it on the lightyear wire).

use arena_skills::{CueKind, CueMessage};
use bevy::math::Vec3;

#[test]
fn cue_message_round_trips_through_serde() {
    let m = CueMessage {
        cue_id: "on_cast".into(),
        source_id: "player".into(),
        position: Vec3::new(1.0, 2.0, 3.0),
        aim_dir: Vec3::new(0.0, 0.0, 1.0),
        position_from: None,
        kind: CueKind::OnCast,
    };
    let json = serde_json::to_string(&m).unwrap();
    let back: CueMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back.cue_id, "on_cast");
    assert_eq!(back.source_id, "player");
    assert_eq!(back.position, Vec3::new(1.0, 2.0, 3.0));
    // Bug 1b: the aim direction must survive the wire so observers fly the bolt correctly.
    assert_eq!(back.aim_dir, Vec3::new(0.0, 0.0, 1.0));
    assert_eq!(back.kind, CueKind::OnCast);
}
