//! Uninstalling: what the launcher put on this PC, part by part (Settings → Uninstall).
//! The launcher's own program file stays until an installer handles it.
//!
//! - Echo VR's versions: the launcher's own folders are deleted; a copy it only knew of
//!   (your folder, the Meta app's) is forgotten, with what the launcher put into it taken
//!   out ([`clean_version`]).
//! - Linux: the private GE-Proton and Wine prefix, and Echo VR's shortcut in Steam.
//! - Windows: Echo VR's entry in SteamVR's library (Revive's manifest). Revive itself and
//!   the game artwork in the Meta app (which uses it too) stay.
//! - Desktop shortcuts, the spark:// link handler, the sign-ins (EchoVRCE's and the game's).
//! - The launcher's data: settings, logs, caches, downloads, and on macOS the site's
//!   storage (WebKit keeps it in ~/Library, outside the data folder).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use super::launcher::store::{InstalledVersion, LauncherState};
use super::launcher::versions::{self, Step};
use super::{echoxr, echoxr_hands, links, paths};

/// One part of what the launcher put on this PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Versions,
    LinuxSetup,
    SteamShortcut,
    SteamVrLibrary,
    DesktopShortcuts,
    LinkHandler,
    SignIn,
    LauncherData,
}

impl Part {
    /// Every part this platform has, in the order they go.
    pub fn all() -> Vec<Part> {
        let linux = cfg!(target_os = "linux");
        let mut out = vec![Part::Versions];
        if linux {
            out.extend([Part::LinuxSetup, Part::SteamShortcut]);
        }
        if cfg!(windows) {
            out.push(Part::SteamVrLibrary);
        }
        out.extend([
            Part::DesktopShortcuts,
            Part::LinkHandler,
            Part::SignIn,
            Part::LauncherData,
        ]);
        out
    }

    pub fn title(self) -> &'static str {
        match self {
            Part::Versions => "Echo VR",
            Part::LinuxSetup => "GE-Proton and the Wine prefix",
            Part::SteamShortcut => "Echo VR in Steam",
            Part::SteamVrLibrary => "Echo VR in SteamVR's library",
            Part::DesktopShortcuts => "Desktop shortcuts",
            Part::LinkHandler => "spark:// links",
            Part::SignIn => "Sign-ins",
            Part::LauncherData => "The launcher's settings, logs and caches",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Part::Versions => "Every version the launcher installed is deleted. Copies it only knew of (your own folder, the Meta app's) stay, without what the launcher put into them.",
            Part::LinuxSetup => "The private GE-Proton, Echo VR's Wine prefix and EchoXR's download.",
            Part::SteamShortcut => "The non-Steam game PLAY starts (Steam closes and restarts for that).",
            Part::SteamVrLibrary => "Revive's entry for Echo VR. Revive itself stays: uninstall it in Windows' Apps.",
            Part::DesktopShortcuts => "The Echo VR shortcuts the launcher made.",
            Part::LinkHandler => "The launcher stops opening spark:// links.",
            Part::SignIn => "EchoVRCE's sign-in, and the game's own in each version.",
            Part::LauncherData => "Everything else the launcher keeps, and its tray (with its start at login); it closes afterwards.",
        }
    }
}

/// What an uninstall did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The versions the launcher no longer has.
    pub removed: Vec<String>,
    /// What couldn't be done, to tell.
    pub notes: Vec<String>,
    /// The launcher's data is gone: it should close.
    pub data_removed: bool,
}

