//! Where things live: per-user cache/log dirs, and the Echo VR install layout.
//!
//! The Echo client lives at `<root>/ready-at-dawn-echo-arena/bin/win10/echovr.exe`; the
//! 2017 event builds start `EchoArena.exe` instead, which may sit in `bin/win7`. Install
//! paths are kept with forward slashes and no trailing slash.

use std::path::{Path, PathBuf};

pub const ARENA_DIR: &str = "ready-at-dawn-echo-arena";
/// The folder the launcher keeps its files in (data, cache).
const APP_DIR: &str = "EchoVR_Launcher";
/// Its name while the launcher was the Echo VR Installer: moved over once.
const OLD_APP_DIR: &str = "EchoVR_Installer";
/// The executable of every build since 2018.
pub const DEFAULT_EXE: &str = "echovr.exe";
/// Every executable name an Echo build has had (the 2017 ones: `EchoArena.exe`).
pub const GAME_EXES: [&str; 2] = [DEFAULT_EXE, "EchoArena.exe"];
/// The executable EchoXR runs: its patched copy of `echovr.exe`, made next to it.
pub const OPENXR_EXE: &str = "echovr_openxr.exe";
/// Every executable a running game can have.
pub const GAME_PROCESSES: [&str; 3] = [DEFAULT_EXE, "EchoArena.exe", OPENXR_EXE];
/// Where a build keeps its executable, newest layout first.
const BIN_DIRS: [&str; 2] = ["bin/win10", "bin/win7"];

/// Per-user cache root. Never the shared temp dir (world-writable on Linux), and a folder
/// of its own: on Windows the system cache folder is the data folder, where
/// `launcher.json` and the logs live.
pub fn cache_dir() -> PathBuf {
    legacy_cache_dir().join("cache")
}

/// Where older versions kept the cache (on Windows: the data folder itself).
pub fn legacy_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

/// Staged downloads (Quest APK/data, licence patch dll, Revive installer).
pub fn downloads_dir() -> PathBuf {
    cache_dir().join("downloads")
}

/// Scratch space for short-lived files.
pub fn temp_dir() -> PathBuf {
    cache_dir().join("tmp")
}

/// Per-user application data (launcher state, logs).
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

/// Moves what the launcher kept as the Echo VR Installer (its data and cache folders) to
/// its own name, once, before anything is written: installs, settings, the sign-in and
/// the logs carry over. Install paths in `launcher.json` under the old folder (the default
/// library) are pointed at the new one.
pub fn move_from_installer() {
    for base in [dirs::data_local_dir(), dirs::cache_dir()]
        .into_iter()
        .flatten()
    {
        let (old, new) = (base.join(OLD_APP_DIR), base.join(APP_DIR));
        if old.is_dir() && !new.exists() && std::fs::rename(&old, &new).is_ok() {
            let state = new.join("launcher.json");
            if let Ok(text) = std::fs::read_to_string(&state) {
                let moved = repoint(
                    &text,
                    &normalize(&old.to_string_lossy()),
                    &normalize(&new.to_string_lossy()),
                );
                if moved != text {
                    let _ = std::fs::write(&state, moved);
                }
            }
        }
    }
}

/// Pure: `json` with every path under `old` (normalized) moved under `new`.
fn repoint(json: &str, old: &str, new: &str) -> String {
    json.replace(&format!("\"{old}/"), &format!("\"{new}/"))
        .replace(&format!("\"{old}\""), &format!("\"{new}\""))
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Backslashes to slashes, trailing slashes trimmed.
pub fn normalize(path: &str) -> String {
    let mut n = path.trim().replace('\\', "/");
    while n.len() > 1 && n.ends_with('/') {
        n.pop();
    }
    n
}

pub fn bin_path(root: &str) -> PathBuf {
    PathBuf::from(format!("{root}/{ARENA_DIR}/bin/win10"))
}

/// Where the executable `exe` (a file name) of the install at `root` is: in the first
/// bin folder that has it, else where the newest layout puts it.
pub fn exe_in(root: &str, exe: &str) -> PathBuf {
    BIN_DIRS
        .iter()
        .map(|dir| PathBuf::from(format!("{root}/{ARENA_DIR}/{dir}/{exe}")))
        .find(|p| p.is_file())
        .unwrap_or_else(|| bin_path(root).join(exe))
}

/// The executable an install at `root` has: `echovr.exe`, or `EchoArena.exe` for the
/// 2017 builds. `None` when there is no Echo install there.
pub fn find_exe(root: &str) -> Option<&'static str> {
    if root.is_empty() {
        return None;
    }
    GAME_EXES.into_iter().find(|e| exe_in(root, e).is_file())
}

