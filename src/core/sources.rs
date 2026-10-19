//! Where files on release.echovr.de are fetched from: the Bunny CDN (`cdn.echovr.de`, a copy
//! of release.echovr.de that bunny.echo syncs every minute) first, release.echovr.de itself
//! when the CDN fails or is behind.
//!
//! The CDN only ever serves a file whose SHA-256 the launcher knows beforehand, and the
//! download is checked against it: a copy the sync hasn't replaced yet doesn't match and is
//! taken from release.echovr.de instead. For the small files that name everything else (the
//! catalogues, the update manifests, `SHA256SUMS`) the checksums come from
//! release.echovr.de's `index.json`; the big files' checksums come from those. So the CDN
//! can never hand out an older version than release.echovr.de has.
//!
//! When release.echovr.de can't be reached, the CDN's own copy of the index stands in, but
//! never one older than an index already seen (its `serial`): a CDN that is behind then
//! leaves the launcher where it was rather than taking it back.
//!
//! URLs stay release.echovr.de's everywhere else (catalogues, allowed hosts, update
//! manifests); only the fetch is redirected here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Deserialize;

/// release.echovr.de: the source of truth, and the fallback.
pub const ORIGIN: &str = "https://release.echovr.de/";
/// The Bunny pull zone in front of the synced copy of [`ORIGIN`].
pub const CDN: &str = "https://cdn.echovr.de/";
/// The checksums of the small files that name everything else, on [`ORIGIN`].
const INDEX: &str = "index.json";
/// Debug builds: another CDN (`http://127.0.0.1:8799/`, to try a CDN that is behind), or
/// `off`.
const CDN_ENV: &str = "ECHOVR_CDN";
/// How long one index is used before it is asked for again: the catalogues are read every
/// 30 seconds, a full check reads a few files at once.
const INDEX_FOR: Duration = Duration::from_secs(20);
const INDEX_TIMEOUT: Duration = Duration::from_secs(10);

/// Paths the CDN never serves: uploads, the dev folders (rewritten by their developers at
/// any time, codes in the path), the live feed (not synced) and the index itself.
const ORIGIN_ONLY: [&str; 8] = [
    "launcher/logs",
    "launcher/dev/",
    "launcher/feed/",
    "quest/launcher/logs",
    "quest/launcher/dev/",
    "quest/launcher/feed/",
    "stats",
    INDEX,
];

#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    /// Raised whenever any listed file changes.
    pub serial: u64,
    /// Path on [`ORIGIN`] (no leading slash) → lowercase SHA-256.
    #[serde(default)]
    pub files: HashMap<String, String>,
}

impl Index {
    fn parse(text: &str) -> Option<Index> {
        serde_json::from_str(text)
            .map_err(|e| tracing::warn!("sources: index.json unreadable: {e}"))
            .ok()
    }

    fn sha_of(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }
}

/// The index a fetch goes by.
#[derive(Debug, Clone)]
enum Known {
    /// release.echovr.de's, or the CDN's when release.echovr.de can't be reached and it isn't
    /// older than one seen before.
    Index(Arc<Index>),
    /// No usable index (none published, or only an older one on the CDN): release.echovr.de
    /// only, as before the CDN.
    None,
}

/// Pure: the index to go by, from release.echovr.de's (`origin`), else the CDN's, given the
/// highest serial seen so far.
fn pick(origin: Option<Index>, cdn: impl FnOnce() -> Option<Index>, seen: u64) -> Known {
    if let Some(i) = origin {
        return Known::Index(Arc::new(i));
    }
    match cdn() {
        Some(i) if i.serial >= seen => Known::Index(Arc::new(i)),
        Some(i) => {
            tracing::warn!(
                "sources: the CDN's index (serial {}) is older than one seen ({seen}); release.echovr.de only",
                i.serial
            );
            Known::None
        }
        None => Known::None,
    }
}

/// The CDN base in use, or `None` when it is switched off (debug builds only).
fn cdn_base() -> Option<String> {
    if cfg!(debug_assertions) {
        if let Ok(v) = std::env::var(CDN_ENV) {
            let v = v.trim();
            if v.eq_ignore_ascii_case("off") || v.is_empty() {
                return None;
            }
            return Some(if v.ends_with('/') {
                v.to_string()
            } else {
                format!("{v}/")
            });
        }
    }
    Some(CDN.to_string())
}

