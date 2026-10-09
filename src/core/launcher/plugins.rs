//! Launcher plugins: apps inside the launcher, each with its own tab in the rail's lower
//! section. A plugin is a folder `plugins/<id>/` in the launcher's data folder holding its
//! `plugin.json` (its page, see [`super::plugin_page`]) and its settings. They come from
//! the plugins catalogue (`https://release.echovr.de/launcher/plugins.json`), pinned by
//! checksum like mods, or are put there by hand while writing one.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::plugin_page::{self, PluginPage};
use super::plugin_settings::{self, FileFormat, Store};
use super::store::LauncherState;
use super::{catalog, relay};
use crate::core::{download, paths};

pub const CATALOG_URL: &str = "https://release.echovr.de/launcher/plugins.json";
/// A plugin's description in its folder.
pub const DESCRIPTION: &str = "plugin.json";
/// Where a plugin's settings are kept when its description doesn't say.
pub const SETTINGS_FILE: &str = "settings.json";
/// What the launcher notes about a plugin it installed.
const INSTALLED: &str = ".installed.json";

/// The folder plugins are in.
pub fn dir() -> PathBuf {
    paths::data_dir().join("plugins")
}

/// An installed plugin.
#[derive(Debug, Clone)]
pub struct Installed {
    pub page: Arc<PluginPage>,
    pub dir: PathBuf,
    /// Its catalogue version, if the launcher installed it (else it was put there by hand).
    pub from_catalog: Option<String>,
    /// What its description got wrong (left out).
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct InstalledNote {
    version: String,
    sha256: String,
}

/// Pure: a plugin's description as the launcher uses it. Settings without a file of their
/// own are kept in [`SETTINGS_FILE`] (a launcher plugin has no nEVR arguments).
pub fn read_description(text: &str) -> (Option<PluginPage>, Vec<String>) {
    let (page, w) = plugin_page::parse(text);
    let page = page.map(|mut p| {
        if let Some(s) = &mut p.settings {
            if s.store == Store::Args {
                s.store = Store::File {
                    file: SETTINGS_FILE.into(),
                    format: FileFormat::Json,
                    live: true,
                };
            }
        }
        p
    });
    (page, w)
}

/// Reads the plugin in `folder`; none if it has no valid description, or its id isn't the
/// folder's name.
pub fn read_one(folder: &Path) -> Option<Installed> {
    let text = std::fs::read_to_string(folder.join(DESCRIPTION)).ok()?;
    let (page, warnings) = read_description(&text);
    for w in &warnings {
        tracing::warn!("plugin {}: {w}", folder.display());
    }
    let page = page?;
    if folder.file_name().and_then(|n| n.to_str()) != Some(page.id.as_str()) {
        tracing::warn!(
            "plugin {}: its id {:?} isn't its folder's name; left out",
            folder.display(),
            page.id
        );
        return None;
    }
    let from_catalog = std::fs::read_to_string(folder.join(INSTALLED))
        .ok()
        .and_then(|t| serde_json::from_str::<InstalledNote>(&t).ok())
        .map(|n| n.version);
    Some(Installed {
        page: Arc::new(page),
        dir: folder.to_path_buf(),
        from_catalog,
        warnings,
    })
}

/// The plugins in `root`, by name (file I/O: on a worker).
pub fn installed_in(root: &Path) -> Vec<Installed> {
    let mut out: Vec<Installed> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| read_one(&e.path()))
        .collect();
    out.sort_by_key(|p| p.page.name.to_lowercase());
    out
}

pub fn installed() -> Vec<Installed> {
    installed_in(&dir())
}

// ---- settings ----

/// A plugin's settings values, defaults where none is kept.
pub fn settings_values(p: &Installed) -> BTreeMap<String, String> {
    p.page
        .settings
        .as_ref()
        .map(|s| plugin_settings::read_in(&p.dir, s))
        .unwrap_or_default()
}

pub fn write_settings(p: &Installed, changes: &[(String, String)]) -> Result<()> {
    let Some(s) = &p.page.settings else {
        bail!("{} has no settings", p.page.name);
    };
    plugin_settings::write_in(&p.dir, s, changes)
}

// ---- what a plugin page sees of the launcher ----

