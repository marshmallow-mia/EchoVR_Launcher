//! Echo VR for PC on Linux: added to Steam as a non-Steam game that runs the launcher
//! (`--play`), which starts the game through a private GE-Proton with EchoXR's OpenXR
//! runtime standing in for Meta's.

pub mod echoxr;
pub mod steam;
pub mod vdf;

use std::path::{Path, PathBuf};

use crate::core::launcher::launch;
use crate::core::launcher::store::{LauncherState, Runtime, Target};

/// What Steam's shortcut runs the launcher with.
pub const PLAY_FLAG: &str = "--play";

/// Cancels a start PLAY made: ends the launcher Steam ran with `--play` (not this one)
/// and what runs in the game's Wine prefix. Steam then sees the shortcut end.
pub fn cancel_start() {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let me = std::process::id();
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::OnlyIfNotSet),
    );
    for (pid, p) in sys.processes() {
        let play = p.cmd().iter().any(|a| a.to_str() == Some(PLAY_FLAG));
        let ours = p.name().to_string_lossy().starts_with("EchoVR_Launche");
        if play && ours && pid.as_u32() != me && p.kill() {
            tracing::info!("cancelled the start (pid {pid})");
        }
    }
    echoxr::kill_prefix();
}

/// Gives xdg-open what it needs on KDE when the launcher was started without the
/// desktop's full environment (from a terminal over SSH, a systemd unit, some app
/// launchers): without `KDE_SESSION_VERSION` it falls back to KDE 3's `kfmclient`, which
/// no current system has, reports success anyway, and links open nowhere. Must run
/// before any thread starts. Returns what it changed, for the log.
pub fn prepare_desktop_env() -> Option<String> {
    if !cfg!(target_os = "linux") || std::env::var_os("KDE_SESSION_VERSION").is_some() {
        return None;
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if !desktop.split(':').any(|d| d.eq_ignore_ascii_case("KDE")) {
        return None;
    }
    let version = if on_path("kde-open") {
        "6"
    } else if on_path("kde-open5") {
        "5"
    } else {
        return None;
    };
    std::env::set_var("KDE_SESSION_VERSION", version);
    Some(format!(
        "KDE_SESSION_VERSION was not set; set it to {version} so links open through kde-open"
    ))
}

/// Logs what decides how links and folders open here (Linux only).
pub fn log_desktop_env(fix: Option<&str>) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| "-".into());
    tracing::info!(
        "desktop: XDG_CURRENT_DESKTOP={} XDG_SESSION_TYPE={} KDE_SESSION_VERSION={} BROWSER={} xdg-open={}",
        var("XDG_CURRENT_DESKTOP"),
        var("XDG_SESSION_TYPE"),
        var("KDE_SESSION_VERSION"),
        var("BROWSER"),
        if on_path("xdg-open") { "yes" } else { "missing" },
    );
    if let Some(fix) = fix {
        tracing::warn!("desktop: {fix}");
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(program).is_file()))
}

/// Where a lobby to join waits for the next `--play` (Steam's link can't carry it).
fn next_lobby_file() -> PathBuf {
    echoxr::root().join("next-lobby")
}

/// Has the next start through Steam join `lobby` (and watch it as a spectator), or not.
pub fn set_next_lobby(lobby: Option<(&str, bool)>) {
    let f = next_lobby_file();
    match lobby {
        Some((l, spectate)) => {
            let _ = std::fs::create_dir_all(echoxr::root());
            let _ = std::fs::write(
                f,
                if spectate {
                    format!("s:{l}")
                } else {
                    l.to_string()
                },
            );
        }
        None => {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// The game `--play` runs: its launcher process and the version, while it runs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Playing {
    pub pid: u32,
    pub version: String,
    /// Steam's `reaper` for this launch (`--play`'s parent). Wine's processes are orphaned
    /// as they start, so the game ends up under it, not under `--play`.
    #[serde(default)]
    pub reaper: Option<u32>,
}

/// `--play`'s parent when it is Steam's per-launch `reaper`.
#[cfg(not(unix))]
fn steam_reaper() -> Option<u32> {
    None
}

/// `--play`'s parent when it is Steam's per-launch `reaper`.
#[cfg(unix)]
fn steam_reaper() -> Option<u32> {
    let parent = std::os::unix::process::parent_id();
    let comm = std::fs::read_to_string(format!("/proc/{parent}/comm")).ok()?;
    (comm.trim() == "reaper").then_some(parent)
}

/// Where `--play` says what it runs (so the launcher knows the game for its own).
fn playing_file() -> PathBuf {
    echoxr::root().join("playing.json")
}

/// What `--play` runs right now, if anything (Linux only).
pub fn playing() -> Option<Playing> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(playing_file()).ok()?).ok()
}

/// What went wrong at `--play`'s last start, for the launcher's window to say once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub title: String,
    pub message: String,
}

