mod trace;
use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(MinimalPlugins)
        .add_plugins(trace::TracePlugin)
        .add_systems(Startup, || info!("arena_game up"))
        .add_systems(Update, |mut exit: MessageWriter<AppExit>| {
            exit.write(AppExit::Success);
        })
        .run();
}
