//! `EchoVR_Launcher --play [--runtime <choice>]`: starts Echo VR without the window, the
//! way PLAY would (the folder made ready, the game's sign-in renewed, the route the launch
//! profile picks), and waits for it to end. What starts Echo VR from elsewhere runs this:
//! Steam's library (on Linux also the Flatpak Steam's hand-off), WiVRn's list, desktop
//! shortcuts, SteamVR's library. `--runtime` picks the route for this start only (WiVRn's
//! list uses `wivrn`). On Linux it goes through Proton (`core::linux::play_from_steam`).
//!
//! What only the window can set up (Revive, EchoXR, Linux, an event build's account), or a
//! start Windows blocked, is told in the window: `--play` leaves its word for it and opens
//! it.

use super::store::{InstalledVersion, LauncherState, Runtime, Target};

/// The flag that starts Echo VR without the window.
pub const FLAG: &str = "--play";
/// `--play`'s choice of route for this start.
pub const RUNTIME_FLAG: &str = "--runtime";
/// `--play`'s choice of version (its id), instead of the one PLAY would start: a desktop
/// shortcut's.
pub const VERSION_FLAG: &str = "--version";

/// What `--play` was asked for beyond the launch profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ask {
    pub runtime: Option<Runtime>,
    pub version: Option<String>,
}

impl Ask {
    pub fn of(args: &[String]) -> Ask {
        Ask {
            runtime: runtime_in(args),
            version: value_of(args, VERSION_FLAG).map(str::to_string),
        }
    }

    /// The installed version to start: the one asked for, else the one PLAY would start.
    pub fn version(&self, state: &LauncherState) -> Option<InstalledVersion> {
        match &self.version {
            Some(id) => state.version(id).filter(|v| v.present()).cloned(),
            None => match state.target(None, |v| v.present()) {
                Target::Installed(v) => Some(v),
                _ => None,
            },
        }
    }
}

/// `--play`: starts Echo VR and waits for it; the process' exit code.
pub fn play(args: &[String]) -> i32 {
    let ask = Ask::of(args);
    if cfg!(target_os = "linux") {
        crate::core::linux::play_from_steam(&ask)
    } else {
        play_here(&ask)
    }
}

/// Pure: the value after `flag` among `args`.
fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).map(String::as_str)
}

/// Pure: the route `--runtime` names among `args` (Settings' tiles' names, any case).
pub fn runtime_in(args: &[String]) -> Option<Runtime> {
    match value_of(args, RUNTIME_FLAG)?.to_ascii_lowercase().as_str() {
        "meta" | "metalink" | "link" => Some(Runtime::MetaLink),
        "vd" | "virtualdesktop" => Some(Runtime::VirtualDesktop),
        "steamvr" | "revive" => Some(Runtime::Revive),
        "wivrn" => Some(Runtime::Wivrn),
        "flat" => Some(Runtime::Flat),
        _ => None,
    }
}

/// Windows (and anything not Linux): the start PLAY makes, without its questions.
#[cfg(not(windows))]
fn play_here(_ask: &Ask) -> i32 {
    tracing::error!("--play: not on this system");
    2
}

