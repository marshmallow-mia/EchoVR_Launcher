//! A game server's own setup in an installed version of the live build, as a launcher
//! plugin's `host` step writes it: `_local/<folder>/config.yaml` (the server account, the
//! services it signs in and registers with, its region and guilds, and the plugins the
//! Mods page has on, a content pack's included) beside a `config.json`, which the game's
//! `-config-path` names, and a start script. nEVR then reads that `config.yaml` instead of
//! the launcher's, and (4.1.0 on) signs the server in only with a cached sign-in beside it,
//! never the player's. nEVR ends a server after each match (for a host's fleet manager to
//! start a fresh one): the script starts it again until it is stopped. On Linux it runs the
//! server in a Wine prefix of its own, not the game's (see `linux::echoxr::server_compat_data`).
//! The launcher writes that folder only in this step: its keeper and PLAY keep to the
//! game's own `_local/config.yaml`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::nevr::{self, PluginLine};
use super::store::InstalledVersion;

/// What a `host` step sets (its templates filled in).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Host {
    /// The folder in `_local` (letters, digits, `-`, `_`).
    pub folder: String,
    pub socket_uri: String,
    pub serverdb_uri: String,
    pub http_uri: String,
    pub http_key: String,
    pub server_key: String,
    pub discord_id: String,
    pub password: String,
    /// `network.regions`: one, or several separated by commas.
    pub regions: String,
    /// `guilds`: one, or several separated by commas.
    pub guilds: String,
    /// The port the command line names (digits; empty: the game's own).
    pub port: String,
    /// The plugins the Mods page has on for the version go into the list too.
    pub mods: bool,
}

/// What the step wrote, for the plugin's page (`save`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub dir: PathBuf,
    pub config_json: PathBuf,
    /// The start script: the server, and again after each match.
    pub script: PathBuf,
    /// How to run the script.
    pub command: String,
}

impl Written {
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "folder": self.dir.to_string_lossy(),
            "config": self.config_json.to_string_lossy(),
            "script": self.script.to_string_lossy(),
            "command": self.command,
        })
    }
}

