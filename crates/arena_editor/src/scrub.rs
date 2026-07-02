//! SIM-BACKED scrubbing (UX spec P3, D2): the scrub head drives the REAL deterministic sim,
//! not a synthetic staging. Dragging to time `t` restarts the cast on the persistent stage
//! (same seed → identical every time) and fast-forwards the fixed-tick sim to `t`, then
//! FREEZES it there — the bolt hangs mid-arc exactly where the game would have it, hits and
//! chains land at their true moments, and you can orbit the frozen instant with the camera
//! (only the SIM is frozen, via a run-condition gate on the obelisk sets; virtual time only
//! pulses fast during the brief catch-up).
//!
//! Verbs: DRAG the strip (seek — forward continues, backward restarts + reseeks), ⟳ REPLAY
//! (restart, run at 1×, auto-freeze at the end), and the charge slider (the cast's charge
//! byte — arcs flatten and damage scales, honestly, because it IS the sim).

use crate::model::derive_vfx_cues;
use crate::model::EditedSkill;
use crate::preview_controller::{stage_cast, PreviewStageReset};
use crate::timeline_geom::strip_span;
use bevy::prelude::*;
use bevy_editor_game::GameState;

/// How fast the catch-up runs (virtual-time relative speed while seeking).
const SEEK_SPEED: f32 = 24.0;

/// The scrub session's state machine.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum ScrubMode {
    /// No scrub session: the sim runs free (Play uses this).
    #[default]
    Idle,
    /// Fast-forwarding the sim toward `target`.
    Seeking,
    /// Sim frozen at the reached time; camera stays live.
    Frozen,
    /// Replaying at 1× from the start; freezes at the strip end.
    Replaying,
}

/// The scrub controller resource. The panel writes `target` (strip drag) / requests a replay;
/// `drive_scrub` runs the machine; `clock` is the sim time since the scrubbed cast began.
#[derive(Resource)]
pub struct ScrubSim {
    pub mode: ScrubMode,
    /// Requested sim time (strip drag). `None` until the first grab.
    pub target: Option<f32>,
    /// Sim seconds since the scrub cast started (ticks with the fixed clock while unfrozen).
    pub clock: f32,
    /// Set by the ⟳ button: restart and play at 1×.
    pub replay_requested: bool,
    /// The cast's charge byte (the strip slider). 85 ≈ 1.0× (tap); 255 = 2.0× (full hold).
    pub charge: u8,
    /// True once a scrub cast has been fired on the stage this session.
    cast_live: bool,
    /// The target the CURRENT seek is running toward. `tick_scrub_clock` freezes the sim the
    /// moment the clock crosses it — BETWEEN fixed ticks, so a fast-forward frame that would
    /// gulp seconds of sim stops on the exact tick instead of overshooting. Comparing the
    /// USER's `target` against this (not against the clock) is what distinguishes a real
    /// backward drag from seek overshoot.
    sought: Option<f32>,
    /// Where a replay auto-freezes (the strip end, captured at replay start).
    end: f32,
}

impl Default for ScrubSim {
    fn default() -> Self {
        Self {
            mode: ScrubMode::Idle,
            target: None,
            clock: 0.0,
            replay_requested: false,
            charge: 85, // ≈ 1.0× — an uncharged tap
            cast_live: false,
            sought: None,
            end: 0.0,
        }
    }
}

impl ScrubSim {
    pub fn frozen(&self) -> bool {
        self.mode == ScrubMode::Frozen
    }
}

/// Run condition gating the obelisk sim sets (+ the preview cosmetic clocks): the sim runs
/// unless the scrub has it frozen. Registered by the skill designer ONLY — the game never sees
/// this gate.
pub fn sim_unfrozen(scrub: Option<Res<ScrubSim>>) -> bool {
    scrub.is_none_or(|s| !s.frozen())
}

