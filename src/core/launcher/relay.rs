//! The classic lobbies: the event builds, played on a community EchoRelay server
//! (github.com/heisthecat31/EchoRelay). A build needs EchoRelay's patch (it logs in
//! without Oculus services and skips the entitlement check) and a `_local/config.json`
//! naming the server, the build (`publisher_lock`) and your account.
//!
//! The patch loads the way the live build's plugins do: a plugin loader in the game's
//! crash reporter's place (`BugSplat64.dll`), here EchoLoader 2, loads the DLLs in
//! `plugins/` before any game code runs. EchoRelay's own installer puts the patch in as
//! `dbgcore.dll` (`dbghelp.dll` on the 2017 builds); a build set up that way is moved over.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};

use super::store::{InstalledVersion, RelayAccount};
use super::versions::Step;
use crate::core::{download, echoxr, http, paths};

/// The classic lobbies server EchoClassicLobbies uses.
pub const DEFAULT_SERVER: &str = "168.119.2.92:6800";
/// EchoRelay's game files (release v0.8.8): the patch every lobby build loads.
const GAME_FILES_URL: &str = "https://github.com/heisthecat31/EchoRelay/releases/download/v0.8.8/EchoRelay-0.8.8-GameFiles.zip";
const GAME_FILES_SHA256: &str = "03e9e585e1dd5b6369fb85cb9b19a98f298d5813a5917eff3bfaf097086cd2ee";
const GAME_FILES: &str = "EchoRelay-0.8.8-GameFiles.zip";
/// The patch itself: one file for every build (the zip has it under each build's name).
pub(crate) const PATCH_SHA256: &str =
    "8a62047d26f25185a5aeafd767544e846169518da8e13f907a372cb56e2e5ccf";
/// EchoLoader 2 (marshmallow-mia's EchoVR_Mod_Loader), on release.echovr.de: the plugin
/// loader, in the crash reporter's place.
const LOADER_URL: &str = "https://release.echovr.de/launcher/event-builds/EchoLoader-2.0.0.dll";
const LOADER_FILE: &str = "EchoLoader-2.0.0.dll";
pub(crate) const LOADER_SHA256: &str =
    "c04b0bbb33d363fb80fa93f8980695b25a9fa4c9e848987c6468cf55c0134ba6";
/// The loader's slot: the crash reporter every event build imports, as the live build does.
const SLOT: &str = super::mods::SLOT;
/// NvrMissingTextures (marshmallow-mia's), on release.echovr.de: Halloween 2017's package
/// lacks about 160 textures its first global level loads, and the game crashes on them;
/// the plugin opens a texture the package has in their place.
const TEXTURES_URL: &str =
    "https://release.echovr.de/launcher/event-builds/NvrMissingTextures-1.0.0.dll";
const TEXTURES_FILE: &str = "NvrMissingTextures-1.0.0.dll";
const TEXTURES_SHA256: &str = "362f0406841489dbb50607522d4d0846c1325e9276ac2bf8e0ac09567be69909";
const TEXTURES_DLL: &str = "NvrMissingTextures.dll";
/// Halloween 2017's lock: the build NvrMissingTextures is for.
const TEXTURES_LOCK: &str = "release4_5";
/// Where the loader finds its plugins, beside the exe.
const PLUGINS: &str = "plugins";
/// EchoRelay's patch, as a plugin.
pub const PATCH_PLUGIN: &str = "EchoRelay.Patch.dll";
/// The loader's settings, beside it: its log and plugins folders (every DLL there loads).
const LOADER_CONFIG: &str = "echoloader.json";
const LOADER_CONFIG_JSON: &str =
    "{\n    \"log_path\": \"plugin_logs\",\n    \"plugins_dir\": \"plugins\"\n}\n";
/// The 2017 builds' own `dbghelp.dll`, as EchoRelay's installer keeps it beside its patch.
const DBGHELP_ORIG: &str = "dbghelp_orig.dll";

/// The relay's limits for an account.
pub const NAME_MAX: usize = 20;
pub const PASSWORD_MAX: usize = 64;

/// The 2017 builds (rad14, `EchoArena.exe`): the patch loads as `dbghelp.dll` there, and
/// they read the older config keys.
pub fn rad14(v: &InstalledVersion) -> bool {
    v.exe_name().eq_ignore_ascii_case("EchoArena.exe")
}

/// The files in `v`'s bin folder that aren't the build's once it is set up (a reinstall
/// leaves them be): the loader in the crash reporter's place and, on the 2017 builds,
/// `dbghelp.dll` (Halloween 2017's archive has EchoRelay's patch in it, and the game's own
/// as `dbghelp_orig.dll`).
pub fn not_the_builds(v: &InstalledVersion) -> Vec<&'static str> {
    if rad14(v) {
        vec![SLOT, "dbghelp.dll", DBGHELP_ORIG]
    } else {
        vec![SLOT]
    }
}