fn outcome_file() -> PathBuf {
    echoxr::root().join("last-start.json")
}

/// What went wrong at the last start, once (Linux only): it is gone after this.
pub fn take_outcome() -> Option<Outcome> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let file = outcome_file();
    let text = std::fs::read_to_string(&file).ok()?;
    let _ = std::fs::remove_file(&file);
    serde_json::from_str(&text).ok()
}

/// What `--play` tells about a start that failed.
const STEAMVR_LOST_GPU: &str = "SteamVR's compositor lost the graphics card while Echo VR ran (VK_ERROR_DEVICE_LOST). SteamVR 2.17 has a known bug that does this with NVIDIA graphics on Linux: in Steam, open SteamVR's Properties, Betas, and choose \"previous\" (SteamVR 2.16.7).";

/// SteamVR's compositor log where it was at a start: whether the compositor lost the GPU
/// since (`vkerror=-4`). A compositor that restarted moved the log to
/// `vrcompositor.previous.txt`, and its successor began a new one.
struct CompositorLog {
    dir: PathBuf,
    since: std::time::SystemTime,
    file: Option<(u64, u64)>,
}

impl CompositorLog {
    fn mark(steam_root: &Path) -> CompositorLog {
        let dir = steam_root.join("logs");
        let file = std::fs::metadata(dir.join("vrcompositor.txt"))
            .ok()
            .map(|m| (file_id(&m), m.len()));
        CompositorLog {
            dir,
            since: std::time::SystemTime::now(),
            file,
        }
    }

    fn lost_gpu(&self) -> bool {
        ["vrcompositor.txt", "vrcompositor.previous.txt"]
            .iter()
            .any(|name| {
                let path = self.dir.join(name);
                let Ok(meta) = std::fs::metadata(&path) else {
                    return false;
                };
                if meta.modified().is_ok_and(|t| t < self.since) {
                    return false;
                }
                let from = match self.file {
                    Some((id, len)) if id == file_id(&meta) => len,
                    _ => 0,
                };
                let bytes = std::fs::read(&path).unwrap_or_default();
                let start = usize::try_from(from).unwrap_or(0).min(bytes.len());
                compositor_lost_gpu(&String::from_utf8_lossy(&bytes[start..]))
            })
    }
}

#[cfg(unix)]
fn file_id(m: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(m)
}

#[cfg(not(unix))]
fn file_id(_: &std::fs::Metadata) -> u64 {
    0
}

/// Pure: whether this part of SteamVR's compositor log says it lost the GPU.
fn compositor_lost_gpu(log: &str) -> bool {
    log.contains("vkerror=-4")
        || log.contains("Unexpected failure when checking for timeline signal")
}

/// The lobby left for this start, and whether to watch it.
fn take_next_lobby() -> Option<(String, bool)> {
    let f = next_lobby_file();
    let text = std::fs::read_to_string(&f).ok();
    let _ = std::fs::remove_file(f);
    let text = text?;
    let (spectate, id) = match text.strip_prefix("s:") {
        Some(id) => (true, id),
        None => (false, text.as_str()),
    };
    launch::lobby_uuid(id).map(|l| (l, spectate))
}

/// The launcher's own executable, as Steam's shortcut should run it (the AppImage when
/// it runs from one).
pub fn launcher_exe() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
}

/// Whether Steam started this launcher for its Echo VR shortcut (Steam's `SteamGameId`
/// is the shortcut's game id): PLAY's start, even when Steam has lost the shortcut's
/// `--play`, as it does when it saves its own copy of the shortcuts at a start.
pub fn started_for_shortcut() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Some(appid) = LauncherState::load().linux_appid else {
        return false;
    };
    let ids: Vec<String> = ["SteamGameId", "SteamOverlayGameId", "SteamAppId"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .collect();
    is_our_shortcut(&ids, appid)
}

/// Pure: whether one of Steam's ids (`SteamGameId` and the like) is shortcut `appid`'s.
fn is_our_shortcut(ids: &[String], appid: u32) -> bool {
    let game = steam::game_id(appid);
    ids.iter()
        .filter_map(|v| v.trim().parse::<u64>().ok())
        .any(|v| v == game || v == u64::from(appid))
}

/// A silent microphone for one game run, on a PC without any: Echo VR (with nEVR)
/// crashes at its start when no capture device is there. A null sink and a source made
/// of its monitor (PipeWire's pulse server, or PulseAudio), unloaded when dropped.
struct Microphone(Vec<String>);

