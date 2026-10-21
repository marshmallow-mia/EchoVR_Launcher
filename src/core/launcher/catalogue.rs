//! `launcher/catalogue.json`: every version of every published third-party mod, prereleases
//! flagged, with what each needs ([`Requires`]) and what it can't run beside, and the
//! versions blocked for everyone. Only launchers since 0.13 read it: `mods.json` stays the
//! launcher's own mods, so older launchers never see a third-party one. Unlisted mods have
//! the same shape, one file each: `launcher/unlisted/<id>.json`.
//!
//! The launcher offers each mod's newest version no block covers (here or in rollback.json),
//! a prerelease only for the mods the player tries prereleases of ([`super::prefs`]). Without
//! the file (not published, not readable) there are no third-party mods to get.

use serde::Deserialize;

use super::mods::ModEntry;
use crate::core::semver;

pub const URL: &str = "https://release.echovr.de/launcher/catalogue.json";
/// Where an unlisted mod's file is: `{UNLISTED}<id>.json`.
pub const UNLISTED: &str = "https://release.echovr.de/launcher/unlisted/";
/// Where a private mod's file is (behind the EchoVRCE sign-in): `{PRIVATE}<id>.json`.
pub const PRIVATE: &str = "https://release.echovr.de/launcher/gated/private/";

/// What Add by id found for an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookUp {
    /// A mod with something to offer: its name, and whether that's prereleases only.
    Found { name: String, prerelease_only: bool },
    /// No unlisted or private mod has it (or it has nothing to offer now).
    None,
    /// A private mod, and no EchoVRCE sign-in.
    SignIn,
    /// A private mod this account isn't allowed to have.
    NotAllowed,
}

/// Pure: what `text` (an unlisted or private mod's file) has for `id`.
pub fn found_in(text: &str, id: &str) -> LookUp {
    let Some(m) = Catalogue::parse(text)
        .ok()
        .and_then(|c| c.mods.into_iter().find(|m| m.id == id))
    else {
        return LookUp::None;
    };
    match (
        m.choose_with(false, &|_| false),
        m.choose_with(true, &|_| false),
    ) {
        (Some(v), _) => LookUp::Found {
            name: v.entry.name.clone(),
            prerelease_only: false,
        },
        (None, Some(v)) => LookUp::Found {
            name: v.entry.name.clone(),
            prerelease_only: true,
        },
        (None, None) => LookUp::None,
    }
}