/// Whether `v` needs NvrMissingTextures.
fn lacks_textures(v: &InstalledVersion) -> bool {
    v.publisher_lock.as_deref() == Some(TEXTURES_LOCK)
}

/// The plugins `v` gets, by file name.
fn plugin_names(v: &InstalledVersion) -> Vec<(&'static str, &'static str)> {
    let mut p = vec![(PATCH_PLUGIN, PATCH_SHA256)];
    if lacks_textures(v) {
        p.push((TEXTURES_DLL, TEXTURES_SHA256));
    }
    p
}

/// Whether `v` is set up: EchoLoader in the slot, its plugins and settings, and none of
/// EchoRelay's own installer's patch left beside the exe.
pub fn in_place(v: &InstalledVersion) -> bool {
    let bin = v.bin_dir();
    download::sha256_matches(&bin.join(SLOT), LOADER_SHA256)
        && plugin_names(v)
            .iter()
            .all(|(name, sha)| download::sha256_matches(&bin.join(PLUGINS).join(name), sha))
        && bin.join(LOADER_CONFIG).is_file()
        && !old_patch_in(&bin, rad14(v))
}

/// Whether EchoRelay's installer's patch is beside the exe in `bin`: as `dbghelp.dll` with
/// the game's kept as `dbghelp_orig.dll` (2017), or as `dbgcore.dll`, a file the 2018 and
/// 2019 builds don't have. (The 2017 builds ship Microsoft's own `dbgcore.dll`: it stays.)
fn old_patch_in(bin: &Path, rad14: bool) -> bool {
    if rad14 {
        bin.join(DBGHELP_ORIG).is_file()
    } else {
        bin.join("dbgcore.dll").is_file()
    }
}

