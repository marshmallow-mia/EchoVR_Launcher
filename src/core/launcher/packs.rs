//! Content packs: mods that bring game data as well as plugins (EchoCombat).
//!
//! A pack is a zip of `plugins/` (its plugins and their settings files) and
//! `content/<content>/` (its content pack: manifests, the packages they add, script DLLs
//! and `content.json`). [`OVERLAY_PLUGIN`], a plugin from the mods catalogue, serves the
//! content pack in place of the stock game files while Echo runs. nEVR has to load it
//! before the game reads its data (`early: true`, which needs a nEVR with the early load
//! pass: [`nevr_loads_early`]). The game's own files never change; with the pack off the
//! game is stock.
//!
//! The Mods page treats a pack as one mod: its plugins are on, off and removed together
//! (see [`super::mods`]).

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::catalog;

/// The plugin that serves content packs.
pub const OVERLAY_PLUGIN: &str = "NvrContentOverlay.dll";
/// Where content packs go, beside `plugins/` in `bin/win10`.
pub const CONTENT: &str = "content";

/// A catalogue entry's pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PackSpec {
    /// The zip: relative to the download mirrors, or https on a trusted host.
    pub url: String,
    pub sha256: String,
    pub size: Option<u64>,
    /// Its content pack: `content/<content>/` in the zip, and what the overlay serves.
    pub content: String,
    /// Its plugins (`plugins/<file>` in the zip), in the order nEVR loads them.
    pub plugins: Vec<String>,
}

impl PackSpec {
    pub fn validate(&self, id: &str) -> Result<()> {
        if self.url.is_empty() || !catalog::is_safe_url(&self.url) {
            bail!("untrusted download for {id}: {}", self.url);
        }
        if self.sha256.len() != 64 || !self.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("{id}'s pack has no valid sha256");
        }
        if !catalog::is_safe_id(&self.content) {
            bail!("{id}'s pack has an invalid content id {:?}", self.content);
        }
        for p in &self.plugins {
            let reserved = [super::mods::SLOT, OVERLAY_PLUGIN]
                .iter()
                .any(|r| r.eq_ignore_ascii_case(p));
            if !super::mods::is_plugin_file(p) || reserved {
                bail!("{id}'s pack has an invalid plugin {p:?}");
            }
        }
        Ok(())
    }
}

/// An installed pack, as the launcher notes it in its choices (`packs` in
/// `launcher-mods.json`): what it put where, and whether it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PackRecord {
    /// The mods catalogue's id.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Its content pack: `bin/win10/content/<content>`.
    pub content: String,
    /// Its plugins in `plugins/`, in load order.
    pub plugins: Vec<String>,
    /// The settings files it put beside them.
    pub settings: Vec<String>,
    pub enabled: bool,
}

impl Default for PackRecord {
    fn default() -> Self {
        PackRecord {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            content: String::new(),
            plugins: Vec::new(),
            settings: Vec::new(),
            enabled: true,
        }
    }
}

impl PackRecord {
    /// Whether `file` is one of its plugins.
    pub fn has_plugin(&self, file: &str) -> bool {
        self.plugins.iter().any(|p| p.eq_ignore_ascii_case(file))
    }
}

/// Pure: whether `name` is a settings file a pack may put beside its plugins (a plain
/// name, `.ini`, `.cfg`, `.txt` or `.json`).
pub fn is_settings_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.len() <= 64
        && !name.starts_with('.')
        && [".ini", ".cfg", ".txt", ".json"]
            .iter()
            .any(|ext| lower.ends_with(ext) && lower.len() > ext.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// What a pack's zip held, unpacked into a staging folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpacked {
    /// Its plugins' file names (in `staging/plugins`), as the spec lists them.
    pub plugins: Vec<String>,
    /// Its settings files (in `staging/plugins`).
    pub settings: Vec<String>,
    /// Its content pack (`staging/content/<content>`).
    pub content: PathBuf,
}

