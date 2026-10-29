//! nEVR's root, `%LOCALAPPDATA%\nevr\echovr` (the Wine prefix's on Linux): one per computer
//! (ADR 0012 in nevr-runtime). From nevr-runtime #1104 on, nEVR's host loads
//! `nevr-runtime.dll` only from there (a root host, [`super::nevr::is_root_host`]); 5.8.1
//! to 5.16 load it from beside the host. From 5.13 on, nEVR keeps its sign-in
//! (`.credentials.json`, which wins over the game's `_local`) and its logs there too.
//!
//! The launcher keeps several versions, each with its own nEVR, so a root host's runtime is
//! kept with the version ([`super::nevr::root_copy`]: from nEVR 6.0 on in its folder in the
//! launcher's, [`super::home`]) and put into the root before a start of that version: what
//! install.ps1 does once, before each start. From nevr-runtime #1250 on the runtime loads
//! plugins only from the root's `plugins` too, so the version's plugins (where the Mods page
//! keeps them, [`super::mods::plugins_in`]) are mirrored there before each start
//! ([`plugins_into`]). Like install.ps1, the launcher notes the install in the root's
//! `installer\nevr-install.txt`, as its owner (`owner=launcher`): nEVR's own scripts leave it
//! alone ([`note_install`]).

use std::collections::BTreeMap;
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
    /// The files it put into the root's `plugins` (path in it, with `/`) → sha256.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    plugins: BTreeMap<String, String>,
    /// The files it put into the root's `content` (its content packs), the same way.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    content: BTreeMap<String, String>,
}

fn read_record(record: &Path) -> Option<Record> {
    std::fs::read_to_string(record)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn write_record(record: &Path, noted: &Record) -> Result<()> {
    if let Some(dir) = record.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(record, serde_json::to_vec_pretty(noted)?)
        .with_context(|| format!("write {}", record.display()))
}

/// `from` copied to `to` beside it first, so `to` is never half a file.
fn copy_whole(from: &Path, to: &Path) -> Result<()> {
    let dir = to.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let name = to.file_name().unwrap_or_default().to_string_lossy();
    let part = dir.join(format!(".{name}.launcher"));
    std::fs::copy(from, &part).with_context(|| format!("copy into {}", dir.display()))?;
    if let Err(e) = std::fs::rename(&part, to) {
        let _ = std::fs::remove_file(&part);
        return Err(e).with_context(|| format!("Couldn't replace {}", to.display()));
    }
    Ok(())
}

/// `target` renamed to `<name>.original-<time>` beside it, as install.ps1 does.
fn set_aside(target: &Path) -> Result<PathBuf> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let aside = target.with_file_name(format!(
        "{name}.original-{}",
        time::OffsetDateTime::now_utc().unix_timestamp()
    ));
    std::fs::rename(target, &aside)
        .with_context(|| format!("Couldn't set aside {}", target.display()))?;
    Ok(aside)
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
    let ours = nevr::root_copy(bin);
    let want = sha256_of(&ours).with_context(|| {
        format!(
            "{} is missing from {}: Repair game files (in PLAY's menu) gets it again",
            nevr::RUNTIME,
            ours.parent().unwrap_or(bin).display()
        )
    })?;
    let target = root.join(nevr::RUNTIME);
    let there = sha256_of(&target).ok();
    if there.as_deref() == Some(want.as_str()) {
        note_install(root, bin, &host, &want);
        return Ok(());
    }
    if let Some(there) = &there {
        if running {
            bail!(
                "Echo VR is running with another version of nEVR. Close it first: nEVR's runtime is one file for every Echo VR on this computer."
            );
        }
        if read_record(record).is_none_or(|r| &r.runtime_sha256 != there) {
            let aside = set_aside(&target)?;
            tracing::info!(
                "nEVR's root: set aside a runtime the launcher didn't put there ({})",
                aside.display()
            );
        }
    }
    copy_whole(&ours, &target)?;
    // The plugins and content packs it put there stay noted ([`plugins_into`] runs after this).
    let before = read_record(record).unwrap_or_default();
    let noted = Record {
        runtime_sha256: want,
        from: bin.to_string_lossy().into_owned(),
        plugins: before.plugins,
        content: before.content,
    };
    write_record(record, &noted)?;
    note_install(root, bin, &host, &noted.runtime_sha256);
    tracing::info!(
        "nEVR's root: put the runtime of {} in place ({})",
        bin.display(),
        target.display()
    );
    Ok(())
}

