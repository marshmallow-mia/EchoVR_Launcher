//! EchoXR (github.com/marshmallow-mia/EchoXR, its OpenXR layer only): `EchoXR.exe` in the game's
//! `bin/win10` starts the game as `echovr_openxr.exe` (its patched copy of `echovr.exe`,
//! made when missing) with `EchoXR\LibOVRRT64_1.dll` answering Echo's LibOVR calls over
//! OpenXR: no Meta services, sign-in or store, and no injection. On Windows it plays on
//! SteamVR (the SteamVR choice "through EchoXR", and Virtual Desktop through SteamVR) or on
//! Virtual Desktop's own OpenXR runtime (`--runtime active`); on Linux through GE-Proton
//! (`linux`).
//!
//! The game's `pnsovr.dll` signs in through Oculus' Platform SDK (`LibOVRPlatform64_1.dll`).
//! Nothing of Meta's: EchoXR brings its own in `EchoXR\` (marshmallow-mia's stand-in: a
//! signed-in user with a per-machine id, no Oculus service), which `EchoXR.exe` puts first
//! on `PATH` and in `LIBOVR_DLL_DIR`. Meta's loader and P2P library, which older launchers
//! read out of Meta's runtime package, are taken out where they put them. The download is
//! pinned by its SHA-256. See docs/launcher/echoxr.md.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};

use crate::core::download::{self, Progress};
use crate::core::launcher::store::{LaunchProfile, Runtime, VdVia};
use crate::core::launcher::versions::Step;
use crate::core::paths;

/// One EchoXR build: its GitHub release zip, pinned by its SHA-256, and the files in it that
/// do the work, with theirs.
pub struct Build {
    pub version: &'static str,
    zip: &'static str,
    url: &'static str,
    sha256: &'static str,
    files: [(&'static str, &'static str); 4],
    /// Whether its `EchoXR.exe` takes `--verbose` (0.6.3 on); an older one would hand it on
    /// to the game, and the event builds quit on an option they don't know.
    verbose: bool,
}

/// EchoXR's OpenXR layer (marshmallow-mia/EchoXR 0.6.4, its GitHub release): unpacked into the
/// game's bin folder (`bin/win10`, or an event build's `bin/win7`). Built with the static C
/// runtime, so it needs no Visual C++ runtime. 0.6.4 stops SteamVR's D3D12 swapchain copy from
/// hanging the GPU (DXGI_ERROR_DEVICE_HUNG while other players' cosmetics loaded): each release
/// waits for the previous one's copy, and the barriers use Echo VR's real image states; 0.6.3
/// logs what the GPU was doing (`--verbose`); 0.6.1 waits until SteamVR hands back a swapchain
/// image; 0.6.0 reads the controller settings in
/// `EchoXR.txt` (the Controllers plugin) and takes the hands from the grip pose; 0.5.0 runs
/// the five event builds; 0.4.2
/// brings the Platform SDK stand-in; 0.4.1 made it run under GE-Proton.
static PINNED: Build = Build {
    version: "0.6.4",
    zip: "EchoXR-OpenXR-v0.6.4.zip",
    url: "https://github.com/marshmallow-mia/EchoXR/releases/download/v0.6.4-rc1/EchoXR-OpenXR-v0.6.4.zip",
    sha256: "1a4de62836f433671f759c46464610a156eaf3560ae831aacf1ff3599be2247b",
    files: [
        (
            LAUNCHER,
            "d79c026595bdbfa01b202fdf48d47285677e498929e05042432e046ee9a8d207",
        ),
        (
            "EchoXR/LibOVRRT64_1.dll",
            "8a05ba9cf377e1643d37c417c3fe064c018b58a3af18344d6e1884544d3e9b7f",
        ),
        (
            "EchoXR/openxr_loader.dll",
            "5c8e622279f37d11c11dd5d62b728c9ec3344432362df7f5d09da70a32ab6b53",
        ),
        (
            PLATFORM_IN_ECHOXR,
            "1efa465ac3357a721c9610833972861b6eaa06650a6271faf134f4347ff97498",
        ),
    ],
    verbose: true,
};

