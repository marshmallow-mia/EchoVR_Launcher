//! Mods: the plugins nEVR runtime loads in a live PC version's `bin/win10` (see
//! [`super::nevr`]).
//!
//! nEVR is the game's crash reporter, `BugSplat64.dll`; it loads the plugins in
//! `plugins/` that `_local/config.yaml` lists, and the launcher writes that list before
//! every start ([`prepare`]): the DLLs in `plugins/` (the community update's), with its own
//! choices applied. The update owns its plugins and `asset_patches/manifest.json` (Update
//! puts them back, Verify reports a change), so the launcher never edits them. It writes:
//! - `_local/launcher-mods.json`, its choices: mods on or off, a plugin on or off, a
//!   plugin's arguments, and the plugins it added (each with its checksum, checked before
//!   the plugin is listed);
//! - `asset_patches/manifest.local.json`: asset patches on or off;
//! - the plugin files it installed itself (from the mods catalogue, or from disk).
//!
//! What nEVR did with them at the last start is in its log.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::catalog;
use super::nevr::{self, PluginLine};
use super::plugin_settings;
use super::store::InstalledVersion;
use super::versions::Step;
use crate::core::{download, paths};

pub const CATALOG_URL: &str = "https://release.echovr.de/launcher/mods.json";
/// The loader's slot: the crash reporter the game imports.
pub const SLOT: &str = "BugSplat64.dll";
/// The game's own crash reporter (the live build's, from its file manifest).
const STOCK_SLOT_SHA256: &str = "618e34d3bfde69846c0202d42951366bff5e03cd2885b6825f55e09fb2cf160b";
/// The launcher's choices, in the game's `_local` beside `config.yaml`.
pub const CHOICES: &str = "launcher-mods.json";
const PLUGINS: &str = "plugins";
/// Where plugins log (beside the exe), each in a folder of its own.
const PLUGIN_LOGS: &str = "plugin_logs";
/// In `plugins/`, but nothing nEVR loads: EchoRelay's patch, which the update still ships
/// (nEVR does what it did).
const NOT_PLUGINS: [&str; 1] = ["dbgcore.dll"];
const ASSETS: &str = "asset_patches/manifest.json";
const ASSETS_OVERLAY: &str = "asset_patches/manifest.local.json";
/// The plugin whose patches `asset_patches` lists.
pub const ASSET_PLUGIN: &str = "NvrAssetPatches.dll";

/// What sits in the loader's slot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Loader {
    /// The game's own crash reporter (or nothing): no mods.
    #[default]
    None,
    /// nEVR runtime, at this version (empty when it doesn't say).
    Nevr { version: String },
    /// A DLL the launcher doesn't know.
    Unknown,
}

impl Loader {
    /// Whether the launcher can change the mods (nEVR is there).
    pub fn editable(&self) -> bool {
        matches!(self, Loader::Nevr { .. })
    }
}

/// Pure: what the slot's file (`slot`, its checksum `slot_sha`) is.
pub fn classify(slot: Option<&[u8]>, slot_sha: Option<&str>) -> Loader {
    let Some(bytes) = slot else {
        return Loader::None;
    };
    if find(bytes, nevr::MARKER).is_some() {
        return Loader::Nevr {
            version: nevr::version_in(bytes).unwrap_or_default(),
        };
    }
    if slot_sha.is_some_and(|s| s.eq_ignore_ascii_case(STOCK_SLOT_SHA256)) {
        Loader::None
    } else {
        Loader::Unknown
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Whether nEVR is in the slot of the game in `bin`.
pub fn nevr_in(bin: &Path) -> bool {
    std::fs::read(bin.join(SLOT)).is_ok_and(|b| find(&b, nevr::MARKER).is_some())
}

// ---- plugin entries ----

/// An entry of a `plugins` list: a file name, or an object.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entry {
    pub file: String,
    pub name: String,
    pub enabled: bool,
    pub args: Map<String, Value>,
    pub sha256: Option<String>,
    /// What the launcher noted on an entry it added.
    pub launcher: Option<Added>,
    /// The game needs it (the catalogue says so): always on.
    pub required: bool,
}

/// Where an entry the launcher added came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Added {
    /// The mods catalogue's id, when installed from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// Added from disk.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
}

impl Entry {
    fn parse(v: &Value) -> Option<Entry> {
        match v {
            Value::String(f) => Some(Entry {
                file: f.clone(),
                enabled: true,
                ..Default::default()
            }),
            Value::Object(o) => {
                let file = o.get("file")?.as_str()?.to_string();
                Some(Entry {
                    name: str_of(o, "name"),
                    enabled: o.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                    args: o
                        .get("args")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default(),
                    sha256: o.get("sha256").and_then(Value::as_str).map(str::to_string),
                    launcher: o
                        .get("launcher")
                        .and_then(|l| serde_json::from_value(l.clone()).ok()),
                    required: false,
                    file,
                })
            }
            _ => None,
        }
    }
}

fn entries(v: Option<&Value>) -> Option<Vec<Entry>> {
    Some(v?.as_array()?.iter().filter_map(Entry::parse).collect())
}

