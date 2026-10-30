//! Echo VR game servers on this PC (the Server host plugin's page): each server runs the
//! live build with nEVR in a game folder of its own (`farm`), signs in as the server
//! account, registers with EchoVRCE and is started again after every match. The servers
//! run in a process of their own (`EchoVR_Launcher --server-host`, `supervisor`), so they
//! stay up while the window is closed; the window reads what they do from files.
//!
//! The way they run follows Mia's Linux servers (Echo-VR-Server-on-Docker): the runtime's
//! quirks (a port it ignores, a hang after Ctrl+C, a match per run) are handled the same
//! way. On Windows the game runs as it is, on Linux through the launcher's GE-Proton, each
//! server in a Wine prefix of its own.

pub mod account;
pub mod autostart;
pub mod config;
pub mod doctor;
pub mod events;
pub mod farm;
pub mod notify;
pub mod proc;
pub mod status;
pub mod supervisor;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::launcher::store::{InstalledVersion, LauncherState};
use super::paths;

/// The supervisor's flag.
pub const FLAG: &str = "--server-host";
/// The plugin whose page this is (its `plugin.json` says `"builtin": "server-host"`).
pub const PLUGIN_ID: &str = "server-host";

/// EchoVRCE's services, the defaults: the ones Mia's live servers (evr-docker) sign in and
/// register with, with a linked account (nEVR 5.4) and with a password alike. Without a
/// `socket_uri` a server never signs in at all; `wss://…:443/nevr` turns a linked account's
/// login away (no Bearer token: "service unavailable").
pub const SOCKET_URI: &str = "ws://g.echovrce.com:80/spr";
pub const SERVERDB_URI: &str = "ws://g.echovrce.com:80/nevr";
pub const HTTP_URI: &str = "https://g.echovrce.com:7350";
/// EchoVRCE's public keys: every game build carries them.
pub const HTTP_KEY: &str = "1c65a082-6712-4a02-8c6d-23d5c32f7409";
pub const SERVER_KEY: &str = "65675147-2a31-40e3-be21-98704de1b027";

/// The first server's port; the next ones count up from it.
pub const FIRST_PORT: u16 = 6792;
/// The game's HTTP API (`-httpport`, on 127.0.0.1) sits this far above the server's port.
pub const HTTP_PORT_OFFSET: u16 = 1000;
pub const TICK_RATE: u32 = 120;
/// At most this many servers.
pub const MAX_SERVERS: usize = 32;

/// Snapshots: made-up servers in a folder of their own instead.
static SNAP_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Snapshots: the servers' files are in `dir` from now on (not the player's).
pub fn snap_dir(dir: PathBuf) {
    let _ = SNAP_DIR.set(dir);
}

/// Where everything of the servers is: `<data>/server-host`.
pub fn dir() -> PathBuf {
    match SNAP_DIR.get() {
        Some(d) => d.clone(),
        None => paths::data_dir().join("server-host"),
    }
}

/// The servers and their settings.
pub fn config_path() -> PathBuf {
    dir().join("servers.json")
}

/// A server's own folder: its game folder (`root`), status, console log, control file.
pub fn server_dir(name: &str) -> PathBuf {
    dir().join("servers").join(name)
}

/// A server's game folder, in its folder `dir`.
pub fn farm_root(dir: &Path) -> PathBuf {
    dir.join("root")
}

/// Its `_local` (config, sign-in).
pub fn farm_local(dir: &Path) -> PathBuf {
    farm_root(dir).join("_local")
}

/// Server `name`'s `%LOCALAPPDATA%`: on Windows its own (in its folder), on Linux its
/// prefix's.
pub fn local_app_data(name: &str) -> Option<PathBuf> {
    let compat = crate::core::linux::echoxr::server_compat_data(&format!("host-{name}"));
    supervisor::local_app_data(&server_dir(name).join("appdata"), &compat)
}

/// Server `name`'s nEVR root, where nEVR 5.13 on keeps its sign-in (6.9.0 on also the one
/// its code got).
pub fn nevr_root(name: &str) -> Option<PathBuf> {
    local_app_data(name).map(|l| super::launcher::nevr_root::root_in(&l))
}