impl Microphone {
    fn ensure() -> Option<Microphone> {
        let list = std::process::Command::new("pactl")
            .args(["list", "short", "sources"])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        if has_microphone(&String::from_utf8_lossy(&list.stdout)) {
            return None;
        }
        let load = |args: &[&str]| -> Option<String> {
            let o = std::process::Command::new("pactl")
                .arg("load-module")
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())?;
            Some(String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|id| !id.is_empty())
        };
        let sink = load(&[
            "module-null-sink",
            "sink_name=echovr_mic_sink",
            "sink_properties=device.description=EchoVR-Silence",
        ])?;
        let mut modules = vec![sink];
        if let Some(source) = load(&[
            "module-remap-source",
            "master=echovr_mic_sink.monitor",
            "source_name=echovr_mic",
            "source_properties=device.description=EchoVR-Microphone",
        ]) {
            modules.push(source);
        }
        tracing::info!("--play: no microphone here; a silent one for this run ({modules:?})");
        Some(Microphone(modules))
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        for id in self.0.iter().rev() {
            let _ = std::process::Command::new("pactl")
                .args(["unload-module", id])
                .status();
        }
    }
}

/// Pure: whether `pactl list short sources` lists a capture device (not just the
/// monitors of outputs).
fn has_microphone(list: &str) -> bool {
    list.lines()
        .filter_map(|l| l.split('\t').nth(1))
        .any(|name| !name.is_empty() && !name.ends_with(".monitor"))
}