fn str_of(o: &Map<String, Value>, key: &str) -> String {
    o.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---- launcher-mods.json (the launcher's choices) ----

/// The launcher's overlay, kept as JSON so keys this version doesn't know survive.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Overlay(Map<String, Value>);

impl Overlay {
    pub fn parse(text: &str) -> Overlay {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(o)) => Overlay(o),
            _ => Overlay::default(),
        }
    }

    pub fn read(path: &Path) -> Overlay {
        std::fs::read_to_string(path)
            .map(|t| Overlay::parse(&t))
            .unwrap_or_default()
    }

    /// Replaces the file at `path` (atomically).
    pub fn write(&self, path: &Path) -> Result<()> {
        let mut o = self.0.clone();
        o.insert("version".into(), 1.into());
        write_json(path, &Value::Object(o))
    }

    /// Mods are on (not "start without mods").
    pub fn enabled(&self) -> bool {
        self.0
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.0.insert("enabled".into(), on.into());
    }

    /// The overrides of `file` (matched as the loader matches, ignoring case).
    fn overrides(&self, file: &str) -> Option<&Map<String, Value>> {
        self.0
            .get("overrides")?
            .as_object()?
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(file))
            .and_then(|(_, v)| v.as_object())
    }

    fn overrides_mut(&mut self, file: &str) -> &mut Map<String, Value> {
        let all = self
            .0
            .entry("overrides")
            .or_insert_with(|| Value::Object(Map::new()));
        if !all.is_object() {
            *all = Value::Object(Map::new());
        }
        let all = all.as_object_mut().expect("an object");
        let key = all
            .keys()
            .find(|k| k.eq_ignore_ascii_case(file))
            .cloned()
            .unwrap_or_else(|| file.to_string());
        let o = all.entry(key).or_insert_with(|| Value::Object(Map::new()));
        if !o.is_object() {
            *o = Value::Object(Map::new());
        }
        o.as_object_mut().expect("an object")
    }

    /// The plugins the launcher added, in order.
    pub fn added(&self) -> Vec<Entry> {
        entries(self.0.get("add")).unwrap_or_default()
    }

    fn added_mut(&mut self, file: &str) -> Option<&mut Map<String, Value>> {
        self.0
            .get_mut("add")?
            .as_array_mut()?
            .iter_mut()
            .filter_map(Value::as_object_mut)
            .find(|o| {
                o.get("file")
                    .and_then(Value::as_str)
                    .is_some_and(|f| f.eq_ignore_ascii_case(file))
            })
    }

    /// Adds `e` (or replaces the entry of its file).
    pub fn add(&mut self, e: &Entry) {
        let mut o = Map::new();
        o.insert("file".into(), e.file.clone().into());
        if !e.name.is_empty() {
            o.insert("name".into(), e.name.clone().into());
        }
        o.insert("enabled".into(), e.enabled.into());
        if !e.args.is_empty() {
            o.insert("args".into(), Value::Object(e.args.clone()));
        }
        if let Some(sha) = &e.sha256 {
            o.insert("sha256".into(), sha.clone().into());
        }
        if let Some(l) = &e.launcher {
            o.insert(
                "launcher".into(),
                serde_json::to_value(l).unwrap_or_default(),
            );
        }
        match self.added_mut(&e.file) {
            Some(existing) => *existing = o,
            None => {
                let list = self
                    .0
                    .entry("add")
                    .or_insert_with(|| Value::Array(Vec::new()));
                if !list.is_array() {
                    *list = Value::Array(Vec::new());
                }
                list.as_array_mut()
                    .expect("an array")
                    .push(Value::Object(o));
            }
        }
    }

    /// Takes `file` out of the added plugins; whether it was there.
    pub fn remove_added(&mut self, file: &str) -> bool {
        let Some(list) = self.0.get_mut("add").and_then(Value::as_array_mut) else {
            return false;
        };
        let before = list.len();
        list.retain(|v| {
            !v.get("file")
                .and_then(Value::as_str)
                .is_some_and(|f| f.eq_ignore_ascii_case(file))
        });
        before != list.len()
    }

    /// Drops what only repeats `shipped` (the update's plugins, on) or a default:
    /// an override equal to the shipped entry, mods on. What is left are real choices,
    /// so a later change to the shipped config isn't masked by a stale copy of it.
    pub fn prune(&mut self, shipped: &[Entry]) {
        if self.0.get("enabled").and_then(Value::as_bool) == Some(true) {
            self.0.remove("enabled");
        }
        let Some(all) = self.0.get_mut("overrides").and_then(Value::as_object_mut) else {
            return;
        };
        all.retain(|file, o| {
            let Some(o) = o.as_object_mut() else {
                return false;
            };
            if let Some(e) = shipped.iter().find(|e| e.file.eq_ignore_ascii_case(file)) {
                let off = o.get("enabled").and_then(Value::as_bool);
                if off == Some(e.enabled) || e.required {
                    o.remove("enabled");
                }
                if o.get("args").is_some_and(|a| same_args(a, &e.args)) {
                    o.remove("args");
                }
            }
            !o.is_empty()
        });
        if all.is_empty() {
            self.0.remove("overrides");
        }
    }

    /// Nothing but the version is left.
    pub fn is_empty(&self) -> bool {
        self.0.keys().all(|k| k == "version")
    }

    /// Turns `file` on or off: on its entry when the launcher added it, else as an
    /// override.
    pub fn set_plugin_enabled(&mut self, file: &str, on: bool) {
        match self.added_mut(file) {
            Some(o) => {
                o.insert("enabled".into(), on.into());
            }
            None => {
                self.overrides_mut(file).insert("enabled".into(), on.into());
            }
        }
    }

    /// Sets `file`'s arguments (each value a string, as the loader hands them on).
    pub fn set_args(&mut self, file: &str, args: &BTreeMap<String, String>) {
        let args: Map<String, Value> = args
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        match self.added_mut(file) {
            Some(o) => {
                o.insert("args".into(), Value::Object(args));
            }
            None => {
                self.overrides_mut(file)
                    .insert("args".into(), Value::Object(args));
            }
        }
    }

    /// Puts `file`'s arguments back to its defaults: an added plugin gets `defaults` (the
    /// catalogue's), a shipped one loses its override (its defaults come from the
    /// catalogue again).
    pub fn reset_args(&mut self, file: &str, defaults: &Map<String, Value>) {
        match self.added_mut(file) {
            Some(o) if defaults.is_empty() => {
                o.remove("args");
            }
            Some(o) => {
                o.insert("args".into(), Value::Object(defaults.clone()));
            }
            None => {
                self.overrides_mut(file).remove("args");
            }
        }
    }

    /// `list` as the loader applies the overlay to it: overrides, then the added
    /// plugins appended (one already listed acts as an override).
    pub fn apply(&self, list: Vec<Entry>) -> Vec<Entry> {
        let mut out: Vec<Entry> = list
            .into_iter()
            .map(|mut e| {
                if let Some(o) = self.overrides(&e.file) {
                    override_entry(&mut e, o);
                }
                e
            })
            .collect();
        for a in self.added() {
            match out
                .iter_mut()
                .find(|e| e.file.eq_ignore_ascii_case(&a.file))
            {
                Some(e) => {
                    e.enabled = a.enabled;
                    if !a.args.is_empty() {
                        e.args = a.args;
                    }
                    if a.sha256.is_some() {
                        e.sha256 = a.sha256;
                    }
                    e.launcher = a.launcher;
                }
                None => out.push(a),
            }
        }
        out
    }
}

fn override_entry(e: &mut Entry, o: &Map<String, Value>) {
    if let Some(on) = o.get("enabled").and_then(Value::as_bool) {
        e.enabled = on;
    }
    if let Some(args) = o.get("args").and_then(Value::as_object) {
        e.args = args.clone();
    }
    if let Some(sha) = o.get("sha256").and_then(Value::as_str) {
        e.sha256 = Some(sha.to_string());
    }
    if let Some(n) = o.get("name").and_then(Value::as_str) {
        e.name = n.to_string();
    }
}

fn write_json(path: &Path, v: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)
        .with_context(|| format!("Couldn't write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't replace {}", path.display())
    })
}

pub use super::nevr::{PluginStatus, Status};

// ---- asset patches ----

/// An asset patch the update ships, and whether it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPatch {
    pub label: String,
    pub enabled: bool,
    /// The game needs it (`"required": true` in the update's manifest): always on, and
    /// NvrAssetPatches loads it whatever the local choices say.
    pub required: bool,
}

/// Pure: the patches of `manifest` (the update's) with `overlay`'s choices, and whether
/// asset patches are on at all.
pub fn asset_patches(manifest: &str, overlay: &str) -> (bool, Vec<AssetPatch>) {
    let m: Value = serde_json::from_str(manifest).unwrap_or_default();
    let o: Value = serde_json::from_str(overlay).unwrap_or_default();
    let all_on = o.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let patches = m
        .get("patches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let label = p.get("label")?.as_str()?.to_string();
            let shipped = p.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let required = p.get("required").and_then(Value::as_bool).unwrap_or(false);
            let chosen = o
                .get("patches")
                .and_then(|ps| ps.get(&label))
                .and_then(|c| c.get("enabled"))
                .and_then(Value::as_bool);
            Some(AssetPatch {
                enabled: required || chosen.unwrap_or(shipped),
                required,
                label,
            })
        })
        .collect();
    (all_on, patches)
}

/// Pure: whether two args objects say the same, compared as the loader hands them on
/// (every value as a string).
fn same_args(a: &Value, b: &Map<String, Value>) -> bool {
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    a.as_object().is_some_and(|a| {
        a.len() == b.len()
            && a.iter()
                .all(|(k, v)| b.get(k).is_some_and(|w| text(v) == text(w)))
    })
}

/// Pure: `overlay` (the asset patches' overlay) with `label` turned on or off (`None`:
/// every patch), keeping only what differs from `manifest` (the shipped patches).
fn set_asset(manifest: &str, overlay: &str, label: Option<&str>, on: bool) -> Value {
    let mut o = match serde_json::from_str::<Value>(overlay) {
        Ok(Value::Object(o)) => o,
        _ => Map::new(),
    };
    o.insert("version".into(), 1.into());
    match label {
        None => {
            o.insert("enabled".into(), on.into());
        }
        Some(label) => {
            let patches = o
                .entry("patches")
                .or_insert_with(|| Value::Object(Map::new()));
            if !patches.is_object() {
                *patches = Value::Object(Map::new());
            }
            let mut choice = Map::new();
            choice.insert("enabled".into(), on.into());
            patches
                .as_object_mut()
                .expect("an object")
                .insert(label.to_string(), Value::Object(choice));
        }
    }
    // Only real choices stay: all on is the default, a patch set as it ships, and none
    // for a required patch (always on).
    let (_, shipped) = asset_patches(manifest, "");
    if o.get("enabled").and_then(Value::as_bool) == Some(true) {
        o.remove("enabled");
    }
    if let Some(ps) = o.get_mut("patches").and_then(Value::as_object_mut) {
        ps.retain(|label, c| {
            let chosen = c.get("enabled").and_then(Value::as_bool);
            let ships = shipped.iter().find(|a| &a.label == label);
            let required = ships.is_some_and(|a| a.required);
            !required && chosen.is_some() && chosen != ships.map(|a| a.enabled).or(Some(true))
        });
        if ps.is_empty() {
            o.remove("patches");
        }
    }
    Value::Object(o)
}

