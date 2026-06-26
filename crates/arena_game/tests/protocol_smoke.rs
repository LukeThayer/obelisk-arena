//! Smoke test: `ProtocolPlugin` adds cleanly to a minimal app — no double-registration / missing
//! channel-direction panic. The component + message + channel registration is the assertion;
//! building without panic proves the protocol is internally consistent (netcode guide §3, plan
//! Task 6).

#[test]
fn protocol_plugin_builds() {
    let mut app = bevy::app::App::new();
    // The protocol's component registration leans on bevy's core type infrastructure; a minimal
    // App + TimePlugin is enough for the registry calls (mirrors wisp's protocol unit coverage).
    app.add_plugins(bevy::time::TimePlugin);
    app.add_plugins(arena_game::net::protocol::ProtocolPlugin);
    // Building without panic is the assertion.
    app.finish();
    app.cleanup();
}
