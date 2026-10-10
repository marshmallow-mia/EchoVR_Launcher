//! nEVR runtime (github.com/EchoTools/nevr-runtime) in the live PC build's slot,
//! `bin/win10/BugSplat64.dll`. It points the game at EchoVRCE, signs it in, brings friends
//! and parties into the game, and loads the plugins `_local/config.yaml` lists from
//! `plugins/`. The launcher:
//! - writes that `config.yaml` before every start (the Mods page's choices);
//! - hands the game a sign-in of its own (`_local/.credentials.json`, a device of the
//!   EchoVRCE account the launcher is signed in with), so it never asks in the browser;
//! - reads what nEVR did with the plugins from its log
//!   (`%LOCALAPPDATA%\EchoVR\logs\nevr-<time>.jsonl`).
//!
//! nEVR looks for `_local` beside the game's exe, then one and two folders up (the
//! game's own `_local`, where the launcher writes); the first `config.yaml` found wins.
//! It refuses to start with a `dbgcore.dll` beside the exe (EchoLoader 1's place); one in
//! `plugins/` is fine.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::store::InstalledVersion;
use crate::core::paths;

/// What every nEVR build logs at its start.
pub const MARKER: &[u8] = b"[NEVR.BOOT]";
pub const CONFIG: &str = "config.yaml";
pub const CREDENTIALS: &str = ".credentials.json";
/// The game's EchoVRCE sign-in while it plays on another server ([`Backend`]): nEVR tries
/// a sign-in it finds before the config's account, so it waits beside it.
pub const CREDENTIALS_ASIDE: &str = ".credentials.json.echovrce";
/// The file nEVR refuses to start beside.
pub const DBGCORE: &str = "dbgcore.dll";
/// The game's own config, which nEVR doesn't need: without one it supplies its built-in
/// one, which turns on friends, parties, presence and matchmaking.
pub const GAME_CONFIG: &str = "config.json";
/// Where a game config nEVR replaces is kept.
pub const GAME_CONFIG_ASIDE: &str = "config.json.pre-nevr";
/// What the EchoRelay-era game config held: the service hosts and the build lock.
const SERVICE_KEYS: [&str; 9] = [
    "apiservice_host",
    "configservice_host",
    "loginservice_host",
    "matchingservice_host",
    "serverdb_host",
    "transactionservice_host",
    "graphservice_host",
    "publisher_lock",
    "api_host",
];

/// A game sign-in with less than this left is replaced before it runs out.
const LOGIN_MARGIN_S: i64 = 24 * 3600;
/// A refresh token's life when it doesn't say (EchoVRCE's are 30 days).
const REFRESH_LIFE_S: i64 = 30 * 24 * 3600;

// ---- the DLL ----

/// Pure: nEVR's version in its DLL `bytes`: its build identity (`4.0.0+182.e418eaa`), else
/// its `git describe` (`v4.0.0-182-ge418eaa-dirty`) without the `v`.
pub fn version_in(bytes: &[u8]) -> Option<String> {
    let runs = || {
        bytes
            .split(|b| !(0x20..0x7f).contains(b))
            .filter(|r| (5..=48).contains(&r.len()))
            .filter_map(|r| std::str::from_utf8(r).ok())
    };
    runs()
        .find(|s| build_identity(s))
        .or_else(|| runs().find(|s| describe(s)).map(|s| &s[1..]))
        .map(str::to_string)
}

