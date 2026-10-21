//! Where files on release.echovr.de are fetched from: the Bunny CDN (`cdn.echovr.de`, a copy
//! of release.echovr.de that bunny.echo syncs every minute) first, release.echovr.de itself
//! only when the CDN fails, is behind, or doesn't have the file yet.
//!
//! The launcher asks the CDN for a file by its SHA-256: release.echovr.de keeps every public
//! file under `store/sha256/<aa>/<sha>` too (`echo-cdn-index`), and such an address only ever
//! holds that one file. So the CDN can't hand out an old copy, or one its cache mixed from two
//! versions, and the download is still checked. The checksums of the small files that name
//! everything else (the catalogues, the update manifests, `SHA256SUMS`) come from an
//! `index.json`; the big files' checksums come from those. The store keeps earlier versions
//! too, so going back to one ([`super::launcher::history`]) works for everyone.
//!
//! The index itself comes from the CDN as well, unless
//! - it is older than an index already seen (its `serial`): the CDN is behind;
//! - echo-cdn-index last `checked` it more than [`STALE`] before the CDN sent it: the sync to
//!   the CDN has stopped;
//!
//! - it has no valid signature: `index.json.sig` beside it holds Ed25519 signatures of its exact
//!   bytes by a key in [`SIGNING_KEYS`] (echo-cdn-index on release.echo signs every index). So a
//!   CDN, a Bunny account or a sync that was tampered with can't hand out other files: what the
//!   index names is checked by its checksum, and the index by its signature;
//!
//! then release.echovr.de's is used (over TLS: it is the source of truth). A dev folder
//! (`launcher/dev/<code>/`) has an index of its own, for its catalogues and files, read the
//! same way.
//!
//! A CDN that can't be reached (no connection, a timeout) is left out for [`CDN_PAUSE`]: until
//! then everything comes straight from release.echovr.de.
//!
//! URLs stay release.echovr.de's everywhere else (catalogues, allowed hosts, update
//! manifests); only the fetch is redirected here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Deserialize;
use time::OffsetDateTime;

/// release.echovr.de: the source of truth, and the fallback.
pub const ORIGIN: &str = "https://release.echovr.de/";
/// The Bunny pull zone in front of the synced copy of [`ORIGIN`].
pub const CDN: &str = "https://cdn.echovr.de/";
/// The checksums of the small files that name everything else (and in each dev folder, of
/// everything in it).
const INDEX: &str = "index.json";
/// Beside each index: its signatures ([`signed`]).
const SIG: &str = "index.json.sig";
/// The Ed25519 public keys an index may be signed with (`echo-cdn-index keygen` prints one). A
/// new key is added here a release before the server signs with it.
const SIGNING_KEYS: [[u8; 32]; 1] = [[
    0xce, 0x04, 0x7f, 0xc8, 0x2a, 0x3d, 0x30, 0x8a, 0xa1, 0x6b, 0x6e, 0xe1, 0x1d, 0xcf, 0xe7, 0xb1,
    0xf5, 0x57, 0xf0, 0x46, 0x59, 0xa4, 0x89, 0x59, 0xa0, 0xe7, 0x07, 0x84, 0xf3, 0x4e, 0x15, 0x95,
]];
/// Every public file by its checksum, on [`ORIGIN`] and [`CDN`]: `<STORE><aa>/<sha>`.
const STORE: &str = "store/sha256/";
/// Debug builds: another CDN (`http://127.0.0.1:8799/`, to try a CDN that is behind), or
/// `off`.
const CDN_ENV: &str = "ECHOVR_CDN";
/// How long one index is used before it is asked for again: the catalogues are read every
/// 30 seconds, a full check reads a few files at once.
const INDEX_FOR: Duration = Duration::from_secs(20);
const INDEX_TIMEOUT: Duration = Duration::from_secs(10);
/// An index on the CDN that echo-cdn-index checked longer ago than this (it does at least
/// every 5 minutes): the sync to the CDN has stopped.
const STALE: Duration = Duration::from_secs(20 * 60);
/// How long a connection to the CDN may take: release.echovr.de is there to fall back on.
pub const CDN_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the CDN is left out after it couldn't be reached.
const CDN_PAUSE: Duration = Duration::from_secs(10 * 60);

