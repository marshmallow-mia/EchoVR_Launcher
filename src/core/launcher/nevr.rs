//! nEVR runtime (github.com/EchoTools/nevr-runtime) in the live PC build's slot,
//! `bin/win10/BugSplat64.dll`. It points the game at EchoVRCE, signs it in, brings friends
//! and parties into the game, and loads the plugins `_local/config.yaml` lists from
//! `plugins/`. The launcher:
//! - sets its keys in that `config.yaml` before every start (the Mods page's plugins, and
//!   the server while a start plays on another one): the rest of the file is the
//!   player's ([`super::yaml_keys`]);
//! - hands the game a sign-in of its own (`_local/.credentials.json`, a device of the
//!   EchoVRCE account the launcher is signed in with), so it never asks in the browser;
//! - reads what nEVR did with the plugins from its log
//!   (`%LOCALAPPDATA%\EchoVR\logs\nevr-<time>.jsonl`).
//!
//! nEVR looks for `_local` beside the game's exe, then one and two folders up (the
//! game's own `_local`, where the launcher writes); the first `config.yaml` found wins.
//! From nevr-runtime #1255 on it reads `config.yaml` only from its root
//! ([`config_into_root`]) or the file `-config-file` names, and from #1250 on its plugins only
//! from the root's `plugins` ([`super::nevr_root::plugins_into`]).
//! It refuses to start with a `dbgcore.dll` beside the exe (EchoLoader 1's place); one in
//! `plugins/` is fine.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::store::InstalledVersion;
use crate::core::paths;

/// What every nEVR build logs at its start (from 5.10 on, only its runtime file carries it).
pub const MARKER: &[u8] = b"[NEVR.BOOT]";
/// The identity every nEVR from 5.4 on carries: `NEVR-BUILD <version> <commit>`. From 5.10 on
/// it is all the slot's file (the host) says of nEVR.
pub const BUILD: &[u8] = b"NEVR-BUILD ";
/// nEVR's second file from 5.10 on: the host in the slot loads it from beside itself, checks
/// its signature first and stops the game (exit [`RUNTIME_REFUSED`]) when it's missing or refused.
pub const RUNTIME: &str = "nevr-runtime.dll";
/// What a host that loads its runtime from nEVR's root (nevr-runtime #1104 on, ADR 0012)
/// carries: its name for a root moved by `NEVR_ROOT` in its log. Older hosts (5.8.1 to 5.16)
/// load it from beside themselves.
pub const ROOT_HOST: &[u8] = b"nevr_root_override";
/// Where a version whose host is a root host kept its runtime in its game folder before it
/// moved to the launcher's folder ([`super::home`], nEVR 6.0 on; an update's manifest path for
/// it still): the launcher puts it into nEVR's root ([`super::nevr_root`]) before a start of
/// it. Beside the exe it would be the old layout, which install.ps1 moves aside.
pub const ROOT_COPY: &str = "nevr-root/nevr-runtime.dll";
/// What a runtime that keeps its sign-in in nEVR's root (5.13 on) carries (UTF-16): the note
/// its one-time move leaves in `_local`. Older ones read only `_local`.
const ROOT_LOGIN: &str = "MOVED-TO-NEVR-ROOT.txt";
/// What a runtime that reads `config.yaml` only from nEVR's root carries (nevr-runtime #1255
/// on): its option naming the exact file, which replaces `-config-path` (refused, exit 64).
pub const CONFIG_FILE_ARG: &str = "-config-file";
/// What a runtime that loads plugins only from nEVR's root carries (nevr-runtime #1250 on,
/// `<root>\plugins`): the end of its line for a run without `LOCALAPPDATA`.
const ROOT_PLUGINS: &str = "never from the game folder";
/// What a runtime whose game server signs in without a password carries (nevr-runtime #1532,
/// 6.9.0 on): the event of the line with the code it prints on its error output for the
/// host to approve on echovrce.com. It keeps the sign-in in its root.
pub const SERVER_SIGN_IN_CODE: &str = "server_sign_in_code";
/// The game's exit code when nEVR's host found no runtime it could load.
pub const RUNTIME_REFUSED: i32 = 78;
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
/// Where a game config nEVR doesn't use is kept (`.old.2`, `.old.3`, … when that is taken).
pub const GAME_CONFIG_OLD: &str = "config.json.old";
/// Where launchers before 0.11.17 kept it.
pub const GAME_CONFIG_PRE_NEVR: &str = "config.json.pre-nevr";
/// A game sign-in with less than this left is replaced before it runs out.
const LOGIN_MARGIN_S: i64 = 24 * 3600;
/// A refresh token's life when it doesn't say (EchoVRCE's are 30 days).
const REFRESH_LIFE_S: i64 = 30 * 24 * 3600;

// ---- the DLL ----

/// Pure: nEVR's version in its DLL `bytes`: the version of its `NEVR-BUILD` identity
/// (`5.10.0`), else its build identity (`4.0.0+182.e418eaa`), else its `git describe`
/// (`v4.0.0-182-ge418eaa-dirty`) without the `v`.
pub fn version_in(bytes: &[u8]) -> Option<String> {
    if let Some(v) = build_version(bytes) {
        return Some(v);
    }
    // A development build's (`5.1.2-dev.279+bfe115d7`).
    if let Some(id) = build_id(bytes) {
        return id.split(' ').next().map(str::to_string);
    }
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

/// Pure: the first `NEVR-BUILD <version> <commit>` in `bytes` as `<version> <commit>`, a
/// development build's version too (`5.1.2-dev.279+bfe115d7`): a host and its runtime carry
/// the same one.
pub fn build_id(bytes: &[u8]) -> Option<String> {
    bytes
        .windows(BUILD.len())
        .enumerate()
        .filter(|(_, w)| *w == BUILD)
        .find_map(|(at, _)| {
            let rest = &bytes[at + BUILD.len()..];
            let end = rest.iter().position(|b| !(0x20..0x7f).contains(b));
            let line = std::str::from_utf8(&rest[..end.unwrap_or(rest.len())]).ok()?;
            let (ver, commit) = line.split_once(' ')?;
            (!ver.is_empty()
                && !ver.contains(' ')
                && (7..=40).contains(&commit.len())
                && commit.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| line.to_string())
        })
}

/// Pure: the version of the first `NEVR-BUILD <version> <commit>` in `bytes`.
fn build_version(bytes: &[u8]) -> Option<String> {
    bytes
        .windows(BUILD.len())
        .enumerate()
        .filter(|(_, w)| *w == BUILD)
        .find_map(|(at, _)| {
            let rest = &bytes[at + BUILD.len()..];
            let end = rest.iter().position(|b| !(0x20..0x7f).contains(b));
            let line = std::str::from_utf8(&rest[..end.unwrap_or(rest.len())]).ok()?;
            let mut parts = line.split(' ');
            let (ver, commit) = (parts.next()?, parts.next()?);
            (semver(ver)
                && (7..=40).contains(&commit.len())
                && commit.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| ver.to_string())
        })
}

/// Whether `bytes` carries nEVR's start marker or its build identity: nEVR, or (from 5.10
/// on) its host, which needs [`RUNTIME`] beside it.
pub fn is_nevr(bytes: &[u8]) -> bool {
    contains(bytes, MARKER) || contains(bytes, BUILD)
}

/// Whether `bytes` is a nEVR host that loads its runtime from [`RUNTIME`] (5.10 on): it
/// carries the identity but not the start marker.
pub fn is_host(bytes: &[u8]) -> bool {
    !contains(bytes, MARKER) && contains(bytes, BUILD)
}

/// Pure: whether runtime `bytes` keeps its sign-in in nEVR's root (5.13 on), where it wins
/// over `_local`'s.
pub fn reads_root_login(bytes: &[u8]) -> bool {
    let wide: Vec<u8> = ROOT_LOGIN
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    contains(bytes, &wide)
}

/// Pure: whether runtime `bytes` reads `config.yaml` only from nEVR's root, or from the file
/// [`CONFIG_FILE_ARG`] names (nevr-runtime #1255 on): `_local`'s is no longer read, and
/// `-config-path` is refused.
pub fn reads_root_config(bytes: &[u8]) -> bool {
    let wide: Vec<u8> = CONFIG_FILE_ARG
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    contains(bytes, CONFIG_FILE_ARG.as_bytes()) || contains(bytes, &wide)
}

/// Pure: whether runtime `bytes` loads plugins only from nEVR's root (`<root>\plugins`,
/// nevr-runtime #1250 on). Its first run moves the files beside the game's exe there.
pub fn loads_root_plugins(bytes: &[u8]) -> bool {
    contains(bytes, ROOT_PLUGINS.as_bytes())
}

/// Pure: whether runtime `bytes` signs a game server without a password in by a code it
/// shows ([`SERVER_SIGN_IN_CODE`], 6.9.0 on). Older ones refuse such a server.
pub fn signs_servers_in_by_code(bytes: &[u8]) -> bool {
    contains(bytes, SERVER_SIGN_IN_CODE.as_bytes())
}

/// Pure: whether runtime `bytes` updates itself (nevr-runtime 6.17.0 on, ADR 0024): at a
/// start it stages the release its channel names and its host swaps it in at the next start,
/// an older one too. It carries the flag that turns that off for a run ([`NO_UPDATE_ARG`]);
/// 6.16.5 and older have neither. Whether its `config.yaml` may say so is
/// [`takes_update_section`].
pub fn updates_itself(bytes: &[u8]) -> bool {
    let wide: Vec<u8> = NO_UPDATE_ARG
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    contains(bytes, NO_UPDATE_ARG.as_bytes()) || contains(bytes, &wide)
}

/// The runtime's flag for a run without its own update check (6.17.0 on): a release its
/// host already swapped in is still confirmed.
pub const NO_UPDATE_ARG: &str = "-no-update";

/// What a runtime whose `config.yaml` may have an `update` section carries (nevr-runtime
/// 6.18.0 on): the list of top-level keys in the message it refuses another key with.
const UPDATE_SECTION: &str = "combat, update";

/// Pure: whether runtime `bytes` takes `config.yaml`'s `update` section (`update.enabled`,
/// `update.channel`; nevr-runtime 6.18.0 on): its own marker, or a `NEVR-BUILD` of 6.18.0 or
/// newer. 6.17.0 updates itself but refuses the section as an unknown key: a client then
/// runs with nEVR's built-in defaults instead of the whole `config.yaml`, and a game server
/// stops. Its channel can't be picked either, so the launcher keeps it current
/// ([`updates_for`]) and turns its check off with [`NO_UPDATE_ARG`] only.
pub fn takes_update_section(bytes: &[u8]) -> bool {
    let first = crate::core::semver::Version::parse("6.18.0");
    updates_itself(bytes)
        && (contains(bytes, UPDATE_SECTION.as_bytes())
            || build_version(bytes)
                .and_then(|v| crate::core::semver::Version::parse(&v))
                .is_some_and(|v| first.is_some_and(|first| v >= first)))
}

/// Pure: who updates a runtime when Settings pick `by` and the runtime takes `config.yaml`'s
/// `update` section (`section`, [`takes_update_section`]): nEVR only when it can be told its
/// channel, else the launcher (6.17.x, whose own check follows a channel nobody picked).
pub fn updates_for(by: NevrUpdates, section: bool) -> NevrUpdates {
    if section {
        by
    } else {
        NevrUpdates::Launcher
    }
}

/// Who updates nEVR's runtime on this PC, from nevr-runtime 6.17.0 on (Settings' "nEVR
/// updates"): nEVR itself, to the release of the channel Settings picks, or the launcher
/// only (Echo VR's updates, as before 6.17.0), with nEVR's own check off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NevrUpdates {
    #[default]
    Nevr,
    Launcher,
}