/// Test builds a dev code can choose instead of [`PINNED`] (its `channel.json`'s `"echoxr"`,
/// [`dev_code::echoxr`](crate::core::launcher::dev_code::echoxr)). Only these: a code can't
/// bring any other build, and the administrator helper takes these too. 0.6.3 logs what the
/// GPU was doing when SteamVR's D3D12 path hangs (DXGI_ERROR_DEVICE_HUNG): video memory,
/// allocations, each queue's work, all of DRED, and with `--verbose max` (the code's
/// `"echoxr_verbose": "max"`) a line a second.
static TESTS: [Build; 1] = [Build {
    version: "0.6.3",
    zip: "EchoXR-OpenXR-v0.6.3.zip",
    url: "https://github.com/marshmallow-mia/EchoXR/releases/download/v0.6.3-rc1/EchoXR-OpenXR-v0.6.3.zip",
    sha256: "a0a3af70228fb81af6a2c3554ddb47f87f4bb32f4f717e83e9731226b40f070f",
    files: [
        (LAUNCHER, "1f4ad3cd8f2e2c253659d57b5977d2d32cf3bdeefe1db5d90fd711f051d1071f"),
        ("EchoXR/LibOVRRT64_1.dll", "f373450d2eca5e15b75ce3ce438930f92c7981eaff80025381d2d8e32c180079"),
        ("EchoXR/openxr_loader.dll", "41adfb2a41c7387822efe6a2b335eb6f3358d659f82c812dcabad712cc618d68"),
        (PLATFORM_IN_ECHOXR, "132c56651ab436b668ddb65cedf29a9356f38b8f3be760c1e9d19aef8d6356d5"),
    ],
    verbose: true,
}];

/// Pure: the build for a dev code's choice `wanted`: that test build, else the pinned one.
fn pick(wanted: Option<&str>) -> &'static Build {
    wanted
        .and_then(|v| TESTS.iter().find(|b| b.version == v))
        .unwrap_or(&PINNED)
}

/// The EchoXR build this launcher puts in place: a dev code's test build, else the pinned one.
pub fn build() -> &'static Build {
    pick(crate::core::launcher::dev_code::echoxr().as_deref())
}

/// Whether `version` is one of the test builds this launcher pins.
pub fn knows_test(version: &str) -> bool {
    TESTS.iter().any(|b| b.version == version)
}

/// [`build`]'s version.
pub fn version() -> &'static str {
    build().version
}

/// Pure: `EchoXR.exe`'s own arguments a dev code adds for build `b`: its `--verbose` level
/// (`verbose`), for a build that takes it.
fn args_for(b: &Build, verbose: Option<&str>) -> Vec<String> {
    match verbose {
        Some(level) if b.verbose => vec!["--verbose".into(), level.into()],
        _ => Vec::new(),
    }
}

/// `EchoXR.exe`'s own arguments from a dev code ([`dev_code::echoxr_verbose`]), for
/// [`build`] (reads the kept `channel.json`, no network).
///
/// [`dev_code::echoxr_verbose`]: crate::core::launcher::dev_code::echoxr_verbose
pub fn dev_args() -> Vec<String> {
    args_for(build(), crate::core::launcher::dev_code::echoxr_verbose())
}

/// Whether test build `version` logs everything with `--verbose max`.
pub fn test_logs_more(version: &str) -> bool {
    TESTS.iter().any(|b| b.version == version && b.verbose)
}

/// Every build the launcher knows.
fn builds() -> impl Iterator<Item = &'static Build> {
    std::iter::once(&PINNED).chain(TESTS.iter())
}

/// Who made it, as the Mods page credits it: EchoXR, and what it builds on.
pub const AUTHORS: &str =
    "heisthecat31, marshmallow-mia, Villagers654 (RiftLift), CrossVR (Revive)";
