//! Going back to the version before an update, part by part, when an update breaks
//! something: the launcher, a version's game update (nEVR), a mod, a plugin.
//!
//! Each update keeps what it replaced (the game update in `_local/previous-update/`, a mod
//! in `_local/previous-mods/`, a plugin as `.<id>.previous`; the launcher's releases stay
//! on the download servers), so going back needs no network but for the launcher. The
//! version gone back from is then held ([`LauncherState::holds`]): it isn't offered again,
//! a newer one is. "Use newest" lets go of the hold.
//!
//! release.echovr.de can pull a version for everyone (`launcher/rollback.json`,
//! [`Pulled`]): who has it is told, and offered the way back; with `auto`, the launcher goes
//! back by itself.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::store::{InstalledVersion, LauncherState};

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

/// Every way back there is now (file I/O: on a worker).
pub fn ways_back(state: &LauncherState) -> Vec<WayBack> {
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
        });
    }
    for v in state.versions.iter().filter(|v| v.present()) {
        out.extend(of_version(v));
    }
    for p in super::plugins::previous_in(&super::plugins::dir()) {
        out.push(WayBack {
            part: Part::Plugin { id: p.id },
            name: p.name,
            now: p.now,
            before: p.before,
            at: None,
        });
    }
    out
}

fn of_version(v: &InstalledVersion) -> Vec<WayBack> {
    let mut out = Vec::new();
    if let Some(p) = super::versions::previous_update(v) {
        let (now, before) = game_labels(&p);
        out.push(WayBack {
            part: Part::Game {
                version: v.id.clone(),
            },
            name: format!("Game update ({})", v.name),
            now,
            before,
            at: p.at,
        });
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
        });
    }
    out
}

/// Goes back (a version's game update, a mod, a plugin; the launcher's is a job of its
/// own, [`super::self_update::install`]). Returns the hold to note: the part's key and
/// what it went back from.
pub fn go_back(state: &LauncherState, part: &Part) -> anyhow::Result<(String, String)> {
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
        Part::Game { version: id } => super::versions::go_back(&version(id)?)?,
        Part::Mod { version: id, file } => super::mods::go_back(&version(id)?, file)?,
        Part::Plugin { id } => super::plugins::go_back(&super::plugins::dir(), id)?,
    };
    Ok((part.key(), from))
}

// ---- release.echovr.de's pulled versions ----

/// `launcher/rollback.json`: versions with a serious problem.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rollback {
    #[serde(default)]
    pub pulled: Vec<Pulled>,
}

/// A pulled version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pulled {
    /// `launcher`, `game`, `mod` or `plugin`.
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

    /// The published list (none when it can't be read: nothing is pulled).
    pub fn load() -> Rollback {
        match crate::core::http::get_text_opt(ROLLBACK_URL) {
            Ok(Some(t)) => Rollback::parse(&t).unwrap_or_else(|e| {
                tracing::warn!("rollback.json unreadable: {e:#}");
                Rollback::default()
            }),
            Ok(None) => Rollback::default(),
            Err(e) => {
                tracing::info!("rollback.json unavailable: {e:#}");
                Rollback::default()
            }
        }
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