/// Unknown names (a choice a later launcher has) are nEVR's own.
impl<'de> Deserialize<'de> for NevrUpdates {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = <String as Deserialize>::deserialize(d)?;
        Ok(match name.as_str() {
            "launcher" => NevrUpdates::Launcher,
            _ => NevrUpdates::Nevr,
        })
    }
}

/// launcher.json's [`NevrUpdates`], for the code that runs without the settings at hand.
static UPDATES: std::sync::Mutex<NevrUpdates> = std::sync::Mutex::new(NevrUpdates::Nevr);

/// Takes launcher.json's choice; called whenever the settings are loaded or saved.
pub fn set_updates(by: NevrUpdates) {
    *UPDATES.lock().unwrap_or_else(|p| p.into_inner()) = by;
}

/// Who updates nEVR's runtime ([`set_updates`]).
pub fn updates() -> NevrUpdates {
    *UPDATES.lock().unwrap_or_else(|p| p.into_inner())
}

/// Pure: the `update` keys the launcher sets in a `config.yaml` for a runtime that takes
/// them (`section`, [`takes_update_section`]): nEVR's channel `channel` when nEVR updates
/// it, its check off when the launcher does. For an older runtime, 6.17.x too, none (it
/// refuses them): the launcher's values go and the player's own come back.
pub fn update_edits(section: bool, by: NevrUpdates, channel: &str) -> Vec<super::yaml_keys::Edit> {
    use super::yaml_keys::Edit;
    let (enabled, channel) = match (section, by) {
        (false, _) => (None, None),
        (true, NevrUpdates::Launcher) => (Some("false".to_string()), None),
        (true, NevrUpdates::Nevr) => (None, Some(json_str(channel))),
    };
    vec![
        Edit::maybe(&["update", "enabled"], enabled),
        Edit::maybe(&["update", "channel"], channel),
    ]
}

/// Whether `bytes` is a host that loads its runtime only from nEVR's root (`%LOCALAPPDATA%\nevr\echovr`).
pub fn is_root_host(bytes: &[u8]) -> bool {
    is_host(bytes) && contains(bytes, ROOT_HOST)
}

/// Pure: where in the game folder (`bin`-relative) the runtime of host `host` is kept:
/// [`ROOT_COPY`] for a root host, else beside it.
pub fn runtime_entry(host: &[u8]) -> &'static str {
    if is_root_host(host) {
        ROOT_COPY
    } else {
        RUNTIME
    }
}

/// Where the runtime of the nEVR in `bin`'s slot is kept ([`runtime_entry`]).
pub fn runtime_path(bin: &Path) -> PathBuf {
    let host = std::fs::read(bin.join(super::mods::SLOT)).unwrap_or_default();
    if is_root_host(&host) {
        root_copy(bin)
    } else {
        bin.join(RUNTIME)
    }
}

/// Where the game in `bin` keeps the runtime it puts into nEVR's root: in its folder in the
/// launcher's ([`super::home`]), else in the game folder ([`ROOT_COPY`]).
pub fn root_copy(bin: &Path) -> PathBuf {
    super::home::of_bin(bin)
        .map(|h| super::home::runtime_in_home(&h))
        .unwrap_or_else(|| bin.join(ROOT_COPY))
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
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

/// Whether the game folder `bin` has a `dbgcore.dll` beside the exe, which nEVR refuses (in
/// any case: Wine finds `DBGCORE.DLL` too).
pub fn stray_dbgcore(bin: &Path) -> bool {
    stray_dbgcore_path(bin).is_some()
}

/// The `dbgcore.dll` beside the exe in `bin`, as it is spelled there.
pub fn stray_dbgcore_path(bin: &Path) -> Option<PathBuf> {
    std::fs::read_dir(bin)
        .ok()?
        .flatten()
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(DBGCORE)
        })
        .map(|e| e.path())
        .filter(|p| p.is_file())
}

// ---- where nEVR looks ----

/// The game's own `_local` (its `config.json`; the launcher's notes too on the old layout).
pub fn local_dir(v: &InstalledVersion) -> PathBuf {
    Path::new(&v.root).join(paths::ARENA_DIR).join("_local")
}

/// Where the launcher keeps its notes on `v` (Mods page choices, Undo, its `config.yaml`):
/// `v`'s folder in the launcher's ([`super::home`]), else the game's `_local`.
pub fn notes_dir(v: &InstalledVersion) -> PathBuf {
    super::home::of(v).unwrap_or_else(|| local_dir(v))
}

/// The `_local` folders nEVR looks in, in its order: beside the exe, one up, two up.
fn search(v: &InstalledVersion) -> [PathBuf; 3] {
    let bin = v.bin_dir();
    let up = bin.parent().map(Path::to_path_buf).unwrap_or(bin.clone());
    let up2 = up.parent().map(Path::to_path_buf).unwrap_or(up.clone());
    [bin.join("_local"), up.join("_local"), up2.join("_local")]
}

/// nEVR's root (its `%LOCALAPPDATA%\nevr\echovr`), where from 5.13 on it keeps its sign-in,
/// which wins over one in `_local` (it moves that one there once). None while the game's
/// `%LOCALAPPDATA%` isn't there yet (a Wine prefix not made yet).
fn login_root() -> Option<PathBuf> {
    #[cfg(test)]
    return TEST_ROOT.with(|r| r.borrow().clone());
    #[cfg(not(test))]
    super::nevr_root::local_app_data()
        .filter(|l| l.is_dir())
        .map(|l| super::nevr_root::root_in(&l))
}

#[cfg(test)]
thread_local! {
    /// The tests' nEVR root (none unless a test sets one: never the computer's).
    static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// nEVR's root when `v`'s nEVR keeps its sign-in there (5.13 on): older ones read only `_local`,
/// so the root's sign-in (another version's) is not theirs.
fn login_root_of(v: &InstalledVersion) -> Option<PathBuf> {
    login_root().filter(|_| super::mods::reads_root_login(&v.bin_dir()))
}

/// Where `v`'s nEVR reads a sign-in: nEVR's root first (5.13 on), then the `_local` folders
/// it looks in. On the new layout ([`super::home`], nEVR 6.0 on) only the root: the launcher
/// keeps none in the game folder.
fn login_dirs(v: &InstalledVersion) -> Vec<PathBuf> {
    let root = login_root_of(v);
    if root.is_some() && super::home::of(v).is_some() {
        return root.into_iter().collect();
    }
    root.into_iter().chain(search(v)).collect()
}

/// The game sign-ins the launcher wrote for `v` (in nEVR's root and each `_local` nEVR
/// looks in).
pub fn logins(v: &InstalledVersion) -> Vec<PathBuf> {
    login_dirs(v)
        .into_iter()
        .map(|d| d.join(CREDENTIALS))
        .filter(|p| p.is_file())
        .collect()
}

/// How every `config.yaml` an older launcher wrote whole started: the file was the
/// launcher's then.
const WRITTEN_BY: &str = "# Written by the Echo VR launcher";

/// The line the launcher keeps at the top of a `config.yaml` it sets its keys in. The
/// file is the player's: only [`LAUNCHER_KEYS`] are the launcher's.
pub const HEADER: &str = "# The Echo VR launcher sets plugins here (its Mods page), the server while you play on another one, and how nEVR updates (Settings). The rest is yours.";

/// The header lines of the files older launchers wrote whole, taken out of the top, and
/// the header before 0.13.17 (without the updates).
pub const OLD_HEADERS: &[&str] = &[
    WRITTEN_BY,
    "# Changes here are replaced",
    "# x-local-plugins: true, which lets",
    "# The Echo VR launcher sets plugins here",
];

/// The keys the launcher sets in nEVR's root's `config.yaml` (#1255 on), as in its own.
const LAUNCHER_KEYS: &[&[&str]] = &[
    &["plugins"],
    &["services", "socket_uri"],
    &["identity", "discord_id"],
    &["auth", "http_uri"],
    &["auth", "password"],
    &["auth", "server_key"],
    &["update", "enabled"],
    &["update", "channel"],
];

/// Pure: whether `text` is a `config.yaml` an older launcher wrote whole (with its header),
/// or one that sets nothing but the local plugins' switch (as its docs said to add). Such
/// a file is started again: the player's own it kept as `.old` comes back.
pub(super) fn written_by_launcher(text: &str) -> bool {
    let text = text.trim_start_matches('\u{feff}');
    if text.lines().any(|l| l == HEADER) {
        return false;
    }
    // Its header on any line: the local plugins' switch added above it, as the docs said.
    if text.lines().any(|l| l.starts_with(WRITTEN_BY)) {
        return true;
    }
    text.lines()
        .filter(|l| !l.starts_with([' ', '\t', '#', '-']) && !l.trim().is_empty())
        .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim()))
        .all(|k| k == super::mods::LOCAL_PLUGINS_KEY)
}

