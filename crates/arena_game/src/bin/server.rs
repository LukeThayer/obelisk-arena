//! arena-server: the headless authoritative server (netcode guide §2.2). MinimalPlugins (no
//! rendering) + the asset/mesh/scene plugins avian's collider cache needs + the lightyear server
//! net stack + the obelisk sim (the combat authority) + the arena server gameplay plugin.
//!
//! Boots, binds the UDP socket, and logs "listening". Player spawning (the connect-time
//! `spawn_player_on_connect` observer) + the rest of gameplay live in `ArenaServerPlugin`.

use bevy::log::LogPlugin;
use bevy::mesh::MeshPlugin;
use bevy::prelude::*;
use bevy::scene::ScenePlugin;
use obelisk_bevy::prelude::*;

fn main() {
    let root = arena_game::arena_root();
    let mut app = App::new();

    // Avian's collider cache reads AssetEvent<Mesh> + SceneSpawner; these registrations are
    // required even though the server renders nothing (wisp's headless-server recipe).
    app.add_plugins((
        MinimalPlugins,
        LogPlugin::default(),
        AssetPlugin {
            file_path: root.join("assets").to_string_lossy().into_owned(),
            ..default()
        },
        TransformPlugin,
        MeshPlugin,
        ScenePlugin,
    ));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    // 1. lightyear server stack (adds ServerPlugins { 1/60 } + ProtocolPlugin). Set the bind addr
    //    from CLI (--ip/--port) before Startup's spawn_server reads it.
    app.add_plugins(arena_game::net::ServerNetPlugin);
    let bind = arena_game::net::parse_addr_args(arena_game::net::default_server_addr());
    app.insert_resource(arena_game::net::server::ServerBind { addr: bind });

    // 2. Physics AFTER ServerPlugins so LightyearAvianPlugin sees the replication infra. This is
    //    the sole avian `PhysicsPlugins` registrant; the obelisk sim is composed WITHOUT its own
    //    `ObeliskSpatialPlugin` (which would double-add-panic the physics group).
    arena_game::add_avian_with_lightyear(&mut app);

    // 3. Obelisk sim (HEADLESS, minus its spatial physics group). Authority for combat. CombatRng
    //    seeded from the session seed.
    arena_game::add_obelisk_sim_headless(&mut app);
    app.add_obelisk_config_constants_default();
    app.add_obelisk_effects(&root.join("config/effects"));
    app.add_obelisk_skills(SkillSource::Dir(root.join("config/skills")));
    app.seed_combat_rng(arena_game::net::session_seed());

    // 4. Egress bridge: convert obelisk CueEvents → CueWireMessage + broadcast the authoritative
    //    NetEvent stream → NetEventMessage, both on the reliable EventChannel to every client
    //    (guide §5.5). The server does NOT spawn cosmetics; it only converts + broadcasts.
    arena_game::skills::register_server_cue_egress(&mut app);

    // 5. Arena server gameplay (player spawning, late-joiner refresh, rounds, cast pipeline).
    app.add_plugins(arena_game::server::ArenaServerPlugin);

    app.run();
}