/// Pure: `url`'s path on release.echovr.de (no leading slash), when it is a plain file
/// there the CDN may serve.
fn cdn_path(url: &str) -> Option<&str> {
    let path = url.strip_prefix(ORIGIN)?;
    if path.is_empty() || path.contains(['?', '#']) || path.split('/').any(|s| s == "..") {
        return None;
    }
    (!ORIGIN_ONLY.iter().any(|p| path.starts_with(p))).then_some(path)
}

/// The CDN's copy of `url` (on release.echovr.de), when it may serve it. Only for a file
/// whose checksum the caller checks.
pub fn cdn_url(url: &str) -> Option<String> {
    let path = cdn_path(url)?;
    Some(format!("{}{path}", cdn_base()?))
}

/// The SHA-256 release.echovr.de's index lists for `url` (the big files it lists too, such
/// as the game's zip).
pub fn indexed_sha(url: &str) -> Option<String> {
    let path = cdn_path(url)?;
    match index() {
        Known::Index(i) => i.sha_of(path).map(str::to_string),
        Known::None => None,
    }
}

static INDEX_CACHE: Mutex<Option<(Instant, Known)>> = Mutex::new(None);

/// The index to go by: release.echovr.de's, else the CDN's (not older than one seen), kept
/// for [`INDEX_FOR`].
fn index() -> Known {
    let mut cache = INDEX_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, known)) = cache.as_ref() {
        if at.elapsed() < INDEX_FOR {
            return known.clone();
        }
    }
    let read = |url: &str| {
        super::http::raw_text(url, INDEX_TIMEOUT)
            .map_err(|e| tracing::info!("sources: {url}: {e:#}"))
            .ok()
            .flatten()
            .and_then(|t| Index::parse(&t))
    };
    let seen = seen_serial();
    let known = pick(
        read(&format!("{ORIGIN}{INDEX}")),
        || cdn_base().and_then(|b| read(&format!("{b}{INDEX}"))),
        seen,
    );
    if let Known::Index(i) = &known {
        if i.serial > seen {
            note_serial(i.serial);
        }
    }
    *cache = Some((Instant::now(), known.clone()));
    known
}

fn serial_path() -> std::path::PathBuf {
    super::paths::data_dir().join("sources.json")
}

#[derive(serde::Serialize, Deserialize, Default)]
struct Seen {
    serial: u64,
}

/// The highest index serial this launcher has seen (0: none yet).
fn seen_serial() -> u64 {
    if cfg!(test) {
        return 0;
    }
    std::fs::read_to_string(serial_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Seen>(&t).ok())
        .map(|s| s.serial)
        .unwrap_or(0)
}

fn note_serial(serial: u64) {
    if cfg!(test) {
        return;
    }
    let _ = std::fs::create_dir_all(super::paths::data_dir());
    if let Ok(t) = serde_json::to_string(&Seen { serial }) {
        let _ = std::fs::write(serial_path(), t);
    }
}

/// Fetches the small text file `url` (`None`: not there, 404/410): from the CDN when it is
/// a file release.echovr.de's index lists and the CDN's copy matches it, else from `url`
/// itself. Any other URL is fetched as it is.
pub fn text(url: &str, timeout: Duration) -> Result<Option<String>> {
    let fetch = |u: &str| super::http::raw_text(u, timeout);
    match cdn_path(url) {
        Some(path) => text_with(url, path, &index(), cdn_base().as_deref(), &fetch),
        None => fetch(url),
    }
}

