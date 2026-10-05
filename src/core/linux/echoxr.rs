//! What Echo VR needs on Linux, the way EchoXR does it (github.com/EchoTools/EchoXR, its
//! OpenXR layer only): a private GE-Proton, whose `wineopenxr` passes OpenXR on to the
//! runtime you play with (SteamVR or WiVRn, `Xr`); a Wine prefix; and EchoXR
//! (`core::echoxr`) in the game's `bin/win10`, with Meta's Platform SDK loader beside the
//! game (a prefix has no Meta app). Everything lives in `<data dir>/linux`; every
//! download is pinned by its SHA-256.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::core::echoxr;
use crate::core::launcher::store::Runtime;
use crate::core::launcher::versions::Step;
use crate::core::paths;

/// GE-Proton, pinned; it carries `wineopenxr`.
pub const PROTON: &str = "GE-Proton11-3";
const PROTON_URL: &str = "https://github.com/GloriousEggroll/proton-ge-custom/releases/download/GE-Proton11-3/GE-Proton11-3.tar.gz";
const PROTON_SHA256: &str = "861c2edc8d40d051fb1e7a692deb953be52bd339c46d90f2b7dde50ddad91266";

const MARKER: &str = "setup.json";
/// Which of Meta's DLLs the setup brings (3: the Platform SDK loader and its P2P library;
/// an older setup fetches what's new).
const PLATFORM_REV: u32 = 3;

/// Where everything goes (EchoXR's zip and Meta's loader too).
pub fn root() -> PathBuf {
    echoxr::store_dir()
}

fn proton_dir() -> PathBuf {
    root().join(PROTON)
}

fn compat_dir() -> PathBuf {
    root().join("compatdata")
}

fn prefix() -> PathBuf {
    compat_dir().join("pfx")
}

/// The game's `%LOCALAPPDATA%` in the prefix (Proton's user is `steamuser`).
pub fn local_app_data() -> PathBuf {
    prefix().join("drive_c/users/steamuser/AppData/Local")
}

/// What this launcher's setup consists of: a different one means setting up again.
fn marker_text() -> String {
    format!(
        "{{\"proton\":\"{PROTON}\",\"echoxr\":\"{}\",\"platform\":{PLATFORM_REV}}}",
        echoxr::VERSION
    )
}

/// Whether the setup is done (and still all there).
pub fn is_set_up() -> bool {
    std::fs::read_to_string(root().join(MARKER)).is_ok_and(|m| m == marker_text())
        && proton_dir().join("proton").is_file()
        && echoxr::is_fetched()
}

/// The OpenXR runtime Echo VR plays with on Linux, through EchoXR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Xr {
    SteamVr,
    Wivrn,
}

impl Xr {
    /// The runtime a launch profile's choice means: WiVRn's tile, else SteamVR (what
    /// Linux's single "VR" choice was before).
    pub fn of(runtime: Runtime) -> Xr {
        if runtime == Runtime::Wivrn {
            Xr::Wivrn
        } else {
            Xr::SteamVr
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Xr::SteamVr => "SteamVR",
            Xr::Wivrn => "WiVRn",
        }
    }

    /// Its OpenXR manifest, where it's installed.
    pub fn manifest(self) -> Option<PathBuf> {
        let home = dirs::home_dir().unwrap_or_default();
        let candidates: Vec<PathBuf> = match self {
            Xr::SteamVr => {
                let mut roots: Vec<PathBuf> = super::steam::root().into_iter().collect();
                roots.extend([
                    home.join(".local/share/Steam"),
                    home.join(".steam/steam"),
                    home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
                ]);
                roots
                    .into_iter()
                    .map(|r| r.join("steamapps/common/SteamVR/steamxr_linux64.json"))
                    .collect()
            }
            Xr::Wivrn => {
                let flatpak = "app/io.github.wivrn.wivrn/current/active/files/share";
                let mut shares: Vec<PathBuf> = std::env::var("XDG_DATA_DIRS")
                    .unwrap_or_default()
                    .split(':')
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect();
                shares.extend([
                    PathBuf::from("/usr/share"),
                    PathBuf::from("/usr/local/share"),
                    home.join(".local/share/flatpak").join(flatpak),
                    PathBuf::from("/var/lib/flatpak").join(flatpak),
                ]);
                shares
                    .into_iter()
                    .map(|s| s.join("openxr/1/openxr_wivrn.json"))
                    .collect()
            }
        };
        candidates.into_iter().find(|p| p.is_file())
    }