/// Pure: whether `name` can be a folder in `_local`.
pub fn valid_folder(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Pure: a comma-separated list as YAML (`[a, b]` quoted), or `None` when empty.
fn list(text: &str) -> Option<String> {
    let items: Vec<String> = text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(quote)
        .collect();
    (!items.is_empty()).then(|| format!("[{}]", items.join(", ")))
}

/// A value as nEVR reads it: JSON-quoted, and `${` as the literal `$${` (nEVR reads `${…}`
/// as an environment variable, and drops the whole file when one isn't set).
pub(crate) fn quote(text: &str) -> String {
    serde_json::to_string(&text.replace("${", "$${")).unwrap_or_else(|_| "\"\"".into())
}

/// Pure: the server's `config.yaml`: `h`'s services and account, then `plugins`.
pub fn render(h: &Host, plugins: &[PluginLine], local: bool) -> String {
    let mut out = String::from(
        "# A game server's own config, written by an Echo VR launcher plugin (host step).\n\
         # The launcher leaves it alone otherwise: change it in the plugin, or by hand.\n",
    );
    let section = |out: &mut String, name: &str, pairs: &[(&str, &str)]| {
        let set: Vec<_> = pairs.iter().filter(|(_, v)| !v.trim().is_empty()).collect();
        if set.is_empty() {
            return;
        }
        out.push_str(&format!("{name}:\n"));
        for (k, v) in set {
            out.push_str(&format!("  {k}: {}\n", quote(v.trim())));
        }
    };
    // A server without a login address never signs in (nEVR refuses to boot it): EchoVRCE's
    // when the plugin left it empty.
    let socket_uri = match h.socket_uri.trim() {
        "" => crate::core::server_host::SOCKET_URI,
        s => s,
    };
    section(
        &mut out,
        "services",
        &[
            ("socket_uri", socket_uri),
            ("serverdb_uri", &h.serverdb_uri),
        ],
    );
    section(&mut out, "identity", &[("discord_id", &h.discord_id)]);
    // The password as given: spaces may belong to it.
    let mut auth = Vec::new();
    for (k, v) in [
        ("http_uri", h.http_uri.trim()),
        ("http_key", h.http_key.trim()),
        ("server_key", h.server_key.trim()),
    ] {
        if !v.is_empty() {
            auth.push(format!("  {k}: {}\n", quote(v)));
        }
    }
    if !h.password.is_empty() {
        auth.push(format!("  password: {}\n", quote(&h.password)));
    }
    if !auth.is_empty() {
        out.push_str("auth:\n");
        out.extend(auth);
    }
    if let Some(regions) = list(&h.regions) {
        out.push_str(&format!("network:\n  regions: {regions}\n"));
    }
    if let Some(guilds) = list(&h.guilds) {
        out.push_str(&format!("guilds: {guilds}\n"));
    }
    out.push_str(&nevr::render_plugin_list(plugins, local));
    out
}

/// Writes `h`'s server setup into `v` (the live build with nEVR in its slot): the folder,
/// its `config.yaml` and `config.json`. Returns where, and how to start the server.
pub fn write(v: &InstalledVersion, h: &Host) -> Result<Written> {
    if v.publisher_lock.is_some() {
        bail!("Game servers are set up in the live build, not in an event build.");
    }
    if !valid_folder(&h.folder) {
        bail!(
            "{:?} can't be a folder name: letters, digits, - and _ only.",
            h.folder
        );
    }
    if !h.port.trim().chars().all(|c| c.is_ascii_digit()) {
        bail!("{:?} can't be a port: digits only.", h.port.trim());
    }
    if !super::mods::nevr_in(&v.bin_dir()) {
        bail!("{} has no nEVR in its slot: a server needs it.", v.name);
    }
    if cfg!(target_os = "linux") && !crate::core::linux::echoxr::proton_script().is_file() {
        bail!("Set Echo VR up on Linux first (PLAY does it): a server runs through its Proton.");
    }
    if [h.discord_id.trim(), h.password.as_str()]
        .iter()
        .any(|s| s.is_empty())
    {
        bail!("A server needs its account: the Discord ID and the password it signs in with.");
    }
    let dir = nevr::local_dir(v).join(&h.folder);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let (plugins, local) = if h.mods {
        super::mods::server_plugins(v)
    } else {
        (Vec::new(), false)
    };
    let config = dir.join(nevr::CONFIG);
    write_private(&config, &render(h, &plugins, local))?;
    let config_json = dir.join("config.json");
    if !config_json.is_file() {
        std::fs::write(&config_json, game_config(v))
            .with_context(|| format!("write {}", config_json.display()))?;
    }
    let script = dir.join(SCRIPT);
    let text = start_script(v, &config_json, h.port.trim(), &h.folder);
    std::fs::write(&script, text).with_context(|| format!("write {}", script.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
    }
    let command = if cfg!(windows) {
        format!("\"{}\"", script.display())
    } else {
        sh_quote(&script.to_string_lossy())
    };
    tracing::info!("{}: a game server's setup in {}", v.id, dir.display());
    Ok(Written {
        dir,
        config_json,
        script,
        command,
    })
}

/// The start script's name in the setup's folder.
const SCRIPT: &str = if cfg!(windows) {
    "start-server.cmd"
} else {
    "start-server.sh"
};

/// The `config.json` beside the server's `config.yaml`: the game's own (set aside for
/// nEVR, or still in place), else just EchoVRCE's publisher lock.
fn game_config(v: &InstalledVersion) -> String {
    let local = nevr::local_dir(v);
    [
        nevr::GAME_CONFIG_OLD,
        nevr::GAME_CONFIG_PRE_NEVR,
        nevr::GAME_CONFIG,
    ]
    .iter()
    .find_map(|f| std::fs::read_to_string(local.join(f)).ok())
    .unwrap_or_else(|| "{\"publisher_lock\": \"echovrce\"}\n".into())
}

/// Writes `text` to `path`, readable only by this user where the OS can say (it holds the
/// server's password).
fn write_private(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// The start script for `v`'s server on `config_json`: on Windows the game itself, on
/// Linux through the launcher's GE-Proton in the server's own prefix (by `folder`).
fn start_script(v: &InstalledVersion, config_json: &Path, port: &str, folder: &str) -> String {
    use crate::core::linux::echoxr;
    if cfg!(target_os = "linux") {
        return linux_script(&LinuxServer {
            compat: &echoxr::server_compat_data(folder),
            proton: &echoxr::proton_script(),
            wineserver: &echoxr::wineserver(),
            exe: &v.exe_path(),
            // `Z:` (the whole disk) is in every prefix; the game's own drives aren't.
            config: &echoxr::wine_path(&[], config_json),
            port,
        });
    }
    windows_script(
        &v.exe_path().to_string_lossy(),
        &config_json.to_string_lossy(),
        port,
    )
}

/// Pure: `-port N`, or nothing for the game's own.
fn port_arg(port: &str) -> String {
    if port.is_empty() {
        String::new()
    } else {
        format!(" -port {port}")
    }
}

/// Pure: the Windows start script (`cmd`). `%` doubled: a batch file reads it as a variable.
/// The server runs in a console window of its own (nEVR's), which Ctrl+C here doesn't
/// reach: closing that window ends it, and this one then asks before starting it again.
pub fn windows_script(exe: &str, config_json: &str, port: &str) -> String {
    let path = |p: &str| p.replace('/', "\\").replace('%', "%%");
    [
        "@echo off".to_string(),
        "rem An Echo VR game server, written by a launcher plugin (host step). nEVR ends the"
            .into(),
        "rem server after each match: this starts it again. To stop it, close the server's"
            .into(),
        "rem window and press S here.".into(),
        ":again".into(),
        "echo Echo VR server running (its own window). To stop it: close that window, then press S here."
            .into(),
        format!(
            "\"{}\" -server -headless -noovr{} -config-path \"{}\"",
            path(exe),
            port_arg(port),
            path(config_json)
        ),
        "choice /c SR /n /t 10 /d R /m \"The server ended. It starts again in 10 seconds: S stops, R starts it now. \""
            .into(),
        "if errorlevel 2 goto again".into(),
    ]
    .iter()
    .map(|l| format!("{l}\r\n"))
    .collect()
}

/// What the Linux start script runs.
pub struct LinuxServer<'a> {
    /// The server's own Proton data (`STEAM_COMPAT_DATA_PATH`).
    pub compat: &'a Path,
    pub proton: &'a Path,
    pub wineserver: &'a Path,
    pub exe: &'a Path,
    /// The setup's `config.json` as the game sees it (`Z:\...`).
    pub config: &'a str,
    pub port: &'a str,
}

/// Pure: the Linux start script (`sh`). Ctrl+C ends everything in the server's prefix:
/// Proton alone going leaves the game running.
pub fn linux_script(s: &LinuxServer) -> String {
    let q = |p: &Path| sh_quote(&p.to_string_lossy());
    [
        "#!/bin/sh".to_string(),
        "# An Echo VR game server, written by a launcher plugin (host step). nEVR ends the".into(),
        "# server after each match: this starts it again. Ctrl+C stops it.".into(),
        format!("export STEAM_COMPAT_DATA_PATH={}", q(s.compat)),
        "export STEAM_COMPAT_CLIENT_INSTALL_PATH=".into(),
        "export UMU_ID=umu-default".into(),
        "mkdir -p \"$STEAM_COMPAT_DATA_PATH\"".into(),
        format!(
            "stop() {{ WINEPREFIX=\"$STEAM_COMPAT_DATA_PATH/pfx\" {} -k; exit 0; }}",
            q(s.wineserver)
        ),
        "trap stop INT TERM HUP".into(),
        "while :; do".into(),
        format!(
            "  {} run {} -server -headless -noovr{} -config-path {}",
            q(s.proton),
            q(s.exe),
            port_arg(s.port),
            sh_quote(s.config)
        ),
        "  sleep 5".into(),
        "done".into(),
    ]
    .iter()
    .map(|l| format!("{l}\n"))
    .collect()
}

/// Pure: `text` as one word for `sh`.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host {
            folder: "echocombat".into(),
            socket_uri: "ws://g.echovrce.com:80/spr".into(),
            serverdb_uri: "ws://g.echovrce.com:80/nevr".into(),
            http_uri: "https://g.echovrce.com:7350".into(),
            http_key: "hk".into(),
            server_key: "sk".into(),
            discord_id: " 123 ".into(),
            password: "p ${x}".into(),
            regions: "uwu".into(),
            guilds: "".into(),
            port: "6792".into(),
            mods: true,
        }
    }

    #[test]
    fn a_server_always_has_its_login_address() {
        let text = render(
            &Host {
                socket_uri: " ".into(),
                serverdb_uri: String::new(),
                ..host()
            },
            &[],
            false,
        );
        assert!(text.contains("services:\n  socket_uri: \"ws://g.echovrce.com:80/spr\"\n"));
    }

    #[test]
    fn writes_a_servers_config() {
        let overlay = PluginLine {
            file: "NvrContentOverlay.dll".into(),
            enabled: true,
            args: serde_json::from_str(r#"{"active":"echocombat"}"#).unwrap(),
            early: true,
        };
        let text = render(&host(), &[overlay], false);
        assert!(text.contains(
            "services:\n  socket_uri: \"ws://g.echovrce.com:80/spr\"\n  serverdb_uri: \"ws://g.echovrce.com:80/nevr\"\n"
        ));
        assert!(text.contains("identity:\n  discord_id: \"123\"\n"));
        assert!(text.contains("  password: \"p $${x}\"\n"));
        assert!(text.contains("network:\n  regions: [\"uwu\"]\n"));
        assert!(!text.contains("guilds"));
        assert!(text.contains("    early: true\n"));
        // Not the launcher's own config: the launcher leaves this one alone.
        assert!(!text.starts_with("# Written by the Echo VR launcher"));
        let no_region = render(
            &Host {
                regions: " , ".into(),
                guilds: "a, b".into(),
                ..host()
            },
            &[],
            false,
        );
        assert!(!no_region.contains("network:"));
        assert!(no_region.contains("guilds: [\"a\", \"b\"]\n"));
    }

    #[test]
    fn writes_start_scripts() {
        let win = windows_script(
            "C:/Games/Echo 100%/echovr.exe",
            "C:/Games/Echo 100%/_local/server/config.json",
            "6792",
        );
        assert!(win.contains(
            r#""C:\Games\Echo 100%%\echovr.exe" -server -headless -noovr -port 6792 -config-path "C:\Games\Echo 100%%\_local\server\config.json""#
        ));
        assert!(win.contains(":again\r\n") && win.ends_with("if errorlevel 2 goto again\r\n"));
        assert!(win.contains("choice /c SR /n /t 10 /d R "));
        let sh = linux_script(&LinuxServer {
            compat: Path::new("/h/servers/server"),
            proton: Path::new("/h/GE-Proton/proton"),
            wineserver: Path::new("/h/GE-Proton/files/bin/wineserver"),
            exe: Path::new("/g/Mia's games/echovr.exe"),
            config: r"Z:\g\_local\server\config.json",
            port: "",
        });
        assert!(sh.starts_with("#!/bin/sh\n"));
        assert!(sh.contains("export STEAM_COMPAT_DATA_PATH='/h/servers/server'\n"));
        assert!(sh.contains(
            r"  '/h/GE-Proton/proton' run '/g/Mia'\''s games/echovr.exe' -server -headless -noovr -config-path 'Z:\g\_local\server\config.json'"
        ));
        assert!(sh.contains("trap stop INT TERM HUP\n"));
    }

    #[test]
    fn checks_the_folder_name() {
        assert!(valid_folder("echocombat"));
        assert!(valid_folder("server_2"));
        assert!(!valid_folder(""));
        assert!(!valid_folder("../x"));
        assert!(!valid_folder("a b"));
    }
}