/// Pure: whether `text` is the launcher's copy of a version's `config.yaml`: one an older
/// launcher wrote whole, or one with its [`HEADER`].
pub(super) fn launchers_copy(text: &str) -> bool {
    written_by_launcher(text)
        || text
            .trim_start_matches('\u{feff}')
            .lines()
            .any(|l| l == HEADER)
}

/// `path` kept as `<name>.old` (`.old.2`, `.old.3`, … when that is taken: nothing of the
/// player's is ever written over). Where it went.
pub fn keep_as_old(path: &Path) -> Result<PathBuf> {
    let old = free_old(path);
    std::fs::rename(path, &old).with_context(|| {
        format!(
            "Couldn't move {} out of the way (to {})",
            path.display(),
            old.display()
        )
    })?;
    Ok(old)
}

/// Sets the launcher's keys (`edits`) in the `config.yaml` at `path` and keeps the rest.
/// One an older launcher wrote whole is the launcher's: it starts again from the player's
/// own that launcher kept as `.old` beside it (the newest), when there is one, with the
/// local plugins' switch it had. Returns that `.old`.
pub(super) fn edit_config(
    path: &Path,
    edits: &[super::yaml_keys::Edit],
) -> Result<Option<PathBuf>> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    if text.trim().is_empty() || !written_by_launcher(&text) {
        super::yaml_keys::apply(path, edits, HEADER, OLD_HEADERS)?;
        return Ok(None);
    }
    let (back, mut base) = match players_old(path) {
        Some((old, t)) => (Some(old), t),
        None => (None, String::new()),
    };
    if local_plugins_in(&text) && !local_plugins_in(&base) {
        base = format!("{}: true\n{base}", super::mods::LOCAL_PLUGINS_KEY);
    }
    super::yaml_keys::apply_from(path, &base, edits, HEADER, OLD_HEADERS)?;
    Ok(back)
}

/// The newest `.old` an older launcher made of the player's `config.yaml` at `path`, and
/// its text (none it wrote itself).
fn players_old(path: &Path) -> Option<(PathBuf, String)> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let dir = path.parent()?;
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix(&name)
                .and_then(|r| r.strip_prefix(".old"))
                .is_some_and(|r| {
                    r.is_empty()
                        || r.strip_prefix('.')
                            .is_some_and(|d| d.parse::<u32>().is_ok())
                })
        })
        .filter_map(|e| {
            let at = e.metadata().ok()?.modified().ok()?;
            let text = std::fs::read_to_string(e.path()).ok()?;
            (!written_by_launcher(&text)).then(|| (at, e.path(), text))
        })
        .max_by_key(|(at, ..)| *at)
        .map(|(_, p, t)| (p, t))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Before a start of `v` with nEVR in its slot: every game `config.json` in a `_local` nEVR
/// looks in goes to `config.json.old`, so nEVR's built-in config (friends, parties,
/// presence, matchmaking) applies. nEVR doesn't take a server from it anyway: a plugin's
/// server is in `config.yaml`. One kept by an older launcher (`.pre-nevr`) becomes `.old`
/// too, and the launcher's own for EchoRelay (gone again when nEVR is back) stays. Where
/// each one the player had in place went.
pub fn arrange_game_config(v: &InstalledVersion) -> Result<Vec<PathBuf>> {
    let mut moved = Vec::new();
    for dir in search(v) {
        let legacy = dir.join(GAME_CONFIG_PRE_NEVR);
        if legacy.is_file() {
            // Named as a config.json kept now would be.
            let target = free_old(&dir.join(GAME_CONFIG));
            std::fs::rename(&legacy, &target)
                .with_context(|| format!("rename {}", legacy.display()))?;
        }
        let path = dir.join(GAME_CONFIG);
        if !path.is_file() || super::relay::is_launchers(&path) {
            continue;
        }
        moved.push(keep_as_old(&path)?);
    }
    Ok(moved)
}

/// The first of `<path>.old`, `.old.2`, … that isn't taken.
fn free_old(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (1..)
        .map(|n| {
            let suffix = if n == 1 {
                ".old".to_string()
            } else {
                format!(".old.{n}")
            };
            path.with_file_name(format!("{name}{suffix}"))
        })
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.with_file_name(format!("{name}.old")))
}

/// Before a start without nEVR, which needs it: a game config kept as `config.json.old`
/// (or by an older launcher as `.pre-nevr`) is back in its place, where that has none.
/// Whether one came back.
pub fn restore_game_config(v: &InstalledVersion) -> Result<bool> {
    let mut restored = false;
    for dir in search(v) {
        let path = dir.join(GAME_CONFIG);
        if path.exists() {
            continue;
        }
        let kept = [GAME_CONFIG_OLD, GAME_CONFIG_PRE_NEVR]
            .iter()
            .map(|f| dir.join(f))
            .find(|p| p.is_file());
        if let Some(kept) = kept {
            std::fs::rename(&kept, &path)
                .with_context(|| format!("put {} back", path.display()))?;
            restored = true;
        }
    }
    Ok(restored)
}