/// `--play`: starts the version PLAY would start, with the launch options, through Proton
/// and EchoXR, and waits for the game to end. Returns the exit code.
pub fn play_from_steam() -> i32 {
    let state = LauncherState::load();
    let Target::Installed(v) = state.target(None, |v| v.present()) else {
        tracing::error!("--play: no installed version to start");
        return 2;
    };
    let Some(steam_root) = steam::root() else {
        tracing::error!("--play: Steam not found");
        return 2;
    };
    let lobby = take_next_lobby();
    let mut profile = state.profile.clone();
    if matches!(profile.runtime, Runtime::MetaLink | Runtime::VirtualDesktop) {
        // Linux's "VR" before there was a choice: SteamVR (the window asks again).
        profile.runtime = Runtime::Revive;
    }
    if lobby.as_ref().is_some_and(|(_, spectate)| *spectate) {
        profile.runtime = crate::core::launcher::store::Runtime::Flat;
        profile.spectator = true;
    }
    // Event builds start bare (the 2019 one quits on flags it doesn't know).
    let args = if v.publisher_lock.is_some() {
        Vec::new()
    } else {
        launch::game_args(&profile, lobby.as_ref().map(|(l, _)| l.as_str()))
    };
    let event = v.publisher_lock.is_some();
    // An event build: in VR always (it can't sign in on the monitor), with EchoLoader and
    // EchoRelay's patch in its folder (PLAY in the launcher's window set it up already; a
    // start from Steam alone may come first).
    if event {
        if profile.runtime == crate::core::launcher::store::Runtime::Flat {
            tracing::error!("--play: event builds always start in VR");
            return 2;
        }
        let cancel = std::sync::atomic::AtomicBool::new(false);
        if let Err(e) = crate::core::launcher::relay::set_up(&v, &cancel, &mut |_| {}) {
            tracing::error!("--play: {} not set up for the classic lobbies: {e:#}", v.id);
            return 2;
        }
    }
    // Started from Steam without the launcher's window too: nEVR's plugin list (the live
    // build's; EchoXR Hands only with it).
    let hands = !event && profile.hands(state.echoxr_hands);
    if !event {
        if let Err(e) = crate::core::launcher::mods::before_start(&v, state.own_game_config, hands)
        {
            tracing::warn!("--play: mods not prepared: {e:#}");
        }
    }
    let playing = Playing {
        pid: std::process::id(),
        version: v.id.clone(),
        reaper: steam_reaper(),
    };
    if let Ok(json) = serde_json::to_vec(&playing) {
        let _ = std::fs::write(playing_file(), json);
    }
    let flat = profile.runtime == crate::core::launcher::store::Runtime::Flat;
    // Flat with nEVR: -windowed (no headset, in a window) instead of -noovr, so the game
    // signs in as your account rather than as a demo player. Spectating stays -noovr
    // -spectatorstream.
    let oculus = flat && !profile.spectator && crate::core::launcher::mods::nevr_in(&v.bin_dir());
    let mut args = args;
    let start = if !flat {
        echoxr::Start::Vr(echoxr::Xr::of(profile.runtime))
    } else if oculus {
        args.retain(|a| !a.eq_ignore_ascii_case("-noovr"));
        if !args.iter().any(|a| a.eq_ignore_ascii_case("-windowed")) {
            args.insert(0, "-windowed".into());
        }
        echoxr::Start::FlatOculus
    } else {
        echoxr::Start::Flat
    };
    tracing::info!("--play: {} {start:?}", v.id);
    // Echo VR crashes at its start without any microphone: a silent one for this run.
    let mic = Microphone::ensure();
    let _ = std::fs::remove_file(outcome_file());
    let vr = matches!(start, echoxr::Start::Vr(_));
    let compositor = matches!(start, echoxr::Start::Vr(echoxr::Xr::SteamVr))
        .then(|| CompositorLog::mark(&steam_root));
    // A Low preset the failed GPU rating saved: rated again this once, now that
    // NvrGpuRating answers (only with it in place, else the game picks Low again).
    let rerate = v
        .bin_dir()
        .join("plugins")
        .join(crate::core::launcher::graphics::PLUGIN)
        .is_file()
        .then(|| {
            crate::core::launcher::graphics::begin(
                &echoxr::local_app_data().join("rad/loneecho"),
                crate::core::launcher::graphics::settings_file(&v),
            )
        })
        .flatten();
    // EchoXR Hands: its OpenXR layer in the game's loader (in VR only).
    let result = echoxr::game_command(&steam_root, &v.bin_dir(), v.exe_name(), &args, start, hands)
        .and_then(|mut c| {
            tracing::info!("--play: {c:?}");
            // Proton's and the game's own output (OpenXR's warnings among it), for this run.
            if let Ok(out) = std::fs::File::create(crate::core::paths::log_dir().join("proton.log"))
            {
                if let Ok(err) = out.try_clone() {
                    c.stdout(out).stderr(err);
                }
            }
            Ok(c.status()?)
        });
    drop(mic);
    if let Some(r) = rerate {
        if let Err(e) = r.finish() {
            tracing::warn!("--play: graphics re-rate: {e:#}");
        }
    }
    let _ = std::fs::remove_file(playing_file());
    match result {
        Ok(status) => {
            let code = status.code().unwrap_or(0);
            // In VR the code is EchoXR's when it stopped the start, else Echo VR's own.
            let failed = vr
                .then(|| crate::core::echoxr::failure(&v.bin_dir(), code))
                .flatten();
            let mut outcome = None;
            if let Some(why) = failed {
                tracing::error!("--play: EchoXR ({code}): {why}");
                outcome = Some(Outcome {
                    title: "Echo VR didn't start through EchoXR".into(),
                    message: why.into(),
                });
            } else {
                tracing::info!("--play: Echo VR ended (exit code {code})");
            }
            if compositor.is_some_and(|c| c.lost_gpu()) {
                tracing::warn!("--play: {STEAMVR_LOST_GPU}");
                outcome = Some(Outcome {
                    title: "SteamVR lost the graphics card".into(),
                    message: match outcome {
                        Some(o) => format!("{}\n\n{STEAMVR_LOST_GPU}", o.message),
                        None => STEAMVR_LOST_GPU.into(),
                    },
                });
            }
            if let Some(json) = outcome.and_then(|o| serde_json::to_vec(&o).ok()) {
                let _ = std::fs::write(outcome_file(), json);
            }
            code
        }
        Err(e) => {
            tracing::error!("--play: {e:#}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_steamvr_lose_the_gpu_after_a_start() {
        let lost = "[Error] - Failed GetDeltas(Compositor): vkerror=-4\n";
        assert!(compositor_lost_gpu(lost));
        assert!(compositor_lost_gpu(
            "ASSERT: \"Unexpected failure when checking for timeline signal\" at vksync.cpp:213"
        ));
        assert!(!compositor_lost_gpu(
            "[Error] - No Vulkan command buffer open in CGpuTiming::MarkEvent!"
        ));

        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let log = logs.join("vrcompositor.txt");
        // A loss before this start doesn't count.
        std::fs::write(&log, lost).unwrap();
        let mark = CompositorLog::mark(dir.path());
        assert!(!mark.lost_gpu());
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut f, b"[Info] - frame\n").unwrap();
        drop(f);
        assert!(!mark.lost_gpu());
        // One during it does, also when the compositor restarted and moved its log.
        std::fs::rename(&log, logs.join("vrcompositor.previous.txt")).unwrap();
        std::fs::write(&log, format!("[Info] - restarted\n{lost}")).unwrap();
        assert!(mark.lost_gpu());
    }

    #[test]
    fn knows_its_shortcut() {
        let appid = 4_071_750_601;
        let game = steam::game_id(appid).to_string();
        assert!(is_our_shortcut(&[game], appid));
        assert!(is_our_shortcut(&["0".into(), appid.to_string()], appid));
        assert!(!is_our_shortcut(&["0".into(), "250820".into()], appid));
        assert!(!is_our_shortcut(&[], appid));
    }

    #[test]
    fn sees_a_microphone() {
        assert!(!has_microphone(""));
        assert!(!has_microphone(
            "57\talsa_output.pci.analog-stereo.monitor\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n"
        ));
        assert!(has_microphone("57\talsa_output.monitor\tPipeWire\tx\tIDLE\n58\talsa_input.usb-mic\tPipeWire\tx\tIDLE\n"));
    }
}
