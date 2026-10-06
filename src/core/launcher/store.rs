//! The launcher's persistent state: `<data dir>/launcher.json`.
//!
//! Written atomically (temp file + rename) so a crash never leaves a half-written file,
//! and versioned so later milestones can migrate it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::catalog::{Catalog, VersionEntry};
use crate::core::paths;

pub const SCHEMA: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Runtime {
    /// echovr.exe on the Oculus/Meta runtime (Link, Air Link, Rift).
    #[default]
    MetaLink,
    /// echovr.exe while Virtual Desktop's streamer provides the Oculus runtime.
    VirtualDesktop,
    /// SteamVR headsets: on Windows through ReviveInjector (or EchoXR, `SteamVrVia`), on
    /// Linux through EchoXR on SteamVR's OpenXR runtime.
    Revive,
    /// Linux: through EchoXR on WiVRn's OpenXR runtime.
    Wivrn,
    /// No headset: `-noovr`.
    Flat,
}

impl Runtime {
    pub const ALL: [Runtime; 5] = [
        Runtime::MetaLink,
        Runtime::VirtualDesktop,
        Runtime::Revive,
        Runtime::Wivrn,
        Runtime::Flat,
    ];
}

/// How the SteamVR choice (`Runtime::Revive`) runs Echo VR on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SteamVrVia {
    /// Revive's injector (installed by the launcher, with administrator rights).
    #[default]
    Revive,
    /// EchoXR's OpenXR runtime in the game's folder: no injection, no administrator.
    EchoXr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LaunchProfile {
    pub runtime: Runtime,
    /// SteamVR (`Runtime::Revive`) through Revive or EchoXR.
    pub steamvr_via: SteamVrVia,
    /// Flat mode only: `-spectatorstream`.
    pub spectator: bool,
    /// `-windowed`
    pub windowed: bool,
    /// Extra arguments, split like a command line (quotes group).
    pub extra_args: String,
}

