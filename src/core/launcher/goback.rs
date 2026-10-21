//! Going back to the version before an update, part by part, when an update breaks
//! something: the launcher, a version's game update (nEVR), a mod, a plugin.
//!
//! Each update keeps what it replaced (the game update in `_local/previous-update/`, a mod
//! in `_local/previous-mods/`, a plugin as `.<id>.previous`), so going back right after an
//! update needs no network. Without such a copy (installed after the update, or gone back
//! once already) the version before comes from release.echovr.de's history
//! ([`super::history`]), which keeps every version; the launcher's releases stay on the
//! download servers anyway. The version gone back from is then held
//! ([`LauncherState::holds`]): it isn't offered again, a newer one is. "Use newest" lets go
//! of the hold.
//!
//! release.echovr.de can pull a version for everyone (`launcher/rollback.json`,
//! [`Pulled`]): who has it is told, and offered the way back; with `auto`, the launcher goes
//! back by itself.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use serde::Deserialize;

use super::history::History;
use super::store::{InstalledVersion, LauncherState};
use super::versions::Step;

/// Where the pulled versions are listed.
pub const ROLLBACK_URL: &str = "https://release.echovr.de/launcher/rollback.json";

/// A part that updates on its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Part {
    Launcher,
    /// The game update of an installed version (by its id).
    Game {
        version: String,
    },
    /// A mod of an installed version (by its plugin file).
    Mod {
        version: String,
        file: String,
    },
    /// A launcher plugin (by its id).
    Plugin {
        id: String,
    },
}

impl Part {
    /// Its key in [`LauncherState::holds`].
    pub fn key(&self) -> String {
        match self {
            Part::Launcher => "launcher".into(),
            Part::Game { version } => format!("game:{version}"),
            Part::Mod { version, file } => format!("mod:{version}:{}", file.to_lowercase()),
            Part::Plugin { id } => format!("plugin:{id}"),
        }
    }
}

/// A way back: a part updated, with what it has now and what it goes back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WayBack {
    pub part: Part,
    /// What it's called ("Launcher", "nEVR runtime (Echo VR)", "NvrGpuRating", ...).
    pub name: String,
    /// The version it has now, and the one before (shown).
    pub now: String,
    pub before: String,
    /// When it was updated, for the game update (RFC 3339).
    pub at: Option<String>,
    /// From release.echovr.de's history (a download), not a copy kept here.
    pub server: bool,
    /// What it downloads, when known.
    pub size: Option<u64>,
}

/// Whether `found` (a version, or for the game its manifest's sha256) is the one held for
/// `part`: not offered as an update.
pub fn held(holds: &BTreeMap<String, String>, part: &Part, found: &str) -> bool {
    holds
        .get(&part.key())
        .is_some_and(|h| h.eq_ignore_ascii_case(found))
}

/// What a version's game update is called: nEVR's version when it changed, else "the
/// update".
fn game_labels(p: &crate::core::pc_update::Previous) -> (String, String) {
    match (&p.nevr_to, &p.nevr_from) {
        (Some(to), Some(from)) if to != from => (
            format!("nEVR {}", short(to)),
            format!("nEVR {}", short(from)),
        ),
        _ => ("this update".into(), "the files before it".into()),
    }
}

/// `5.2.1+3.abc` → `5.2.1`.
fn short(v: &str) -> &str {
    v.split(['+', '-']).next().unwrap_or(v)
}

/// Every way back there is now (file I/O and network: on a worker).
pub fn ways_back(state: &LauncherState) -> Vec<WayBack> {
    ways_with(state, History::load().as_ref())
}

