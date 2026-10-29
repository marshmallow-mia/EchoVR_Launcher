//! A version's nEVR in the launcher's own folder (`<data>/nevr/<id>`), not in its game folder.
//!
//! nEVR's own layout (nevr-runtime's launcher-install-root.md) puts only its host,
//! `BugSplat64.dll`, into the game folder: the runtime and the plugins are in nEVR's root,
//! one for every Echo VR of a Windows account. The launcher keeps several versions, each
//! with its own nEVR and mods, so each version's are kept here and put into the root before
//! its start ([`super::nevr_root`]): its runtime (`nevr-runtime.dll`), its mods (`plugins/`,
//! the Mods page's), and the launcher's notes on it (its Mods page choices, what an update
//! shipped and kept for Undo, the `config.yaml` it writes, a host step's folders).
//!
//! A version whose nEVR loads plugins only from the root (nevr-runtime #1250 on) moves here
//! once and for good ([`move_here`]): at the launcher's start, before each start of it and
//! after each update. What the old layout left in its game folder and nothing reads any more
//! (the launcher's `config.yaml`s there, a runtime beside the host, the last update's Undo)
//! goes to `<data>/archived/<id>/<date>/`, never deleted. There is no going back across the
//! move: an older nEVR loads its plugins from beside the game, so an update or Undo that
//! would put one in is refused.
//!
//! What stays in the game folder is what the game, nEVR or a plugin read there: the host,
//! the game's own `_local/config.json`, `log_filter_config.json` (nEVR reads it beside the
//! exe), `asset_patches/` (NvrAssetPatches does), the content packs and the plugins' logs.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::nevr;
use super::store::InstalledVersion;
use crate::core::paths;

/// The folder of every version's, in the launcher's data folder.
const HOMES: &str = "nevr";
/// What the old layout left, in the launcher's data folder.
const ARCHIVED: &str = "archived";
/// A version's folder is in use once this is in it: which game folder it is for.
const STRUCTURE: &str = "structure.json";
/// The mods' folder in it (as in the game folder before).
const PLUGINS: &str = "plugins";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Structure {
    /// The game folder's `bin/win10` (normalized).
    bin: String,
    /// The version's id when it moved here.
    id: String,
    /// When (RFC 3339).
    at: String,
}

/// The launcher's data folder (in tests: one a test sets, else none, so every version keeps
/// the old layout).
fn data() -> Option<PathBuf> {
    #[cfg(test)]
    return TEST_DATA.with(|d| d.borrow().clone());
    #[cfg(not(test))]
    Some(paths::data_dir())
}

