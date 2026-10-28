//! nEVR's root, `%LOCALAPPDATA%\nevr\echovr` (the Wine prefix's on Linux): one per computer
//! (ADR 0012 in nevr-runtime). From nevr-runtime #1104 on, nEVR's host loads
//! `nevr-runtime.dll` only from there (a root host, [`super::nevr::is_root_host`]); 5.8.1
//! to 5.16 load it from beside the host. From 5.13 on, nEVR keeps its sign-in
//! (`.credentials.json`, which wins over the game's `_local`) and its logs there too.
//!
//! The launcher keeps several versions, each with its own nEVR, so a root host's runtime is
//! kept in its game folder ([`super::nevr::ROOT_COPY`]) and put into the root before a
//! start of that version: what install.ps1 does once, before each start.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::nevr;
use crate::core::paths;

/// What the launcher put into the root last: a runtime there that isn't this one was put
/// there by something else (nEVR's installer), and is set aside, not replaced.
const RECORD: &str = "nevr-root.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Record {
    /// The runtime's sha256.
    runtime_sha256: String,
    /// The game folder it came from.
    from: String,
}

/// The game's `%LOCALAPPDATA%`: Windows' own, or the Wine prefix's on Linux. None elsewhere.
pub fn local_app_data() -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        Some(crate::core::linux::echoxr::local_app_data())
    } else if cfg!(windows) {
        dirs::data_local_dir()
    } else {
        None
    }
}

/// nEVR's root in `local` (a `%LOCALAPPDATA%`).
pub fn root_in(local: &Path) -> PathBuf {
    local.join("nevr").join("echovr")
}

/// nEVR's root on this computer.
pub fn root() -> Option<PathBuf> {
    local_app_data().map(|l| root_in(&l))
}

/// Before a start of the game in `bin`: when its host is a root host, its runtime is in the
/// root (`root`), as its folder keeps it. A runtime the launcher didn't put there goes aside
/// (`nevr-runtime.dll.original-<time>`, as install.ps1 does); one of another version
/// isn't replaced while Echo VR runs (`running`: its file is in use, and the game playing
/// on it would be told nothing). `record` notes what the launcher put there.
pub fn put_in_place(bin: &Path, root: &Path, record: &Path, running: bool) -> Result<()> {
    let host = std::fs::read(bin.join(super::mods::SLOT)).unwrap_or_default();
    if !nevr::is_root_host(&host) {
        return Ok(());
    }
    let ours = bin.join(nevr::ROOT_COPY);
    let want = sha256_of(&ours).with_context(|| {
        format!(
            "{} is missing from {}: Repair game files (in PLAY's menu) gets it again",
            nevr::RUNTIME,
            bin.display()
        )
    })?;
    let target = root.join(nevr::RUNTIME);
    let there = sha256_of(&target).ok();
    if there.as_deref() == Some(want.as_str()) {
        return Ok(());
    }
    if let Some(there) = &there {
        if running {
            bail!(
                "Echo VR is running with another version of nEVR. Close it first: nEVR's runtime is one file for every Echo VR on this computer."
            );
        }
        let noted: Option<Record> = std::fs::read_to_string(record)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        if noted.is_none_or(|r| &r.runtime_sha256 != there) {
            let aside = target.with_file_name(format!(
                "{}.original-{}",
                nevr::RUNTIME,
                time::OffsetDateTime::now_utc().unix_timestamp()
            ));
            std::fs::rename(&target, &aside)
                .with_context(|| format!("Couldn't set aside {}", target.display()))?;
            tracing::info!(
                "nEVR's root: set aside a runtime the launcher didn't put there ({})",
                aside.display()
            );
        }
    }
    std::fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
    // Copied beside it first, so the root never holds half a file.
    let part = root.join(format!(".{}.launcher", nevr::RUNTIME));
    std::fs::copy(&ours, &part).with_context(|| format!("copy into {}", root.display()))?;
    if let Err(e) = std::fs::rename(&part, &target) {
        let _ = std::fs::remove_file(&part);
        return Err(e).with_context(|| format!("Couldn't replace {}", target.display()));
    }
    let noted = Record {
        runtime_sha256: want,
        from: bin.to_string_lossy().into_owned(),
    };
    if let Some(dir) = record.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(record, serde_json::to_vec_pretty(&noted)?)
        .with_context(|| format!("write {}", record.display()))?;
    tracing::info!(
        "nEVR's root: put the runtime of {} in place ({})",
        bin.display(),
        target.display()
    );
    Ok(())
}

/// [`put_in_place`] on this computer: for a start of the game in `bin`.
pub fn put_in_place_for(bin: &Path) -> Result<()> {
    let Some(root) = root() else {
        return Ok(());
    };
    let running = !super::game::running_now().is_empty();
    put_in_place(bin, &root, &paths::data_dir().join(RECORD), running)
}