/// Whether the nEVR of `v` signs a server without an account in by a code it shows
/// (6.9.0 on): the servers need no account then.
pub fn signs_in_by_code(v: &InstalledVersion) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    // The page asks every few seconds: a runtime is read once (again when it changes).
    type Seen = HashMap<PathBuf, (std::time::SystemTime, u64, bool)>;
    static SEEN: Mutex<Option<Seen>> = Mutex::new(None);
    let runtime = super::launcher::nevr::runtime_path(&v.bin_dir());
    let Ok(meta) = std::fs::metadata(&runtime) else {
        return false;
    };
    let key = (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len());
    let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    let seen = seen.get_or_insert_with(HashMap::new);
    if let Some(&(m, l, yes)) = seen.get(&runtime) {
        if (m, l) == key {
            return yes;
        }
    }
    let yes =
        std::fs::read(&runtime).is_ok_and(|b| super::launcher::nevr::signs_servers_in_by_code(&b));
    seen.insert(runtime, (key.0, key.1, yes));
    yes
}

/// One game server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    /// Its name: lowercase letters, digits and `-` (also its folder's).
    pub name: String,
    /// Its UDP port (and TCP): forwarded 1:1 to this PC.
    pub port: u16,
    /// It should run (Start / Stop).
    pub run: bool,
    /// Where it registers (`uwu`, `euw`, …), comma-separated; empty: EchoVRCE's default.
    pub regions: String,
    /// The guilds it hosts for (ids), comma-separated; empty: any.
    pub guilds: String,
    /// Frames per second (10-1000).
    pub tick_rate: u32,
    /// Seconds an empty server stays in its match before going back to the lobby (0: off).
    pub empty_ttl: u32,
    /// The mods the Mods page has on load on it too (a content pack such as EchoCombat).
    pub mods: bool,
    /// More arguments for the game, space-separated.
    pub extra_args: String,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            name: String::new(),
            port: FIRST_PORT,
            run: false,
            regions: String::new(),
            guilds: String::new(),
            tick_rate: TICK_RATE,
            empty_ttl: 0,
            mods: true,
            extra_args: String::new(),
        }
    }
}

impl Server {
    /// The game's HTTP API port.
    pub fn http_port(&self) -> u16 {
        self.port.saturating_add(HTTP_PORT_OFFSET)
    }

    pub fn dir(&self) -> PathBuf {
        server_dir(&self.name)
    }
}

/// Where the servers sign in and register: EchoVRCE, unless another Nakama is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Services {
    pub socket_uri: String,
    pub serverdb_uri: String,
    pub http_uri: String,
    pub http_key: String,
    pub server_key: String,
}

impl Default for Services {
    fn default() -> Self {
        Services {
            socket_uri: SOCKET_URI.into(),
            serverdb_uri: SERVERDB_URI.into(),
            http_uri: HTTP_URI.into(),
            http_key: HTTP_KEY.into(),
            server_key: SERVER_KEY.into(),
        }
    }
}

/// How the servers sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SignIn {
    /// No account: from nEVR 6.9.0 on each server signs in by the code it shows the first
    /// time it starts (approved on echovrce.com) and keeps that sign-in in its nEVR root.
    /// Older versions can't start a server so.
    #[default]
    None,
    /// An EchoVRCE session linked for the servers (a device link): no password. Each
    /// server gets a session of its own from it.
    Device,
    /// The account's Discord ID and password.
    Password,
}

/// The server account as last seen (never a secret: those are in `account::secrets`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Account {
    pub sign_in: SignIn,
    pub discord_id: String,
    /// The account on the game service (a device link).
    pub user_id: String,
    pub username: String,
}

/// What the servers tell about themselves when something happens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Alerts {
    /// On the desktop.
    pub desktop: bool,
    /// A Discord webhook (empty: none).
    pub webhook: String,
}

impl Default for Alerts {
    fn default() -> Self {
        Alerts {
            desktop: true,
            webhook: String::new(),
        }
    }
}