/// Unpacks `zip` into `staging` (made empty first): the spec's plugins and settings files
/// from `plugins/`, and `content/<content>/` as it is. Files at the top (a README) are left
/// out; anything else refuses the pack, as does a plugin it lacks or a content pack
/// without `content.json`.
pub fn unpack(zip: impl Read + Seek, spec: &PackSpec, staging: &Path) -> Result<Unpacked> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).with_context(|| format!("create {}", staging.display()))?;
    let content_root = Path::new(CONTENT).join(&spec.content);
    let mut archive = zip::ZipArchive::new(zip).context("the pack isn't a zip")?;
    let mut plugins = Vec::new();
    let mut settings = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            bail!("the pack has an unsafe path: {}", entry.name());
        };
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if entry.is_dir() {
            continue; // folders come with their files
        }
        let target = match parts.as_slice() {
            [_top] => continue,
            [dir, name] if dir == "plugins" => {
                if let Some(p) = spec.plugins.iter().find(|p| p.eq_ignore_ascii_case(name)) {
                    plugins.push(p.clone());
                } else if is_settings_file(name) {
                    settings.push(name.clone());
                } else {
                    bail!("the pack has a file in plugins/ it doesn't list: {name}");
                }
                staging.join("plugins").join(name)
            }
            _ if rel.starts_with(&content_root) && rel != content_root => staging.join(&rel),
            _ => bail!(
                "the pack has a file outside plugins/ and content/{}/: {}",
                spec.content,
                rel.display()
            ),
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)
            .with_context(|| format!("write {}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("write {}", target.display()))?;
    }
    if let Some(missing) = spec
        .plugins
        .iter()
        .find(|p| !plugins.iter().any(|q| q.eq_ignore_ascii_case(p)))
    {
        bail!("the pack lacks its plugin {missing}");
    }
    let content = staging.join(&content_root);
    if !content.join("content.json").is_file() {
        bail!("the pack has no content/{}/content.json", spec.content);
    }
    Ok(Unpacked {
        plugins: spec.plugins.clone(),
        settings,
        content,
    })
}

/// Pure: whether the nEVR build `slot` (its bytes) has the early load pass content packs
/// need (it logs "... marked early").
pub fn loads_early(slot: &[u8]) -> bool {
    slot.windows(12).any(|w| w == b"marked early")
}

/// Whether the nEVR in `bin`'s slot has the early load pass.
pub fn nevr_loads_early(bin: &Path) -> bool {
    super::mods::slot_facts(bin).1
}

/// The game's folder for the one whose `bin/win10` is `bin`.
fn game_dir(bin: &Path) -> Option<&Path> {
    bin.parent()?.parent()
}

/// What a content pack's `content.json` says about the game it was built from: its data
/// folder and, per manifest it replaces, the stock manifest's sha256.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ContentJson {
    data_dir: String,
    manifests: Vec<StockManifest>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StockManifest {
    package: String,
    stock_sha256: String,
}

