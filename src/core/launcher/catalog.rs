//! The versions catalogue (`versions.json` on files.echovr.de).
//!
//! ```json
//! { "schema": 1,
//!   "versions": [
//!     { "id": "pc-34.4.631547.1", "name": "Echo VR 34.4 (PC)", "channel": "stable",
//!       "platform": "pc", "url": "ready-at-dawn-echo-arena.zip", "size": 4270000000,
//!       "sha256": null, "update_manifest": "https://files.echovr.de/updates/update.manifest",
//!       "notes": "The last official build, with community patches.",
//!       "hosted": "live", "summary": "Where everyone plays" } ] }
//! ```
//!
//! `hosted` marks the builds the community's main servers run: `live` (the current one)
//! or `event`. The Install page lists them first, tagged, above the rest. `summary` is
//! one short line for that list. Both are optional.
//!
//! `version` and `released` (YYYY-MM-DD) describe an older build in that list; `exe` is
//! its executable's file name when it isn't `echovr.exe` (the 2017 builds start
//! `EchoArena.exe`). All three are optional.
//!
//! `url` is either relative -- served by the fastest download mirror -- or an absolute
//! https URL on a trusted host; empty while a listed build can't be downloaded yet.
//! Everything is validated before it is used for downloads or folder names; an entry that
//! fails is left out, the rest of the list stays.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use super::quest;
use crate::core::{pc_update, quest_update};

pub const CATALOG_URL: &str = "https://files.echovr.de/launcher/versions.json";
/// The live PC build's file manifest (its zip is `pc.zip`, the old name kept as a link).
pub const PC_FILES_MANIFEST: &str = "https://files.echovr.de/pc.zip.manifest";
const TRUSTED_HOSTS: [&str; 2] = ["files.echovr.de", "evr.echo.taxi"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    #[default]
    Pc,
    Quest,
}

/// Which builds the main servers run (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hosted {
    Live,
    Event,
    /// A value this version of the launcher doesn't know: shown as not hosted.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VersionEntry {
    pub id: String,
    pub name: String,
    pub channel: String,
    pub platform: Platform,
    /// PC: the game zip. Quest: the APK.
    pub url: String,
    /// Quest: `_data.zip`.
    pub data_url: Option<String>,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub update_manifest: Option<String>,
    /// PC: the build's file manifest, a checksum for every file in its zip (installs and
    /// reinstalls check the game files against it).
    pub files_manifest: Option<String>,
    pub notes: String,
    pub hosted: Option<Hosted>,
    pub summary: String,
    /// The build's own version number, e.g. "16.0.253636.0".
    pub version: String,
    /// When it came out (YYYY-MM-DD).
    pub released: String,
    /// The executable's file name when it isn't `echovr.exe`.
    pub exe: Option<String>,
    /// An event build, played on the classic lobbies (EchoRelay) server as this build:
    /// it gets EchoRelay's patch and a config naming the server and your account.
    pub publisher_lock: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Catalog {
    pub schema: u32,
    pub versions: Vec<VersionEntry>,
    /// Not in the file: true when the built-in fallback is shown.
    #[serde(skip)]
    pub builtin: bool,
}

/// Ids become folder names: lowercase letters, digits, `.`, `-`, `_`.
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

/// A download reference: relative to the mirror (safe path), or https on a trusted host.
pub fn is_safe_url(url: &str) -> bool {
    if !url.contains("://") {
        return crate::core::manifest::is_safe_path(url);
    }
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https" && u.host_str().is_some_and(|h| TRUSTED_HOSTS.contains(&h))
    })
}

impl VersionEntry {
    /// Run by the main servers (live or an event).
    pub fn is_hosted(&self) -> bool {
        matches!(self.hosted, Some(Hosted::Live | Hosted::Event))
    }

    /// Whether `url` is served by the mirrors (relative) rather than absolute.
    pub fn uses_mirror(&self) -> bool {
        !self.url.contains("://")
    }

    /// Whether it can be downloaded (listed builds may not be on the servers yet).
    pub fn downloadable(&self) -> bool {
        !self.url.is_empty()
    }

    /// The second line of its row in VERSIONS: "v1.76 · 2017-10-19", or empty.
    pub fn version_line(&self) -> String {
        let version = (!self.version.is_empty()).then(|| format!("v{}", self.version));
        let released = (!self.released.is_empty()).then(|| self.released.clone());
        [version, released]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("  ·  ")
    }