/// Pure but for `fetch`: [`text`] given the index and CDN base.
fn text_with(
    url: &str,
    path: &str,
    known: &Known,
    cdn: Option<&str>,
    fetch: &dyn Fn(&str) -> Result<Option<String>>,
) -> Result<Option<String>> {
    if let (Known::Index(i), Some(cdn)) = (known, cdn) {
        if let Some(sha) = i.sha_of(path) {
            let from = format!("{cdn}{path}");
            match fetch(&from) {
                Ok(Some(t)) if sha256_hex(t.as_bytes()).eq_ignore_ascii_case(sha) => {
                    return Ok(Some(t))
                }
                Ok(Some(_)) => tracing::info!(
                    "sources: the CDN's {path} is behind release.echovr.de's; taking that"
                ),
                Ok(None) => tracing::info!("sources: the CDN doesn't have {path} yet"),
                Err(e) => tracing::info!("sources: CDN {path}: {e:#}"),
            }
        }
    }
    fetch(url)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::cell::RefCell;

    fn index(serial: u64, files: &[(&str, &str)]) -> Index {
        Index {
            serial,
            files: files
                .iter()
                .map(|(p, t)| (p.to_string(), sha256_hex(t.as_bytes())))
                .collect(),
        }
    }

    const MODS: &str = "https://release.echovr.de/launcher/mods.json";
    const CDN_MODS: &str = "https://cdn.echovr.de/launcher/mods.json";

    /// A fake network: URL → body (`None`: 404), `Err` for an unreachable host.
    fn net<'a>(
        answers: &'a [(&'a str, Result<Option<&'a str>, ()>)],
        asked: &'a RefCell<Vec<String>>,
    ) -> impl Fn(&str) -> Result<Option<String>> + 'a {
        move |u| {
            asked.borrow_mut().push(u.to_string());
            match answers.iter().find(|(a, _)| *a == u) {
                Some((_, Ok(b))) => Ok(b.map(str::to_string)),
                _ => Err(anyhow!("unreachable")),
            }
        }
    }

    fn known(i: Index) -> Known {
        Known::Index(Arc::new(i))
    }

    #[test]
    fn the_cdn_serves_a_file_that_matches_the_index() {
        let asked = RefCell::new(Vec::new());
        let k = known(index(5, &[("launcher/mods.json", "new")]));
        let answers = [(CDN_MODS, Ok(Some("new"))), (MODS, Ok(Some("new")))];
        let t = text_with(
            MODS,
            "launcher/mods.json",
            &k,
            Some(CDN),
            &net(&answers, &asked),
        );
        assert_eq!(t.unwrap().as_deref(), Some("new"));
        assert_eq!(*asked.borrow(), [CDN_MODS]);
    }

    #[test]
    fn a_cdn_copy_that_is_behind_is_taken_from_release_instead() {
        let k = known(index(5, &[("launcher/mods.json", "new")]));
        for cdn in [Ok(Some("old")), Ok(None), Err(())] {
            let asked = RefCell::new(Vec::new());
            let answers = [(CDN_MODS, cdn), (MODS, Ok(Some("new")))];
            let t = text_with(
                MODS,
                "launcher/mods.json",
                &k,
                Some(CDN),
                &net(&answers, &asked),
            );
            assert_eq!(t.unwrap().as_deref(), Some("new"));
            assert_eq!(*asked.borrow(), [CDN_MODS, MODS]);
        }
    }

    #[test]
    fn without_an_index_entry_only_release_is_asked() {
        let asked = RefCell::new(Vec::new());
        let answers = [(CDN_MODS, Ok(Some("old"))), (MODS, Ok(Some("new")))];
        for k in [known(index(5, &[])), Known::None] {
            asked.borrow_mut().clear();
            let t = text_with(
                MODS,
                "launcher/mods.json",
                &k,
                Some(CDN),
                &net(&answers, &asked),
            );
            assert_eq!(t.unwrap().as_deref(), Some("new"));
            assert_eq!(*asked.borrow(), [MODS]);
        }
        // The CDN switched off.
        asked.borrow_mut().clear();
        let k = known(index(5, &[("launcher/mods.json", "old")]));
        let t = text_with(MODS, "launcher/mods.json", &k, None, &net(&answers, &asked));
        assert_eq!(t.unwrap().as_deref(), Some("new"));
        assert_eq!(*asked.borrow(), [MODS]);
    }

    #[test]
    fn release_answers_even_when_it_differs_from_the_index() {
        // Published between the index and the file: release.echovr.de is the truth.
        let asked = RefCell::new(Vec::new());
        let k = known(index(5, &[("launcher/mods.json", "new")]));
        let answers = [(CDN_MODS, Ok(Some("old"))), (MODS, Ok(Some("newer")))];
        let t = text_with(
            MODS,
            "launcher/mods.json",
            &k,
            Some(CDN),
            &net(&answers, &asked),
        );
        assert_eq!(t.unwrap().as_deref(), Some("newer"));
    }

    #[test]
    fn the_cdn_index_stands_in_only_when_not_older_than_one_seen() {
        let got = |k: Known| match k {
            Known::Index(i) => Some(i.serial),
            Known::None => None,
        };
        // release.echovr.de's always wins, and the CDN isn't asked.
        let k = pick(Some(index(3, &[])), || panic!("CDN asked"), 9);
        assert_eq!(got(k), Some(3));
        // release.echovr.de unreachable: the CDN's, if it is as new as the last seen.
        assert_eq!(got(pick(None, || Some(index(9, &[])), 9)), Some(9));
        assert_eq!(got(pick(None, || Some(index(10, &[])), 9)), Some(10));
        assert_eq!(got(pick(None, || Some(index(8, &[])), 9)), None);
        assert_eq!(got(pick(None, || None, 9)), None);
    }

    #[test]
    fn the_cdn_serves_release_paths_only_and_never_the_live_ones() {
        assert_eq!(
            cdn_url("https://release.echovr.de/updates-nevr/BugSplat64.dll").as_deref(),
            Some("https://cdn.echovr.de/updates-nevr/BugSplat64.dll")
        );
        assert_eq!(
            cdn_url("https://release.echovr.de/pc.zip").as_deref(),
            Some("https://cdn.echovr.de/pc.zip")
        );
        for url in [
            "https://release.echovr.de/launcher/logs",
            "https://release.echovr.de/launcher/dev/abc/mods.json",
            "https://release.echovr.de/launcher/feed/servers.json",
            "https://release.echovr.de/quest/launcher/dev/abc/versions.json",
            "https://release.echovr.de/stats/",
            "https://release.echovr.de/index.json",
            "https://release.echovr.de/pc.zip?x=1",
            "https://release.echovr.de/../etc/passwd",
            "https://release.echovr.de/",
            "https://files.echovr.de/updates/update.manifest",
            "https://evr.echo.taxi/pc.zip",
            "https://github.com/marshmallow-mia/EchoVR_Launcher/releases/download/v1/x.zip",
        ] {
            assert_eq!(cdn_url(url), None, "{url}");
        }
    }

    /// Against release.echovr.de (needs network): what comes back matches its index, from
    /// the CDN or not. With `ECHOVR_CDN=http://127.0.0.1:8799` serving older copies, they are
    /// passed over (that server's log shows them asked for).
    #[test]
    #[ignore]
    fn live_files_match_the_index() {
        let Known::Index(i) = super::index() else {
            panic!("no index on release.echovr.de");
        };
        for path in [
            "launcher/mods.json",
            "launcher/plugins.json",
            "updates-nevr/update.manifest",
            "launcher/download/SHA256SUMS",
        ] {
            let url = format!("{ORIGIN}{path}");
            let t = text(&url, Duration::from_secs(60)).unwrap().unwrap();
            assert_eq!(
                Some(sha256_hex(t.as_bytes()).as_str()),
                i.sha_of(path),
                "{path}"
            );
        }
        // A big file: the CDN's copy only if it matches.
        let dir = tempfile::tempdir().unwrap();
        let url = format!("{ORIGIN}updates-nevr/BugSplat64.dll");
        let manifest = text(
            &format!("{ORIGIN}updates-nevr/update.manifest"),
            INDEX_TIMEOUT,
        )
        .unwrap()
        .unwrap();
        let sha = manifest
            .lines()
            .find_map(|l| {
                l.split_whitespace()
                    .collect::<Vec<_>>()
                    .get(..3)
                    .and_then(|w| {
                        (w[0] == "add" && w[1] == "BugSplat64.dll").then(|| w[2].to_string())
                    })
            })
            .unwrap();
        let dest = dir.path().join("BugSplat64.dll");
        crate::core::http::download_checked(&url, &sha, &dest, None).unwrap();
        assert!(crate::core::download::sha256_matches(&dest, &sha));
        // A zip whose checksum only the index has (as the game's).
        let path = "ready-at-dawn-echo-arena_assets.zip";
        let job = crate::core::download::Job {
            url: format!("{ORIGIN}{path}"),
            dir: dir.path().to_path_buf(),
            filename: "assets.zip".into(),
            use_mirror: false,
            fresh: false,
            extract: false,
            sha256: None,
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let got = crate::core::download::run(&job, &cancel, &mut |_| {}).unwrap();
        assert!(crate::core::download::sha256_matches(
            &got,
            i.sha_of(path).unwrap()
        ));
    }

    #[test]
    fn index_format() {
        let i = Index::parse(r#"{"schema":1,"serial":42,"files":{"launcher/mods.json":"AB"}}"#)
            .unwrap();
        assert_eq!(i.serial, 42);
        assert_eq!(i.sha_of("launcher/mods.json"), Some("AB"));
        assert!(Index::parse("not json").is_none());
    }
}
