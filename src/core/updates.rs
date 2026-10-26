//! What can be updated, and telling the player once: the launcher (its latest GitHub
//! release), each installed Echo VR that gets updates (its update manifest changed since
//! the launcher last brought it up to date), and the plugins installed from the mods
//! catalogue (the catalogue has another version), and launcher plugins: an installed one's
//! update, and a plugin the catalogues list that wasn't listed before (a dev folder's too,
//! so testers don't miss it). Nothing is installed here: the launcher installs on a click. EchoXR and EchoXR Hands come with the launcher (their versions
//! are pinned in it), so a launcher update brings theirs.
//!
//! The launcher checks at its start and every 15 minutes, and so does the tray
//! (`ui::tray`) while the launcher is closed; in between, the open launcher reads the mods
//! and plugins catalogues every 30 seconds ([`check_catalogs`]). Each finding is announced
//! on the desktop once: the ones announced are kept in `updates-notified.json`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::launcher::goback::{self, Part};
use super::launcher::mods::{self, ModCatalog, Source};
use super::launcher::plugins::{self as launcher_plugins, Installed, PluginCatalog};
use super::launcher::store::{InstalledVersion, LauncherState};
use super::launcher::{update_check, versions};
use super::paths;

/// How often the launcher and the tray look.
pub const INTERVAL: Duration = Duration::from_secs(15 * 60);
/// How often the open launcher reads the mods and plugins catalogues (a dev folder's too)
/// in between: what they add or update shows within half a minute.
pub const CATALOG_INTERVAL: Duration = Duration::from_secs(30);

/// Something that can be updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A newer launcher release.
    Launcher { version: String, url: String },
    /// An installed Echo VR's update manifest changed (`hash`: the new one's).
    Game {
        id: String,
        name: String,
        hash: String,
    },
    /// A catalogue plugin in an installed version has another version in the catalogue.
    Plugin {
        version_id: String,
        version_name: String,
        file: String,
        name: String,
        to: String,
    },
    /// An installed launcher plugin has another version in the catalogue.
    LauncherPlugin {
        id: String,
        name: String,
        to: String,
    },
    /// A launcher plugin the catalogues didn't list before (`dev`: from the dev folder).
    NewPlugin { id: String, name: String, dev: bool },
}

impl Finding {
    /// What it is, for "announced once": the same update is the same key.
    pub fn key(&self) -> String {
        match self {
            Finding::Launcher { version, .. } => format!("launcher:{version}"),
            Finding::Game { id, hash, .. } => format!("game:{id}:{hash}"),
            Finding::Plugin {
                version_id,
                file,
                to,
                ..
            } => format!("plugin:{version_id}:{}:{to}", file.to_ascii_lowercase()),
            Finding::LauncherPlugin { id, to, .. } => format!("launcher-plugin:{id}:{to}"),
            Finding::NewPlugin { id, .. } => format!("new-plugin:{id}"),
        }
    }

    /// Whether the catalogues tell it (a plugin's update, a new plugin), rather than the
    /// launcher's release or a game's manifest.
    pub fn is_catalogs(&self) -> bool {
        catalog_key(&self.key())
    }

    /// One line for a notice or a notification.
    pub fn text(&self) -> String {
        match self {
            Finding::Launcher { version, .. } => {
                format!("Echo VR Launcher {version} is available")
            }
            Finding::Game { name, .. } => format!("An update for {name} is available"),
            Finding::Plugin {
                version_name,
                name,
                to,
                ..
            } => format!("{name} {to} is available (in {version_name})"),
            Finding::LauncherPlugin { name, to, .. } => {
                format!("The {name} plugin {to} is available")
            }
            Finding::NewPlugin { name, dev, .. } => format!(
                "New {}plugin: {name}, on the Plugins page",
                if *dev { "dev " } else { "" }
            ),
        }
    }
}

