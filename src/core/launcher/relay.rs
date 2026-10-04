//! The classic lobbies: the event builds, played on a community EchoRelay server
//! (github.com/heisthecat31/EchoRelay) the way its EchoClassicLobbies installer sets them
//! up. A build needs EchoRelay's patch (it logs in without Oculus services and skips the
//! entitlement check) and a `_local/config.json` naming the server, the build
//! (`publisher_lock`) and your account.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};

use super::store::{InstalledVersion, RelayAccount};
use crate::core::{download, http, paths};

/// The classic lobbies server EchoClassicLobbies uses.
pub const DEFAULT_SERVER: &str = "168.119.2.92:6800";
/// EchoRelay's game files (release v0.8.8): the patch every lobby build loads.
const GAME_FILES_URL: &str = "https://github.com/heisthecat31/EchoRelay/releases/download/v0.8.8/EchoRelay-0.8.8-GameFiles.zip";
const GAME_FILES_SHA256: &str = "03e9e585e1dd5b6369fb85cb9b19a98f298d5813a5917eff3bfaf097086cd2ee";
const GAME_FILES: &str = "EchoRelay-0.8.8-GameFiles.zip";
/// The patch itself: one file for every build, under either name.
pub(crate) const PATCH_SHA256: &str =
    "8a62047d26f25185a5aeafd767544e846169518da8e13f907a372cb56e2e5ccf";
/// The 2017 builds' own `dbghelp.dll`, kept for the patch to forward to.
const DBGHELP_ORIG: &str = "dbghelp_orig.dll";

/// The relay's limits for an account.
pub const NAME_MAX: usize = 20;
pub const PASSWORD_MAX: usize = 64;

/// The 2017 builds (rad14, `EchoArena.exe`): the patch loads as `dbghelp.dll` there, and
/// they read the older config keys.
pub fn rad14(v: &InstalledVersion) -> bool {
    v.exe_name().eq_ignore_ascii_case("EchoArena.exe")
}

/// The file in `v`'s bin folder the patch is.
pub fn patch_file(v: &InstalledVersion) -> &'static str {
    if rad14(v) {
        "dbghelp.dll"
    } else {
        "dbgcore.dll"
    }
}

/// Puts EchoRelay's patch into `v`, fetching EchoRelay's game files once (checked against
/// their pinned checksum). Nothing changes when it is in already.
pub fn apply_patch(v: &InstalledVersion, cancel: &AtomicBool) -> Result<()> {
    let bin = v.bin_dir();
    if download::sha256_matches(&bin.join(patch_file(v)), PATCH_SHA256) {
        return Ok(());
    }
    let zip = game_files(cancel)?;
    let member = if rad14(v) {
        "christmas/bin/win7/dbghelp.dll"
    } else {
        "bin/win7/dbgcore.dll"
    };
    let dll = read_member(&zip, member)?;
    if !download::sha256_reader(&mut dll.as_slice())?.eq_ignore_ascii_case(PATCH_SHA256) {
        bail!("EchoRelay's patch doesn't match its checksum. Please try again.");
    }
    install_patch(&bin, rad14(v), &dll)?;
    tracing::info!("EchoRelay's patch is in {}", bin.display());
    Ok(())
}

/// Writes the patch `dll` into `bin`. On the 2017 builds the game's own `dbghelp.dll`
/// becomes `dbghelp_orig.dll` first, once: the patch forwards to it.
fn install_patch(bin: &Path, rad14: bool, dll: &[u8]) -> Result<()> {
    if !bin.is_dir() {
        bail!("Couldn't find the game's folder {}.", bin.display());
    }
    let name = if rad14 { "dbghelp.dll" } else { "dbgcore.dll" };
    let dst = bin.join(name);
    if rad14 {
        let orig = bin.join(DBGHELP_ORIG);
        if !orig.exists() && dst.is_file() {
            std::fs::rename(&dst, &orig)
                .with_context(|| format!("Couldn't keep the game's {name}"))?;
        }
    }
    write_atomic(&dst, dll)
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
    fn the_2017_patch_keeps_the_games_dbghelp_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dbghelp.dll"), b"the game's").unwrap();
        install_patch(dir.path(), true, b"patch").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("dbghelp.dll")).unwrap(),
            b"patch"
        );
        assert_eq!(
            std::fs::read(dir.path().join(DBGHELP_ORIG)).unwrap(),
            b"the game's"
        );
        // Again (a newer patch, or a reinstall): the kept original stays the game's.
        install_patch(dir.path(), true, b"patch 2").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join(DBGHELP_ORIG)).unwrap(),
            b"the game's"
        );
        assert_eq!(
            std::fs::read(dir.path().join("dbghelp.dll")).unwrap(),
            b"patch 2"
        );

        install_patch(dir.path(), false, b"patch").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("dbgcore.dll")).unwrap(),
            b"patch"
        );
        assert!(install_patch(&dir.path().join("missing"), false, b"x").is_err());
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

    /// The pinned release asset is still there and still the same.
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
    }
}
