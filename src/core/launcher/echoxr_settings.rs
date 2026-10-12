//! EchoXR's controller settings, kept by a launcher plugin: the plugin whose settings store
//! is marked `echoxr` keeps `EchoXR.txt` in its own folder, and the launcher keeps a copy
//! beside EchoXR in every installed version (`bin/<win10|win7>/EchoXR/EchoXR.txt`), where
//! EchoXR reads it again while the game runs. So the settings reach every start, the
//! launcher's and Steam's, WiVRn's or a shortcut's.
//!
//! A copy starts with [`MARKER`]: only marked copies are replaced or removed. A file a
//! player wrote by hand is kept as `EchoXR.txt.bak` the first time the plugin's settings
//! replace it.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::plugin_settings::{Settings, Store, ECHOXR_FILE};
use super::plugins;
use super::store::InstalledVersion;

/// The first line of a copy the launcher keeps.
pub const MARKER: &str = "# Kept by the Echo VR launcher's Controllers plugin: change it there.";

/// Pure: whether settings `s` are EchoXR's.
pub fn is_echoxr(s: &Settings) -> bool {
    matches!(s.store, Store::File { echoxr: true, .. })
        || s.fields()
            .any(|f| matches!(f.store, Some(Store::File { echoxr: true, .. })))
}

/// The settings text of the installed plugin that keeps EchoXR's, found without logging
/// what descriptions get wrong (the plugins page does that): for preparing a version and
/// its fingerprint (file I/O).
pub fn current() -> Option<String> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(plugins::dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path().join(plugins::DESCRIPTION)).ok()?;
            let page = plugins::read_description(&text).0?;
            (e.file_name().to_str() == Some(page.id.as_str())
                && page.settings.as_ref().is_some_and(is_echoxr))
            .then(|| (page.name.to_lowercase(), e.path()))
        })
        .collect();
    found.sort();
    let (_, dir) = found.into_iter().next()?;
    Some(std::fs::read_to_string(dir.join(ECHOXR_FILE)).unwrap_or_default())
}

/// Pure: the copy of settings text `source`.
pub fn copy_text(source: &str) -> String {
    let body = source
        .strip_prefix(MARKER)
        .map_or(source, |rest| rest.trim_start_matches(['\r', '\n']));
    format!("{MARKER}\n{body}")
}

/// Pure: whether `text` is a copy the launcher keeps.
pub fn is_copy(text: &str) -> bool {
    text.starts_with(MARKER)
}

/// The EchoXR folders in `v` (event builds keep theirs in `bin/win7`).
fn folders(v: &InstalledVersion) -> Vec<PathBuf> {
    let bin = v.bin_dir();
    let mut out = vec![bin.join(crate::core::echoxr::DIR)];
    if let Some(arena) = bin.parent() {
        for other in ["win10", "win7"] {
            let d = arena.join(other).join(crate::core::echoxr::DIR);
            if !out.contains(&d) {
                out.push(d);
            }
        }
    }
    out
}

/// Keeps `source` (or, with none, no copy) in EchoXR folder `dir`, if EchoXR is there.
/// Whether it changed anything (file I/O).
pub fn apply_to(dir: &Path, source: Option<&str>) -> Result<bool> {
    if !dir.is_dir() {
        return Ok(false);
    }
    let path = dir.join(ECHOXR_FILE);
    let have = std::fs::read_to_string(&path).ok();
    match source {
        Some(text) => {
            let want = copy_text(text);
            if have.as_deref() == Some(want.as_str()) {
                return Ok(false);
            }
            if let Some(own) = have.as_deref().filter(|t| !is_copy(t)) {
                let bak = dir.join(format!("{ECHOXR_FILE}.bak"));
                if !bak.exists() {
                    std::fs::write(&bak, own)
                        .with_context(|| format!("Couldn't keep {}", bak.display()))?;
                }
            }
            write_atomic(&path, &want)?;
            Ok(true)
        }
        None => match have {
            Some(t) if is_copy(&t) => {
                std::fs::remove_file(&path)
                    .with_context(|| format!("Couldn't remove {}", path.display()))?;
                Ok(true)
            }
            _ => Ok(false),
        },
    }
}

/// Keeps `source` in every EchoXR folder of `v` (file I/O).
pub fn apply(v: &InstalledVersion, source: Option<&str>) -> Result<()> {
    for dir in folders(v) {
        apply_to(&dir, source)?;
    }
    Ok(())
}

/// Keeps `source` in every installed version; logs the ones it couldn't (file I/O).
pub fn apply_all(versions: &[InstalledVersion], source: Option<&str>) {
    for v in versions {
        if let Err(e) = apply(v, source) {
            tracing::warn!("EchoXR settings in {}: {e:#}", v.id);
        }
    }
}

/// Replaces `path` with `text`; a game that has the file open for reading at that moment
/// (Windows) gets a few more tries.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("txt.launcher.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("Couldn't write {}", tmp.display()))?;
    let mut last = None;
    for _ in 0..4 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(last.expect("an error")).with_context(|| format!("Couldn't replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_are_marked_and_only_marked_ones_change() {
        let dir = tempfile::tempdir().expect("dir");
        let echoxr = dir.path().join("EchoXR");

        // No EchoXR in this version: nothing is written.
        assert!(!apply_to(&echoxr, Some("Pitch = 5\n")).expect("ok"));
        assert!(!echoxr.exists());

        std::fs::create_dir(&echoxr).expect("dir");
        assert!(apply_to(&echoxr, Some("Pitch = 5\n")).expect("ok"));
        let path = echoxr.join(ECHOXR_FILE);
        let copy = std::fs::read_to_string(&path).expect("copy");
        assert_eq!(copy, format!("{MARKER}\nPitch = 5\n"));
        // The same again changes nothing; a copy of a copy isn't marked twice.
        assert!(!apply_to(&echoxr, Some("Pitch = 5\n")).expect("ok"));
        assert_eq!(copy_text(&copy), copy);

        // The plugin went: the copy goes too.
        assert!(apply_to(&echoxr, None).expect("ok"));
        assert!(!path.exists());

        // A file the player wrote is never removed, and kept aside when the plugin takes over.
        std::fs::write(&path, "Pitch = -7\n").expect("own");
        assert!(!apply_to(&echoxr, None).expect("ok"));
        assert_eq!(std::fs::read_to_string(&path).expect("own"), "Pitch = -7\n");
        assert!(apply_to(&echoxr, Some("Yaw = 2\n")).expect("ok"));
        assert_eq!(
            std::fs::read_to_string(echoxr.join("EchoXR.txt.bak")).expect("bak"),
            "Pitch = -7\n"
        );
        assert!(is_copy(&std::fs::read_to_string(&path).expect("copy")));
    }

    #[test]
    fn every_echoxr_folder_of_a_version() {
        let dir = tempfile::tempdir().expect("dir");
        let arena = dir.path().join("ready-at-dawn-echo-arena/bin");
        for b in ["win10/EchoXR", "win7/EchoXR"] {
            std::fs::create_dir_all(arena.join(b)).expect("dir");
        }
        let v = InstalledVersion {
            id: "pc-latest".into(),
            root: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        apply(&v, Some("Roll = 3\n")).expect("ok");
        for b in ["win10/EchoXR", "win7/EchoXR"] {
            assert!(arena.join(b).join(ECHOXR_FILE).is_file(), "{b}");
        }
    }
}