/// `servers.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// The installed version the servers run (its id); empty: the live build the launcher
    /// has selected, else the first one.
    pub version: String,
    pub servers: Vec<Server>,
    /// What a new server starts with.
    pub defaults: Server,
    pub services: Services,
    /// The game's publisher lock (`_local/config.json`).
    pub publisher_lock: String,
    /// The address the servers bind (`network.internal_ip`); empty: nEVR's choice.
    pub internal_ip: String,
    /// The address they tell EchoVRCE (`network.external_ip`); empty: the one EchoVRCE
    /// sees them come from.
    pub external_ip: String,
    pub account: Account,
    pub alerts: Alerts,
}

impl Config {
    pub fn load() -> Config {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Config {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .map(Config::filled)
            .unwrap_or_else(|| Config::default().filled())
    }

    /// Empty values back to their defaults (and 0.12.3-beta.1's login address, which turns
    /// a linked account away, to the one that signs it in).
    fn filled(mut self) -> Config {
        if self.services.socket_uri.trim() == "wss://g.echovrce.com:443/nevr" {
            self.services.socket_uri = SOCKET_URI.into();
        }
        let d = Services::default();
        let s = &mut self.services;
        for (v, dv) in [
            (&mut s.socket_uri, d.socket_uri),
            (&mut s.http_uri, d.http_uri),
            (&mut s.http_key, d.http_key),
        ] {
            if v.trim().is_empty() {
                *v = dv;
            }
        }
        if self.publisher_lock.trim().is_empty() {
            self.publisher_lock = "echovrce".into();
        }
        self
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("create {}", d.display()))?;
        }
        let part = path.with_extension("json.part");
        std::fs::write(&part, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("write {}", part.display()))?;
        std::fs::rename(&part, path).with_context(|| format!("replace {}", path.display()))
    }

    pub fn server(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.name == name)
    }

    pub fn server_mut(&mut self, name: &str) -> Option<&mut Server> {
        self.servers.iter_mut().find(|s| s.name == name)
    }

    /// The next free name (`echo-N`).
    pub fn free_name(&self) -> String {
        (1..)
            .map(|n| format!("echo-{n}"))
            .find(|n| self.server(n).is_none())
            .unwrap_or_default()
    }

    /// The next free port from `from` (also leaving the others' HTTP ports alone).
    pub fn free_port(&self, from: u16) -> u16 {
        let taken = |p: u16| {
            self.servers
                .iter()
                .any(|s| s.port == p || s.http_port() == p || s.port == p + HTTP_PORT_OFFSET)
        };
        (from.max(1024)..=65535 - HTTP_PORT_OFFSET)
            .find(|&p| !taken(p))
            .unwrap_or(from)
    }

    /// Adds `n` servers with the defaults on free ports; their names.
    pub fn add(&mut self, n: usize) -> Result<Vec<String>> {
        if self.servers.len() + n > MAX_SERVERS {
            bail!("At most {MAX_SERVERS} servers.");
        }
        let mut names = Vec::new();
        for _ in 0..n {
            let from = self
                .servers
                .iter()
                .map(|s| s.port + 1)
                .max()
                .unwrap_or(self.defaults.port.max(1024));
            let s = Server {
                name: self.free_name(),
                port: self.free_port(from),
                run: false,
                ..self.defaults.clone()
            };
            names.push(s.name.clone());
            self.servers.push(s);
        }
        Ok(names)
    }

    /// Whether the servers sign in by the codes they show: no account is set, and the
    /// version they run does that (nEVR 6.9.0 on).
    pub fn by_code(&self, state: &LauncherState) -> bool {
        self.account.sign_in == SignIn::None && self.version_in(state).is_some_and(signs_in_by_code)
    }

    /// The installed version the servers run: the one set, else the selected live build,
    /// else the first live build.
    pub fn version_in<'a>(&self, state: &'a LauncherState) -> Option<&'a InstalledVersion> {
        let live = |v: &&InstalledVersion| v.publisher_lock.is_none() && v.present();
        if !self.version.is_empty() {
            return state.version(&self.version).filter(live);
        }
        state
            .selected
            .as_deref()
            .and_then(|id| state.version(id))
            .filter(live)
            .or_else(|| state.versions.iter().find(live))
    }
}

