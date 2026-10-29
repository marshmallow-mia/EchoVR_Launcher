//! release.echovr.de's history of every part (`launcher/history.json`, written by
//! `echo-cdn-index`): every version each update manifest, mod and plugin had, oldest first,
//! all their files kept in its store by checksum ([`crate::core::sources`]). So a player
//! goes back to the version before even without a copy of their own (installed after the
//! update): [`super::goback`] takes a kept copy first, else this.
//!
//! An entry whose files aren't all in the store is `incomplete` and never offered.
//!
//! With a dev code set, the dev folder's own `history.json` (the same format, its URLs
//! relative to the folder) comes in too, for the mods and plugins that folder lists: a tester
//! goes back from one dev build to the one before it, and from the first to the published one
//! before that.
//!
//! Every mod entry taken from here is checked like a catalogue's ([`ModEntry`]'s rules: its
//! id, its file name, a download on a trusted host with a checksum) before anything installs
//! it; one that isn't so is never offered.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

use super::mods::ModEntry;
use super::plugins::PluginEntry;
use crate::core::manifest::{Action, Entry, Manifest};
use crate::core::{pc_update, sources};

pub const HISTORY_URL: &str = "https://release.echovr.de/launcher/history.json";
/// One read serves a refresh of the ways back and the going back that follows.
const KEEP_FOR: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct History {
    /// Path on release.echovr.de → every version of that update manifest.
    pub updates: BTreeMap<String, Vec<Published>>,
    /// Catalogue id → every version of that mod's (or pack's) entry.
    pub mods: BTreeMap<String, Vec<Value>>,
    /// Catalogue id → every version of that plugin's entry.
    pub plugins: BTreeMap<String, Vec<Value>>,
}

/// One version of an update manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Published {
    /// The manifest's sha256 (in the store); `None`: taken away (a channel without an
    /// overlay is exactly main).
    pub sha: Option<String>,
    /// When it was published (RFC 3339).
    pub at: String,
    /// What its first comment says it is ("nEVR runtime 5.2.1").
    pub title: String,
    pub incomplete: bool,
}

/// What every catalogue entry in the history has besides the catalogue's own fields.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Listed {
    incomplete: bool,
}

impl History {
    pub fn parse(text: &str) -> Result<History> {
        Ok(serde_json::from_str(text)?)
    }

