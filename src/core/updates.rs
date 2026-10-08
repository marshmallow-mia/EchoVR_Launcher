//! What can be updated, and telling the player once: the launcher (its latest GitHub
//! release), each installed Echo VR that gets updates (its update manifest changed since
//! the launcher last brought it up to date), and the plugins installed from the mods
//! catalogue (the catalogue has another version). Nothing is installed here: the launcher
//! installs on a click. EchoXR and EchoXR Hands come with the launcher (their versions
//! are pinned in it), so a launcher update brings theirs.
//!
//! The launcher checks at its start and every 15 minutes, and so does the tray
//! (`ui::tray`) while the launcher is closed. Each finding is announced on the desktop
//! once: the ones announced are kept in `updates-notified.json`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::launcher::mods::{self, ModCatalog, Source};
use super::launcher::store::{InstalledVersion, LauncherState};
use super::launcher::{update_check, versions};
use super::paths;

/// How often the launcher and the tray look.
pub const INTERVAL: Duration = Duration::from_secs(15 * 60);

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
        }
    }

    /// One line for a notice or a notification.
    pub fn text(&self) -> String {
        match self {
            Finding::Launcher { version, .. } => {
                format!("Echo VR Launcher {version} is out")
            }
            Finding::Game { name, .. } => format!("An update for {name} is ready"),
            Finding::Plugin {
                version_name,
                name,
                to,
                ..
            } => format!("{name} {to} is out (in {version_name})"),
        }
    }
}

/// What a check found, and what it learned on the way.
#[derive(Debug, Clone, Default)]
pub struct Check {
    pub findings: Vec<Finding>,
    /// Versions without a recorded manifest yet: the manifest they start from (version id,
    /// hash). Recorded silently: they were brought up to date just before (or before this
    /// launcher kept track).
    pub baselines: Vec<(String, String)>,
    /// The catalogue fetched, for the Mods page's Update buttons.
    pub catalog: Option<ModCatalog>,
    /// The launcher channel its launcher finding is from.
    pub launcher_channel: update_check::Channel,
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
        match crate::core::http::get_bytes(&url) {
            Ok(bytes) => {
                let hash = sha256_hex(&bytes);
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
    let catalog = ModCatalog::load();
    if !catalog.builtin {
        for v in state.versions.iter().filter(|v| v.present()) {
            out.findings
                .extend(plugin_findings(v, &mods::read(v).plugins, &catalog));
        }
        out.catalog = Some(catalog);
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
    plugins
        .iter()
        .filter_map(|p| {
            let Source::Catalog { version, .. } = &p.source else {
                return None;
            };
            let m = catalog
                .entry_for(&p.file)
                .filter(|m| m.downloadable() && m.version != *version)?;
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

// ---- announcing each finding once ----

fn notified_file() -> PathBuf {
    paths::data_dir().join("updates-notified.json")
}

/// The findings of `findings` not announced yet; records them as announced. The record
/// keeps only what is still found, so it doesn't grow.
pub fn not_yet_announced(findings: &[Finding]) -> Vec<Finding> {
    not_yet_announced_in(&notified_file(), findings)
}

fn not_yet_announced_in(file: &Path, findings: &[Finding]) -> Vec<Finding> {
    let before: BTreeSet<String> = std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let (fresh, now) = split_new(&before, findings);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(&now) {
        let _ = std::fs::write(file, text);
    }
    fresh
}

/// Pure: the findings not in `before`, and the keys to keep (every finding's).
fn split_new(before: &BTreeSet<String>, findings: &[Finding]) -> (Vec<Finding>, BTreeSet<String>) {
    let fresh = findings
        .iter()
        .filter(|f| !before.contains(&f.key()))
        .cloned()
        .collect();
    (fresh, findings.iter().map(Finding::key).collect())
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
    let summary = if fresh.len() == 1 {
        "Update available".to_string()
    } else {
        format!("{} updates available", fresh.len())
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
        }
    }

    #[test]
    fn a_game_starts_from_its_first_manifest() {
        assert_eq!(game_state(None, "ab", false), GameState::Baseline);
        assert_eq!(game_state(Some("AB"), "ab", false), GameState::Current);
        assert_eq!(game_state(Some("ab"), "cd", false), GameState::Outdated);
        assert_eq!(game_state(Some(FAILED), "cd", false), GameState::Outdated);
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
            not_yet_announced_in(&file, std::slice::from_ref(&launcher)),
            vec![launcher.clone()]
        );
        assert_eq!(
            not_yet_announced_in(&file, std::slice::from_ref(&launcher)),
            vec![]
        );
        assert_eq!(
            not_yet_announced_in(&file, &[launcher.clone(), game.clone()]),
            vec![game.clone()]
        );
        // Gone (installed) and found again later: a new update, announced again.
        assert_eq!(not_yet_announced_in(&file, &[]), vec![]);
        assert_eq!(
            not_yet_announced_in(&file, std::slice::from_ref(&game)),
            vec![game]
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
        assert!(b.starts_with("An update for Echo VR (PC, latest) is ready"));
        let many: Vec<Finding> = (0..6)
            .map(|i| Finding::Launcher {
                version: format!("0.{i}"),
                url: String::new(),
            })
            .collect();
        let (s, b) = notification_text(&many);
        assert_eq!(s, "6 updates available");
        assert!(b.contains("and 2 more"));
    }
}
