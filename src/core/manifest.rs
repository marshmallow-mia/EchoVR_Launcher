//! Parsed update manifest, shared by the PC and Quest update paths.
//!
//! Body grammar (whitespace-separated; `#` comments and blank lines ignored):
//!
//! ```text
//!   add  path/to/file.dll  sha256hex
//!   del  path/to/old.dll
//! ```
//!
//! The Quest manifest additionally carries two header comments:
//!
//! ```text
//!   # BASE_APK: echo_quest_27-08-2026.001.apk 0a7fa5f9...
//!   # Target:  /sdcard/Android/media/com.readyatdawn.r15
//! ```
//!
//! A build's file manifest (`<archive>.manifest` beside each build, e.g. `pc.zip.manifest`)
//! describes the archive it lists:
//!
//! ```text
//!   # Archive:  pc.zip
//!   # SHA256:   9a498cb0...
//!   # Size:     5024528313
//!   # Root:     ready-at-dawn-echo-arena/
//! ```
//!
//! Manifest paths end up in `adb shell` scripts (including `rm -rf`) and in local
//! filesystem paths, so entry paths and the target root are validated strictly here.
//! This is the single choke point -- do not re-implement parsing elsewhere.

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Add,
    Del,
}

/// One manifest line. `sha256` is `None` for `del` entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub action: Action,
    pub path: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub entries: Vec<Entry>,
    /// Manifest URL up to (excluding) the last '/'; entries resolve against it.
    pub base_url: String,
    /// APK filename from the `# BASE_APK:` header.
    pub base_apk_name: Option<String>,
    /// SHA-256 of the base APK this manifest was built against.
    pub base_apk_sha: Option<String>,
    /// On-device root the entry paths are relative to (Quest manifests only).
    pub target_root: Option<String>,
    /// A build's file manifest: the archive's file name, beside the manifest.
    pub archive: Option<String>,
    /// ...its SHA-256 and size,
    pub archive_sha: Option<String>,
    pub archive_size: Option<u64>,
    /// ...and the one folder in it the entry paths are relative to ("name/").
    pub archive_root: Option<String>,
}

/// Only unreserved path characters, no `.`/`..`/empty segments.
///
/// `+` is allowed after the first character because the PC manifest ships
/// `libstdc++-6.dll`. It is neither a shell metacharacter nor a glob character.
pub fn is_safe_path(path: &str) -> bool {
    let mut chars = path.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphanumeric() || first == '.' || first == '_') {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '+' | '-')) {
        return false;
    }
    // Every segment must name something: rejects "", ".", ".." and "a//b" / "a/./b".
    path.split('/')
        .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
        && !path.contains("..")
}

/// The Quest target must be an Echo VR app media dir -- nothing else may reach `rm -rf`.
pub fn is_safe_target(root: &str) -> bool {
    const PREFIX: &str = "/sdcard/Android/media/com.readyatdawn.";
    match root.strip_prefix(PREFIX) {
        Some(rest) => !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()),
        None => false,
    }
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `# BASE_APK: <name> <sha>` -> (name, sha)
fn parse_base_apk_header(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("BASE_APK:")?;
    let mut it = rest.split_whitespace();
    let name = it.next()?;
    let sha = it.next()?;
    if it.next().is_some() || !is_sha256_hex(sha) {
        return None;
    }
    Some((name.to_string(), sha.to_string()))
}

/// `# <key>: <value>` -> value, for a single-word value.
fn header<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.strip_prefix('#')?.trim_start().strip_prefix(key)?;
    let mut it = rest.strip_prefix(':')?.split_whitespace();
    let value = it.next()?;
    it.next().is_none().then_some(value)
}

/// Reads a build file manifest's archive headers from `line` into `m`, unless unsafe.
fn parse_archive_header(m: &mut Manifest, line: &str) {
    if let Some(name) = header(line, "Archive").filter(|n| is_safe_path(n) && !n.contains('/')) {
        m.archive = Some(name.to_string());
    } else if let Some(sha) = header(line, "SHA256").filter(|s| is_sha256_hex(s)) {
        m.archive_sha = Some(sha.to_ascii_lowercase());
    } else if let Some(size) = header(line, "Size").and_then(|s| s.parse().ok()) {
        m.archive_size = Some(size);
    } else if let Some(name) = root_header(line) {
        m.archive_root = Some(format!("{name}/"));
    }
}