    /// The published history, with the dev folders' of the dev codes in use (`None`: not
    /// there, or not readable now), kept a minute.
    pub fn load() -> Option<History> {
        type Kept = (Instant, Vec<String>, Option<History>);
        static CACHE: Mutex<Option<Kept>> = Mutex::new(None);
        let codes = super::dev_code::codes();
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, c, h)) = cache.as_ref() {
            if at.elapsed() < KEEP_FOR && *c == codes {
                return h.clone();
            }
        }
        let mut h = read(HISTORY_URL, "history.json");
        // What each dev folder lists: its history counts for those ids only.
        let listed = |name: &str, field: &str| -> BTreeMap<String, Vec<String>> {
            super::dev_code::kept(name)
                .into_iter()
                .map(|(code, text)| {
                    let ids = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v.get(field).and_then(Value::as_array).cloned())
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_string))
                        .collect();
                    (code, ids)
                })
                .collect()
        };
        let (dev_mods, dev_plugins) = (listed("mods", "mods"), listed("plugins", "plugins"));
        for code in &codes {
            let folder = super::dev_code::folder(code);
            if let Some(dev) = read(
                &format!("{folder}history.json"),
                "a dev folder's history.json",
            ) {
                let none = Vec::new();
                h.get_or_insert_with(History::default).merge_dev(
                    dev,
                    &folder,
                    dev_mods.get(code).unwrap_or(&none),
                    dev_plugins.get(code).unwrap_or(&none),
                );
            }
        }
        *cache = Some((Instant::now(), codes, h.clone()));
        h
    }

    /// Pure: a dev folder's history (at `folder`, with a trailing slash) into this one, for
    /// the mods and plugins its catalogues list (`mods`, `plugins`: ids): each id's versions
    /// from both, by when they came out. Its URLs, relative to the folder, are made whole
    /// (and so go to the store, [`from_store`]); an entry with one out of the folder is left
    /// out, as the dev catalogue's own reader does.
    pub fn merge_dev(&mut self, dev: History, folder: &str, mods: &[String], plugins: &[String]) {
        for (mine, theirs, ids) in [
            (&mut self.mods, dev.mods, mods),
            (&mut self.plugins, dev.plugins, plugins),
        ] {
            for (id, entries) in theirs {
                if !ids.contains(&id) {
                    tracing::info!("a dev folder's history has {id}, which its catalogue doesn't list: left out");
                    continue;
                }
                let list = mine.entry(id).or_default();
                list.extend(entries.into_iter().filter_map(|mut e| {
                    let pack_ok = e.get_mut("pack").is_none_or(|p| whole_url(p, folder));
                    (whole_url(&mut e, folder) && pack_ok).then_some(e)
                }));
                let at = |e: &Value| {
                    e.get("at")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                list.sort_by_key(at);
            }
        }
    }

    /// The version listed before `version` of mod (or pack) `id` that `ok` takes (not
    /// blocked, no prerelease unless asked for) and that is a sound entry of that mod, ready
    /// to install from the store.
    pub fn mod_before(
        &self,
        id: &str,
        version: &str,
        ok: impl Fn(&ModEntry) -> bool,
    ) -> Option<ModEntry> {
        let fits = |e: &ModEntry| e.id == id && e.validate().is_ok() && ok(e);
        let e: ModEntry = before(self.mods.get(id)?, version, |e: &ModEntry| &e.version, fits)?;
        Some(from_store_entry(e))
    }

    /// The newest complete version of mod `id` older than `version` that `ok` takes (not
    /// blocked, no prerelease unless asked for) and that is a sound entry of that mod, ready
    /// to install from the store. Versions that aren't semantic versions aren't considered.
    pub fn mod_newest_below(
        &self,
        id: &str,
        version: &str,
        ok: impl Fn(&ModEntry) -> bool,
    ) -> Option<ModEntry> {
        use crate::core::semver::Version;
        let below = Version::parse(version)?;
        let e: ModEntry = self
            .mods
            .get(id)?
            .iter()
            .filter(|v| {
                !serde_json::from_value::<Listed>((*v).clone())
                    .unwrap_or_default()
                    .incomplete
            })
            .filter_map(|v| serde_json::from_value::<ModEntry>(v.clone()).ok())
            .filter(|e| e.id == id && e.validate().is_ok())
            .filter_map(|e| Some((Version::parse(&e.version)?, e)))
            .filter(|(v, e)| *v < below && ok(e))
            .max_by(|a, b| a.0.cmp(&b.0))?
            .1;
        Some(from_store_entry(e))
    }

    /// The version listed before `version` of plugin `id`, ready to install from the store.
    pub fn plugin_before(&self, id: &str, version: &str) -> Option<PluginEntry> {
        let mut e: PluginEntry = before(
            self.plugins.get(id)?,
            version,
            |e: &PluginEntry| &e.version,
            |_| true,
        )?;
        e.url = from_store(&e.url, &e.sha256);
        Some(e)
    }
}

/// A history entry's downloads at their store addresses.
fn from_store_entry(mut e: ModEntry) -> ModEntry {
    if let Some(sha) = e.sha256.clone() {
        e.url = from_store(&e.url, &sha);
    }
    if let Some(p) = e.pack.as_mut() {
        p.url = from_store(&p.url, &p.sha256);
    }
    e
}