/// The event builds by their classic lobbies server id, with the catalogue id of the
/// version that is that build.
pub const EVENT_BUILDS: &[(&str, &str)] = &[
    ("halloween2017", "pc-halloween-2017"),
    ("christmas", "pc-christmas-2017"),
    ("halloween", "pc-halloween-2018"),
    ("winter", "pc-christmas-2018"),
    ("summer", "pc-summer-2019"),
];

/// The installed version a page's `play` names: a classic lobbies build id (`halloween`),
/// a catalogue id or an installed version's id.
pub fn version_for(state: &LauncherState, name: &str) -> Option<String> {
    let catalog_id = EVENT_BUILDS
        .iter()
        .find(|(b, _)| *b == name)
        .map_or(name, |(_, c)| *c);
    state
        .versions
        .iter()
        .find(|v| v.id == name || v.catalog_id.as_deref() == Some(catalog_id) || v.id == catalog_id)
        .map(|v| v.id.clone())
}

/// Pure: the `launcher` part of a page's context.
pub fn launcher_context(state: &LauncherState) -> Value {
    let account = state.relay_account.clone().unwrap_or_default();
    let builds: Vec<Value> = EVENT_BUILDS
        .iter()
        .filter_map(|(build, catalog_id)| {
            let v = state
                .versions
                .iter()
                .find(|v| v.catalog_id.as_deref() == Some(*catalog_id))?;
            Some(json!({"id": build, "version": v.id, "name": v.name}))
        })
        .collect();
    json!({
        "relay": {
            "server": if state.relay_server.trim().is_empty() { relay::DEFAULT_SERVER } else { state.relay_server.trim() },
            "name": account.name,
            "password": account.password,
        },
        "event_builds": builds,
    })
}

// ---- the catalogue ----

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct PluginEntry {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub author: String,
    pub version: String,
    /// A zip with `plugin.json` at its root: relative to the download mirrors, or https on
    /// a trusted host.
    pub url: String,
    pub sha256: String,
    pub size: Option<u64>,
    pub homepage: String,
    /// From the dev folder of the dev code in use ([`super::dev_code`]), not published.
    #[serde(skip)]
    pub dev: bool,
}

