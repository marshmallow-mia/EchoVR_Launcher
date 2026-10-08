//! Starting the PC game with a launch profile.
//!
//! Commands are built as argv (never through a shell) and the game's output is discarded
//! -- the old launcher piped stderr without reading it, which can stall the game.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use super::store::{LaunchProfile, Runtime};
use crate::core::{echoxr, revive};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// More environment for the game (EchoXR Hands' OpenXR layer).
    pub env: Vec<(String, String)>,
}

/// Splits extra arguments like a command line: whitespace separates, double quotes group.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Arguments back into one command line (for shortcuts): those with spaces in quotes.
pub fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.chars().any(char::is_whitespace) {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A lobby id as the game expects it: a UUID (any `.node` suffix dropped).
pub fn lobby_uuid(input: &str) -> Option<String> {
    let s = input.trim();
    // Accept pasted spark:// / echo.taxi links: the UUID is the last path segment.
    let last = s.rsplit('/').next().unwrap_or(s);
    let id = last.split('.').next().unwrap_or(last);
    let hex: String = id.chars().filter(|c| *c != '-').collect();
    (hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()) && id.len() == 36)
        .then(|| id.to_ascii_uppercase())
}

/// The game arguments for a profile (without the executable).
pub fn game_args(profile: &LaunchProfile, lobby: Option<&str>) -> Vec<String> {
    let mut args = Vec::new();
    if profile.runtime == Runtime::Flat {
        args.push("-noovr".to_string());
        if profile.spectator {
            args.push("-spectatorstream".into());
        }
        // Always: nEVR (and EchoRelay's patch) take -windowed for "no headset"; with
        // -noovr alone the game still starts Meta's runtime ("Failed to initialize the
        // Oculus VR session"). Never in VR: it would start the game on the monitor.
        args.push("-windowed".into());
    }
    if let Some(l) = lobby {
        args.push("-lobbyid".into());
        args.push(l.to_string());
    }
    args.extend(split_args(&profile.extra_args));
    args
}

/// What starts the game for some choices, where this PC has it: Revive's folder (SteamVR
/// through Revive) and Virtual Desktop's streamer (Virtual Desktop's Oculus mode).
#[derive(Debug, Clone, Copy, Default)]
pub struct Tools<'a> {
    pub revive_dir: Option<&'a str>,
    pub vd_streamer: Option<&'a Path>,
}

pub fn build(
    profile: &LaunchProfile,
    exe: &Path,
    tools: &Tools,
    lobby: Option<&str>,
) -> Result<Command> {
    let cwd = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let args = game_args(profile, lobby);
    // EchoXR.exe beside the game starts it (as its copy, echovr_openxr.exe) on an OpenXR
    // runtime: SteamVR's (SteamVR, or Virtual Desktop's SteamVR driver), or Virtual
    // Desktop's own. Its arguments first; the game gets the rest.
    if profile.through_echoxr() {
        let mut a = profile.echoxr_args();
        a.extend(args);
        return Ok(Command {
            program: cwd.join(echoxr::LAUNCHER),
            args: a,
            cwd,
            env: Vec::new(),
        });
    }
    match profile.runtime {
        // Virtual Desktop's Oculus mode: its streamer starts the game and injects itself,
        // as its Games list (or its tray's "Inject Game") does; Meta's runtime alone has no
        // headset under Virtual Desktop.
        Runtime::VirtualDesktop => {
            let Some(streamer) = tools.vd_streamer else {
                bail!("Virtual Desktop's streamer isn't installed. Install Virtual Desktop Streamer, or choose another way under Virtual Desktop in Settings.");
            };
            let mut a = vec![exe.to_string_lossy().into_owned()];
            a.extend(args);
            Ok(Command {
                program: streamer.to_path_buf(),
                args: a,
                cwd,
                env: Vec::new(),
            })
        }
        // WiVRn is Linux's, which starts through Steam (`core::linux`), not here.
        Runtime::MetaLink | Runtime::Wivrn | Runtime::Flat => Ok(Command {
            program: exe.to_path_buf(),
            args,
            cwd,
            env: Vec::new(),
        }),
        Runtime::Revive => {
            let Some(dir) = tools.revive_dir else {
                bail!("Revive is not installed. Set up SteamVR from the PLAY button first.");
            };
            let mut a = vec![exe.to_string_lossy().into_owned(), "-nosymbollookup".into()];
            a.extend(args);
            a.push("/app".into());
            a.push(revive::APP_ID.into());
            Ok(Command {
                program: Path::new(dir).join(revive::REVIVE_INJECTOR),
                args: a,
                cwd: PathBuf::from(dir),
                env: Vec::new(),
            })
        }
    }
}

