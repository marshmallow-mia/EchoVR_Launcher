//! "Delete cache": removes downloaded/staged files, and the game zips that cancelled
//! installs leave in the launcher's version folders. The adb this session runs (in the
//! cache too) stays, and so does everything outside the cache: settings, logs, installs.

use std::path::{Path, PathBuf};

use super::paths;

fn targets(roots: &[String]) -> Vec<PathBuf> {
    let cache = paths::cache_dir();
    let mut v = vec![cache.join("downloads"), cache.join("tmp")];
    // What older versions left in the old cache folder, which on Windows is the data
    // folder: only their own subfolders, never the folder itself.
    let old = paths::legacy_cache_dir();
    v.push(old.join("downloads"));
    v.push(old.join("tmp"));
    if let Ok(entries) = std::fs::read_dir(&old) {
        v.extend(
            entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("platform-tools-")
                })
                .map(|e| e.path()),
        );
    }
    let zip = format!("{}.zip", paths::ARENA_DIR);
    v.extend(roots.iter().map(|r| Path::new(r).join(&zip)));
    v
}

/// How many bytes `delete_all` would free.
pub fn size(roots: &[String]) -> u64 {
    fn walk(p: &Path) -> u64 {
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() => std::fs::read_dir(p)
                .map(|entries| entries.flatten().map(|e| walk(&e.path())).sum())
                .unwrap_or(0),
            Ok(m) => m.len(),
            Err(_) => 0,
        }
    }
    targets(roots).iter().map(|p| walk(p)).sum()
}

/// Deletes everything it can; returns the paths that existed but could not be removed.
/// `roots` are the version folders the launcher installs into.
pub fn delete_all(roots: &[String]) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for p in targets(roots) {
        let result = if p.is_dir() {
            std::fs::remove_dir_all(&p)
        } else if p.exists() {
            std::fs::remove_file(&p)
        } else {
            continue;
        };
        match result {
            Ok(()) => tracing::info!("Deleted: {}", p.display()),
            Err(e) => {
                tracing::warn!("Failed to delete {}: {e}", p.display());
                failed.push(p);
            }
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_folders() {
        let t = targets(&["C:/EchoVR/versions/pc-34.4".into()]);
        assert_eq!(t[0], paths::cache_dir().join("downloads"));
        assert!(t
            .last()
            .unwrap()
            .ends_with("pc-34.4/ready-at-dawn-echo-arena.zip"));
        // Never a whole folder that holds launcher.json or the logs.
        for p in &t {
            assert_ne!(p, &paths::data_dir());
            assert_ne!(p, &paths::legacy_cache_dir());
            assert_ne!(p, &paths::cache_dir());
            assert!(!paths::data_dir().starts_with(p), "{}", p.display());
            assert!(!paths::log_dir().starts_with(p), "{}", p.display());
        }
    }
}