    /// Whether its service runs: SteamVR's `vrserver`, or WiVRn's socket. EchoXR is told
    /// so; without it, it gives up after 20 s (exit code 5) rather than hanging.
    fn running(self) -> bool {
        let xdg = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        service_in(
            self,
            xdg.as_deref(),
            crate::core::launcher::game::program_running("vrserver"),
        )
    }

    /// Starts its service when it isn't running, and waits for it a while (SteamVR
    /// takes its time). Returns whether it runs.
    fn start(self) -> bool {
        if self.running() {
            return true;
        }
        let quiet = |c: &mut std::process::Command| {
            c.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        };
        let started = match self {
            Xr::SteamVr => super::steam::root().is_some_and(|root| {
                super::steam::open(&root, "steam://run/250820")
                    .inspect_err(|e| tracing::warn!("--play: SteamVR: {e:#}"))
                    .is_ok()
            }),
            Xr::Wivrn => {
                let mut c = std::process::Command::new("wivrn-server");
                quiet(&mut c);
                c.spawn().is_ok() || {
                    let mut c = std::process::Command::new("flatpak");
                    c.args(["run", "--command=wivrn-server", "io.github.wivrn.wivrn"]);
                    quiet(&mut c);
                    c.spawn().is_ok()
                }
            }
        };
        if !started {
            tracing::warn!("--play: couldn't start {}", self.name());
            return false;
        }
        tracing::info!("--play: started {}, waiting for it", self.name());
        let wait = match self {
            Xr::SteamVr => Duration::from_secs(60),
            Xr::Wivrn => Duration::from_secs(15),
        };
        let since = Instant::now();
        while since.elapsed() < wait {
            std::thread::sleep(Duration::from_millis(500));
            if self.running() {
                return true;
            }
        }
        false
    }
}

/// The runtimes installed here, SteamVR first. Looked up at most every 10 seconds (the
/// Settings page asks every frame).
pub fn installed() -> Vec<Xr> {
    static SEEN: std::sync::Mutex<Option<(Instant, Vec<Xr>)>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, xr)) = seen
        .as_ref()
        .filter(|(at, _)| at.elapsed() < Duration::from_secs(10))
    {
        let _ = at;
        return xr.clone();
    }
    let xr: Vec<Xr> = [Xr::SteamVr, Xr::Wivrn]
        .into_iter()
        .filter(|x| x.manifest().is_some())
        .collect();
    *seen = Some((Instant::now(), xr.clone()));
    xr
}

/// The OpenXR runtime the game is to use: `XR_RUNTIME_JSON` when set (by hand), else the
/// chosen one's manifest.
pub fn openxr_runtime(xr: Xr) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("XR_RUNTIME_JSON").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    xr.manifest()
}

/// Pure but for the file checks: whether `xr`'s service runs, given the runtime folder
/// and whether `vrserver` runs.
fn service_in(xr: Xr, xdg_runtime_dir: Option<&Path>, vrserver: bool) -> bool {
    match xr {
        Xr::SteamVr => vrserver,
        Xr::Wivrn => xdg_runtime_dir.is_some_and(|d| d.join("wivrn/comp_ipc").exists()),
    }
}

