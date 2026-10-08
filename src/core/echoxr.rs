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

/// EchoXR's OpenXR layer (marshmallow-mia/EchoXR 0.5.0, its GitHub release): unpacked into the
/// game's bin folder (`bin/win10`, or an event build's `bin/win7`). Built with the static C
/// runtime, so it needs no Visual C++ runtime. 0.5.0 runs the five event builds; 0.4.2
/// brings the Platform SDK stand-in; 0.4.1 made it run under GE-Proton.
pub const VERSION: &str = "0.5.0";
const ZIP: &str = "EchoXR-OpenXR-v0.5.0.zip";
const URL: &str =
    "https://github.com/marshmallow-mia/EchoXR/releases/download/v0.5.0-rc1/EchoXR-OpenXR-v0.5.0.zip";
const SHA256: &str = "cfc58aa67f6c5f0dd492cc27ce682be58ed57c255b6bb3b5b069f57b1715a0e2";
/// Who made it, as the Mods page credits it: EchoXR, and what it builds on.
pub const AUTHORS: &str =
    "heisthecat31, marshmallow-mia, Villagers654 (RiftLift), CrossVR (Revive)";
/// What starts the game, in its `bin/win10`.
pub const LAUNCHER: &str = "EchoXR.exe";
/// The files that do the work, with their hashes.
const FILES: [(&str, &str); 4] = [
    (
        LAUNCHER,
        "d024c0400afa62cac0f91c730ca912823b25535950b0c3c8c5aec535ad3dba65",
    ),
    (
        "EchoXR/LibOVRRT64_1.dll",
        "b7595078eb23ece899b9772dc7b03131ff71597fb5ae4e79d383cbf42e858932",
    ),
    (
        "EchoXR/openxr_loader.dll",
        "a60093f3e01198c19ea1dab7ee9de3561fba0ac68a47a690d634697fbf36040c",
    ),
    (
        PLATFORM_IN_ECHOXR,
        "c8436034b36c6afbb15cf0f3f886f92d36a5587dda5c5c09774a65cc26734295",
    ),
];
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
    store_dir().join(ZIP)
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
    if !download::sha256_matches(&zip_path(), SHA256) {
        on(Step::Status(format!("Downloading EchoXR {VERSION}...")));
        let zip = fetch_pinned(URL, ZIP, SHA256, cancel, on)?;
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

/// Whether `file` is EchoXR's Platform SDK stand-in (or Meta's loader an older launcher
/// put in place): the launcher's to remove.
pub fn is_launchers_platform(file: &Path) -> bool {
    FILES
        .iter()
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
    install_zip(zip, bin)
}

/// The pinned zip, as the launcher keeps it (for the administrator helper, which checks it).
pub fn zip_file() -> PathBuf {
    zip_path()
}

/// Whether `zip` is the pinned EchoXR zip.
pub fn is_pinned_zip(zip: &mut (impl std::io::Read + std::io::Seek)) -> Result<bool> {
    zip.rewind()?;
    let ok = download::sha256_reader(zip)?.eq_ignore_ascii_case(SHA256);
    zip.rewind()?;
    Ok(ok)
}

/// [`install_into`] from the zip `zip`.
pub fn install_zip(zip: impl std::io::Read + std::io::Seek, bin: &Path) -> Result<()> {
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
        if let Some((_, sha)) = FILES.iter().find(|(n, _)| Path::new(n) == rel) {
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
        install_zip(std::io::Cursor::new(zip.get_ref().clone()), bin).unwrap();
        assert!(!ini.exists());
        // Someone's own ini stays.
        std::fs::write(&ini, "CheckForUpdates = 1\r\n").unwrap();
        install_zip(std::io::Cursor::new(zip.into_inner()), bin).unwrap();
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

    /// The pinned EchoXR zip is on files.echovr.de, its files are the pinned builds, and
    /// it unpacks into a game folder with the launcher's echoxr.ini.
    #[test]
    #[ignore = "network"]
    fn installs_the_pinned_echoxr() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join(ZIP);
        crate::core::http::download_to(URL, &zip, None).unwrap();
        assert!(download::sha256_matches(&zip, SHA256));
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        for (name, sha) in FILES {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut archive.by_name(name).unwrap(), &mut bytes).unwrap();
            assert_eq!(
                download::sha256_reader(&mut bytes.as_slice()).unwrap(),
                sha,
                "{name}"
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