/// Takes out of `v`'s folder what the launcher put there: EchoXR (its files and its copy of
/// the game), EchoXR Hands' plugin, the plugins it added, its mod choices, the
/// `config.yaml` it writes and the game's sign-in; a game config it set aside comes back.
/// Meta's Platform DLLs stay (the game's own install may have them too). Returns what
/// couldn't be removed.
pub fn clean_version(v: &InstalledVersion) -> Vec<String> {
    let bin = v.bin_dir();
    let mut files: Vec<PathBuf> = vec![
        bin.join(echoxr::LAUNCHER),
        bin.join(paths::OPENXR_EXE),
        bin.join("plugins").join(echoxr_hands::PLUGIN),
    ];
    files.extend(super::launcher::mods::launcher_files(v));
    files.extend(super::launcher::nevr::logins(v));
    // The Platform SDK beside the game, when it is EchoXR's stand-in or Meta's an older
    // launcher put there (never one of the player's own).
    let platform = bin.join(echoxr::PLATFORM_DLL);
    if echoxr::is_launchers_platform(&platform) {
        files.push(platform);
    }
    let mut failed = Vec::new();
    if let Err(e) = echoxr::remove_old_meta(&bin) {
        failed.push(format!("{e:#}"));
    }
    for f in files.iter().filter(|f| f.is_file()) {
        if let Err(e) = std::fs::remove_file(f) {
            failed.push(format!("{}: {e}", f.display()));
        }
    }
    let dir = bin.join(echoxr::DIR);
    if dir.is_dir() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            failed.push(format!("{}: {e}", dir.display()));
        }
    }
    if let Err(e) = super::launcher::nevr::restore_game_config(v) {
        failed.push(format!("{e:#}"));
    }
    failed
}

/// The desktop shortcuts the launcher makes for `v` (as `setup::shortcut` names them).
pub fn shortcut_names(v: &InstalledVersion) -> Vec<String> {
    let name = match &v.publisher_lock {
        Some(_) => format!("Echo VR {}", v.name),
        None => "Echo VR".to_string(),
    };
    vec![format!("{name} (Revive)"), name]
}

/// Pure: the folder in the launcher's data folder `data` to keep because the game library
/// `library` is in it (and versions stay), if any.
pub fn kept_in_data(data: &Path, library: &Path, versions_go: bool) -> Option<PathBuf> {
    if versions_go {
        return None;
    }
    let rest = library.strip_prefix(data).ok()?;
    let first = rest.components().next()?;
    Some(data.join(first))
}

/// Deletes everything in `dir` except `keep` (going on past what can't go, which it
/// names), then `dir` itself when it's empty.
pub fn remove_dir_except(dir: &Path, keep: Option<&Path>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut failed = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if Some(path.as_path()) == keep {
            continue;
        }
        let r = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = r {
            failed.push(format!("{}: {e}", path.display()));
        }
    }
    let _ = std::fs::remove_dir(dir);
    if failed.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("couldn't remove {}", failed.join(", "))
    }
}

/// The launcher's data was removed this run: nothing may write it again.
static FINISHED: AtomicBool = AtomicBool::new(false);

pub fn finished() -> bool {
    FINISHED.load(Ordering::Relaxed)
}

/// After the window closed (it writes `window.ron` then), the data folder once more: the
/// log the launcher kept open goes too. Windows can't delete an open file, so a command
/// does that a moment after the launcher has exited; the folder itself goes only when
/// empty (a library inside it stays).
pub fn after_exit() {
    if !finished() {
        return;
    }
    let data = paths::data_dir();
    let leftovers = [data.join("logs"), data.join("window.ron")];
    if cfg!(windows) {
        let q = |p: &Path| format!("\"{}\"", p.display());
        let script = format!(
            "timeout /t 2 /nobreak >nul & rmdir /S /Q {} & del /Q {} & rmdir {}",
            q(&leftovers[0]),
            q(&leftovers[1]),
            q(&data)
        );
        let _ = crate::core::process::command("cmd")
            .args(["/C", &script])
            .spawn();
    } else {
        let _ = std::fs::remove_dir_all(&leftovers[0]);
        let _ = std::fs::remove_file(&leftovers[1]);
        let _ = std::fs::remove_dir(&data);
    }
}

