//! SIM-BACKED scrubbing (UX spec P3, D2): the scrub head drives the REAL deterministic sim,
//! not a synthetic staging. Dragging to time `t` restarts the cast on the persistent stage
//! (same seed → identical every time) and runs the fixed-tick sim to `t` SYNCHRONOUSLY —
//! `drive_scrub` is an exclusive system that calls `world.run_schedule(FixedUpdate)` for
//! exactly the ticks needed, so every drag frame ends with the sim AT the pointer (no
//! multi-frame catch-up, no virtual-time games, camera untouched). Between drags the sim is
//! FROZEN via a run-condition gate on the obelisk sets: the bolt hangs mid-arc exactly where
//! the game would have it, and hits/chains have resolved iff the target is past their true
//! moments.
//!
//! Verbs: DRAG the strip (forward continues from the current tick; backward restarts and
//! re-sims the prefix — deterministic, identical every time), ⟳ REPLAY (restart, run ambient
//! at 1×, auto-freeze at the strip end), and the charge slider (the cast's charge byte —
//! arcs flatten and damage scales, honestly, because it IS the sim).

use crate::model::derive_vfx_cues;
use crate::model::EditedSkill;
use crate::preview_controller::{stage_cast, PreviewStageReset};
use crate::timeline_geom::strip_span;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy_editor_game::GameState;

/// Hard cap on fixed ticks one synchronous seek may run (10 s of sim at 60 Hz) — a guard
/// against a runaway span, not a tuning knob.
const MAX_SEEK_TICKS: u32 = 600;

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
    /// The target the last seek served. Comparing the USER's `target` against this (never
    /// against the clock, which sits one tick past by construction) distinguishes a real new
    /// request from holding still.
    sought: Option<f32>,
    /// Where a replay auto-freezes (the strip end, captured at replay start).
    end: f32,
    /// True only INSIDE `drive_scrub`'s synchronous tick loop — unfreezes the obelisk sets
    /// for the manually-run schedule while the ambient fixed loop stays frozen.
    exclusive_running: bool,
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
            exclusive_running: false,
        }
    }
}

/// Run condition gating the obelisk sim sets (+ the preview cosmetic clocks). The ambient
/// fixed loop runs the sim only when NO scrub session holds it (Idle) or a replay is playing;
/// during Seeking/Frozen all sim advancement happens inside `drive_scrub`'s synchronous loop
/// (which sets `exclusive_running`). Registered by the skill designer ONLY — the game never
/// sees this gate.
pub fn sim_unfrozen(scrub: Option<Res<ScrubSim>>) -> bool {
    scrub.is_none_or(|s| match s.mode {
        ScrubMode::Idle | ScrubMode::Replaying => true,
        ScrubMode::Seeking | ScrubMode::Frozen => s.exclusive_running,
    })
}

/// Tick the scrub clock with the fixed clock while a scrub cast is live; freeze a replay the
/// tick it crosses the strip end. Gated like the sim, ordered before the obelisk sets.
pub fn tick_scrub_clock(time: Res<Time<Fixed>>, mut scrub: ResMut<ScrubSim>) {
    if !scrub.cast_live {
        return;
    }
    scrub.clock += time.delta_secs();
    if scrub.mode == ScrubMode::Replaying && scrub.clock >= scrub.end {
        scrub.mode = ScrubMode::Frozen;
    }
}

/// The scrub state machine — an EXCLUSIVE system: a new target runs the fixed schedule
/// synchronously for exactly the ticks needed (restarting first for backward targets), so the
/// frame ends with the sim AT the pointer and frozen there. Replay runs on the ambient fixed
/// loop at 1× and freezes at the strip end. Entering Play ends the session.
pub fn drive_scrub(world: &mut World) {
    // Play owns the sim: end any scrub session the moment we leave Editing.
    let game_state = *world.resource::<State<GameState>>().get();
    if game_state != GameState::Editing {
        let mut scrub = world.resource_mut::<ScrubSim>();
        if scrub.mode != ScrubMode::Idle {
            scrub.mode = ScrubMode::Idle;
            scrub.cast_live = false;
        }
        return;
    }

    let span = strip_span(&world.resource::<EditedSkill>().timeline).max(0.0001);

    // Replay request: restart, then let the ambient loop play it at 1×.
    if world.resource::<ScrubSim>().replay_requested {
        restart_cast(world);
        let mut scrub = world.resource_mut::<ScrubSim>();
        scrub.replay_requested = false;
        scrub.mode = ScrubMode::Replaying;
        scrub.end = span;
        scrub.target = None;
        return;
    }

    if world.resource::<ScrubSim>().mode == ScrubMode::Replaying {
        return; // ambient loop is playing; tick_scrub_clock freezes it at the end
    }

    // Seek requests.
    let (target, needs_restart) = {
        let scrub = world.resource::<ScrubSim>();
        let Some(raw) = scrub.target else { return };
        let target = raw.clamp(0.0, span);
        if scrub.sought == Some(target) && scrub.cast_live {
            return; // already there — hold the frozen instant
        }
        // Backward (or no cast yet): restart and re-sim the prefix. NEVER compare the clock
        // (it sits at tick granularity past the sought time by construction).
        (target, !scrub.cast_live || target < scrub.clock - 1e-4)
    };
    if needs_restart {
        restart_cast(world);
    }
    // Synchronous seek: run the fixed schedule tick by tick until the clock reaches the
    // target. `exclusive_running` unfreezes the obelisk sets for these manual runs only.
    {
        let mut scrub = world.resource_mut::<ScrubSim>();
        scrub.sought = Some(target);
        scrub.mode = ScrubMode::Seeking;
        scrub.exclusive_running = true;
    }
    let mut guard = 0;
    while world.resource::<ScrubSim>().clock < target && guard < MAX_SEEK_TICKS {
        world.run_schedule(FixedUpdate);
        guard += 1;
    }
    let mut scrub = world.resource_mut::<ScrubSim>();
    scrub.exclusive_running = false;
    scrub.mode = ScrubMode::Frozen;
}

/// Restart the deterministic scrub cast: reset the stage (heal, reposition, clear hitboxes +
/// cosmetics, reseed, clear cooldowns), re-register the edited timeline, cast with the
/// session's charge, zero the clock. Uses `run_system_once` so the queued commands (despawns,
/// the cast) are APPLIED before the caller's synchronous tick loop starts.
fn restart_cast(world: &mut World) {
    let mut tl = world.resource::<EditedSkill>().timeline.clone();
    tl.vfx_cues = derive_vfx_cues(&tl);
    let charge = world.resource::<ScrubSim>().charge;
    world
        .run_system_once(|mut reset: PreviewStageReset| reset.reset_stage())
        .expect("stage reset runs");
    world
        .run_system_once(move |mut reset: PreviewStageReset| {
            stage_cast(&mut reset, tl.clone(), Some(charge));
        })
        .expect("stage cast runs");
    let mut scrub = world.resource_mut::<ScrubSim>();
    scrub.clock = 0.0;
    scrub.cast_live = true;
    scrub.sought = None;
}