/// [`ways_back`] given release.echovr.de's history (`None`: unreadable, kept copies only).
fn ways_with(state: &LauncherState, h: Option<&History>) -> Vec<WayBack> {
    let mut out = Vec::new();
    let now = env!("CARGO_PKG_VERSION");
    // The version it updated from, else the release before it on the download servers.
    let before = super::self_update::supported()
        .then(|| {
            state
                .launcher_previous
                .clone()
                .or_else(|| super::self_update::previous_release(now))
        })
        .flatten()
        .filter(|b| b != now);
    if let Some(before) = before {
        out.push(WayBack {
            part: Part::Launcher,
            name: "Echo VR Launcher".into(),
            now: now.into(),
            before,
            at: None,
            server: true,
            size: None,
        });
    }
    for v in state.versions.iter().filter(|v| v.present()) {
        out.extend(of_version(v, h));
    }
    let root = super::plugins::dir();
    let kept = super::plugins::previous_in(&root);
    for p in &kept {
        out.push(WayBack {
            part: Part::Plugin { id: p.id.clone() },
            name: p.name.clone(),
            now: p.now.clone(),
            before: p.before.clone(),
            at: None,
            server: false,
            size: None,
        });
    }
    if let Some(h) = h {
        for p in super::plugins::installed_in(&root) {
            let Some(now) = p.from_catalog.as_deref() else {
                continue;
            };
            if kept.iter().any(|k| k.id == p.page.id) {
                continue;
            }
            if let Some(e) = h.plugin_before(&p.page.id, now) {
                out.push(WayBack {
                    part: Part::Plugin {
                        id: p.page.id.clone(),
                    },
                    name: p.page.name.clone(),
                    now: now.to_string(),
                    before: e.version,
                    at: None,
                    server: true,
                    size: e.size,
                });
            }
        }
    }
    out
}

fn of_version(v: &InstalledVersion, h: Option<&History>) -> Vec<WayBack> {
    let mut out = Vec::new();
    let game = Part::Game {
        version: v.id.clone(),
    };
    let name = format!("Game update ({})", v.name);
    if let Some(p) = super::versions::previous_update(v) {
        let (now, before) = game_labels(&p);
        out.push(WayBack {
            part: game,
            name,
            now,
            before,
            at: p.at,
            server: false,
            size: None,
        });
    } else if let Some(h) = h {
        match super::versions::server_back(v, h) {
            Ok(Some(b)) => out.push(WayBack {
                part: game,
                name,
                now: b.now,
                before: b.before,
                at: Some(b.at),
                server: true,
                size: None,
            }),
            Ok(None) => {}
            Err(e) => tracing::info!("go back: {}'s earlier updates: {e:#}", v.id),
        }
    }
    for m in super::mods::previous(v) {
        out.push(WayBack {
            part: Part::Mod {
                version: v.id.clone(),
                file: m.file,
            },
            name: format!(
                "{} ({})",
                if m.name.is_empty() { "Mod" } else { &m.name },
                v.name
            ),
            now: m.now,
            before: m.before,
            at: None,
            server: false,
            size: None,
        });
    }
    if let Some(h) = h {
        let rollback = Rollback::cached();
        for (file, id, now, name) in super::mods::from_catalog_unkept(v) {
            if let Some(e) = h.mod_before(&id, &now, |e| fits_back(&rollback, e)) {
                let size = e.size.or(e.pack.as_ref().and_then(|p| p.size));
                let shown = if name.is_empty() {
                    e.name.clone()
                } else {
                    name
                };
                out.push(WayBack {
                    part: Part::Mod {
                        version: v.id.clone(),
                        file,
                    },
                    name: format!(
                        "{} ({})",
                        if shown.is_empty() { "Mod" } else { &shown },
                        v.name
                    ),
                    now,
                    before: e.version,
                    at: None,
                    server: true,
                    size,
                });
            }
        }
    }
    out
}