/// Removes `parts`, in their order, reporting each step; `consent` before asking Windows
/// for administrator rights (SteamVR's library in Program Files).
pub fn run(
    parts: &[Part],
    state: &LauncherState,
    consent: &mut dyn FnMut() -> bool,
    on: &mut dyn FnMut(Step),
) -> Outcome {
    let mut out = Outcome::default();
    let has = |p: Part| parts.contains(&p);
    for part in Part::all().into_iter().filter(|p| has(*p)) {
        on(Step::Status(format!("Removing: {}...", part.title())));
        let r: Result<()> = match part {
            Part::Versions => {
                for v in &state.versions {
                    let r = if v.external {
                        let failed = clean_version(v);
                        out.notes.extend(failed);
                        Ok(())
                    } else {
                        versions::remove(v, &state.library)
                    };
                    match r {
                        Ok(()) => out.removed.push(v.id.clone()),
                        Err(e) => out.notes.push(format!("{}: {e:#}", v.name)),
                    }
                }
                Ok(())
            }
            Part::LinuxSetup => {
                let dir = super::linux::echoxr::root();
                if dir.is_dir() {
                    std::fs::remove_dir_all(&dir)
                        .with_context(|| format!("remove {}", dir.display()))
                } else {
                    Ok(())
                }
            }
            Part::SteamShortcut => remove_steam_shortcut(),
            Part::SteamVrLibrary => {
                if super::revive::find_revive_dir().is_some() {
                    super::elevation::set_library_entry(None, "", consent)
                } else {
                    Ok(())
                }
            }
            Part::DesktopShortcuts => {
                let mut names: Vec<String> =
                    state.versions.iter().flat_map(shortcut_names).collect();
                names.dedup();
                for n in names {
                    if let Err(e) = super::platform::remove_shortcut(&n) {
                        out.notes.push(format!("{e:#}"));
                    }
                }
                Ok(())
            }
            Part::LinkHandler => {
                links::unregister();
                Ok(())
            }
            Part::SignIn => {
                super::echovrce::store::forget();
                for v in &state.versions {
                    for f in super::launcher::nevr::logins(v) {
                        let _ = std::fs::remove_file(f);
                    }
                }
                Ok(())
            }
            Part::LauncherData => {
                // The tray goes first: it reads the data folder.
                super::tray::quit();
                if let Err(e) = super::tray::set_autostart(false) {
                    out.notes.push(format!("The tray's start at login: {e:#}"));
                }
                remove_site_data(&mut out.notes);
                let data = paths::data_dir();
                let keep = kept_in_data(&data, Path::new(&state.library), has(Part::Versions));
                let r = remove_dir_except(&data, keep.as_deref())
                    .and_then(|()| remove_dir_except(&paths::cache_dir(), None));
                let _ = std::fs::remove_dir(paths::legacy_cache_dir());
                // The log the launcher has open may stay (Windows): it goes at exit.
                let r = r.or_else(|e| {
                    if !cfg!(windows) {
                        return Err(e);
                    }
                    tracing::info!("data: {e:#}");
                    Ok(())
                });
                out.data_removed = r.is_ok();
                if out.data_removed {
                    FINISHED.store(true, Ordering::Relaxed);
                }
                r
            }
        };
        if let Err(e) = r {
            tracing::warn!("uninstall {part:?}: {e:#}");
            out.notes.push(format!("{}: {e:#}", part.title()));
        }
    }
    out
}

/// Echo VR's shortcut out of Steam: Steam closes for that, and starts again when it ran.
/// The app's bundle identifier (`scripts/package.sh`): WebKit's folders for the packaged
/// launcher are named after it; for a bare executable, after the executable.
#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "de.echovr.launcher";