// ---- the view ----

/// Where a plugin comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The community update ships it.
    Shipped,
    /// The launcher installed it from the mods catalogue.
    Catalog { id: String, version: String },
    /// Added from disk (or put into the folder by hand).
    Local,
}

/// The loader config's switch for plugins that aren't verified (see [`Plugin::verified`]):
/// `x-local-plugins: true` at the top of `_local/config.yaml` (nEVR leaves `x-` keys to
/// others; the launcher keeps it when it writes the file).
pub const LOCAL_PLUGINS_KEY: &str = "x-local-plugins";

/// A plugin as the Mods page shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub file: String,
    /// Its own name (what it reported at the last start), else the configured one, else
    /// the file's.
    pub name: String,
    /// Its own version, from the last start.
    pub version: String,
    pub source: Source,
    /// nEVR loads it (turned on).
    pub enabled: bool,
    /// The launcher added it (from the catalogue or from disk).
    pub added: bool,
    /// It comes from the community update or the mods catalogue (a file they name), or it
    /// is EchoXR Hands' (pinned by checksum). Anything else (added from disk, or put into
    /// the folder by hand) loads only with local plugins on ([`LOCAL_PLUGINS_KEY`]).
    pub verified: bool,
    pub args: Map<String, Value>,
    /// Its default arguments (the catalogue's; none for a DLL from disk).
    pub defaults: Map<String, Value>,
    /// The game needs it: always on, also with "Start without mods".
    pub required: bool,
    /// The file is there.
    pub present: bool,
    /// What the loader did with it at the last start.
    pub status: Option<PluginStatus>,
    /// Its settings, from a description, with their values (none: just its arguments).
    pub settings: Option<plugin_settings::PluginSettings>,
}

impl Plugin {
    /// The launcher put its file there (and may take it away).
    pub fn removable(&self) -> bool {
        self.added
    }

    /// Its arguments differ from its defaults.
    pub fn changed(&self) -> bool {
        !same_args(&Value::Object(self.args.clone()), &self.defaults)
    }

    /// Its arguments as text fields: strings as they are, anything else as JSON.
    pub fn arg_strings(&self) -> BTreeMap<String, String> {
        strings(&self.args)
    }

    /// Its default arguments as text fields.
    pub fn default_strings(&self) -> BTreeMap<String, String> {
        strings(&self.defaults)
    }
}

/// Arguments as text: strings as they are, anything else as JSON.
fn strings(args: &Map<String, Value>) -> BTreeMap<String, String> {
    args.iter()
        .map(|(k, v)| {
            let text = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), text)
        })
        .collect()
}

/// A version's mods, as the Mods page shows them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModView {
    pub loader: Loader,
    /// Mods on (the overlay's "start without mods" is off).
    pub enabled: bool,
    pub plugins: Vec<Plugin>,
    /// The last start, if nEVR ever started.
    pub status: Option<Status>,
    /// A `config.yaml` nEVR reads instead of the launcher's.
    pub shadowed: Option<PathBuf>,
    /// The game's own `_local/config.json` nEVR loads (none: its built-in one).
    pub game_config: Option<PathBuf>,
    /// A `dbgcore.dll` beside the exe: nEVR won't start with it.
    pub stray_dbgcore: bool,
    /// Asset patches on at all, and each.
    pub assets_enabled: bool,
    pub asset_patches: Vec<AssetPatch>,
    /// Unverified plugins load ([`LOCAL_PLUGINS_KEY`] in the loader's config).
    pub local_plugins: bool,
}

/// The folder its plugins are in.
pub fn plugins_dir(v: &InstalledVersion) -> PathBuf {
    v.bin_dir().join(PLUGINS)
}

/// The folder its plugins log to (each in a folder of its own).
pub fn log_dir(bin: &Path) -> PathBuf {
    bin.join(PLUGIN_LOGS)
}

fn choices_path(v: &InstalledVersion) -> PathBuf {
    nevr::local_dir(v).join(CHOICES)
}

/// What the launcher put into `v` for mods, to take out when it lets go of `v`: the
/// plugins it added (that are still its own), its choices file, and the `config.yaml` it
/// writes for nEVR.
pub fn launcher_files(v: &InstalledVersion) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Overlay::read(&choices_path(v))
        .added()
        .iter()
        .filter(|a| a.launcher.is_some())
        .map(|a| plugins_dir(v).join(&a.file))
        .collect();
    out.push(choices_path(v));
    out.push(nevr::local_dir(v).join(nevr::CONFIG));
    out
}

/// The plugin files in `v`'s plugins folder.
fn plugin_files(v: &InstalledVersion) -> Vec<String> {
    dlls_in(&plugins_dir(v))
        .into_iter()
        .filter(|f| !NOT_PLUGINS.iter().any(|n| n.eq_ignore_ascii_case(f)))
        .collect()
}

/// Reads `v`'s mods (file I/O: on a worker).
pub fn read(v: &InstalledVersion) -> ModView {
    let bin = v.bin_dir();
    let slot = std::fs::read(bin.join(SLOT)).ok();
    let slot_sha = slot
        .as_ref()
        .and_then(|b| download::sha256_reader(&mut b.as_slice()).ok());
    let loader = classify(slot.as_deref(), slot_sha.as_deref());
    let overlay = Overlay::read(&choices_path(v));
    let catalog = ModCatalog::cached();
    let status = nevr::log_dir().and_then(|d| Status::read(&d));
    let files = plugin_files(v);
    let read = |p: &str| std::fs::read_to_string(bin.join(p)).unwrap_or_default();
    let (assets_enabled, asset_patches) = asset_patches(&read(ASSETS), &read(ASSETS_OVERLAY));
    let dir = plugins_dir(v);
    let mut plugins = plugins(&overlay, &files, status.as_ref(), &catalog);
    for p in plugins.iter_mut().filter(|p| p.present) {
        p.settings = plugin_settings::read(&dir, p, catalog.entry_for(&p.file));
    }
    ModView {
        enabled: overlay.enabled(),
        plugins,
        loader,
        status,
        shadowed: nevr::shadowing_config(v),
        game_config: nevr::game_config_in_use(v),
        stray_dbgcore: nevr::stray_dbgcore(&bin),
        assets_enabled,
        asset_patches,
        local_plugins: nevr::local_plugins(v),
    }
}

/// The DLL file names in `dir`, sorted.
fn dlls_in(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_ascii_lowercase().ends_with(".dll"))
        .collect();
    out.sort_by_key(|n| n.to_ascii_lowercase());
    out
}

/// Pure: the update's plugins: the DLLs in the folder the launcher didn't add, on, with
/// the catalogue's default arguments and whether the game needs them.
fn shipped(overlay: &Overlay, files: &[String], catalog: &ModCatalog) -> Vec<Entry> {
    let added = overlay.added();
    files
        .iter()
        .filter(|f| !added.iter().any(|a| a.file.eq_ignore_ascii_case(f)))
        .map(|f| {
            let known = catalog.entry_for(f);
            Entry {
                file: f.clone(),
                enabled: true,
                args: known.map(|m| m.args.clone()).unwrap_or_default(),
                required: known.is_some_and(|m| m.required),
                ..Default::default()
            }
        })
        .collect()
}