/// Takes the runtime the launcher put into the root away again (uninstall), when it is
/// still the one it put there. Whether it did.
pub fn take_away() -> bool {
    let Some(root) = root() else {
        return false;
    };
    take_away_from(&root, &paths::data_dir().join(RECORD))
}

fn take_away_from(root: &Path, record: &Path) -> bool {
    let target = root.join(nevr::RUNTIME);
    let noted: Option<Record> = std::fs::read_to_string(record)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let ours = noted.is_some_and(|r| sha256_of(&target).is_ok_and(|s| s == r.runtime_sha256));
    let _ = std::fs::remove_file(record);
    ours && std::fs::remove_file(&target).is_ok()
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    Ok(crate::core::download::sha256_reader(&mut f)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "NEVR-BUILD 5.17.0 abcdef0";

    fn game(dir: &Path, runtime: &str) -> PathBuf {
        let bin = dir.join("bin");
        std::fs::create_dir_all(bin.join("nevr-root")).unwrap();
        std::fs::write(
            bin.join(super::super::mods::SLOT),
            format!("MZ nevr_root_override\0{ID}\0"),
        )
        .unwrap();
        std::fs::write(bin.join(nevr::ROOT_COPY), runtime).unwrap();
        bin
    }

    #[test]
    fn puts_a_root_hosts_runtime_into_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let (root, record) = (dir.path().join("root"), dir.path().join("rec.json"));
        let a = game(&dir.path().join("a"), "runtime a");
        let b = game(&dir.path().join("b"), "runtime b");
        let in_root = || std::fs::read_to_string(root.join(nevr::RUNTIME)).unwrap();
        put_in_place(&a, &root, &record, false).unwrap();
        assert_eq!(in_root(), "runtime a");
        // The same one again changes nothing, even while Echo VR runs.
        put_in_place(&a, &root, &record, true).unwrap();
        // Another version's: not while Echo VR runs; else in its place, nothing set aside.
        assert!(put_in_place(&b, &root, &record, true).is_err());
        assert_eq!(in_root(), "runtime a");
        put_in_place(&b, &root, &record, false).unwrap();
        assert_eq!(in_root(), "runtime b");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        // One the launcher didn't put there (nEVR's installer) is set aside, never replaced.
        std::fs::write(root.join(nevr::RUNTIME), "installer's").unwrap();
        put_in_place(&a, &root, &record, false).unwrap();
        assert_eq!(in_root(), "runtime a");
        let aside: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("nevr-runtime.dll.original-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(
            std::fs::read_to_string(root.join(&aside[0])).unwrap(),
            "installer's"
        );
        // Uninstall takes ours away, and only ours.
        assert!(take_away_from(&root, &record));
        assert!(!root.join(nevr::RUNTIME).exists());
        std::fs::write(root.join(nevr::RUNTIME), "installer's").unwrap();
        assert!(!take_away_from(&root, &record));
        assert!(root.join(nevr::RUNTIME).exists());
    }

    /// Before a start of a real game folder (`ECHOVR_TEST_BIN`), into a root of its own
    /// (`ECHOVR_TEST_ROOT`): `cargo test puts_a_game_folders_runtime -- --ignored`.
    #[test]
    #[ignore]
    fn puts_a_game_folders_runtime_into_its_root() {
        let bin = PathBuf::from(std::env::var("ECHOVR_TEST_BIN").unwrap());
        let root = PathBuf::from(std::env::var("ECHOVR_TEST_ROOT").unwrap());
        let record = root.with_file_name("launcher-nevr-root.json");
        put_in_place(&bin, &root, &record, false).unwrap();
        println!("{:?}", std::fs::read_dir(&root).map(|d| d.count()));
    }

    #[test]
    fn leaves_the_root_alone_for_an_older_host() {
        let dir = tempfile::tempdir().unwrap();
        let (root, record) = (dir.path().join("root"), dir.path().join("rec.json"));
        let bin = game(dir.path(), "runtime");
        std::fs::write(
            bin.join(super::super::mods::SLOT),
            format!("MZ host\0{ID}\0"),
        )
        .unwrap();
        put_in_place(&bin, &root, &record, false).unwrap();
        assert!(!root.exists());
        // A root host whose runtime is missing says so (Repair).
        let bin = game(&dir.path().join("x"), "runtime");
        std::fs::remove_file(bin.join(nevr::ROOT_COPY)).unwrap();
        let e = put_in_place(&bin, &root, &record, false).unwrap_err();
        assert!(format!("{e:#}").contains("Repair game files"), "{e:#}");
    }
}