#[cfg(windows)]
fn play_here(ask: &Ask) -> i32 {
    use super::launch;
    use super::mods::loader_problem;
    use super::ready::{self, Part};
    use super::store::SteamVrVia;
    use crate::core::linux::{self as handoff, Outcome};

    let state = LauncherState::load();
    let Some(v) = ask.version(&state) else {
        tracing::error!("--play: no installed version to start ({:?})", ask.version);
        return tell(
            "Echo VR isn't installed",
            "Install Echo VR in the launcher first.",
        );
    };
    let mut profile = state.profile.clone();
    if let Some(r) = ask.runtime {
        profile.runtime = r;
    }
    let lobby = handoff::take_next_lobby();
    if lobby.as_ref().is_some_and(|(_, spectate)| *spectate) {
        profile.runtime = Runtime::Flat;
        profile.spectator = true;
    }
    let event = v.publisher_lock.is_some();
    let window = |title: &str, message: &str| tell(title, message);
    if event && state.relay_account.is_none() {
        return window(
            "Your classic lobbies account first",
            "Event builds play as your account on the classic lobbies server: press PLAY in the launcher once to set it.",
        );
    }
    // What PLAY's preparation sets up, which needs the window (and maybe administrator
    // rights): not here.
    let revive_dir = match (profile.runtime, profile.steamvr_via) {
        (Runtime::Revive, SteamVrVia::Revive) => match crate::core::revive::find_revive_dir() {
            Some(d) => Some(d),
            None => {
                return window(
                    "SteamVR isn't set up yet",
                    "Press PLAY in the launcher once: it sets up Revive for SteamVR.",
                )
            }
        },
        _ => None,
    };
    if profile.through_echoxr() && !crate::core::echoxr::is_fetched() {
        return window(
            "EchoXR isn't set up yet",
            "Press PLAY in the launcher once: it gets EchoXR for this route.",
        );
    }
    // The live build loads nEVR at its start: without it, or blocked, Windows only says
    // "unable to start correctly".
    if !event {
        if let Some((file, _)) = loader_problem(&v.bin_dir()) {
            return window(
                "Windows blocked Echo VR's mod loader",
                &format!("{file} (nEVR) is missing or can't be read, as a rule because antivirus took it. Press PLAY in the launcher: it offers to allow and repair it."),
            );
        }
    }
    let this_start = LauncherState {
        profile: profile.clone(),
        ..state.clone()
    };
    if let Err(f) = ready::prepare(&this_start, &v) {
        match f.step {
            Part::ClassicLobby | Part::EchoRelay => {
                tracing::error!("--play: {}: {f}", v.id);
                return window(f.step.title(), &format!("{:#}", f.error));
            }
            Part::EchoXr if crate::core::revive::needs_elevation(&f.error) => {
                return window(
                    "EchoXR needs administrator rights here",
                    "Press PLAY in the launcher once: it sets EchoXR up in this folder with administrator rights.",
                );
            }
            Part::EchoXr => {
                tracing::error!("--play: {}: {f}", v.id);
                return window(f.step.title(), &format!("{:#}", f.error));
            }
            Part::Mods | Part::EchoRelayMods => tracing::warn!("--play: {}: {f}", v.id),
        }
    }
    super::game_sign_in::renew_stored(&state.versions);

    let vd_streamer = (profile.runtime == Runtime::VirtualDesktop && !profile.through_echoxr())
        .then(launch::vd_streamer)
        .flatten();
    let tools = launch::Tools {
        revive_dir: revive_dir.as_deref(),
        vd_streamer: vd_streamer.as_deref(),
    };
    let exe = v.exe_path();
    let command = if event {
        launch::build_relay(&profile, &exe, &tools)
    } else {
        launch::build(
            &profile,
            &exe,
            &tools,
            lobby.as_ref().map(|(l, _)| l.as_str()),
        )
    };
    let through_echoxr = profile.through_echoxr();
    let hands = through_echoxr && !event && profile.hands(state.echoxr_hands);
    let command = command.map(|mut c| {
        if hands {
            c.env
                .extend(crate::core::echoxr_hands::layer_env(&v.bin_dir(), |p| {
                    p.to_string_lossy().replace('/', "\\")
                }));
        }
        c
    });
    let mut child = match command.and_then(|c| launch::spawn(&c)) {
        Ok(child) => child,
        Err(e) => {
            tracing::error!("--play: {e:#}");
            return window("Couldn't start Echo VR", &format!("{e:#}"));
        }
    };
    tracing::info!("--play: {} started ({:?})", v.id, profile.runtime);
    handoff::set_playing(Some(&handoff::Playing {
        pid: std::process::id(),
        version: v.id.clone(),
        reaper: None,
    }));
    let code = child.wait().ok().and_then(|s| s.code());
    // Virtual Desktop's streamer and Revive's injector hand the game on and end: Steam
    // shows Echo VR running as long as this does.
    while super::game::running_now().iter().any(|g| {
        g.role == super::game::Role::Client
            && g.exe
                .as_deref()
                .is_none_or(|e| super::game::in_folder(e, &v.root))
    }) {
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    handoff::set_playing(None);
    let Some(code) = code else {
        return 0;
    };
    if let Some(l) = super::game::loader_failure(code).filter(|_| vd_streamer.is_none()) {
        return window(
            "Windows blocked Echo VR's mod loader",
            &format!("Echo VR didn't start: {l} (code {:#x}), as a rule BugSplat64.dll, its mod loader (nEVR). Press PLAY in the launcher: it offers to allow and repair it.", code as u32),
        );
    }
    if code == super::nevr::RUNTIME_REFUSED {
        return window(
            "nEVR couldn't start",
            &super::nevr::runtime_refused_message(),
        );
    }
    if through_echoxr {
        if let Some(why) = crate::core::echoxr::exit_message_for(code, &profile) {
            let _ = handoff::write_outcome(&Outcome {
                title: "Echo VR didn't start through EchoXR".into(),
                message: why.into(),
            });
        }
    }
    tracing::info!("--play: Echo VR ended (exit code {code})");
    code
}

/// Leaves `title` and `message` for the launcher's window, which says it, and opens it.
#[cfg(windows)]
fn tell(title: &str, message: &str) -> i32 {
    tracing::warn!("--play: {title}: {message}");
    let _ = crate::core::linux::write_outcome(&crate::core::linux::Outcome {
        title: title.into(),
        message: message.into(),
    });
    crate::core::tray::open_window();
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_route_for_this_start() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(runtime_in(&args(&["x", "--play"])), None);
        assert_eq!(
            runtime_in(&args(&["x", "--play", "--runtime", "WiVRn"])),
            Some(Runtime::Wivrn)
        );
        assert_eq!(
            runtime_in(&args(&["x", "--play", "--runtime", "steamvr"])),
            Some(Runtime::Revive)
        );
        assert_eq!(
            runtime_in(&args(&["x", "--play", "--runtime", "flat"])),
            Some(Runtime::Flat)
        );
        assert_eq!(runtime_in(&args(&["x", "--play", "--runtime"])), None);
        assert_eq!(
            runtime_in(&args(&["x", "--play", "--runtime", "oculus?"])),
            None
        );
        let ask = Ask::of(&args(&[
            "x",
            "--play",
            "--version",
            "pc-34.4",
            "--runtime",
            "flat",
        ]));
        assert_eq!(ask.version.as_deref(), Some("pc-34.4"));
        assert_eq!(ask.runtime, Some(Runtime::Flat));
        assert_eq!(Ask::of(&args(&["x", "--play"])), Ask::default());
    }
}