/// Paths the CDN never serves: uploads, the live feed (not synced), what's behind the
/// EchoVRCE sign-in, stats and the index itself (the main one is read here, not as a file).
const ORIGIN_ONLY: [&str; 7] = [
    "launcher/logs",
    "launcher/feed/",
    "launcher/gated/",
    "quest/launcher/logs",
    "quest/launcher/feed/",
    "stats",
    INDEX,
];
/// Where the dev folders are, each with an index of its own.
const DEV_ROOTS: [&str; 2] = ["launcher/dev/", "quest/launcher/dev/"];

#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    /// Raised whenever any listed file changes.
    pub serial: u64,
    /// When echo-cdn-index last looked (RFC 3339), changed or not.
    #[serde(default)]
    pub checked: Option<String>,
    /// Path on [`ORIGIN`] (no leading slash; in a dev folder's, inside the folder) →
    /// lowercase SHA-256.
    #[serde(default)]
    pub files: HashMap<String, String>,
}

/// `index.json.sig`: the signatures of the index now and of the one before it (the CDN may
/// have either).
#[derive(Debug, Deserialize)]
struct Signatures {
    #[serde(default)]
    signatures: Vec<Signature>,
}

#[derive(Debug, Deserialize)]
struct Signature {
    /// The first 16 hex digits of the public key's SHA-256.
    key: String,
    /// Of the index it signs.
    sha256: String,
    /// Base64, 64 bytes.
    sig: String,
}

/// Pure: whether `sigs` (an `index.json.sig`) holds a valid signature of exactly `text` by one
/// of `keys`.
fn signed(text: &str, sigs: &str, keys: &[[u8; 32]]) -> bool {
    use base64::Engine;
    use ed25519_dalek::{Signature as Sig, Verifier, VerifyingKey};
    let Ok(sigs) = serde_json::from_str::<Signatures>(sigs) else {
        return false;
    };
    let sha = sha256_hex(text.as_bytes());
    sigs.signatures
        .iter()
        .filter(|s| s.sha256.eq_ignore_ascii_case(&sha))
        .any(|s| {
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&s.sig) else {
                return false;
            };
            let Ok(sig) = Sig::from_slice(&bytes) else {
                return false;
            };
            keys.iter().any(|k| {
                sha256_hex(k)[..16].eq_ignore_ascii_case(&s.key)
                    && VerifyingKey::from_bytes(k)
                        .is_ok_and(|vk| vk.verify(text.as_bytes(), &sig).is_ok())
            })
        })
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

    /// Whether the sync to the CDN has stopped: echo-cdn-index checked this copy more than
    /// [`STALE`] before the CDN sent it (`sent`). One that doesn't say when is taken for
    /// stale too.
    fn stale(&self, sent: OffsetDateTime) -> bool {
        let checked = self.checked.as_deref().and_then(|c| {
            OffsetDateTime::parse(c, &time::format_description::well_known::Rfc3339).ok()
        });
        checked.is_none_or(|c| sent - c > STALE)
    }
}

/// The index a fetch goes by.
#[derive(Debug, Clone)]
enum Known {
    /// The CDN's, or release.echovr.de's when the CDN's can't be used.
    Index(Arc<Index>),
    /// No usable index (none published, or only an older one on the CDN): release.echovr.de
    /// only, as before the CDN.
    None,
}