impl PluginEntry {
    fn validate(&self) -> Result<()> {
        if !catalog::is_safe_id(&self.id) {
            bail!("invalid plugin id {:?}", self.id);
        }
        if self.name.trim().is_empty() {
            bail!("{} has no name", self.id);
        }
        if !self.url.is_empty() {
            if !catalog::is_safe_url(&self.url) {
                bail!("untrusted download for {}: {}", self.id, self.url);
            }
            if self.sha256.len() != 64 || !self.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("{} has no valid sha256", self.id);
            }
        }
        if !self.homepage.is_empty() && !self.homepage.starts_with("https://") {
            bail!("invalid homepage for {}", self.id);
        }
        Ok(())
    }

    pub fn downloadable(&self) -> bool {
        !self.url.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PluginCatalog {
    pub plugins: Vec<PluginEntry>,
    /// The built-in list is shown (the published one couldn't be fetched).
    pub builtin: bool,
}

impl PluginCatalog {
    /// The catalogue in `text`; entries that fail validation (or repeat an id) are left out.
    pub fn parse(text: &str) -> Result<PluginCatalog> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            plugins: Vec<Value>,
        }
        let raw: Raw = serde_json::from_str(text)?;
        let mut seen = std::collections::HashSet::new();
        let mut plugins = Vec::new();
        for (i, value) in raw.plugins.into_iter().enumerate() {
            let entry = serde_json::from_value::<PluginEntry>(value)
                .map_err(anyhow::Error::from)
                .and_then(|m| m.validate().map(|()| m));
            match entry {
                Ok(m) if seen.insert(m.id.clone()) => plugins.push(m),
                Ok(m) => tracing::warn!("plugins catalogue: duplicate id {}, left out", m.id),
                Err(e) => tracing::warn!("plugins catalogue: entry {i} left out: {e:#}"),
            }
        }
        Ok(PluginCatalog {
            plugins,
            builtin: false,
        })
    }

    /// A dev folder's catalogue (of `code`): every entry downloads from that folder (a
    /// relative `url` resolves into it, anything outside is left out) and is marked
    /// [`PluginEntry::dev`].
    pub fn parse_dev(text: &str, code: &str) -> Result<PluginCatalog> {
        let mut c = PluginCatalog::parse(text)?;
        c.plugins.retain_mut(|p| {
            if p.url.is_empty() {
                return true;
            }
            match super::dev_code::resolve(code, &p.url) {
                Some(u) => {
                    p.url = u;
                    true
                }
                None => {
                    tracing::warn!(
                        "dev plugins catalogue: {} doesn't download from its dev folder, left out",
                        p.id
                    );
                    false
                }
            }
        });
        for p in &mut c.plugins {
            p.dev = true;
        }
        Ok(c)
    }

    /// `dev`'s entries over these: a published one with a dev entry's id gives way to it;
    /// the dev entries come first.
    pub fn with_dev(mut self, dev: PluginCatalog) -> PluginCatalog {
        self.plugins
            .retain(|p| !dev.plugins.iter().any(|d| d.id == p.id));
        let mut plugins = dev.plugins;
        plugins.append(&mut self.plugins);
        self.plugins = plugins;
        self
    }

    pub fn builtin() -> PluginCatalog {
        let mut c = PluginCatalog::parse(include_str!("../../../docs/launcher/plugins.json"))
            .unwrap_or_default();
        c.builtin = true;
        c
    }

    fn cache_path() -> PathBuf {
        paths::data_dir().join("plugins.json")
    }

    /// The published catalogue (kept), else the last one kept, else the built-in one.
    pub fn load() -> PluginCatalog {
        let main = PluginCatalog::load_main();
        match super::dev_code::load("plugins")
            .map(|(code, text)| PluginCatalog::parse_dev(&text, &code))
        {
            Some(Ok(d)) => main.with_dev(d),
            Some(Err(e)) => {
                tracing::warn!("dev plugins catalogue unreadable: {e:#}");
                main
            }
            None => main,
        }
    }

    /// The published catalogue (kept), else the last one kept, else the built-in one.
    fn load_main() -> PluginCatalog {
        let fetched = crate::core::http::get_text(CATALOG_URL)
            .and_then(|t| PluginCatalog::parse(&t).map(|c| (c, t)));
        match fetched {
            Ok((c, text)) => {
                let _ = std::fs::create_dir_all(paths::data_dir());
                let _ = std::fs::write(Self::cache_path(), text);
                c
            }
            Err(e) => {
                tracing::info!("plugins catalogue unavailable ({e:#}); using the last one kept");
                std::fs::read_to_string(Self::cache_path())
                    .ok()
                    .and_then(|t| PluginCatalog::parse(&t).ok())
                    .unwrap_or_else(PluginCatalog::builtin)
            }
        }
    }

    pub fn entry(&self, id: &str) -> Option<&PluginEntry> {
        self.plugins.iter().find(|p| p.id == id)
    }
}

/// The catalogue entry that updates `p` (it came from the catalogue in another version).
pub fn update_for<'a>(p: &Installed, c: &'a PluginCatalog) -> Option<&'a PluginEntry> {
    let have = p.from_catalog.as_deref()?;
    c.entry(&p.page.id)
        .filter(|e| e.downloadable() && !e.version.is_empty() && e.version != have)
}

// ---- installing ----

/// Installs (or updates) `e` into `root`: downloads its zip (checked against its
/// checksum), checks its description, then replaces the plugin's folder, keeping its
/// settings files.
pub fn install_into(
    root: &Path,
    e: &PluginEntry,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(download::Progress),
) -> Result<()> {
    if !e.downloadable() {
        bail!("{} can't be downloaded yet.", e.name);
    }
    let zip = download::fetch_pinned(
        &e.url,
        &paths::downloads_dir(),
        &format!("plugin-{}-{}.zip", e.id, e.version),
        &e.sha256,
        cancel,
        on,
    )?;
    let result = unpack(root, e, &zip, cancel);
    let _ = std::fs::remove_file(&zip);
    result
}