/// Pure: the plugins the page lists, and `config.yaml` names: the update's, then those
/// the launcher added, with its choices applied (a required one stays on).
pub fn plugins(
    overlay: &Overlay,
    files: &[String],
    status: Option<&Status>,
    catalog: &ModCatalog,
) -> Vec<Plugin> {
    let has = |f: &str| files.iter().any(|x| x.eq_ignore_ascii_case(f));
    overlay
        .apply(shipped(overlay, files, catalog))
        .into_iter()
        .map(|e| {
            let source = match &e.launcher {
                Some(Added {
                    catalog_id: Some(id),
                    version,
                    ..
                }) => Source::Catalog {
                    id: id.clone(),
                    version: version.clone(),
                },
                Some(_) => Source::Local,
                None => Source::Shipped,
            };
            let known = catalog.entry_for(&e.file);
            let defaults = match &source {
                Source::Local => Map::new(),
                _ => known.map(|m| m.args.clone()).unwrap_or_default(),
            };
            let verified = match &source {
                Source::Catalog { .. } => true,
                Source::Shipped => {
                    known.is_some()
                        || e.file
                            .eq_ignore_ascii_case(crate::core::echoxr_hands::PLUGIN)
                }
                Source::Local => false,
            };
            // Required by its name, however it got there (the update, or added by hand).
            let mut e = e;
            e.required |= known.is_some_and(|m| m.required);
            let mut p = plugin(e, source, defaults, has, status);
            p.verified = verified;
            p
        })
        .collect()
}

fn plugin(
    e: Entry,
    source: Source,
    defaults: Map<String, Value>,
    has: impl Fn(&str) -> bool,
    status: Option<&Status>,
) -> Plugin {
    let st = status.and_then(|s| s.of(&e.file)).cloned();
    let stem = e
        .file
        .rsplit_once('.')
        .map_or(e.file.as_str(), |(s, _)| s)
        .to_string();
    let name = st
        .as_ref()
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| Some(e.name.clone()).filter(|n| !n.is_empty()))
        .unwrap_or(stem);
    Plugin {
        present: has(&e.file),
        name,
        version: st.as_ref().map(|s| s.version.clone()).unwrap_or_default(),
        added: e.launcher.is_some(),
        verified: false,
        source,
        enabled: e.enabled || e.required,
        required: e.required,
        args: e.args,
        defaults,
        status: st,
        settings: None,
        file: e.file,
    }
}

/// Before starting `v` with nEVR in its slot: takes out a `dbgcore.dll` beside the exe
/// (the old loader's place, which nEVR won't start beside), arranges the game's
/// `_local/config.json` (an obsolete EchoVRCE one is set aside so nEVR's built-in one,
/// with friends and parties, applies; with `own_game_config` yours is used), and writes
/// `config.yaml`. Without nEVR there is nothing to do.
pub fn before_start(v: &InstalledVersion, own_game_config: bool, hands: bool) -> Result<()> {
    let bin = v.bin_dir();
    if !nevr_in(&bin) {
        // Without nEVR the game needs its config again (Verify put the stock DLL back).
        if nevr::restore_game_config(v)? == nevr::GameConfigStep::Restored {
            tracing::info!("no nEVR: the _local/config.json set aside for it is back");
        }
        return Ok(());
    }
    if nevr::stray_dbgcore(&bin) {
        let path = bin.join(nevr::DBGCORE);
        std::fs::remove_file(&path).with_context(|| {
            format!(
                "Couldn't remove {}, which nEVR won't start beside",
                path.display()
            )
        })?;
        tracing::info!("removed {} (nEVR won't start beside it)", path.display());
    }
    match nevr::arrange_game_config(v, own_game_config)? {
        nevr::GameConfigStep::SetAside => tracing::info!(
            "moved the EchoVRCE-era _local/config.json aside: nEVR's built-in one applies"
        ),
        nevr::GameConfigStep::Restored => {
            tracing::info!("your _local/config.json is back (Use my own config.json)")
        }
        nevr::GameConfigStep::Nothing => {}
    }
    // EchoXR Hands' plugin in plugins/ for this start, or out of it; nEVR's list follows.
    if let Err(e) = crate::core::echoxr_hands::apply(&bin, hands) {
        tracing::warn!("hand tracking: {e:#}");
    }
    prepare(v)
}

/// Before a start: writes `v`'s `config.yaml` from its plugins and the launcher's
/// choices. With mods off only the required plugins are listed (NvrAssetPatches then
/// loads only its required patches). A plugin the launcher added whose file no longer
/// matches the checksum it noted is left out.
pub fn prepare(v: &InstalledVersion) -> Result<()> {
    let overlay = Overlay::read(&choices_path(v));
    let catalog = ModCatalog::cached();
    let local = nevr::local_plugins(v);
    nevr::write_config(v, &config_lines(v, &overlay, &catalog, local), local)
}

fn config_lines(
    v: &InstalledVersion,
    overlay: &Overlay,
    catalog: &ModCatalog,
    local: bool,
) -> Vec<PluginLine> {
    let mods_on = overlay.enabled();
    let added = overlay.added();
    let mut lines = Vec::new();
    for p in plugins(overlay, &plugin_files(v), None, catalog) {
        if !p.present || !(mods_on || p.required) {
            continue;
        }
        if !p.verified && !local {
            tracing::info!(
                "{}: not verified, and local plugins are off: left out",
                p.file
            );
            continue;
        }
        let pinned = added
            .iter()
            .find(|a| a.file.eq_ignore_ascii_case(&p.file))
            .and_then(|a| a.sha256.clone());
        if let Some(want) = pinned {
            let have = download::sha256_file(&plugins_dir(v).join(&p.file)).unwrap_or_default();
            if !have.eq_ignore_ascii_case(&want) {
                tracing::warn!("{} changed since it was added: left out", p.file);
                continue;
            }
        }
        let mut args = p.args;
        // An argument nEVR would read as an environment variable could keep every plugin
        // from loading (one saved before the launcher refused them).
        args.retain(|key, value| {
            let env = nevr::expands_env(key) || nevr::expands_env(&value.to_string());
            if env {
                tracing::warn!("{}: argument {key} holds ${{…}}: left out", p.file);
            }
            !env
        });
        if !mods_on && p.file.eq_ignore_ascii_case(ASSET_PLUGIN) {
            // An older build loads every patch whatever it's told: none of it, then.
            if !knows_required_only(&plugins_dir(v).join(&p.file)) {
                continue;
            }
            args.insert("required_only".into(), Value::String("true".into()));
        }
        lines.push(PluginLine {
            file: p.file,
            enabled: p.enabled,
            args,
        });
    }
    lines
}

/// Whether the NvrAssetPatches build at `path` takes `required_only` (1.2.0 on).
fn knows_required_only(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|b| find(&b, b"required_only").is_some())
}

/// Mods on or off for `v` (the overlay's "start without mods").
pub fn set_enabled(v: &InstalledVersion, on: bool) -> Result<()> {
    edit_overlay(v, |o| o.set_enabled(on))
}

/// `file` on or off (a required plugin can't go off).
pub fn set_plugin_enabled(v: &InstalledVersion, file: &str, on: bool) -> Result<()> {
    if !on
        && ModCatalog::cached()
            .entry_for(file)
            .is_some_and(|m| m.required)
    {
        bail!("{file} is needed by the game: it can't be turned off.");
    }
    edit_overlay(v, |o| o.set_plugin_enabled(file, on))
}

/// Puts `file`'s arguments back to its defaults: the catalogue's, none for a DLL added
/// from disk (as [`plugins`] shows them).
pub fn reset_args(v: &InstalledVersion, file: &str) -> Result<()> {
    let local = Overlay::read(&choices_path(v))
        .added()
        .iter()
        .any(|a| a.file.eq_ignore_ascii_case(file) && a.launcher.as_ref().is_some_and(|l| l.local));
    let defaults = if local {
        Map::new()
    } else {
        ModCatalog::cached()
            .entry_for(file)
            .map(|m| m.args.clone())
            .unwrap_or_default()
    };
    edit_overlay(v, |o| o.reset_args(file, &defaults))
}

/// `file`'s arguments. None may hold `${…}`: nEVR would read it as an environment
/// variable, and an unset one makes it load no plugins at all.
pub fn set_args(v: &InstalledVersion, file: &str, args: &BTreeMap<String, String>) -> Result<()> {
    if let Some((key, _)) = args
        .iter()
        .find(|(k, v)| nevr::expands_env(k) || nevr::expands_env(v))
    {
        bail!("{key}: nEVR reads ${{…}} as an environment variable, and loads no plugins at all when it isn't set. Leave it out.");
    }
    edit_overlay(v, |o| o.set_args(file, args))
}

