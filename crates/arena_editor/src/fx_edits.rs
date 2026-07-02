//! Pure cosmetic-lane edit helpers backing the "Cosmetic Lanes" rows in the skill panel.
//!
//! Each helper mutates the in-flight [`SkillFx`] (via [`EditedSkillFx`](crate::model::EditedSkillFx))
//! for one authored cue lane: ensure a lane exists for a cue id, set its anim clip / particle
//! effect / particle socket, or push a `bevy_vfx` param binding. `cue_keys_for` lists the LOCKED
//! cue-id VALUES the panel offers as lane slots (the sorted [`derive_vfx_cues`] values). All logic
//! is engine-neutral data mutation so it is unit-tested directly; the egui rows that call it are a
//! compile-gated shell.

use arena_skills::{AnimLayer, CueKind, LaneEvent, ParticleSpec, SkillFx, VfxParamBinding};
use obelisk_bevy::assets::CastTimeline;

use crate::model::derive_vfx_cues;

/// Return (inserting if absent) the lane bound to `cue_id`. Idempotent: a second call with the same
/// `cue_id` returns the existing lane without adding another.
pub fn ensure_lane<'a>(fx: &'a mut SkillFx, cue_id: &str, kind: CueKind) -> &'a mut LaneEvent {
    fx.lanes
        .entry(cue_id.to_string())
        .or_insert_with(|| LaneEvent {
            lane_id: cue_id.to_string(),
            kind,
            particle: None,
            projectile: None,
            beam: None,
            anim: None,
        })
}

/// Set (or clear, with `None`) the anim clip a lane drives on the source rig at `layer`/`weight`.
pub fn set_anim_clip(lane: &mut LaneEvent, clip: Option<String>, layer: u32, weight: f32) {
    match clip {
        None => lane.anim = None,
        Some(c) => {
            lane.anim = Some(AnimLayer {
                state: String::new(),
                clip: Some(c),
                layer,
                weight,
            })
        }
    }
}

/// Ensure the lane has a `ParticleSpec`, returning it for mutation. Seeds a sensible default burst.
fn ensure_particle(lane: &mut LaneEvent) -> &mut ParticleSpec {
    lane.particle.get_or_insert_with(|| ParticleSpec {
        count: 12,
        lifetime: 0.4,
        color: [1.0, 0.5, 0.1],
        speed: 4.0,
        effect: None,
        socket: None,
        offset: bevy::math::Vec3::ZERO,
        param_bindings: Vec::new(),
    })
}

/// Set (or clear) the named `bevy_vfx` effect the lane's particle burst spawns.
pub fn set_particle_effect(lane: &mut LaneEvent, effect: Option<String>) {
    ensure_particle(lane).effect = effect;
}

/// Set (or clear) the rig socket the lane's particle burst attaches to.
pub fn set_particle_socket(lane: &mut LaneEvent, socket: Option<String>) {
    ensure_particle(lane).socket = socket;
}

/// Push a `bevy_vfx` param binding onto the lane's particle burst.
pub fn add_param_binding(lane: &mut LaneEvent, b: VfxParamBinding) {
    ensure_particle(lane).param_bindings.push(b);
}

/// The sorted LOCKED cue-id VALUES the panel offers as lane slots — the values of
/// [`derive_vfx_cues`] (`{id}_cast`, `{id}_window_{wid}`, `{id}_impact`).
pub fn cue_keys_for(tl: &CastTimeline) -> Vec<String> {
    let mut v: Vec<String> = derive_vfx_cues(tl).into_values().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edits::add_collision_window;
    use crate::model::blank_cast_timeline;
    use arena_skills::VfxBindSource;

    #[test]
    fn ensure_lane_is_idempotent() {
        let mut fx = crate::model::blank_skillfx("firebolt");
        ensure_lane(&mut fx, "firebolt_cast", CueKind::OnCast);
        ensure_lane(&mut fx, "firebolt_cast", CueKind::OnCast);
        assert_eq!(fx.lanes.len(), 1);
        assert_eq!(fx.lanes["firebolt_cast"].kind, CueKind::OnCast);
    }

    #[test]
    fn setters_mutate_the_lane_as_expected() {
        let mut fx = crate::model::blank_skillfx("firebolt");
        {
            let lane = ensure_lane(&mut fx, "firebolt_cast", CueKind::OnCast);
            set_anim_clip(lane, Some("cast_release".into()), 1, 0.8);
            set_particle_effect(lane, Some("muzzle".into()));
            set_particle_socket(lane, Some("wand_tip".into()));
            add_param_binding(
                lane,
                VfxParamBinding {
                    param: "scale".into(),
                    source: VfxBindSource::Charge,
                    min: 0.5,
                    max: 2.0,
                },
            );
        }
        let lane = &fx.lanes["firebolt_cast"];
        let anim = lane.anim.as_ref().expect("anim set");
        assert_eq!(anim.clip.as_deref(), Some("cast_release"));
        assert_eq!(anim.layer, 1);
        assert_eq!(anim.weight, 0.8);
        let particle = lane.particle.as_ref().expect("particle set");
        assert_eq!(particle.effect.as_deref(), Some("muzzle"));
        assert_eq!(particle.socket.as_deref(), Some("wand_tip"));
        assert_eq!(particle.param_bindings.len(), 1);
        assert_eq!(particle.param_bindings[0].param, "scale");

        // Clearing the anim clip removes the layer entirely.
        set_anim_clip(fx.lanes.get_mut("firebolt_cast").unwrap(), None, 0, 1.0);
        assert!(fx.lanes["firebolt_cast"].anim.is_none());
    }

    #[test]
    fn cue_keys_for_firebolt_with_one_window_lists_the_locked_values() {
        let mut tl = blank_cast_timeline("firebolt");
        add_collision_window(&mut tl);
        let keys = cue_keys_for(&tl);
        assert!(keys.contains(&"firebolt_cast".to_string()));
        assert!(keys.contains(&"firebolt_window_window_0".to_string()));
        assert!(keys.contains(&"firebolt_impact".to_string()));
        // Sorted.
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