/// Pure: whether `key` ([`Finding::key`]) is a catalogue finding's.
fn catalog_key(key: &str) -> bool {
    ["plugin:", "launcher-plugin:", "new-plugin:"]
        .iter()
        .any(|k| key.starts_with(k))
}

/// What a check found, and what it learned on the way.
#[derive(Debug, Clone, Default)]
pub struct Check {
    pub findings: Vec<Finding>,
    /// Only the catalogues were read ([`check_catalogs`]): the launcher's and the games'
    /// findings are the last full check's.
    pub catalogs_only: bool,
    /// Versions without a recorded manifest yet: the manifest they start from (version id,
    /// hash). Recorded silently: they were brought up to date just before (or before this
    /// launcher kept track).
    pub baselines: Vec<(String, String)>,
    /// The catalogue fetched, for the Mods page's Update buttons.
    pub catalog: Option<ModCatalog>,
    /// The launcher plugins catalogue fetched (with the dev folder's), for the Plugins page.
    pub plugins: Option<PluginCatalog>,
    /// The ids it lists, kept as seen ([`apply_baselines`]).
    pub plugins_listed: Vec<String>,
    /// The launcher channel its launcher finding is from.
    pub launcher_channel: update_check::Channel,
    /// Versions this launcher has that release.echovr.de pulled (full checks only).
    pub pulled: Vec<goback::Affected>,
    /// The alerts for this launcher ([`crate::core::launcher::alerts`]); `None`: not read.
    pub alerts: Option<Vec<crate::core::launcher::alerts::Alert>>,
}

/// Debug builds: `ECHOVR_FAKE_UPDATE=1` makes every Echo VR that gets updates look
/// outdated (to try the notices and notifications).
fn fake_game_update() -> bool {
    cfg!(debug_assertions) && std::env::var_os("ECHOVR_FAKE_UPDATE").is_some()
}

/// Looks for updates (network; seconds). Failures leave a kind out quietly.
pub fn check(state: &LauncherState) -> Check {
    let mut out = Check {
        launcher_channel: state.launcher_channel,
        ..Check::default()
    };
    match update_check::newer(state.launcher_channel) {
        // The way back from a beta isn't an update to announce: Advanced settings has it.
        Ok(Some(r)) if r.back => {}
        Ok(Some(r)) => out.findings.push(Finding::Launcher {
            version: r.version,
            url: r.url,
        }),
        Ok(None) => {}
        Err(e) => tracing::info!("updates: launcher check failed: {e:#}"),
    }
    for v in state.versions.iter().filter(|v| v.present()) {
        let Some(url) = versions::manifest_url(v) else {
            continue;
        };
        // The live build's: main's with the channel's overlays (pc_update::fetch_manifest).
        match crate::core::pc_update::fetch_manifest(&url) {
            Ok((_, text)) => {
                let hash = sha256_hex(text.as_bytes());
                match game_state(v.manifest_sha256.as_deref(), &hash, fake_game_update()) {
                    GameState::Baseline => out.baselines.push((v.id.clone(), hash)),
                    GameState::Outdated => out.findings.push(Finding::Game {
                        id: v.id.clone(),
                        name: v.name.clone(),
                        hash,
                    }),
                    GameState::Current => {}
                }
            }
            Err(e) => tracing::info!("updates: {}'s manifest: {e:#}", v.id),
        }
    }
    catalogs(state, &mut out);
    out.findings.retain(|f| !held(state, f));
    out.pulled = pulled(state);
    out
}

/// The versions this launcher has that release.echovr.de pulled, with their ways back.
fn pulled(state: &LauncherState) -> Vec<goback::Affected> {
    let rollback = goback::Rollback::load();
    if rollback.pulled.is_empty() {
        return Vec::new();
    }
    let ways = goback::ways_back(state);
    let version = |id: &str| state.versions.iter().find(|v| v.id == id);
    goback::affected(
        &rollback,
        &ways,
        // The update it has: the one its kept copy came from, else the one last seen.
        &|id| {
            let v = version(id)?;
            versions::previous_update(v)
                .map(|p| p.manifest_sha256)
                .or_else(|| v.manifest_sha256.clone())
        },
        &|id, file| {
            mods::read(version(id)?)
                .plugins
                .into_iter()
                .find(|p| p.file.eq_ignore_ascii_case(file))
                .and_then(|p| match p.source {
                    Source::Catalog { id, .. } => Some(id),
                    _ => None,
                })
        },
    )
}