/// `X.Y.Z+N.hash`.
fn build_identity(s: &str) -> bool {
    let Some((ver, build)) = s.split_once('+') else {
        return false;
    };
    let Some((n, hash)) = build.split_once('.') else {
        return false;
    };
    semver(ver)
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
        && (7..=40).contains(&hash.len())
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `vX.Y.Z`, `vX.Y.Z-N-ghash`, either with `-dirty`.
fn describe(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('v') else {
        return false;
    };
    let rest = rest.strip_suffix("-dirty").unwrap_or(rest);
    let mut parts = rest.splitn(3, '-');
    let ver = parts.next().unwrap_or_default();
    match (parts.next(), parts.next()) {
        (None, None) => semver(ver),
        (Some(n), Some(g)) => {
            semver(ver)
                && n.bytes().all(|b| b.is_ascii_digit())
                && g.strip_prefix('g')
                    .is_some_and(|h| h.len() >= 7 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        }
        _ => false,
    }
}

fn semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether the game folder `bin` has a `dbgcore.dll` beside the exe, which nEVR refuses.
pub fn stray_dbgcore(bin: &Path) -> bool {
    bin.join(DBGCORE).is_file()
}

// ---- where nEVR looks ----

/// The game's own `_local`, where the launcher writes.
pub fn local_dir(v: &InstalledVersion) -> PathBuf {
    Path::new(&v.root).join(paths::ARENA_DIR).join("_local")
}

/// The `_local` folders nEVR looks in, in its order: beside the exe, one up, two up.
fn search(v: &InstalledVersion) -> [PathBuf; 3] {
    let bin = v.bin_dir();
    let up = bin.parent().map(Path::to_path_buf).unwrap_or(bin.clone());
    let up2 = up.parent().map(Path::to_path_buf).unwrap_or(up.clone());
    [bin.join("_local"), up.join("_local"), up2.join("_local")]
}

/// The game sign-ins the launcher wrote for `v` (in each `_local` nEVR looks in).
pub fn logins(v: &InstalledVersion) -> Vec<PathBuf> {
    search(v)
        .into_iter()
        .map(|d| d.join(CREDENTIALS))
        .filter(|p| p.is_file())
        .collect()
}

/// A `config.yaml` nEVR reads instead of the launcher's (one nearer the exe), if any.
pub fn shadowing_config(v: &InstalledVersion) -> Option<PathBuf> {
    let ours = local_dir(v).join(CONFIG);
    search(v)
        .into_iter()
        .map(|d| d.join(CONFIG))
        .take_while(|p| !same_file(p, &ours))
        .find(|p| p.is_file())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Pure: whether a game config (its text) is an obsolete EchoVRCE one: nothing but the
/// EchoRelay-era service settings, every host on echovrce.com (the archive's, and the
/// personalized copies with a Discord id in the login host). One pointing at another
/// server, or with anything else in it, is someone's own.
pub fn obsolete_echovrce_config(text: &str) -> bool {
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    o.iter().all(|(k, v)| {
        SERVICE_KEYS.contains(&k.as_str())
            && (!k.ends_with("_host") || v.as_str().is_some_and(on_echovrce))
    })
}

/// Whether `url`'s host is echovrce.com or one of its subdomains.
fn on_echovrce(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|h| h == "echovrce.com" || h.ends_with(".echovrce.com"))
}

/// What [`arrange_game_config`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameConfigStep {
    Nothing,
    /// An obsolete EchoVRCE config went to `config.json.pre-nevr`.
    SetAside,
    /// "Use my own config.json": the one set aside earlier is back.
    Restored,
}

/// Before a start of `v` with nEVR in its slot: with `own` (Settings, Launch options: "Use
/// my own config.json") the game's config stays, and one set aside earlier is put back;
/// otherwise an obsolete EchoVRCE config is set aside, so nEVR uses its built-in one
/// (friends and parties on). Every `_local` nEVR looks in counts.
pub fn arrange_game_config(v: &InstalledVersion, own: bool) -> Result<GameConfigStep> {
    if own {
        return restore_game_config(v);
    }
    let mut step = GameConfigStep::Nothing;
    for dir in search(v) {
        let path = dir.join(GAME_CONFIG);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !obsolete_echovrce_config(&text) {
            continue;
        }
        std::fs::rename(&path, dir.join(GAME_CONFIG_ASIDE))
            .with_context(|| format!("move {} aside", path.display()))?;
        step = GameConfigStep::SetAside;
    }
    Ok(step)
}

/// Puts a game config set aside for nEVR back where it was (when that place has none):
/// for "Use my own config.json", and before a start without nEVR, which needs it.
pub fn restore_game_config(v: &InstalledVersion) -> Result<GameConfigStep> {
    let mut step = GameConfigStep::Nothing;
    for dir in search(v) {
        let (path, aside) = (dir.join(GAME_CONFIG), dir.join(GAME_CONFIG_ASIDE));
        if !path.exists() && aside.is_file() {
            std::fs::rename(&aside, &path)
                .with_context(|| format!("put {} back", path.display()))?;
            step = GameConfigStep::Restored;
        }
    }
    Ok(step)
}

/// The game config nEVR loads for `v` (the first `config.json` in its search), if any.
pub fn game_config_in_use(v: &InstalledVersion) -> Option<PathBuf> {
    search(v)
        .into_iter()
        .map(|d| d.join(GAME_CONFIG))
        .find(|p| p.is_file())
}

// ---- config.yaml ----

/// A plugin as `config.yaml` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginLine {
    pub file: String,
    pub enabled: bool,
    pub args: Map<String, Value>,
    /// Loaded in nEVR's early pass, before the game reads its data (a content pack's
    /// overlay; see [`super::packs`]).
    pub early: bool,
}

/// Pure: whether nEVR reads `text` as an environment variable: a `${` with a `}` after it
/// (an unterminated `${` it keeps as it is). nEVR has no escape for it, and an unset
/// variable makes it drop the whole `config.yaml`, so a client loads no plugins at all.
pub fn expands_env(text: &str) -> bool {
    text.find("${").is_some_and(|i| text[i + 2..].contains('}'))
}

/// Pure: whether a `config.yaml` turns local (unverified) plugins on: a top-level
/// `x-local-plugins: true` (or yes / on).
pub fn local_plugins_in(text: &str) -> bool {
    text.lines().any(|l| {
        l.strip_prefix(super::mods::LOCAL_PLUGINS_KEY)
            .and_then(|r| r.trim_start().strip_prefix(':'))
            .map(|v| {
                v.split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(['"', '\''])
            })
            .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "on"))
    })
}

/// Whether `v`'s loader config turns local plugins on.
pub fn local_plugins(v: &InstalledVersion) -> bool {
    std::fs::read_to_string(local_dir(v).join(CONFIG)).is_ok_and(|t| local_plugins_in(&t))
}

/// A server other than EchoVRCE that a start plays on (a content pack's own, Settings):
/// its game socket, and the account there. nEVR logs in, matchmakes and (as a dedicated
/// server) registers there instead.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Backend {
    /// `ws://host:7350/ws?format=evr&token=<server key>`.
    pub socket_uri: String,
    pub server_key: String,
    pub discord_id: String,
    pub password: String,
}

/// Nakama's port, when an address leaves it out.
const BACKEND_PORT: u16 = 7350;