impl LaunchProfile {
    /// Whether hand tracking (`wanted`: EchoXR Hands is on) plays along with a start
    /// like this: in VR through EchoXR, whose OpenXR session its layer reads the fingers
    /// in. On Linux that's SteamVR or WiVRn; on Windows, SteamVR through EchoXR.
    pub fn hands(&self, wanted: bool) -> bool {
        let linux = cfg!(target_os = "linux");
        wanted
            && match self.runtime {
                Runtime::Revive => linux || self.steamvr_via == SteamVrVia::EchoXr,
                Runtime::Wivrn => linux,
                _ => false,
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InstalledVersion {
    /// Unique within the launcher; also the folder name for managed installs.
    pub id: String,
    pub name: String,
    /// Install root (the folder holding `ready-at-dawn-echo-arena`), normalized.
    pub root: String,
    /// Imported/added by the user: never moved or deleted by the launcher.
    pub external: bool,
    /// Catalogue entry it was installed from.
    pub catalog_id: Option<String>,
    pub update_manifest: Option<String>,
    pub installed_at: Option<String>,
    /// The licence patch (`pnsovr.dll`) is applied; updates leave that file alone.
    pub patched: bool,
    /// The executable's file name when it isn't `echovr.exe` (the 2017 builds'
    /// `EchoArena.exe`).
    pub exe: Option<String>,
    /// An event build, played on the classic lobbies relay as this build.
    pub publisher_lock: Option<String>,
    /// The sha256 of its update manifest when the launcher last brought it up to date
    /// (`core::updates`): another manifest means an update is out.
    pub manifest_sha256: Option<String>,
}

impl InstalledVersion {
    /// Its executable's file name.
    pub fn exe_name(&self) -> &str {
        self.exe.as_deref().unwrap_or(paths::DEFAULT_EXE)
    }

    /// Its executable.
    pub fn exe_path(&self) -> PathBuf {
        paths::exe_in(&self.root, self.exe_name())
    }

    /// The folder holding its executable, where updates and the licence patch go.
    pub fn bin_dir(&self) -> PathBuf {
        let exe = self.exe_path();
        exe.parent().map(Path::to_path_buf).unwrap_or(exe)
    }

    /// Whether its executable is there.
    pub fn present(&self) -> bool {
        !self.root.is_empty() && self.exe_path().is_file()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherState {
    pub schema: u32,
    /// Where managed versions are installed.
    pub library: String,
    pub versions: Vec<InstalledVersion>,
    pub selected: Option<String>,
    pub profile: LaunchProfile,
    pub last_lobby: String,
    /// The one-time import of pre-existing installs has run.
    pub imported: bool,
    /// Minimize the launcher window once Echo VR has started.
    pub minimize_on_launch: bool,
    /// Echo VR uses your own `_local/config.json`: with nEVR, the launcher doesn't set an
    /// EchoVRCE-era one aside (nEVR's built-in config, with friends and parties, is off).
    pub own_game_config: bool,
    /// The live PC update from another channel instead (a test one):
    /// `https://{release,files}.echovr.de/updates-*/update.manifest`. `None`: the live one.
    /// `ECHOVR_UPDATE_MANIFEST` comes first.
    pub update_manifest: Option<String>,
    /// Owns Echo VR on a Meta account (`Some(false)`: a new player, who needs the
    /// licence patch). `None` until asked: at an install, or a version's first PLAY.
    pub owner: Option<bool>,
    /// SteamVR setup also installs the game artwork for the SteamVR library.
    pub revive_artwork: bool,
    /// SteamVR setup also puts Echo VR into SteamVR's library.
    pub revive_library: bool,
    /// The Play page shows the launch options under PLAY.
    pub show_launch_options: bool,
    /// The Quest's Wi-Fi address: read over USB, typed, or found by a scan.
    pub quest_ip: Option<String>,
    /// Reach the Quest's ADB over the network too (turned on over USB once).
    pub quest_adb_network: bool,
    /// The EchoVRCE account echovrce.com inside the launcher was signed in for.
    pub vrce_site_account: Option<String>,
    /// Linux: the Steam shortcut that starts Echo VR (its appid), once set up.
    pub linux_appid: Option<u32>,
    /// spark:// links were turned off in Settings: don't register for them at start.
    pub spark_links_off: bool,
    /// The classic lobbies (EchoRelay) server the event builds play on, `host:port`.
    pub relay_server: String,
    /// Your account there, asked for at an event build's first PLAY.
    pub relay_account: Option<RelayAccount>,
    /// Event builds can be installed. On by default since 0.11.2 (also for a launcher.json
    /// from before, once: schema 1), except on Linux, where they don't start yet; off,
    /// they are listed as coming soon. `"event_builds": false` in this file turns them off.
    pub event_builds: bool,
    /// EchoXR Hands is on (Mods page): it plays along whenever EchoXR runs on SteamVR.
    pub echoxr_hands: bool,
    /// The rail is unfolded: the tabs' names beside their icons (new players start so).
    pub rail_open: bool,
    /// Updates found are announced on the desktop too.
    pub desktop_notifications: bool,
    /// The tray keeps looking for updates while the launcher is closed.
    pub tray: bool,
    /// The tray starts at login.
    pub tray_at_login: bool,
}

/// An account on the classic lobbies server. The password sits in the game's own config
/// in plain text too (the game reads it from there), so it isn't hidden here either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RelayAccount {
    pub name: String,
    pub password: String,
}

/// What the PLAY button acts on: the selected version, installed or not.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Installed(InstalledVersion),
    /// Installed, but `echovr.exe` is gone from its folder.
    Missing(InstalledVersion),
    /// A catalogue version that is not installed yet.
    Available(VersionEntry),
    None,
}

impl Default for LauncherState {
    fn default() -> Self {
        LauncherState {
            schema: SCHEMA,
            library: default_library(),
            versions: Vec::new(),
            selected: None,
            profile: LaunchProfile::default(),
            last_lobby: String::new(),
            imported: false,
            minimize_on_launch: true,
            own_game_config: false,
            update_manifest: None,
            owner: None,
            revive_artwork: true,
            revive_library: true,
            show_launch_options: false,
            quest_ip: None,
            quest_adb_network: false,
            vrce_site_account: None,
            linux_appid: None,
            spark_links_off: false,
            relay_server: super::relay::DEFAULT_SERVER.into(),
            event_builds: EVENT_BUILDS_DEFAULT,
            echoxr_hands: false,
            rail_open: true,
            desktop_notifications: true,
            tray: true,
            tray_at_login: false,
            relay_account: None,
        }
    }
}

/// Event builds are offered unless turned off, except on Linux (they don't start there yet).
pub const EVENT_BUILDS_DEFAULT: bool = !cfg!(target_os = "linux");

pub fn default_library() -> String {
    if cfg!(windows) {
        "C:/EchoVR/versions".into()
    } else {
        paths::normalize(&paths::data_dir().join("versions").to_string_lossy())
    }
}

pub fn state_file() -> PathBuf {
    paths::data_dir().join("launcher.json")
}

impl LauncherState {
    pub fn load() -> LauncherState {
        let s = Self::load_upgraded(&state_file());
        crate::core::pc_update::set_channel(s.update_manifest.as_deref());
        s
    }

    /// [`Self::load_from`], and an older schema brought up to date is saved at once, so a
    /// later edit of the file isn't undone by the next start.
    pub fn load_upgraded(path: &Path) -> LauncherState {
        let s = Self::load_from(path);
        if Self::schema_on_disk(path).is_some_and(|v| v < SCHEMA) {
            if let Err(e) = s.save_to(path) {
                tracing::warn!("launcher.json not upgraded: {e:#}");
            }
        }
        s
    }

    /// The schema of the launcher.json at `path`, if it has one.
    fn schema_on_disk(path: &Path) -> Option<u32> {
        let text = std::fs::read_to_string(path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        v.get("schema")?.as_u64().map(|n| n as u32)
    }

    pub fn load_from(path: &Path) -> LauncherState {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<LauncherState>(&text)
                .map(LauncherState::migrated)
                .unwrap_or_else(|e| {
                    // Keep the broken file for inspection instead of silently losing it.
                    tracing::error!("launcher.json unreadable ({e}); starting fresh");
                    let _ = std::fs::rename(path, path.with_extension("json.broken"));
                    LauncherState::default()
                }),
            Err(_) => LauncherState::default(),
        }
    }

    /// A launcher.json of an older schema brought up to this one: schema 1 had event
    /// builds off by default, so they are turned on (where they are offered) once.
    pub fn migrated(mut self) -> LauncherState {
        if self.schema < 2 {
            self.event_builds = self.event_builds || EVENT_BUILDS_DEFAULT;
        }
        self.schema = self.schema.max(SCHEMA);
        self
    }

    pub fn save(&self) -> Result<()> {
        crate::core::pc_update::set_channel(self.update_manifest.as_deref());
        self.save_to(&state_file())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let dir = path.parent().context("state file has no parent")?;
        std::fs::create_dir_all(dir)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    pub fn version(&self, id: &str) -> Option<&InstalledVersion> {
        self.versions.iter().find(|v| v.id == id)
    }

    /// The PLAY target: the selected installed version, else the selected catalogue PC
    /// version, else the first installed one, else the first catalogue PC version.
    /// `present` tells whether an installed version still has its game files.
    pub fn target(
        &self,
        catalog: Option<&Catalog>,
        present: impl Fn(&InstalledVersion) -> bool,
    ) -> Target {
        let installed = |v: &InstalledVersion| {
            if present(v) {
                Target::Installed(v.clone())
            } else {
                Target::Missing(v.clone())
            }
        };
        let available = |id: &str| {
            catalog
                .and_then(|c| c.pc().find(|e| e.id == id))
                .map(|e| Target::Available(e.clone()))
        };
        if let Some(id) = self.selected.as_deref() {
            if let Some(v) = self.version(id) {
                return installed(v);
            }
            if let Some(t) = available(id) {
                return t;
            }
        }
        if let Some(v) = self.versions.first() {
            return installed(v);
        }
        catalog
            .and_then(|c| c.pc().next())
            .map(|e| Target::Available(e.clone()))
            .unwrap_or(Target::None)
    }

    /// The installed copy of catalogue version `id`, if there is one.
    /// Whether `e` can be installed: it's on the servers and, for an event build, event
    /// builds are on (`event_builds`) or it is installed already.
    pub fn offers(&self, e: &VersionEntry) -> bool {
        e.downloadable()
            && (e.publisher_lock.is_none()
                || self.event_builds
                || self.installed_from(&e.id).is_some())
    }

    /// Whether an event build is installed, or can be.
    pub fn has_event_builds(&self) -> bool {
        self.event_builds || self.versions.iter().any(|v| v.publisher_lock.is_some())
    }

    pub fn installed_from(&self, id: &str) -> Option<&InstalledVersion> {
        self.versions
            .iter()
            .find(|v| v.id == id || v.catalog_id.as_deref() == Some(id))
    }

    /// Adds or replaces (by id) a version, keeping the list stable.
    pub fn upsert(&mut self, v: InstalledVersion) {
        match self.versions.iter_mut().find(|x| x.id == v.id) {
            Some(x) => *x = v,
            None => self.versions.push(v),
        }
    }

    pub fn has_root(&self, root: &str) -> bool {
        let n = paths::normalize(root);
        self.versions
            .iter()
            .any(|v| paths::normalize(&v.root).eq_ignore_ascii_case(&n))
    }

    /// An id not yet used, derived from `base`.
    pub fn free_id(&self, base: &str) -> String {
        let base: String = base
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let base = if base.is_empty() {
            "version".to_string()
        } else {
            base
        };
        if self.version(&base).is_none() {
            return base;
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| self.version(id).is_none())
            .expect("infinite")
    }

    /// Registers pre-existing installs once: Echo VR in the Meta library. Returns how many
    /// were added.
    pub fn import_existing(&mut self) -> usize {
        let mut candidates = Vec::new();
        if let Some(base) = crate::core::platform::oculus_base_path() {
            let sep = if base.ends_with(['\\', '/']) { "" } else { "/" };
            candidates.push(format!("{base}{sep}Software/Software"));
        }
        let mut added = 0;
        for c in candidates {
            let root = paths::normalize(&paths::resolve_install_root(&c));
            if paths::has_echo_install(&root) && self.add_external(&root, None).is_some() {
                added += 1;
            }
        }
        self.imported = true;
        added
    }

    /// Adds an existing folder as an external version. `None` if it is already known.
    pub fn add_external(&mut self, root: &str, name: Option<String>) -> Option<String> {
        let root = paths::normalize(root);
        if self.has_root(&root) {
            return None;
        }
        let id = self.free_id("existing");
        // A 2017 build starts EchoArena.exe.
        let exe = paths::find_exe(&root)
            .filter(|e| *e != paths::DEFAULT_EXE)
            .map(str::to_string);
        // No update named: a live build's folder gets the live update anyway, and an
        // older build's never gets the live files written into it.
        self.versions.push(InstalledVersion {
            id: id.clone(),
            name: name.unwrap_or_else(|| format!("Existing install ({root})")),
            root,
            external: true,
            exe,
            ..Default::default()
        });
        if self.selected.is_none() {
            self.selected = Some(id.clone());
        }
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("launcher.json");
        assert_eq!(LauncherState::load_from(&f), LauncherState::default());
        let mut s = LauncherState::default();
        s.profile.runtime = Runtime::Revive;
        s.upsert(InstalledVersion {
            id: "a".into(),
            root: "/x".into(),
            ..Default::default()
        });
        s.save_to(&f).unwrap();
        assert_eq!(LauncherState::load_from(&f), s);
        // Unknown/missing fields are tolerated.
        std::fs::write(&f, r#"{"schema":1,"library":"/lib","future":true}"#).unwrap();
        assert_eq!(LauncherState::load_from(&f).library, "/lib");
    }

    #[test]
    fn broken_file_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("launcher.json");
        std::fs::write(&f, "{not json").unwrap();
        assert_eq!(LauncherState::load_from(&f), LauncherState::default());
        assert!(dir.path().join("launcher.json.broken").exists());
    }

    #[test]
    fn ids_and_external_roots() {
        let mut s = LauncherState::default();
        assert_eq!(s.free_id("Echo 34.4 (PC)"), "echo-34.4--pc-");
        let a = s.add_external("C:\\Games\\Echo\\", None).unwrap();
        assert_eq!(a, "existing");
        assert!(
            s.add_external("C:/Games/Echo", None).is_none(),
            "same root twice"
        );
        assert_eq!(s.add_external("D:/Echo", None).unwrap(), "existing-2");
        assert_eq!(s.selected.as_deref(), Some("existing"));
    }

    #[test]
    fn hand_tracking_plays_along_through_echoxr() {
        let mut p = LaunchProfile {
            runtime: Runtime::Revive,
            steamvr_via: SteamVrVia::EchoXr,
            ..Default::default()
        };
        assert!(p.hands(true));
        assert!(!p.hands(false));
        p.runtime = Runtime::Flat;
        assert!(!p.hands(true));
        // WiVRn and SteamVR through Revive: only on Linux, where EchoXR plays both.
        p.runtime = Runtime::Wivrn;
        assert_eq!(p.hands(true), cfg!(target_os = "linux"));
        p.runtime = Runtime::Revive;
        p.steamvr_via = SteamVrVia::Revive;
        assert_eq!(p.hands(true), cfg!(target_os = "linux"));
    }

    #[test]
    fn event_builds_are_on_unless_turned_off() {
        let c = Catalog::builtin();
        let event = c.pc().find(|e| e.publisher_lock.is_some()).unwrap().clone();
        let live = c.pc().find(|e| e.publisher_lock.is_none()).unwrap();
        // A fresh launcher offers them (not on Linux, where they don't start yet).
        let s = LauncherState::default();
        assert!(s.offers(live));
        assert_eq!(s.offers(&event), EVENT_BUILDS_DEFAULT);
        // Turned off: coming soon.
        let mut s = LauncherState {
            event_builds: false,
            ..Default::default()
        };
        assert!(!s.offers(&event) && !s.has_event_builds());
        // Installed before: still offered (reinstall).
        s.upsert(InstalledVersion {
            id: event.id.clone(),
            root: "/e".into(),
            publisher_lock: event.publisher_lock.clone(),
            ..Default::default()
        });
        assert!(s.offers(&event) && s.has_event_builds());
    }

    #[test]
    fn an_older_launcher_json_gets_event_builds_once() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("launcher.json");
        // Saved by 0.11.1 or older, with them off (the default then).
        std::fs::write(&f, r#"{"schema":1,"event_builds":false}"#).unwrap();
        let s = LauncherState::load_from(&f);
        assert_eq!((s.event_builds, s.schema), (EVENT_BUILDS_DEFAULT, SCHEMA));
        // Turned off since: stays off.
        std::fs::write(&f, r#"{"schema":2,"event_builds":false}"#).unwrap();
        assert!(!LauncherState::load_from(&f).event_builds);
        // The upgrade is saved at once.
        std::fs::write(&f, r#"{"schema":1,"event_builds":false}"#).unwrap();
        LauncherState::load_upgraded(&f);
        assert_eq!(LauncherState::schema_on_disk(&f), Some(SCHEMA));
        assert_eq!(
            LauncherState::load_from(&f).event_builds,
            EVENT_BUILDS_DEFAULT
        );
    }

    fn catalog() -> Catalog {
        let mut c = Catalog::builtin();
        c.versions.push(VersionEntry {
            id: "pc-old".into(),
            name: "Old".into(),
            ..Default::default()
        });
        c
    }

    #[test]
    fn targets() {
        let c = catalog();
        let mut s = LauncherState::default();
        // Nothing installed: the first catalogue PC version.
        assert!(
            matches!(s.target(Some(&c), |_| true), Target::Available(e) if e.id == "pc-latest")
        );
        assert_eq!(s.target(None, |_| true), Target::None);
        // A selected catalogue version.
        s.selected = Some("pc-old".into());
        assert!(matches!(s.target(Some(&c), |_| true), Target::Available(e) if e.id == "pc-old"));
        assert!(s.installed_from("pc-old").is_none());
        // Installed and selected; its folder may be gone.
        s.upsert(InstalledVersion {
            id: "pc-old".into(),
            root: "/lib/pc-old".into(),
            catalog_id: Some("pc-old".into()),
            ..Default::default()
        });
        assert!(matches!(s.target(Some(&c), |_| true), Target::Installed(v) if v.id == "pc-old"));
        assert!(matches!(s.target(Some(&c), |_| false), Target::Missing(_)));
        assert_eq!(s.installed_from("pc-old").unwrap().root, "/lib/pc-old");
        // An unknown selection falls back to the first installed version.
        s.selected = Some("gone".into());
        assert!(matches!(s.target(Some(&c), |_| true), Target::Installed(v) if v.id == "pc-old"));
    }
}