/// Whether `f` is the version the player went back from ([`goback`]): not offered again.
fn held(state: &LauncherState, f: &Finding) -> bool {
    let (part, found) = match f {
        Finding::Launcher { version, .. } => (Part::Launcher, version),
        Finding::Game { id, hash, .. } => (
            Part::Game {
                version: id.clone(),
            },
            hash,
        ),
        Finding::Plugin {
            version_id,
            file,
            to,
            ..
        } => (
            Part::Mod {
                version: version_id.clone(),
                file: file.clone(),
            },
            to,
        ),
        Finding::LauncherPlugin { id, to, .. } => (Part::Plugin { id: id.clone() }, to),
        Finding::NewPlugin { .. } => return false,
    };
    goback::held(&state.holds, &part, found)
}

/// Reads only the mods and plugins catalogues (network; a second or two): what the open
/// launcher looks at every [`CATALOG_INTERVAL`] between full checks.
pub fn check_catalogs(state: &LauncherState) -> Check {
    let mut out = Check {
        launcher_channel: state.launcher_channel,
        catalogs_only: true,
        ..Check::default()
    };
    catalogs(state, &mut out);
    out.findings.retain(|f| !held(state, f));
    out
}

/// The catalogues' part of a check: their plugin updates and the plugins they list, and
/// the alerts.
fn catalogs(state: &LauncherState, out: &mut Check) {
    out.alerts = Some(crate::core::launcher::alerts::fetch(state));
    let catalog = ModCatalog::load();
    if !catalog.builtin {
        for v in state.versions.iter().filter(|v| v.present()) {
            out.findings
                .extend(plugin_findings(v, &mods::read(v).plugins, &catalog));
        }
        out.catalog = Some(catalog);
    }
    let plugins = PluginCatalog::load();
    if !plugins.builtin {
        out.findings.extend(launcher_plugin_findings(
            &launcher_plugins::installed(),
            &plugins,
            state.plugins_seen.as_deref(),
        ));
        out.plugins_listed = plugins.plugins.iter().map(|p| p.id.clone()).collect();
        out.plugins = Some(plugins);
    }
}