/// Pure: the game socket for `address` as a server's host gives it out: a `ws://` or
/// `wss://` address as it is, or `host[:port]` on Nakama's `/ws`; asking for the game's
/// own message format, and with `key` as its token when it carries none. `None`: not
/// an address.
pub fn socket_uri(address: &str, key: &str) -> Option<String> {
    let a = address.trim();
    let mut url = if a.starts_with("ws://") || a.starts_with("wss://") {
        url::Url::parse(a).ok()?
    } else {
        if a.is_empty() || a.contains(['/', '?', '#', ' ']) {
            return None;
        }
        let with_port = if a.contains(':') {
            a.to_string()
        } else {
            format!("{a}:{BACKEND_PORT}")
        };
        url::Url::parse(&format!("ws://{with_port}/ws")).ok()?
    };
    url.host_str().filter(|h| !h.is_empty())?;
    let has = |k: &str| url.query_pairs().any(|(n, _)| n == k);
    let mut add = Vec::new();
    if !has("format") {
        add.push(("format", "evr"));
    }
    if !key.trim().is_empty() && !has("token") {
        add.push(("token", key.trim()));
    }
    if !add.is_empty() {
        url.query_pairs_mut().extend_pairs(add);
    }
    Some(url.to_string())
}

/// Pure: the launcher's `config.yaml`: the plugins (none with mods off), the local
/// plugins switch when it is on, and another server than EchoVRCE when this start plays
/// on one. Values are written as JSON, which YAML reads as it is.
pub fn render_config(plugins: &[PluginLine], local: bool, backend: Option<&Backend>) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let mut out = render_plugins(plugins, local);
    if let Some(b) = backend {
        // nEVR reads ${…} in any value as an environment variable (and drops the whole
        // file when one isn't set): $${ is a literal ${.
        let v = |s: &str| q(&s.replace("${", "$${"));
        out.push_str("# The server this start plays on (Settings), instead of EchoVRCE.\n");
        out.push_str(&format!("services:\n  socket_uri: {}\n", v(&b.socket_uri)));
        if !b.discord_id.is_empty() {
            out.push_str(&format!("identity:\n  discord_id: {}\n", v(&b.discord_id)));
        }
        let mut auth = String::new();
        if !b.password.is_empty() {
            auth.push_str(&format!("  password: {}\n", v(&b.password)));
        }
        if !b.server_key.is_empty() {
            auth.push_str(&format!("  server_key: {}\n", v(&b.server_key)));
        }
        if !auth.is_empty() {
            out.push_str("auth:\n");
            out.push_str(&auth);
        }
    }
    out
}

/// Pure: the plugins part of [`render_config`].
fn render_plugins(plugins: &[PluginLine], local: bool) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let mut out = String::from(
        "# Written by the Echo VR launcher before every start, from its Mods page.\n\
         # Changes here are replaced: change mods in the launcher. Kept: the line below,\n\
         # x-local-plugins: true, which lets plugins load that aren't verified.\n",
    );
    if local {
        out.push_str(&format!("{}: true\n", super::mods::LOCAL_PLUGINS_KEY));
    }
    if plugins.is_empty() {
        out.push_str("plugins: []\n");
        return out;
    }
    out.push_str("plugins:\n");
    for p in plugins {
        let name = p.file.rsplit_once('.').map_or(p.file.as_str(), |(s, _)| s);
        out.push_str(&format!("  - name: {}\n", q(name)));
        out.push_str(&format!("    file: {}\n", q(&p.file)));
        out.push_str(&format!("    enabled: {}\n", p.enabled));
        if p.early {
            out.push_str("    early: true\n");
        }
        if !p.args.is_empty() {
            out.push_str(&format!("    args: {}\n", Value::Object(p.args.clone())));
        }
    }
    out
}

/// Writes `v`'s `config.yaml` (only when it changed).
pub fn write_config(
    v: &InstalledVersion,
    plugins: &[PluginLine],
    local: bool,
    backend: Option<&Backend>,
) -> Result<()> {
    let dir = local_dir(v);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(CONFIG);
    let text = render_config(plugins, local, backend);
    if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
        return Ok(());
    }
    let part = path.with_extension("yaml.part");
    std::fs::write(&part, &text).with_context(|| format!("Couldn't write {}", part.display()))?;
    std::fs::rename(&part, &path).with_context(|| {
        let _ = std::fs::remove_file(&part);
        format!("Couldn't replace {}", path.display())
    })
}

// ---- the game's sign-in ----

/// What nEVR keeps of its EchoVRCE sign-in: the refresh token (it never stores the
/// session token), until when it is good (Unix seconds), and whose it is.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GameLogin {
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub refresh_token_expiry: i64,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub username: String,
}

impl std::fmt::Debug for GameLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print a token.
        f.debug_struct("GameLogin")
            .field("refresh_token_expiry", &self.refresh_token_expiry)
            .field("user_id", &self.user_id)
            .finish()
    }
}

impl GameLogin {
    /// A sign-in for the game from a session linked for it: `refresh_token` (a JWT, whose
    /// expiry it carries) for `account`.
    pub fn new(refresh_token: &str, user_id: &str, username: &str, now: i64) -> GameLogin {
        GameLogin {
            refresh_token: refresh_token.to_string(),
            refresh_token_expiry: crate::core::echovrce::jwt_exp(refresh_token)
                .unwrap_or(now + REFRESH_LIFE_S),
            user_id: user_id.to_string(),
            username: username.to_string(),
        }
    }

    /// Whether it is `user_id`'s and good for a while yet.
    pub fn good_for(&self, user_id: &str, now: i64) -> bool {
        self.user_id == user_id
            && !self.refresh_token.is_empty()
            && self.refresh_token_expiry - now > LOGIN_MARGIN_S
    }
}

/// The sign-in nEVR would use for `v` (the first `.credentials.json` in its search), or
/// the one waiting while it plays on another server.
pub fn read_login(v: &InstalledVersion) -> Option<GameLogin> {
    [CREDENTIALS, CREDENTIALS_ASIDE]
        .into_iter()
        .flat_map(|name| search(v).map(|d| d.join(name)))
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
}