/// Goes back (a version's game update, a mod, a plugin; the launcher's is a job of its
/// own, [`super::self_update::install`]): to the copy kept, else to the version before from
/// release.echovr.de's history. Returns the hold to note: the part's key and what it went
/// back from.
pub fn go_back(
    state: &LauncherState,
    part: &Part,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> anyhow::Result<(String, String)> {
    let version = |id: &str| {
        state
            .versions
            .iter()
            .find(|v| v.id == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("That Echo VR isn't installed any more."))
    };
    let from = match part {
        Part::Launcher => anyhow::bail!("The launcher goes back through its own update."),
        Part::Game { version: id } => super::versions::go_back(&version(id)?, cancel, on)?,
        Part::Mod { version: id, file } => {
            let v = version(id)?;
            if super::mods::previous(&v)
                .iter()
                .any(|p| p.file.eq_ignore_ascii_case(file))
            {
                super::mods::go_back(&v, file)?
            } else {
                let (_, cid, now, _) = super::mods::from_catalog_unkept(&v)
                    .into_iter()
                    .find(|(f, ..)| f.eq_ignore_ascii_case(file))
                    .ok_or_else(|| anyhow::anyhow!("There is no earlier version of {file}."))?;
                let rollback = Rollback::cached();
                let e = History::load()
                    .and_then(|h| h.mod_before(&cid, &now, |e| fits_back(&rollback, e)))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "There is no earlier version of {file} on release.echovr.de."
                        )
                    })?;
                super::mods::go_back_to(&v, file, &e, cancel, on)?
            }
        }
        Part::Plugin { id } => {
            let root = super::plugins::dir();
            if super::plugins::previous_in(&root)
                .iter()
                .any(|p| p.id == *id)
            {
                super::plugins::go_back(&root, id)?
            } else {
                let now = super::plugins::installed_in(&root)
                    .into_iter()
                    .find(|p| p.page.id == *id)
                    .and_then(|p| p.from_catalog)
                    .ok_or_else(|| {
                        anyhow::anyhow!("That plugin isn't installed from the catalogue.")
                    })?;
                let e = History::load()
                    .and_then(|h| h.plugin_before(id, &now))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "There is no earlier version of this plugin on release.echovr.de."
                        )
                    })?;
                super::plugins::go_back_to(&root, &e, cancel)?
            }
        }
    };
    Ok((part.key(), from))
}

/// Whether going back to version `version` of mod `id` is fine as far as prereleases go:
/// a prerelease only for a mod the player takes prereleases of.
pub fn may_go_back_to(id: &str, version: &str) -> bool {
    !crate::core::semver::Version::parse(version).is_some_and(|v| v.is_prerelease())
        || super::prefs::get().prereleases.contains(id)
}

/// Pure but for the prerelease choice: a history entry to go back to, neither blocked nor an
/// unwanted prerelease.
fn fits_back(rollback: &Rollback, e: &super::mods::ModEntry) -> bool {
    rollback
        .block_for(Some(&e.id), &e.file, &e.version, e.sha256.as_deref())
        .is_none()
        && may_go_back_to(&e.id, &e.version)
}

// ---- release.echovr.de's pulled versions ----

/// `launcher/rollback.json`: versions with a serious problem.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rollback {
    #[serde(default)]
    pub pulled: Vec<Pulled>,
    /// Mod versions blocked for everyone (schema 2, `echo-mod reject`): never offered, and
    /// an installed one goes back to the newest that isn't, or stays out of the start.
    #[serde(default)]
    pub blocked: Vec<Block>,
}

/// A block: versions of one mod nobody should run.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct Block {
    /// `mod` (others are for later).
    pub part: String,
    /// The mod's catalogue id (or its file).
    pub id: String,
    /// The versions: a range ([`crate::core::semver`]), prereleases included, or `all`.
    pub versions: String,
    /// Or one file exactly, by its sha256.
    pub sha: String,
    /// `off` (out of the start) or `remove` (its files set aside as well).
    pub action: String,
    /// What is wrong, for the player.
    pub note: String,
    /// An alert that says more ([`super::alerts`]).
    pub alert: String,
}

impl Block {
    /// Pure: whether it blocks mod `id` (catalogue id, if known) in `file` at `version`
    /// (its file's `sha`, if known).
    pub fn covers(&self, id: Option<&str>, file: &str, version: &str, sha: Option<&str>) -> bool {
        if self.part != "mod" || self.id.is_empty() {
            return false;
        }
        let named = id.is_some_and(|i| i == self.id) || file.eq_ignore_ascii_case(&self.id);
        let versions = match self.versions.trim() {
            "" if !self.sha.is_empty() => true,
            "" => false,
            // Every version: also one that isn't a semantic version.
            "all" | "*" => true,
            r => crate::core::semver::in_range(version, r, true),
        };
        let file_ok = self.sha.is_empty() || sha.is_some_and(|s| s.eq_ignore_ascii_case(&self.sha));
        named && versions && file_ok
    }