/// Pure: updates of the launcher plugins `installed` in `catalog`, and the plugins it lists
/// that aren't installed and aren't in `seen` (none before anything was seen: the first
/// check only records what there is).
fn launcher_plugin_findings(
    installed: &[Installed],
    catalog: &PluginCatalog,
    seen: Option<&[String]>,
) -> Vec<Finding> {
    let mut out: Vec<Finding> = installed
        .iter()
        .filter_map(|p| {
            let e = launcher_plugins::update_for(p, catalog)?;
            Some(Finding::LauncherPlugin {
                id: p.page.id.clone(),
                name: p.page.name.clone(),
                to: e.version.clone(),
            })
        })
        .collect();
    if let Some(seen) = seen {
        out.extend(
            catalog
                .plugins
                .iter()
                .filter(|e| e.downloadable() && !seen.contains(&e.id))
                .filter(|e| !installed.iter().any(|p| p.page.id == e.id))
                .map(|e| Finding::NewPlugin {
                    id: e.id.clone(),
                    name: e.name.clone(),
                    dev: e.dev,
                }),
        );
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[derive(Debug, PartialEq, Eq)]
enum GameState {
    /// Nothing recorded yet: this manifest is where it starts.
    Baseline,
    Current,
    Outdated,
}

/// Pure: an installed version whose manifest was `recorded` (at its last update), now
/// that the manifest is `now`.
fn game_state(recorded: Option<&str>, now: &str, fake: bool) -> GameState {
    match recorded {
        _ if fake => GameState::Outdated,
        None => GameState::Baseline,
        Some(r) if r.eq_ignore_ascii_case(now) => GameState::Current,
        Some(_) => GameState::Outdated,
    }
}

/// Pure: the plugins of `v` installed from the catalogue that it has in another version.
fn plugin_findings(
    v: &InstalledVersion,
    plugins: &[mods::Plugin],
    catalog: &ModCatalog,
) -> Vec<Finding> {
    let mut packs = std::collections::HashSet::new();
    plugins
        .iter()
        .filter_map(|p| {
            let Source::Catalog { version, .. } = &p.source else {
                return None;
            };
            // A pack's plugins update with it: one finding for the pack.
            let m = match &p.pack {
                Some(_) => catalog.pack_for(&p.file),
                None => catalog.entry_for(&p.file),
            }
            // Only newer versions: going back to stable is the player's click on the Mods page.
            .filter(|m| {
                m.downloadable()
                    && crate::core::launcher::catalogue::offer(version, &m.version)
                        == Some(crate::core::launcher::catalogue::Offer::Update)
            })?;
            if m.pack.is_some() && !packs.insert(m.id.clone()) {
                return None;
            }
            // One that needs the player's OK (a block, nEVR, a mod it needs or conflicts with)
            // is offered on the Mods page only, never announced or updated from Play.
            if let Some(why) = mods::needs_ok(v, plugins, m, &goback::Rollback::cached()) {
                tracing::info!("updates: {why}: left to the Mods page");
                return None;
            }
            Some(Finding::Plugin {
                version_id: v.id.clone(),
                version_name: v.name.clone(),
                file: p.file.clone(),
                name: m.name.clone(),
                to: m.version.clone(),
            })
        })
        .collect()
}

/// Records what `check` learned in `state` (the baselines); true when it changed.
pub fn apply_baselines(state: &mut LauncherState, check: &Check) -> bool {
    let mut changed = false;
    // The launcher plugins listed are seen from now on (also ones a dev code listed, so a
    // code used again doesn't announce them again).
    if check.plugins.is_some() {
        changed |= state.plugins_seen.is_none();
        let seen = state.plugins_seen.get_or_insert_with(Vec::new);
        for id in &check.plugins_listed {
            if !seen.contains(id) {
                seen.push(id.clone());
                changed = true;
            }
        }
    }
    for (id, hash) in &check.baselines {
        if let Some(v) = state.versions.iter_mut().find(|v| &v.id == id) {
            if v.manifest_sha256.is_none() {
                v.manifest_sha256 = Some(hash.clone());
                changed = true;
            }
        }
    }
    changed
}

/// What an update or install leaves for the next check: a successful one starts from the
/// manifest then current (recorded at the next check), a failed one is still outdated.
pub fn after_update(v: &mut InstalledVersion, ok: bool) {
    v.manifest_sha256 = if ok { None } else { Some(FAILED.into()) };
}

/// The recorded manifest of a version whose last update failed: no manifest has it.
const FAILED: &str = "update-failed";

/// What going back from `v`'s last update leaves for the next check: its files aren't
/// any manifest's, so the update is out again (unless held, [`goback`]).
pub fn after_going_back(v: &mut InstalledVersion) {
    v.manifest_sha256 = Some(WENT_BACK.into());
}

const WENT_BACK: &str = "went-back";

// ---- announcing each finding once ----

fn notified_file() -> PathBuf {
    paths::data_dir().join("updates-notified.json")
}

/// The findings of `check` not announced yet; records them as announced. The record
/// keeps only what is still found, so it doesn't grow (after a catalogues-only check, the
/// launcher's and the games' stay as the last full check left them).
pub fn not_yet_announced(check: &Check) -> Vec<Finding> {
    not_yet_announced_in(&notified_file(), &check.findings, check.catalogs_only)
}

fn not_yet_announced_in(file: &Path, findings: &[Finding], catalogs_only: bool) -> Vec<Finding> {
    let before: BTreeSet<String> = std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let (fresh, now) = split_new(&before, findings, catalogs_only);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(&now) {
        let _ = std::fs::write(file, text);
    }
    fresh
}

/// Pure: the findings not in `before`, and the keys to keep: every finding's, and with
/// `catalogs_only` the ones of `before` such a check doesn't look at.
fn split_new(
    before: &BTreeSet<String>,
    findings: &[Finding],
    catalogs_only: bool,
) -> (Vec<Finding>, BTreeSet<String>) {
    let fresh = findings
        .iter()
        .filter(|f| !before.contains(&f.key()))
        .cloned()
        .collect();
    let mut keep: BTreeSet<String> = findings.iter().map(Finding::key).collect();
    if catalogs_only {
        keep.extend(before.iter().filter(|k| !catalog_key(k)).cloned());
    }
    (fresh, keep)
}

/// The desktop notification for `fresh` findings (one for all of them).
pub fn notify_desktop(fresh: &[Finding]) {
    if fresh.is_empty() {
        return;
    }
    let (summary, body) = notification_text(fresh);
    super::desktop_notify::show(&summary, &body);
}

/// Pure: a notification's title and text for `fresh` (at least one).
fn notification_text(fresh: &[Finding]) -> (String, String) {
    let new = fresh.iter().all(|f| matches!(f, Finding::NewPlugin { .. }));
    let summary = match (fresh.len(), new) {
        (1, true) => "New plugin".to_string(),
        (n, true) => format!("{n} new plugins"),
        (1, false) => "Update available".to_string(),
        (n, false) => format!("{n} updates available"),
    };
    let mut lines: Vec<String> = fresh.iter().take(4).map(Finding::text).collect();
    if fresh.len() > 4 {
        lines.push(format!("and {} more", fresh.len() - 4));
    }
    lines.push("Open the Echo VR Launcher to install.".into());
    (summary, lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::launcher::mods::{ModEntry, Plugin};

    fn plugin(file: &str, source: Source) -> Plugin {
        Plugin {
            file: file.into(),
            name: file.into(),
            version: String::new(),
            source,
            enabled: true,
            added: true,
            verified: true,
            args: Default::default(),
            defaults: Default::default(),
            required: false,
            present: true,
            status: None,
            settings: None,
            pack: None,
        }
    }

    #[test]
    fn a_game_starts_from_its_first_manifest() {
        assert_eq!(game_state(None, "ab", false), GameState::Baseline);
        assert_eq!(game_state(Some("AB"), "ab", false), GameState::Current);
        assert_eq!(game_state(Some("ab"), "cd", false), GameState::Outdated);
        assert_eq!(game_state(Some(FAILED), "cd", false), GameState::Outdated);
        // Gone back from an update: the update is out again, unless held.
        let mut v = InstalledVersion::default();
        after_going_back(&mut v);
        assert_eq!(
            game_state(v.manifest_sha256.as_deref(), "cd", false),
            GameState::Outdated
        );
        let mut state = LauncherState::default();
        let f = Finding::Game {
            id: "pc-latest".into(),
            name: "Echo VR".into(),
            hash: "cd".into(),
        };
        assert!(!held(&state, &f));
        state.holds.insert("game:pc-latest".into(), "cd".into());
        assert!(held(&state, &f));
        assert_eq!(game_state(Some("ab"), "ab", true), GameState::Outdated);

        let mut state = LauncherState::default();
        state.versions.push(InstalledVersion {
            id: "pc".into(),
            ..Default::default()
        });
        let check = Check {
            baselines: vec![("pc".into(), "ab".into())],
            ..Default::default()
        };
        assert!(apply_baselines(&mut state, &check));
        assert_eq!(state.versions[0].manifest_sha256.as_deref(), Some("ab"));
        // A recorded one isn't overwritten.
        let other = Check {
            baselines: vec![("pc".into(), "cd".into())],
            ..Default::default()
        };
        assert!(!apply_baselines(&mut state, &other));
        after_update(&mut state.versions[0], false);
        assert_eq!(state.versions[0].manifest_sha256.as_deref(), Some(FAILED));
        after_update(&mut state.versions[0], true);
        assert_eq!(state.versions[0].manifest_sha256, None);
    }

    #[test]
    fn plugins_from_the_catalogue_with_another_version() {
        let v = InstalledVersion {
            id: "pc".into(),
            name: "Echo VR".into(),
            ..Default::default()
        };
        let entry = |file: &str, version: &str| ModEntry {
            id: file.into(),
            name: format!("{file} plugin"),
            version: version.into(),
            file: file.into(),
            url: "plugins/x.dll".into(),
            ..Default::default()
        };
        let catalog = ModCatalog {
            mods: vec![
                entry("A.dll", "1.1"),
                entry("B.dll", "2.0"),
                entry("C.dll", "1.0"),
            ],
            builtin: false,
            ..Default::default()
        };
        let catalog_source = |v: &str| Source::Catalog {
            id: "x".into(),
            version: v.into(),
        };
        let plugins = vec![
            plugin("a.DLL", catalog_source("1.0")),
            plugin("B.dll", catalog_source("2.0")),
            plugin("C.dll", Source::Local),
        ];
        assert_eq!(
            plugin_findings(&v, &plugins, &catalog),
            vec![Finding::Plugin {
                version_id: "pc".into(),
                version_name: "Echo VR".into(),
                file: "a.DLL".into(),
                name: "A.dll plugin".into(),
                to: "1.1".into(),
            }]
        );
    }

    #[test]
    fn each_finding_is_announced_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notified.json");
        let launcher = Finding::Launcher {
            version: "0.11.0".into(),
            url: "https://github.com/x".into(),
        };
        let game = Finding::Game {
            id: "pc".into(),
            name: "Echo VR".into(),
            hash: "ab".into(),
        };
        assert_eq!(
            not_yet_announced_in(&file, std::slice::from_ref(&launcher), false),
            vec![launcher.clone()]
        );
        assert_eq!(
            not_yet_announced_in(&file, std::slice::from_ref(&launcher), false),
            vec![]
        );
        assert_eq!(
            not_yet_announced_in(&file, &[launcher.clone(), game.clone()], false),
            vec![game.clone()]
        );
        // Gone (installed) and found again later: a new update, announced again.
        assert_eq!(not_yet_announced_in(&file, &[], false), vec![]);
        assert_eq!(
            not_yet_announced_in(&file, std::slice::from_ref(&game), false),
            vec![game]
        );
    }

    /// The catalogues every 30 seconds in between: their findings come and go with them,
    /// the launcher's and the games' stay announced.
    #[test]
    fn a_catalogues_check_keeps_the_rest_announced() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notified.json");
        let launcher = Finding::Launcher {
            version: "0.11.14".into(),
            url: String::new(),
        };
        let plugin = Finding::NewPlugin {
            id: "game-server".into(),
            name: "Game server".into(),
            dev: true,
        };
        assert!(!launcher.is_catalogs() && plugin.is_catalogs());
        assert_eq!(
            not_yet_announced_in(&file, std::slice::from_ref(&launcher), false),
            vec![launcher.clone()]
        );
        assert_eq!(
            not_yet_announced_in(&file, std::slice::from_ref(&plugin), true),
            vec![plugin.clone()]
        );
        assert_eq!(not_yet_announced_in(&file, &[], true), vec![]);
        // The next full check: the launcher's update was announced already.
        assert_eq!(
            not_yet_announced_in(&file, std::slice::from_ref(&launcher), false),
            vec![]
        );
    }

    #[test]
    fn notification_texts() {
        let one = [Finding::Game {
            id: "pc".into(),
            name: "Echo VR (PC, latest)".into(),
            hash: "ab".into(),
        }];
        let (s, b) = notification_text(&one);
        assert_eq!(s, "Update available");
        assert!(b.starts_with("An update for Echo VR (PC, latest) is available"));
        let many: Vec<Finding> = (0..6)
            .map(|i| Finding::Launcher {
                version: format!("0.{i}"),
                url: String::new(),
            })
            .collect();
        let (s, b) = notification_text(&many);
        assert_eq!(s, "6 updates available");
        assert!(b.contains("and 2 more"));
        let new = [Finding::NewPlugin {
            id: "game-server".into(),
            name: "Game server".into(),
            dev: true,
        }];
        let (s, b) = notification_text(&new);
        assert_eq!(s, "New plugin");
        assert!(b.starts_with("New dev plugin: Game server, on the Plugins page"));
    }

    fn entry(id: &str, version: &str, url: &str) -> launcher_plugins::PluginEntry {
        launcher_plugins::PluginEntry {
            id: id.into(),
            name: id.to_uppercase(),
            version: version.into(),
            url: url.into(),
            ..Default::default()
        }
    }

    fn installed(id: &str, from_catalog: Option<&str>) -> Installed {
        Installed {
            page: std::sync::Arc::new(crate::core::launcher::plugin_page::PluginPage {
                id: id.into(),
                name: id.to_uppercase(),
                ..Default::default()
            }),
            dir: PathBuf::new(),
            from_catalog: from_catalog.map(String::from),
            warnings: Vec::new(),
            ..Default::default()
        }
    }

    /// An installed plugin's update, and a plugin listed for the first time; nothing new
    /// before anything was seen, nor what's installed, seen or not downloadable yet.
    #[test]
    fn launcher_plugins_new_and_updated() {
        let catalog = PluginCatalog {
            plugins: vec![
                entry("event-lobbies", "0.1.3", "plugins/e.zip"),
                entry("game-server", "0.1.0", "files/g.zip"),
                entry("old", "1.0", "plugins/o.zip"),
                entry("soon", "", ""),
            ],
            ..Default::default()
        };
        let have = [
            installed("event-lobbies", Some("0.1.2")),
            installed("by-hand", None),
        ];
        let first = launcher_plugin_findings(&have, &catalog, None);
        assert_eq!(
            first,
            [Finding::LauncherPlugin {
                id: "event-lobbies".into(),
                name: "EVENT-LOBBIES".into(),
                to: "0.1.3".into(),
            }]
        );
        let seen = ["event-lobbies".to_string(), "old".to_string()];
        let later = launcher_plugin_findings(&have, &catalog, Some(&seen));
        assert_eq!(later.len(), 2);
        assert_eq!(later[1].key(), "new-plugin:game-server");
    }

    /// The first check records what's listed; later ones add to it (a dev code's plugins
    /// stay seen when the code goes).
    #[test]
    fn plugins_seen_baseline() {
        let mut state = LauncherState::default();
        let mut check = Check {
            plugins: Some(PluginCatalog::default()),
            plugins_listed: vec!["a".into()],
            ..Check::default()
        };
        assert!(apply_baselines(&mut state, &check));
        assert_eq!(state.plugins_seen.as_deref(), Some(&["a".to_string()][..]));
        assert!(!apply_baselines(&mut state, &check));
        check.plugins_listed = vec!["b".into()];
        assert!(apply_baselines(&mut state, &check));
        assert_eq!(
            state.plugins_seen.as_deref(),
            Some(&["a".to_string(), "b".to_string()][..])
        );
        // No catalogue (offline): nothing recorded.
        let mut fresh = LauncherState::default();
        assert!(!apply_baselines(&mut fresh, &Check::default()));
        assert!(fresh.plugins_seen.is_none());
    }
}