/// The echovrce.com site's storage inside the window (its session, cookies and cache).
/// On Windows and Linux it is in the data folder already; on macOS WebKit keeps it in
/// ~/Library, named after the app.
fn remove_site_data(notes: &mut Vec<String>) {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let lib = home.join("Library");
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()));
        for name in [Some(BUNDLE_ID.to_string()), exe].into_iter().flatten() {
            let cookies = format!("{name}.binarycookies");
            for p in [
                lib.join("WebKit").join(&name),
                lib.join("Caches").join(&name).join("WebKit"),
                lib.join("HTTPStorages").join(&name),
                lib.join("HTTPStorages").join(&cookies),
            ] {
                let r = if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else if p.exists() {
                    std::fs::remove_file(&p)
                } else {
                    continue;
                };
                if let Err(e) = r {
                    notes.push(format!("{}: {e}", p.display()));
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = notes;
}

fn remove_steam_shortcut() -> Result<()> {
    use super::linux::steam;
    let Some(root) = steam::root() else {
        return Ok(());
    };
    let was_running = steam::running();
    steam::shutdown(&root)?;
    let r = steam::remove_shortcut(&root).map(|_| ());
    if was_running {
        let _ = steam::start(&root);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_library_inside_the_data_folder() {
        let data = Path::new("/home/u/.local/share/EchoVR_Launcher");
        let lib = data.join("versions");
        assert_eq!(kept_in_data(data, &lib, false), Some(lib.clone()));
        assert_eq!(
            kept_in_data(data, &lib.join("deeper"), false),
            Some(lib.clone())
        );
        assert_eq!(kept_in_data(data, &lib, true), None);
        assert_eq!(kept_in_data(data, Path::new("/games/echo"), false), None);
    }

    #[test]
    fn removes_all_but_what_it_keeps() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(data.join("versions/pc-latest")).unwrap();
        std::fs::create_dir_all(data.join("logs")).unwrap();
        std::fs::write(data.join("launcher.json"), "{}").unwrap();
        std::fs::write(data.join("logs/a.log"), "x").unwrap();
        let keep = data.join("versions");
        remove_dir_except(&data, Some(&keep)).unwrap();
        assert!(keep.join("pc-latest").is_dir());
        assert!(!data.join("logs").exists() && !data.join("launcher.json").exists());
        remove_dir_except(&data, None).unwrap();
        assert!(!data.exists());
    }

    #[test]
    fn names_its_shortcuts() {
        let v = InstalledVersion {
            id: "pc-latest".into(),
            name: "Echo VR (PC, latest)".into(),
            ..Default::default()
        };
        assert_eq!(shortcut_names(&v), ["Echo VR (Revive)", "Echo VR"]);
    }

    #[test]
    fn cleans_what_it_put_into_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let v = InstalledVersion {
            id: "mine".into(),
            name: "Mine".into(),
            root: dir.path().to_string_lossy().into_owned(),
            external: true,
            ..Default::default()
        };
        let bin = v.bin_dir();
        std::fs::create_dir_all(bin.join("EchoXR/Hands")).unwrap();
        std::fs::create_dir_all(bin.join("plugins")).unwrap();
        for f in [
            "echovr.exe",
            "EchoXR.exe",
            "echovr_openxr.exe",
            "LibOVRPlatform64_1.dll",
        ] {
            std::fs::write(bin.join(f), "MZ").unwrap();
        }
        std::fs::write(bin.join("plugins/EchoXRHands.dll"), "MZ").unwrap();
        std::fs::write(bin.join("plugins/NvrAssetPatches.dll"), "MZ").unwrap();
        assert!(clean_version(&v).is_empty());
        assert!(bin.join("echovr.exe").is_file());
        assert!(bin.join("LibOVRPlatform64_1.dll").is_file());
        assert!(bin.join("plugins/NvrAssetPatches.dll").is_file());
        assert!(!bin.join("EchoXR.exe").exists() && !bin.join("echovr_openxr.exe").exists());
        assert!(!bin.join("EchoXR").exists() && !bin.join("plugins/EchoXRHands.dll").exists());
    }
}
