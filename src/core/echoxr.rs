//! EchoXR (github.com/EchoTools/EchoXR, its OpenXR layer only): `EchoXR.exe` in the game's
//! `bin/win10` starts the game as `echovr_openxr.exe` (its patched copy of `echovr.exe`,
//! made when missing) with `EchoXR\LibOVRRT64_1.dll` answering Echo's LibOVR calls over
//! OpenXR: no Meta services, sign-in or store, and no injection. On Windows it plays on
//! SteamVR (the SteamVR choice "through EchoXR"); on Linux through GE-Proton (`linux`).
//!
//! `pnsovr.dll` logs in through Meta's Platform SDK loader (`LibOVRPlatform64_1.dll`, which
//! loads the game's own `LibOVRPlatformImpl64_1.dll` -- the community update ships it --
//! and that needs `LibOVRP2P64_1.dll`). The Meta app brings the loader and P2P along.
//! Without the app, both are read out of Meta's own runtime package, never shipped. Every
//! download is pinned by its SHA-256. See docs/launcher/echoxr.md.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};

use crate::core::download::{self, Progress};
use crate::core::launcher::versions::Step;
use crate::core::{paths, remote_zip};

/// EchoXR's OpenXR layer (EchoTools/EchoXR 0.4.1, its GitHub release): unpacked into the
/// game's `bin/win10`. Built with the static C runtime, so it needs no Visual C++ runtime.
/// 0.4.1 runs under GE-Proton: OpenXR's values from its `Wine\XR`, and a swapchain's image
/// count asked for before the first acquire.
pub const VERSION: &str = "0.4.1";
const ZIP: &str = "EchoXR-OpenXR-v0.4.1.zip";
const URL: &str =
    "https://github.com/EchoTools/EchoXR/releases/download/v0.4.1-rc1/EchoXR-OpenXR-v0.4.1.zip";
const SHA256: &str = "1777c7788458e97d8abcb3b0ec54776ec85310ed88a659d3b5679e045acb4f93";
/// Who made it, as the Mods page credits it: EchoXR, and what it builds on.
pub const AUTHORS: &str =
    "heisthecat31, marshmallow-mia, Villagers654 (RiftLift), CrossVR (Revive)";
/// What starts the game, in its `bin/win10`.
pub const LAUNCHER: &str = "EchoXR.exe";
/// The files that do the work, with their hashes.
const FILES: [(&str, &str); 3] = [
    (
        LAUNCHER,
        "8d62f7b950f0e498db95ee814d550481e77528c41e2e053ae50c07eda939e52c",
    ),
    (
        "EchoXR/LibOVRRT64_1.dll",
        "e34b8b4477306ab8c3cf8b1224a5d4097c8b1ba6eb8291cfa7cbd50f9244d666",
    ),
    (
        "EchoXR/openxr_loader.dll",
        "fbaf08c7e489cdac94d6eea5770ae0378e3c8d8fcf3764e0740b2aa73246415c",
    ),
];
/// What older launchers wrote as `EchoXR/echoxr.ini` (0.3.0's updater off). 0.4.0 has no
/// updater and no ini: this one goes.
const OLD_INI: &str =
    "# Kept by the Echo VR launcher, which updates EchoXR itself\r\nCheckForUpdates = 0\r\n";
/// EchoXR's folder in `bin/win10`: its runtime, and where its `PATH` points first.
pub const DIR: &str = "EchoXR";

/// Meta's PC runtime package; only its Platform SDK loader is read out of it.
const META_RUNTIME_URL: &str =
    "https://securecdn.oculus.com/binaries/download/?id=3766757683456363";