/// Before a start of `v` on another server than EchoVRCE: its EchoVRCE sign-ins wait
/// beside them (nEVR would try them there first). Returns whether one did.
pub fn login_aside(v: &InstalledVersion) -> Result<bool> {
    let mut moved = false;
    for d in search(v) {
        let (path, aside) = (d.join(CREDENTIALS), d.join(CREDENTIALS_ASIDE));
        if path.is_file() {
            // Windows won't replace a hidden file (nEVR hides it): the old one goes first.
            if aside.is_file() {
                std::fs::remove_file(&aside)
                    .with_context(|| format!("remove {}", aside.display()))?;
            }
            std::fs::rename(&path, &aside)
                .with_context(|| format!("move {} aside", path.display()))?;
            moved = true;
        }
    }
    Ok(moved)
}

/// Before a start of `v` on EchoVRCE: a sign-in set aside is back (unless a newer one is
/// there already). Returns whether one came back.
pub fn login_back(v: &InstalledVersion) -> Result<bool> {
    let mut back = false;
    for d in search(v) {
        let (path, aside) = (d.join(CREDENTIALS), d.join(CREDENTIALS_ASIDE));
        if !aside.is_file() {
            continue;
        }
        if path.is_file() {
            std::fs::remove_file(&aside).with_context(|| format!("remove {}", aside.display()))?;
        } else {
            std::fs::rename(&aside, &path)
                .with_context(|| format!("put {} back", path.display()))?;
            back = true;
        }
    }
    Ok(back)
}

/// Gives `v`'s game `login`: in the game's `_local`, beside `config.yaml`, with any other
/// sign-in nEVR would read first removed.
pub fn write_login(v: &InstalledVersion, login: &GameLogin) -> Result<()> {
    let dir = local_dir(v);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let ours = dir.join(CREDENTIALS);
    for p in search(v)
        .into_iter()
        .flat_map(|d| [d.join(CREDENTIALS), d.join(CREDENTIALS_ASIDE)])
    {
        // nEVR hides the file, and Windows won't overwrite a hidden file in place.
        if p.is_file() {
            std::fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    let text = serde_json::to_string_pretty(login)?;
    write_private(&ours, text.as_bytes())
}

/// Removes `v`'s game sign-ins that are `user_id`'s (signing out of the launcher signs
/// the game out too).
pub fn forget_login(v: &InstalledVersion, user_id: &str) {
    for p in search(v)
        .into_iter()
        .flat_map(|d| [d.join(CREDENTIALS), d.join(CREDENTIALS_ASIDE)])
    {
        let theirs = std::fs::read_to_string(&p)
            .ok()
            .and_then(|t| serde_json::from_str::<GameLogin>(&t).ok())
            .is_some_and(|l| l.user_id == user_id);
        if theirs {
            let _ = std::fs::remove_file(&p);
        }
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("Couldn't write {}", path.display()))?;
    f.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("Couldn't write {}", path.display()))
}

// ---- what it did ----

/// Where nEVR logs: `%LOCALAPPDATA%\EchoVR\logs` (on Linux, the Wine prefix's).
pub fn log_dir() -> Option<PathBuf> {
    let local = if cfg!(target_os = "linux") {
        Some(crate::core::linux::echoxr::local_app_data())
    } else {
        dirs::data_local_dir()
    };
    local.map(|d| d.join("EchoVR").join("logs"))
}

/// Its log of a start's first moments, beside the exe.
pub fn boot_log(bin: &Path) -> PathBuf {
    bin.join("logs").join("nevr-boot.jsonl")
}

/// Whether `name` is one of nEVR's logs of a start (`nevr-<time>.jsonl`).
pub fn is_run_log(name: &str) -> bool {
    name.starts_with("nevr-") && name.ends_with(".jsonl") && name != "nevr-boot.jsonl"
}

/// Pure: whether a start's log (nEVR's, which has the game's own lines) says VR didn't come
/// up on the Oculus runtime: no headset (`ovrError_NoHmd`: Meta's runtime under Virtual
/// Desktop, when VD didn't start the game), or no swap chain for it (seen in Virtual
/// Desktop's Oculus mode). The game then stops with an error.
pub fn swap_chain_failed(log: &str) -> bool {
    log.contains("Failed to create OVR D3D swap chain") || log.contains("ovrError_NoHmd")
}

/// Whether `name` is one of nEVR's crash records.
pub fn is_crash_log(name: &str) -> bool {
    name.starts_with("nevr-crash-") && name.ends_with(".txt")
}

/// The newest log of a start in `dir` (by its name, which is the time it started).
pub fn newest_run_log(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| is_run_log(n))
        .max()
        .map(|n| dir.join(n))
}

/// What nEVR did with one plugin at a start.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginStatus {
    /// Its file (`NvrAssetPatches.dll`), as far as the log says.
    pub file: String,
    /// The name it reports.
    pub name: String,
    pub version: String,
    pub api: u32,
    pub capabilities: u32,
    /// `loaded`, `skipped` or `failed`.
    pub status: String,
    pub error: String,
}

impl PluginStatus {
    pub fn loaded(&self) -> bool {
        self.status == "loaded"
    }

    /// Whether this is about plugin file `file`.
    pub fn is(&self, file: &str) -> bool {
        let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
        self.file.eq_ignore_ascii_case(file) || self.name.eq_ignore_ascii_case(stem)
    }
}

/// What nEVR did at a start, from its log.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// When it started, as its log is named (`2026-10-04T18-00-00.123`).
    pub started: String,
    pub plugins: Vec<PluginStatus>,
    /// `plugin load complete: N/M loaded` was logged.
    pub complete: bool,
}