/// What starts the game, in its `bin/win10`.
pub const LAUNCHER: &str = "EchoXR.exe";
/// EchoXR's Platform SDK stand-in, as the zip has it.
pub const PLATFORM_IN_ECHOXR: &str = "EchoXR/LibOVRPlatform64_1.dll";
/// The Platform SDK loader's file name.
pub const PLATFORM_DLL: &str = "LibOVRPlatform64_1.dll";
/// What older launchers wrote as `EchoXR/echoxr.ini` (0.3.0's updater off). 0.4.0 has no
/// updater and no ini: this one goes.
const OLD_INI: &str =
    "# Kept by the Echo VR launcher, which updates EchoXR itself\r\nCheckForUpdates = 0\r\n";
/// EchoXR's folder in `bin/win10`: its runtime, and where its `PATH` points first.
pub const DIR: &str = "EchoXR";

/// Meta's loader and P2P library as older launchers put them in place (read out of
/// Meta's runtime package): taken out again where they are those very files.
const OLD_META: [(&str, &str); 2] = [
    (
        "LibOVRPlatform64_1.dll",
        "c8f99087f457bed5c0549da82d7b976d6687017c72ddc25277babadaab16001b",
    ),
    (
        "LibOVRP2P64_1.dll",
        "7e4c14c4902af052c21f549a0a0d9f12084592e8d778337b197738f35bc507a9",
    ),
];

/// Pure: what an exit code of `EchoXR.exe` means (`None`: Echo's own exit code).
pub fn exit_message(code: i32) -> Option<&'static str> {
    Some(match code {
        2 => "EchoXR.exe isn't next to echovr.exe in the game's bin\\win10 folder.",
        3 => "EchoXR's runtime files are missing from the game's folder: PLAY in the launcher puts them back.",
        4 => "EchoXR couldn't make echovr_openxr.exe, its copy of the game (see EchoXR\\launcher.log in the game's bin\\win10 folder).",
        5 => "No OpenXR runtime answered: start SteamVR (or Monado / WiVRn) and wake the headset first.",
        6 => "The OpenXR runtime has no headset: connect it and wake it up, then try again.",
        7 => "Echo VR couldn't be started (see EchoXR\\launcher.log in the game's bin\\win10 folder).",
        _ => return None,
    })
}

/// Pure: [`exit_message`] for a start with `profile`: through Virtual Desktop, what to
/// start and connect is VD's.
pub fn exit_message_for(code: i32, profile: &LaunchProfile) -> Option<&'static str> {
    let vd = profile.runtime == Runtime::VirtualDesktop;
    Some(match (code, profile.vd_via) {
        (5, VdVia::VdXr) if vd => "Virtual Desktop's OpenXR runtime didn't answer: start its streamer and connect from your headset first.",
        (5, _) if vd => "SteamVR didn't answer: connect from your headset in Virtual Desktop, start SteamVR, then try again.",
        (6, _) if vd => "Virtual Desktop has no headset yet: connect from your headset, then try again.",
        _ => return exit_message(code),
    })
}

/// Pure: Echo VR's own exit code in the last start EchoXR logged (`launcher.log`'s
/// "Echo exited with code N"). EchoXR passes the game's code on, and Echo's codes overlap
/// with EchoXR's own (2-7): a start that logged one ended with the game's.
pub fn game_exit_code(log: &str) -> Option<i64> {
    log.lines()
        .rev()
        .take_while(|l| !l.starts_with("===== "))
        .find_map(|l| {
            l.split_once("Echo exited with code ")?
                .1
                .trim()
                .parse()
                .ok()
        })
}

/// Why EchoXR (not Echo VR) ended the last start in `bin` with `code`, if it did.
pub fn failure(bin: &Path, code: i32) -> Option<&'static str> {
    let log = std::fs::read_to_string(bin.join(DIR).join("launcher.log")).unwrap_or_default();
    if game_exit_code(&log).is_some() {
        return None;
    }
    exit_message(code)
}

/// Where the launcher keeps EchoXR's zip and Meta's loader: on Linux with the rest of its
/// setup, elsewhere in a folder of its own.
pub fn store_dir() -> PathBuf {
    let name = if cfg!(target_os = "linux") {
        "linux"
    } else {
        "echoxr"
    };
    paths::data_dir().join(name)
}