/// The environment Proton runs with (for setup steps and the game).
fn proton_env(steam_root: &Path, game_dir: Option<&Path>) -> Vec<(String, String)> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let mut env = vec![
        ("STEAM_COMPAT_DATA_PATH".into(), s(&compat_dir())),
        ("STEAM_COMPAT_CLIENT_INSTALL_PATH".into(), s(steam_root)),
        (
            "STEAM_COMPAT_LIBRARY_PATHS".into(),
            s(&steam_root.join("steamapps")),
        ),
        ("SteamAppId".into(), "0".into()),
        ("SteamGameId".into(), "0".into()),
        ("UMU_ID".into(), "umu-default".into()),
        ("UMU_USE_STEAM".into(), "0".into()),
        ("PROTON_LOG".into(), "0".into()),
        // Quiet, but for OpenXR's warnings and errors: they say why VR didn't start.
        ("WINEDEBUG".into(), "-all,err+openxr,warn+openxr".into()),
    ];
    if let Some(dir) = game_dir {
        env.push(("STEAM_COMPAT_INSTALL_PATH".into(), s(dir)));
    }
    env
}

/// Runs `proton <verb> <args>` and waits. Its output is kept: the last lines go to the
/// log when it fails.
fn proton(steam_root: &Path, verb: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(proton_dir().join("proton"))
        .arg(verb)
        .args(args)
        .envs(proton_env(steam_root, None))
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("couldn't run Proton ({verb})"))?;
    if !out.status.success() {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(20)..].join("\n");
        tracing::error!("Proton {verb} {args:?} failed ({}):\n{tail}", out.status);
        bail!(
            "Proton {verb} {} failed ({})",
            args.first().unwrap_or(&""),
            out.status
        );
    }
    Ok(())
}

/// Sets everything up (each step skipped when already done): GE-Proton, EchoXR, Meta's
/// Platform SDK loader, and the prefix.
pub fn setup(steam_root: &Path, cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<()> {
    std::fs::create_dir_all(root())?;
    let _ = std::fs::remove_file(root().join(MARKER));

    if !proton_dir().join("proton").is_file() {
        on(Step::Status(format!("Downloading {PROTON}...")));
        let archive = echoxr::fetch_pinned(
            PROTON_URL,
            &format!("{PROTON}.tar.gz"),
            PROTON_SHA256,
            cancel,
            on,
        )?;
        on(Step::Status(format!("Unpacking {PROTON}...")));
        let gz = flate2::read::GzDecoder::new(std::fs::File::open(&archive)?);
        tar::Archive::new(gz)
            .unpack(root())
            .with_context(|| format!("unpack {PROTON}"))?;
        let _ = std::fs::remove_file(&archive);
    }
    if !proton_dir()
        .join("files/lib/wine/x86_64-windows/wineopenxr.dll")
        .is_file()
    {
        bail!("{PROTON} has no OpenXR bridge (wineopenxr), which EchoXR needs");
    }

    echoxr::fetch(cancel, on)?;

    on(Step::Status("Preparing the Wine prefix...".into()));
    std::fs::create_dir_all(compat_dir())?;
    // `run`, not `runinprefix`: only `run` creates the prefix, and GE-Proton's own
    // drive setup fails without one (no pfx/dosdevices yet).
    proton(steam_root, "run", &["cmd.exe", "/c", "exit"])?;
    // Let the prefix settle before the game first uses it.
    let _ = std::process::Command::new(proton_dir().join("files/bin/wineserver"))
        .args(["-k", "-w"])
        .env("WINEPREFIX", prefix())
        .status();
    std::fs::write(root().join(MARKER), marker_text())?;
    Ok(())
}

/// Ends everything running in the game's Wine prefix (a start STOP cancels: Proton, the
/// game, EchoXR), without waiting.
pub fn kill_prefix() {
    let wineserver = proton_dir().join("files/bin/wineserver");
    if !wineserver.is_file() || !prefix().is_dir() {
        return;
    }
    match std::process::Command::new(wineserver)
        .arg("-k")
        .env("WINEPREFIX", prefix())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => tracing::info!("stopped what runs in the Wine prefix"),
        Err(e) => tracing::warn!("wineserver -k: {e}"),
    }
}