/// True when an Echo install exists directly under `root`.
pub fn has_echo_install(root: &str) -> bool {
    find_exe(root).is_some()
}

/// Resolves the Echo install ROOT from whatever folder the user picked: the root itself,
/// the `ready-at-dawn-echo-arena` folder, a folder inside it, or a folder up to three
/// levels above the install. Returns the cleaned selection unchanged when nothing is found
/// (the caller's validation then flags it invalid).
pub fn resolve_install_root(selected: &str) -> String {
    let norm = normalize(selected);
    if norm.is_empty() {
        return norm;
    }
    // 1) Walk up: the root is the ancestor (incl. the selection) holding the marker.
    let mut cur = Some(PathBuf::from(&norm));
    for _ in 0..8 {
        let Some(c) = cur else { break };
        let cs = normalize(&c.to_string_lossy());
        if has_echo_install(&cs) {
            return cs;
        }
        cur = c
            .parent()
            .map(Path::to_path_buf)
            .filter(|p| !p.as_os_str().is_empty());
    }
    // 2) Bounded downward search -- covers picking a folder above the install.
    search_down(Path::new(&norm), 3).unwrap_or(norm)
}

fn search_down(dir: &Path, depth: i32) -> Option<String> {
    if depth < 0 || !dir.is_dir() {
        return None;
    }
    let c = normalize(&dir.to_string_lossy());
    if has_echo_install(&c) {
        return Some(c);
    }
    let mut kids: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    kids.sort();
    kids.iter().find_map(|k| search_down(k, depth - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_install_paths_with_the_folder() {
        let old = "C:/Users/a/AppData/Local/EchoVR_Installer";
        let new = "C:/Users/a/AppData/Local/EchoVR_Launcher";
        let json = r#"{"versions":[{"root":"C:/Users/a/AppData/Local/EchoVR_Installer/versions/pc-latest"},{"root":"D:/Games/EchoVR_Installer2"}],"library":"C:/Users/a/AppData/Local/EchoVR_Installer"}"#;
        let moved = repoint(json, old, new);
        assert!(moved
            .contains(r#""root":"C:/Users/a/AppData/Local/EchoVR_Launcher/versions/pc-latest""#));
        assert!(moved.contains(r#""library":"C:/Users/a/AppData/Local/EchoVR_Launcher""#));
        assert!(moved.contains("D:/Games/EchoVR_Installer2"));
    }

    fn fake_install(root: &Path) {
        let bin = root.join("ready-at-dawn-echo-arena/bin/win10");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("echovr.exe"), b"").unwrap();
    }

    #[test]
    fn finds_either_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let new = tmp.path().join("new");
        fake_install(&new);
        let new = normalize(&new.to_string_lossy());
        assert_eq!(find_exe(&new), Some("echovr.exe"));
        let old = tmp.path().join("old");
        let bin = old.join("ready-at-dawn-echo-arena/bin/win7");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("EchoArena.exe"), b"").unwrap();
        let old = normalize(&old.to_string_lossy());
        assert_eq!(find_exe(&old), Some("EchoArena.exe"));
        assert_eq!(exe_in(&old, "EchoArena.exe"), bin.join("EchoArena.exe"));
        assert!(has_echo_install(&old));
        assert_eq!(find_exe(&normalize(&tmp.path().to_string_lossy())), None);
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize("C:\\EchoVR\\"), "C:/EchoVR");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("  /a/b// "), "/a/b");
    }

    #[test]
    fn resolves_root_from_anywhere_nearby() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("games").join("Echo");
        fake_install(&root);
        let want = normalize(&root.to_string_lossy());
        for pick in [
            root.clone(),
            root.join("ready-at-dawn-echo-arena"),
            root.join("ready-at-dawn-echo-arena/bin/win10"),
            tmp.path().join("games"),
            tmp.path().to_path_buf(),
        ] {
            assert_eq!(
                resolve_install_root(&pick.to_string_lossy()),
                want,
                "{}",
                pick.display()
            );
        }
        assert!(has_echo_install(&want));
        let nothing = tmp.path().join("empty");
        std::fs::create_dir_all(&nothing).unwrap();
        let n = normalize(&nothing.to_string_lossy());
        assert_eq!(resolve_install_root(&n), n);
    }
}