/// [`put_in_place`] and [`plugins_into`] on this computer: for a start of the game in `bin`.
pub fn put_in_place_for(bin: &Path) -> Result<()> {
    let Some(root) = root() else {
        return Ok(());
    };
    let running = !super::game::running_now().is_empty();
    let record = paths::data_dir().join(RECORD);
    put_in_place(bin, &root, &record, running)?;
    plugins_into(bin, &root, &record, running)
}

/// Before a start of the game in `bin`, after [`put_in_place`]: when its runtime loads plugins
/// only from the root (nevr-runtime #1250 on), the root's `plugins` holds what `bin`'s
/// `plugins` holds (subfolders too). What the launcher put there for another start and this
/// one lacks goes; a file of the same name it didn't put there is set aside
/// (`<name>.original-<time>`); anything else there is left alone (nEVR loads only the plugins
/// `config.yaml` lists). Another set isn't put there while Echo VR runs (`running`).
///
/// Its content packs (`bin`'s `content`, [`super::packs::CONTENT`]) go into the root's
/// `content` the same way: NvrContentOverlay 1.1.0 on reads a pack from `content` beside the
/// `plugins` folder it was loaded from.
pub fn plugins_into(bin: &Path, root: &Path, record: &Path, running: bool) -> Result<()> {
    let runtime = std::fs::read(nevr::root_copy(bin)).unwrap_or_default();
    if !nevr::loads_root_plugins(&runtime) {
        return Ok(());
    }
    let mut noted = read_record(record).unwrap_or_default();
    // Neither set changes while Echo VR runs: `mirror` refuses before it touches anything, and
    // the content's only refuses when the plugins' didn't change either.
    let dest = super::mods::plugins_in(root);
    if mirror(
        &super::mods::plugins_in(bin),
        &dest,
        &mut noted.plugins,
        running,
    )? {
        write_record(record, &noted)?;
        tracing::info!(
            "nEVR's root: the plugins of {} in place ({}, {} files)",
            bin.display(),
            dest.display(),
            noted.plugins.len()
        );
    }
    std::fs::create_dir_all(&dest).with_context(|| format!("create {}", dest.display()))?;
    let content = root.join(super::packs::CONTENT);
    if mirror(
        &bin.join(super::packs::CONTENT),
        &content,
        &mut noted.content,
        running,
    )? {
        write_record(record, &noted)?;
        tracing::info!(
            "nEVR's root: the content packs of {} in place ({}, {} files)",
            bin.display(),
            content.display(),
            noted.content.len()
        );
    }
    Ok(())
}

/// `dest` holds what `from` holds (subfolders too), `noted` being what the launcher put there
/// before (path with `/` → sha256), then what it put there now. What it put there and `from`
/// lacks goes (and the folders that leaves empty, `dest` too); a file of the same name it
/// didn't put there is set aside; anything else is left alone. Nothing changes while Echo VR
/// runs (`running`): an error when something would. Whether anything changed.
fn mirror(
    from: &Path,
    dest: &Path,
    noted: &mut BTreeMap<String, String>,
    running: bool,
) -> Result<bool> {
    let mut want = BTreeMap::new();
    files_in(from, "", &mut want)?;
    let in_place = |rel: &str, sha: &str| {
        sha256_of(&dest.join(rel)).is_ok_and(|s| s == sha)
            && noted.get(rel).is_some_and(|n| n == sha)
    };
    let gone = noted.keys().any(|rel| !want.contains_key(rel));
    if !gone && want.iter().all(|(rel, (_, sha))| in_place(rel, sha)) {
        return Ok(false);
    }
    if running {
        bail!(
            "Echo VR is running with other mods. Close it first: nEVR loads one set of plugins for every Echo VR on this computer."
        );
    }
    for rel in noted.keys().filter(|rel| !want.contains_key(*rel)) {
        remove_noted(dest, rel);
    }
    for (rel, (from, sha)) in &want {
        let to = dest.join(rel);
        match sha256_of(&to) {
            Ok(there) if &there == sha => continue,
            // A file it put there for another start (or a mod changed since) is replaced.
            Ok(_) if noted.contains_key(rel) => {}
            Ok(_) => {
                let aside = set_aside(&to)?;
                tracing::info!(
                    "nEVR's root: set aside a file the launcher didn't put there ({})",
                    aside.display()
                );
            }
            Err(_) => {}
        }
        copy_whole(from, &to)?;
    }
    *noted = want.into_iter().map(|(rel, (_, sha))| (rel, sha)).collect();
    Ok(true)
}