fn zip_path() -> PathBuf {
    store_dir().join(build().zip)
}

/// Whether EchoXR has been fetched (and is still there).
pub fn is_fetched() -> bool {
    zip_path().is_file()
}

/// Downloads `url` into the cache as `name` and checks it against `sha256`.
pub fn fetch_pinned(
    url: &str,
    name: &str,
    sha256: &str,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<PathBuf> {
    let dir = paths::downloads_dir().join(store_dir().file_name().unwrap_or_default());
    download::fetch_pinned(url, &dir, name, sha256, cancel, &mut |p| {
        if let Progress::Percent(v) = p {
            on(Step::Percent(v));
        }
    })
}

/// Fetches EchoXR when it is missing.
pub fn fetch(cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<()> {
    std::fs::create_dir_all(store_dir())?;
    let b = build();
    if !download::sha256_matches(&zip_path(), b.sha256) {
        on(Step::Status(format!("Downloading EchoXR {}...", b.version)));
        let zip = fetch_pinned(b.url, b.zip, b.sha256, cancel, on)?;
        std::fs::copy(&zip, zip_path()).context("keep EchoXR")?;
        let _ = std::fs::remove_file(zip);
    }
    // Meta's loader as older launchers kept it: never used again.
    let _ = std::fs::remove_dir_all(store_dir().join("oculus"));
    Ok(())
}

/// Takes Meta's loader and P2P library out of `dir` where an older launcher put them
/// there (only those very files).
pub fn remove_old_meta(dir: &Path) -> Result<()> {
    for (name, sha) in OLD_META {
        let f = dir.join(name);
        if download::sha256_matches(&f, sha) {
            std::fs::remove_file(&f).with_context(|| format!("remove {}", f.display()))?;
            tracing::info!("removed Meta's {name} from {}", dir.display());
        }
    }
    Ok(())
}

/// Whether `file` is EchoXR's Platform SDK stand-in (any build's, or Meta's loader an older
/// launcher put in place): the launcher's to remove.
pub fn is_launchers_platform(file: &Path) -> bool {
    builds()
        .flat_map(|b| b.files.iter())
        .filter(|(n, _)| *n == PLATFORM_IN_ECHOXR)
        .chain(OLD_META.iter().filter(|(n, _)| *n == PLATFORM_DLL))
        .any(|(_, sha)| download::sha256_matches(file, sha))
}

/// Linux: EchoXR's Platform SDK stand-in beside the game in `bin` (the first place the
/// game looks), Meta's copies an older launcher put there gone. For every start through the
/// Oculus platform, in VR and on the monitor; Linux has no Meta app it could get in the way
/// of.
pub fn platform_beside_game(bin: &Path) -> Result<()> {
    remove_old_meta(bin)?;
    let stand_in = bin.join(PLATFORM_IN_ECHOXR);
    let target = bin.join(PLATFORM_DLL);
    let want = std::fs::read(&stand_in)
        .with_context(|| format!("EchoXR's {PLATFORM_DLL} isn't in {}", bin.display()))?;
    if std::fs::read(&target).ok().as_deref() != Some(want.as_slice()) {
        put(&target, &want)?;
    }
    Ok(())
}

/// Puts EchoXR into the game's bin folder `bin` (next to `echovr.exe`): only what is
/// missing or differs. Meta's files an older launcher put into `EchoXR\` go.
pub fn install_into(bin: &Path) -> Result<()> {
    let zip = std::fs::File::open(zip_path())
        .context("EchoXR isn't downloaded: PLAY in the launcher downloads it")?;
    install_zip(zip, build(), bin)
}

/// [`build`]'s zip, as the launcher keeps it (for the administrator helper, which checks it).
pub fn zip_file() -> PathBuf {
    zip_path()
}

/// Which of the launcher's EchoXR builds `zip` is, if it is one (the pinned one or a test
/// build: never another zip).
pub fn known_zip(zip: &mut (impl std::io::Read + std::io::Seek)) -> Result<Option<&'static Build>> {
    zip.rewind()?;
    let sha = download::sha256_reader(zip)?;
    zip.rewind()?;
    Ok(builds().find(|b| b.sha256.eq_ignore_ascii_case(&sha)))
}

/// [`install_into`] from the zip `zip` of `build`.
pub fn install_zip(
    zip: impl std::io::Read + std::io::Seek,
    build: &Build,
    bin: &Path,
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(zip)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            bail!("EchoXR's zip has an unsafe path: {}", entry.name());
        };
        let out = bin.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes)?;
        if let Some((_, sha)) = build.files.iter().find(|(n, _)| Path::new(n) == rel) {
            if !download::sha256_reader(&mut bytes.as_slice())?.eq_ignore_ascii_case(sha) {
                bail!("{} in EchoXR's zip isn't the expected build", rel.display());
            }
        }
        if std::fs::read(&out).is_ok_and(|have| have == bytes) {
            continue;
        }
        put(&out, &bytes)?;
    }
    // An older launcher's ini, for 0.3.0's updater: only ever our own text.
    let ini = bin.join(DIR).join("echoxr.ini");
    if std::fs::read(&ini).is_ok_and(|have| have == OLD_INI.as_bytes()) {
        std::fs::remove_file(&ini).with_context(|| format!("remove {}", ini.display()))?;
    }
    remove_old_meta(&bin.join(DIR))?;
    Ok(())
}