/// How `--play` starts the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// In the headset, through `EchoXR.exe` and this OpenXR runtime.
    Vr(Xr),
    /// On the monitor with `-noovr`: the demo platform (a `DMO-` login, linked by code).
    Flat,
    /// On the monitor with EchoRelay's `-windowed`, logged in through the Oculus platform
    /// (the licence patch's identity). Meta's platform needs its service, which a prefix
    /// lacks, so its loader is swapped for a stand-in that logs in offline.
    FlatOculus,
}

/// The stand-in for Meta's `LibOVRPlatform64_1.dll` (NoOvrEchoVR_on_Linux's
/// libovrplatform, MIT): an offline logged-in user, and the mic through WASAPI.
#[cfg(target_os = "linux")]
const PLATFORM_STUB: &[u8] =
    include_bytes!("../../../assets/linux/libovrplatform/LibOVRPlatform64_1.dll");
#[cfg(not(target_os = "linux"))]
const PLATFORM_STUB: &[u8] = &[];
const PLATFORM_DLL: &str = "LibOVRPlatform64_1.dll";

/// Pure: `unix` as a Windows path in a prefix whose drives map to `drives` (letter,
/// Unix folder): the most specific drive wins, as in Wine; `Z:` otherwise.
pub fn wine_path(drives: &[(char, PathBuf)], unix: &Path) -> String {
    let (letter, rest) = drives
        .iter()
        .filter_map(|(l, root)| unix.strip_prefix(root).ok().map(|r| (*l, root, r)))
        .max_by_key(|(_, root, _)| root.as_os_str().len())
        .map(|(l, _, r)| (l, r.to_path_buf()))
        .unwrap_or(('z', unix.strip_prefix("/").unwrap_or(unix).to_path_buf()));
    let rest = rest.to_string_lossy().replace('/', "\\");
    format!("{}:\\{rest}", letter.to_ascii_uppercase())
}

/// The prefix's drives (`dosdevices`), as Wine has them.
fn drives() -> Vec<(char, PathBuf)> {
    let dir = prefix().join("dosdevices");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let mut chars = name.chars();
            let (letter, colon) = (chars.next()?, chars.next()?);
            if colon != ':' || chars.next().is_some() {
                return None;
            }
            let target = std::fs::canonicalize(e.path()).ok()?;
            Some((letter, target))
        })
        .collect()
}