    /// Set its files aside too, not only keep it out.
    pub fn removes(&self) -> bool {
        self.action == "remove"
    }
}

/// A pulled version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pulled {
    /// `launcher`, `game`, `mod` or `plugin`.
    #[serde(default)]
    pub part: String,
    /// The version pulled: the launcher's, a mod's or a plugin's; for the game the sha256
    /// of its update manifest (`sha`).
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub sha: String,
    /// The mod's or plugin's catalogue id (a mod: or its file).
    #[serde(default)]
    pub id: String,
    /// What is wrong, for the player.
    #[serde(default)]
    pub note: String,
    /// Go back without asking (when nothing is running).
    #[serde(default)]
    pub auto: bool,
}

impl Rollback {
    pub fn parse(text: &str) -> anyhow::Result<Rollback> {
        Ok(serde_json::from_str(text)?)
    }

    /// The published list (none when it can't be read: nothing is pulled; blocks as kept
    /// last). Kept for [`Rollback::cached`]. Asked for with a short timeout: it is read
    /// before every start.
    pub fn load() -> Rollback {
        let path = crate::core::paths::data_dir().join("rollback.json");
        let got = super::test_feed::read("rollback.json")
            .map(Ok)
            .unwrap_or_else(|| {
                crate::core::sources::text(ROLLBACK_URL, std::time::Duration::from_secs(8))
            });
        match got {
            Ok(Some(t)) => match Rollback::parse(&t) {
                Ok(r) => {
                    let _ = std::fs::create_dir_all(crate::core::paths::data_dir());
                    let _ = std::fs::write(&path, &t);
                    r
                }
                // A broken file never lifts a block: the last good one holds.
                Err(e) => {
                    tracing::warn!("rollback.json unreadable: {e:#}; the last one kept holds");
                    Rollback {
                        pulled: Vec::new(),
                        ..Rollback::cached()
                    }
                }
            },
            Ok(None) => {
                let _ = std::fs::remove_file(&path);
                Rollback::default()
            }
            Err(e) => {
                tracing::info!("rollback.json unavailable: {e:#}");
                Rollback {
                    pulled: Vec::new(),
                    ..Rollback::cached()
                }
            }
        }
    }

    /// The list as kept last (no network: before a start, while reading the mods).
    pub fn cached() -> Rollback {
        if cfg!(test) {
            return Rollback::default();
        }
        std::fs::read_to_string(crate::core::paths::data_dir().join("rollback.json"))
            .ok()
            .and_then(|t| Rollback::parse(&t).ok())
            .unwrap_or_default()
    }

    /// The block that covers mod `id` in `file` at `version`, if any ([`Block::covers`]).
    pub fn block_for(
        &self,
        id: Option<&str>,
        file: &str,
        version: &str,
        sha: Option<&str>,
    ) -> Option<&Block> {
        self.blocked
            .iter()
            .find(|b| b.covers(id, file, version, sha))
    }
}

/// A pulled version this launcher has, with its way back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Affected {
    pub way: WayBack,
    pub note: String,
    pub auto: bool,
}