/// Sets `v` up for the classic lobbies: EchoLoader 2 in the crash reporter's place, and
/// EchoRelay's patch (and NvrMissingTextures for Halloween 2017) in `plugins/`, each
/// fetched once and checked against its pinned checksum. Nothing changes when it's set up.
pub fn set_up(v: &InstalledVersion, cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<()> {
    if in_place(v) {
        return Ok(());
    }
    let zip = game_files(cancel)?;
    let patch = read_member(&zip, "bin/win7/dbgcore.dll")?;
    if !download::sha256_reader(&mut patch.as_slice())?.eq_ignore_ascii_case(PATCH_SHA256) {
        bail!("EchoRelay's patch doesn't match its checksum. Please try again.");
    }
    on(Step::Status("Downloading EchoLoader...".into()));
    let loader = std::fs::read(echoxr::fetch_pinned(
        LOADER_URL,
        LOADER_FILE,
        LOADER_SHA256,
        cancel,
        on,
    )?)?;
    let mut plugins = vec![(PATCH_PLUGIN, patch)];
    if lacks_textures(v) {
        on(Step::Status("Downloading NvrMissingTextures...".into()));
        let dll = echoxr::fetch_pinned(TEXTURES_URL, TEXTURES_FILE, TEXTURES_SHA256, cancel, on)?;
        plugins.push((TEXTURES_DLL, std::fs::read(dll)?));
    }
    let bin = v.bin_dir();
    install(&bin, rad14(v), &loader, &plugins)?;
    tracing::info!("EchoLoader and EchoRelay's patch are in {}", bin.display());
    Ok(())
}

/// Writes the loader, its settings and `plugins` into `bin`, taking EchoRelay's
/// installer's patch out first: the game's own `dbghelp.dll` back (2017), or the
/// `dbgcore.dll` the build doesn't have gone.
fn install(bin: &Path, rad14: bool, loader: &[u8], plugins: &[(&str, Vec<u8>)]) -> Result<()> {
    if !bin.is_dir() {
        bail!("Couldn't find the game's folder {}.", bin.display());
    }
    if old_patch_in(bin, rad14) {
        let result = if rad14 {
            std::fs::rename(bin.join(DBGHELP_ORIG), bin.join("dbghelp.dll"))
        } else {
            std::fs::remove_file(bin.join("dbgcore.dll"))
        };
        result.context(
            "Couldn't take EchoRelay's old patch out. Please close Echo VR and try again.",
        )?;
    }
    let dir = bin.join(PLUGINS);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    for (name, dll) in plugins {
        write_atomic(&dir.join(name), dll)?;
    }
    write_atomic(&bin.join(LOADER_CONFIG), LOADER_CONFIG_JSON.as_bytes())?;
    write_atomic(&bin.join(SLOT), loader)
}

/// EchoRelay's game files in the download folder, fetched once and checked.
fn game_files(cancel: &AtomicBool) -> Result<PathBuf> {
    let zip = paths::downloads_dir().join(GAME_FILES);
    if download::sha256_matches(&zip, GAME_FILES_SHA256) {
        return Ok(zip);
    }
    std::fs::create_dir_all(paths::downloads_dir())?;
    http::download_to(GAME_FILES_URL, &zip, Some(cancel))
        .context("Couldn't download EchoRelay's game files from GitHub")?;
    if !download::sha256_matches(&zip, GAME_FILES_SHA256) {
        let _ = std::fs::remove_file(&zip);
        bail!("EchoRelay's game files from GitHub don't match their checksum. Please try again.");
    }
    Ok(zip)
}

fn read_member(zip: &Path, name: &str) -> Result<Vec<u8>> {
    let f = std::fs::File::open(zip).with_context(|| format!("open {}", zip.display()))?;
    let mut archive = zip::ZipArchive::new(f)?;
    let mut entry = archive
        .by_name(name)
        .with_context(|| format!("{name} isn't in EchoRelay's game files"))?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn write_atomic(dst: &Path, bytes: &[u8]) -> Result<()> {
    let part = dst.with_extension("part");
    std::fs::write(&part, bytes).with_context(|| format!("Couldn't write {}", part.display()))?;
    std::fs::rename(&part, dst).with_context(|| {
        let _ = std::fs::remove_file(&part);
        format!(
            "Couldn't replace {}. Please close Echo VR and try again.",
            dst.display()
        )
    })
}

/// Pure: the game's `_local/config.json` for build `lock` on `server` as `account`: the
/// keys the 2018/2019 builds read, and for the 2017 builds (`rad14`) the older ones too.
pub fn config_json(server: &str, lock: &str, rad14: bool, account: &RelayAccount) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let login = format!(
        "ws://{server}/login?auth={}&displayname={}",
        enc(&account.password),
        enc(account.name.trim())
    );
    let ws = |path: &str| serde_json::Value::from(format!("ws://{server}/{path}"));
    let mut c = serde_json::Map::new();
    c.insert(
        "apiservice_host".into(),
        format!("http://{server}/api").into(),
    );
    c.insert("configservice_host".into(), ws("config"));
    c.insert("loginservice_host".into(), login.clone().into());
    c.insert("matchingservice_host".into(), ws("matching"));
    c.insert("transactionservice_host".into(), ws("transaction"));
    c.insert("socialservice_host".into(), ws("social"));
    c.insert("serverdb_host".into(), ws("serverdb"));
    c.insert("publisher_lock".into(), lock.into());
    if rad14 {
        c.insert("login_host".into(), login.into());
        c.insert("matchmaker_host".into(), ws("matching"));
        c.insert("radserverdb_host".into(), ws("serverdb"));
        c.insert("port_retries".into(), 20.into());
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(c)).unwrap_or_default()
}

/// Points event build `v` at `server` as `account`: writes its `_local/config.json`, the
/// archive's own kept once beside it as `config.json.orig`.
pub fn write_config(v: &InstalledVersion, server: &str, account: &RelayAccount) -> Result<()> {
    let Some(lock) = &v.publisher_lock else {
        bail!("{} isn't an event build", v.name);
    };
    let local = Path::new(&v.root).join(paths::ARENA_DIR).join("_local");
    std::fs::create_dir_all(&local).with_context(|| format!("create {}", local.display()))?;
    let config = local.join("config.json");
    let orig = local.join("config.json.orig");
    if config.is_file() && !orig.exists() {
        let _ = std::fs::copy(&config, &orig);
    }
    write_atomic(
        &config,
        config_json(server, lock, rad14(v), account).as_bytes(),
    )
}

/// Pure: whether `server` reads as `host:port` (a name or an IPv4 address, and a port).
pub fn valid_server(server: &str) -> bool {
    let Some((host, port)) = server.trim().rsplit_once(':') else {
        return false;
    };
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['.', '-'])
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
    host_ok && port.parse::<u16>().is_ok_and(|p| p > 0)
}