/// `# Root: <folder>/` -> the folder's name. It may have spaces ("Echo VR Halloween
/// 2017/"), but must be one plain folder: no separators, no `..`, nothing hidden.
fn root_header(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('#')?.trim_start().strip_prefix("Root")?;
    let name = rest.strip_prefix(':')?.trim().trim_end_matches('/');
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && !name.contains("..")
        && !name
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control());
    plain.then_some(name)
}

/// `# Target: <root>` -> root
fn parse_target_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("Target:")?;
    let mut it = rest.split_whitespace();
    let root = it.next()?;
    if it.next().is_some() {
        return None;
    }
    Some(root.to_string())
}

impl Manifest {
    pub fn parse(content: &str, manifest_url: &str) -> Result<Manifest> {
        let mut m = Manifest::default();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Headers are comments, so they must be matched before the '#' skip.
            if trimmed.starts_with('#') {
                if let Some((name, sha)) = parse_base_apk_header(trimmed) {
                    m.base_apk_name = Some(name);
                    m.base_apk_sha = Some(sha);
                } else if let Some(root) = parse_target_header(trimmed) {
                    m.target_root = Some(root);
                } else {
                    parse_archive_header(&mut m, trimmed);
                }
                continue;
            }

            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            if tokens.len() < 2 {
                continue;
            }
            let action = match tokens[0] {
                "add" => Action::Add,
                "del" => Action::Del,
                other => bail!("Unknown manifest action: {other}"),
            };
            let path = tokens[1];
            if !is_safe_path(path) {
                bail!("Unsafe path in manifest: {path}");
            }
            let sha256 = match action {
                Action::Add => {
                    let sha = tokens
                        .get(2)
                        .with_context(|| format!("Missing SHA-256 for manifest entry: {path}"))?;
                    if !is_sha256_hex(sha) {
                        bail!("Invalid SHA-256 for manifest entry: {path}");
                    }
                    Some(sha.to_ascii_lowercase())
                }
                Action::Del => None,
            };
            m.entries.push(Entry {
                action,
                path: path.to_string(),
                sha256,
            });
        }