/// Some of `file`'s arguments (key, value), the others as they are.
pub fn set_some_args(v: &InstalledVersion, file: &str, some: &[(String, String)]) -> Result<()> {
    let o = Overlay::read(&choices_path(v));
    let mut args = o
        .apply(shipped(&o, &plugin_files(v), &ModCatalog::cached()))
        .into_iter()
        .find(|e| e.file.eq_ignore_ascii_case(file))
        .map(|e| strings(&e.args))
        .unwrap_or_default();
    for (k, val) in some {
        args.insert(k.clone(), val.clone());
    }
    set_args(v, file, &args)
}

fn edit_overlay(v: &InstalledVersion, f: impl FnOnce(&mut Overlay)) -> Result<()> {
    let path = choices_path(v);
    let mut o = Overlay::read(&path);
    f(&mut o);
    o.prune(&shipped(&o, &plugin_files(v), &ModCatalog::cached()));
    if !o.is_empty() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
    }
    write_or_remove(&path, o.is_empty(), || o.write(&path))
}

/// Writes an overlay, or deletes it when it holds no choice any more.
fn write_or_remove(path: &Path, empty: bool, write: impl FnOnce() -> Result<()>) -> Result<()> {
    if !empty {
        return write();
    }
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("Couldn't remove {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Asset patch `label` on or off (`None`: all of them).
pub fn set_asset_patch(v: &InstalledVersion, label: Option<&str>, on: bool) -> Result<()> {
    let bin = v.bin_dir();
    let path = bin.join(ASSETS_OVERLAY);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let manifest = std::fs::read_to_string(bin.join(ASSETS)).unwrap_or_default();
    let o = set_asset(&manifest, &text, label, on);
    let empty = o
        .as_object()
        .is_none_or(|m| m.keys().all(|k| k == "version"));
    write_or_remove(&path, empty, || write_json(&path, &o))
}

/// Pure: whether `name` is a plugin file name the launcher takes (no folders).
pub fn is_plugin_file(name: &str) -> bool {
    name.len() <= 64
        && !name.starts_with('.')
        && name.to_ascii_lowercase().ends_with(".dll")
        && name.len() > 4
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Puts `src` into `v`'s plugins folder as `file` (staged beside it, then renamed).
fn place(v: &InstalledVersion, src: &Path, file: &str) -> Result<()> {
    let dir = plugins_dir(v);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let target = dir.join(file);
    let staged = dir.join(format!(".{file}.download"));
    std::fs::copy(src, &staged).with_context(|| format!("Couldn't copy into {}", dir.display()))?;
    std::fs::rename(&staged, &target).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        if crate::core::pc_update::is_locked(&e) {
            anyhow::anyhow!(
                "Couldn't replace {file}: Echo VR uses it. Close Echo VR and try again."
            )
        } else {
            anyhow::Error::from(e).context(format!("replace {}", target.display()))
        }
    })
}

/// A file the update ships (or one in the folder the launcher didn't put there) can't be
/// replaced by one it installs.
fn check_free(v: &InstalledVersion, file: &str) -> Result<()> {
    let view = read(v);
    if let Some(p) = view
        .plugins
        .iter()
        .find(|p| p.file.eq_ignore_ascii_case(file) && p.present && !p.removable())
    {
        bail!(
            "{} already has a plugin named {file} ({}). Remove it first, or rename the file.",
            v.name,
            p.name
        );
    }
    Ok(())
}

/// Installs mod `e` into `v`: downloaded, checked against its checksum, put into the
/// plugins folder and added (on) with that checksum, which [`prepare`] checks again.
pub fn install(
    v: &InstalledVersion,
    e: &ModEntry,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<()> {
    let Some(sha) = e.sha256.as_deref().filter(|_| e.downloadable()) else {
        bail!(
            "{} can't be downloaded: it comes with the community update.",
            e.name
        );
    };
    check_free(v, &e.file)?;
    on(Step::Status(format!("Downloading {}...", e.name)));
    let file = download::fetch_pinned(
        &e.url,
        &paths::downloads_dir().join("mods"),
        &e.file,
        sha,
        cancel,
        &mut |p| {
            if let download::Progress::Percent(p) = p {
                on(Step::Percent(p));
            }
        },
    )?;
    on(Step::Status(format!("Installing {}...", e.name)));
    place(v, &file, &e.file)?;
    let _ = std::fs::remove_file(&file);
    edit_overlay(v, |o| {
        o.add(&Entry {
            file: e.file.clone(),
            name: e.name.clone(),
            enabled: true,
            args: e.args.clone(),
            sha256: Some(sha.to_ascii_lowercase()),
            launcher: Some(Added {
                catalog_id: Some(e.id.clone()),
                version: e.version.clone(),
                local: false,
            }),
            required: false,
        })
    })
}

/// Adds the DLL at `src` to `v` (copied into its plugins folder, on, with its checksum).
/// Returns the file name.
pub fn add_local(v: &InstalledVersion, src: &Path) -> Result<String> {
    let file = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !is_plugin_file(&file) {
        bail!("{file:?} isn't a plugin file name the launcher takes: letters, digits, '.', '-' and '_', ending in .dll.");
    }
    if file.eq_ignore_ascii_case(SLOT) {
        bail!("{SLOT} is the mod loader itself, not a plugin.");
    }
    if NOT_PLUGINS.iter().any(|n| n.eq_ignore_ascii_case(&file)) {
        bail!("{file} isn't a plugin nEVR loads.");
    }
    check_free(v, &file)?;
    let sha = download::sha256_file(src)?;
    place(v, src, &file)?;
    edit_overlay(v, |o| {
        o.add(&Entry {
            file: file.clone(),
            enabled: true,
            sha256: Some(sha),
            launcher: Some(Added {
                local: true,
                ..Default::default()
            }),
            ..Default::default()
        })
    })?;
    Ok(file)
}

/// Takes plugin `file` out of `v`: its entry, and its file (only one the launcher put
/// there).
pub fn remove(v: &InstalledVersion, file: &str) -> Result<()> {
    let view = read(v);
    let Some(p) = view
        .plugins
        .iter()
        .find(|p| p.file.eq_ignore_ascii_case(file))
    else {
        bail!("{file} isn't a plugin of {}.", v.name);
    };
    if !p.removable() {
        bail!("{file} comes with the community update: turn it off instead.");
    }
    let path = plugins_dir(v).join(&p.file);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) if crate::core::pc_update::is_locked(&e) => {
            bail!("Couldn't delete {file}: Echo VR uses it. Close Echo VR and try again.")
        }
        Err(e) => return Err(anyhow::Error::from(e).context(format!("delete {}", path.display()))),
    }
    edit_overlay(v, |o| {
        o.remove_added(file);
    })
}

// ---- the mods catalogue (mods.json on release.echovr.de) ----

/// A mod in the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModEntry {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub author: String,
    pub version: String,
    /// The plugin's file name in `plugins/`.
    pub file: String,
    /// Relative to the download mirrors, or https on a trusted host. Empty for a shipped
    /// one.
    pub url: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    /// The plugin ABI version it was built for.
    pub api: Option<u32>,
    /// What it does: observes-only, cosmetic, alters-gameplay, alters-rules, network,
    /// hooks-engine.
    pub capabilities: Vec<String>,
    /// Its default arguments.
    pub args: Map<String, Value>,
    pub homepage: String,
    /// It comes with the community update: shown, never downloaded.
    pub shipped: bool,
    /// The game needs it: always on, also with "Start without mods".
    pub required: bool,
    /// What can be set, and how (see [`super::plugin_settings`]).
    pub settings: Option<Value>,
}

impl ModEntry {
    pub fn downloadable(&self) -> bool {
        !self.shipped && !self.url.is_empty()
    }