/// The command that starts Echo VR from its bin folder `bin` through Proton, with `args`
/// for the game, the way `start` says (see [`Start`]). In VR, `EchoXR.exe` makes
/// `echovr_openxr.exe` on its first run, turns Proton's OpenXR on itself (no OpenVR
/// runtime needed) and points Echo at the runtime.
pub fn game_command(
    steam_root: &Path,
    bin: &Path,
    args: &[String],
    start: Start,
    hands: bool,
) -> Result<std::process::Command> {
    let game_root = bin
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == paths::ARENA_DIR))
        .unwrap_or(bin)
        .to_path_buf();
    let xr = match start {
        Start::Vr(xr) => {
            let manifest = openxr_runtime(xr).with_context(|| {
                format!(
                    "{} isn't installed on this PC. Install it, or choose the other VR runtime in Settings.",
                    xr.name()
                )
            })?;
            Some((xr, manifest))
        }
        _ => None,
    };
    if start == Start::FlatOculus {
        // The stand-in in place of Meta's loader (put back by the next start in VR).
        let target = bin.join(PLATFORM_DLL);
        if std::fs::read(&target).ok().as_deref() != Some(PLATFORM_STUB) {
            std::fs::write(&target, PLATFORM_STUB)
                .with_context(|| format!("couldn't write {}", target.display()))?;
        }
    } else {
        // Meta's loader and P2P library beside the game: a prefix has no Meta app to
        // bring them.
        echoxr::install_into(bin, Some(bin))?;
        echoxr::refresh_openxr_exe(bin)?;
    }
    let mut rw = format!("{}:{}", game_root.display(), prefix().display());
    if let Ok(more) = std::env::var("PRESSURE_VESSEL_FILESYSTEMS_RW") {
        rw = format!("{rw}:{more}");
    }
    let program = if xr.is_some() {
        bin.join(echoxr::LAUNCHER)
    } else {
        bin.join(paths::DEFAULT_EXE)
    };
    let mut c = std::process::Command::new(proton_dir().join("proton"));
    c.arg("waitforexitandrun")
        .arg(program)
        .args(args)
        .current_dir(bin)
        .envs(proton_env(steam_root, Some(&game_root)))
        .env("PRESSURE_VESSEL_FILESYSTEMS_RW", rw);
    if start == Start::FlatOculus {
        // pnsovr checks that the loaded LibOVRPlatform64_1.dll is in this folder, written
        // as the game sees it (else: "Failed to initialize the Oculus VR Platform SDK").
        let canonical = std::fs::canonicalize(bin).unwrap_or_else(|_| bin.to_path_buf());
        let dir = format!("{}\\", wine_path(&drives(), &canonical));
        tracing::info!("--play: LIBOVR_DLL_DIR={dir}");
        c.env("LIBOVR_DLL_DIR", dir);
    }
    // nEVR is the game's BugSplat64.dll, which Wine has no builtin of: the game's own
    // folder wins without an override.
    if let Some((xr, manifest)) = xr {
        tracing::info!("--play: {} at {}", xr.name(), manifest.display());
        c.env("XR_RUNTIME_JSON", manifest)
            .env("PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES", "1");
        if xr.start() {
            c.env("ECHOXR_VR_SERVICE", "ready");
        } else {
            tracing::warn!("--play: {} isn't running", xr.name());
        }
        if hands {
            // EchoXR Hands' OpenXR layer, in the game's loader (paths as the game sees them).
            let drives = drives();
            let env = crate::core::echoxr_hands::layer_env(bin, |p| {
                let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
                wine_path(&drives, &p)
            });
            tracing::info!("--play: hand tracking layer {env:?}");
            c.envs(env);
        }
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_paths_as_wine_shows_them() {
        let drives = vec![
            ('c', PathBuf::from("/p/drive_c")),
            ('x', PathBuf::from("/home/mia")),
            ('z', PathBuf::from("/")),
        ];
        let bin = Path::new("/home/mia/.local/share/E/bin/win10");
        assert_eq!(wine_path(&drives, bin), r"X:\.local\share\E\bin\win10");
        assert_eq!(wine_path(&drives, Path::new("/opt/echo")), r"Z:\opt\echo");
        assert_eq!(wine_path(&[], Path::new("/opt/echo")), r"Z:\opt\echo");
    }

    #[test]
    fn sees_the_vr_service() {
        let dir = tempfile::tempdir().unwrap();
        assert!(service_in(Xr::SteamVr, None, true));
        assert!(!service_in(Xr::SteamVr, Some(dir.path()), false));
        assert!(!service_in(Xr::Wivrn, Some(dir.path()), true));
        std::fs::create_dir_all(dir.path().join("wivrn")).unwrap();
        std::fs::write(dir.path().join("wivrn/comp_ipc"), b"").unwrap();
        assert!(service_in(Xr::Wivrn, Some(dir.path()), false));
        assert!(!service_in(Xr::SteamVr, Some(dir.path()), false));
        assert_eq!(Xr::of(Runtime::Wivrn), Xr::Wivrn);
        assert_eq!(Xr::of(Runtime::MetaLink), Xr::SteamVr);
    }
}