    fn validate(&self) -> Result<()> {
        if !is_safe_id(&self.id) {
            bail!("invalid version id {:?}", self.id);
        }
        if self.downloadable() && !is_safe_url(&self.url) {
            bail!("untrusted download for {}: {}", self.id, self.url);
        }
        if let Some(exe) = &self.exe {
            let plain = !exe.is_empty()
                && exe.len() <= 64
                && !exe.starts_with('.')
                && exe.to_ascii_lowercase().ends_with(".exe")
                && exe
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
            if !plain {
                bail!("invalid executable name for {}: {exe:?}", self.id);
            }
        }
        if let Some(d) = &self.data_url {
            if !is_safe_url(d) {
                bail!("untrusted data download for {}: {d}", self.id);
            }
        }
        if let Some(m) = &self.update_manifest {
            if !(m.contains("://") && is_safe_url(m)) {
                bail!("untrusted update manifest for {}: {m}", self.id);
            }
        }
        if let Some(lock) = &self.publisher_lock {
            let plain = !lock.is_empty()
                && lock.len() <= 32
                && lock.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !plain {
                bail!("invalid publisher lock for {}: {lock:?}", self.id);
            }
        }
        if let Some(m) = &self.files_manifest {
            if !(m.contains("://") && is_safe_url(m)) {
                bail!("untrusted file manifest for {}: {m}", self.id);
            }
        }
        if let Some(h) = &self.sha256 {
            if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("invalid sha256 for {}", self.id);
            }
        }
        Ok(())
    }
}

/// An event build's archive on files.echovr.de and what the relay knows it by.
struct Event {
    archive: &'static str,
    size: u64,
    version: &'static str,
    released: &'static str,
    /// The 2017 builds (rad14): `EchoArena.exe`, and the older config keys.
    rad14: bool,
    lock: &'static str,
}