    fn validate(&self) -> Result<()> {
        if !catalog::is_safe_id(&self.id) {
            bail!("invalid mod id {:?}", self.id);
        }
        if !is_plugin_file(&self.file) || self.file.eq_ignore_ascii_case(SLOT) {
            bail!("invalid plugin file for {}: {:?}", self.id, self.file);
        }
        if !self.shipped {
            if self.url.is_empty() || !catalog::is_safe_url(&self.url) {
                bail!("untrusted download for {}: {}", self.id, self.url);
            }
            let sha = self.sha256.as_deref().unwrap_or_default();
            if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("{} has no valid sha256", self.id);
            }
        }
        if !self.homepage.is_empty() && !self.homepage.starts_with("https://") {
            bail!("invalid homepage for {}", self.id);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModCatalog {
    pub mods: Vec<ModEntry>,
    /// The built-in list is shown (the published one couldn't be fetched).
    pub builtin: bool,
}

impl ModCatalog {
    /// The catalogue in `text`; entries that fail validation (or repeat an id) are left
    /// out and logged.
    pub fn parse(text: &str) -> Result<ModCatalog> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            mods: Vec<Value>,
        }
        let raw: Raw = serde_json::from_str(text)?;
        let mut seen = std::collections::HashSet::new();
        let mut mods = Vec::new();
        for (i, value) in raw.mods.into_iter().enumerate() {
            let entry = serde_json::from_value::<ModEntry>(value)
                .map_err(anyhow::Error::from)
                .and_then(|m| m.validate().map(|()| m));
            match entry {
                Ok(m) if seen.insert(m.id.clone()) => mods.push(m),
                Ok(m) => tracing::warn!("mods catalogue: duplicate id {}, left out", m.id),
                Err(e) => tracing::warn!("mods catalogue: entry {i} left out: {e:#}"),
            }
        }
        Ok(ModCatalog {
            mods,
            builtin: false,
        })
    }

    /// The draft in this repo, for when the published one can't be had.
    pub fn builtin() -> ModCatalog {
        let mut c =
            ModCatalog::parse(include_str!("../../../docs/launcher/mods.json")).unwrap_or_default();
        c.builtin = true;
        c
    }

    /// The published catalogue (kept for [`ModCatalog::cached`]), else the last one kept,
    /// else the built-in one.
    pub fn load() -> ModCatalog {
        let fetched = crate::core::http::get_text(CATALOG_URL)
            .and_then(|t| ModCatalog::parse(&t).map(|c| (c, t)));
        match fetched {
            Ok((c, text)) => {
                let _ = std::fs::create_dir_all(paths::data_dir());
                let _ = std::fs::write(Self::cache_path(), text);
                c
            }
            Err(e) => {
                tracing::info!("mods catalogue unavailable ({e:#}); using the last one kept");
                ModCatalog::cached()
            }
        }
    }

    fn cache_path() -> PathBuf {
        paths::data_dir().join("mods.json")
    }

    /// The last catalogue fetched, else the built-in one: no network, for reading the
    /// mods and before a start. (Tests always get the built-in one.)
    pub fn cached() -> ModCatalog {
        if cfg!(test) {
            return ModCatalog::builtin();
        }
        std::fs::read_to_string(Self::cache_path())
            .ok()
            .and_then(|t| ModCatalog::parse(&t).ok())
            .unwrap_or_else(ModCatalog::builtin)
    }

    /// The entry of plugin file `file`.
    pub fn entry_for(&self, file: &str) -> Option<&ModEntry> {
        self.mods.iter().find(|m| m.file.eq_ignore_ascii_case(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_the_loaders_apart() {
        let nevr = b"MZ....[NEVR.BOOT] up\0\x014.0.0+182.e418eaa\0rest".as_slice();
        assert_eq!(
            classify(Some(nevr), None),
            Loader::Nevr {
                version: "4.0.0+182.e418eaa".into()
            }
        );
        assert!(classify(Some(nevr), None).editable());
        assert_eq!(classify(Some(b"MZ"), Some(STOCK_SLOT_SHA256)), Loader::None);
        assert_eq!(classify(None, None), Loader::None);
        // EchoLoader 2 (the event builds' loader, not the live build's), or anything else.
        assert_eq!(
            classify(Some(b"MZ ECHOLOADER_ID:2.0.0:open"), Some("00")),
            Loader::Unknown
        );
        assert!(!Loader::Unknown.editable());
    }

    #[test]
    fn applies_the_launchers_choices() {
        let mut o = Overlay::default();
        o.set_plugin_enabled("NVRASSETPATCHES.dll", false);
        o.add(&Entry {
            file: "Mine.dll".into(),
            enabled: true,
            sha256: Some("cd".repeat(32)),
            launcher: Some(Added {
                local: true,
                ..Default::default()
            }),
            ..Default::default()
        });
        let files: Vec<String> = ["NvrAssetPatches.dll", "Mine.dll", "Stray.dll"]
            .map(String::from)
            .into();
        let ps = plugins(&o, &files, None, &ModCatalog::default());
        let find = |f: &str| ps.iter().find(|p| p.file == f).unwrap();
        assert_eq!(ps.len(), 3);
        assert!(!find("NvrAssetPatches.dll").enabled);
        assert_eq!(find("NvrAssetPatches.dll").source, Source::Shipped);
        // A DLL put into the folder by hand loads, as the update's do.
        assert!(find("Stray.dll").enabled && !find("Stray.dll").removable());
        assert_eq!(find("Mine.dll").source, Source::Local);
        assert!(find("Mine.dll").enabled && find("Mine.dll").removable());

        // Arguments replace the plugin's own; on/off on an added entry stays on it.
        o.set_args(
            "NvrAssetPatches.dll",
            &BTreeMap::from([("logging".into(), "verbose".into())]),
        );
        o.set_plugin_enabled("mine.dll", false);
        let ps = plugins(&o, &files, None, &ModCatalog::default());
        let find = |f: &str| ps.iter().find(|p| p.file == f).unwrap();
        assert_eq!(find("NvrAssetPatches.dll").args["logging"], "verbose");
        assert!(!find("Mine.dll").enabled);
        assert!(o.overrides("mine.dll").is_none());
        assert!(o.remove_added("MINE.DLL"));
        assert!(!o.remove_added("mine.dll"));
    }

    #[test]
    fn overlay_round_trips_and_keeps_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CHOICES);
        std::fs::write(
            &path,
            r#"{"version":1,"future":{"x":1},"overrides":{"A.dll":{"required":true}}}"#,
        )
        .unwrap();
        let mut o = Overlay::read(&path);
        assert!(o.enabled());
        o.set_enabled(false);
        o.set_plugin_enabled("a.dll", false);
        o.add(&Entry {
            file: "B.dll".into(),
            enabled: true,
            launcher: Some(Added {
                catalog_id: Some("b".into()),
                version: "1.0".into(),
                local: false,
            }),
            ..Default::default()
        });
        o.write(&path).unwrap();
        let back = Overlay::read(&path);
        assert!(!back.enabled());
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["future"]["x"], 1);
        assert_eq!(raw["overrides"]["A.dll"]["required"], true);
        assert_eq!(raw["overrides"]["A.dll"]["enabled"], false);
        assert_eq!(raw["add"][0]["launcher"]["catalog_id"], "b");
        assert!(raw["add"][0]["launcher"].get("local").is_none());
        assert_eq!(back.added()[0].launcher.as_ref().unwrap().version, "1.0");
        // Garbage reads as empty.
        assert_eq!(Overlay::parse("[1,2]"), Overlay::default());
    }

    #[test]
    fn shows_what_nevr_did() {
        let s = Status::parse(
            "[NEVR.PLUGIN] Loaded: NvrAssetPatches v1.1.0 (API v5) caps=0x22 via plugins\\NvrAssetPatches.dll",
        );
        let ps = plugins(
            &Overlay::default(),
            &["NvrAssetPatches.dll".into(), "Other.dll".into()],
            Some(&s),
            &ModCatalog::default(),
        );
        assert_eq!(ps[0].version, "1.1.0");
        assert!(ps[0].status.as_ref().unwrap().loaded());
        assert!(ps[1].status.is_none());
    }