/// `dest`'s file `rel` removed, and the folders up to `dest` (itself too) that leaves empty.
fn remove_noted(dest: &Path, rel: &str) {
    let path = dest.join(rel);
    let _ = std::fs::remove_file(&path);
    for dir in path.ancestors().skip(1) {
        if !dir.starts_with(dest) || std::fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

/// nEVR's install record in its root (`installer\nevr-install.txt`, nevr-runtime's
/// launcher-install-root.md): `key=value` lines, UTF-8 without a BOM.
const INSTALL_RECORD: &str = "installer/nevr-install.txt";
/// Its `owner` for the launcher's installs: nEVR's install.ps1 and uninstall.ps1 leave a
/// record with another owner than theirs alone.
const OWNER: &str = "launcher";

/// Pure: the record for the install of host `host` (sha256 `host_sha`) with runtime
/// `runtime_sha` from the game in `bin`, into `root`.
fn install_record(
    root: &Path,
    bin: &Path,
    version: &str,
    host_sha: &str,
    runtime_sha: &str,
) -> String {
    [
        ("backup", ""),
        ("legacy", ""),
        ("runtime_aside", ""),
        ("runtime_legacy", ""),
        ("runtime_root", &as_the_game_sees(root)),
        ("game_dir", &as_the_game_sees(bin)),
        ("release", version),
        ("installed_sha256", host_sha),
        ("runtime_sha256", runtime_sha),
        ("owner", OWNER),
    ]
    .iter()
    .map(|(k, v)| format!("{k}={v}\n"))
    .collect()
}

/// `path` as the game sees it: under Wine (Linux) its Windows path (`C:` for the prefix's
/// own `drive_c`, else `Z:`).
fn as_the_game_sees(path: &Path) -> String {
    if !cfg!(target_os = "linux") {
        return path.to_string_lossy().into_owned();
    }
    let drive_c: Vec<(char, PathBuf)> = path
        .ancestors()
        .find(|a| a.file_name().is_some_and(|n| n == "drive_c"))
        .map(|c| vec![('c', c.to_path_buf())])
        .unwrap_or_default();
    crate::core::linux::echoxr::wine_path(&drive_c, path)
}

/// Notes in `root` that the launcher installed nEVR there (the host `host` in `bin`, runtime
/// `runtime_sha`), only when that changed. A record of nEVR's own installer (another owner)
/// goes to `installer\aside` first, as nEVR's scripts set files aside. Only logged when it
/// fails: the start doesn't need it.
pub fn note_install(root: &Path, bin: &Path, host: &[u8], runtime_sha: &str) {
    let path = root.join(INSTALL_RECORD);
    let version = nevr::version_in(host).unwrap_or_default();
    let text = install_record(
        root,
        bin,
        &version,
        &crate::core::pc_update::sha256_hex(host),
        runtime_sha,
    );
    let there = std::fs::read_to_string(&path).ok();
    if there.as_deref() == Some(text.as_str()) {
        return;
    }
    let result = (|| -> Result<()> {
        if there
            .as_deref()
            .is_some_and(|t| !t.lines().any(|l| l == format!("owner={OWNER}")))
        {
            let aside = root.join("installer").join("aside").join(format!(
                "nevr-install.txt.launcher-{}",
                time::OffsetDateTime::now_utc().unix_timestamp()
            ));
            std::fs::create_dir_all(aside.parent().unwrap_or(root))?;
            std::fs::rename(&path, &aside)?;
            tracing::info!(
                "nEVR's root: set aside the record of nEVR's installer ({})",
                aside.display()
            );
        }
        std::fs::create_dir_all(path.parent().unwrap_or(root))?;
        std::fs::write(&path, &text)?;
        Ok(())
    })();
    if let Err(e) = result {
        tracing::warn!("nEVR's root: couldn't write {}: {e:#}", path.display());
    }
}

/// The files under `dir` (its subfolders too), as `prefix` + their path with `/` → (path,
/// sha256).
fn files_in(dir: &Path, prefix: &str, out: &mut BTreeMap<String, (PathBuf, String)>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for e in entries.flatten() {
        let path = e.path();
        let rel = format!("{prefix}{}", e.file_name().to_string_lossy());
        if path.is_dir() {
            files_in(&path, &format!("{rel}/"), out)?;
        } else if path.is_file() {
            let sha = sha256_of(&path).with_context(|| format!("read {}", path.display()))?;
            out.insert(rel, (path, sha));
        }
    }
    Ok(())
}

/// Takes the runtime the launcher put into the root away again (uninstall), when it is
/// still the one it put there, and the `config.yaml` it wrote there (#1255 on). Whether it
/// took the runtime.
pub fn take_away() -> bool {
    let Some(root) = root() else {
        return false;
    };
    take_away_from(&root, &paths::data_dir().join(RECORD))
}

fn take_away_from(root: &Path, record: &Path) -> bool {
    let config = root.join(nevr::CONFIG);
    if std::fs::read_to_string(&config).is_ok_and(|t| nevr::written_by_launcher(&t)) {
        let _ = std::fs::remove_file(&config);
    }
    let target = root.join(nevr::RUNTIME);
    let noted = read_record(record);
    // The plugins it put there go too (#1250 on), and its content packs.
    for r in noted.iter() {
        for rel in r.plugins.keys() {
            remove_noted(&super::mods::plugins_in(root), rel);
        }
        for rel in r.content.keys() {
            remove_noted(&root.join(super::packs::CONTENT), rel);
        }
    }
    let ours = noted.is_some_and(|r| sha256_of(&target).is_ok_and(|s| s == r.runtime_sha256));
    let _ = std::fs::remove_file(record);
    // Its install record goes too (one of another owner's stays).
    let install = root.join(INSTALL_RECORD);
    if std::fs::read_to_string(&install)
        .is_ok_and(|t| t.lines().any(|l| l == format!("owner={OWNER}")))
    {
        let _ = std::fs::remove_file(&install);
    }
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
        // Beside it only the install record, the launcher's (nEVR's scripts leave it alone).
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        let noted = std::fs::read_to_string(root.join(INSTALL_RECORD)).unwrap();
        assert!(noted.lines().any(|l| l == "owner=launcher"), "{noted}");
        assert!(
            noted.contains("runtime_sha256=") && noted.contains("release=5.17.0"),
            "{noted}"
        );
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
        // Uninstall takes ours away, and only ours (its install record too).
        assert!(take_away_from(&root, &record));
        assert!(!root.join(nevr::RUNTIME).exists());
        assert!(!root.join(INSTALL_RECORD).exists());
        std::fs::write(root.join(nevr::RUNTIME), "installer's").unwrap();
        assert!(!take_away_from(&root, &record));
        assert!(root.join(nevr::RUNTIME).exists());
        // The config.yaml it wrote there goes too (#1255 on); someone's own stays.
        std::fs::write(
            root.join(nevr::CONFIG),
            "# Written by the Echo VR launcher\n",
        )
        .unwrap();
        take_away_from(&root, &record);
        assert!(!root.join(nevr::CONFIG).exists());
        std::fs::write(root.join(nevr::CONFIG), "services: {}\n").unwrap();
        take_away_from(&root, &record);
        assert!(root.join(nevr::CONFIG).exists());
    }

    #[test]
    fn the_record_names_the_paths_the_game_sees() {
        let root = Path::new("/p/pfx/drive_c/users/steamuser/AppData/Local/nevr/echovr");
        let bin = Path::new("/data/g/bin/win10");
        if cfg!(target_os = "linux") {
            assert_eq!(
                as_the_game_sees(root),
                r"C:\users\steamuser\AppData\Local\nevr\echovr"
            );
            assert_eq!(as_the_game_sees(bin), r"Z:\data\g\bin\win10");
        } else {
            assert_eq!(as_the_game_sees(bin), bin.to_string_lossy());
        }
    }

    #[test]
    fn the_install_record_of_nevrs_installer_goes_aside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let theirs = "backup=C:\\x\nowner=nevr-install\n";
        std::fs::create_dir_all(root.join("installer")).unwrap();
        std::fs::write(root.join(INSTALL_RECORD), theirs).unwrap();
        let bin = Path::new("/g/bin/win10");
        note_install(&root, bin, format!("MZ {ID}\0").as_bytes(), "abc");
        let ours = std::fs::read_to_string(root.join(INSTALL_RECORD)).unwrap();
        assert!(
            ours.ends_with("owner=launcher\n") && ours.contains("runtime_sha256=abc\n"),
            "{ours}"
        );
        let aside: Vec<_> = std::fs::read_dir(root.join("installer/aside"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(std::fs::read_to_string(aside[0].path()).unwrap(), theirs);
        // The same install again writes nothing (nothing set aside again).
        note_install(&root, bin, format!("MZ {ID}\0").as_bytes(), "abc");
        assert_eq!(
            std::fs::read_dir(root.join("installer/aside"))
                .unwrap()
                .count(),
            1
        );
        // Uninstall leaves another owner's record.
        std::fs::write(root.join(INSTALL_RECORD), theirs).unwrap();
        take_away_from(&root, &dir.path().join("rec.json"));
        assert!(root.join(INSTALL_RECORD).is_file());
    }

    /// Before a start of a real game folder (`ECHOVR_TEST_BIN`), into a root of its own
    /// (`ECHOVR_TEST_ROOT`): `cargo test puts_a_game_folders_runtime -- --ignored`. Its
    /// `config.yaml` too when `ECHOVR_TEST_CONFIG` names one.
    #[test]
    #[ignore]
    fn puts_a_game_folders_runtime_into_its_root() {
        let bin = PathBuf::from(std::env::var("ECHOVR_TEST_BIN").unwrap());
        let root = PathBuf::from(std::env::var("ECHOVR_TEST_ROOT").unwrap());
        let record = root.with_file_name("launcher-nevr-root.json");
        put_in_place(&bin, &root, &record, false).unwrap();
        plugins_into(&bin, &root, &record, false).unwrap();
        if let Ok(config) = std::env::var("ECHOVR_TEST_CONFIG") {
            let text = std::fs::read_to_string(config).unwrap();
            println!("config: {:?}", nevr::config_into(&root, &text).unwrap());
        }
        println!("{}", std::fs::read_to_string(&record).unwrap());
    }

    /// A game folder whose runtime loads plugins from the root (#1250 on), with `plugins`.
    fn plugins_game(dir: &Path, files: &[(&str, &str)]) -> PathBuf {
        let bin = game(
            dir,
            "runtime: plugins load from the root, never from the game folder",
        );
        let plugins = super::super::mods::plugins_in(&bin);
        let _ = std::fs::remove_dir_all(&plugins);
        for (rel, text) in files {
            let p = plugins.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        bin
    }

    /// The root's `plugins`, as path → text (set-aside files by their name's start).
    fn listed(dir: &Path) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let mut all = BTreeMap::new();
        files_in(dir, "", &mut all).unwrap();
        for (rel, (path, _)) in all {
            let rel = match rel.find(".original-") {
                Some(i) => format!("{}.original", &rel[..i]),
                None => rel,
            };
            out.insert(rel, std::fs::read_to_string(path).unwrap());
        }
        out
    }

    #[test]
    fn mirrors_a_versions_plugins_into_the_root_for_a_runtime_that_loads_them_there() {
        let dir = tempfile::tempdir().unwrap();
        let (root, record) = (dir.path().join("root"), dir.path().join("rec.json"));
        let plugins = super::super::mods::plugins_in(&root);
        let a = plugins_game(
            &dir.path().join("a"),
            &[
                ("One.dll", "one a"),
                ("One.plugin.json", "{}"),
                ("Two/data.bin", "two"),
            ],
        );
        let b = plugins_game(&dir.path().join("b"), &[("One.dll", "one b")]);
        put_in_place(&a, &root, &record, false).unwrap();
        plugins_into(&a, &root, &record, false).unwrap();
        let want_a: BTreeMap<String, String> = [
            ("One.dll", "one a"),
            ("One.plugin.json", "{}"),
            ("Two/data.bin", "two"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .into();
        assert_eq!(listed(&plugins), want_a);
        // The game folder's own stay: they are what the Mods page keeps.
        assert!(super::super::mods::plugins_in(&a).join("One.dll").is_file());
        // The same set again changes nothing, even while Echo VR runs; another set waits.
        plugins_into(&a, &root, &record, true).unwrap();
        assert!(plugins_into(&b, &root, &record, true).is_err());
        assert_eq!(listed(&plugins), want_a);
        // Another version's: what it lacks of ours goes; someone else's file stays.
        std::fs::write(plugins.join("Mine.dll"), "mine").unwrap();
        plugins_into(&b, &root, &record, false).unwrap();
        let want_b: BTreeMap<String, String> = [("One.dll", "one b"), ("Mine.dll", "mine")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .into();
        assert_eq!(listed(&plugins), want_b);
        // The runtime's record survives the plugins' (and the other way round).
        put_in_place(&b, &root, &record, false).unwrap();
        let noted = read_record(&record).unwrap();
        assert_eq!(noted.plugins.len(), 1);
        // A file of one of ours' names that it didn't put there is set aside, not replaced.
        std::fs::write(plugins.join("One.plugin.json"), "installer's").unwrap();
        plugins_into(&a, &root, &record, false).unwrap();
        let mut want = want_a.clone();
        want.insert("Mine.dll".into(), "mine".into());
        want.insert("One.plugin.json.original".into(), "installer's".into());
        assert_eq!(listed(&plugins), want);
        // One of ours a plugin changed since is just put back.
        std::fs::write(plugins.join("One.plugin.json"), "changed").unwrap();
        plugins_into(&a, &root, &record, false).unwrap();
        assert_eq!(listed(&plugins), want);
        // Uninstall takes away what it put there, and nothing else.
        assert!(take_away_from(&root, &record));
        let left: BTreeMap<String, String> = [
            ("Mine.dll", "mine"),
            ("One.plugin.json.original", "installer's"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .into();
        assert_eq!(listed(&plugins), left);
    }

    /// `plugins_game` with content packs in `bin/content` too.
    fn content_game(dir: &Path, content: &[(&str, &str)]) -> PathBuf {
        let bin = plugins_game(dir, &[("NvrContentOverlay.dll", "overlay")]);
        for (rel, text) in content {
            let p = bin.join(super::super::packs::CONTENT).join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        bin
    }

    fn texts(files: &[(&str, &str)]) -> BTreeMap<String, String> {
        files
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn mirrors_a_versions_content_packs_into_the_root_for_a_runtime_that_loads_plugins_there() {
        let dir = tempfile::tempdir().unwrap();
        let (root, record) = (dir.path().join("root"), dir.path().join("rec.json"));
        let content = root.join(super::super::packs::CONTENT);
        let a = content_game(
            &dir.path().join("a"),
            &[
                ("combat/content.json", "{a}"),
                ("combat/packages/p_1", "pkg a"),
            ],
        );
        let b = content_game(&dir.path().join("b"), &[("other/content.json", "{b}")]);
        put_in_place(&a, &root, &record, false).unwrap();
        plugins_into(&a, &root, &record, false).unwrap();
        let want_a = texts(&[
            ("combat/content.json", "{a}"),
            ("combat/packages/p_1", "pkg a"),
        ]);
        assert_eq!(listed(&content), want_a);
        // The game folder's stay: they are what the Mods page keeps.
        assert!(a.join("content/combat/content.json").is_file());
        // The same set again changes nothing, even while Echo VR runs; another set waits,
        // though its plugins are the same.
        plugins_into(&a, &root, &record, true).unwrap();
        assert!(plugins_into(&b, &root, &record, true).is_err());
        assert_eq!(listed(&content), want_a);
        // Another version's: the pack it lacks goes (its folders too); someone else's stays.
        std::fs::create_dir_all(content.join("theirs")).unwrap();
        std::fs::write(content.join("theirs/content.json"), "theirs").unwrap();
        plugins_into(&b, &root, &record, false).unwrap();
        let want_b = texts(&[
            ("other/content.json", "{b}"),
            ("theirs/content.json", "theirs"),
        ]);
        assert_eq!(listed(&content), want_b);
        assert!(!content.join("combat").exists());
        // The runtime's record keeps the content's (a runtime put there again rewrites it).
        std::fs::remove_file(root.join(nevr::RUNTIME)).unwrap();
        put_in_place(&b, &root, &record, false).unwrap();
        assert_eq!(read_record(&record).unwrap().content.len(), 1);
        // A file of one of ours' names that it didn't put there is set aside, not replaced.
        std::fs::create_dir_all(content.join("combat")).unwrap();
        std::fs::write(content.join("combat/content.json"), "installer's").unwrap();
        plugins_into(&a, &root, &record, false).unwrap();
        let mut want = want_a.clone();
        want.insert("theirs/content.json".into(), "theirs".into());
        want.insert("combat/content.json.original".into(), "installer's".into());
        assert_eq!(listed(&content), want);
        // Uninstall takes away what it put there, and nothing else.
        assert!(take_away_from(&root, &record));
        let left = texts(&[
            ("theirs/content.json", "theirs"),
            ("combat/content.json.original", "installer's"),
        ]);
        assert_eq!(listed(&content), left);
        assert!(!content.join("combat/packages").exists());
    }

    #[test]
    fn removing_a_noted_file_takes_the_folders_it_empties_up_to_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("up/content");
        std::fs::create_dir_all(dest.join("p/packages")).unwrap();
        std::fs::write(dest.join("p/packages/x"), "x").unwrap();
        std::fs::write(dest.join("p/content.json"), "{}").unwrap();
        remove_noted(&dest, "p/packages/x");
        assert!(!dest.join("p/packages").exists());
        assert!(dest.join("p/content.json").is_file());
        remove_noted(&dest, "p/content.json");
        assert!(!dest.exists());
        assert!(dir.path().join("up").is_dir());
    }

    #[test]
    fn makes_no_content_folder_in_the_root_without_packs() {
        let dir = tempfile::tempdir().unwrap();
        let (root, record) = (dir.path().join("root"), dir.path().join("rec.json"));
        let bin = plugins_game(dir.path(), &[("One.dll", "one")]);
        plugins_into(&bin, &root, &record, false).unwrap();
        assert!(super::super::mods::plugins_in(&root).is_dir());
        assert!(!root.join(super::super::packs::CONTENT).exists());
        // The last pack going takes the root's content folder with it.
        let with = content_game(&dir.path().join("w"), &[("p/content.json", "{}")]);
        plugins_into(&with, &root, &record, false).unwrap();
        assert!(root.join("content/p/content.json").is_file());
        plugins_into(&bin, &root, &record, false).unwrap();
        assert!(!root.join(super::super::packs::CONTENT).exists());
    }

    #[test]
    fn leaves_the_roots_plugins_alone_for_a_runtime_that_loads_them_beside_the_game() {
        let dir = tempfile::tempdir().unwrap();
        let (root, record) = (dir.path().join("root"), dir.path().join("rec.json"));
        let bin = game(dir.path(), "runtime 5.18");
        std::fs::create_dir_all(super::super::mods::plugins_in(&bin)).unwrap();
        std::fs::write(super::super::mods::plugins_in(&bin).join("One.dll"), "one").unwrap();
        std::fs::create_dir_all(bin.join("content/combat")).unwrap();
        std::fs::write(bin.join("content/combat/content.json"), "{}").unwrap();
        plugins_into(&bin, &root, &record, false).unwrap();
        assert!(!root.exists());
        // A record from before the plugins (alpha.5's) still reads.
        std::fs::write(&record, r#"{"runtime_sha256": "ab", "from": "x"}"#).unwrap();
        assert!(read_record(&record).is_some_and(|r| r.plugins.is_empty()));
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
