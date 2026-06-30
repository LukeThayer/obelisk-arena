//! Server-authoritative state → replicated mirrors + net-event observability. `sync_cast_state`
//! stamps each player's obelisk cast phase into the replicated `NetworkedCastState` so the OTHER
//! client can animate the cast; `sync_networked_health` mirrors obelisk life → `NetworkedHealth`
//! for the HUD; `trace_server_net_events` traces the authoritative `NetEvent` stream for the
//! headless harness.

use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use obelisk_bevy::prelude::*;
use serde_json::json;

use crate::net::input::ArenaInput;
use crate::net::protocol::{NetworkedCastState, NetworkedHealth, NetworkedPlayer, ObeliskNetId};
use crate::trace;

/// Map an optional obelisk `SkillPhase` to the replicated `NetworkedCastState.cast_phase` byte:
/// `None` → 0 (not casting), `Windup` → 1, `Active` → 2, `Recovery` → 3. `SkillPhase::Done` is the
/// terminal phase obelisk removes the `ActiveCast` on, so it maps to 0 (no cast) too. Pure helper so
/// the byte mapping is unit-testable without booting an app.
fn cast_phase_byte(phase: Option<SkillPhase>) -> u8 {
    match phase {
        Some(SkillPhase::Windup) => 1,
        Some(SkillPhase::Active) => 2,
        Some(SkillPhase::Recovery) => 3,
        Some(SkillPhase::Done) | None => 0,
    }
}

/// Stamp each player's obelisk cast state into its replicated `NetworkedCastState` so the OTHER
/// client can drive a cast animation on this player's remote rig (Bug 1a). Runs every `Update` (a
/// caster usually stands still while casting, so this is NOT gated on `Changed`). The `ActiveCast`
/// (the real obelisk cast, post-release) takes precedence; before release, a player CHARGING shows
/// Windup (1) so the opponent sees the cast wind up the instant charging begins (Bug 4). The
/// charging signal rides the replicated `ActionState<ArenaInput>` lightyear maintains.
///
/// Writes `cast_phase`/`cast_skill` ONLY when they change so a delta is shipped, not every frame.
#[allow(clippy::type_complexity)]
pub(crate) fn sync_cast_state(
    mut q: Query<
        (
            Option<&ActiveCast>,
            Option<&ActionState<ArenaInput>>,
            &mut NetworkedCastState,
        ),
        With<NetworkedPlayer>,
    >,
) {
    for (active, action, mut cast) in &mut q {
        let active_phase = cast_phase_byte(active.map(|c| c.phase));
        let charging = action.map(|a| a.0.charging).unwrap_or(false);
        let phase = if active_phase != 0 {
            active_phase
        } else if charging {
            1
        } else {
            0
        };
        // A simple "is casting" marker is enough here (the client only needs phase to animate).
        let skill = if phase == 0 { 0 } else { 1 };
        if cast.cast_phase != phase {
            cast.cast_phase = phase;
        }
        if cast.cast_skill != skill {
            cast.cast_skill = skill;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// HP mirror (guide §5.6): mirror obelisk life → replicated `NetworkedHealth`.
//
// The obelisk sim owns the authoritative life (`Attributes`/`StatBlock.current_life`); the client
// HUD must read a REPLICATED snapshot, not compute damage. Each `Update` we copy `life_of` +
// `max_life_of` (via `ObeliskRead`) into the player's `NetworkedHealth { current, max }`. Lightyear
// replicates the component change to every client (the spawn already inserted `NetworkedHealth`).
//
// We write the component every frame the value differs (not unconditionally) so lightyear only
// ships a delta on a real hp change — and so a throttled trace fires exactly on the hp drop the
// headless net-test asserts (50 → 30 after the first firebolt hit).
// ---------------------------------------------------------------------------------------------

/// Mirror each networked player's obelisk life into its replicated `NetworkedHealth`. Reads
/// `ObeliskRead` (the authoritative life facade); writes the component only when it changes so
/// lightyear ships a delta and the trace fires on the real drop.
pub(crate) fn sync_networked_health(
    read: ObeliskRead,
    mut players: Query<(Entity, &ObeliskNetId, &mut NetworkedHealth), With<NetworkedPlayer>>,
) {
    for (entity, net_id, mut health) in &mut players {
        let Some(current) = read.life_of(entity) else {
            continue;
        };
        let max = read.max_life_of(entity).unwrap_or(current);
        // Only write (and trace) on an actual change so replication ships a delta, not every tick.
        if (health.current - current).abs() > f64::EPSILON
            || (health.max - max).abs() > f64::EPSILON
        {
            health.current = current;
            health.max = max;
            trace::event(
                "hp",
                json!({ "obelisk_id": net_id.0, "current": current, "max": max }),
            );
        }
    }
}

/// Server-side observability: trace every obelisk `NetEvent` the sim mirrors (the same stream the
/// egress bridge broadcasts). Independent `MessageReader` cursor from `egress_net_events`, so both
/// see every event. Gives the headless harness the server-authoritative `CastBegan`/`DamageResolved`
/// to compare the clients' echoed values against.
pub(crate) fn trace_server_net_events(mut net: MessageReader<obelisk_bevy::net::NetEvent>) {
    for ev in net.read() {
        crate::skills::trace_net_event("server", ev);
    }
}

#[cfg(test)]
mod tests {
    use super::cast_phase_byte;
    use obelisk_bevy::prelude::SkillPhase;

    /// The phase→byte mapping the client decodes (`NetworkedCastState.cast_phase`): no cast → 0,
    /// Windup → 1, Active → 2, Recovery → 3; the terminal `Done` (obelisk removes ActiveCast on it)
    /// collapses to 0. Pins the cast-state wire contract.
    #[test]
    fn cast_phase_byte_maps_each_phase() {
        assert_eq!(cast_phase_byte(None), 0);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Windup)), 1);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Active)), 2);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Recovery)), 3);
        assert_eq!(cast_phase_byte(Some(SkillPhase::Done)), 0);
    }
}