/// Pure: whether `name` can be a server's name.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=31).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Pure: a comma-separated list, trimmed, without empty entries.
pub fn list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Pure: what is wrong with `s` among `all` (the other servers): `None` when it is fine.
pub fn check_server(s: &Server, all: &[Server]) -> Option<String> {
    if !valid_name(&s.name) {
        return Some(format!(
            "{:?} can't be a server's name: lowercase letters, digits and - (up to 31).",
            s.name
        ));
    }
    if s.port < 1024 || s.port > 65535 - HTTP_PORT_OFFSET {
        return Some(format!(
            "Port {} can't be used: 1024 to {}.",
            s.port,
            65535 - HTTP_PORT_OFFSET
        ));
    }
    for o in all.iter().filter(|o| o.name != s.name) {
        if [o.port, o.http_port()].contains(&s.port)
            || [o.port, o.http_port()].contains(&s.http_port())
        {
            return Some(format!(
                "Port {} is {}'s already (each server also takes its port + {HTTP_PORT_OFFSET}).",
                s.port, o.name
            ));
        }
    }
    if !(10..=1000).contains(&s.tick_rate) {
        return Some(format!(
            "A tick rate of {} can't be used: 10 to 1000.",
            s.tick_rate
        ));
    }
    if s.empty_ttl > 86400 {
        return Some("An empty server stays at most 86400 seconds (a day).".into());
    }
    for r in list(&s.regions) {
        if r.chars().any(char::is_whitespace) {
            return Some(format!("{r:?} can't be a region."));
        }
    }
    for g in list(&s.guilds) {
        if !g.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Some(format!("{g:?} can't be a guild id."));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_lists() {
        assert!(valid_name("echo-1"));
        assert!(valid_name("1st"));
        assert!(!valid_name("-a"));
        assert!(!valid_name("Echo"));
        assert!(!valid_name(""));
        assert!(!valid_name(&"a".repeat(32)));
        assert_eq!(list(" euw, ,uwu "), vec!["euw", "uwu"]);
    }

    #[test]
    fn adds_servers_on_free_ports() {
        let mut c = Config::default().filled();
        assert_eq!(c.add(2).unwrap(), vec!["echo-1", "echo-2"]);
        assert_eq!(c.servers[0].port, 6792);
        assert_eq!(c.servers[1].port, 6793);
        c.servers.remove(0);
        assert_eq!(c.add(1).unwrap(), vec!["echo-1"]);
        assert_eq!(c.servers[1].port, 6794);
        assert!(c.add(MAX_SERVERS).is_err());
    }

    #[test]
    fn checks_a_server() {
        let mut c = Config::default();
        c.add(2).unwrap();
        let mut s = c.servers[1].clone();
        assert_eq!(check_server(&s, &c.servers), None);
        s.port = 6792;
        assert!(check_server(&s, &c.servers).unwrap().contains("echo-1"));
        s.port = 7792; // echo-1's HTTP port
        assert!(check_server(&s, &c.servers).is_some());
        s.port = 6800;
        s.tick_rate = 5;
        assert!(check_server(&s, &c.servers).is_some());
        s.tick_rate = 120;
        s.guilds = "123, a b".into();
        assert!(check_server(&s, &c.servers).is_some());
    }

    #[test]
    fn keeps_echovrce_unless_told() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("servers.json");
        std::fs::write(&p, r#"{"services": {"socket_uri": "  "}}"#).unwrap();
        let old = dir.path().join("old.json");
        std::fs::write(
            &old,
            r#"{"services": {"socket_uri": "wss://g.echovrce.com:443/nevr"}}"#,
        )
        .unwrap();
        assert_eq!(Config::load_from(&old).services.socket_uri, SOCKET_URI);
        let c = Config::load_from(&p);
        assert_eq!(c.services.socket_uri, SOCKET_URI);
        assert_eq!(c.publisher_lock, "echovrce");
        c.save_to(&p).unwrap();
        assert_eq!(Config::load_from(&p), c);
    }
}
