#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
mod ui;
mod version;

fn main() {
    // Before any thread starts (see feed::init_local_offset).
    core::launcher::feed::init_local_offset();
    let desktop_fix = core::linux::prepare_desktop_env();
    core::paths::move_from_installer();
    let args: Vec<String> = std::env::args().collect();
    // Elevated helper mode: the app relaunches itself with this flag (as admin) to perform
    // privileged operations for the normal process. Never starts the GUI.
    if args.get(1).map(String::as_str) == Some(core::elevation::HELPER_FLAG) {
        core::log::init("admin-helper.log");
        std::process::exit(core::elevation::helper_main(&args));
    }

    // A spark:// link clicked elsewhere: for the launcher already running (this one then
    // exits), or for this one once it is up.
    if let Some(link) = args.get(1).filter(|a| core::links::parse(a).is_some()) {
        if core::links::hand_over(link) {
            return;
        }
    }

    // Linux: Steam's shortcut runs the launcher to start the game (see core::linux), with
    // --play, or without it once Steam has dropped the shortcut's launch options.
    if args.get(1).map(String::as_str) == Some(core::linux::PLAY_FLAG)
        || (args.len() == 1 && core::linux::started_for_shortcut())
    {
        core::log::init("play.log");
        std::process::exit(core::linux::play_from_steam());
    }

    core::log::init("EchoVR_Launcher.log");
    core::linux::log_desktop_env(desktop_fix.as_deref());
    let result = ui::run();
    core::elevation::shutdown();
    core::uninstall::after_exit();
    if let Err(e) = result {
        tracing::error!("fatal: {e}");
        eprintln!("{e}");
        std::process::exit(1);
    }
}