/// Pure: whether `account` is one the relay takes.
pub fn valid_account(account: &RelayAccount) -> bool {
    let name = account.name.trim();
    !name.is_empty()
        && name.chars().count() <= NAME_MAX
        && !account.password.is_empty()
        && account.password.chars().count() <= PASSWORD_MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> RelayAccount {
        RelayAccount {
            name: " Pebbles & Co ".into(),
            password: "p@ss word".into(),
        }
    }

    #[test]
    fn configs_for_the_2019_and_2017_builds() {
        let c: serde_json::Value = serde_json::from_str(&config_json(
            DEFAULT_SERVER,
            "rad15_summer",
            false,
            &account(),
        ))
        .unwrap();
        assert_eq!(c["publisher_lock"], "rad15_summer");
        assert_eq!(
            c["loginservice_host"],
            "ws://168.119.2.92:6800/login?auth=p%40ss+word&displayname=Pebbles+%26+Co"
        );
        assert_eq!(c["apiservice_host"], "http://168.119.2.92:6800/api");
        assert_eq!(c["serverdb_host"], "ws://168.119.2.92:6800/serverdb");
        assert!(
            c.get("login_host").is_none(),
            "only the 2017 builds read those"
        );

        let old: serde_json::Value = serde_json::from_str(&config_json(
            "relay.example:777",
            "release4_5",
            true,
            &account(),
        ))
        .unwrap();
        assert_eq!(old["login_host"], old["loginservice_host"]);
        assert_eq!(old["matchmaker_host"], "ws://relay.example:777/matching");
        assert_eq!(old["radserverdb_host"], "ws://relay.example:777/serverdb");
        assert_eq!(old["port_retries"], 20);
    }

    #[test]
    fn the_loader_takes_the_old_patchs_place() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        let plugins = [(PATCH_PLUGIN, b"patch".to_vec())];
        // 2017, as EchoRelay's installer left it: its patch as dbghelp.dll, the game's kept.
        std::fs::write(bin.join("dbghelp.dll"), b"patch").unwrap();
        std::fs::write(bin.join(DBGHELP_ORIG), b"the game's").unwrap();
        std::fs::write(bin.join("dbgcore.dll"), b"Microsoft's").unwrap();
        std::fs::write(bin.join(SLOT), b"crash reporter").unwrap();
        assert!(old_patch_in(bin, true));
        install(bin, true, b"loader", &plugins).unwrap();
        assert_eq!(
            std::fs::read(bin.join("dbghelp.dll")).unwrap(),
            b"the game's"
        );
        assert!(!bin.join(DBGHELP_ORIG).exists());
        assert_eq!(
            std::fs::read(bin.join("dbgcore.dll")).unwrap(),
            b"Microsoft's"
        );
        assert_eq!(std::fs::read(bin.join(SLOT)).unwrap(), b"loader");
        assert_eq!(
            std::fs::read(bin.join("plugins").join(PATCH_PLUGIN)).unwrap(),
            b"patch"
        );
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bin.join(LOADER_CONFIG)).unwrap()).unwrap();
        assert_eq!(config["plugins_dir"], "plugins");
        // Again: nothing of the game's is touched.
        install(bin, true, b"loader", &plugins).unwrap();
        assert_eq!(
            std::fs::read(bin.join("dbghelp.dll")).unwrap(),
            b"the game's"
        );

        // 2018/2019: the patch was dbgcore.dll, which those builds don't have.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        std::fs::write(bin.join("dbgcore.dll"), b"patch").unwrap();
        install(bin, false, b"loader", &plugins).unwrap();
        assert!(!bin.join("dbgcore.dll").exists());
        assert!(!old_patch_in(bin, false));
        assert!(install(&bin.join("missing"), false, b"x", &plugins).is_err());
    }

    #[test]
    fn servers_and_accounts() {
        assert!(valid_server(DEFAULT_SERVER));
        assert!(valid_server("relay.echo.example:777"));
        for bad in [
            "",
            "host",
            "host:",
            ":777",
            "host:0",
            "host:99999",
            "a b:1",
            "-x:1",
        ] {
            assert!(!valid_server(bad), "{bad}");
        }
        assert!(valid_account(&account()));
        let long = RelayAccount {
            name: "x".repeat(21),
            password: "p".into(),
        };
        assert!(!valid_account(&long));
        assert!(!valid_account(&RelayAccount::default()));
    }

    /// The pinned downloads are still there and still the same.
    #[test]
    #[ignore = "network"]
    fn the_pinned_game_files_are_on_github() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join(GAME_FILES);
        http::download_to(GAME_FILES_URL, &zip, None).unwrap();
        assert!(download::sha256_matches(&zip, GAME_FILES_SHA256));
        for member in ["bin/win7/dbgcore.dll", "christmas/bin/win7/dbghelp.dll"] {
            let dll = read_member(&zip, member).unwrap();
            assert_eq!(
                download::sha256_reader(&mut dll.as_slice()).unwrap(),
                PATCH_SHA256
            );
        }
        for (url, name, sha) in [
            (LOADER_URL, LOADER_FILE, LOADER_SHA256),
            (TEXTURES_URL, TEXTURES_FILE, TEXTURES_SHA256),
        ] {
            let path = dir.path().join(name);
            http::download_to(url, &path, None).unwrap();
            assert!(download::sha256_matches(&path, sha), "{url}");
        }
    }
}