impl Status {
    pub fn of(&self, file: &str) -> Option<&PluginStatus> {
        self.plugins.iter().rev().find(|p| p.is(file))
    }

    /// Reads the newest start's log in `dir`.
    pub fn read(dir: &Path) -> Option<Status> {
        let path = newest_run_log(dir)?;
        let text = std::fs::read_to_string(&path).ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        let started = name
            .trim_start_matches("nevr-")
            .trim_end_matches(".jsonl")
            .to_string();
        Some(Status {
            started,
            ..Status::parse(&text)
        })
    }

    /// Pure: the plugin lines of a log (JSON lines with a `msg`, or plain lines). A
    /// plugin is named by the name it reports (`asset_patches`); its file comes from the
    /// path when the line has one, else from the load order logged before (the config's
    /// names, in the order the results follow). nEVR loads in two passes when a plugin is
    /// marked early, each with its own load order.
    pub fn parse(text: &str) -> Status {
        let mut st = Status::default();
        let mut order: Vec<String> = Vec::new();
        // How many results came before the load order in use.
        let mut before = 0;
        for line in text.lines() {
            let msg = message(line);
            let Some(rest) = msg.split("[NEVR.PLUGIN]").nth(1) else {
                continue;
            };
            let rest = rest.trim();
            // The load order lists what nEVR goes on to load: its results (loaded or
            // failed, not the skipped ones) follow it in that order.
            let tried = |st: &Status| st.plugins.iter().filter(|p| p.status != "skipped").count();
            let next_file = |st: &Status, order: &[String], before: usize| {
                order
                    .get(tried(st) - before)
                    .map(|n| file_of(n))
                    .unwrap_or_default()
            };
            if let Some(list) = rest.strip_prefix("load order (priority-sorted):") {
                before = tried(&st);
                order = list
                    .split(',')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect();
            } else if rest.starts_with("plugin load complete") {
                st.complete = true;
            } else if let Some(mut p) = loaded(rest) {
                if p.file.is_empty() {
                    p.file = next_file(&st, &order, before);
                }
                st.plugins.push(p);
            } else if let Some(r) = rest.strip_prefix("SKIPPED ") {
                let (name, why) = r.split_once(" — ").unwrap_or((r, ""));
                st.plugins.push(PluginStatus {
                    file: file_of(name.trim()),
                    name: name.trim().to_string(),
                    status: "skipped".into(),
                    error: why.trim().to_string(),
                    ..Default::default()
                });
            } else if let Some(p) = failed(rest) {
                st.plugins.push(p);
            }
        }
        st
    }
}

/// A JSON log line's message, or the line.
fn message(line: &str) -> String {
    let t = line.trim();
    if t.starts_with('{') {
        if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(t) {
            for k in ["msg", "message"] {
                if let Some(m) = o.get(k).and_then(Value::as_str) {
                    return m.to_string();
                }
            }
        }
    }
    t.to_string()
}

/// A path's file name (`plugins\X.dll` → `X.dll`); a bare name gets `.dll`.
fn file_of(path: &str) -> String {
    let base = path.rsplit(['\\', '/']).next().unwrap_or(path).trim();
    if base.to_ascii_lowercase().ends_with(".dll") {
        base.to_string()
    } else {
        format!("{base}.dll")
    }
}

/// `Loaded: NAME vX.Y.Z (API vN) caps=0xCC… via PATH`.
fn loaded(rest: &str) -> Option<PluginStatus> {
    let r = rest.strip_prefix("Loaded: ")?;
    let (head, via) = r.split_once(" via ").unwrap_or((r, ""));
    let (name, after) = head.split_once(" v")?;
    let (version, after) = after.split_once(' ').unwrap_or((after, ""));
    let api = after
        .split_once("(API v")
        .and_then(|(_, a)| a.split(')').next())
        .and_then(|a| a.parse().ok())
        .unwrap_or_default();
    let capabilities = after
        .split_once("caps=0x")
        .map(|(_, c)| {
            c.chars()
                .take_while(char::is_ascii_hexdigit)
                .collect::<String>()
        })
        .and_then(|c| u32::from_str_radix(&c, 16).ok())
        .unwrap_or_default();
    // `via plugins\X.dll`, or how it was initialized (`via InitEx`): no file then.
    let via = via.split_whitespace().next().unwrap_or_default();
    let is_path = via.to_ascii_lowercase().ends_with(".dll");
    Some(PluginStatus {
        file: if is_path { file_of(via) } else { String::new() },
        name: name.to_string(),
        version: version.to_string(),
        api,
        capabilities,
        status: "loaded".into(),
        error: String::new(),
    })
}