/// Pure: a relative path of plain names (`/`), as the overlay takes them.
fn is_plain_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains(['\\', ':']))
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The game's manifest that content pack `content` in `bin` needs as it shipped and that
/// isn't (its package id), as NvrContentOverlay checks it before serving the pack: an
/// older installer repacked the game's data, and the overlay would serve nothing. `None`:
/// all as shipped, or nothing to check (no readable `content.json`; the overlay says why).
/// The manifests are hashed again only when their size or time changed.
pub fn changed_data(bin: &Path, content: &str) -> Option<String> {
    stock_manifests(bin, content)
        .into_iter()
        .find(|(path, stock)| !cached_sha256(path).is_some_and(|h| h.eq_ignore_ascii_case(stock)))
        .and_then(|(path, _)| path.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// The game's manifests content pack `content` in `bin` needs as they shipped, with
/// their stock sha256 (none without a readable `content.json`).
pub fn stock_manifests(bin: &Path, content: &str) -> Vec<(PathBuf, String)> {
    let pack = std::fs::read_to_string(bin.join(CONTENT).join(content).join("content.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<ContentJson>(&t).ok());
    let (Some(pack), Some(game)) = (pack.filter(|p| is_plain_path(&p.data_dir)), game_dir(bin))
    else {
        return Vec::new();
    };
    let manifests = game.join(&pack.data_dir).join("manifests");
    pack.manifests
        .into_iter()
        .filter(|m| is_hex(&m.package, 16) && is_hex(&m.stock_sha256, 64))
        .map(|m| (manifests.join(&m.package), m.stock_sha256))
        .collect()
}

/// NvrContentOverlay's log, in its default folder (relative to `bin/win10`).
const OVERLAY_LOG: &str = "plugin_logs/NvrContentOverlay/NvrContentOverlay.log";

/// Why NvrContentOverlay served nothing at its last start, as its log says it (the line
/// before "the game runs without the pack"), in words for the Mods page. `None`: it
/// served the pack, or it never ran.
pub fn overlay_refusal(bin: &Path) -> Option<String> {
    let text = std::fs::read_to_string(bin.join(OVERLAY_LOG)).ok()?;
    overlay_refusal_in(&text)
}

/// Pure: [`overlay_refusal`] of the log `text`.
fn overlay_refusal_in(text: &str) -> Option<String> {
    // Each line: "[18:27:37 +394ms] what it says".
    let said: Vec<&str> = text
        .lines()
        .map(|l| l.split_once("] ").map_or(l, |(_, rest)| rest).trim())
        .collect();
    let start = said
        .iter()
        .rposition(|l| l.starts_with("NvrContentOverlay "))?;
    let last = &said[start..];
    let gave_up = last
        .iter()
        .position(|l| *l == "the game runs without the pack")?;
    let why = last.get(gave_up.checked_sub(1)?)?;
    Some(if why.contains("the pack was built from") {
        "it served nothing: the game's files aren't as they shipped (an older installer changed them). Repair game files puts them back.".into()
    } else {
        format!("it served nothing: {why}")
    })
}

/// `path`'s sha256, kept while its size and time stay the same. `None`: it can't be read.
fn cached_sha256(path: &Path) -> Option<String> {
    type Seen = (u64, std::time::SystemTime, String);
    static SEEN: std::sync::Mutex<std::collections::BTreeMap<PathBuf, Seen>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    let meta = std::fs::metadata(path).ok()?;
    let stamp = (meta.len(), meta.modified().ok()?);
    if let Some((l, t, sha)) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        if (*l, *t) == stamp {
            return Some(sha.clone());
        }
    }
    let sha = crate::core::download::sha256_file(path).ok()?;
    SEEN.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.to_path_buf(), (stamp.0, stamp.1, sha.clone()));
    Some(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live build's data folder, relative to the game's folder.
    const DATA_DIR: &str = "_data/5932408047/rad15/win10";
    use std::io::Write;

    fn spec() -> PackSpec {
        PackSpec {
            url: "packs/Test-1.0.0.zip".into(),
            sha256: "a".repeat(64),
            size: None,
            content: "test".into(),
            plugins: vec!["One.dll".into(), "Two.dll".into()],
        }
    }

    fn zip_of(files: &[(&str, &[u8])]) -> std::io::Cursor<Vec<u8>> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(bytes).unwrap();
        }
        let mut c = w.finish().unwrap();
        c.set_position(0);
        c
    }

    #[test]
    fn a_pack_spec_is_checked() {
        assert!(spec().validate("t").is_ok());
        let bad = |f: &dyn Fn(&mut PackSpec)| {
            let mut s = spec();
            f(&mut s);
            s.validate("t").is_err()
        };
        assert!(bad(&|s| s.url = "https://example.com/x.zip".into()));
        assert!(bad(&|s| s.url = "../x.zip".into()));
        assert!(bad(&|s| s.sha256 = "xyz".into()));
        assert!(bad(&|s| s.content = "../up".into()));
        assert!(bad(&|s| s.plugins = vec!["BugSplat64.dll".into()]));
        assert!(bad(&|s| s.plugins = vec![OVERLAY_PLUGIN.into()]));
        assert!(bad(&|s| s.plugins = vec!["sub/One.dll".into()]));
    }

    #[test]
    fn settings_files_are_plain_names() {
        assert!(is_settings_file("PowerBoost.ini"));
        assert!(is_settings_file("CombatBots_mpl_combat_dyson.json"));
        assert!(!is_settings_file("evil.dll"));
        assert!(!is_settings_file(".ini"));
        assert!(!is_settings_file("a b.ini"));
    }

    #[test]
    fn unpacks_plugins_settings_and_content() {
        let dir = tempfile::tempdir().unwrap();
        let zip = zip_of(&[
            ("README.txt", b"read me"),
            ("plugins/One.dll", b"one"),
            ("plugins/Two.dll", b"two"),
            ("plugins/One.ini", b"[x]"),
            ("content/test/content.json", b"{}"),
            ("content/test/scripts/0123456789abcdef.dll", b"MZ"),
        ]);
        let u = unpack(zip, &spec(), dir.path()).unwrap();
        assert_eq!(u.plugins, vec!["One.dll", "Two.dll"]);
        assert_eq!(u.settings, vec!["One.ini"]);
        assert!(u.content.join("scripts/0123456789abcdef.dll").is_file());
        assert!(dir.path().join("plugins/Two.dll").is_file());
        assert!(!dir.path().join("README.txt").exists());
    }

    #[test]
    fn refuses_what_a_pack_may_not_bring() {
        let refused = |files: &[(&str, &[u8])]| {
            let dir = tempfile::tempdir().unwrap();
            unpack(zip_of(files), &spec(), dir.path()).is_err()
        };
        let base: Vec<(&str, &[u8])> = vec![
            ("plugins/One.dll", b"1"),
            ("plugins/Two.dll", b"2"),
            ("content/test/content.json", b"{}"),
        ];
        let with = |extra: (&'static str, &'static [u8])| {
            let mut v = base.clone();
            v.push(extra);
            v
        };
        assert!(!refused(&base));
        assert!(
            refused(&with(("plugins/Other.dll", b"x"))),
            "an unlisted plugin"
        );
        assert!(
            refused(&with(("bin/win10/dbgcore.dll", b"x"))),
            "a loader beside the exe"
        );
        assert!(
            refused(&with(("content/other/x", b"x"))),
            "another pack's content"
        );
        assert!(
            refused(&with(("plugins/sub/One.dll", b"x"))),
            "a folder in plugins/"
        );
        assert!(refused(&with(("../evil.dll", b"x"))), "a path climbing out");
        assert!(refused(&base[..2]), "no content.json");
        assert!(refused(&[base[0], base[2]]), "a missing plugin");
    }

    #[test]
    fn knows_a_nevr_with_the_early_pass() {
        assert!(loads_early(
            b"..[NEVR.PLUGIN] early pass: %zu plugin(s) marked early; .."
        ));
        assert!(!loads_early(b"nEVR 4.0.1"));
    }

    #[test]
    fn reads_why_the_overlay_served_nothing() {
        let refused = "[18:27:36 +288ms] NvrContentOverlay 1.0.0, echocombat\n\
            [18:27:37 +394ms] pack 'echocombat': the game's manifest 48037dc70b0ecab2 is not stock (sha256 a9), the pack was built from e2: not serving it (restore the stock game files)\n\
            [18:27:37 +394ms] the game runs without the pack\n";
        assert!(overlay_refusal_in(refused)
            .unwrap()
            .contains("aren't as they shipped"));
        let changed = refused.replace(
            "the game's manifest 48037dc70b0ecab2 is not stock (sha256 a9), the pack was built from e2: not serving it (restore the stock game files)",
            "manifests/x is changed (sha256 1, want 2)",
        );
        assert_eq!(
            overlay_refusal_in(&changed).as_deref(),
            Some("it served nothing: pack 'echocombat': manifests/x is changed (sha256 1, want 2)")
        );
        // A later start that served it: nothing to say.
        let served = format!("{refused}[19:00:00 +1ms] NvrContentOverlay 1.0.0, echocombat\n[19:00:00 +2ms] serving the pack\n");
        assert_eq!(overlay_refusal_in(&served), None);
        assert_eq!(overlay_refusal_in(""), None);
    }

    #[test]
    fn sees_game_data_that_isnt_as_it_shipped() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("ready-at-dawn-echo-arena");
        let bin = game.join("bin/win10");
        let manifests = game.join(DATA_DIR).join("manifests");
        let pack = bin.join(CONTENT).join("test");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::create_dir_all(&manifests).unwrap();
        let stock = manifests.join("48037dc70b0ecab2");
        std::fs::write(&stock, b"stock").unwrap();
        let sha = crate::core::download::sha256_file(&stock).unwrap();
        let json = |data_dir: &str| {
            format!(
                r#"{{"format": 1, "id": "test", "data_dir": "{data_dir}",
                    "manifests": [{{"package": "48037dc70b0ecab2", "stock_sha256": "{}"}}]}}"#,
                sha.to_uppercase()
            )
        };
        assert_eq!(
            changed_data(&bin, "test"),
            None,
            "no content.json: nothing to check"
        );
        std::fs::write(pack.join("content.json"), json(DATA_DIR)).unwrap();
        assert_eq!(changed_data(&bin, "test"), None);
        // An old installer's stock copy beside it doesn't matter.
        std::fs::write(manifests.join("48037dc70b0ecab2.bak"), b"stock").unwrap();
        assert_eq!(changed_data(&bin, "test"), None);
        // Repacked (another size, so the cache sees it).
        std::fs::write(&stock, b"repacked").unwrap();
        assert_eq!(
            changed_data(&bin, "test").as_deref(),
            Some("48037dc70b0ecab2")
        );
        std::fs::remove_file(&stock).unwrap();
        assert_eq!(
            changed_data(&bin, "test").as_deref(),
            Some("48037dc70b0ecab2")
        );
        // A data folder climbing out isn't followed.
        std::fs::write(pack.join("content.json"), json("../x")).unwrap();
        assert_eq!(changed_data(&bin, "test"), None);
    }
}