/// Pure: the last complete entry `keep` takes before the last one with `version`, with
/// another version. For a version the history doesn't know (a dev folder's build): the newest
/// complete one older than it, never a newer one.
fn before<T: DeserializeOwned + Clone>(
    list: &[Value],
    version: &str,
    version_of: impl Fn(&T) -> &String,
    keep: impl Fn(&T) -> bool,
) -> Option<T> {
    let parsed: Vec<(T, bool)> = list
        .iter()
        .filter_map(|v| {
            let e = serde_json::from_value::<T>(v.clone()).ok()?;
            let l = serde_json::from_value::<Listed>(v.clone()).unwrap_or_default();
            Some((e, l.incomplete))
        })
        .collect();
    let Some(now) = parsed.iter().rposition(|(e, _)| version_of(e) == version) else {
        let complete = || {
            parsed
                .iter()
                .filter(|(e, incomplete)| !incomplete && keep(e))
        };
        let older = super::update_check::newest_below(
            complete().map(|(e, _)| version_of(e).as_str()),
            version,
        )?;
        return complete()
            .rev()
            .find(|(e, _)| *version_of(e) == older)
            .map(|(e, _)| e.clone());
    };
    parsed[..now]
        .iter()
        .rev()
        .find(|(e, incomplete)| !incomplete && version_of(e) != version && keep(e))
        .map(|(e, _)| e.clone())
}

/// Fetches and reads a history (`None`: not there, or not readable now; `what` for the log).
fn read(url: &str, what: &str) -> Option<History> {
    match crate::core::http::get_text_opt(url) {
        Ok(Some(t)) => History::parse(&t)
            .map_err(|e| tracing::warn!("{what} unreadable: {e:#}"))
            .ok(),
        Ok(None) => None,
        Err(e) => {
            let e = super::dev_code::redact(&format!("{e:#}"));
            tracing::info!("{what} unavailable: {e}");
            None
        }
    }
}

/// Pure: an entry's `url` relative to a dev folder (`files/x.zip`) as a whole one in it;
/// whether it is in the folder (an entry without one is too).
fn whole_url(e: &mut Value, folder: &str) -> bool {
    let Some(url) = e.get("url").and_then(Value::as_str) else {
        return true;
    };
    match super::dev_code::resolve_in(folder, url) {
        Some(whole) => {
            e["url"] = Value::String(whole);
            true
        }
        None => false,
    }
}

/// Pure: `url` (on release.echovr.de) as its store address, which keeps an earlier version
/// after the file at `url` was replaced; a download elsewhere stays as it is.
fn from_store(url: &str, sha: &str) -> String {
    match sources::origin_store_url(sha) {
        Some(s) if url.starts_with(sources::ORIGIN) => s,
        _ => url.to_string(),
    }
}

// ---- the game update ----

/// One state of an update with its channel's overlays: what [`pc_update::fetch_manifest`]
/// gave at that time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub at: String,
    /// What it is: the overlay's title when one with lines of its own was on, else main's.
    pub title: String,
    /// The manifest's sha256 and its overlays' (`None`: none then), in order.
    pub main: String,
    pub overlays: Vec<(String, Option<String>)>,
}

/// Pure: the states the update at `url` with `overlays` (URLs on release.echovr.de) went
/// through, oldest first, from `h`. A state with an incomplete part is left out.
pub fn timeline(h: &History, url: &str, overlays: &[&str]) -> Vec<State> {
    let key = |u: &str| u.strip_prefix(sources::ORIGIN).unwrap_or(u).to_string();
    let empty = Vec::new();
    let main = h.updates.get(&key(url)).unwrap_or(&empty);
    let layers: Vec<&Vec<Published>> = overlays
        .iter()
        .map(|o| h.updates.get(&key(o)).unwrap_or(&empty))
        .collect();
    let mut times: Vec<&str> = main
        .iter()
        .chain(layers.iter().flat_map(|l| l.iter()))
        .map(|p| p.at.as_str())
        .collect();
    times.sort_unstable();
    times.dedup();
    let at = |list: &'_ [Published], t: &str| -> Option<Published> {
        list.iter().rev().find(|p| p.at.as_str() <= t).cloned()
    };
    let mut out: Vec<State> = Vec::new();
    for t in times {
        let Some(m) = at(main, t).filter(|m| m.sha.is_some() && !m.incomplete) else {
            continue;
        };
        let parts: Vec<Option<Published>> = layers.iter().map(|l| at(l, t)).collect();
        if parts.iter().flatten().any(|p| p.incomplete) {
            continue;
        }
        let title = parts
            .iter()
            .flatten()
            .rev()
            .find(|p| p.sha.is_some() && p.title.starts_with("nEVR runtime "))
            .map(|p| p.title.clone())
            .unwrap_or(m.title.clone());
        let state = State {
            at: t.to_string(),
            title,
            main: m.sha.clone().unwrap_or_default(),
            overlays: overlays
                .iter()
                .zip(parts)
                .map(|(o, p)| (o.to_string(), p.and_then(|p| p.sha)))
                .collect(),
        };
        if out.last().is_none_or(|l| {
            (l.main.as_str(), &l.overlays) != (state.main.as_str(), &state.overlays)
        }) {
            out.push(state);
        }
    }
    out
}