/// Tick the scrub clock with the fixed clock while a scrub cast is live, and FREEZE the sim
/// the tick the clock crosses the sought time / replay end. Runs in FixedUpdate ordered before
/// the obelisk sets and gated like them: a fast-forward frame that runs many fixed ticks stops
/// on the exact tick (the remaining obelisk iterations that frame see the freeze and skip),
/// instead of overshooting by the whole frame's virtual gulp.
pub fn tick_scrub_clock(time: Res<Time<Fixed>>, mut scrub: ResMut<ScrubSim>) {
    if !scrub.cast_live {
        return;
    }
    scrub.clock += time.delta_secs();
    match scrub.mode {
        ScrubMode::Seeking if scrub.sought.is_some_and(|t| scrub.clock >= t) => {
            scrub.mode = ScrubMode::Frozen;
        }
        ScrubMode::Replaying if scrub.clock >= scrub.end => {
            scrub.mode = ScrubMode::Frozen;
        }
        _ => {}
    }
}

/// The scrub state machine (Update):
/// - a new/backward `target` (or a replay request) RESTARTS the stage cast — deterministic
///   reseed, so every replay is identical;
/// - Seeking runs virtual time at [`SEEK_SPEED`] until `clock` reaches `target`, then freezes;
/// - Replaying runs at 1× and freezes at the strip end;
/// - entering Play (GameState != Editing) ends the session and returns everything to normal.
#[allow(clippy::too_many_arguments)]
pub fn drive_scrub(
    mut scrub: ResMut<ScrubSim>,
    edited: Res<EditedSkill>,
    game_state: Res<State<GameState>>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut reset: PreviewStageReset,
) {
    // Play owns the sim: end any scrub session the moment we leave Editing.
    if *game_state.get() != GameState::Editing {
        if scrub.mode != ScrubMode::Idle {
            scrub.mode = ScrubMode::Idle;
            scrub.cast_live = false;
            virtual_time.set_relative_speed(1.0);
        }
        return;
    }

    let span = strip_span(&edited.timeline).max(0.0001);

    // Replay request: restart and run at 1×; `tick_scrub_clock` freezes at the strip end.
    if scrub.replay_requested {
        scrub.replay_requested = false;
        restart_cast(&mut scrub, &edited, &mut reset);
        scrub.mode = ScrubMode::Replaying;
        scrub.end = span;
        scrub.target = None;
        virtual_time.set_relative_speed(1.0);
        return;
    }

    match scrub.mode {
        ScrubMode::Replaying => {
            // Freeze happens tick-precise in `tick_scrub_clock`; restore normal speed once it
            // lands (harmless if already 1×).
        }
        ScrubMode::Idle | ScrubMode::Frozen | ScrubMode::Seeking => {
            let Some(raw) = scrub.target else {
                return;
            };
            let target = raw.clamp(0.0, span);
            if scrub.sought == Some(target) && scrub.cast_live {
                // Same request as the running/finished seek: hold. (Overshoot past `target`
                // is seek granularity, NOT a backward drag — never compare clock to target
                // for restart decisions.)
                if scrub.mode == ScrubMode::Frozen {
                    virtual_time.set_relative_speed(1.0);
                }
                return;
            }
            // New request: restart only if we can't reach it by running FORWARD.
            if !scrub.cast_live || target < scrub.clock - 1e-4 {
                restart_cast(&mut scrub, &edited, &mut reset);
            }
            scrub.sought = Some(target);
            if scrub.clock >= target {
                scrub.mode = ScrubMode::Frozen;
                virtual_time.set_relative_speed(1.0);
            } else {
                scrub.mode = ScrubMode::Seeking;
                virtual_time.set_relative_speed(SEEK_SPEED);
            }
        }
    }
}

/// Restart the deterministic scrub cast: reset the stage (heal, reposition, clear hitboxes +
/// cosmetics, reseed, clear cooldowns), re-register the edited timeline, cast with the
/// session's charge, zero the clock.
fn restart_cast(scrub: &mut ScrubSim, edited: &EditedSkill, reset: &mut PreviewStageReset) {
    reset.reset_stage();
    let mut tl = edited.timeline.clone();
    tl.vfx_cues = derive_vfx_cues(&tl);
    stage_cast(reset, tl, Some(scrub.charge));
    scrub.clock = 0.0;
    scrub.cast_live = true;
    scrub.sought = None;
}