/// Pure but for what it reads of the game's manifest: the ways back whose current version
/// is pulled. `game_sha` gives a version's recorded update manifest sha256 (what it has
/// now), `mod_id` a mod file's catalogue id, `plugin_now` nothing (plugins carry their
/// version in the way back).
pub fn affected(
    rollback: &Rollback,
    ways: &[WayBack],
    game_sha: &dyn Fn(&str) -> Option<String>,
    mod_id: &dyn Fn(&str, &str) -> Option<String>,
) -> Vec<Affected> {
    ways.iter()
        .filter_map(|w| {
            let p = rollback
                .pulled
                .iter()
                .find(|p| match (&w.part, p.part.as_str()) {
                    (Part::Launcher, "launcher") => p.version == w.now,
                    (Part::Game { version }, "game") => {
                        !p.sha.is_empty()
                            && game_sha(version).is_some_and(|s| s.eq_ignore_ascii_case(&p.sha))
                    }
                    (Part::Mod { version, file }, "mod") => {
                        p.version == w.now
                            && (p.id.eq_ignore_ascii_case(file)
                                || mod_id(version, file).is_some_and(|id| id == p.id))
                    }
                    (Part::Plugin { id }, "plugin") => p.version == w.now && p.id == *id,
                    _ => false,
                })?;
            Some(Affected {
                way: w.clone(),
                note: p.note.clone(),
                auto: p.auto,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn way(part: Part, now: &str, before: &str) -> WayBack {
        WayBack {
            part,
            name: "x".into(),
            now: now.into(),
            before: before.into(),
            at: None,
            server: false,
            size: None,
        }
    }

    #[test]
    fn holds_skip_only_the_version_gone_back_from() {
        let mut holds = BTreeMap::new();
        let p = Part::Plugin {
            id: "game-server".into(),
        };
        assert!(!held(&holds, &p, "0.3.0"));
        holds.insert(p.key(), "0.3.0".into());
        assert!(held(&holds, &p, "0.3.0"));
        // A newer one (the fix) is offered again.
        assert!(!held(&holds, &p, "0.3.1"));
        // Another part isn't held.
        assert!(!held(&holds, &Part::Plugin { id: "other".into() }, "0.3.0"));
        let g = Part::Game {
            version: "pc-latest".into(),
        };
        holds.insert(g.key(), "ABC".into());
        assert!(held(&holds, &g, "abc"));
        assert_eq!(
            Part::Mod {
                version: "pc-latest".into(),
                file: "NvrGpuRating.dll".into()
            }
            .key(),
            "mod:pc-latest:nvrgpurating.dll"
        );
    }

    #[test]
    fn pulled_versions_reach_who_has_them() {
        let r = Rollback::parse(
            r#"{"schema":1,"pulled":[
              {"part":"game","sha":"BAD","note":"crashes on start","auto":true},
              {"part":"launcher","version":"0.11.18","note":"no start"},
              {"part":"mod","id":"gpu-rating","version":"1.1.0"},
              {"part":"plugin","id":"game-server","version":"0.3.0"},
              {"part":"something new","version":"1"}]}"#,
        )
        .unwrap();
        let ways = [
            way(Part::Launcher, "0.11.18", "0.11.17"),
            way(
                Part::Game {
                    version: "pc-latest".into(),
                },
                "nEVR 5.2.1",
                "nEVR 4.1.0",
            ),
            way(
                Part::Game {
                    version: "other".into(),
                },
                "this update",
                "the files before it",
            ),
            way(
                Part::Mod {
                    version: "pc-latest".into(),
                    file: "NvrGpuRating.dll".into(),
                },
                "1.1.0",
                "1.0.0",
            ),
            way(
                Part::Plugin {
                    id: "game-server".into(),
                },
                "0.3.1",
                "0.3.0",
            ),
        ];
        let game = |v: &str| Some(if v == "pc-latest" { "bad" } else { "good" }.to_string());
        let mod_id = |_: &str, f: &str| (f == "NvrGpuRating.dll").then(|| "gpu-rating".into());
        let hit = affected(&r, &ways, &game, &mod_id);
        let names: Vec<_> = hit.iter().map(|a| a.way.part.key()).collect();
        assert_eq!(
            names,
            [
                "launcher",
                "game:pc-latest",
                "mod:pc-latest:nvrgpurating.dll"
            ]
        );
        assert!(hit[1].auto && !hit[0].auto);
        assert_eq!(hit[1].note, "crashes on start");
        // Nothing listed, nothing pulled.
        assert!(affected(&Rollback::default(), &ways, &game, &mod_id).is_empty());
    }
}
