//! The Quest's community content (the Echo Combat update's asset patches): the `add`/`del`
//! lines of files.echovr.de's Quest update, mirrored to release.echovr.de's Quest tree, put
//! into the game's media folder after nEVR's APK and game data, the way Crafter's Quest
//! installer does it. The list isn't signed, so it may only write game content: files under
//! `asset_patches/` or `files/`, no program file, no hidden file, each checked by its
//! SHA-256 (nEVR's ADR 0023 plans a signed `content` kind for this).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use super::adb::{self, PACKAGE};
use super::download::{redact, sha256_matches};
use super::error::UiError;
use super::http::{self, Cancelled};
use super::manifest::{Entry, Manifest};
use super::quest_install::with_reconnect;

/// The Quest update, mirrored from files.echovr.de/updates/quest.
pub const CONTENT_URL: &str = "https://release.echovr.de/quest/updates/update.manifest";

/// Where the list's paths are relative to.
pub fn root() -> String {
    format!("/sdcard/Android/media/{PACKAGE}")
}

const ALLOWED_ROOTS: [&str; 2] = ["asset_patches/", "files/"];
const REFUSED_EXTENSIONS: [&str; 13] = [
    ".so", ".dex", ".apk", ".jar", ".dll", ".odex", ".vdex", ".oat", ".art", ".sh", ".exe", ".bat",
    ".cmd",
];
const MAX_ENTRIES: usize = 1000;

/// Cap on one batched `sha256sum` call, in paths and in characters.
const HASH_BATCH_PATHS: usize = 50;
const HASH_BATCH_CHARS: usize = 3000;

/// Whether the list may write `path` (already past [`super::manifest::is_safe_path`]).
pub fn allowed(path: &str) -> bool {
    ALLOWED_ROOTS
        .iter()
        .any(|r| path.len() > r.len() && path.starts_with(r))
        && !path.split('/').any(|s| s.starts_with('.'))
        && !REFUSED_EXTENSIONS
            .iter()
            .any(|x| path.to_ascii_lowercase().ends_with(x))
}

/// Why the list can't be used, or `None`.
pub fn refusal(m: &Manifest) -> Option<String> {
    if m.entries.len() > MAX_ENTRIES {
        return Some(format!("it has {} entries", m.entries.len()));
    }
    m.entries
        .iter()
        .find(|e| !allowed(&e.path))
        .map(|e| format!("it asks for {}, which isn't game content", e.path))
}

/// Puts the list's content onto the headset: `del`s first, then each `add` whose file on
/// the headset has another checksum.
pub fn apply(cancel: &AtomicBool, staging: &Path, status: &mut dyn FnMut(String)) -> Result<()> {
    status("Checking the community content...".into());
    let m = Manifest::fetch(CONTENT_URL).context("Couldn't read the Quest's content list")?;
    if let Some(why) = refusal(&m) {
        return Err(UiError::new(
            "Couldn't add the community content",
            format!("The Quest's content list wasn't used: {why}."),
        )
        .into());
    }
    let root = root();
    let dels: Vec<&Entry> = m.dels().collect();
    let adds: Vec<&Entry> = m.adds().collect();
    for e in &dels {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        // Paths are checked by the parser: no shell metacharacters.
        let script = format!("rm -f {root}/{}", e.path);
        if !with_reconnect(&format!("rm {}", e.path), status, || {
            adb::shell(&script).success()
        }) {
            tracing::warn!("Quest content: couldn't delete {}", e.path);
        }
    }
    let have = remote_hashes(&root, &adds);
    let todo: Vec<&&Entry> = adds
        .iter()
        .filter(|e| {
            let sha = e.sha256.as_deref().unwrap_or_default();
            !have
                .get(&e.path)
                .is_some_and(|h| h.eq_ignore_ascii_case(sha))
        })
        .collect();
    if todo.is_empty() {
        tracing::info!("Quest content: {} file(s) up to date", adds.len());
        return Ok(());
    }
    std::fs::create_dir_all(staging)?;
    let local = staging.join("content.tmp");
    let mut made = HashSet::new();
    for (i, e) in todo.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        status(format!(
            "Adding the community content ({}/{})...",
            i + 1,
            todo.len()
        ));
        let sha = e.sha256.as_deref().unwrap_or_default();
        let url = m.url_for(e);
        tracing::info!("Quest content: downloading {}", redact(&url));
        http::download_checked(&url, sha, &local, Some(cancel))
            .with_context(|| format!("Downloading {} failed", e.path))?;
        if !sha256_matches(&local, sha) {
            let _ = std::fs::remove_file(&local);
            return Err(UiError::new(
                format!("A download came out damaged: {}", e.path),
                "It doesn't match its checksum, so it wasn't copied to your Quest.",
            )
            .into());
        }
        let remote = format!("{root}/{}", e.path);
        if let Some((parent, _)) = remote.rsplit_once('/') {
            if made.insert(parent.to_string()) {
                adb::shell(&format!("mkdir -p {parent}"));
            }
        }
        let pushed = with_reconnect(&format!("push {}", e.path), status, || {
            adb::push_file(&local, &remote)
        });
        let _ = std::fs::remove_file(&local);
        if !pushed {
            return Err(UiError::new(
                "Transfer Failed",
                format!("Couldn't copy {} to your Quest.", e.path),
            )
            .into());
        }
    }
    tracing::info!(
        "Quest content: {} of {} file(s) copied",
        todo.len(),
        adds.len()
    );
    Ok(())
}