/// The game config nEVR loads for `v` (the first `config.json` in its search), if any.
#[cfg(test)]
fn game_config_in_use(v: &InstalledVersion) -> Option<PathBuf> {
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

/// Whether `v`'s loader config turns local plugins on: the launcher's copy (`_local`'s, as
/// its docs say), the player's own nEVR reads before it (an older nEVR), or nEVR's root's
/// (#1255 on) where the player set it (not the launcher, from its copy).
pub fn local_plugins(v: &InstalledVersion) -> bool {
    let on = |p: &Path| std::fs::read_to_string(p).is_ok_and(|t| local_plugins_in(&t));
    let ours = notes_dir(v).join(CONFIG);
    if on(&ours) {
        return true;
    }
    if super::mods::reads_root_config(&v.bin_dir()) {
        let key = super::mods::LOCAL_PLUGINS_KEY;
        return login_root().is_some_and(|r| {
            let p = r.join(CONFIG);
            on(&p) && !super::yaml_keys::read_notes(&p).keys.contains_key(key)
        });
    }
    nearer_config(v).is_some_and(|p| on(&p))
}

/// The player's `config.yaml` an older nEVR reads before the launcher's copy: the first in
/// its search, when that isn't the copy.
fn nearer_config(v: &InstalledVersion) -> Option<PathBuf> {
    let ours = notes_dir(v).join(CONFIG);
    search(v)
        .into_iter()
        .map(|d| d.join(CONFIG))
        .find(|p| p.is_file())
        .filter(|p| !same_file(p, &ours))
}

/// A server other than EchoVRCE that a start plays on (a content pack's own, Settings):
/// its game socket, and the account there. nEVR logs in, matchmakes and (as a dedicated
/// server) registers there instead.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Backend {
    /// `ws://host:7350/ws?format=evr&token=<server key>`.
    pub socket_uri: String,
    /// The same server's HTTP API (`http://host:7350`): nEVR's EchoVRCE sign-in (a device
    /// code approved on echovrce.com) asks it instead of EchoVRCE, is turned down at once,
    /// and the game signs in with the account below. Without it, that sign-in would open
    /// EchoVRCE's page and wait five minutes.
    pub http_uri: String,
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

/// Pure: the HTTP API beside game socket `socket_uri`: the same host and port, `http` for
/// `ws` and `https` for `wss`.
pub fn http_uri(socket_uri: &str) -> Option<String> {
    let url = url::Url::parse(socket_uri).ok()?;
    let scheme = match url.scheme() {
        "ws" => "http",
        "wss" => "https",
        _ => return None,
    };
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Pure: the keys the launcher sets in `config.yaml` for a start: the plugins (none with
/// mods off), and another server than EchoVRCE when this start plays on one (none of
/// those otherwise: the player's own come back). Values are written as JSON, which YAML
/// reads as it is.
pub fn config_edits(
    plugins: &[PluginLine],
    backend: Option<&Backend>,
) -> Vec<super::yaml_keys::Edit> {
    use super::yaml_keys::Edit;
    // nEVR reads ${…} in any value as an environment variable (and drops the whole file
    // when one isn't set): $${ is a literal ${.
    let v = |s: &str| json_str(&s.replace("${", "$${"));
    let field = |f: fn(&Backend) -> &str| backend.map(f).filter(|s| !s.is_empty()).map(v);
    vec![
        Edit::set(&["plugins"], plugin_list(plugins)),
        Edit::maybe(&["services", "socket_uri"], field(|b| &b.socket_uri)),
        Edit::maybe(&["identity", "discord_id"], field(|b| &b.discord_id)),
        Edit::maybe(&["auth", "http_uri"], field(|b| &b.http_uri)),
        Edit::maybe(&["auth", "password"], field(|b| &b.password)),
        Edit::maybe(&["auth", "server_key"], field(|b| &b.server_key)),
    ]
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Pure: a `config.yaml`'s `plugins:` list, as the launcher sets it.
pub fn plugin_list(plugins: &[PluginLine]) -> super::yaml_keys::Val {
    use super::yaml_keys::Val;
    if plugins.is_empty() {
        return Val::Inline("[]".into());
    }
    let mut out = String::new();
    for p in plugins {
        let name = p.file.rsplit_once('.').map_or(p.file.as_str(), |(s, _)| s);
        out.push_str(&format!("  - name: {}\n", json_str(name)));
        out.push_str(&format!("    file: {}\n", json_str(&p.file)));
        out.push_str(&format!("    enabled: {}\n", p.enabled));
        if p.early {
            out.push_str("    early: true\n");
        }
        if !p.args.is_empty() {
            out.push_str(&format!("    args: {}\n", Value::Object(p.args.clone())));
        }
    }
    Val::Block(out)
}

/// Sets the launcher's keys for a start of `v` in its `config.yaml`, and in the player's
/// own an older nEVR reads before it. Returns the player's own configs an older launcher
/// had kept as `.old` that are back now.
pub fn write_config(
    v: &InstalledVersion,
    plugins: &[PluginLine],
    backend: Option<&Backend>,
) -> Result<Vec<PathBuf>> {
    let dir = notes_dir(v);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let mut edits = config_edits(plugins, backend);
    // How nEVR updates (6.18.0 on): into the root's with the rest ([`config_into_root`]).
    edits.extend(update_edits(
        super::mods::takes_update_section(&v.bin_dir()),
        updates(),
        &crate::core::pc_update::nevr_channel_picked(),
    ));
    let mut back: Vec<PathBuf> = edit_config(&dir.join(CONFIG), &edits)?
        .into_iter()
        .collect();
    if !super::mods::reads_root_config(&v.bin_dir()) {
        if let Some(near) = nearer_config(v) {
            back.extend(edit_config(&near, &edits)?);
        }
    }
    Ok(back)
}

/// Before a start of `v` whose runtime reads `config.yaml` only from nEVR's root (#1255 on):
/// the launcher's keys from its `config.yaml` for `v` (its `_local` one: the root is one
/// for every version) set in the root's, and the local plugins' switch as the copy has it.
/// The rest of the root's is the player's. Returns the player's own an older launcher had
/// kept as `.old` there, when it is back.
pub fn config_into_root(v: &InstalledVersion) -> Result<Option<PathBuf>> {
    let Some(root) = login_root().filter(|_| super::mods::reads_root_config(&v.bin_dir())) else {
        return Ok(None);
    };
    let ours = notes_dir(v).join(CONFIG);
    let Ok(text) = std::fs::read_to_string(&ours) else {
        return Ok(None);
    };
    let mut edits = super::yaml_keys::noted_edits(&ours, LAUNCHER_KEYS);
    let key = [super::mods::LOCAL_PLUGINS_KEY];
    edits.push(if local_plugins_in(&text) {
        super::yaml_keys::Edit::inline(&key, "true")
    } else {
        super::yaml_keys::Edit::none(&key)
    });
    edit_config(&root.join(CONFIG), &edits)
}

/// Uninstall: the launcher's keys taken back out of the `config.yaml` at `path` (the
/// player's values back), and the file gone when nothing else is left in it.
pub fn take_back_config(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let notes = super::yaml_keys::notes_path(path);
    if written_by_launcher(&text) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(notes);
        return;
    }
    let names: Vec<Vec<String>> = super::yaml_keys::read_notes(path)
        .keys
        .keys()
        .map(|k| k.split('.').map(str::to_string).collect())
        .collect();
    let edits: Vec<super::yaml_keys::Edit> = names
        .into_iter()
        .map(|path| super::yaml_keys::Edit { path, want: None })
        .collect();
    match super::yaml_keys::apply_from(path, &text, &edits, HEADER, OLD_HEADERS) {
        Ok(out)
            if out
                .lines()
                .all(|l| l.trim().is_empty() || l.trim_start().starts_with('#')) =>
        {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(notes);
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(
            "{}: the launcher's keys not taken out: {e:#}",
            path.display()
        ),
    }
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

/// The sign-in nEVR would use for `v` (the first `.credentials.json` in its search, nEVR's
/// root first), or the one waiting while it plays on another server.
pub fn read_login(v: &InstalledVersion) -> Option<GameLogin> {
    [CREDENTIALS, CREDENTIALS_ASIDE]
        .into_iter()
        .flat_map(|name| login_dirs(v).into_iter().map(move |d| d.join(name)))
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
}

/// Before a start of `v` on another server than EchoVRCE: its EchoVRCE sign-ins wait
/// beside them (nEVR would try them there first). Returns whether one did.
pub fn login_aside(v: &InstalledVersion) -> Result<bool> {
    let mut moved = false;
    for d in login_dirs(v) {
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
    for d in login_dirs(v) {
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

/// Gives `v`'s game `login`: in the game's `_local`, and in nEVR's root when its nEVR reads it
/// there (5.13 on; on the new layout only there), with any other sign-in nEVR would read first
/// removed.
pub fn write_login(v: &InstalledVersion, login: &GameLogin) -> Result<()> {
    let dir = local_dir(v);
    // The new layout: in nEVR's root only (where its prefix is there to write in).
    let root_only = login_root_of(v).is_some() && super::home::of(v).is_some();
    for p in login_dirs(v)
        .into_iter()
        .flat_map(|d| [d.join(CREDENTIALS), d.join(CREDENTIALS_ASIDE)])
    {
        // nEVR hides the file, and Windows won't overwrite a hidden file in place.
        if p.is_file() {
            std::fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    let text = serde_json::to_string_pretty(login)?;
    if !root_only {
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        write_private(&dir.join(CREDENTIALS), text.as_bytes())?;
    }
    if let Some(root) = login_root_of(v) {
        std::fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
        write_private(&root.join(CREDENTIALS), text.as_bytes())?;
    }
    Ok(())
}

/// Gives the game in a folder of its own (a game server's `_local`) `login`, beside its
/// `config.yaml`.
pub fn write_login_at(local: &Path, login: &GameLogin) -> Result<()> {
    std::fs::create_dir_all(local).with_context(|| format!("create {}", local.display()))?;
    let path = local.join(CREDENTIALS);
    // nEVR hides the file, and Windows won't overwrite a hidden file in place.
    if path.is_file() {
        std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    write_private(&path, serde_json::to_string_pretty(login)?.as_bytes())
}

/// Removes `v`'s game sign-ins that are `user_id`'s (signing out of the launcher signs
/// the game out too).
pub fn forget_login(v: &InstalledVersion, user_id: &str) {
    // nEVR's root too, whichever version put it there.
    for p in login_root()
        .into_iter()
        .chain(search(v))
        .flat_map(|d| [d.join(CREDENTIALS), d.join(CREDENTIALS_ASIDE)])
    {
        let theirs = std::fs::read_to_string(&p)
            .ok()
            .and_then(|t| serde_json::from_str::<GameLogin>(&t).ok())
            .is_some_and(|l| l.user_id == user_id);
        if theirs {
            match std::fs::remove_file(&p) {
                Ok(()) => tracing::info!("{}: signed the game out of EchoVRCE", v.id),
                Err(e) => tracing::warn!("{}: {} stayed: {e}", v.id, p.display()),
            }
        }
    }
}

#[cfg(unix)]
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
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
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("Couldn't write {}", path.display()))
}

// ---- what it did ----

// ---- how much it logs ----

/// How much nEVR writes to its log (`nevr-<time>.jsonl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// nEVR's own defaults: info and up, the game's noisiest lines left out.
    #[default]
    Normal,
    /// Everything, debug lines too, and none of the game's lines left out.
    Detailed,
    /// Warnings and errors.
    Quiet,
}

/// The logging choices: what the Mods page's Logs menu sets.
///
/// Not offered: nEVR's `passthrough_to_engine`, which would fill Echo VR's own log files
/// (`_local/r14logs`, nearly empty under nEVR, whose log has the game's lines). With it
/// on, a headless server under Proton printed nothing after nEVR's log filter started
/// (GE-Proton 11-3, 2026-10-13): the file always says off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Logging {
    pub level: LogLevel,
}

/// What a nEVR start reads its log settings from, in the order it looks (the first found
/// wins), beside the exe. Having one at all replaces nEVR's built-in defaults, so what it
/// doesn't name is off or empty (no file log, no rotation, no suppressed lines).
const LOG_FILTER_FILES: [&str; 4] = [
    "log_filter_config.yml",
    "log_filter_config.yaml",
    "log_filter_config.jsonc",
    "log_filter_config.json",
];
/// The one the launcher writes: JSON, whose `//` comments nEVR's reader drops.
const LOG_FILTER_FILE: &str = "log_filter_config.json";
const LOG_FILTER_NOTE: &str =
    "// Written by the Echo VR launcher (Mods > Logs). Setting Logging back to Normal removes it.";

/// Pure: nEVR's log filter for `l`, or None for nEVR's own defaults. nEVR reads its levels
/// as 1 debug, 2 info, 4 warning, 8 error.
pub fn render_log_filter(l: &Logging) -> Option<String> {
    // The channels nobody wants to read stay out, except when everything is asked for.
    let (level, max_line, channels) = match l.level {
        LogLevel::Normal => return None,
        LogLevel::Detailed => (1, 2000, "[]"),
        LogLevel::Quiet => (
            4,
            500,
            r#"["[LOADING TIPS]", "[DIALOGUE]", "[DEBUGPRINT]"]"#,
        ),
    };
    Some(format!(
        "{LOG_FILTER_NOTE}\n{{\n  \"min_level\": {level},\n  \"max_line_length\": {max_line},\n  \"passthrough_to_engine\": false,\n  \"console_enabled\": true,\n  \"console_color\": true,\n  \"file_enabled\": true,\n  \"file_jsonl\": true,\n  \"rotate_enabled\": true,\n  \"rotate_max_size_mb\": 50,\n  \"rotate_keep\": 5,\n  \"suppress_channels\": {channels}\n}}\n"
    ))
}

/// A log filter in `bin` that isn't the launcher's (someone's own: a game server's, or a
/// developer's), which the launcher leaves as it is.
pub fn own_log_filter(bin: &Path) -> Option<PathBuf> {
    LOG_FILTER_FILES.iter().map(|n| bin.join(n)).find(|p| {
        std::fs::read_to_string(p)
            .map(|t| {
                !t.trim_start_matches('\u{feff}')
                    .starts_with(LOG_FILTER_NOTE)
            })
            .unwrap_or(false)
    })
}

/// What [`apply_logging`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoggingStep {
    /// The file is as asked (written, or taken away for the defaults).
    Done,
    /// A file of someone's own is there: the choice doesn't reach nEVR.
    Own(PathBuf),
}

/// Writes (or takes away) nEVR's log filter beside the exe in `bin` for `l`.
pub fn apply_logging(bin: &Path, l: &Logging) -> Result<LoggingStep> {
    if let Some(own) = own_log_filter(bin) {
        return Ok(LoggingStep::Own(own));
    }
    let path = bin.join(LOG_FILTER_FILE);
    match render_log_filter(l) {
        Some(text) => {
            if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
                return Ok(LoggingStep::Done);
            }
            std::fs::write(&path, text)
                .with_context(|| format!("Couldn't write {}", path.display()))?;
        }
        None => match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("Couldn't remove {}", path.display())),
        },
    }
    Ok(LoggingStep::Done)
}

/// The name of the log nEVR's host keeps (5.8 on), in its root's `logs`.
pub const HOST_LOG: &str = "nevr-bootstrap.jsonl";

/// Where nEVR logs a start (on Linux, in the Wine prefix): its root's `logs` from 5.13 on,
/// `%LOCALAPPDATA%\EchoVR\logs` before (5.13 moves those logs over once).
pub fn log_dirs() -> Vec<PathBuf> {
    super::nevr_root::local_app_data()
        .map(|l| log_dirs_in(&l))
        .unwrap_or_default()
}

/// Pure: [`log_dirs`] in the `%LOCALAPPDATA%` `local`.
pub fn log_dirs_in(local: &Path) -> Vec<PathBuf> {
    vec![
        super::nevr_root::root_in(local).join("logs"),
        local.join("EchoVR").join("logs"),
    ]
}

/// A folder nEVR writes logs, crash records or reports to, with its name in an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub dir: PathBuf,
    /// Its subfolders too (a plugin's logs, in folders of their own).
    pub deep: bool,
}

impl Place {
    fn new(label: &str, dir: PathBuf, deep: bool) -> Self {
        Place {
            label: label.into(),
            dir,
            deep,
        }
    }
}

/// Pure: every folder nEVR (6.x and before) writes logs to in the `%LOCALAPPDATA%` `local`
/// with the `%TEMP%` `temp`: its root's `logs` (starts, crash records, its host's log) and
/// `crashes` (crash reports, dumps), the host's fallbacks in `temp`, the logs 5.x kept in
/// `EchoVR\logs` (5.13 moves them, a move can leave some), its installer's, and the root's
/// `plugin_logs`. Labels start with `prefix` (a server's name, or nothing).
pub fn log_places_in(local: &Path, temp: &Path, prefix: &str) -> Vec<Place> {
    let root = super::nevr_root::root_in(local);
    let temp_root = temp.join("nevr").join("echovr");
    let l = |s: &str| format!("{prefix}{s}");
    vec![
        Place::new(&l("root.logs"), root.join("logs"), false),
        Place::new(&l("root.crashes"), root.join("crashes"), false),
        Place::new(&l("root.plugin_logs"), root.join("plugin_logs"), true),
        Place::new(&l("temp.logs"), temp_root.join("logs"), false),
        Place::new(&l("temp.crashes"), temp_root.join("crashes"), false),
        Place::new(&l("EchoVR.logs"), local.join("EchoVR").join("logs"), false),
        Place::new(
            &l("installer.logs"),
            local.join("nevr").join("installer").join("logs"),
            false,
        ),
    ]
}

/// Pure: the folders nEVR writes logs to beside the game in `bin`: `logs` (the boot log, and
/// the run log and crash record when `%LOCALAPPDATA%` can't be written) and `plugin_logs`.
/// The host's last fallback, `bin` itself, is [`is_host_fallback`]'s.
pub fn bin_places(bin: &Path, prefix: &str) -> Vec<Place> {
    vec![
        Place::new(&format!("{prefix}logs"), bin.join("logs"), false),
        Place::new(
            &format!("{prefix}plugin_logs"),
            super::mods::log_dir(bin),
            true,
        ),
    ]
}

/// Whether `name` in the game's folder is one nEVR's host writes there when it can write
/// nowhere else: `nevr-<pid>-<n>.txt` (or `.dmp`).
pub fn is_host_fallback(name: &str) -> bool {
    name.strip_prefix("nevr-")
        .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
        && (name.ends_with(".txt") || name.ends_with(".dmp"))
}

/// The game's `%TEMP%` beside its `%LOCALAPPDATA%` `local`: Windows' own, the Wine prefix's
/// (`AppData\Local\Temp`) on Linux.
pub fn temp_for(local: &Path) -> PathBuf {
    if cfg!(windows) {
        std::env::temp_dir()
    } else {
        local.join("Temp")
    }
}

/// [`log_places_in`] on this computer (on Linux, in the Wine prefix).
pub fn log_places() -> Vec<Place> {
    super::nevr_root::local_app_data()
        .map(|l| log_places_in(&l, &temp_for(&l), ""))
        .unwrap_or_default()
}

/// Pure: the start a crash record or report is of: `nevr-crash-<run>.txt`,
/// `nevr-crash-<run>-<n>.report.txt` (`.sent`/`.rejected` once sent) → `<run>`, the `"run"`
/// of every line of that start's log.
pub fn crash_run(name: &str) -> Option<String> {
    let rest = name.strip_prefix("nevr-crash-")?;
    if let Some(run) = rest.strip_suffix(".txt").filter(|r| !r.contains('.')) {
        return (!run.is_empty()).then(|| run.to_string());
    }
    let report = [".report.txt", ".report.txt.sent", ".report.txt.rejected"]
        .iter()
        .find_map(|s| rest.strip_suffix(s))?;
    let (run, n) = report.rsplit_once('-')?;
    (!run.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .then(|| run.to_string())
}

/// The start a log of a start (`nevr-<time>.jsonl`) is of: its first line's `"run"`.
pub fn run_of(path: &Path) -> Option<String> {
    use std::io::{BufRead, Read};
    let f = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(f.take(64 << 10))
        .read_line(&mut line)
        .ok()?;
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    v["run"].as_str().map(str::to_string)
}

/// The one of [`log_dirs`] with the newest log of a start in it (the root's when neither
/// has one yet).
pub fn log_dir() -> Option<PathBuf> {
    let dirs = log_dirs();
    dirs.iter()
        .filter_map(|d| {
            crate::core::logs::newest_in(d, is_run_log)
                .into_iter()
                .next()
                .map(|(_, t)| (d.clone(), t))
        })
        .max_by_key(|(_, t)| *t)
        .map(|(d, _)| d)
        .or_else(|| dirs.into_iter().next())
}

/// The log nEVR's host (5.8 on) keeps of every start, in its root (the Wine prefix's on
/// Linux): which runtime it found, and whether it loaded it.
pub fn host_log() -> Option<PathBuf> {
    Some(super::nevr_root::root()?.join("logs").join(HOST_LOG))
}

/// Pure: why the last start in a host log (`text`, [`host_log`]) didn't load nEVR's runtime,
/// when it didn't: the check's reason, else what happened.
pub fn runtime_refusal_in(text: &str) -> Option<String> {
    let last = text
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|r| r["event"] == "runtime_load")?;
    match last["outcome"].as_str()? {
        "loaded" => None,
        "runtime_absent" => Some(format!("{RUNTIME} is missing")),
        outcome => Some(
            last["verify_reason"]
                .as_str()
                .filter(|r| !r.is_empty())
                .map(|r| format!("{RUNTIME} was refused: {r}"))
                .unwrap_or_else(|| format!("{RUNTIME}: {outcome}")),
        ),
    }
}

/// Why the last start didn't load nEVR's runtime, from its host's log.
pub fn runtime_refusal() -> Option<String> {
    runtime_refusal_in(&std::fs::read_to_string(host_log()?).ok()?)
}

/// What to tell when a start ended with [`RUNTIME_REFUSED`].
pub fn runtime_refused_message() -> String {
    let why = runtime_refusal().unwrap_or_else(|| format!("{RUNTIME} is missing or was refused"));
    format!("nEVR stopped Echo VR at its start: {why}. Repair game files (in PLAY's menu) downloads nEVR's files again.")
}

/// Whether `name` is one of nEVR's logs of a start (`nevr-<time>.jsonl`), not its first
/// moments' (`nevr-boot.jsonl`), its host's record (`nevr-bootstrap.jsonl`, in the root's
/// `logs` with the runs' from 5.13 on) or its updater's (`nevr-update.jsonl`).
pub fn is_run_log(name: &str) -> bool {
    name.strip_prefix("nevr-")
        .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
        && name.ends_with(".jsonl")
}

/// Pure: whether a start's log (nEVR's, which has the game's own lines) says VR didn't come
/// up on the Oculus runtime: no headset (`ovrError_NoHmd`: Meta's runtime under Virtual
/// Desktop, when VD didn't start the game), or no swap chain for it (seen in Virtual
/// Desktop's Oculus mode). The game then stops with an error.
pub fn swap_chain_failed(log: &str) -> bool {
    log.contains("Failed to create OVR D3D swap chain") || log.contains("ovrError_NoHmd")
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
    fn a_runtime_from_6_17_0_on_updates_itself() {
        assert!(updates_itself(b"MZ\0-no-update\0"));
        let wide: Vec<u8> = "-no-update"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(updates_itself(&wide));
        assert!(!updates_itself(b"MZ NEVR-BUILD 6.16.5 abcdef0\0"));
    }

    #[test]
    fn only_a_runtime_from_6_18_0_on_takes_the_update_section() {
        // 6.17.0 updates itself but refuses `update:` in config.yaml.
        assert!(!takes_update_section(
            b"MZ NEVR-BUILD 6.17.0 abcdef0\0-no-update\0"
        ));
        assert!(!takes_update_section(
            b"MZ NEVR-BUILD 6.17.0-dev.125+1a874753 abcdef0\0-no-update\0"
        ));
        assert!(takes_update_section(
            b"MZ NEVR-BUILD 6.18.0 abcdef0\0-no-update\0"
        ));
        assert!(takes_update_section(
            b"MZ NEVR-BUILD 6.18.1 abcdef0\0-no-update\0"
        ));
        // A development build after the fix: by its list of keys.
        assert!(takes_update_section(
            b"MZ NEVR-BUILD 6.17.1-dev.3+abc1234 abcdef0\0-no-update\0graphics, combat, update, maps\0"
        ));
        // Without the flag it doesn't update itself at all.
        assert!(!takes_update_section(b"MZ NEVR-BUILD 6.18.1 abcdef0\0"));
        // 6.17.x is kept current by the launcher, whatever Settings pick.
        assert_eq!(updates_for(NevrUpdates::Nevr, false), NevrUpdates::Launcher);
        assert_eq!(updates_for(NevrUpdates::Nevr, true), NevrUpdates::Nevr);
        assert_eq!(
            updates_for(NevrUpdates::Launcher, true),
            NevrUpdates::Launcher
        );
    }

    #[test]
    fn sets_how_nevr_updates_for_a_runtime_that_updates_itself() {
        use super::super::yaml_keys::{Edit, Val};
        let want = |e: &[Edit]| -> Vec<Option<Val>> { e.iter().map(|e| e.want.clone()).collect() };
        // nEVR updates itself: to the channel Settings picks.
        let e = update_edits(true, NevrUpdates::Nevr, "beta");
        assert_eq!(e[0].path, ["update", "enabled"]);
        assert_eq!(e[1].path, ["update", "channel"]);
        assert_eq!(want(&e), [None, Some(Val::Inline("\"beta\"".into()))]);
        // The launcher updates it: nEVR's check off.
        let e = update_edits(true, NevrUpdates::Launcher, "beta");
        assert_eq!(want(&e), [Some(Val::Inline("false".into())), None]);
        // An older runtime refuses the keys: none (the player's own come back).
        for by in [NevrUpdates::Nevr, NevrUpdates::Launcher] {
            assert_eq!(want(&update_edits(false, by, "beta")), [None, None]);
        }
    }

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

    #[test]
    fn finds_the_build_identity() {
        let id = b"\0NEVR-BUILD 5.10.0 c8b9cf4c8d39627ec49446127223fdf4ad95574c\0";
        // Before anything else the DLL says (5.4.3's also has a stray "v3.6.4-…").
        let mut dll = b"MZ\0/v3.6.4-101dc3df7f.clean\0".to_vec();
        dll.extend_from_slice(id);
        assert_eq!(version_in(&dll).as_deref(), Some("5.10.0"));
        assert!(is_nevr(&dll) && is_host(&dll));
        dll.extend_from_slice(MARKER);
        assert!(is_nevr(&dll) && !is_host(&dll));
        // Not an identity: no commit, or a version that isn't one.
        assert_eq!(build_version(b"NEVR-BUILD 5.10.0\0"), None);
        assert_eq!(build_version(b"NEVR-BUILD %s %s\0"), None);
        assert!(!is_nevr(b"MZ stock"));
    }

    #[test]
    fn reads_why_the_runtime_was_refused() {
        let load = |outcome: &str, reason: &str| {
            format!(
                r#"{{"event":"runtime_load","outcome":"{outcome}","verify":"refused","verify_reason":"{reason}","tag":"NEVR.HOST"}}"#
            )
        };
        let versions = r#"{"event":"versions","host_version":"5.10.0","tag":"NEVR.HOST"}"#;
        let refused = format!(
            "{}\n{versions}\n{}\n{}",
            load("loaded", ""),
            load(
                "runtime_refused",
                "timestamp invalid: cannot make the chain engine: error 2147942487"
            ),
            r#"{"event":"game_stop","exit_code":78}"#
        );
        assert_eq!(
            runtime_refusal_in(&refused).as_deref(),
            Some("nevr-runtime.dll was refused: timestamp invalid: cannot make the chain engine: error 2147942487")
        );
        assert_eq!(
            runtime_refusal_in(&load("runtime_absent", "")).as_deref(),
            Some("nevr-runtime.dll is missing")
        );
        // The last start counts.
        assert_eq!(
            runtime_refusal_in(&format!("{refused}\n{}", load("loaded", ""))),
            None
        );
        assert_eq!(runtime_refusal_in("not json\n"), None);
    }

    #[test]
    fn finds_a_dbgcore_in_any_case() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!stray_dbgcore(dir.path()));
        std::fs::write(dir.path().join("DBGCORE.DLL"), "x").unwrap();
        assert!(stray_dbgcore(dir.path()));
        assert_eq!(
            stray_dbgcore_path(dir.path()).unwrap().file_name().unwrap(),
            "DBGCORE.DLL"
        );
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

    /// Pure: the launcher's keys set in an empty `config.yaml`.
    fn render(plugins: &[PluginLine], backend: Option<&Backend>) -> String {
        let edits = config_edits(plugins, backend);
        super::super::yaml_keys::apply_to("", &edits, &Default::default()).0
    }

    #[test]
    fn writes_the_plugins() {
        let mut args = Map::new();
        args.insert("logging".into(), Value::String("normal".into()));
        let text = render(
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
            None,
        );
        assert!(text.contains(
            "plugins:\n  - name: \"NvrAssetPatches\"\n    file: \"NvrAssetPatches.dll\"\n    enabled: true\n    args: {\"logging\":\"normal\"}\n"
        ));
        assert!(text.contains("  - name: \"Other\"\n    file: \"Other.dll\"\n    enabled: false\n"));
        assert_eq!(render(&[], None), "plugins: []\n");
        assert!(!local_plugins_in(&text));
    }

    #[test]
    fn writes_another_server() {
        let b = Backend {
            socket_uri: "ws://127.0.0.1:7350/ws?format=evr&token=k".into(),
            http_uri: "http://127.0.0.1:7350".into(),
            server_key: "k".into(),
            discord_id: "900000000000000001".into(),
            password: "pa${ss".into(),
        };
        let text = render(&[], Some(&b));
        assert!(text.contains(
            "services:\n  socket_uri: \"ws://127.0.0.1:7350/ws?format=evr&token=k\"\n\
             identity:\n  discord_id: \"900000000000000001\"\n\
             auth:\n  http_uri: \"http://127.0.0.1:7350\"\n  password: \"pa$${ss\"\n  server_key: \"k\"\n"
        ));
        // Without one: EchoVRCE, nothing of the kind.
        assert!(!render(&[], None).contains("services:"));
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
        // Its HTTP API, for nEVR's sign-in.
        assert_eq!(
            http_uri("ws://127.0.0.1:7350/ws?format=evr&token=k").as_deref(),
            Some("http://127.0.0.1:7350")
        );
        assert_eq!(
            http_uri("wss://combat.example.org/ws").as_deref(),
            Some("https://combat.example.org")
        );
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

    /// nEVR 5.13 on logs a start in its root's `logs`, beside its host's own record.
    #[test]
    fn finds_a_starts_log_in_either_folder() {
        assert!(is_run_log("nevr-2026-10-28T07-32-54.258.jsonl"));
        assert!(!is_run_log("nevr-bootstrap.jsonl"));
        assert!(!is_run_log("nevr-boot.jsonl"));
        assert!(!is_run_log("nevr-update.jsonl"));
        assert!(!is_run_log("nevr-crash-68c0-1dd68cae112e7c4.txt"));
        let local = Path::new("L");
        assert_eq!(
            log_dirs_in(local),
            [
                local.join("nevr").join("echovr").join("logs"),
                local.join("EchoVR").join("logs")
            ]
        );
    }

    /// nEVR 6.x's every log folder (nevr-runtime v6.12.0: README "Files the runtime uses",
    /// the host's fallbacks in host_paths.h and crash_capture.h), in the Wine prefix too.
    #[test]
    fn knows_every_folder_nevr_logs_to() {
        let local = Path::new("pfx/drive_c/users/steamuser/AppData/Local");
        let places = log_places_in(local, &local.join("Temp"), "server-a.");
        let at = |label: &str| {
            places
                .iter()
                .find(|p| p.label == label)
                .unwrap_or_else(|| panic!("{label}: {places:?}"))
        };
        let root = local.join("nevr/echovr");
        assert_eq!(at("server-a.root.logs").dir, root.join("logs"));
        assert_eq!(at("server-a.root.crashes").dir, root.join("crashes"));
        assert!(at("server-a.root.plugin_logs").deep);
        assert_eq!(
            at("server-a.temp.crashes").dir,
            local.join("Temp/nevr/echovr/crashes")
        );
        assert_eq!(at("server-a.EchoVR.logs").dir, local.join("EchoVR/logs"));
        assert_eq!(
            at("server-a.installer.logs").dir,
            local.join("nevr/installer/logs")
        );
        let bin = Path::new("game/bin/win10");
        assert_eq!(
            bin_places(bin, "pc-latest.")
                .iter()
                .map(|p| (p.label.as_str(), p.deep))
                .collect::<Vec<_>>(),
            [("pc-latest.logs", false), ("pc-latest.plugin_logs", true)]
        );
        assert!(is_host_fallback("nevr-31568-1.txt"));
        assert!(is_host_fallback("nevr-31568-1.dmp"));
        assert!(!is_host_fallback("nevr-runtime.dll"));
        assert!(!is_host_fallback("nevr-crash-1.txt"));
    }

    /// A crash record or report names its start's run, which every line of that start's log
    /// carries (its file name is only the time it started).
    #[test]
    fn ties_a_crash_to_its_start() {
        let run = Some("68c0-1dd68cae112e7c4".to_string());
        assert_eq!(crash_run("nevr-crash-68c0-1dd68cae112e7c4.txt"), run);
        assert_eq!(
            crash_run("nevr-crash-68c0-1dd68cae112e7c4-1.report.txt"),
            run
        );
        assert_eq!(
            crash_run("nevr-crash-68c0-1dd68cae112e7c4-12.report.txt.sent"),
            run
        );
        assert_eq!(crash_run("nevr-crash-.txt"), None);
        assert_eq!(crash_run("nevr-crash-x.report.txt"), None);
        assert_eq!(crash_run("nevr-host-1-2-1.report.txt"), None);
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("nevr-2026-10-31T00-00-45.000.jsonl");
        std::fs::write(
            &log,
            "{\"ts\":\"2026-10-31T00:00:45.900Z\",\"run\":\"68c0-1dd68cae112e7c4\",\"level\":\"info\",\"msg\":\"x\"}\n{}\n",
        )
        .unwrap();
        assert_eq!(run_of(&log), run);
        std::fs::write(&log, "not json\n").unwrap();
        assert_eq!(run_of(&log), None);
    }

    /// From #1255 on the runtime reads config.yaml only from its root: the launcher's keys
    /// for the version that starts are set there, and the rest stays the player's; an
    /// older runtime's root is left alone.
    #[test]
    fn puts_the_config_into_nevrs_root_for_a_runtime_that_reads_it_there() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nevr/echovr");
        TEST_ROOT.with(|r| *r.borrow_mut() = Some(root.clone()));
        let nevr = |v: &InstalledVersion, version: &str, root_config: bool| {
            let id = format!("NEVR-BUILD {version} abcdef0\0");
            let mut runtime = format!("[NEVR.BOOT] {id}").into_bytes();
            if root_config {
                runtime.extend(b"use -config-file <file>\0");
            }
            std::fs::write(
                v.bin_dir().join(super::super::mods::SLOT),
                format!("MZ {id}"),
            )
            .unwrap();
            std::fs::write(v.bin_dir().join(RUNTIME), runtime).unwrap();
        };
        let v = version_at(&dir.path().join("game"));
        let older = version_at(&dir.path().join("older"));
        nevr(&v, "5.19.0", true);
        nevr(&older, "5.18.0", false);
        assert!(super::super::mods::reads_root_config(&v.bin_dir()));
        assert!(!super::super::mods::reads_root_config(&older.bin_dir()));
        let in_root = || std::fs::read_to_string(root.join(CONFIG)).ok();
        let line = |p: &str| PluginLine {
            early: false,
            file: format!("{p}.dll"),
            enabled: true,
            args: Map::new(),
        };
        let other = Backend {
            socket_uri: "ws://other/ws".into(),
            ..Backend::default()
        };
        // An older runtime reads _local's: the root stays as it is.
        write_config(&older, &[], None).unwrap();
        assert_eq!(config_into_root(&older).unwrap(), None);
        assert_eq!(in_root(), None);
        // One that reads only the root's gets the launcher's keys, again when they change.
        write_config(&v, &[line("A")], None).unwrap();
        assert_eq!(config_into_root(&v).unwrap(), None);
        assert_eq!(
            in_root().unwrap(),
            format!(
                "{HEADER}\nplugins:\n  - name: \"A\"\n    file: \"A.dll\"\n    enabled: true\n"
            )
        );
        // The player's own keys there stay, and their server comes back after another's.
        let mine = format!("{HEADER}\n# mine\nservices:\n  socket_uri: ws://mine\nplugins: []\nnetwork:\n  tick_rate_hz: 90\n");
        std::fs::write(root.join(CONFIG), &mine).unwrap();
        write_config(&v, &[], Some(&other)).unwrap();
        config_into_root(&v).unwrap();
        let t = in_root().unwrap();
        assert!(
            t.contains("# mine\nservices:\n  socket_uri: \"ws://other/ws\"\n"),
            "{t}"
        );
        assert!(t.contains("tick_rate_hz: 90"));
        write_config(&v, &[], None).unwrap();
        config_into_root(&v).unwrap();
        assert_eq!(in_root().unwrap(), mine);
        // The local plugins' switch in the launcher's copy reaches the root, and goes again.
        let ours = local_dir(&v).join(CONFIG);
        let copy = std::fs::read_to_string(&ours).unwrap();
        std::fs::write(&ours, format!("x-local-plugins: true\n{copy}")).unwrap();
        assert!(local_plugins(&v));
        config_into_root(&v).unwrap();
        assert!(local_plugins_in(&in_root().unwrap()));
        std::fs::write(&ours, &copy).unwrap();
        assert!(!local_plugins(&v), "the root's is the launcher's");
        config_into_root(&v).unwrap();
        assert_eq!(in_root().unwrap(), mine);
        // Set there by the player: it counts.
        std::fs::write(root.join(CONFIG), format!("{mine}x-local-plugins: true\n")).unwrap();
        assert!(local_plugins(&v));
        // An older launcher's whole file there: the player's own it kept as .old is back.
        std::fs::write(
            root.join(CONFIG),
            "# Written by the Echo VR launcher before every start.\nplugins: []\n",
        )
        .unwrap();
        std::fs::write(
            root.join("config.yaml.old"),
            "services:\n  socket_uri: ws://mine\n",
        )
        .unwrap();
        let _ = std::fs::remove_file(super::super::yaml_keys::notes_path(&root.join(CONFIG)));
        assert_eq!(
            config_into_root(&v).unwrap(),
            Some(root.join("config.yaml.old"))
        );
        assert_eq!(
            in_root().unwrap(),
            format!("{HEADER}\nservices:\n  socket_uri: ws://mine\nplugins: []\n")
        );
        assert_eq!(config_into_root(&v).unwrap(), None);
        // Uninstall: the launcher's keys go, the player's stay.
        take_back_config(&root.join(CONFIG));
        assert_eq!(
            in_root().unwrap(),
            format!("{HEADER}\nservices:\n  socket_uri: ws://mine\n")
        );
        TEST_ROOT.with(|r| *r.borrow_mut() = None);
    }

    /// nEVR 5.13 on keeps its sign-in in its root, and that one wins over `_local`'s.
    #[test]
    fn hands_the_sign_in_to_nevrs_root_too() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nevr/echovr");
        TEST_ROOT.with(|r| *r.borrow_mut() = Some(root.clone()));
        let v = version_at(&dir.path().join("game"));
        let older = version_at(&dir.path().join("older"));
        let nevr = |v: &InstalledVersion, version: &str, root_login: bool| {
            let id = format!("NEVR-BUILD {version} abcdef0\0");
            let mut runtime = format!("[NEVR.BOOT] {id}").into_bytes();
            if root_login {
                runtime.extend(ROOT_LOGIN.encode_utf16().flat_map(u16::to_le_bytes));
            }
            std::fs::write(
                v.bin_dir().join(super::super::mods::SLOT),
                format!("MZ {id}"),
            )
            .unwrap();
            std::fs::write(v.bin_dir().join(RUNTIME), runtime).unwrap();
        };
        nevr(&v, "5.16.0", true);
        nevr(&older, "5.10.0", false);
        // One nEVR moved into its root, of another account: what nEVR would use.
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(CREDENTIALS),
            r#"{"refresh_token":"old","user_id":"b"}"#,
        )
        .unwrap();
        assert_eq!(read_login(&v).unwrap().user_id, "b");
        // An older nEVR reads only `_local`: the root's sign-in isn't its (it gets its own).
        assert!(read_login(&older).is_none());
        let theirs = GameLogin::new("rt0", "a", "Ace", 1_000);
        write_login(&older, &theirs).unwrap();
        assert_eq!(read_login(&v).unwrap().user_id, "b");
        assert_eq!(read_login(&older).unwrap(), theirs);
        let login = GameLogin::new("rt", "a", "Ace", 1_000);
        write_login(&v, &login).unwrap();
        for d in [&root, &local_dir(&v)] {
            let t = std::fs::read_to_string(d.join(CREDENTIALS)).unwrap();
            assert_eq!(serde_json::from_str::<GameLogin>(&t).unwrap(), login);
        }
        // Another server: both wait aside, and come back.
        assert!(login_aside(&v).unwrap());
        assert!(!root.join(CREDENTIALS).exists());
        assert!(login_back(&v).unwrap());
        assert!(root.join(CREDENTIALS).is_file());
        assert_eq!(logins(&v).len(), 2);
        assert_eq!(logins(&older).len(), 1);
        forget_login(&v, "a");
        assert!(read_login(&v).is_none());
        assert!(read_login(&older).is_some());
        TEST_ROOT.with(|r| *r.borrow_mut() = None);
    }

    #[test]
    fn keeps_the_local_plugins_switch() {
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
            r#"{"msg":"[NEVR.PLUGIN] plugin trust: mode=unsigned-allowed source=default build=release literal=\"NEVR-PLUGIN-TRUST release\" (x)"}"#,
            r#"{"msg":"[NEVR.PLUGIN] plugin=NvrAssetPatches file=NvrAssetPatches.dll UNSIGNED, loaded because plugin signatures are not required in this run (plugin trust mode unsigned-allowed; start with -require-signed-plugins to refuse it)"}"#,
            r#"{"msg":"[NEVR.PLUGIN] plugin load complete: 1/3 loaded"}"#,
        ]
        .join("\n");
        let st = Status::parse(&log);
        assert!(st.complete);
        // The trust lines (5.7 on: unsigned plugins load) are no plugins of their own.
        assert_eq!(st.plugins.len(), 3);
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
    fn keeps_every_game_config_as_old() {
        // The archive's (a personalized copy, with a Discord id in the login host), and
        // one pointing at another server.
        let archive = r#"{"apiservice_host":"http://g.echovrce.com:80/api","loginservice_host":"ws://g.echovrce.com:80/login?discordid=1&password=x","publisher_lock":"echovrce"}"#;
        let private = archive.replace("g.echovrce.com", "relay.example.org");
        let dir = tempfile::tempdir().unwrap();
        let v = version_at(dir.path());
        assert!(arrange_game_config(&v).unwrap().is_empty());
        std::fs::create_dir_all(local_dir(&v)).unwrap();
        let path = local_dir(&v).join(GAME_CONFIG);
        let old = local_dir(&v).join(GAME_CONFIG_OLD);
        // Any game config is kept as .old (nEVR takes no server from it): the archive's,
        // and one pointing at another server alike.
        std::fs::write(&path, archive).unwrap();
        assert_eq!(arrange_game_config(&v).unwrap(), vec![old.clone()]);
        assert!(!path.exists() && game_config_in_use(&v).is_none());
        assert!(arrange_game_config(&v).unwrap().is_empty());
        std::fs::write(&path, &private).unwrap();
        let old2 = local_dir(&v).join("config.json.old.2");
        assert_eq!(arrange_game_config(&v).unwrap(), vec![old2.clone()]);
        assert_eq!(std::fs::read_to_string(&old).unwrap(), archive);
        assert_eq!(std::fs::read_to_string(&old2).unwrap(), private);
        // A start without nEVR, which needs one: .old comes back, and goes again after.
        assert!(restore_game_config(&v).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), archive);
        assert!(!restore_game_config(&v).unwrap());
        assert_eq!(arrange_game_config(&v).unwrap(), vec![old.clone()]);
        // The launcher's own for EchoRelay stays (it goes with EchoRelay's other files).
        let relay = r#"{"publisher_lock":"echovrce","x-echovr-launcher":"echorelay"}"#;
        std::fs::write(&path, relay).unwrap();
        assert!(arrange_game_config(&v).unwrap().is_empty());
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
        // An older launcher's .pre-nevr becomes .old, without a notice (it was aside).
        std::fs::remove_file(&old).unwrap();
        std::fs::rename(&old2, local_dir(&v).join(GAME_CONFIG_PRE_NEVR)).unwrap();
        assert!(arrange_game_config(&v).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&old).unwrap(), private);
        assert!(!local_dir(&v).join(GAME_CONFIG_PRE_NEVR).exists());
        // An older launcher's .pre-nevr comes back for a start without nEVR too.
        std::fs::rename(&old, local_dir(&v).join(GAME_CONFIG_PRE_NEVR)).unwrap();
        assert!(restore_game_config(&v).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), private);
        // One beside the exe (read first) goes too.
        std::fs::remove_file(&path).unwrap();
        let near = v.bin_dir().join("_local");
        std::fs::create_dir_all(&near).unwrap();
        std::fs::write(near.join(GAME_CONFIG), archive).unwrap();
        assert_eq!(
            arrange_game_config(&v).unwrap(),
            vec![near.join(GAME_CONFIG_OLD)]
        );
        assert!(game_config_in_use(&v).is_none());
    }

    #[test]
    fn leaves_someone_elses_config_alone() {
        assert!(written_by_launcher(
            "# Written by the Echo VR launcher before every start.\nplugins: []\n"
        ));
        assert!(written_by_launcher("x-local-plugins: true\n"));
        assert!(written_by_launcher(""));
        assert!(!written_by_launcher(&format!("{HEADER}\nplugins: []\n")));
        assert!(launchers_copy(&format!("{HEADER}\nplugins: []\n")));
        assert!(!written_by_launcher(
            "services:\n  socket_uri: \"ws://h/spr\"\nplugins:\n  - name: \"A\"\n"
        ));
        assert!(!written_by_launcher(
            "# my server\nauth:\n  password: \"p\"\n"
        ));

        let dir = tempfile::tempdir().unwrap();
        let v = version_at(dir.path());
        let own = local_dir(&v).join(CONFIG);
        std::fs::create_dir_all(local_dir(&v)).unwrap();
        // Someone's own keeps every line: only the plugins are set in it.
        let mine = "# my notes\nidentity:\n  discord_id: \"1\"\nplugins:\n  - name: \"Music\"\n";
        std::fs::write(&own, mine).unwrap();
        assert!(write_config(&v, &[], None).unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(&own).unwrap(),
            format!("{HEADER}\n# my notes\nidentity:\n  discord_id: \"1\"\nplugins: []\n")
        );
        assert!(!local_dir(&v).join("config.yaml.old").exists());
        // The local plugins' switch the player adds stays on.
        let t = std::fs::read_to_string(&own).unwrap();
        std::fs::write(&own, format!("{t}x-local-plugins: true\n")).unwrap();
        write_config(&v, &[], None).unwrap();
        assert!(local_plugins(&v));
        // An older launcher's whole file, with the switch added: the switch stays, and
        // the player's own it kept as .old comes back (the newest).
        std::fs::write(
            &own,
            "x-local-plugins: true\n# Written by the Echo VR launcher before every start.\nplugins: []\n",
        )
        .unwrap();
        let _ = std::fs::remove_file(super::super::yaml_keys::notes_path(&own));
        std::fs::write(local_dir(&v).join("config.yaml.old"), "a: 1\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(local_dir(&v).join("config.yaml.old.2"), "b: 2\n").unwrap();
        assert_eq!(
            write_config(&v, &[], None).unwrap(),
            vec![local_dir(&v).join("config.yaml.old.2")]
        );
        assert_eq!(
            std::fs::read_to_string(&own).unwrap(),
            format!("{HEADER}\nx-local-plugins: true\nb: 2\nplugins: []\n")
        );
        assert!(local_plugins(&v));
        // A game server's, in its own folder, isn't nEVR's search: it stays.
        let server = local_dir(&v).join("server-1");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(server.join(CONFIG), "services:\n  socket_uri: \"ws://h\"\n").unwrap();
        write_config(&v, &[], None).unwrap();
        assert_eq!(
            std::fs::read_to_string(server.join(CONFIG)).unwrap(),
            "services:\n  socket_uri: \"ws://h\"\n"
        );
    }

    /// nEVR's log filter: none for the defaults; otherwise a file nEVR reads (levels 1/2/4,
    /// every key it needs to keep its file log on), which the launcher alone takes away.
    #[test]
    fn writes_nevrs_log_filter_only_when_asked() {
        assert_eq!(render_log_filter(&Logging::default()), None);
        let all = |level| Logging { level };
        let read = |l: &Logging| -> Value {
            // As nEVR reads it: `//` comments dropped.
            let text = render_log_filter(l).unwrap();
            let json: String = text.lines().filter(|l| !l.starts_with("//")).collect();
            serde_json::from_str(&json).unwrap()
        };
        let detailed = read(&all(LogLevel::Detailed));
        assert_eq!(detailed["min_level"], 1);
        assert_eq!(detailed["suppress_channels"], serde_json::json!([]));
        // Having a file replaces the defaults: its file log must be named on, and the
        // game's logger must not be passed through (it stopped a start once).
        for l in [all(LogLevel::Detailed), all(LogLevel::Quiet)] {
            let j = read(&l);
            assert_eq!(j["file_enabled"], true, "{l:?}");
            assert_eq!(j["rotate_enabled"], true, "{l:?}");
            assert_eq!(j["file_jsonl"], true, "{l:?}");
            assert_eq!(j["passthrough_to_engine"], false, "{l:?}");
        }
        assert_eq!(read(&all(LogLevel::Quiet))["min_level"], 4);

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        // Written, rewritten, taken away again.
        assert_eq!(
            apply_logging(bin, &all(LogLevel::Detailed)).unwrap(),
            LoggingStep::Done
        );
        let file = bin.join(LOG_FILTER_FILE);
        assert!(file.is_file());
        apply_logging(bin, &all(LogLevel::Quiet)).unwrap();
        assert!(std::fs::read_to_string(&file)
            .unwrap()
            .contains("\"min_level\": 4"));
        apply_logging(bin, &Logging::default()).unwrap();
        assert!(!file.exists());
        // No file and nothing asked: nothing happens, and no error.
        apply_logging(bin, &Logging::default()).unwrap();

        // Someone's own filter, whatever its format, is left alone (and wins over ours).
        for name in LOG_FILTER_FILES {
            let own = bin.join(name);
            std::fs::write(&own, "min_level: 1\n").unwrap();
            assert_eq!(own_log_filter(bin), Some(own.clone()));
            assert_eq!(
                apply_logging(bin, &all(LogLevel::Detailed)).unwrap(),
                LoggingStep::Own(own.clone())
            );
            assert_eq!(
                apply_logging(bin, &Logging::default()).unwrap(),
                LoggingStep::Own(own.clone())
            );
            assert_eq!(std::fs::read_to_string(&own).unwrap(), "min_level: 1\n");
            std::fs::remove_file(&own).unwrap();
        }
    }

    #[test]
    fn sets_its_keys_in_a_config_read_before_ours() {
        let dir = tempfile::tempdir().unwrap();
        let v = version_at(dir.path());
        write_config(&v, &[], None).unwrap();
        // Nearer the exe (an older nEVR reads it first): the player's, with the plugins set.
        let near = v.bin_dir().join("_local");
        std::fs::create_dir_all(&near).unwrap();
        std::fs::write(near.join(CONFIG), "mine: 1\n").unwrap();
        write_config(&v, &[], None).unwrap();
        assert_eq!(
            std::fs::read_to_string(near.join(CONFIG)).unwrap(),
            format!("{HEADER}\nmine: 1\nplugins: []\n")
        );
        assert!(local_dir(&v).join(CONFIG).is_file());
        assert!(!near.join("config.yaml.old").exists());
    }
}