/// A state's manifest and text, its manifests read from the store by checksum (`texts`).
pub fn manifest_of(
    url: &str,
    s: &State,
    texts: &dyn Fn(&str) -> Result<Option<String>>,
) -> Result<(Manifest, String)> {
    let text = |sha: &str| {
        texts(sha)?.ok_or_else(|| anyhow!("release.echovr.de no longer has the update {sha}"))
    };
    let main = text(&s.main)?;
    let mut over = Vec::new();
    for (o, sha) in &s.overlays {
        if let Some(sha) = sha {
            over.push((o.as_str(), text(sha)?));
        }
    }
    pc_update::with_overlays(url, &main, &over)
}

/// Pure: a manifest's effect, path (lowercase) → the checksum it puts there (`None`: it
/// deletes the file).
pub fn effect(m: &Manifest) -> BTreeMap<String, Option<String>> {
    m.entries
        .iter()
        .map(|e| (e.path.to_lowercase(), e.sha256.clone()))
        .collect()
}

/// Pure: which state is installed and which to go back to, given each state's effect,
/// whether its files are what is on disk, and its text's sha256. The installed one is the
/// latest whose files are on disk, else the one `recorded` names, else the newest; the
/// way back is the latest before it that changes something.
pub fn step_back(
    effects: &[BTreeMap<String, Option<String>>],
    on_disk: &[bool],
    shas: &[String],
    recorded: Option<&str>,
) -> Option<(usize, usize)> {
    let now = on_disk
        .iter()
        .rposition(|d| *d)
        .or_else(|| recorded.and_then(|r| shas.iter().rposition(|s| s.eq_ignore_ascii_case(r))))
        .or_else(|| effects.len().checked_sub(1))?;
    let back = (0..now).rev().find(|&i| effects[i] != effects[now])?;
    Some((now, back))
}

/// Whether every file `m` adds is on disk with its checksum; `disk` gives a path's checksum
/// on disk (each file is read once for all states).
pub fn on_disk(m: &Manifest, disk: &mut dyn FnMut(&str) -> Option<String>) -> bool {
    m.adds().all(|e| {
        e.sha256
            .as_deref()
            .is_some_and(|sha| disk(&e.path).is_some_and(|d| d.eq_ignore_ascii_case(sha)))
    })
}

/// An update manifest by its checksum, from the store (they never change: kept while the
/// launcher runs).
pub fn text(sha: &str) -> Result<Option<String>> {
    static CACHE: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    if let Some(t) = CACHE.lock().unwrap_or_else(|p| p.into_inner()).get(sha) {
        return Ok(Some(t.clone()));
    }
    let t = sources::stored_text(sha)?;
    if let Some(t) = &t {
        CACHE
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(sha.to_string(), t.clone());
    }
    Ok(t)
}

/// Pure: `back` with a `del` for every file `now` adds that `back` doesn't have (so going
/// back takes away what the newer update brought).
pub fn going_back(now: &Manifest, mut back: Manifest) -> Manifest {
    let has = |p: &str| back.entries.iter().any(|e| e.path.eq_ignore_ascii_case(p));
    let extra: Vec<Entry> = now
        .adds()
        .filter(|e| !has(&e.path))
        .map(|e| Entry {
            action: Action::Del,
            path: e.path.clone(),
            sha256: None,
            base: None,
        })
        .collect();
    back.entries.extend(extra);
    back
}