/// Parses `sha256sum` output into path -> hash; other lines are skipped.
fn parse_hash_listing(output: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in output.lines().map(str::trim) {
        let mut parts = line.splitn(2, char::is_whitespace);
        let (Some(hash), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            map.insert(path.trim().to_string(), hash.to_ascii_lowercase());
        }
    }
    map
}

/// The headset's checksums of the `adds`' files, in few adb calls (none known: all copied).
fn remote_hashes(root: &str, adds: &[&Entry]) -> HashMap<String, String> {
    let mut hashes = HashMap::new();
    let mut batch: Vec<&str> = Vec::new();
    let mut chars = 0;
    let flush = |batch: &mut Vec<&str>, hashes: &mut HashMap<String, String>| {
        let script = format!("cd {root} && sha256sum {} 2>/dev/null", batch.join(" "));
        hashes.extend(parse_hash_listing(&adb::shell(&script).output));
        batch.clear();
    };
    for e in adds {
        if !batch.is_empty()
            && (batch.len() >= HASH_BATCH_PATHS || chars + e.path.len() > HASH_BATCH_CHARS)
        {
            flush(&mut batch, &mut hashes);
            chars = 0;
        }
        batch.push(&e.path);
        chars += e.path.len() + 1;
    }
    if !batch.is_empty() {
        flush(&mut batch, &mut hashes);
    }
    hashes
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "d5f216f7df4194345187abd6ffb91de4c463e983fbac1e08173177302db2cf37";

    #[test]
    fn only_game_content() {
        assert!(allowed("asset_patches/489bb35d53ca50e9/2dfe2e7610506f03"));
        assert!(allowed("files/_data/5932408047/rad15/android/packages/x"));
        assert!(!allowed("asset_patches/"));
        assert!(!allowed("lib/arm64/libassetpatch.so"));
        assert!(!allowed("asset_patches/x/libassetpatch.so"));
        assert!(!allowed("files/x.APK"));
        assert!(!allowed("asset_patches/.hidden/x"));
        assert!(!allowed("config.yaml"));
    }

    #[test]
    fn the_live_list_is_used() {
        let m = Manifest::parse(
            &format!(
                "# Echo VR Quest update manifest -- 2026-08-27\n# Target:  /sdcard/Android/media/com.readyatdawn.r15\n\nadd  asset_patches/489bb35d53ca50e9/2dfe2e7610506f03  {SHA}\ndel  asset_patches/old/x\n"
            ),
            CONTENT_URL,
        )
        .unwrap();
        assert_eq!(refusal(&m), None);
        assert_eq!(
            m.url_for(&m.entries[0]),
            "https://release.echovr.de/quest/updates/asset_patches/489bb35d53ca50e9/2dfe2e7610506f03"
        );
        let bad = Manifest::parse(&format!("add  lib/x.so  {SHA}\n"), CONTENT_URL).unwrap();
        assert!(refusal(&bad).is_some());
    }

    #[test]
    fn reads_sha256sum_output() {
        let out = format!("{SHA}  asset_patches/a/b\nsha256sum: asset_patches/a/c: No such file\n");
        let m = parse_hash_listing(&out);
        assert_eq!(m.get("asset_patches/a/b").map(String::as_str), Some(SHA));
        assert_eq!(m.len(), 1);
    }
}