/// Puts the plugin in `zip` into `root/<id>` (see [`install_into`]).
pub fn unpack(root: &Path, e: &PluginEntry, zip: &Path, cancel: &AtomicBool) -> Result<()> {
    let staged = root.join(format!(".{}.new", e.id));
    let _ = std::fs::remove_dir_all(&staged);
    let top = crate::core::zip::top_folder(zip)?;
    crate::core::zip::extract_root(zip, top.as_deref(), &staged, cancel)?;
    let text = std::fs::read_to_string(staged.join(DESCRIPTION))
        .with_context(|| format!("{} has no {DESCRIPTION}", e.name));
    let check = text.and_then(|t| match read_description(&t).0 {
        Some(p) if p.id == e.id => Ok(p),
        Some(p) => bail!("the download is plugin {:?}, not {:?}", p.id, e.id),
        None => bail!("{}'s description can't be read", e.name),
    });
    let page = match check {
        Ok(p) => p,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(err);
        }
    };
    let dest = root.join(&e.id);
    // Its settings stay.
    if let Some(s) = &page.settings {
        for f in plugin_settings::store_files(s) {
            let old = dest.join(&f);
            if old.is_file() && !staged.join(&f).exists() {
                std::fs::copy(&old, staged.join(&f))?;
            }
        }
    }
    let note = InstalledNote {
        version: e.version.clone(),
        sha256: e.sha256.clone(),
    };
    std::fs::write(staged.join(INSTALLED), serde_json::to_string(&note)?)?;
    if dest.exists() {
        std::fs::remove_dir_all(&dest).with_context(|| format!("remove {}", dest.display()))?;
    }
    std::fs::rename(&staged, &dest).with_context(|| format!("move {} into place", e.name))?;
    tracing::info!("plugin {} {} installed", e.id, e.version);
    Ok(())
}

pub fn install(
    e: &PluginEntry,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(download::Progress),
) -> Result<()> {
    install_into(&dir(), e, cancel, on)
}

/// Removes plugin `id` and its settings.
pub fn remove_from(root: &Path, id: &str) -> Result<()> {
    if !catalog::is_safe_id(id) {
        bail!("invalid plugin id {id:?}");
    }
    let d = root.join(id);
    if d.exists() {
        std::fs::remove_dir_all(&d).with_context(|| format!("remove {}", d.display()))?;
    }
    Ok(())
}