/// The Platform SDK loader and the P2P library the game's implementation needs, as that
/// package has them. (The implementation, `LibOVRPlatformImpl64_1.dll`, is the game's.)
pub const PLATFORM: [(&str, &str); 2] = [
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

fn platform_dir() -> PathBuf {
    store_dir().join("oculus")
}

/// Whether EchoXR and Meta's loader have been fetched (and are still there).
pub fn is_fetched() -> bool {
    zip_path().is_file()
        && PLATFORM
            .iter()
            .all(|(n, _)| platform_dir().join(n).is_file())
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

/// Fetches what is missing: EchoXR, and Meta's Platform SDK loader.
pub fn fetch(cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<()> {
    std::fs::create_dir_all(store_dir())?;
    if !download::sha256_matches(&zip_path(), SHA256) {
        on(Step::Status(format!("Downloading EchoXR {VERSION}...")));
        let zip = fetch_pinned(URL, ZIP, SHA256, cancel, on)?;
        std::fs::copy(&zip, zip_path()).context("keep EchoXR")?;
        let _ = std::fs::remove_file(zip);
    }
    let missing: Vec<(String, String)> = PLATFORM
        .iter()
        .filter(|(n, sha)| !download::sha256_matches(&platform_dir().join(n), sha))
        .map(|(n, sha)| (n.to_string(), sha.to_string()))
        .collect();
    if !missing.is_empty() {
        on(Step::Status("Reading Meta's Platform SDK loader...".into()));
        remote_zip::extract_members(
            META_RUNTIME_URL,
            "",
            &missing,
            &platform_dir(),
            cancel,
            &mut |_, _| {},
        )
        .context("Couldn't read the Platform SDK loader out of Meta's runtime package")?;
    }
    Ok(())
}

/// Puts EchoXR into the game's bin folder `bin` (next to `echovr.exe`), and Meta's Platform
/// SDK loader and P2P library into `platform_to` when given: only what is missing or
/// differs.
pub fn install_into(bin: &Path, platform_to: Option<&Path>) -> Result<()> {
    let zip = std::fs::File::open(zip_path())
        .context("EchoXR isn't downloaded: PLAY in the launcher downloads it")?;
    install_zip(zip, bin, platform_to)
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
pub fn install_zip(
    zip: impl std::io::Read + std::io::Seek,
    bin: &Path,
    platform_to: Option<&Path>,
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
    if let Some(to) = platform_to {
        std::fs::create_dir_all(to)?;
        for (name, sha) in PLATFORM {
            let dll = to.join(name);
            if !download::sha256_matches(&dll, sha) {
                std::fs::copy(platform_dir().join(name), &dll)
                    .with_context(|| format!("copy {name}"))?;
            }
        }
    }
    Ok(())
}

/// Where Meta's Platform SDK loader goes for the game in `bin` on Windows: nowhere when
/// the Meta app brings it, else EchoXR's folder (first on `PATH` only when EchoXR starts
/// the game, so other starts never see it).
pub fn platform_dir_for(bin: &Path) -> Option<PathBuf> {
    crate::core::platform::oculus_base_path()
        .is_none()
        .then(|| bin.join(DIR))
}

/// Gets the game's bin folder `bin` ready for EchoXR, where that needs no administrator
/// rights: EchoXR in it (Meta's loader into `platform_to` when given), its copy of the
/// game current, and the folder writable for EchoXR to make that copy at its first start.
/// Fails as the write did otherwise.
pub fn prepare(bin: &Path, platform_to: Option<&Path>) -> Result<()> {
    install_into(bin, platform_to)?;
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
        install_zip(std::io::Cursor::new(zip.get_ref().clone()), bin, None).unwrap();
        assert!(!ini.exists());
        // Someone's own ini stays.
        std::fs::write(&ini, "CheckForUpdates = 1\r\n").unwrap();
        install_zip(std::io::Cursor::new(zip.into_inner()), bin, None).unwrap();
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

    /// Meta's package still has the pinned Platform SDK loader and P2P library, and they
    /// can be read out of it alone.
    #[test]
    #[ignore = "network"]
    fn reads_the_platform_loader_out_of_metas_package() {
        let dir = tempfile::tempdir().unwrap();
        let members: Vec<(String, String)> = PLATFORM
            .iter()
            .map(|(n, sha)| (n.to_string(), sha.to_string()))
            .collect();
        remote_zip::extract_members(
            META_RUNTIME_URL,
            "",
            &members,
            dir.path(),
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap();
        for (name, sha) in PLATFORM {
            assert!(
                download::sha256_matches(&dir.path().join(name), sha),
                "{name}"
            );
        }
    }
}
