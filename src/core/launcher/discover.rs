//! Echo VR copies already on this PC that the launcher doesn't know yet, so the install
//! card can offer them instead of a download: in the Meta (Oculus) app's libraries, where
//! the old installer put it (`C:\EchoVR`), in the launcher's own library folder (a lost
//! `launcher.json`), and in Wine prefixes on Linux.

use std::path::{Path, PathBuf};

use super::store::LauncherState;
use crate::core::{paths, platform};

/// Where a Wine prefix keeps Meta's library, as the Meta Quest app installs it.
const PREFIX_LIBRARY: &str = "drive_c/Program Files/Oculus/Software/Software";

/// The install roots worth looking at (each the folder that would hold
/// `ready-at-dawn-echo-arena`), most likely first.
fn candidates(library: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(base) = platform::oculus_base_path() {
        out.push(Path::new(&base).join("Software/Software"));
    }
    out.extend(
        platform::meta_libraries()
            .into_iter()
            .map(|lib| Path::new(&lib).join("Software")),
    );
    if cfg!(windows) {
        out.push("C:/Program Files/Oculus/Software/Software".into());
        out.push("C:/EchoVR".into());
    }
    // The launcher's own library: versions installed before its state file was lost.
    if let Ok(dir) = std::fs::read_dir(library) {
        let mut kids: Vec<PathBuf> = dir.filter_map(Result::ok).map(|e| e.path()).collect();
        kids.sort();
        out.extend(kids);
    }
    if cfg!(target_os = "linux") {
        if let Some(home) = dirs::home_dir() {
            out.push(home.join(".wine").join(PREFIX_LIBRARY));
        }
        if let Some(steam) = crate::core::linux::steam::root() {
            if let Ok(dir) = std::fs::read_dir(steam.join("steamapps/compatdata")) {
                out.extend(
                    dir.filter_map(Result::ok)
                        .map(|e| e.path().join("pfx").join(PREFIX_LIBRARY)),
                );
            }
        }
    }
    out
}

/// Pure (but for the file checks): the roots among `candidates` that hold Echo VR and
/// aren't in `state` yet, each once.
fn installs_in(candidates: Vec<PathBuf>, state: &LauncherState) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for c in candidates {
        let root = paths::normalize(&c.to_string_lossy());
        if paths::has_echo_install(&root)
            && !state.has_root(&root)
            && !found.iter().any(|f| f.eq_ignore_ascii_case(&root))
        {
            found.push(root);
        }
    }
    found
}

/// Echo VR installs on this PC that the launcher doesn't know yet.
pub fn find_installs(state: &LauncherState) -> Vec<String> {
    installs_in(candidates(&state.library), state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(root: &Path) {
        let bin = root.join(paths::ARENA_DIR).join("bin/win10");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(paths::DEFAULT_EXE), b"MZ").unwrap();
    }

    #[test]
    fn finds_what_the_library_doesnt_know() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("versions");
        let (known, lost, meta) = (
            library.join("pc-latest"),
            library.join("old"),
            dir.path().join("Oculus/Software/Software"),
        );
        install(&known);
        install(&lost);
        install(&meta);
        std::fs::create_dir_all(library.join("empty")).unwrap();
        let mut state = LauncherState {
            library: paths::normalize(&library.to_string_lossy()),
            ..LauncherState::default()
        };
        state.add_external(&known.to_string_lossy(), None);

        let mut c = candidates(&state.library);
        c.insert(0, meta.clone());
        // The same folder twice (another library entry for it): once.
        c.push(meta.clone());
        let found = installs_in(c, &state);
        assert_eq!(
            found,
            [
                paths::normalize(&meta.to_string_lossy()),
                paths::normalize(&lost.to_string_lossy()),
            ]
        );
    }
}