/// `NAME (PATH): ERROR` and `Required plugin NAME (PATH) failed: ERROR`.
fn failed(rest: &str) -> Option<PluginStatus> {
    let r = rest.strip_prefix("Required plugin ").unwrap_or(rest);
    let (name, after) = r.split_once(" (")?;
    let (path, why) = after.split_once(')')?;
    let why = why
        .trim_start_matches(" failed")
        .trim_start_matches(':')
        .trim();
    if name.contains(' ') || why.is_empty() {
        return None;
    }
    Some(PluginStatus {
        file: file_of(path),
        name: name.to_string(),
        status: "failed".into(),
        error: why.to_string(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_the_swap_chain_fail() {
        // A Virtual Desktop start through Meta's runtime (a player's log, 0.11.10-beta.2).
        let log = r#"{"ts":"2026-10-08T18:45:29.598Z","run":"246c-1dd57552f7d65b6","level":"info","msg":"[EVR] Initializing OVR D3D components..."}
{"ts":"2026-10-08T18:45:29.611Z","run":"246c-1dd57552f7d65b6","level":"error","msg":"[EVR] Failed to create OVR D3D swap chain: "}
{"ts":"2026-10-08T18:45:29.611Z","run":"246c-1dd57552f7d65b6","level":"info","msg":"[EVR] Unknown error while loading the game."}"#;
        assert!(swap_chain_failed(log));
        // Meta's runtime under Virtual Desktop, started without VD (beta.4).
        let no_hmd = r#"{"ts":"2026-10-08T19:05:15.313Z","run":"6198-1dd5757f290f868","level":"info","msg":"[EVR] OVR Error:\n  Code: -1007 -- ovrError_NoHmd\n  Description: ovr_Create: No HMD attached.\n"}"#;
        assert!(swap_chain_failed(no_hmd));
        assert!(!swap_chain_failed(
            r#"{"level":"info","msg":"[EVR] Successfully initialized OVR session."}"#
        ));
    }

    #[test]
    fn tells_what_nevr_reads_as_a_variable() {
        assert!(expands_env("${HOME}"));
        assert!(expands_env("a ${X:-b} c"));
        assert!(!expands_env("${X"));
        assert!(!expands_env("costs $5 {each}"));
        assert!(!expands_env("{${"));
        assert!(expands_env("${${A}"));
    }

    #[test]
    fn finds_the_version() {
        let mut dll = b"MZ\0\0junk\0".to_vec();
        dll.extend_from_slice(b"\0v4.0.0-182-ge418eaa-dirty\0\x01x\0");
        assert_eq!(
            version_in(&dll).as_deref(),
            Some("4.0.0-182-ge418eaa-dirty")
        );
        dll.extend_from_slice(b"\x024.0.0+182.e418eaa\0");
        assert_eq!(version_in(&dll).as_deref(), Some("4.0.0+182.e418eaa"));
        assert_eq!(version_in(b"\0v3.2.0\0").as_deref(), Some("3.2.0"));
        assert_eq!(version_in(b"\0vertex\0v1.2\0"), None);
    }

    /// The DLL Mia ships, when it's on this machine.
    #[test]
    fn reads_the_real_dll() {
        let Some(home) = dirs::home_dir() else { return };
        let Ok(dll) = std::fs::read(home.join("Downloads/BugSplat64.dll")) else {
            return;
        };
        assert!(dll.windows(MARKER.len()).any(|w| w == MARKER));
        assert_eq!(version_in(&dll).as_deref(), Some("4.0.0+182.e418eaa"));
    }

    #[test]
    fn writes_the_plugins() {
        let mut args = Map::new();
        args.insert("logging".into(), Value::String("normal".into()));
        let text = render_config(
            &[
                PluginLine {
                    early: false,
                    file: "NvrAssetPatches.dll".into(),
                    enabled: true,
                    args,
                },
                PluginLine {
                    early: false,
                    file: "Other.dll".into(),
                    enabled: false,
                    args: Map::new(),
                },
            ],
            false,
            None,
        );
        assert!(text.contains(
            "plugins:\n  - name: \"NvrAssetPatches\"\n    file: \"NvrAssetPatches.dll\"\n    enabled: true\n    args: {\"logging\":\"normal\"}\n"
        ));
        assert!(text.contains("  - name: \"Other\"\n    file: \"Other.dll\"\n    enabled: false\n"));
        assert!(render_config(&[], false, None).ends_with("plugins: []\n"));
        assert!(!local_plugins_in(&text));
    }

    #[test]
    fn writes_another_server() {
        let b = Backend {
            socket_uri: "ws://127.0.0.1:7350/ws?format=evr&token=k".into(),
            server_key: "k".into(),
            discord_id: "900000000000000001".into(),
            password: "pa${ss".into(),
        };
        let text = render_config(&[], false, Some(&b));
        assert!(text.contains(
            "services:\n  socket_uri: \"ws://127.0.0.1:7350/ws?format=evr&token=k\"\n\
             identity:\n  discord_id: \"900000000000000001\"\n\
             auth:\n  password: \"pa$${ss\"\n  server_key: \"k\"\n"
        ));
        // Without one: EchoVRCE, nothing of the kind.
        assert!(!render_config(&[], false, None).contains("services:"));
    }

    #[test]
    fn makes_the_game_socket_of_an_address() {
        let s = |a: &str, k: &str| socket_uri(a, k);
        assert_eq!(
            s("127.0.0.1", "").as_deref(),
            Some("ws://127.0.0.1:7350/ws?format=evr")
        );
        assert_eq!(
            s("192.168.178.126:7350", "abc").as_deref(),
            Some("ws://192.168.178.126:7350/ws?format=evr&token=abc")
        );
        assert_eq!(
            s("wss://combat.example.org/ws", "abc").as_deref(),
            Some("wss://combat.example.org/ws?format=evr&token=abc")
        );
        // An address that carries them keeps its own.
        assert_eq!(
            s("ws://h:7350/ws?format=evr&token=own", "abc").as_deref(),
            Some("ws://h:7350/ws?format=evr&token=own")
        );
        for bad in ["", "  ", "http://h", "h/ws", "a b"] {
            assert_eq!(s(bad, ""), None, "{bad}");
        }
    }

    #[test]
    fn puts_the_echovrce_sign_in_aside_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let v = InstalledVersion {
            root: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let local = local_dir(&v);
        std::fs::create_dir_all(&local).unwrap();
        let login = GameLogin::new("rt", "u1", "name", 0);
        write_login(&v, &login).unwrap();
        assert!(login_aside(&v).unwrap());
        assert!(!local.join(CREDENTIALS).exists());
        // Still the game's sign-in as far as the launcher goes: no new device for it.
        assert_eq!(read_login(&v).map(|l| l.user_id).as_deref(), Some("u1"));
        assert!(login_back(&v).unwrap());
        assert!(local.join(CREDENTIALS).is_file());
        assert!(!local.join(CREDENTIALS_ASIDE).exists());
        assert!(!login_back(&v).unwrap());
    }

    #[test]
    fn keeps_the_local_plugins_switch() {
        let on = render_config(&[], true, None);
        assert!(on.contains("\nx-local-plugins: true\n"));
        assert!(local_plugins_in(&on));
        for yes in [
            "x-local-plugins: true",
            "x-local-plugins:yes",
            "x-local-plugins: \"on\" # dev",
            "x-local-plugins: True",
        ] {
            assert!(local_plugins_in(yes), "{yes}");
        }
        for no in [
            "x-local-plugins: false",
            "  x-local-plugins: true",
            "# x-local-plugins: true",
            "x-local-plugins-x: true",
            "",
        ] {
            assert!(!local_plugins_in(no), "{no}");
        }
    }

    #[test]
    fn reads_the_plugin_lines() {
        let log = [
            r#"{"ts":"x","level":"info","msg":"[NEVR.PLUGIN] 2 plugin(s) configured; loading in list order from C:\\g\\_local\\config.yaml"}"#,
            r#"{"ts":"x","level":"info","msg":"[NEVR.PLUGIN] Loaded: NvrAssetPatches v1.1.0 (API v5) caps=0x22 via plugins\\NvrAssetPatches.dll"}"#,
            "[NEVR.PLUGIN] SKIPPED log_filter — superseded by the built-in log filter.",
            r#"{"msg":"[NEVR.PLUGIN] Broken (plugins\\Broken.dll): missing NvrPluginGetInfo export"}"#,
            r#"{"msg":"[NEVR.PLUGIN] plugin load complete: 1/3 loaded"}"#,
        ]
        .join("\n");
        let st = Status::parse(&log);
        assert!(st.complete);
        let p = st.of("NvrAssetPatches.dll").unwrap();
        assert!(p.loaded());
        assert_eq!(
            (p.version.as_str(), p.api, p.capabilities),
            ("1.1.0", 5, 0x22)
        );
        assert_eq!(st.of("log_filter.dll").unwrap().status, "skipped");
        let b = st.of("Broken.dll").unwrap();
        assert_eq!(
            (b.status.as_str(), b.error.as_str()),
            ("failed", "missing NvrPluginGetInfo export")
        );
        assert!(st.of("Missing.dll").is_none());
    }

    fn version_at(root: &Path) -> InstalledVersion {
        let bin = root.join(paths::ARENA_DIR).join("bin/win10");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("echovr.exe"), b"").unwrap();
        InstalledVersion {
            id: "pc-latest".into(),
            root: root.to_string_lossy().into_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn hands_the_game_its_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let v = version_at(dir.path());
        // One nEVR made itself beside the exe would be read first: it goes.
        let near = v.bin_dir().join("_local");
        std::fs::create_dir_all(&near).unwrap();
        std::fs::write(
            near.join(CREDENTIALS),
            r#"{"refresh_token":"old","user_id":"b"}"#,
        )
        .unwrap();
        assert_eq!(read_login(&v).unwrap().user_id, "b");
        let login = GameLogin::new("rt", "a", "Ace", 1_000);
        assert_eq!(login.refresh_token_expiry, 1_000 + REFRESH_LIFE_S);
        write_login(&v, &login).unwrap();
        assert!(!near.join(CREDENTIALS).exists());
        let read = read_login(&v).unwrap();
        assert_eq!(read, login);
        assert!(read.good_for("a", 1_000));
        assert!(!read.good_for("b", 1_000));
        assert!(!read.good_for("a", 1_000 + REFRESH_LIFE_S));
        let text = std::fs::read_to_string(local_dir(&v).join(CREDENTIALS)).unwrap();
        for key in [
            "refresh_token",
            "refresh_token_expiry",
            "user_id",
            "username",
        ] {
            assert!(text.contains(&format!("\"{key}\"")));
        }
        forget_login(&v, "b");
        assert!(read_login(&v).is_some());
        forget_login(&v, "a");
        assert!(read_login(&v).is_none());
    }

    #[test]
    fn names_a_plugin_by_the_load_order() {
        // As nEVR 4.0.0 logs it on the Linux test box.
        let log = [
            r#"{"msg":"[NEVR.PLUGIN] 1 plugin(s) configured; loading in list order from X:\\g\\plugins\\\n"}"#,
            r#"{"msg":"[NEVR.PLUGIN] load order (priority-sorted): NvrAssetPatches\n"}"#,
            r#"{"msg":"[NEVR.PLUGIN] Loaded: asset_patches v1.1.0 (API v5) caps=0x22 via InitEx\n"}"#,
            r#"{"msg":"[NEVR.PLUGIN] plugin load complete: 1/1 loaded\n"}"#,
        ]
        .join("\n");
        let st = Status::parse(&log);
        let p = st.of("NvrAssetPatches.dll").unwrap();
        assert!(p.loaded());
        assert_eq!(
            (p.name.as_str(), p.version.as_str()),
            ("asset_patches", "1.1.0")
        );
        // A plugin skipped before the load order doesn't shift it.
        let skipped = format!(
            "{}\n{log}",
            r#"{"msg":"[NEVR.PLUGIN] SKIPPED Extra — disabled\n"}"#
        );
        let st = Status::parse(&skipped);
        assert!(st.of("NvrAssetPatches.dll").unwrap().loaded());
        assert_eq!(st.of("Extra.dll").unwrap().status, "skipped");
    }

    #[test]
    fn names_plugins_of_both_load_passes() {
        // A plugin marked early loads in a pass of its own, with its own load order.
        let log = [
            r#"{"msg":"[NEVR.PLUGIN] early pass: 1 plugin(s) marked early; loading them before the game reads its data, from X:\\g\\plugins\\"}"#,
            r#"{"msg":"[NEVR.PLUGIN] load order (priority-sorted): NvrContentOverlay"}"#,
            r#"{"msg":"[NEVR.PLUGIN] Loaded: content_overlay v1.0.0 (API v5) caps=0x24 via InitEx"}"#,
            r#"{"msg":"[NEVR.PLUGIN] early pass complete: 1/1 loaded"}"#,
            r#"{"msg":"[NEVR.PLUGIN] 3 plugin(s) configured, 3 enabled; loading in list order from X:\\g\\plugins\\"}"#,
            r#"{"msg":"[NEVR.PLUGIN] load order (priority-sorted): NvrGpuRating, DroneFix"}"#,
            r#"{"msg":"[NEVR.PLUGIN] Loaded: gpu_rating v1.0.0 (API v5) caps=0x22 via InitEx"}"#,
            r#"{"msg":"[NEVR.PLUGIN] Loaded: drone_fix v1.0.0 (API v5) caps=0x24 via InitEx"}"#,
            r#"{"msg":"[NEVR.PLUGIN] plugin load complete: 3/3 loaded"}"#,
        ]
        .join("\n");
        let st = Status::parse(&log);
        let name = |f: &str| st.of(f).map(|p| p.name.clone()).unwrap_or_default();
        assert_eq!(name("NvrContentOverlay.dll"), "content_overlay");
        assert_eq!(name("NvrGpuRating.dll"), "gpu_rating");
        assert_eq!(name("DroneFix.dll"), "drone_fix");
        assert!(st.complete);
    }

    #[test]
    fn sets_aside_only_obsolete_echovrce_configs() {
        // The archive's, and a personalized copy (a Discord id in the login host).
        let archive = r#"{"apiservice_host":"http://g.echovrce.com:80/api","configservice_host":"ws://g.echovrce.com:80/config","loginservice_host":"ws://g.echovrce.com:80/login?discordid=1&password=x","matchingservice_host":"ws://g.echovrce.com:80/matching","serverdb_host":"ws://g.echovrce.com:80/serverdb","transactionservice_host":"ws://g.echovrce.com:80/transaction","publisher_lock":"echovrce"}"#;
        assert!(obsolete_echovrce_config(archive));
        // Another server, extra keys, a look-alike host, not JSON: someone's own.
        let private = archive.replace("g.echovrce.com", "relay.example.org");
        assert!(!obsolete_echovrce_config(&private));
        assert!(!obsolete_echovrce_config(
            r#"{"publisher_lock":"x","fov":90}"#
        ));
        assert!(!obsolete_echovrce_config(
            r#"{"loginservice_host":"ws://echovrce.com.evil.net/login"}"#
        ));
        assert!(!obsolete_echovrce_config("not json"));

        let dir = tempfile::tempdir().unwrap();
        let v = version_at(dir.path());
        assert_eq!(
            arrange_game_config(&v, false).unwrap(),
            GameConfigStep::Nothing
        );
        std::fs::create_dir_all(local_dir(&v)).unwrap();
        let path = local_dir(&v).join(GAME_CONFIG);
        std::fs::write(&path, archive).unwrap();
        assert_eq!(
            arrange_game_config(&v, false).unwrap(),
            GameConfigStep::SetAside
        );
        assert!(!path.exists() && game_config_in_use(&v).is_none());
        // "Use my own config.json": it comes back, and then stays.
        assert_eq!(
            arrange_game_config(&v, true).unwrap(),
            GameConfigStep::Restored
        );
        assert_eq!(game_config_in_use(&v), Some(path.clone()));
        assert_eq!(
            arrange_game_config(&v, true).unwrap(),
            GameConfigStep::Nothing
        );
        std::fs::write(&path, &private).unwrap();
        assert_eq!(
            arrange_game_config(&v, false).unwrap(),
            GameConfigStep::Nothing
        );
        assert!(path.exists());
        // One beside the exe (read first) is set aside too, and comes back without nEVR.
        std::fs::remove_file(&path).unwrap();
        let near = v.bin_dir().join("_local");
        std::fs::create_dir_all(&near).unwrap();
        std::fs::write(near.join(GAME_CONFIG), archive).unwrap();
        assert_eq!(
            arrange_game_config(&v, false).unwrap(),
            GameConfigStep::SetAside
        );
        assert!(game_config_in_use(&v).is_none());
        assert_eq!(restore_game_config(&v).unwrap(), GameConfigStep::Restored);
        assert_eq!(game_config_in_use(&v), Some(near.join(GAME_CONFIG)));
    }

    #[test]
    fn notices_a_config_read_before_ours() {
        let dir = tempfile::tempdir().unwrap();
        let v = version_at(dir.path());
        write_config(&v, &[], false, None).unwrap();
        assert_eq!(shadowing_config(&v), None);
        let near = v.bin_dir().join("_local");
        std::fs::create_dir_all(&near).unwrap();
        std::fs::write(near.join(CONFIG), "plugins: []\n").unwrap();
        assert_eq!(shadowing_config(&v), Some(near.join(CONFIG)));
    }
}