#[cfg(test)]
thread_local! {
    static TEST_DATA: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Tests: the launcher's data folder is `dir` on this thread.
#[cfg(test)]
pub fn set_test_data(dir: Option<&Path>) {
    TEST_DATA.with(|d| *d.borrow_mut() = dir.map(Path::to_path_buf));
}

/// Tests: `bin` has moved to a folder in `data` (the test's data folder, [`set_test_data`]).
#[cfg(test)]
pub fn moved_for_test(data: &Path, bin: &Path) -> PathBuf {
    let home = data.join(HOMES).join("test");
    std::fs::create_dir_all(&home).unwrap();
    let s = Structure {
        bin: key(bin),
        id: "test".into(),
        at: String::new(),
    };
    std::fs::write(home.join(STRUCTURE), serde_json::to_vec(&s).unwrap()).unwrap();
    home
}

fn same_bin(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn key(bin: &Path) -> String {
    paths::normalize(&bin.to_string_lossy())
}

fn read_structure(dir: &Path) -> Option<Structure> {
    let t = std::fs::read_to_string(dir.join(STRUCTURE)).ok()?;
    serde_json::from_str(&t).ok()
}

/// The folder of the version whose game is in `bin`, once it moved here; None while it keeps
/// the old layout (and for anything that is no version's: nEVR's root, a server's copy).
pub fn of_bin(bin: &Path) -> Option<PathBuf> {
    let homes = data()?.join(HOMES);
    let want = key(bin);
    std::fs::read_dir(homes)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|dir| read_structure(dir).is_some_and(|s| same_bin(&s.bin, &want)))
}

/// [`of_bin`] for `v`.
pub fn of(v: &InstalledVersion) -> Option<PathBuf> {
    of_bin(&v.bin_dir())
}

/// Where the launcher keeps its notes on the game in `bin` (choices, Undo, `config.yaml`):
/// its folder here, else the game's `_local` (the old layout).
pub fn notes_for_bin(bin: &Path) -> PathBuf {
    of_bin(bin).unwrap_or_else(|| {
        bin.parent()
            .and_then(Path::parent)
            .unwrap_or(bin)
            .join("_local")
    })
}

/// The version's mods (nEVR's `plugins`) in its folder `home`.
pub fn plugins_in_home(home: &Path) -> PathBuf {
    home.join(PLUGINS)
}

/// The runtime kept for nEVR's root in its folder `home`.
pub fn runtime_in_home(home: &Path) -> PathBuf {
    home.join(nevr::RUNTIME)
}

/// Where the update's file `path` (relative to `bin`, as the update manifest has it) is for
/// the game in `bin` whose folder here is `home`: its runtime and plugins in `home`, the rest
/// in `bin`.
pub fn at(bin: &Path, home: Option<&Path>, path: &str) -> PathBuf {
    if let Some(home) = home {
        if path.eq_ignore_ascii_case(nevr::ROOT_COPY) {
            return runtime_in_home(home);
        }
        let lower = path.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("plugins/") {
            return plugins_in_home(home).join(&path[path.len() - rest.len()..]);
        }
    }
    bin.join(path)
}

/// Whether the runtime in `bytes` needs the new layout: it loads plugins only from nEVR's
/// root (nevr-runtime #1250 on).
pub fn needs_home(runtime: &[u8]) -> bool {
    nevr::loads_root_plugins(runtime)
}

/// What [`move_here`] did.
#[derive(Debug, Default, PartialEq)]
pub struct Moved {
    pub home: PathBuf,
    /// Files and folders that moved into `home`.
    pub moved: Vec<String>,
    /// What went to the archive.
    pub archived: Vec<String>,
    pub archive: Option<PathBuf>,
}

/// Moves `v` to the new layout ([`move_into`]) when its nEVR needs it, at the launcher's
/// start, before a start of it, after an update. Nothing while Echo VR runs (its files may
/// be held): the next start does it.
pub fn move_here(v: &InstalledVersion) -> Result<Option<Moved>> {
    if v.publisher_lock.is_some() {
        return Ok(None);
    }
    let Some(data) = data() else {
        return Ok(None);
    };
    let bin = v.bin_dir();
    let home = match of_bin(&bin) {
        Some(home) => home,
        None => {
            let runtime = std::fs::read(bin.join(nevr::ROOT_COPY)).unwrap_or_default();
            if !needs_home(&runtime) {
                return Ok(None);
            }
            free_home(&data.join(HOMES), &v.id, &bin)
        }
    };
    let game = Path::new(&v.root).join(paths::ARENA_DIR);
    let plan = plan(&game, &bin, &home);
    if plan.is_empty() && home.join(STRUCTURE).is_file() {
        return Ok(None);
    }
    if !super::game::running_now().is_empty() {
        bail!(
            "Echo VR is running: the launcher moves {}'s nEVR and mods once it is closed.",
            v.name
        );
    }
    let archive = data.join(ARCHIVED).join(safe_name(&v.id)).join(today());
    let root = super::nevr_root::root();
    let done = move_into(&game, &bin, &home, &archive, &v.id, plan, root.as_deref())?;
    tracing::info!(
        "{}: nEVR and its mods are in {} now ({} moved, {} archived in {})",
        v.id,
        done.home.display(),
        done.moved.len(),
        done.archived.len(),
        archive.display()
    );
    Ok(Some(done))
}

/// [`move_here`] for every installed version (at the launcher's start). What failed is
/// logged; the next start of that version tries again.
pub fn move_all(state: &super::store::LauncherState) {
    for v in &state.versions {
        if let Err(e) = move_here(v) {
            tracing::warn!("{}: not moved to the new layout yet: {e:#}", v.id);
        }
    }
}

/// A folder for `id` in `homes`: its own name, else (taken by another game folder's) with a
/// number.
fn free_home(homes: &Path, id: &str, bin: &Path) -> PathBuf {
    let name = safe_name(id);
    let want = key(bin);
    (1..)
        .map(|n| {
            if n == 1 {
                homes.join(&name)
            } else {
                homes.join(format!("{name}-{n}"))
            }
        })
        .find(|dir| read_structure(dir).is_none_or(|s| same_bin(&s.bin, &want)))
        .unwrap_or_else(|| homes.join(&name))
}

/// `id` as a folder name.
fn safe_name(id: &str) -> String {
    let s: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() || s.chars().all(|c| c == '.') {
        "version".into()
    } else {
        s
    }
}

fn today() -> String {
    let d = time::OffsetDateTime::now_utc().date();
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// What moves where: into the version's folder, or to the archive.
#[derive(Debug, Default, PartialEq)]
struct Plan {
    /// (from, to in the version's folder).
    into_home: Vec<(PathBuf, PathBuf)>,
    /// From, archived at its path in the game folder.
    archive: Vec<PathBuf>,
    /// The game's sign-in in `_local`, for nEVR's root when that has none yet.
    login: Option<PathBuf>,
}

impl Plan {
    fn is_empty(&self) -> bool {
        self.into_home.is_empty() && self.archive.is_empty()
    }
}

/// The launcher's notes in `_local` that move with the version: file or folder names.
const NOTES: [&str; 5] = [
    super::mods::CHOICES,
    super::mods::SHIPPED_NOTE,
    "launcher-left-out.json",
    super::mods::QUARANTINE,
    "previous-mods",
];

/// Pure but for reading the folders: what the old layout has in `game` (its `bin`) that moves.
fn plan(game: &Path, bin: &Path, home: &Path) -> Plan {
    let local = game.join("_local");
    let mut p = Plan::default();
    let plugins = bin.join(PLUGINS);
    if plugins.is_dir() {
        p.into_home.push((plugins, plugins_in_home(home)));
    }
    for name in NOTES {
        let from = local.join(name);
        if from.exists() {
            p.into_home.push((from, home.join(name)));
        }
    }
    // The runtime last: until it moved, the old layout still has what a start needs.
    let runtime = bin.join(nevr::ROOT_COPY);
    if runtime.is_file() {
        p.into_home.push((runtime, runtime_in_home(home)));
    }
    // The launcher's config.yaml: nEVR 6.0 reads only the root's (it says to delete this one).
    let config = local.join(nevr::CONFIG);
    if std::fs::read_to_string(&config).is_ok_and(|t| nevr::written_by_launcher(&t)) {
        p.into_home.push((config, home.join(nevr::CONFIG)));
    }
    // A host step's folders (its config.yaml, config.json and start script).
    for e in std::fs::read_dir(&local).into_iter().flatten().flatten() {
        let dir = e.path();
        if dir.is_dir()
            && ["start-server.cmd", "start-server.sh"]
                .iter()
                .any(|s| dir.join(s).is_file())
        {
            p.into_home.push((dir, home.join(e.file_name())));
        }
    }
    // What nothing reads any more: the archive.
    let up = bin.parent().unwrap_or(bin).join("_local");
    for dir in [local.clone(), bin.join("_local"), up] {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let config = name == nevr::CONFIG || name.starts_with("config.yaml.");
            if config && e.path().is_file() && !p.into_home.iter().any(|(f, _)| *f == e.path()) {
                p.archive.push(e.path());
            }
        }
    }
    for name in ["previous-update", "previous-update.new"] {
        if local.join(name).exists() {
            p.archive.push(local.join(name));
        }
    }
    for name in [nevr::CREDENTIALS, nevr::CREDENTIALS_ASIDE] {
        let f = local.join(name);
        if f.is_file() {
            if name == nevr::CREDENTIALS {
                p.login = Some(f.clone());
            }
            p.archive.push(f);
        }
    }
    for e in std::fs::read_dir(bin).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        let old_runtime = lower == nevr::RUNTIME || lower.starts_with("nevr-runtime.dll.legacy-");
        let staged = lower.starts_with('.') && lower.ends_with(".download");
        if (old_runtime || staged) && e.path().is_file() {
            p.archive.push(e.path());
        }
    }
    // The old copy's folder, with anything else left in it.
    let root_copy = bin.join("nevr-root");
    if root_copy.is_dir() {
        p.archive.push(root_copy);
    }
    p
}