    #[test]
    fn asset_patches_take_the_launchers_choices() {
        let manifest = r#"{"version":"1.0","patches":[
            {"label":"netgun_base","enabled":true},{"label":"poster_a_tex","enabled":true},
            {"label":"off_by_update","enabled":false},{"enabled":true}]}"#;
        let (on, ps) = asset_patches(manifest, "");
        assert!(on);
        assert_eq!(ps.len(), 3);
        assert!(!ps[2].enabled);
        let o = set_asset(manifest, "", Some("poster_a_tex"), false);
        let o = set_asset(manifest, &o.to_string(), None, false);
        let (on, ps) = asset_patches(manifest, &o.to_string());
        assert!(!on);
        assert!(ps[0].enabled && !ps[1].enabled);
        assert_eq!(o["version"], 1);
        // Back to how they ship: nothing is left but the version.
        let o = set_asset(manifest, &o.to_string(), Some("poster_a_tex"), true);
        let o = set_asset(manifest, &o.to_string(), None, true);
        assert_eq!(o, serde_json::json!({"version": 1}));
        // Turning on what the update ships off is a real choice and stays.
        let o = set_asset(manifest, "", Some("off_by_update"), true);
        assert_eq!(o["patches"]["off_by_update"]["enabled"], true);
    }

    #[test]
    fn overlay_keeps_only_real_choices() {
        let files: Vec<String> = ["NvrAssetPatches.dll", "Other.dll"]
            .map(String::from)
            .into();
        let mut o = Overlay::default();
        o.set_enabled(false);
        o.set_plugin_enabled("Other.dll", false);
        let mut args = BTreeMap::new();
        args.insert("logging".to_string(), "verbose".to_string());
        o.set_args("NvrAssetPatches.dll", &args);
        o.prune(&shipped(&o, &files, &ModCatalog::default()));
        assert!(!o.enabled());
        assert!(!o.is_empty());
        // Everything set back to how it ships: nothing is left.
        o.set_enabled(true);
        o.set_plugin_enabled("OTHER.DLL", true);
        let o2 = {
            let mut o2 = o.clone();
            o2.set_args("NvrAssetPatches.dll", &BTreeMap::new());
            o2.prune(&shipped(&o2, &files, &ModCatalog::default()));
            o2
        };
        assert!(o2.is_empty(), "{:?}", o2.0);
    }

    fn catalog() -> ModCatalog {
        ModCatalog::parse(
            r#"{"schema":1,"mods":[
              {"id":"asset-patches","file":"NvrAssetPatches.dll","shipped":true,"required":true,
               "args":{"logging":"normal"}},
              {"id":"extra","file":"Extra.dll","shipped":true,"args":{"mode":"a"}}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn required_plugins_stay_on() {
        let files: Vec<String> = ["NvrAssetPatches.dll", "Extra.dll"]
            .map(String::from)
            .into();
        let mut o = Overlay::default();
        // An old "off" for the required plugin: ignored, and pruned as no choice.
        o.set_plugin_enabled("NvrAssetPatches.dll", false);
        o.set_plugin_enabled("Extra.dll", false);
        let ps = plugins(&o, &files, None, &catalog());
        let find = |f: &str| ps.iter().find(|p| p.file == f).unwrap();
        assert!(find("NvrAssetPatches.dll").enabled && find("NvrAssetPatches.dll").required);
        assert!(!find("Extra.dll").enabled && !find("Extra.dll").required);
        // Shipped plugins get the catalogue's arguments as their defaults.
        assert_eq!(find("Extra.dll").args["mode"], "a");
        assert!(!find("Extra.dll").changed());
        o.prune(&shipped(&o, &files, &catalog()));
        assert!(o.overrides("NvrAssetPatches.dll").is_none());
        assert!(o.overrides("Extra.dll").is_some());
    }

    #[test]
    fn arguments_reset_to_the_catalogues() {
        let files: Vec<String> = ["Extra.dll".to_string()].into();
        let mut o = Overlay::default();
        o.set_args("Extra.dll", &BTreeMap::from([("mode".into(), "b".into())]));
        let p = &plugins(&o, &files, None, &catalog())[0];
        assert!(p.changed());
        assert_eq!(p.defaults["mode"], "a");
        o.reset_args("Extra.dll", &p.defaults);
        o.prune(&shipped(&o, &files, &catalog()));
        assert!(o.is_empty(), "{:?}", o.0);
        // Saving exactly the defaults leaves nothing behind either.
        o.set_args("Extra.dll", &BTreeMap::from([("mode".into(), "a".into())]));
        o.prune(&shipped(&o, &files, &catalog()));
        assert!(o.is_empty(), "{:?}", o.0);
        // An added plugin gets its catalogue arguments back.
        o.add(&Entry {
            file: "Mine.dll".into(),
            enabled: true,
            args: serde_json::json!({"x":"1"}).as_object().cloned().unwrap(),
            launcher: Some(Added {
                catalog_id: Some("mine".into()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let defaults = serde_json::json!({"x":"0"}).as_object().cloned().unwrap();
        o.reset_args("Mine.dll", &defaults);
        assert_eq!(o.added()[0].args["x"], "0");
    }

    #[test]
    fn required_asset_patches_stay_on() {
        let manifest = r#"{"patches":[{"label":"netgun_base","required":true},
            {"label":"poster_a_tex"}]}"#;
        let o = set_asset(manifest, "", Some("netgun_base"), false);
        assert_eq!(o, serde_json::json!({"version": 1}));
        let o = set_asset(manifest, &o.to_string(), Some("poster_a_tex"), false);
        // A stale "off" written by hand for a required patch: shown on anyway, dropped.
        let stale = r#"{"version":1,"patches":{"netgun_base":{"enabled":false}}}"#;
        let (_, ps) = asset_patches(manifest, stale);
        assert!(ps[0].enabled && ps[0].required);
        let o2 = set_asset(manifest, stale, Some("poster_a_tex"), false);
        assert!(o2["patches"].get("netgun_base").is_none());
        let (_, ps) = asset_patches(manifest, &o.to_string());
        assert!(ps[0].enabled && !ps[1].enabled);
    }

    /// A live version with nEVR in its slot, the update's plugin, and EchoRelay's patch
    /// still in `plugins/`.
    fn live(dir: &Path) -> InstalledVersion {
        let root = dir.join("pc");
        let bin = root.join(paths::ARENA_DIR).join("bin/win10");
        std::fs::create_dir_all(bin.join("plugins")).unwrap();
        std::fs::write(bin.join("echovr.exe"), "MZ").unwrap();
        std::fs::write(bin.join(SLOT), "MZ [NEVR.BOOT] \x004.0.0+1.abcdef0\0").unwrap();
        // 1.2.0 on: it takes required_only.
        std::fs::write(
            bin.join("plugins/NvrAssetPatches.dll"),
            "MZ shipped required_only",
        )
        .unwrap();
        std::fs::write(bin.join("plugins/dbgcore.dll"), "MZ relay").unwrap();
        InstalledVersion {
            id: "pc-latest".into(),
            name: "Echo VR".into(),
            root: paths::normalize(&root.to_string_lossy()),
            ..Default::default()
        }
    }

    #[test]
    fn writes_nevrs_list_before_a_start() {
        let dir = tempfile::tempdir().unwrap();
        let v = live(dir.path());
        let bin = v.bin_dir();
        let yaml = nevr::local_dir(&v).join(nevr::CONFIG);
        // The old loader's place: nEVR won't start beside it, so it goes.
        std::fs::write(bin.join("dbgcore.dll"), "MZ EchoLoader 1").unwrap();
        before_start(&v, false, false).unwrap();
        assert!(!bin.join("dbgcore.dll").exists());
        assert!(bin.join("plugins/dbgcore.dll").exists());
        let text = std::fs::read_to_string(&yaml).unwrap();
        assert!(text.contains("file: \"NvrAssetPatches.dll\"\n    enabled: true"));
        assert!(!text.contains("dbgcore"));
        assert_eq!(
            read(&v).loader,
            Loader::Nevr {
                version: "4.0.0+1.abcdef0".into()
            }
        );

        // A plugin added from disk isn't verified: left out while local plugins are off,
        // and a DLL put into the folder by hand too.
        let src = dir.path().join("MyMod.dll");
        std::fs::write(&src, "MZ mine").unwrap();
        add_local(&v, &src).unwrap();
        std::fs::write(bin.join("plugins/Dropped.dll"), "MZ dropped").unwrap();
        prepare(&v).unwrap();
        let text = std::fs::read_to_string(&yaml).unwrap();
        assert!(!text.contains("MyMod") && !text.contains("Dropped"));
        let view = read(&v);
        assert!(!view.local_plugins);
        assert!(view.plugins.iter().filter(|p| !p.verified).count() == 2);
        assert!(view
            .plugins
            .iter()
            .any(|p| p.file == "NvrAssetPatches.dll" && p.verified));
        // With local plugins on in the loader's config they're listed, while their file is
        // the one added; the switch stays when the launcher writes the file again.
        std::fs::write(&yaml, format!("{text}x-local-plugins: true\n")).unwrap();
        prepare(&v).unwrap();
        let text = std::fs::read_to_string(&yaml).unwrap();
        assert!(text.contains("\"MyMod.dll\"") && text.contains("\"Dropped.dll\""));
        assert!(nevr::local_plugins_in(&text) && read(&v).local_plugins);
        std::fs::remove_file(bin.join("plugins/Dropped.dll")).unwrap();
        std::fs::write(bin.join("plugins/MyMod.dll"), "MZ swapped").unwrap();
        prepare(&v).unwrap();
        assert!(!std::fs::read_to_string(&yaml).unwrap().contains("MyMod"));

        // Mods off: only the required ones, NvrAssetPatches with its required patches.
        set_enabled(&v, false).unwrap();
        prepare(&v).unwrap();
        let text = std::fs::read_to_string(&yaml).unwrap();
        assert!(text.contains("file: \"NvrAssetPatches.dll\"\n    enabled: true"));
        assert!(text.contains("\"required_only\":\"true\""));
        assert!(!text.contains("MyMod"));
        // An older NvrAssetPatches would load every patch: left out with mods off.
        std::fs::write(bin.join("plugins/NvrAssetPatches.dll"), "MZ 1.1.0").unwrap();
        prepare(&v).unwrap();
        assert!(!std::fs::read_to_string(&yaml)
            .unwrap()
            .contains("NvrAssetPatches"));
        std::fs::write(
            bin.join("plugins/NvrAssetPatches.dll"),
            "MZ shipped required_only",
        )
        .unwrap();

        // Without nEVR, nothing is written.
        std::fs::write(bin.join(SLOT), "MZ stock").unwrap();
        std::fs::remove_file(&yaml).unwrap();
        before_start(&v, false, false).unwrap();
        assert!(!yaml.exists());
    }

    #[test]
    fn no_argument_nevr_reads_as_a_variable() {
        let dir = tempfile::tempdir().unwrap();
        let v = live(dir.path());
        let yaml = nevr::local_dir(&v).join(nevr::CONFIG);
        let args = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        // Refused when saved...
        let env = args(&[("logging", "verbose"), ("log_dir", "${HOME}/logs")]);
        assert!(set_args(&v, ASSET_PLUGIN, &env).is_err());
        // ...and one saved before is left out of config.yaml, the plugin and the rest stay.
        edit_overlay(&v, |o| o.set_args(ASSET_PLUGIN, &env)).unwrap();
        prepare(&v).unwrap();
        let text = std::fs::read_to_string(&yaml).unwrap();
        assert!(text.contains("\"NvrAssetPatches.dll\""));
        assert!(text.contains("\"logging\":\"verbose\""));
        assert!(!text.contains("${"));
        // An unterminated ${ is no variable to nEVR.
        set_args(&v, ASSET_PLUGIN, &args(&[("note", "costs ${5")])).unwrap();
    }

    #[test]
    fn installs_adds_and_removes_plugins() {
        let dir = tempfile::tempdir().unwrap();
        let v = live(dir.path());
        let bin = v.bin_dir();
        let src = dir.path().join("MyMod.dll");
        std::fs::write(&src, "MZ mine").unwrap();
        assert_eq!(add_local(&v, &src).unwrap(), "MyMod.dll");
        let view = read(&v);
        // EchoRelay's patch isn't a plugin nEVR loads: not listed.
        assert!(view.plugins.iter().all(|p| p.file != "dbgcore.dll"));
        let mine = view.plugins.iter().find(|p| p.file == "MyMod.dll").unwrap();
        assert!(mine.enabled && mine.present && mine.removable());
        let o = Overlay::read(&nevr::local_dir(&v).join(CHOICES));
        assert_eq!(
            o.added()[0].sha256.as_deref(),
            Some(download::sha256_file(&src).unwrap().as_str())
        );
        // The update's plugin can't be replaced or removed.
        let shadow = dir.path().join("NvrAssetPatches.dll");
        std::fs::write(&shadow, "MZ other").unwrap();
        assert!(add_local(&v, &shadow).is_err());
        assert!(remove(&v, "NvrAssetPatches.dll").is_err());
        assert!(add_local(&v, &dir.path().join("BugSplat64.dll")).is_err());
        let relay = dir.path().join("dbgcore.dll");
        std::fs::write(&relay, "MZ").unwrap();
        assert!(add_local(&v, &relay).is_err());

        // The game needs it: it can't be turned off, and stays on with mods off.
        assert!(set_plugin_enabled(&v, "NvrAssetPatches.dll", false).is_err());
        set_enabled(&v, false).unwrap();
        let view = read(&v);
        assert!(!view.enabled);
        let shipped = view
            .plugins
            .iter()
            .find(|p| p.file == "NvrAssetPatches.dll")
            .unwrap();
        assert!(shipped.enabled && shipped.required);
        assert_eq!(shipped.defaults["logging"], "normal");

        // A required plugin added by hand (the update doesn't ship it yet) is required
        // all the same: on, also with mods off, and it can't be turned off.
        let fix = dir.path().join("NvrXmlHttpFix.dll");
        std::fs::write(&fix, "MZ fix").unwrap();
        add_local(&v, &fix).unwrap();
        let p = read(&v)
            .plugins
            .into_iter()
            .find(|p| p.file == "NvrXmlHttpFix.dll")
            .unwrap();
        assert!(p.required && p.enabled);
        assert!(set_plugin_enabled(&v, "NvrXmlHttpFix.dll", false).is_err());
        // It came from disk: listed only with local plugins on.
        assert!(!p.verified);
        prepare(&v).unwrap();
        let yaml = nevr::local_dir(&v).join(nevr::CONFIG);
        assert!(!std::fs::read_to_string(&yaml)
            .unwrap()
            .contains("NvrXmlHttpFix.dll"));
        std::fs::write(&yaml, "x-local-plugins: true\n").unwrap();
        prepare(&v).unwrap();
        assert!(std::fs::read_to_string(&yaml)
            .unwrap()
            .contains("NvrXmlHttpFix.dll"));

        remove(&v, "MyMod.dll").unwrap();
        assert!(!bin.join("plugins/MyMod.dll").exists());
        assert!(read(&v).plugins.iter().all(|p| p.file != "MyMod.dll"));
    }

    #[test]
    fn catalogue_takes_only_safe_entries() {
        let sha = "ab".repeat(32);
        let c = ModCatalog::parse(&format!(
            r#"{{"schema":1,"mods":[
              {{"id":"good","name":"Good","file":"Good.dll","url":"mods/Good.dll","sha256":"{sha}"}},
              {{"id":"shipped","file":"NvrAssetPatches.dll","shipped":true}},
              {{"id":"nosha","file":"a.dll","url":"mods/a.dll"}},
              {{"id":"evil","file":"e.dll","url":"https://evil.example/e.dll","sha256":"{sha}"}},
              {{"id":"path","file":"../x.dll","shipped":true}},
              {{"id":"slot","file":"BugSplat64.dll","shipped":true}},
              {{"id":"exe","file":"run.exe","shipped":true}},
              {{"id":"good","file":"Again.dll","shipped":true}}]}}"#
        ))
        .unwrap();
        let ids: Vec<_> = c.mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["good", "shipped"]);
        assert!(c.mods[0].downloadable() && !c.mods[1].downloadable());
    }

    #[test]
    fn the_draft_catalogue_is_valid() {
        let draft = include_str!("../../../docs/launcher/mods.json");
        let raw: Value = serde_json::from_str(draft).unwrap();
        let c = ModCatalog::parse(draft).unwrap();
        assert_eq!(c.mods.len(), raw["mods"].as_array().unwrap().len());
        assert!(ModCatalog::builtin().builtin);
    }
}