impl Catalog {
    /// The catalogue in `text`. An entry that can't be read (an unknown platform, a
    /// mistake) or fails validation is left out and logged, as is a second entry with an
    /// id already listed; the list fails only when nothing usable is left.
    pub fn parse(text: &str) -> Result<Catalog> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            schema: u32,
            #[serde(default)]
            versions: Vec<serde_json::Value>,
        }
        let raw: Raw = serde_json::from_str(text)?;
        let mut seen = std::collections::HashSet::new();
        let mut versions = Vec::new();
        for (i, value) in raw.versions.into_iter().enumerate() {
            let entry = serde_json::from_value::<VersionEntry>(value)
                .map_err(anyhow::Error::from)
                .and_then(|v| v.validate().map(|()| v));
            match entry {
                Ok(v) if seen.insert(v.id.clone()) => versions.push(v),
                Ok(v) => tracing::warn!("versions catalogue: duplicate id {}, left out", v.id),
                Err(e) => tracing::warn!("versions catalogue: entry {i} left out: {e:#}"),
            }
        }
        if versions.is_empty() {
            bail!("the versions catalogue lists no usable version");
        }
        Ok(Catalog {
            schema: raw.schema,
            versions,
            builtin: false,
        })
    }

    pub fn fetch() -> Result<Catalog> {
        Catalog::parse(&crate::core::http::get_text(CATALOG_URL)?)
    }

    /// What the installer has always offered, and the event builds to come, for when the
    /// catalogue is unreachable or not published yet.
    pub fn builtin() -> Catalog {
        // The event builds, played on the classic lobbies relay. Halloween 2017's archive
        // comes ready for it; the others get EchoRelay's patch when installed.
        let event = |id: &str, name: &str, ev: Event| VersionEntry {
            id: id.into(),
            name: name.into(),
            channel: "event".into(),
            platform: Platform::Pc,
            url: format!("https://files.echovr.de/{}", ev.archive),
            files_manifest: Some(format!("https://files.echovr.de/{}.manifest", ev.archive)),
            size: Some(ev.size),
            notes: format!("The {name} event build, on the community's classic lobbies server."),
            hosted: Some(Hosted::Event),
            summary: "Classic lobby on the community relay".into(),
            version: ev.version.into(),
            released: ev.released.into(),
            exe: ev.rad14.then(|| "EchoArena.exe".into()),
            publisher_lock: Some(ev.lock.into()),
            ..Default::default()
        };
        Catalog {
            schema: 1,
            builtin: true,
            versions: vec![
                VersionEntry {
                    id: "pc-latest".into(),
                    name: "Echo VR (PC, latest)".into(),
                    channel: "stable".into(),
                    platform: Platform::Pc,
                    url: "ready-at-dawn-echo-arena.zip".into(),
                    update_manifest: Some(pc_update::PC_MANIFEST_URL.into()),
                    files_manifest: Some(PC_FILES_MANIFEST.into()),
                    notes: "The current PC client with community updates.".into(),
                    hosted: Some(Hosted::Live),
                    summary: "Played on the community servers".into(),
                    ..Default::default()
                },
                event(
                    "pc-halloween-2017",
                    "Halloween 2017",
                    Event {
                        archive: "halloween2017.zip",
                        size: 1_337_594_928,
                        version: "1.76",
                        released: "2017-10-19",
                        rad14: true,
                        lock: "release4_5",
                    },
                ),
                event(
                    "pc-christmas-2017",
                    "Christmas 2017",
                    // EchoRelay knows this build by the live lock, not its own (ea_rel6_0).
                    Event {
                        archive: "echo-vr-pc-6.0.zip",
                        size: 1_444_472_325,
                        version: "6",
                        released: "2017-12-19",
                        rad14: true,
                        lock: "rad15_live",
                    },
                ),
                event(
                    "pc-halloween-2018",
                    "Halloween 2018",
                    Event {
                        archive: "echo-vr-pc-16.0.253636.0.zip",
                        size: 3_168_109_836,
                        version: "16.0.253636.0",
                        released: "2018-10-17",
                        rad14: false,
                        lock: "rad15_halloween",
                    },
                ),
                event(
                    "pc-christmas-2018",
                    "Christmas 2018",
                    Event {
                        archive: "echo-vr-pc-18.3.268902.0.zip",
                        size: 3_343_976_938,
                        version: "18.3.268902.0",
                        released: "2018-12-17",
                        rad14: false,
                        lock: "rad15_winter",
                    },
                ),
                event(
                    "pc-summer-2019",
                    "Summer 2019",
                    Event {
                        archive: "echo-vr-pc-23.2.340872.0.zip",
                        size: 3_341_195_444,
                        version: "23.2.340872.0",
                        released: "2019-07-24",
                        rad14: false,
                        lock: "rad15_summer",
                    },
                ),
                VersionEntry {
                    id: "quest-latest".into(),
                    name: "Echo VR (Quest, latest)".into(),
                    channel: "stable".into(),
                    platform: Platform::Quest,
                    url: quest::FALLBACK_APK.into(),
                    data_url: Some("_data.zip".into()),
                    update_manifest: Some(quest_update::QUEST_MANIFEST_URL.into()),
                    notes: "Installed over USB from the Quest side of Play.".into(),
                    ..Default::default()
                },
            ],
        }
    }

    /// The published catalogue, or the built-in one.
    pub fn load() -> Catalog {
        Catalog::fetch().unwrap_or_else(|e| {
            tracing::info!("versions catalogue unavailable ({e:#}); using the built-in list");
            Catalog::builtin()
        })
    }

    pub fn pc(&self) -> impl Iterator<Item = &VersionEntry> {
        self.versions.iter().filter(|v| v.platform == Platform::Pc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"schema":1,"versions":[
      {"id":"pc-34.4","name":"Echo 34.4","platform":"pc","url":"ready-at-dawn-echo-arena.zip",
       "update_manifest":"https://files.echovr.de/updates/update.manifest"},
      {"id":"pc-beta","name":"Beta","platform":"pc","url":"https://evr.echo.taxi/beta.zip",
       "sha256":"0a7fa5f9cfc173013e152a75fac2ded7ca4f66b8d8530f598c0c2530b5cf0973"}]}"#;

    #[test]
    fn parses_and_classifies() {
        let c = Catalog::parse(GOOD).unwrap();
        assert_eq!(c.versions.len(), 2);
        assert!(c.versions[0].uses_mirror());
        assert!(!c.versions[1].uses_mirror());
        assert_eq!(c.pc().count(), 2);
    }

    #[test]
    fn reads_hosted_and_summary() {
        let c = Catalog::parse(
            r#"{"versions":[
              {"id":"a","url":"a.zip","hosted":"live","summary":"Where everyone plays"},
              {"id":"b","url":"b.zip","hosted":"event"},
              {"id":"c","url":"c.zip","hosted":"weekend"},
              {"id":"d","url":"d.zip"}]}"#,
        )
        .unwrap();
        let hosted: Vec<_> = c.versions.iter().map(|v| v.hosted).collect();
        assert_eq!(
            hosted,
            [
                Some(Hosted::Live),
                Some(Hosted::Event),
                Some(Hosted::Unknown),
                None
            ]
        );
        assert_eq!(c.versions[0].summary, "Where everyone plays");
        let shown: Vec<_> = c.versions.iter().map(VersionEntry::is_hosted).collect();
        assert_eq!(shown, [true, true, false, false]);
    }

    #[test]
    fn rejects_untrusted_entries() {
        for bad in [
            r#"{"versions":[{"id":"../x","url":"a.zip"}]}"#,
            r#"{"versions":[{"id":"Upper","url":"a.zip"}]}"#,
            r#"{"versions":[{"id":"a","url":"https://evil.example/a.zip"}]}"#,
            r#"{"versions":[{"id":"a","url":"http://files.echovr.de/a.zip"}]}"#,
            r#"{"versions":[{"id":"a","url":"../a.zip"}]}"#,
            r#"{"versions":[{"id":"a","url":"a.zip","update_manifest":"update.manifest"}]}"#,
            r#"{"versions":[{"id":"a","url":"a.zip","sha256":"xyz"}]}"#,
            r#"{"versions":[{"id":"a","url":"a.zip","exe":"../evil.exe"}]}"#,
            r#"{"versions":[{"id":"a","url":"a.zip","exe":"run.bat"}]}"#,
            r#"{"versions":[{"id":"a","platform":"switch","url":"a.zip"}]}"#,
            r#"{"versions":[]}"#,
            r#"not json"#,
        ] {
            assert!(Catalog::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn leaves_out_only_the_bad_entries() {
        let c = Catalog::parse(
            r#"{"schema":1,"versions":[
              {"id":"good","url":"a.zip"},
              {"id":"evil","url":"https://evil.example/a.zip"},
              {"id":"vr","platform":"switch","url":"b.zip"},
              {"id":"good","url":"c.zip"},
              {"id":"soon","name":"Halloween 2017","version":"1.76","released":"2017-10-19",
               "exe":"EchoArena.exe"}]}"#,
        )
        .unwrap();
        let ids: Vec<_> = c.versions.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["good", "soon"]);
        assert_eq!(c.versions[0].url, "a.zip");
        let soon = &c.versions[1];
        assert!(!soon.downloadable());
        assert_eq!(soon.exe.as_deref(), Some("EchoArena.exe"));
        assert_eq!(soon.version_line(), "v1.76  ·  2017-10-19");
        assert_eq!(c.versions[0].version_line(), "");
    }

    #[test]
    fn draft_matches_the_builtin_list() {
        let draft = include_str!("../../../docs/launcher/versions.json");
        let raw: serde_json::Value = serde_json::from_str(draft).unwrap();
        let listed = raw["versions"].as_array().unwrap().len();
        let c = Catalog::parse(draft).unwrap();
        assert_eq!(c.versions.len(), listed, "every draft entry is usable");
        let ids = |c: &Catalog| c.versions.iter().map(|v| v.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&c), ids(&Catalog::builtin()));
    }

    #[test]
    fn builtin_is_valid() {
        let b = Catalog::builtin();
        for v in &b.versions {
            v.validate().unwrap();
        }
        assert!(b.builtin);
        // Every PC build can be downloaded: the live one, then the five event builds,
        // each with its file checksums and played on the relay.
        assert!(b
            .pc()
            .all(|v| v.downloadable() && v.files_manifest.is_some()));
        let events: Vec<_> = b.pc().filter(|v| v.publisher_lock.is_some()).collect();
        assert_eq!(events.len(), 5);
        assert!(events
            .iter()
            .all(|v| v.hosted == Some(Hosted::Event) && v.update_manifest.is_none()));
        assert!(b.pc().next().is_some_and(|v| v.publisher_lock.is_none()));
    }
}