/// Gets the game's bin folder `bin` ready for EchoXR, where that needs no administrator
/// rights: EchoXR in it (with its Platform SDK stand-in), its copy of the game current,
/// and the folder writable for EchoXR to make that copy at its first start. Fails as the
/// write did otherwise.
pub fn prepare(bin: &Path) -> Result<()> {
    install_into(bin)?;
    refresh_openxr_exe(bin)?;
    if !bin.join(paths::OPENXR_EXE).is_file() {
        let probe = bin.join(".echoxr-write-test");
        std::fs::write(&probe, b"")
            .with_context(|| format!("EchoXR can't write into {}", bin.display()))?;
        let _ = std::fs::remove_file(probe);
    }
    Ok(())
}

/// Has EchoXR make its copy of the game in `bin` now, as it would at its first start
/// (`--setup-only`), and waits.
pub fn make_openxr_exe(bin: &Path) -> Result<()> {
    if bin.join(paths::OPENXR_EXE).is_file() {
        return Ok(());
    }
    let status = crate::core::process::command(bin.join(LAUNCHER))
        .arg("--setup-only")
        .current_dir(bin)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("couldn't run EchoXR.exe")?;
    match status.code() {
        Some(0) => Ok(()),
        Some(code) if exit_message(code).is_some() => {
            bail!("{}", exit_message(code).unwrap_or_default())
        }
        _ => bail!(
            "EchoXR couldn't make {} ({status}): see {}",
            paths::OPENXR_EXE,
            bin.join(DIR).join("launcher.log").display()
        ),
    }
}

/// Deletes EchoXR's copy of the game (`echovr_openxr.exe`) when `echovr.exe` changed since
/// it was made (an update or a reinstall: the copy is older, or another size). EchoXR
/// never makes it again by itself; it does once it is gone.
pub fn refresh_openxr_exe(bin: &Path) -> Result<()> {
    let copy = bin.join(paths::OPENXR_EXE);
    let (Ok(c), Ok(g)) = (
        std::fs::metadata(&copy),
        std::fs::metadata(bin.join(paths::DEFAULT_EXE)),
    ) else {
        return Ok(());
    };
    let older = matches!((c.modified(), g.modified()), (Ok(c), Ok(g)) if c < g);
    if older || c.len() != g.len() {
        std::fs::remove_file(&copy)
            .with_context(|| format!("remove the outdated {}", paths::OPENXR_EXE))?;
        tracing::info!("echovr.exe changed: {} goes", copy.display());
    }
    Ok(())
}