/// [`build`] for an event build on the classic lobbies relay, as the relay's own launcher
/// starts it: no arguments at all (the 2019 build quits on any it doesn't know), from its
/// game folder. These builds always start in VR, so Flat plays like Meta Link. Through
/// EchoXR, `EchoXR.exe` starts from the bin folder and starts the game from its game folder.
pub fn build_relay(profile: &LaunchProfile, exe: &Path, tools: &Tools) -> Result<Command> {
    let runtime = match profile.runtime {
        Runtime::Flat => Runtime::MetaLink,
        rt => rt,
    };
    let bare = LaunchProfile {
        runtime,
        steamvr_via: profile.steamvr_via,
        vd_via: profile.vd_via,
        ..Default::default()
    };
    let mut c = build(&bare, exe, tools, None)?;
    // Revive's injector starts from Revive's folder, EchoXR.exe from the bin folder.
    if runtime != Runtime::Revive && !bare.through_echoxr() {
        // bin/win7/<exe> -> the game folder.
        if let Some(game) = exe.ancestors().nth(3) {
            c.cwd = game.to_path_buf();
        }
    }
    Ok(c)
}

/// Virtual Desktop's streamer on this PC (Windows): the running one's executable, else the
/// one where its installer puts it.
pub fn vd_streamer() -> Option<PathBuf> {
    const EXE: &str = "VirtualDesktop.Streamer.exe";
    if !cfg!(windows) {
        return None;
    }
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
    );
    let running = sys
        .processes()
        .values()
        .filter(|p| p.name().to_string_lossy().eq_ignore_ascii_case(EXE))
        .find_map(|p| p.exe().map(Path::to_path_buf))
        .filter(|p| p.is_file());
    running.or_else(|| {
        let dir = std::env::var_os("ProgramFiles").map(PathBuf::from)?;
        Some(dir.join("Virtual Desktop Streamer").join(EXE)).filter(|p| p.is_file())
    })
}

/// Things worth warning about before launching (not errors: the user may know better).
pub fn preflight(profile: &LaunchProfile) -> Option<String> {
    let running = |name: &str| super::game::process_running(name);
    match profile.runtime {
        Runtime::MetaLink if cfg!(windows) && !running("OVRServer_x64.exe") => Some(
            "The Meta Quest Link app (Oculus runtime) doesn't seem to be running.\nStart it and connect your headset first."
                .into(),
        ),
        Runtime::VirtualDesktop if cfg!(windows) && !running("VirtualDesktop.Streamer.exe") => Some(
            "The Virtual Desktop Streamer doesn't seem to be running.\nStart it and connect from your headset first."
                .into(),
        ),
        _ => None,
    }
}