/// Looks `id` up: an unlisted mod, else a private one (with the EchoVRCE session).
pub fn look_up(id: &str) -> Result<LookUp, String> {
    if let Some(t) = super::test_feed::read(&format!("unlisted/{id}.json")) {
        return Ok(t.map_or(LookUp::None, |t| found_in(&t, id)));
    }
    let get = |url: String| crate::core::http::get_text_opt(&url);
    match get(format!("{UNLISTED}{id}.json")) {
        Ok(Some(t)) => return Ok(found_in(&t, id)),
        Ok(None) => {}
        Err(e) => return Err(format!("{e:#}")),
    }
    match get(format!("{PRIVATE}{id}.json")) {
        Ok(Some(t)) => Ok(found_in(&t, id)),
        Ok(None) => Ok(LookUp::None),
        Err(e) => match crate::core::http::status_of(&e) {
            Some(401) => Ok(LookUp::SignIn),
            Some(403) => Ok(LookUp::NotAllowed),
            _ => Err(format!("{e:#}")),
        },
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Catalogue {
    pub mods: Vec<CatalogueMod>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CatalogueMod {
    pub id: String,
    pub name: String,
    /// The GitHub account that published it (shown on its row: a community mod).
    pub publisher: String,
    /// Its repository, `owner/name`.
    pub repo: String,
    pub visibility: String,
    /// Newest first (the server's order; [`CatalogueMod::choose`] doesn't rely on it).
    pub versions: Vec<CatalogueVersion>,
}

/// One version: a mods.json entry (with its [`ModEntry::prerelease`], `requires` and
/// `conflicts`) plus what only this file has.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CatalogueVersion {
    #[serde(flatten)]
    pub entry: ModEntry,
    /// When it came out (RFC 3339).
    pub released: Option<String>,
    /// Blocked for everyone (`echo-mod reject`): never offered.
    pub blocked: Option<Blocked>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Requires {
    /// The nEVR versions it runs on, a range (`>=4.1.0`).
    pub nevr: Option<String>,
    /// Mods it needs on beside it.
    pub mods: Vec<Dependency>,
}

impl Requires {
    pub fn is_empty(&self) -> bool {
        self.nevr.is_none() && self.mods.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Dependency {
    pub id: String,
    /// The versions that do, a range (`>=1.2.0`); empty: any.
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Blocked {
    /// `off` or `remove`.
    pub action: String,
    pub note: String,
    /// The alert that says more ([`super::alerts`]), if any.
    pub alert: String,
}

impl Catalogue {
    /// The catalogue in `text`; a mod or version that doesn't read right is left out.
    pub fn parse(text: &str) -> anyhow::Result<Catalogue> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            mods: Vec<serde_json::Value>,
        }
        let raw: Raw = serde_json::from_str(text)?;
        let mods = raw
            .mods
            .into_iter()
            .filter_map(|v| match serde_json::from_value::<CatalogueMod>(v) {
                Ok(m) if super::catalog::is_safe_id(&m.id) => Some(m),
                Ok(m) => {
                    tracing::warn!("catalogue: invalid mod id {:?}, left out", m.id);
                    None
                }
                Err(e) => {
                    tracing::warn!("catalogue: a mod left out: {e}");
                    None
                }
            })
            .collect();
        Ok(Catalogue { mods })
    }
}

impl CatalogueMod {
    /// The version to offer: the newest one that isn't blocked and is a semantic version,
    /// a prerelease only with `prereleases`.
    #[cfg(test)]
    pub fn choose(&self, prereleases: bool) -> Option<&CatalogueVersion> {
        self.choose_with(prereleases, &|_| false)
    }

    /// [`CatalogueMod::choose`], leaving out what `blocked` says (rollback.json's blocks).
    pub fn choose_with(
        &self,
        prereleases: bool,
        blocked: &dyn Fn(&ModEntry) -> bool,
    ) -> Option<&CatalogueVersion> {
        self.offered(prereleases)
            .filter(|v| !blocked(&v.entry))
            .max_by(|a, b| version(a).cmp(&version(b)))
    }

    /// Whether it has a prerelease that isn't blocked (⋯ → Prereleases is offered then).
    pub fn has_prereleases(&self) -> bool {
        self.offered(true).any(|v| v.entry.prerelease)
    }

    fn offered(&self, prereleases: bool) -> impl Iterator<Item = &CatalogueVersion> {
        self.versions.iter().filter(move |v| {
            let parsed = semver::Version::parse(&v.entry.version);
            // A prerelease by its flag or by its version: either keeps it for those who ask.
            let pre = v.entry.prerelease || parsed.as_ref().is_some_and(|p| p.is_prerelease());
            v.blocked.is_none()
                && v.entry.id == self.id
                && parsed.is_some()
                && (prereleases || !pre)
        })
    }
}

fn version(v: &CatalogueVersion) -> Option<semver::Version> {
    semver::Version::parse(&v.entry.version)
}

/// What to offer for a mod whose installed version is `installed` when the catalogue has
/// `offered`: an update (newer), going back to stable (a prerelease is installed and the
/// player doesn't take prereleases of it any more), or nothing. Versions that aren't
/// semantic versions keep the old rule: any other version is an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    Update,
    ToStable,
}

/// Pure: see [`Offer`].
pub fn offer(installed: &str, offered: &str) -> Option<Offer> {
    match (
        semver::Version::parse(installed),
        semver::Version::parse(offered),
    ) {
        (Some(i), Some(o)) if o > i => Some(Offer::Update),
        (Some(i), Some(o)) if i.is_prerelease() && !o.is_prerelease() && o != i => {
            Some(Offer::ToStable)
        }
        (Some(_), Some(_)) => None,
        _ => (installed != offered).then_some(Offer::Update),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> Catalogue {
        let sha = "a".repeat(64);
        let v = |version: &str, pre: bool, blocked: bool| {
            format!(
                r#"{{"id": "cool", "name": "Cool", "version": "{version}", "file": "Cool.dll",
                    "url": "https://release.echovr.de/launcher/mods/Cool-{version}.dll", "sha256": "{sha}",
                    "prerelease": {pre}, "requires": {{"nevr": ">=4.1.0", "mods": [{{"id": "base", "version": ">=1.0.0"}}]}},
                    "conflicts": ["other"]{}}}"#,
                if blocked {
                    r#", "blocked": {"action": "off", "note": "broken"}"#
                } else {
                    ""
                }
            )
        };
        Catalogue::parse(&format!(
            r#"{{"schema": 1, "mods": [{{"id": "cool", "name": "Cool", "publisher": "dev", "repo": "dev/cool",
                "versions": [{}, {}, {}, {}]}}, {{"id": "../bad", "versions": []}}]}}"#,
            v("1.3.0-beta.1", true, false),
            v("1.2.0", false, true),
            v("1.1.0", false, false),
            v("1.0.0", false, false)
        ))
        .unwrap()
    }

    #[test]
    fn offers_the_newest_unblocked_and_prereleases_only_when_asked() {
        let c = cat();
        assert_eq!(c.mods.len(), 1);
        let m = &c.mods[0];
        assert_eq!(m.choose(false).unwrap().entry.version, "1.1.0");
        assert_eq!(m.choose(true).unwrap().entry.version, "1.3.0-beta.1");
        assert!(m.has_prereleases());
        let v = &m.choose(false).unwrap().entry;
        assert_eq!(v.requires.nevr.as_deref(), Some(">=4.1.0"));
        assert_eq!(v.requires.mods[0].id, "base");
        assert_eq!(v.conflicts, vec!["other".to_string()]);
        assert!(m.choose(true).unwrap().entry.prerelease);
    }

    #[test]
    fn offers() {
        assert_eq!(offer("1.0.0", "1.1.0"), Some(Offer::Update));
        assert_eq!(offer("1.1.0", "1.1.0"), None);
        assert_eq!(offer("1.2.0", "1.1.0"), None);
        assert_eq!(offer("1.3.0-beta.1", "1.2.0"), Some(Offer::ToStable));
        assert_eq!(offer("1.3.0-beta.1", "1.3.0"), Some(Offer::Update));
        assert_eq!(offer("1.3.0-beta.1", "1.3.0-beta.2"), Some(Offer::Update));
        // Not semver: any other version, as before.
        assert_eq!(offer("2024-1", "2024-2"), Some(Offer::Update));
        assert_eq!(offer("x", "x"), None);
    }
}