/// What a state is called: "nEVR 5.2.1", else the update of its day.
pub fn label(s: &State) -> String {
    if let Some(v) = s.title.strip_prefix("nEVR runtime ") {
        return format!("nEVR {}", v.split(['+', '-']).next().unwrap_or(v));
    }
    format!("the update of {}", s.at.get(..10).unwrap_or(&s.at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(sha: Option<&str>, at: &str, title: &str) -> Published {
        Published {
            sha: sha.map(str::to_string),
            at: at.into(),
            title: title.into(),
            incomplete: false,
        }
    }

    const MAIN: &str = "https://release.echovr.de/updates-nevr/update.manifest";
    const BETA: &str = "https://release.echovr.de/updates-nevr-beta/update.manifest";

    fn history() -> History {
        let mut h = History::default();
        h.updates.insert(
            "updates-nevr/update.manifest".into(),
            vec![
                p(
                    Some("m401"),
                    "2026-10-06T14:25:48+00:00",
                    "nEVR runtime 4.0.1",
                ),
                p(
                    Some("m410"),
                    "2026-10-11T17:45:44+00:00",
                    "nEVR runtime 4.1.0",
                ),
                p(
                    Some("m521"),
                    "2026-10-19T08:44:00+00:00",
                    "nEVR runtime 5.2.1",
                ),
            ],
        );
        h.updates.insert(
            "updates-nevr-beta/update.manifest".into(),
            vec![
                p(
                    Some("b401"),
                    "2026-10-10T10:17:26+00:00",
                    "nEVR runtime 4.0.1+3.9b56520",
                ),
                p(
                    Some("bnone"),
                    "2026-10-11T17:45:44+00:00",
                    "empty, the beta is main now (nEVR 4.1.0).",
                ),
                p(
                    Some("b521"),
                    "2026-10-17T20:09:49+00:00",
                    "nEVR runtime 5.2.1",
                ),
                p(
                    Some("bnone2"),
                    "2026-10-19T08:44:30+00:00",
                    "nothing of its own",
                ),
            ],
        );
        h
    }

    #[test]
    fn main_goes_through_its_own_versions() {
        let t = timeline(&history(), MAIN, &[]);
        let labels: Vec<_> = t.iter().map(label).collect();
        assert_eq!(labels, ["nEVR 4.0.1", "nEVR 4.1.0", "nEVR 5.2.1"]);
        assert!(t.iter().all(|s| s.overlays.is_empty()));
    }

    #[test]
    fn beta_goes_through_main_and_its_overlay_together() {
        let t = timeline(&history(), MAIN, &[BETA]);
        let got: Vec<_> = t
            .iter()
            .map(|s| (s.main.as_str(), s.overlays[0].1.as_deref().unwrap_or("-")))
            .collect();
        assert_eq!(
            got,
            [
                // Before the history has an overlay: main alone.
                ("m401", "-"),
                ("m401", "b401"),
                ("m410", "bnone"),
                ("m410", "b521"),
                ("m521", "b521"),
                ("m521", "bnone2"),
            ]
        );
        assert_eq!(label(&t[3]), "nEVR 5.2.1");
        // An overlay without a runtime of its own: main's.
        assert_eq!(label(&t[2]), "nEVR 4.1.0");
        assert_eq!(label(&t[1]), "nEVR 4.0.1");
    }

    #[test]
    fn incomplete_or_taken_away_parts_are_never_a_state() {
        let mut h = history();
        h.updates.get_mut("updates-nevr/update.manifest").unwrap()[1].incomplete = true;
        let t = timeline(&h, MAIN, &[]);
        assert_eq!(
            t.iter().map(|s| s.main.as_str()).collect::<Vec<_>>(),
            ["m401", "m521"]
        );
        // An overlay taken away is no overlay.
        h.updates
            .get_mut("updates-nevr-beta/update.manifest")
            .unwrap()
            .push(p(None, "2026-10-20T00:00:00+00:00", "none"));
        let t = timeline(&h, MAIN, &[BETA]);
        assert_eq!(t.last().unwrap().overlays[0].1, None);
    }

    fn eff(pairs: &[(&str, &str)]) -> BTreeMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(p, s)| (p.to_string(), Some(s.to_string())))
            .collect()
    }

    #[test]
    fn going_back_steps_to_the_last_state_that_changes_something() {
        let effects = [
            eff(&[("bugsplat64.dll", "4")]),
            eff(&[("bugsplat64.dll", "5")]),
            eff(&[("bugsplat64.dll", "5")]), // the same files, another text (a comment)
        ];
        let shas = ["a".to_string(), "b".into(), "c".into()];
        // The newest on disk: back to 4.
        assert_eq!(
            step_back(&effects, &[false, true, true], &shas, None),
            Some((2, 0))
        );
        // 4 on disk (gone back before): nothing further back.
        assert_eq!(
            step_back(&effects, &[true, false, false], &shas, None),
            None
        );
        // Files changed by hand: the recorded state, else the newest.
        assert_eq!(
            step_back(&effects, &[false; 3], &shas, Some("B")),
            Some((1, 0))
        );
        assert_eq!(
            step_back(&effects, &[false; 3], &shas, Some("went-back")),
            Some((2, 0))
        );
        assert_eq!(step_back(&[], &[], &[], None), None);
    }

    #[test]
    fn going_back_takes_away_what_the_newer_update_added() {
        let now = Manifest::parse(
            &format!(
                "add BugSplat64.dll {}\nadd plugins/New.dll {}\n",
                "5".repeat(64),
                "6".repeat(64)
            ),
            MAIN,
        )
        .unwrap();
        let back =
            Manifest::parse(&format!("add bugsplat64.dll {}\n", "4".repeat(64)), MAIN).unwrap();
        let m = going_back(&now, back);
        let dels: Vec<_> = m.dels().map(|e| e.path.as_str()).collect();
        assert_eq!(dels, ["plugins/New.dll"]);
        assert_eq!(m.adds().count(), 1);
    }

    /// Against release.echovr.de (needs network): main's nEVR versions, every manifest read
    /// from the store by its checksum.
    #[test]
    #[ignore]
    fn live_the_history_has_main_s_versions() {
        let h = History::load().expect("history.json on release.echovr.de");
        let t = timeline(&h, pc_update::PC_MANIFEST_URL, &[]);
        let labels: Vec<_> = t.iter().map(label).collect();
        println!("{labels:?}");
        assert!(labels.ends_with(&["nEVR 4.1.0".to_string(), "nEVR 5.2.1".to_string()]));
        for s in &t {
            let (m, text) = manifest_of(pc_update::PC_MANIFEST_URL, s, &text).unwrap();
            assert_eq!(pc_update::sha256_hex(text.as_bytes()), s.main);
            assert!(m.adds().count() > 0);
        }
        let t = timeline(&h, pc_update::PC_MANIFEST_URL, &[pc_update::BETA_OVERLAY]);
        println!("beta: {:?}", t.iter().map(label).collect::<Vec<_>>());
    }

    /// Against release.echovr.de (needs network): a plugin installed at its newest version
    /// goes back to the one before from the history, in a folder of its own.
    #[test]
    #[ignore]
    fn live_a_plugin_goes_back_from_the_history() {
        use crate::core::launcher::plugins;
        let h = History::load().expect("history.json on release.echovr.de");
        let list = &h.plugins["event-lobbies"];
        let newest: PluginEntry = serde_json::from_value(list.last().unwrap().clone()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        plugins::install_into(root.path(), &newest, &cancel, &mut |_| {}).unwrap();
        let back = h.plugin_before("event-lobbies", &newest.version).unwrap();
        assert!(back.url.contains("/store/sha256/"), "{}", back.url);
        let from = plugins::go_back_to(root.path(), &back, &cancel).unwrap();
        assert_eq!(from, newest.version);
        let now = plugins::installed_in(root.path());
        assert_eq!(now[0].from_catalog.as_deref(), Some(back.version.as_str()));
        // No copy of the newer one kept: "Use newest" downloads it as an update.
        assert!(plugins::previous_in(root.path()).is_empty());
    }

    #[test]
    fn the_version_before_comes_from_the_store() {
        let sha = |c: &str| c.repeat(64);
        let h = History::parse(&format!(
            r#"{{"schema":1,
              "mods":{{"gpu":[
                {{"id":"gpu","version":"1.0.0","file":"G.dll","url":"https://release.echovr.de/launcher/mods/G-1.0.0.dll","sha256":"{a}","at":"1"}},
                {{"id":"gpu","version":"1.1.0","file":"G.dll","url":"https://release.echovr.de/launcher/mods/G-1.1.0.dll","sha256":"{b}","at":"2","incomplete":true}},
                {{"id":"gpu","version":"1.2.0","file":"G.dll","url":"https://release.echovr.de/launcher/mods/G-1.2.0.dll","sha256":"{c}","at":"3"}}]}},
              "plugins":{{"lobbies":[
                {{"id":"lobbies","version":"0.1.1","url":"https://github.com/x/y.zip","sha256":"{a}","at":"1"}},
                {{"id":"lobbies","version":"0.1.2","url":"https://release.echovr.de/launcher/plugins/l.zip","sha256":"{b}","at":"2"}}]}}}}"#,
            a = sha("a"),
            b = sha("b"),
            c = sha("c"),
        ))
        .unwrap();
        // 1.1.0 is incomplete: 1.2.0 goes back to 1.0.0, from the store.
        let m = h.mod_before("gpu", "1.2.0", |_| true).unwrap();
        assert_eq!(m.version, "1.0.0");
        assert_eq!(
            m.url,
            format!("https://release.echovr.de/store/sha256/aa/{}", sha("a"))
        );
        assert!(h.mod_before("gpu", "1.0.0", |_| true).is_none());
        // A version the history doesn't know (a dev folder's build): the newest complete
        // one older than it, never a newer one.
        assert_eq!(
            h.mod_before("gpu", "1.2.1", |_| true).unwrap().version,
            "1.2.0"
        );
        assert_eq!(
            h.mod_before("gpu", "1.1.5", |_| true).unwrap().version,
            "1.0.0"
        );
        assert!(h.mod_before("gpu", "0.9.0", |_| true).is_none());
        // A download elsewhere stays where it is.
        let p = h.plugin_before("lobbies", "0.1.2").unwrap();
        assert_eq!(
            (p.version.as_str(), p.url.as_str()),
            ("0.1.1", "https://github.com/x/y.zip")
        );
    }

    #[test]
    fn a_dev_folder_goes_back_through_its_own_builds_then_the_published_ones() {
        let sha = |c: &str| c.repeat(64);
        let mut h = History::parse(&format!(
            r#"{{"mods":{{"gpu":[
                {{"id":"gpu","version":"1.0.0","file":"G.dll","url":"https://release.echovr.de/launcher/mods/G-1.0.0.dll","sha256":"{a}","at":"2026-10-07T19:09:00+00:00"}}]}}}}"#,
            a = sha("a"),
        ))
        .unwrap();
        let dev = History::parse(&format!(
            r#"{{"mods":{{
                "gpu":[{{"id":"gpu","version":"1.0.1-test","file":"G.dll","url":"files/G-1.0.1-test.dll","sha256":"{b}","at":"2026-10-19T12:00:00+00:00"}}],
                "combat":[
                  {{"id":"combat","version":"1.1.7","file":"C.dll","url":"files/C-1.1.7.dll","sha256":"{c}","at":"2026-10-18T10:00:00+00:00"}},
                  {{"id":"combat","version":"1.1.8","file":"C.dll","url":"files/C-1.1.8.dll","sha256":"{d}","at":"2026-10-19T10:00:00+00:00"}},
                  {{"id":"combat","version":"1.1.9","file":"C.dll","url":"../../evil.dll","sha256":"{e}","at":"2026-10-19T11:00:00+00:00"}}]}},
              "plugins":{{"server":[{{"id":"server","version":"0.2.1","url":"files/s.zip","sha256":"{a}","at":"2026-10-19T10:00:00+00:00"}}]}}}}"#,
            b = sha("b"),
            c = sha("c"),
            d = sha("d"),
            e = sha("e"),
            a = sha("a"),
        ))
        .unwrap();
        let ids = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        h.merge_dev(
            dev.clone(),
            "https://release.echovr.de/launcher/dev/abc/",
            &ids(&["gpu", "combat"]),
            &ids(&["server"]),
        );
        // A dev build before the next one, from the store (the folder may not have it any more).
        let m = h.mod_before("combat", "1.1.8", |_| true).unwrap();
        assert_eq!(m.version, "1.1.7");
        assert_eq!(
            m.url,
            format!("https://release.echovr.de/store/sha256/cc/{}", sha("c"))
        );
        // The first dev build: back to the published version.
        let m = h.mod_before("gpu", "1.0.1-test", |_| true).unwrap();
        assert_eq!(m.version, "1.0.0");
        assert_eq!(h.plugins["server"].len(), 1);
        // An entry with a URL out of the folder is left out.
        assert_eq!(h.mods["combat"].len(), 2);
        // A dev history counts only for what its folder lists: a published mod's id it doesn't
        // list stays as published.
        let mut only_combat = History::default();
        only_combat.merge_dev(
            dev,
            "https://release.echovr.de/launcher/dev/abc/",
            &ids(&["combat"]),
            &[],
        );
        assert!(!only_combat.mods.contains_key("gpu"));
        assert!(only_combat.plugins.is_empty());
    }

    #[test]
    fn going_back_takes_only_sound_wanted_entries() {
        let sha = |c: &str| c.repeat(64);
        let h = History::parse(&format!(
            r#"{{"mods":{{"cool":[
                {{"id":"cool","version":"1.0.0","file":"Cool.dll","url":"https://release.echovr.de/launcher/mods/Cool-1.0.0.dll","sha256":"{a}","at":"1"}},
                {{"id":"cool","version":"1.1.0-beta.1","file":"Cool.dll","url":"https://release.echovr.de/launcher/mods/Cool-1.1.0-beta.1.dll","sha256":"{b}","at":"2"}},
                {{"id":"cool","version":"1.1.0","file":"../BugSplat64.dll","url":"https://release.echovr.de/launcher/mods/x.dll","sha256":"{c}","at":"3"}},
                {{"id":"cool","version":"1.1.1","file":"Cool.dll","url":"https://evil.example/Cool.dll","sha256":"{c}","at":"4"}},
                {{"id":"other","version":"1.1.2","file":"Cool.dll","url":"https://release.echovr.de/launcher/mods/o.dll","sha256":"{c}","at":"5"}},
                {{"id":"cool","version":"1.2.0","file":"Cool.dll","url":"https://release.echovr.de/launcher/mods/Cool-1.2.0.dll","sha256":"{d}","at":"6"}}]}}}}"#,
            a = sha("a"),
            b = sha("b"),
            c = sha("c"),
            d = sha("d"),
        ))
        .unwrap();
        // A file name out of plugins/, a download elsewhere, another mod's entry: never.
        assert_eq!(
            h.mod_before("cool", "1.2.0", |_| true).unwrap().version,
            "1.1.0-beta.1"
        );
        let no_pre = |e: &ModEntry| !e.version.contains('-');
        assert_eq!(
            h.mod_before("cool", "1.2.0", no_pre).unwrap().version,
            "1.0.0"
        );
        assert_eq!(
            h.mod_newest_below("cool", "1.2.0", no_pre).unwrap().version,
            "1.0.0"
        );
        assert_eq!(
            h.mod_newest_below("cool", "1.2.0", |_| true)
                .unwrap()
                .version,
            "1.1.0-beta.1"
        );
        assert!(h
            .mod_newest_below("cool", "1.2.0", |e| e.version != "1.0.0" && no_pre(e))
            .is_none());
    }
}