/// Does `plan`: the version's runtime, mods and notes into `home`, what the old layout left
/// to `archive` (at its path in `game`), then `home` noted as `bin`'s. A file of the same
/// name in `home` is replaced by the game folder's while the move isn't done (one cut short);
/// once it is, the version's own stays and the game folder's (an older launcher's) goes to the
/// archive. The game's sign-in goes into nEVR's root (`root`) first when that has none.
fn move_into(
    game: &Path,
    bin: &Path,
    home: &Path,
    archive: &Path,
    id: &str,
    plan: Plan,
    root: Option<&Path>,
) -> Result<Moved> {
    std::fs::create_dir_all(home).with_context(|| format!("create {}", home.display()))?;
    let mut done = Moved {
        home: home.to_path_buf(),
        ..Moved::default()
    };
    let moved = home.join(STRUCTURE).is_file();
    for (from, to) in &plan.into_home {
        let clash = moved.then(|| archive.join(rel(game, from)));
        move_merging(from, to, clash.as_deref())?;
        done.moved.push(rel(game, from));
    }
    // The old copy's folder, once its runtime moved and nothing else is in it, just goes.
    let _ = std::fs::remove_dir(bin.join("nevr-root"));
    // The root is made for it: an nEVR from before the root (5.2.1) never had one.
    if let (Some(login), Some(root)) = (&plan.login, root) {
        let there = root.join(nevr::CREDENTIALS);
        if !there.exists() && std::fs::create_dir_all(root).is_ok() {
            let _ = std::fs::copy(login, &there);
        }
    }
    for from in &plan.archive {
        if !from.exists() {
            continue;
        }
        let to = archive.join(rel(game, from));
        move_merging(from, &to, None)?;
        done.archived.push(rel(game, from));
    }
    if !done.archived.is_empty() {
        done.archive = Some(archive.to_path_buf());
    }
    let s = Structure {
        bin: key(bin),
        id: id.to_string(),
        at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    };
    if read_structure(home).is_none_or(|old| old.bin != s.bin) {
        let path = home.join(STRUCTURE);
        std::fs::write(&path, serde_json::to_vec_pretty(&s)?)
            .with_context(|| format!("write {}", path.display()))?;
    }
    Ok(done)
}