pub fn remove(id: &str) -> Result<()> {
    remove_from(&dir(), id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_folder_plugins_go_over_published_ones() {
        const CODE: &str = "abcdefghijklmnopqrstuvwxyz";
        let sha = "b".repeat(64);
        let dev = format!(
            r#"{{"plugins": [
                {{"id": "event-lobbies", "name": "Event lobbies (dev)", "version": "0.2.0",
                  "url": "files/event-lobbies.zip", "sha256": "{sha}"}},
                {{"id": "outside", "name": "Outside", "url": "https://release.echovr.de/launcher/plugins/x.zip", "sha256": "{sha}"}}
            ]}}"#
        );
        let d = PluginCatalog::parse_dev(&dev, CODE).unwrap();
        assert_eq!(d.plugins.len(), 1);
        assert!(d.plugins[0].dev);
        assert_eq!(
            d.plugins[0].url,
            format!("https://release.echovr.de/launcher/dev/{CODE}/files/event-lobbies.zip")
        );
        let both = PluginCatalog::builtin().with_dev(d);
        assert_eq!(both.entry("event-lobbies").unwrap().version, "0.2.0");
        assert_eq!(
            both.plugins
                .iter()
                .filter(|p| p.id == "event-lobbies")
                .count(),
            1
        );
    }
    use crate::core::launcher::store::{InstalledVersion, RelayAccount};

    const DESC: &str = r#"{"schema": 1, "id": "event-lobbies", "name": "Event lobbies",
        "settings": {"sections": [{"title": "Account", "fields": [{"key": "name", "type": "text", "label": "Name"}]}]},
        "page": {"columns": [[{"title": "X", "blocks": [{"text": "hi"}]}]]}}"#;

    fn zip_with(files: &[(&str, &str)]) -> tempfile::NamedTempFile {
        let f = tempfile::NamedTempFile::new().unwrap();
        let mut z = ::zip::ZipWriter::new(std::fs::File::create(f.path()).unwrap());
        for (name, text) in files {
            z.start_file(*name, ::zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut z, text.as_bytes()).unwrap();
        }
        z.finish().unwrap();
        f
    }

    fn entry(id: &str, version: &str) -> PluginEntry {
        PluginEntry {
            id: id.into(),
            name: "Event lobbies".into(),
            version: version.into(),
            ..Default::default()
        }
    }

    #[test]
    fn installs_keeps_settings_on_update_and_removes() {
        let root = tempfile::tempdir().unwrap();
        let no = AtomicBool::new(false);
        let z = zip_with(&[("event-lobbies/plugin.json", DESC)]);
        unpack(root.path(), &entry("event-lobbies", "0.1.0"), z.path(), &no).unwrap();
        let all = installed_in(root.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].from_catalog.as_deref(), Some("0.1.0"));
        // Its settings are kept in settings.json in its folder.
        write_settings(&all[0], &[("name".into(), "Alice".into())]).unwrap();
        assert_eq!(settings_values(&all[0])["name"], "Alice");

        let z2 = zip_with(&[("plugin.json", DESC)]);
        unpack(
            root.path(),
            &entry("event-lobbies", "0.2.0"),
            z2.path(),
            &no,
        )
        .unwrap();
        let all = installed_in(root.path());
        assert_eq!(all[0].from_catalog.as_deref(), Some("0.2.0"));
        assert_eq!(settings_values(&all[0])["name"], "Alice");

        let catalog = PluginCatalog {
            plugins: vec![PluginEntry {
                url: "plugins/x.zip".into(),
                ..entry("event-lobbies", "0.3.0")
            }],
            builtin: false,
        };
        assert_eq!(
            update_for(&all[0], &catalog).map(|e| e.version.as_str()),
            Some("0.3.0")
        );

        remove_from(root.path(), "event-lobbies").unwrap();
        assert!(installed_in(root.path()).is_empty());
    }

    #[test]
    fn refuses_a_download_that_is_another_plugin() {
        let root = tempfile::tempdir().unwrap();
        let z = zip_with(&[("plugin.json", DESC)]);
        let err = unpack(
            root.path(),
            &entry("other", "1"),
            z.path(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not \"other\""), "{err}");
        assert!(installed_in(root.path()).is_empty());
    }

    #[test]
    fn a_folder_must_be_named_after_its_plugin() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("wrong")).unwrap();
        std::fs::write(root.path().join("wrong/plugin.json"), DESC).unwrap();
        assert!(installed_in(root.path()).is_empty());
    }

    #[test]
    fn the_launcher_context_lists_installed_event_builds() {
        let mut s = LauncherState {
            relay_account: Some(RelayAccount {
                name: "Alice".into(),
                password: "pw".into(),
            }),
            ..Default::default()
        };
        s.versions.push(InstalledVersion {
            id: "pc-halloween-2018".into(),
            name: "Halloween 2018".into(),
            catalog_id: Some("pc-halloween-2018".into()),
            publisher_lock: Some("rad15_halloween".into()),
            ..Default::default()
        });
        let c = launcher_context(&s);
        assert_eq!(c["relay"]["name"], "Alice");
        assert_eq!(c["relay"]["server"], relay::DEFAULT_SERVER);
        assert_eq!(
            c["event_builds"],
            json!([{"id": "halloween", "version": "pc-halloween-2018", "name": "Halloween 2018"}])
        );
        assert_eq!(
            version_for(&s, "halloween").as_deref(),
            Some("pc-halloween-2018")
        );
        assert_eq!(version_for(&s, "summer"), None);
    }

    /// Installs every downloadable plugin of the published catalogue into a temporary
    /// folder, as Get does.
    #[test]
    #[ignore = "network"]
    fn installs_the_published_plugins() {
        let c = PluginCatalog::load();
        assert!(!c.builtin, "the published catalogue couldn't be fetched");
        let root = tempfile::tempdir().unwrap();
        for e in c.plugins.iter().filter(|e| e.downloadable()) {
            install_into(root.path(), e, &AtomicBool::new(false), &mut |_| {}).unwrap();
        }
        let all = installed_in(root.path());
        assert!(all.iter().any(|p| p.page.id == "event-lobbies"), "{all:?}");
        assert!(
            all.iter().all(|p| p.warnings.is_empty()),
            "{:?}",
            all.iter().map(|p| &p.warnings).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_builtin_catalogue_parses() {
        let c = PluginCatalog::builtin();
        assert!(c.entry("event-lobbies").is_some());
    }
}