/// Writes `bytes` to `path` (making its folder).
fn put(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_its_exit_codes() {
        for code in 2..=7 {
            assert!(exit_message(code).is_some(), "{code}");
        }
        // Echo's own exit codes are Echo's.
        for code in [0, 1, 8, -1, 0xC000_0005_u32 as i32] {
            assert_eq!(exit_message(code), None);
        }
        // Through Virtual Desktop, what to start is VD's.
        let mut p = LaunchProfile {
            runtime: Runtime::Revive,
            ..Default::default()
        };
        assert_eq!(exit_message_for(5, &p), exit_message(5));
        p.runtime = Runtime::VirtualDesktop;
        p.vd_via = VdVia::VdXr;
        assert!(exit_message_for(5, &p)
            .unwrap()
            .contains("Virtual Desktop's OpenXR"));
        p.vd_via = VdVia::SteamVr;
        assert!(exit_message_for(5, &p).unwrap().contains("SteamVR"));
        assert!(exit_message_for(6, &p).unwrap().contains("Virtual Desktop"));
        assert_eq!(exit_message_for(4, &p), exit_message(4));
        assert_eq!(exit_message_for(1, &p), None);
    }

    #[test]
    fn takes_an_old_launchers_ini_away() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        // A zip with just the runtime folder: enough to install from.
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.add_directory("EchoXR/", zip::write::SimpleFileOptions::default())
            .unwrap();
        let zip = zip.finish().unwrap();
        let ini = bin.join(DIR).join("echoxr.ini");
        std::fs::create_dir_all(ini.parent().unwrap()).unwrap();
        std::fs::write(&ini, OLD_INI).unwrap();
        install_zip(std::io::Cursor::new(zip.get_ref().clone()), &PINNED, bin).unwrap();
        assert!(!ini.exists());
        // Someone's own ini stays.
        std::fs::write(&ini, "CheckForUpdates = 1\r\n").unwrap();
        install_zip(std::io::Cursor::new(zip.into_inner()), &PINNED, bin).unwrap();
        assert!(ini.exists());
    }

    #[test]
    fn tells_echos_exit_code_from_echoxrs() {
        let log = "===== 2026-10-05 14:51:06 =====\n[14:51:06] EchoXR launcher 0.4.0\n\
                   [14:51:15] launching: \"X:\\g\\echovr_openxr.exe\"\n\
                   [15:00:23] Echo exited with code 3\n";
        assert_eq!(game_exit_code(log), Some(3));
        // A later start that EchoXR itself ended (no game code): its code is EchoXR's.
        let later = format!("{log}\n===== 2026-10-05 15:10:00 =====\n[15:10:02] ERROR: OpenXR isn't available (exit code 5)\n");
        assert_eq!(game_exit_code(&later), None);
        assert_eq!(game_exit_code(""), None);
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        std::fs::create_dir_all(bin.join(DIR)).unwrap();
        std::fs::write(bin.join(DIR).join("launcher.log"), log).unwrap();
        assert_eq!(failure(bin, 3), None);
        std::fs::write(bin.join(DIR).join("launcher.log"), &later).unwrap();
        assert!(failure(bin, 5).is_some());
    }

    #[test]
    fn makes_a_changed_games_copy_again() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        let (game, copy) = (bin.join(paths::DEFAULT_EXE), bin.join(paths::OPENXR_EXE));
        // Nothing to refresh yet.
        refresh_openxr_exe(bin).unwrap();
        std::fs::write(&game, b"game v1").unwrap();
        std::fs::write(&copy, b"GAME v1").unwrap();
        let t = std::time::SystemTime::now();
        let set = |p: &Path, at: std::time::SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(at)
                .unwrap();
        };
        set(&game, t - std::time::Duration::from_secs(60));
        set(&copy, t);
        refresh_openxr_exe(bin).unwrap();
        assert!(copy.exists(), "made from the current game");
        // The game was updated after the copy was made.
        set(&game, t + std::time::Duration::from_secs(60));
        refresh_openxr_exe(bin).unwrap();
        assert!(!copy.exists());
        // Another size: another build.
        std::fs::write(&copy, b"GAME v1, longer").unwrap();
        set(&copy, t + std::time::Duration::from_secs(120));
        refresh_openxr_exe(bin).unwrap();
        assert!(!copy.exists());
    }

    /// Every EchoXR zip the launcher knows is on GitHub, and its files are the pinned ones.
    #[test]
    #[ignore = "network"]
    fn installs_the_pinned_echoxr() {
        for b in builds() {
            let dir = tempfile::tempdir().unwrap();
            let zip = dir.path().join(b.zip);
            crate::core::http::download_to(b.url, &zip, None).unwrap();
            assert!(download::sha256_matches(&zip, b.sha256), "{}", b.version);
            let mut f = std::fs::File::open(&zip).unwrap();
            assert_eq!(
                known_zip(&mut f).unwrap().map(|k| k.version),
                Some(b.version)
            );
            let mut archive = zip::ZipArchive::new(f).unwrap();
            for (name, sha) in b.files {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut archive.by_name(name).unwrap(), &mut bytes)
                    .unwrap();
                assert_eq!(
                    download::sha256_reader(&mut bytes.as_slice()).unwrap(),
                    sha,
                    "{} {name}",
                    b.version
                );
            }
        }
    }

    /// A dev code picks a test build by its version; anything else is the pinned build.
    #[test]
    fn a_dev_code_picks_only_a_known_test_build() {
        assert_eq!(pick(None).version, PINNED.version);
        assert_eq!(pick(Some("0.6.3")).version, "0.6.3");
        assert_eq!(pick(Some("0.6.3")).zip, "EchoXR-OpenXR-v0.6.3.zip");
        for other in ["9.9.9", "", "0.6.1 ", "../x", "0.6.2"] {
            assert_eq!(pick(Some(other)).version, PINNED.version, "{other:?}");
        }
        // No code in tests: the pinned build.
        assert_eq!(build().version, PINNED.version);
        // --verbose only for a build whose EchoXR.exe takes it (0.6.3 on): an older one hands it
        // to the game.
        assert!(PINNED.verbose);
        assert_eq!(args_for(&PINNED, Some("max")), ["--verbose", "max"]);
        assert!(args_for(&PINNED, None).is_empty());
        for b in TESTS.iter().filter(|b| b.verbose) {
            assert_eq!(args_for(b, Some("max")), ["--verbose", "max"]);
            assert!(args_for(b, None).is_empty());
            assert!(test_logs_more(b.version));
        }
        assert!(!test_logs_more(PINNED.version));
        assert!(dev_args().is_empty());
        // Every build has its own zip name, so they're kept side by side.
        let names: std::collections::BTreeSet<_> = builds().map(|b| b.zip).collect();
        assert_eq!(names.len(), builds().count());
        for b in builds() {
            assert!(b
                .url
                .starts_with("https://github.com/marshmallow-mia/EchoXR/releases/download/"));
            assert!(b.url.ends_with(b.zip));
            assert_eq!(b.sha256.len(), 64, "{}", b.version);
            assert!(
                b.files.iter().all(|(_, sha)| sha.len() == 64),
                "{}",
                b.version
            );
        }
    }

    /// Meta's files an older launcher put in place go (only those very files), and
    /// EchoXR's stand-in goes beside the game on Linux.
    #[test]
    fn takes_metas_files_out_and_the_stand_in_beside_the_game() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        std::fs::create_dir_all(bin.join(DIR)).unwrap();
        std::fs::write(bin.join(PLATFORM_IN_ECHOXR), b"MZ stand-in").unwrap();
        std::fs::write(bin.join("LibOVRP2P64_1.dll"), b"someone else's").unwrap();
        platform_beside_game(bin).unwrap();
        assert_eq!(
            std::fs::read(bin.join(PLATFORM_DLL)).unwrap(),
            b"MZ stand-in"
        );
        assert!(bin.join("LibOVRP2P64_1.dll").is_file());
        // Not the pinned files: neither is the launcher's.
        assert!(!is_launchers_platform(&bin.join(PLATFORM_DLL)));
        // No stand-in in EchoXR\: an error, not a Meta fallback.
        let empty = tempfile::tempdir().unwrap();
        assert!(platform_beside_game(empty.path()).is_err());
    }
}