/// `path` relative to `game` with `/`, else its name.
fn rel(game: &Path, path: &Path) -> String {
    path.strip_prefix(game)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// Moves `from` (a file or a folder) to `to`, merging into a folder there: a file of the same
/// name is replaced, or with `clash` kept, `from`'s going to `clash` instead. Across drives a
/// copy, then the delete.
fn move_merging(from: &Path, to: &Path, clash: Option<&Path>) -> Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
        for e in std::fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
            let e = e?;
            let clash = clash.map(|c| c.join(e.file_name()));
            move_merging(&e.path(), &to.join(e.file_name()), clash.as_deref())?;
        }
        return std::fs::remove_dir(from).with_context(|| format!("remove {}", from.display()));
    }
    if let Some(clash) = clash.filter(|_| to.is_file()) {
        return move_merging(from, clash, None);
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    if to.is_dir() {
        std::fs::remove_dir_all(to).with_context(|| format!("remove {}", to.display()))?;
    } else if to.exists() {
        // Windows won't rename over a hidden file (nEVR hides its sign-in).
        std::fs::remove_file(to).with_context(|| format!("remove {}", to.display()))?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)
        .with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
    std::fs::remove_file(from).with_context(|| format!("remove {}", from.display()))
}

/// Takes `v`'s folder away (uninstall: the launcher lets go of `v`). What couldn't go.
pub fn remove(v: &InstalledVersion) -> Option<String> {
    let home = of(v)?;
    std::fs::remove_dir_all(&home)
        .err()
        .map(|e| format!("{}: {e}", home.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNTIME_6: &str =
        "NEVR-BUILD 6.0.0 049d5ebc plugins load from X, never from the game folder";

    /// A version on the old layout in `dir`: nEVR 6.0's pair, mods, notes and leftovers.
    fn old_layout(dir: &Path) -> InstalledVersion {
        let root = dir.join("game");
        let game = root.join(paths::ARENA_DIR);
        let bin = game.join("bin/win10");
        let local = game.join("_local");
        for d in [
            bin.join("nevr-root"),
            bin.join("plugins/sub"),
            local.join("previous-update/files"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let w = |p: PathBuf, t: &str| std::fs::write(p, t).unwrap();
        w(bin.join("echovr.exe"), "exe");
        w(
            bin.join(super::super::mods::SLOT),
            "MZ nevr_root_override NEVR-BUILD 6.0.0 x",
        );
        w(bin.join(nevr::ROOT_COPY), RUNTIME_6);
        w(bin.join("plugins/NvrAssetPatches.dll"), "assets");
        w(bin.join("plugins/sub/data.bin"), "data");
        w(
            bin.join("nevr-runtime.dll.legacy-1700000000"),
            "old runtime",
        );
        w(bin.join("log_filter_config.json"), "{}");
        w(local.join(super::super::mods::CHOICES), "{}");
        w(
            local.join(nevr::CONFIG),
            "# Written by the Echo VR launcher\nplugins: []\n",
        );
        w(local.join("config.yaml.old"), "mine: 1\n");
        w(
            local.join("config.json"),
            "{\"publisher_lock\": \"echovrce\"}",
        );
        w(local.join("previous-update/previous.json"), "{}");
        w(local.join(nevr::CREDENTIALS), "{}");
        InstalledVersion {
            id: "main".into(),
            name: "Main".into(),
            root: paths::normalize(&root.to_string_lossy()),
            ..InstalledVersion::default()
        }
    }

    #[test]
    fn a_version_on_nevr_6_moves_out_of_its_game_folder() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        set_test_data(Some(&data));
        let v = old_layout(dir.path());
        let bin = v.bin_dir();
        let game = Path::new(&v.root).join(paths::ARENA_DIR);
        assert_eq!(of(&v), None);
        let plan = plan(&game, &bin, &data.join("nevr/main"));
        // nEVR's root isn't there yet (5.2.1 had none).
        let root = dir.path().join("local/nevr/echovr");
        let done = move_into(
            &game,
            &bin,
            &data.join("nevr/main"),
            &data.join("archived/main/d"),
            "main",
            plan,
            Some(&root),
        )
        .unwrap();
        let home = data.join("nevr/main");
        assert_eq!(of(&v), Some(home.clone()));
        // Its runtime, mods and notes in its folder.
        assert_eq!(
            std::fs::read_to_string(home.join(nevr::RUNTIME)).unwrap(),
            RUNTIME_6
        );
        assert!(home.join("plugins/NvrAssetPatches.dll").is_file());
        assert!(home.join("plugins/sub/data.bin").is_file());
        assert!(home.join(super::super::mods::CHOICES).is_file());
        assert!(home.join(nevr::CONFIG).is_file());
        // What nothing reads any more: archived at its place in the game folder.
        let a = data.join("archived/main/d");
        assert!(a.join("_local/config.yaml.old").is_file());
        assert!(a.join("_local/previous-update/previous.json").is_file());
        assert!(a
            .join("bin/win10/nevr-runtime.dll.legacy-1700000000")
            .is_file());
        assert!(a.join("_local/.credentials.json").is_file());
        // The sign-in went into nEVR's root first.
        assert!(root.join(nevr::CREDENTIALS).is_file());
        assert_eq!(done.archive.as_deref(), Some(a.as_path()));
        // Of the launcher's, only the host is left in the game folder; the game's and the
        // plugins' own files stay.
        assert!(bin.join(super::super::mods::SLOT).is_file());
        assert!(bin.join("log_filter_config.json").is_file());
        assert!(game.join("_local/config.json").is_file());
        for gone in ["plugins", "nevr-root", "nevr-runtime.dll.legacy-1700000000"] {
            assert!(!bin.join(gone).exists(), "{gone}");
            assert!(!a.join("bin/win10").join(gone).is_dir(), "{gone} archived");
        }
        for gone in [
            nevr::CONFIG,
            "config.yaml.old",
            "previous-update",
            super::super::mods::CHOICES,
        ] {
            assert!(!game.join("_local").join(gone).exists(), "{gone}");
        }
        // Once moved, nothing is left to do, and the paths are its folder's.
        assert!(plan_of(&v).is_empty());
        assert_eq!(super::super::mods::plugins_dir(&v), home.join("plugins"));
        assert_eq!(
            super::super::mods::choices_path(&v),
            home.join(super::super::mods::CHOICES)
        );
        assert_eq!(nevr::root_copy(&bin), home.join(nevr::RUNTIME));
        set_test_data(None);
    }

    fn plan_of(v: &InstalledVersion) -> Plan {
        let bin = v.bin_dir();
        plan(
            &Path::new(&v.root).join(paths::ARENA_DIR),
            &bin,
            &of(v).unwrap(),
        )
    }

    #[test]
    fn an_older_nevr_keeps_the_old_layout() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        set_test_data(Some(&data));
        let v = old_layout(dir.path());
        std::fs::write(v.bin_dir().join(nevr::ROOT_COPY), "NEVR-BUILD 5.18.0 x").unwrap();
        assert_eq!(move_here(&v).unwrap(), None);
        assert!(v.bin_dir().join("plugins/NvrAssetPatches.dll").is_file());
        assert!(!data.join(HOMES).exists());
        // Event builds never move.
        let event = InstalledVersion {
            publisher_lock: Some("rad15_live".into()),
            ..v.clone()
        };
        assert_eq!(move_here(&event).unwrap(), None);
        set_test_data(None);
    }

    #[test]
    fn a_move_cut_short_goes_on_where_it_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        set_test_data(Some(&data));
        let v = old_layout(dir.path());
        let home = data.join("nevr/main");
        // A first try moved the runtime and a plugin, then stopped before noting the folder.
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        std::fs::rename(v.bin_dir().join(nevr::ROOT_COPY), home.join(nevr::RUNTIME)).unwrap();
        std::fs::write(home.join("plugins/NvrAssetPatches.dll"), "older").unwrap();
        // Not noted yet: the old layout's paths still count, and the free folder is the same.
        assert_eq!(of(&v), None);
        assert_eq!(free_home(&data.join(HOMES), "main", &v.bin_dir()), home);
        let p = plan(
            &Path::new(&v.root).join(paths::ARENA_DIR),
            &v.bin_dir(),
            &home,
        );
        let game = Path::new(&v.root).join(paths::ARENA_DIR);
        move_into(
            &game,
            &v.bin_dir(),
            &home,
            &data.join("archived/main/d"),
            "main",
            p,
            None,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join("plugins/NvrAssetPatches.dll")).unwrap(),
            "assets"
        );
        assert_eq!(
            std::fs::read_to_string(home.join(nevr::RUNTIME)).unwrap(),
            RUNTIME_6
        );
        assert_eq!(of(&v), Some(home));
        set_test_data(None);
    }

    #[test]
    fn once_moved_the_versions_own_files_win_over_an_older_launchers() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        set_test_data(Some(&data));
        let v = old_layout(dir.path());
        let (bin, game) = (v.bin_dir(), Path::new(&v.root).join(paths::ARENA_DIR));
        let home = data.join("nevr/main");
        let p = plan(&game, &bin, &home);
        move_into(
            &game,
            &bin,
            &home,
            &data.join("archived/main/1"),
            "main",
            p,
            None,
        )
        .unwrap();
        // An older launcher put the old layout's files back.
        std::fs::create_dir_all(bin.join("plugins")).unwrap();
        std::fs::write(bin.join("plugins/NvrAssetPatches.dll"), "older launcher's").unwrap();
        std::fs::write(bin.join("plugins/New.dll"), "new").unwrap();
        std::fs::write(
            game.join("_local").join(super::super::mods::CHOICES),
            "older",
        )
        .unwrap();
        let p = plan_of(&v);
        move_into(
            &game,
            &bin,
            &home,
            &data.join("archived/main/2"),
            "main",
            p,
            None,
        )
        .unwrap();
        let read = |p: PathBuf| std::fs::read_to_string(p).unwrap();
        assert_eq!(read(home.join("plugins/NvrAssetPatches.dll")), "assets");
        assert_eq!(read(home.join(super::super::mods::CHOICES)), "{}");
        assert_eq!(read(home.join("plugins/New.dll")), "new");
        let a = data.join("archived/main/2");
        assert_eq!(
            read(a.join("bin/win10/plugins/NvrAssetPatches.dll")),
            "older launcher's"
        );
        assert_eq!(
            read(a.join("_local").join(super::super::mods::CHOICES)),
            "older"
        );
        assert!(!bin.join("plugins").exists());
        set_test_data(None);
    }

    #[test]
    fn another_game_folder_with_the_same_id_gets_a_folder_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let homes = dir.path().join("nevr");
        std::fs::create_dir_all(homes.join("main")).unwrap();
        let s = Structure {
            bin: "/elsewhere/bin/win10".into(),
            ..Structure::default()
        };
        std::fs::write(
            homes.join("main").join(STRUCTURE),
            serde_json::to_vec(&s).unwrap(),
        )
        .unwrap();
        assert_eq!(
            free_home(&homes, "main", Path::new("/games/bin/win10")),
            homes.join("main-2")
        );
        assert_eq!(
            free_home(&homes, "main", Path::new("/elsewhere/bin/win10")),
            homes.join("main")
        );
        assert_eq!(safe_name("a/b:c"), "a_b_c");
        assert_eq!(safe_name(".."), "version");
    }

    #[test]
    fn an_updates_runtime_and_plugins_go_to_the_versions_folder() {
        let (bin, home) = (Path::new("/g/bin/win10"), Path::new("/d/nevr/main"));
        assert_eq!(
            at(bin, Some(home), nevr::ROOT_COPY),
            home.join(nevr::RUNTIME)
        );
        assert_eq!(
            at(bin, Some(home), "plugins/NvrAssetPatches.dll"),
            home.join("plugins/NvrAssetPatches.dll")
        );
        assert_eq!(
            at(bin, Some(home), "Plugins/X.dll"),
            home.join("plugins/X.dll")
        );
        assert_eq!(
            at(bin, Some(home), "BugSplat64.dll"),
            bin.join("BugSplat64.dll")
        );
        assert_eq!(at(bin, None, "plugins/X.dll"), bin.join("plugins/X.dll"));
        assert_eq!(at(bin, None, nevr::ROOT_COPY), bin.join(nevr::ROOT_COPY));
    }
}