pub fn spawn(cmd: &Command) -> Result<std::process::Child> {
    if !cfg!(windows) {
        bail!("Launching the PC game is only supported on Windows.");
    }
    tracing::info!("launch: {} {:?}", cmd.program.display(), cmd.args);
    let child = crate::core::process::command(&cmd.program)
        .args(&cmd.args)
        .current_dir(&cmd.cwd)
        .envs(cmd.env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::launcher::store::{SteamVrVia, VdVia};

    const LOBBY: &str = "0f5c1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b";

    #[test]
    fn joins_what_split_splits() {
        let args = split_args(r#"-a "b c" -d"#);
        assert_eq!(join_args(&args), r#"-a "b c" -d"#);
        assert_eq!(split_args(&join_args(&args)), args);
    }

    #[test]
    fn splits_like_a_command_line() {
        assert_eq!(split_args(r#"-a  "b c" d"" "#), ["-a", "b c", "d"]);
        assert!(split_args("  ").is_empty());
        assert_eq!(split_args(r#""""#), [""]);
    }

    #[test]
    fn parses_lobby_ids() {
        let up = LOBBY.to_ascii_uppercase();
        assert_eq!(lobby_uuid(LOBBY).as_deref(), Some(up.as_str()));
        assert_eq!(
            lobby_uuid(&format!("spark://c/{LOBBY}")).as_deref(),
            Some(up.as_str())
        );
        assert_eq!(
            lobby_uuid(&format!("https://echo.taxi/spark://j/{LOBBY}")).as_deref(),
            Some(up.as_str())
        );
        assert_eq!(
            lobby_uuid(&format!("{LOBBY}.nakama2_us-east")).as_deref(),
            Some(up.as_str())
        );
        assert_eq!(lobby_uuid("not-a-lobby"), None);
    }

    #[test]
    fn argv_per_runtime() {
        let exe = Path::new("C:/E/ready-at-dawn-echo-arena/bin/win10/echovr.exe");
        let mut p = LaunchProfile {
            extra_args: "-foo".into(),
            ..Default::default()
        };
        let c = build(&p, exe, &Tools::default(), Some(LOBBY)).unwrap();
        assert_eq!(c.program, exe);
        assert_eq!(c.args, ["-lobbyid", LOBBY, "-foo"]);
        assert_eq!(c.cwd, Path::new("C:/E/ready-at-dawn-echo-arena/bin/win10"));
        // An event build starts bare, from its game folder, even in Flat.
        let old = Path::new("C:/E/ready-at-dawn-echo-arena/bin/win7/echovr.exe");
        let flat = LaunchProfile {
            runtime: Runtime::Flat,
            ..p.clone()
        };
        let c = build_relay(&flat, old, &Tools::default()).unwrap();
        assert!(c.args.is_empty());
        assert_eq!(c.cwd, Path::new("C:/E/ready-at-dawn-echo-arena"));
        // Through EchoXR: EchoXR.exe beside it, from the bin folder, bare too.
        let xr = LaunchProfile {
            runtime: Runtime::Revive,
            steamvr_via: SteamVrVia::EchoXr,
            ..p.clone()
        };
        let c = build_relay(&xr, old, &Tools::default()).unwrap();
        assert_eq!(
            c.program,
            Path::new("C:/E/ready-at-dawn-echo-arena/bin/win7").join(echoxr::LAUNCHER)
        );
        assert!(c.args.is_empty());
        assert_eq!(c.cwd, Path::new("C:/E/ready-at-dawn-echo-arena/bin/win7"));

        // Flat: always in a window (-windowed is nEVR's "no headset"), Windowed or not.
        p.runtime = Runtime::Flat;
        assert_eq!(
            build(&p, exe, &Tools::default(), None).unwrap().args,
            ["-noovr", "-windowed", "-foo"]
        );
        p.spectator = true;
        assert_eq!(
            build(&p, exe, &Tools::default(), None).unwrap().args,
            ["-noovr", "-spectatorstream", "-windowed", "-foo"]
        );
        p.windowed = true;
        // Virtual Desktop: in its Oculus mode its streamer starts the game (and injects
        // itself); through SteamVR or VD's OpenXR runtime, EchoXR.exe (told to take the
        // system's runtime for VD's own).
        let bin = Path::new("C:/E/ready-at-dawn-echo-arena/bin/win10");
        let streamer =
            Path::new("C:/Program Files/Virtual Desktop Streamer/VirtualDesktop.Streamer.exe");
        let vd_tools = Tools {
            vd_streamer: Some(streamer),
            ..Default::default()
        };
        // In VR, Windowed left on doesn't start it flat (nEVR: -windowed = no headset).
        p.runtime = Runtime::VirtualDesktop;
        assert_eq!(
            build(&p, exe, &vd_tools, None).unwrap().args,
            [exe.to_string_lossy().as_ref(), "-foo"]
        );
        let vd = |via| LaunchProfile {
            runtime: Runtime::VirtualDesktop,
            vd_via: via,
            extra_args: "-foo".into(),
            ..Default::default()
        };
        assert!(build(&vd(VdVia::Meta), exe, &Tools::default(), None).is_err());
        let c = build(&vd(VdVia::Meta), exe, &vd_tools, Some(LOBBY)).unwrap();
        assert_eq!(c.program, streamer);
        assert_eq!(
            c.args,
            [exe.to_string_lossy().as_ref(), "-lobbyid", LOBBY, "-foo"]
        );
        assert_eq!(c.cwd, bin);
        let c = build(&vd(VdVia::SteamVr), exe, &Tools::default(), Some(LOBBY)).unwrap();
        assert_eq!(c.program, bin.join(echoxr::LAUNCHER));
        assert_eq!(c.args, ["-lobbyid", LOBBY, "-foo"]);
        assert_eq!(c.cwd, bin);
        let c = build(&vd(VdVia::VdXr), exe, &Tools::default(), Some(LOBBY)).unwrap();
        assert_eq!(c.program, bin.join(echoxr::LAUNCHER));
        assert_eq!(c.args, ["--runtime", "active", "-lobbyid", LOBBY, "-foo"]);
        // Event builds: EchoXR.exe from the bin folder, only its own arguments; in the
        // Oculus mode VD's streamer, from the game folder.
        let win7 = Path::new("C:/E/ready-at-dawn-echo-arena/bin/win7");
        let c = build_relay(&vd(VdVia::SteamVr), old, &Tools::default()).unwrap();
        assert_eq!(c.program, win7.join(echoxr::LAUNCHER));
        assert!(c.args.is_empty());
        assert_eq!(c.cwd, win7);
        let c = build_relay(&vd(VdVia::VdXr), old, &Tools::default()).unwrap();
        assert_eq!(c.args, ["--runtime", "active"]);
        assert_eq!(c.cwd, win7);
        let c = build_relay(&vd(VdVia::Meta), old, &vd_tools).unwrap();
        assert_eq!(c.program, streamer);
        assert_eq!(c.args, [old.to_string_lossy().as_ref()]);
        assert_eq!(c.cwd, Path::new("C:/E/ready-at-dawn-echo-arena"));

        // SteamVR through EchoXR: EchoXR.exe beside the game, the game's arguments, no
        // Revive needed; event builds can't.
        p = LaunchProfile {
            runtime: Runtime::Revive,
            steamvr_via: SteamVrVia::EchoXr,
            windowed: true,
            extra_args: "-foo".into(),
            ..Default::default()
        };
        let c = build(&p, exe, &Tools::default(), Some(LOBBY)).unwrap();
        assert_eq!(
            c.program,
            Path::new("C:/E/ready-at-dawn-echo-arena/bin/win10/EchoXR.exe")
        );
        assert_eq!(c.args, ["-lobbyid", LOBBY, "-foo"]);
        assert_eq!(c.cwd, Path::new("C:/E/ready-at-dawn-echo-arena/bin/win10"));
        // An event build plays through EchoXR too, bare.
        assert!(build_relay(&p, old, &Tools::default())
            .unwrap()
            .args
            .is_empty());

        p = LaunchProfile {
            runtime: Runtime::Revive,
            ..Default::default()
        };
        assert!(build(&p, exe, &Tools::default(), None).is_err());
        // An event build: nothing but what Revive itself needs.
        let revive = Tools {
            revive_dir: Some("C:/Program Files/Revive"),
            ..Default::default()
        };
        let relay = build_relay(&p, exe, &revive).unwrap();
        assert!(!relay
            .args
            .iter()
            .any(|a| a == "-windowed" || a == "-lobbyid"));
        let c = build(&p, exe, &revive, None).unwrap();
        assert_eq!(
            c.program,
            Path::new("C:/Program Files/Revive/ReviveInjector.exe")
        );
        assert_eq!(c.args[0], exe.to_string_lossy());
        assert_eq!(
            c.args[c.args.len() - 2..],
            ["/app", "ready-at-dawn-echo-arena"]
        );
    }
}