        if let Some(root) = &m.target_root {
            if !is_safe_target(root) {
                bail!("Unsafe target root in manifest: {root}");
            }
        }
        m.base_url = match manifest_url.rfind('/') {
            Some(i) => manifest_url[..i].to_string(),
            None => String::new(),
        };
        Ok(m)
    }

    pub fn adds(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.action == Action::Add)
    }

    pub fn dels(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.action == Action::Del)
    }

    pub fn url_for(&self, e: &Entry) -> String {
        format!("{}/{}", self.base_url, e.path)
    }

    /// A build file manifest's archive, beside it.
    pub fn archive_url(&self) -> Option<String> {
        self.archive
            .as_ref()
            .map(|a| format!("{}/{a}", self.base_url))
    }

    /// Downloads and parses. Errors on network failure or a malformed/unsafe manifest.
    pub fn fetch(url: &str) -> Result<Manifest> {
        let text = crate::core::http::get_text(url)
            .with_context(|| format!("Could not download the update manifest ({url})"))?;
        Manifest::parse(&text, url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0a7fa5f9d2b8c1e4f6a3b5c7d9e1f2a4b6c8d0e2f4a6b8c0d2e4f6a8b0c2d4e6";

    #[test]
    fn parses_adds_and_dels_in_order() {
        let m = Manifest::parse(
            &format!("# comment\n\nadd a/b.dll {SHA}\ndel old.dll\nadd libstdc++-6.dll {SHA}\n"),
            "https://files.echovr.de/updates/update.manifest",
        )
        .unwrap();
        assert_eq!(m.entries.len(), 3);
        assert_eq!(m.adds().count(), 2);
        assert_eq!(m.dels().next().unwrap().path, "old.dll");
        assert_eq!(m.base_url, "https://files.echovr.de/updates");
        assert_eq!(
            m.url_for(&m.entries[0]),
            "https://files.echovr.de/updates/a/b.dll"
        );
    }

    #[test]
    fn parses_build_manifest_headers() {
        let m = Manifest::parse(
            &format!(
                "# Echo VR build file manifest\n# Archive:  pc.zip\n# Base URL: https://files.echovr.de\n# SHA256:   {}\n# Size:     5024528313\n# Root:     ready-at-dawn-echo-arena/\n# Files:    1\n#\nadd  bin/win10/echovr.exe  {SHA}\n",
                SHA.to_uppercase()
            ),
            "https://files.echovr.de/pc.zip.manifest",
        )
        .unwrap();
        assert_eq!(m.archive.as_deref(), Some("pc.zip"));
        assert_eq!(m.archive_sha.as_deref(), Some(SHA));
        assert_eq!(m.archive_size, Some(5_024_528_313));
        assert_eq!(m.archive_root.as_deref(), Some("ready-at-dawn-echo-arena/"));
        assert_eq!(
            m.archive_url().as_deref(),
            Some("https://files.echovr.de/pc.zip")
        );
        // A root folder with spaces is fine.
        let spaced =
            Manifest::parse("# Root:     Echo VR Halloween 2017/\n", "https://h/m").unwrap();
        assert_eq!(
            spaced.archive_root.as_deref(),
            Some("Echo VR Halloween 2017/")
        );
        // Unsafe names are dropped, not used.
        let bad = Manifest::parse("# Archive: ../x.zip\n# Root: ../\n", "https://h/m").unwrap();
        assert_eq!((bad.archive, bad.archive_root), (None, None));
    }

    #[test]
    fn parses_quest_headers() {
        let m = Manifest::parse(
            &format!(
                "# BASE_APK: echo_quest_27-08-2026.001.apk {SHA}\n# Target:  /sdcard/Android/media/com.readyatdawn.r15\nadd x {SHA}"
            ),
            "u/m",
        )
        .unwrap();
        assert_eq!(
            m.base_apk_name.as_deref(),
            Some("echo_quest_27-08-2026.001.apk")
        );
        assert_eq!(m.base_apk_sha.as_deref(), Some(SHA));
        assert_eq!(
            m.target_root.as_deref(),
            Some("/sdcard/Android/media/com.readyatdawn.r15")
        );
    }

    #[test]
    fn rejects_unsafe_paths() {
        for bad in [
            "../x", "a/../b", "/abs", "a b", "a;rm", "$(x)", "a`b`", ".", "a/./b", "a//b", "a/",
            "-rf",
        ] {
            assert!(!is_safe_path(bad), "{bad} should be unsafe");
            let text = format!("del {bad}");
            // Some of these split into several tokens; only the ones that survive tokenizing matter.
            if !bad.contains(' ') {
                assert!(
                    Manifest::parse(&text, "u/m").is_err(),
                    "{bad} should fail to parse"
                );
            }
        }
        assert!(is_safe_path(".hidden/file"));
        assert!(is_safe_path("_local/config.json"));
    }

    #[test]
    fn rejects_unknown_action_missing_or_bad_hash() {
        assert!(Manifest::parse("mv a b", "u/m").is_err());
        assert!(Manifest::parse("add a", "u/m").is_err());
        assert!(Manifest::parse("add a nothex", "u/m").is_err());
    }

    /// The live manifests must stay parseable. `cargo test -- --ignored` (needs network).
    #[test]
    #[ignore]
    fn live_manifests_parse() {
        let pc = Manifest::fetch(crate::core::pc_update::PC_MANIFEST_URL).unwrap();
        assert!(pc.adds().count() > 0);
        let q = Manifest::fetch(crate::core::quest_update::QUEST_MANIFEST_URL).unwrap();
        assert!(q.base_apk_name.is_some() && q.target_root.is_some());
    }

    #[test]
    fn rejects_unsafe_target() {
        for bad in [
            "/sdcard",
            "/sdcard/Android/media/com.readyatdawn.",
            "/sdcard/Android/media/com.readyatdawn.r15/../..",
            "/sdcard/Android/media/com.other.app",
        ] {
            let text = format!("# Target: {bad}\n");
            assert!(Manifest::parse(&text, "u/m").is_err(), "{bad}");
        }
    }
}