/// Pure: the index to go by. The CDN's (with when the CDN sent it), unless it is older than
/// the highest serial `seen` or stale; then release.echovr.de's (`origin`). A stale CDN copy
/// is still better than none when release.echovr.de can't be reached either.
fn pick(
    cdn: Option<(Index, OffsetDateTime)>,
    origin: impl FnOnce() -> Option<Index>,
    seen: u64,
) -> Known {
    let mut stale = None;
    match cdn {
        Some((i, _)) if i.serial < seen => tracing::warn!(
            "sources: the CDN's index (serial {}) is older than one seen ({seen}); from release.echovr.de",
            i.serial
        ),
        Some((i, sent)) if i.stale(sent) => {
            tracing::warn!(
                "sources: the CDN's index was last checked {}; from release.echovr.de",
                i.checked.as_deref().unwrap_or("never")
            );
            stale = Some(i);
        }
        Some((i, _)) => return Known::Index(Arc::new(i)),
        None => {}
    }
    match origin().or(stale) {
        Some(i) => Known::Index(Arc::new(i)),
        None => Known::None,
    }
}

/// The CDN base configured: [`CDN`], or in debug builds another one or none.
fn configured_cdn() -> Option<String> {
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

/// When the CDN couldn't be reached last, while that is less than [`CDN_PAUSE`] ago.
static CDN_DOWN: Mutex<Option<Instant>> = Mutex::new(None);

/// The CDN base in use: `None` when it is switched off, or left out for now because it
/// couldn't be reached.
fn cdn_base() -> Option<String> {
    let mut down = CDN_DOWN.lock().unwrap_or_else(|p| p.into_inner());
    match *down {
        Some(at) if at.elapsed() < CDN_PAUSE => return None,
        Some(_) => {
            tracing::info!("sources: trying the CDN again");
            *down = None;
        }
        None => {}
    }
    configured_cdn()
}

/// Whether `url` is on the CDN.
pub fn is_cdn(url: &str) -> bool {
    configured_cdn().is_some_and(|b| url.starts_with(&b))
}

/// A request to `url` failed with `e`: one to the CDN that got no answer at all (no
/// connection, a timeout) leaves the CDN out for [`CDN_PAUSE`]. A 404 or a wrong file
/// doesn't: the CDN is there, release.echovr.de stands in for that one file.
pub fn failed(url: &str, e: &anyhow::Error) {
    if let Some(r) = e.chain().find_map(|c| c.downcast_ref::<reqwest::Error>()) {
        failed_request(url, r);
    }
}

/// [`failed`] with the request's own error.
pub fn failed_request(url: &str, e: &reqwest::Error) {
    if !(e.is_connect() || e.is_timeout()) || !is_cdn(url) {
        return;
    }
    let mut down = CDN_DOWN.lock().unwrap_or_else(|p| p.into_inner());
    if down.is_none() {
        tracing::warn!(
            "sources: the CDN can't be reached ({e}); release.echovr.de only for {} minutes",
            CDN_PAUSE.as_secs() / 60
        );
    }
    *down = Some(Instant::now());
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

/// Pure: which index lists `path`: (the folder it is in, with a trailing slash, or "" for
/// the main one; `path` inside that folder).
fn index_of(path: &str) -> (&str, &str) {
    for root in DEV_ROOTS {
        if let Some(rest) = path.strip_prefix(root) {
            if let Some(i) = rest.find('/').filter(|i| *i > 0) {
                return path.split_at(root.len() + i + 1);
            }
        }
    }
    ("", path)
}

/// Pure: the address of the file with `sha` on `base` (the CDN, or release.echovr.de); it
/// only ever holds that file.
fn store_url(base: &str, sha: &str) -> Option<String> {
    let sha = sha.to_ascii_lowercase();
    (sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("{base}{STORE}{}/{sha}", &sha[..2]))
}

/// Where the file `url` (on release.echovr.de) that must have `sha` can be had by its
/// checksum, in order: the CDN's store, then release.echovr.de's. Empty for a file elsewhere
/// or a live one. `url` itself comes after these (the caller's fallback).
pub fn by_checksum(url: &str, sha: &str) -> Vec<String> {
    if cdn_path(url).is_none() {
        return Vec::new();
    }
    let mut out: Vec<String> = cdn_base()
        .and_then(|cdn| store_url(&cdn, sha))
        .into_iter()
        .collect();
    out.extend(store_url(ORIGIN, sha).filter(|u| u != url));
    out
}

/// The address on release.echovr.de of the file with `sha`, for a version only the store
/// still has (an earlier one, [`super::launcher::history`]).
pub fn origin_store_url(sha: &str) -> Option<String> {
    store_url(ORIGIN, sha)
}

/// The text with `sha` (an earlier update manifest) by its checksum: from the CDN's store,
/// else release.echovr.de's, checked. `None`: release.echovr.de doesn't have it.
pub fn stored_text(sha: &str) -> Result<Option<String>> {
    let origin = store_url(ORIGIN, sha).ok_or_else(|| anyhow::anyhow!("not a checksum: {sha}"))?;
    let mut tries = by_checksum(&origin, sha);
    tries.push(origin);
    let mut out = Ok(None);
    for from in tries {
        out = match super::http::raw_text(&from, Duration::from_secs(60)) {
            Ok(Some(t)) if sha256_hex(t.as_bytes()).eq_ignore_ascii_case(sha) => {
                return Ok(Some(t))
            }
            Ok(Some(_)) => {
                tracing::warn!("sources: {from} isn't what its checksum says");
                Ok(None)
            }
            other => other,
        };
    }
    out
}

/// The SHA-256 an index lists for `url` (the big files it lists too, such as the game's
/// zip; in a dev folder, everything in it).
pub fn indexed_sha(url: &str) -> Option<String> {
    let (folder, path) = index_of(cdn_path(url)?);
    match index(folder) {
        Known::Index(i) => i.sha_of(path).map(str::to_string),
        Known::None => None,
    }
}

/// Each folder's index ("" for the main one) as last read.
static INDEX_CACHE: Mutex<Option<HashMap<String, (Instant, Known)>>> = Mutex::new(None);

/// `text` without dev codes, for logs.
fn redact(text: &str) -> String {
    super::launcher::dev_code::redact(text)
}

/// The index of `folder` ("" for the main one, else a dev folder's path with a trailing
/// slash) to go by: the CDN's, else release.echovr.de's ([`pick`]), kept for [`INDEX_FOR`].
fn index(folder: &str) -> Known {
    let mut cache = INDEX_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((at, known)) = cache.get(folder) {
        if at.elapsed() < INDEX_FOR {
            return known.clone();
        }
    }
    let fetch = |url: &str| {
        super::http::dated_text(url, INDEX_TIMEOUT)
            .map_err(|e| tracing::info!("sources: {}: {}", redact(url), redact(&format!("{e:#}"))))
            .ok()
            .flatten()
    };
    let read = |url: &str| fetch(url).and_then(|(t, sent)| Some((Index::parse(&t)?, sent)));
    let seen = seen_serial(folder);
    let known = pick(
        cdn_base().and_then(|b| {
            let (text, sent) = fetch(&format!("{b}{folder}{INDEX}"))?;
            let sigs = fetch(&format!("{b}{folder}{SIG}")).map(|(t, _)| t);
            if !sigs.is_some_and(|s| signed(&text, &s, &SIGNING_KEYS)) {
                tracing::warn!(
                    "sources: the CDN's {} has no valid signature; from release.echovr.de",
                    redact(&format!("{folder}{INDEX}"))
                );
                return None;
            }
            Some((
                Index::parse(&text)?,
                sent.unwrap_or_else(OffsetDateTime::now_utc),
            ))
        }),
        || read(&format!("{ORIGIN}{folder}{INDEX}")).map(|(i, _)| i),
        seen,
    );
    if let Known::Index(i) = &known {
        if i.serial > seen {
            note_serial(folder, i.serial);
        }
    }
    cache.insert(folder.to_string(), (Instant::now(), known.clone()));
    known
}

fn serial_path() -> std::path::PathBuf {
    super::paths::data_dir().join("sources.json")
}

#[derive(serde::Serialize, Deserialize, Default)]
struct Seen {
    serial: u64,
    /// Each dev folder's, by a hash of its path (never the code itself).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    folders: HashMap<String, u64>,
}

/// Pure: the key of `folder`'s serial in [`Seen::folders`].
fn folder_key(folder: &str) -> String {
    sha256_hex(folder.as_bytes())[..16].to_string()
}

fn read_seen() -> Seen {
    std::fs::read_to_string(serial_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Seen>(&t).ok())
        .unwrap_or_default()
}

/// The highest serial of `folder`'s index this launcher has seen (0: none yet).
fn seen_serial(folder: &str) -> u64 {
    if cfg!(test) {
        return 0;
    }
    let seen = read_seen();
    if folder.is_empty() {
        seen.serial
    } else {
        seen.folders.get(&folder_key(folder)).copied().unwrap_or(0)
    }
}

fn note_serial(folder: &str, serial: u64) {
    if cfg!(test) {
        return;
    }
    let mut seen = read_seen();
    if folder.is_empty() {
        seen.serial = serial;
    } else {
        seen.folders.insert(folder_key(folder), serial);
    }
    let _ = std::fs::create_dir_all(super::paths::data_dir());
    if let Ok(t) = serde_json::to_string(&seen) {
        let _ = std::fs::write(serial_path(), t);
    }
}

/// Fetches the small text file `url` (`None`: not there, 404/410): from the CDN's store when
/// it is a file an index lists (and what comes matches), else from `url` itself. Any other
/// URL is fetched as it is.
pub fn text(url: &str, timeout: Duration) -> Result<Option<String>> {
    let fetch = |u: &str| super::http::raw_text(u, timeout);
    match cdn_path(url) {
        Some(path) => {
            let (folder, inside) = index_of(path);
            text_with(url, inside, &index(folder), cdn_base().as_deref(), &fetch)
        }
        None => fetch(url),
    }
}

/// Pure but for `fetch`: [`text`] given the index (`path` as it lists it) and CDN base.
fn text_with(
    url: &str,
    path: &str,
    known: &Known,
    cdn: Option<&str>,
    fetch: &dyn Fn(&str) -> Result<Option<String>>,
) -> Result<Option<String>> {
    if let (Known::Index(i), Some(cdn)) = (known, cdn) {
        if let Some((sha, from)) = i
            .sha_of(path)
            .and_then(|sha| Some((sha, store_url(cdn, sha)?)))
        {
            match fetch(&from) {
                Ok(Some(t)) if sha256_hex(t.as_bytes()).eq_ignore_ascii_case(sha) => {
                    return Ok(Some(t))
                }
                Ok(Some(_)) => tracing::warn!(
                    "sources: the CDN's {from} isn't what its checksum says; from release.echovr.de"
                ),
                Ok(None) => tracing::info!("sources: the CDN doesn't have {} yet", redact(url)),
                Err(e) => tracing::info!("sources: CDN {}: {e:#}", redact(url)),
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
            checked: Some("2026-10-19T12:00:00+00:00".into()),
            files: files
                .iter()
                .map(|(p, t)| (p.to_string(), sha256_hex(t.as_bytes())))
                .collect(),
        }
    }

    const MODS: &str = "https://release.echovr.de/launcher/mods.json";

    /// The CDN's store address of a file with this text.
    fn cdn_of(text: &str) -> String {
        store_url(CDN, &sha256_hex(text.as_bytes())).unwrap()
    }

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
        let cdn = cdn_of("new");
        let answers = [(cdn.as_str(), Ok(Some("new"))), (MODS, Ok(Some("new")))];
        let t = text_with(
            MODS,
            "launcher/mods.json",
            &k,
            Some(CDN),
            &net(&answers, &asked),
        );
        assert_eq!(t.unwrap().as_deref(), Some("new"));
        assert_eq!(*asked.borrow(), [cdn]);
    }

    #[test]
    fn a_cdn_copy_that_isnt_the_file_is_taken_from_release_instead() {
        let k = known(index(5, &[("launcher/mods.json", "new")]));
        let addr = cdn_of("new");
        // Mixed by the CDN's cache, not there yet, the CDN down.
        for cdn in [Ok(Some("nld")), Ok(None), Err(())] {
            let asked = RefCell::new(Vec::new());
            let answers = [(addr.as_str(), cdn), (MODS, Ok(Some("new")))];
            let t = text_with(
                MODS,
                "launcher/mods.json",
                &k,
                Some(CDN),
                &net(&answers, &asked),
            );
            assert_eq!(t.unwrap().as_deref(), Some("new"));
            assert_eq!(*asked.borrow(), [addr.clone(), MODS.to_string()]);
        }
    }

    #[test]
    fn without_an_index_entry_only_release_is_asked() {
        let asked = RefCell::new(Vec::new());
        let old = cdn_of("old");
        let answers = [(old.as_str(), Ok(Some("old"))), (MODS, Ok(Some("new")))];
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
        let new = cdn_of("new");
        let answers = [(new.as_str(), Ok(None)), (MODS, Ok(Some("newer")))];
        let t = text_with(
            MODS,
            "launcher/mods.json",
            &k,
            Some(CDN),
            &net(&answers, &asked),
        );
        assert_eq!(t.unwrap().as_deref(), Some("newer"));
    }

    /// When the CDN sent an index checked at 12:00, `minutes` later.
    // A key and a signature made with OpenSSL 3 (`openssl pkeyutl -sign -rawin`), as
    // echo-cdn-index makes them.
    const TEST_KEY: [u8; 32] = [
        0x9d, 0x9f, 0x07, 0x72, 0x12, 0x2c, 0xb7, 0x08, 0x0b, 0x11, 0x7f, 0xea, 0x2b, 0x90, 0x5c,
        0xd0, 0x4e, 0x7d, 0xbd, 0xb6, 0x13, 0x39, 0x4b, 0x85, 0x20, 0x2a, 0x55, 0x56, 0xe0, 0xbd,
        0x57, 0x4d,
    ];
    const TEST_TEXT: &str = "{\"serial\": 7}\n";
    const TEST_SIG: &str =
        "5TUonRDBwkB2yJ+sqgw0VftyQyA8ZzKHEIUkPBqM9pqx+QvWErZKB90m9aVXGYAxzrHFmhgCLiLoKGx7a/0hDA==";

    fn sig_file(key: &str, sha: &str, sig: &str) -> String {
        format!(
            r#"{{"schema": 1, "signatures": [{{"key": "{key}", "sha256": "{sha}", "sig": "{sig}"}}]}}"#
        )
    }

    #[test]
    fn an_index_counts_only_with_its_signature() {
        let sha = sha256_hex(TEST_TEXT.as_bytes());
        let good = sig_file("b843e365d60c89af", &sha, TEST_SIG);
        assert!(signed(TEST_TEXT, &good, &[TEST_KEY]));
        // Another text, another key, a key named wrongly, a broken file: no.
        assert!(!signed("{\"serial\": 8}\n", &good, &[TEST_KEY]));
        assert!(!signed(TEST_TEXT, &good, &SIGNING_KEYS));
        assert!(!signed(
            TEST_TEXT,
            &sig_file("0000000000000000", &sha, TEST_SIG),
            &[TEST_KEY]
        ));
        assert!(!signed(TEST_TEXT, "not json", &[TEST_KEY]));
        let mut bad = TEST_SIG.to_string();
        bad.replace_range(0..1, "6");
        assert!(!signed(
            TEST_TEXT,
            &sig_file("b843e365d60c89af", &sha, &bad),
            &[TEST_KEY]
        ));
        // The one before is kept beside it: either matches.
        let both = format!(
            r#"{{"signatures": [{{"key": "x", "sha256": "{}", "sig": "AA=="}}, {{"key": "b843e365d60c89af", "sha256": "{sha}", "sig": "{TEST_SIG}"}}]}}"#,
            "0".repeat(64)
        );
        assert!(signed(TEST_TEXT, &both, &[TEST_KEY]));
    }

    fn sent(minutes: i64) -> OffsetDateTime {
        OffsetDateTime::parse(
            "2026-10-19T12:00:00+00:00",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap()
            + time::Duration::minutes(minutes)
    }

    #[test]
    fn the_cdn_index_is_used_unless_it_is_behind_or_stale() {
        let got = |k: Known| match k {
            Known::Index(i) => Some(i.serial),
            Known::None => None,
        };
        // The CDN's, as new as the last seen or newer: release.echovr.de isn't asked.
        let k = pick(
            Some((index(9, &[]), sent(1))),
            || panic!("release asked"),
            9,
        );
        assert_eq!(got(k), Some(9));
        assert_eq!(
            got(pick(Some((index(10, &[]), sent(19))), || None, 9)),
            Some(10)
        );
        // Behind: release.echovr.de's (the truth, whatever its serial); none, if it can't
        // be reached either.
        assert_eq!(
            got(pick(
                Some((index(8, &[]), sent(1))),
                || Some(index(7, &[])),
                9
            )),
            Some(7)
        );
        assert_eq!(got(pick(Some((index(8, &[]), sent(1))), || None, 9)), None);
        // Stale (the sync stopped): release.echovr.de's; the CDN's when it can't be reached.
        assert_eq!(
            got(pick(
                Some((index(9, &[]), sent(21))),
                || Some(index(12, &[])),
                9
            )),
            Some(12)
        );
        assert_eq!(
            got(pick(Some((index(9, &[]), sent(21))), || None, 9)),
            Some(9)
        );
        let mut unsaid = index(9, &[]);
        unsaid.checked = None;
        assert_eq!(
            got(pick(Some((unsaid, sent(0))), || Some(index(11, &[])), 9)),
            Some(11)
        );
        // The CDN can't be reached: release.echovr.de's.
        assert_eq!(got(pick(None, || Some(index(3, &[])), 9)), Some(3));
        assert_eq!(got(pick(None, || None, 9)), None);
    }

    #[test]
    fn a_dev_folder_has_an_index_of_its_own() {
        assert_eq!(index_of("launcher/mods.json"), ("", "launcher/mods.json"));
        assert_eq!(
            index_of("launcher/dev/abc/files/C-1.1.8.zip"),
            ("launcher/dev/abc/", "files/C-1.1.8.zip")
        );
        assert_eq!(
            index_of("quest/launcher/dev/q1/versions.json"),
            ("quest/launcher/dev/q1/", "versions.json")
        );
        // Not inside a folder.
        assert_eq!(index_of("launcher/dev/x"), ("", "launcher/dev/x"));
        assert_eq!(index_of("launcher/dev//x"), ("", "launcher/dev//x"));
        // Its serial is kept under a hash, never the code.
        let key = folder_key("launcher/dev/mysecretcode/");
        assert_eq!(key.len(), 16);
        assert!(!key.contains("mysecret"));
        // Its catalogue by checksum, as the folder's index names it.
        let asked = RefCell::new(Vec::new());
        let url = "https://release.echovr.de/launcher/dev/abc/mods.json";
        let k = known(index(2, &[("mods.json", "dev")]));
        let addr = cdn_of("dev");
        let answers = [(addr.as_str(), Ok(Some("dev"))), (url, Ok(Some("dev")))];
        let t = text_with(url, "mods.json", &k, Some(CDN), &net(&answers, &asked));
        assert_eq!(t.unwrap().as_deref(), Some("dev"));
        assert_eq!(*asked.borrow(), [addr]);
    }

    #[test]
    fn a_cdn_that_cant_be_reached_is_told_from_one_that_answers() {
        let refused = crate::core::http::block_on(async {
            reqwest::Client::new()
                .get("http://127.0.0.1:1/x")
                .send()
                .await
                .unwrap_err()
        });
        assert!(refused.is_connect());
        let e = anyhow::Error::new(refused).context("GET x");
        assert!(e
            .chain()
            .find_map(|c| c.downcast_ref::<reqwest::Error>())
            .is_some_and(|r| r.is_connect()));
        // Requests elsewhere never pause the CDN.
        assert!(!is_cdn("http://127.0.0.1:1/x"));
        assert!(!is_cdn("https://release.echovr.de/store/sha256/ab/x"));
        assert!(
            is_cdn("https://cdn.echovr.de/store/sha256/ab/x") || std::env::var(CDN_ENV).is_ok()
        );
    }

    #[test]
    fn files_are_asked_for_by_checksum_and_never_the_live_ones() {
        let sha = "AB".repeat(32);
        let low = sha.to_lowercase();
        assert_eq!(
            by_checksum(
                "https://release.echovr.de/updates-nevr/BugSplat64.dll",
                &sha
            ),
            [
                format!("https://cdn.echovr.de/store/sha256/ab/{low}"),
                format!("https://release.echovr.de/store/sha256/ab/{low}"),
            ]
        );
        // An earlier version only the store has: its address is the fallback itself.
        let stored = origin_store_url(&sha).unwrap();
        assert_eq!(
            by_checksum(&stored, &sha),
            [format!("https://cdn.echovr.de/store/sha256/ab/{low}")]
        );
        // A dev folder's file too (the store holds those as well).
        assert_eq!(
            by_checksum(
                "https://release.echovr.de/launcher/dev/abc/files/C.zip",
                &sha
            )
            .len(),
            2
        );
        // Not a checksum: nothing by checksum.
        assert!(by_checksum("https://release.echovr.de/pc.zip", "abc").is_empty());
        for url in [
            "https://release.echovr.de/launcher/logs",
            "https://release.echovr.de/launcher/feed/servers.json",
            "https://release.echovr.de/quest/launcher/feed/servers.json",
            "https://release.echovr.de/stats/",
            "https://release.echovr.de/index.json",
            "https://release.echovr.de/pc.zip?x=1",
            "https://release.echovr.de/../etc/passwd",
            "https://release.echovr.de/",
            "https://files.echovr.de/updates/update.manifest",
            "https://evr.echo.taxi/pc.zip",
            "https://github.com/marshmallow-mia/EchoVR_Launcher/releases/download/v1/x.zip",
        ] {
            assert!(by_checksum(url, &sha).is_empty(), "{url}");
        }
    }

    /// Against release.echovr.de (needs network): what comes back matches its index, from
    /// the CDN or not. With `ECHOVR_CDN=http://127.0.0.1:8799` serving older copies, they are
    /// passed over (that server's log shows them asked for).
    /// The live index, on release.echovr.de and the CDN, is signed with a key the launcher
    /// knows (run by hand: `cargo test live_index_is_signed -- --ignored`).
    #[test]
    #[ignore]
    fn live_index_is_signed() {
        for base in [ORIGIN, CDN] {
            let get = |p: &str| {
                super::super::http::raw_text(&format!("{base}{p}"), INDEX_TIMEOUT)
                    .unwrap()
                    .unwrap_or_else(|| panic!("no {base}{p}"))
            };
            // The two are fetched one after the other: a new index may come in between.
            let ok = (0..3).any(|_| signed(&get(INDEX), &get(SIG), &SIGNING_KEYS));
            assert!(ok, "{base}{INDEX} isn't signed");
        }
    }

    #[test]
    #[ignore]
    fn live_files_match_the_index() {
        let Known::Index(i) = super::index("") else {
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
        let i = Index::parse(r#"{"schema":1,"serial":42,"checked":"2026-10-19T17:00:00+00:00","files":{"launcher/mods.json":"AB"}}"#)
            .unwrap();
        assert_eq!(i.serial, 42);
        let at = |t: &str| {
            OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339).unwrap()
        };
        assert!(!i.stale(at("2026-10-19T17:19:00+00:00")));
        assert!(i.stale(at("2026-10-19T17:21:00+00:00")));
        // The CDN's Date header.
        let date = OffsetDateTime::parse(
            "Mon, 19 Oct 2026 17:05:00 GMT",
            &time::format_description::well_known::Rfc2822,
        )
        .unwrap();
        assert!(!i.stale(date));
        assert_eq!(i.sha_of("launcher/mods.json"), Some("AB"));
        assert!(Index::parse("not json").is_none());
    }
}
