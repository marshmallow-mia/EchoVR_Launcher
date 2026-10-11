//! Steam on Linux: Echo VR as a non-Steam game (a shortcut in `shortcuts.vdf` that runs the
//! launcher with `--play`, which starts the game through Proton itself), closing and
//! restarting Steam around that edit (it rewrites the file from memory while it runs), and
//! starting the game through `steam://rungameid`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::vdf::{self, Bin, Text};

/// The shortcut's name in the library; it also identifies the launcher's own entry.
pub const SHORTCUT_NAME: &str = "Echo VR";
/// SteamID64 of the first account: account IDs (the `userdata` folders) count from here.
const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;
/// Flathub's Steam. Its files are under `~/.var/app/<id>`, which its sandbox sees at that
/// same path; it runs in a pid namespace of its own.
pub const FLATPAK_ID: &str = "com.valvesoftware.Steam";

/// The Flatpak's own folder, `~/.var/app/com.valvesoftware.Steam`.
pub fn flatpak_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".var/app").join(FLATPAK_ID))
}

/// Whether `root` is the Flatpak's Steam.
pub fn is_flatpak(root: &Path) -> bool {
    root.components().any(|c| c.as_os_str() == FLATPAK_ID)
}

/// Where Steam is (Windows): the folder its registry key names.
#[cfg(windows)]
pub fn root() -> Option<PathBuf> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let path: String = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Valve\Steam")
        .ok()?
        .get_value("SteamPath")
        .ok()?;
    let root = PathBuf::from(path.replace('/', "\\"));
    root.join("config").is_dir().then_some(root)
}

/// Where Steam is: the usual install (or its `~/.steam` link) or the Flatpak's. With both
/// on this PC, the one running, else the one signed in to last (see [`choose`]).
#[cfg(not(windows))]
pub fn root() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let has_config = |p: &PathBuf| p.join("config").is_dir();
    // `~/.steam/steam` first: Steam keeps it pointing at its install (Debian's is elsewhere).
    let native = [home.join(".steam/steam"), home.join(".local/share/Steam")]
        .into_iter()
        .find(has_config)
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .filter(|p| !is_flatpak(p));
    // Only an installed Flatpak counts (a removed one leaves its data behind); never through
    // its `.steam/steam` link, which names a path as the sandbox sees it.
    let flatpak = flatpak_installed(&home)
        .then(|| {
            let app = home.join(".var/app").join(FLATPAK_ID);
            [app.join(".local/share/Steam"), app.join("data/Steam")]
                .into_iter()
                .find(has_config)
        })
        .flatten();
    match (native, flatpak) {
        (Some(native), Some(flatpak)) => {
            let found = |root: PathBuf, flatpak: bool| Found {
                running: client_running(&root),
                used: std::fs::metadata(root.join("config/loginusers.vdf"))
                    .and_then(|m| m.modified())
                    .ok(),
                program: flatpak || super::on_path("steam"),
                root,
            };
            Some(choose(found(native, false), found(flatpak, true)))
        }
        (native, flatpak) => native.or(flatpak),
    }
}

/// What the Flatpak Steam's shortcut runs: a script in the Flatpak's own folder (which its
/// sandbox sees) that hands the start to the launcher outside the sandbox. The launcher
/// itself, its files and Proton are out of the sandbox's sight.
pub fn play_script() -> Option<PathBuf> {
    flatpak_home().map(|h| h.join(".local/share/EchoVR_Launcher/steam-play.sh"))
}

/// Pure: [`play_script`]'s text, for the launcher at `launcher`.
pub fn play_script_text(launcher: &Path) -> String {
    let quoted = format!("'{}'", launcher.to_string_lossy().replace('\'', r"'\''"));
    format!(
        "#!/bin/sh\n\
         # Echo VR Launcher: Steam's \"Echo VR\" starts the game through the launcher, outside\n\
         # Steam's sandbox (org.freedesktop.Flatpak, which the launcher asked for), in the\n\
         # home folder Steam runs in (the sandbox's $HOME is the same path as outside).\n\
         exec flatpak-spawn --host --env=HOME=\"$HOME\" {quoted} --play \"$@\"\n"
    )
}

/// Writes [`play_script`] for the launcher at `launcher` (again when it moved). Returns
/// where it is.
pub fn write_play_script(launcher: &Path) -> Result<PathBuf> {
    let path = play_script().context("no home folder")?;
    let text = play_script_text(launcher);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("make {} executable", path.display()))?;
    }
    Ok(path)
}

/// The D-Bus name a Flatpak needs for `flatpak-spawn --host`.
const SPAWN_NAME: &str = "org.freedesktop.Flatpak";

/// Whether the Flatpak Steam may start programs outside its sandbox (`flatpak-spawn
/// --host`), as its shortcut's script does.
pub fn flatpak_can_spawn() -> bool {
    std::process::Command::new("flatpak")
        .args(["info", "--show-permissions", FLATPAK_ID])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| talks_to_spawn(&String::from_utf8_lossy(&o.stdout)))
}

/// Pure: whether `flatpak info --show-permissions` lets the app talk to [`SPAWN_NAME`].
fn talks_to_spawn(permissions: &str) -> bool {
    let mut session_bus = false;
    for line in permissions.lines().map(str::trim) {
        if line.starts_with('[') {
            session_bus = line == "[Session Bus Policy]";
        } else if session_bus {
            if let Some(policy) = line
                .strip_prefix(SPAWN_NAME)
                .and_then(|l| l.strip_prefix('='))
            {
                return matches!(policy, "talk" | "own");
            }
        }
    }
    false
}

/// Lets the Flatpak Steam start programs outside its sandbox (this user's override; Steam
/// started afterwards has it). The player said yes first: it loosens Steam's sandbox.
pub fn allow_spawn() -> Result<()> {
    flatpak_override(&format!("--talk-name={SPAWN_NAME}"))
}

/// Takes [`allow_spawn`]'s permission back.
pub fn disallow_spawn() -> Result<()> {
    flatpak_override(&format!("--no-talk-name={SPAWN_NAME}"))
}

fn flatpak_override(arg: &str) -> Result<()> {
    let status = std::process::Command::new("flatpak")
        .args(["override", "--user", arg, FLATPAK_ID])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("Couldn't run flatpak")?;
    if !status.success() {
        bail!("flatpak override {arg} failed ({status})");
    }
    Ok(())
}

/// Every Steam install's root this PC may have (whether it's the player's or not): the
/// native one's and the Flatpak's places. For finding SteamVR, wherever it is.
pub fn all_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let app = home.join(".var/app").join(FLATPAK_ID);
    let mut roots: Vec<PathBuf> = root().into_iter().collect();
    for r in [
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
        app.join(".local/share/Steam"),
        app.join("data/Steam"),
    ] {
        if !roots.contains(&r) {
            roots.push(r);
        }
    }
    roots
}

/// The Steam libraries of the install at `root` (`steamapps/libraryfolders.vdf`), its own
/// first: where its games (SteamVR among them) may be.
pub fn libraries(root: &Path) -> Vec<PathBuf> {
    let listed = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf"))
        .ok()
        .and_then(|t| vdf::parse_text(&t).ok())
        .map(|t| library_paths(&t))
        .unwrap_or_default();
    let mut all = vec![root.to_path_buf()];
    for p in listed {
        if !all.contains(&p) {
            all.push(p);
        }
    }
    all
}

/// Pure: the `path`s `libraryfolders.vdf` lists.
fn library_paths(t: &Text) -> Vec<PathBuf> {
    let Some(Text::Map(folders)) = t.get("libraryfolders") else {
        return Vec::new();
    };
    folders
        .iter()
        .filter_map(|(_, f)| f.get("path").and_then(Text::as_str))
        .map(PathBuf::from)
        .collect()
}

/// Whether Flathub's Steam is installed, for this user or system-wide.
#[cfg_attr(windows, allow(dead_code))]
fn flatpak_installed(home: &Path) -> bool {
    [
        home.join(".local/share/flatpak/app").join(FLATPAK_ID),
        Path::new("/var/lib/flatpak/app").join(FLATPAK_ID),
    ]
    .iter()
    .any(|p| p.is_dir())
}

/// A Steam install found, for [`choose`].
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(windows, allow(dead_code))]
struct Found {
    root: PathBuf,
    /// Its client runs now.
    running: bool,
    /// When an account last signed in there (`config/loginusers.vdf`).
    used: Option<std::time::SystemTime>,
    /// Its program is here to run (`steam` on `PATH`; the Flatpak's always is).
    program: bool,
}

/// Pure: which of the native and the Flatpak Steam is the player's. The one running; else
/// the Flatpak when the native one's program is gone (its files left behind); else the one
/// signed in to last; else the native one.
#[cfg_attr(windows, allow(dead_code))]
fn choose(native: Found, flatpak: Found) -> PathBuf {
    if native.running != flatpak.running {
        return if native.running {
            native.root
        } else {
            flatpak.root
        };
    }
    if !native.program {
        return flatpak.root;
    }
    match (native.used, flatpak.used) {
        (Some(n), Some(f)) if f > n => flatpak.root,
        (None, Some(_)) => flatpak.root,
        _ => native.root,
    }
}

/// The `userdata` folder of the account that signed in last (`loginusers.vdf`), else of
/// the one used most recently.
pub fn user_dir(root: &Path) -> Option<PathBuf> {
    let users = root.join("userdata");
    let from_login = std::fs::read_to_string(root.join("config/loginusers.vdf"))
        .ok()
        .and_then(|t| vdf::parse_text(&t).ok())
        .and_then(|t| most_recent_account(&t))
        .map(|id| users.join(id.to_string()))
        .filter(|p| p.is_dir());
    from_login.or_else(|| {
        std::fs::read_dir(&users)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .chars()
                    .all(|c| c.is_ascii_digit())
            })
            .max_by_key(|e| {
                std::fs::metadata(e.path().join("config/localconfig.vdf"))
                    .and_then(|m| m.modified())
                    .ok()
            })
            .map(|e| e.path())
    })
}

/// The account ID of the user marked `MostRecent` in `loginusers.vdf`.
fn most_recent_account(t: &Text) -> Option<u64> {
    let Text::Map(users) = t.get("users")? else {
        return None;
    };
    users
        .iter()
        .find(|(_, u)| u.get("MostRecent").and_then(Text::as_str) == Some("1"))
        .or_else(|| users.first())
        .and_then(|(id, _)| id.parse::<u64>().ok())
        .and_then(|id64| id64.checked_sub(STEAM_ID64_BASE))
}

/// The id Steam gives a non-Steam game: from its executable (as written in the shortcut,
/// quotes included) and its name.
pub fn shortcut_appid(exe: &str, name: &str) -> u32 {
    crc32fast::hash(format!("{exe}{name}").as_bytes()) | 0x8000_0000
}

/// The id `steam://rungameid/` takes for that shortcut.
pub fn game_id(appid: u32) -> u64 {
    (u64::from(appid) << 32) | 0x0200_0000
}

/// A non-Steam game to add.
#[derive(Debug, Clone, PartialEq)]
pub struct Shortcut {
    /// The executable, unquoted.
    pub exe: PathBuf,
    /// Its arguments.
    pub launch_options: String,
    pub icon: Option<PathBuf>,
    /// In SteamVR's library (Steam's "Include in VR Library"). Without SteamVR, Steam would
    /// ask for a headset at every start.
    pub openvr: bool,
}

fn quoted(p: &Path) -> String {
    format!("\"{}\"", p.to_string_lossy())
}

/// `shortcuts.vdf`'s pairs with the launcher's shortcut put in (replacing an earlier one,
/// found by its name); every other game stays as it was. Returns the pairs and its appid.
pub fn with_shortcut(mut pairs: Vec<(String, Bin)>, s: &Shortcut) -> (Vec<(String, Bin)>, u32) {
    let exe = quoted(&s.exe);
    let appid = shortcut_appid(&exe, SHORTCUT_NAME);
    let start_dir = s.exe.parent().map(quoted).unwrap_or_default();
    let entry = Bin::Map(vec![
        ("appid".into(), Bin::Int(appid)),
        ("AppName".into(), Bin::Str(SHORTCUT_NAME.into())),
        ("Exe".into(), Bin::Str(exe)),
        ("StartDir".into(), Bin::Str(start_dir)),
        (
            "icon".into(),
            Bin::Str(
                s.icon
                    .as_deref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
        ),
        ("ShortcutPath".into(), Bin::Str(String::new())),
        ("LaunchOptions".into(), Bin::Str(s.launch_options.clone())),
        ("IsHidden".into(), Bin::Int(0)),
        ("AllowDesktopConfig".into(), Bin::Int(1)),
        ("AllowOverlay".into(), Bin::Int(1)),
        ("OpenVR".into(), Bin::Int(u32::from(s.openvr))),
        ("Devkit".into(), Bin::Int(0)),
        ("DevkitGameID".into(), Bin::Str(String::new())),
        ("DevkitOverrideAppID".into(), Bin::Int(0)),
        ("LastPlayTime".into(), Bin::Int(0)),
        ("FlatpakAppID".into(), Bin::Str(String::new())),
        ("tags".into(), Bin::Map(Vec::new())),
    ]);
    let pos = pairs
        .iter()
        .position(|(k, _)| k.eq_ignore_ascii_case("shortcuts"));
    let list = match pos {
        Some(i) => &mut pairs[i].1,
        None => {
            pairs.push(("shortcuts".into(), Bin::Map(Vec::new())));
            &mut pairs.last_mut().expect("just pushed").1
        }
    };
    if let Bin::Map(games) = list {
        games.retain(|(_, g)| g.get("AppName").and_then(Bin::as_str) != Some(SHORTCUT_NAME));
        // Steam numbers its entries 0, 1, 2, ...: number them again.
        games.push((String::new(), entry));
        for (i, (k, _)) in games.iter_mut().enumerate() {
            *k = i.to_string();
        }
    }
    (pairs, appid)
}

/// Pure: the appid of the launcher's shortcut in `shortcuts.vdf`'s pairs when it runs
/// `s`'s executable (Steam keeps it; it may drop the launch options, which `--play` copes
/// with): then Steam needs no change, and no restart.
pub fn shortcut_in(pairs: &[(String, Bin)], s: &Shortcut) -> Option<u32> {
    let exe = quoted(&s.exe);
    let (_, Bin::Map(games)) = pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("shortcuts"))?
    else {
        return None;
    };
    games.iter().find_map(|(_, g)| {
        let ours = g.get("AppName").and_then(Bin::as_str) == Some(SHORTCUT_NAME)
            && g.get("Exe").and_then(Bin::as_str) == Some(exe.as_str());
        match g.get("appid") {
            Some(Bin::Int(id)) if ours => Some(*id),
            _ => None,
        }
    })
}

/// The launcher's shortcut for the current Steam user, when it is in place (see
/// [`shortcut_in`]).
pub fn installed_shortcut(root: &Path, s: &Shortcut) -> Option<u32> {
    let file = user_dir(root)?.join("config/shortcuts.vdf");
    shortcut_in(&vdf::parse_bin(&std::fs::read(file).ok()?).ok()?, s)
}

/// Pure: `shortcuts.vdf`'s pairs without the launcher's shortcut (found by its name), the
/// other games renumbered; and whether it was there.
pub fn without_shortcut(mut pairs: Vec<(String, Bin)>) -> (Vec<(String, Bin)>, bool) {
    let mut found = false;
    for (k, list) in pairs.iter_mut() {
        if !k.eq_ignore_ascii_case("shortcuts") {
            continue;
        }
        if let Bin::Map(games) = list {
            let before = games.len();
            games.retain(|(_, g)| g.get("AppName").and_then(Bin::as_str) != Some(SHORTCUT_NAME));
            found |= games.len() != before;
            for (i, (k, _)) in games.iter_mut().enumerate() {
                *k = i.to_string();
            }
        }
    }
    (pairs, found)
}

/// Whether the current Steam user has the launcher's shortcut (by its name), whichever
/// launcher it runs.
pub fn has_shortcut(root: &Path) -> bool {
    user_dir(root)
        .and_then(|u| std::fs::read(u.join("config/shortcuts.vdf")).ok())
        .and_then(|b| vdf::parse_bin(&b).ok())
        .is_some_and(|pairs| without_shortcut(pairs).1)
}

/// Takes the launcher's shortcut out of the current Steam user's games. Steam must be
/// closed. False when it wasn't there.
pub fn remove_shortcut(root: &Path) -> Result<bool> {
    let Some(user) = user_dir(root) else {
        return Ok(false);
    };
    let file = user.join("config/shortcuts.vdf");
    let Ok(bytes) = std::fs::read(&file) else {
        return Ok(false);
    };
    let pairs = vdf::parse_bin(&bytes).with_context(|| format!("read {}", file.display()))?;
    let (pairs, found) = without_shortcut(pairs);
    if found {
        replace_file(&file, &vdf::write_bin(&pairs))?;
    }
    Ok(found)
}

/// Writes `bytes` to `path` in one step, keeping the first original as `<name>.echovr-bak`.
fn replace_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.is_file() {
        let backup = path.with_extension(format!(
            "{}.echovr-bak",
            path.extension().and_then(|e| e.to_str()).unwrap_or("vdf")
        ));
        if !backup.exists() {
            std::fs::copy(path, &backup).with_context(|| format!("back up {}", path.display()))?;
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("echovr-tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

/// Adds (or updates) the launcher's shortcut for the current Steam user. Steam must be
/// closed. Returns the shortcut's appid.
pub fn install_shortcut(root: &Path, s: &Shortcut) -> Result<u32> {
    let user = user_dir(root).context("No Steam account found: sign in to Steam once first.")?;
    let file = user.join("config/shortcuts.vdf");
    let pairs = match std::fs::read(&file) {
        Ok(b) => vdf::parse_bin(&b).with_context(|| format!("read {}", file.display()))?,
        Err(_) => Vec::new(),
    };
    let (pairs, appid) = with_shortcut(pairs, s);
    replace_file(&file, &vdf::write_bin(&pairs))?;
    Ok(appid)
}

/// Steam's processes (its client and web helpers) of the install at `root`: the native
/// one's, or those in the Flatpak's sandbox. Each with its pids, the host's first (see
/// [`ns_pids`]).
fn processes(root: &Path) -> Vec<(String, Vec<u32>)> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let flatpak = is_flatpak(root);
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes()
        .iter()
        .filter_map(|(pid, p)| {
            let name = p.name().to_str()?;
            let name = name.strip_suffix(".exe").unwrap_or(name);
            if !matches!(name, "steam" | "steamwebhelper") {
                return None;
            }
            let pid = pid.as_u32();
            (in_flatpak(pid) == flatpak).then(|| {
                let status = std::fs::read_to_string(format!("/proc/{pid}/status"));
                let pids = status.map(|s| ns_pids(&s)).unwrap_or_default();
                (
                    name.to_string(),
                    if pids.is_empty() { vec![pid] } else { pids },
                )
            })
        })
        .collect()
}

/// Whether host process `pid` runs in the Flatpak Steam's sandbox (whose root has its
/// `.flatpak-info`).
fn in_flatpak(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/root/.flatpak-info")).is_ok_and(|info| {
        info.lines()
            .any(|l| l.trim() == format!("name={FLATPAK_ID}"))
    })
}

/// Pure: a process's pids from its `/proc/<pid>/status` (`NSpid:`), from the host's pid
/// namespace to its innermost one: one for a process on the host, two in a Flatpak.
fn ns_pids(status: &str) -> Vec<u32> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("NSpid:"))
        .map(|pids| {
            pids.split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether Steam's client (at `root`) is running.
pub fn running(root: &Path) -> bool {
    !processes(root).is_empty()
}

/// The command that talks to Steam: `steam`, or the Flatpak's (Windows: its `steam.exe`).
fn steam_command(root: &Path) -> std::process::Command {
    if cfg!(windows) {
        return crate::core::process::command(root.join("steam.exe"));
    }
    if is_flatpak(root) {
        let mut c = std::process::Command::new("flatpak");
        c.args(["run", FLATPAK_ID]);
        c
    } else {
        std::process::Command::new("steam")
    }
}

/// Whether SteamVR runs. Closing Steam under it leaves its processes behind, and the
/// Steam started next comes up broken (a second client for the next `steam://` link).
pub fn vr_running() -> bool {
    ["vrserver", "vrmonitor", "vrcompositor"]
        .iter()
        .any(|p| crate::core::launcher::game::program_running(p))
}

/// Where Steam notes the pid of its running client. The Flatpak's notes the pid as its
/// sandbox sees it.
#[cfg(not(windows))]
fn pid_file(root: &Path) -> Option<std::path::PathBuf> {
    if is_flatpak(root) {
        return flatpak_home().map(|h| h.join(".steam/steam.pid"));
    }
    dirs::home_dir().map(|h| h.join(".steam/steam.pid"))
}

/// The pid Steam noted for its running client: in its pid file (the Flatpak's as its
/// sandbox sees it), on Windows in its registry key.
fn noted_pid(root: &Path) -> Option<u32> {
    #[cfg(windows)]
    {
        let _ = root;
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        let pid: u32 = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Valve\Steam\ActiveProcess")
            .ok()?
            .get_value("pid")
            .ok()?;
        Some(pid)
    }
    #[cfg(not(windows))]
    {
        let text = std::fs::read_to_string(pid_file(root)?).ok()?;
        text.trim().parse().ok()
    }
}

/// Pure: Steam is up when the client it noted runs (`alive`, by the pid as Steam sees it)
/// and its web helper (the window, which Steam starts last) does too.
fn ready(noted: Option<u32>, alive: impl Fn(u32) -> bool, webhelper: bool) -> bool {
    noted.is_some_and(|pid| pid > 0 && alive(pid)) && webhelper
}

/// Pure: whether a `steam` process with these pids (the host's first) is the client a pid
/// file names: by its innermost pid, which is the host's for the native Steam and the
/// sandbox's for the Flatpak's.
fn is_client(pids: &[u32], noted: u32) -> bool {
    pids.last() == Some(&noted)
}

/// Whether the client of the Steam at `root` runs (the pid it noted is a running `steam`).
#[cfg_attr(windows, allow(dead_code))]
fn client_running(root: &Path) -> bool {
    noted_pid(root).is_some_and(|noted| {
        processes(root)
            .iter()
            .any(|(name, pids)| name == "steam" && is_client(pids, noted))
    })
}

/// Whether Steam is up now (see [`ready`]).
pub fn is_ready(root: &Path) -> bool {
    let processes = processes(root);
    let alive = |noted: u32| {
        processes
            .iter()
            .any(|(name, pids)| name == "steam" && is_client(pids, noted))
    };
    let webhelper = processes.iter().any(|(name, _)| name == "steamwebhelper");
    ready(noted_pid(root), alive, webhelper)
}

/// Waits (up to `limit`) until Steam is up, so a `steam://` link goes to that client
/// instead of starting another. Returns whether it is.
pub fn wait_ready(root: &Path, limit: Duration) -> bool {
    let started = std::time::Instant::now();
    loop {
        if is_ready(root) {
            return true;
        }
        if started.elapsed() >= limit {
            tracing::warn!("Steam isn't up after {}s", limit.as_secs());
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Closes Steam and waits (up to a minute) until it is gone. Not under SteamVR.
pub fn shutdown(root: &Path) -> Result<()> {
    if !running(root) {
        return Ok(());
    }
    if vr_running() {
        bail!("SteamVR is running. Close SteamVR first: Steam has to restart once for this.");
    }
    steam_command(root)
        .arg("-shutdown")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Couldn't ask Steam to close")?;
    for _ in 0..120 {
        std::thread::sleep(Duration::from_millis(500));
        if !running(root) {
            // Give it a moment to finish writing its files.
            std::thread::sleep(Duration::from_secs(1));
            return Ok(());
        }
    }
    bail!("Steam didn't close. Close it yourself and try again.")
}

/// Starts Steam again, in the background, and waits (up to 90 seconds) until it is up.
pub fn start(root: &Path) -> Result<()> {
    steam_command(root)
        .arg("-silent")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Couldn't start Steam")?;
    if wait_ready(root, Duration::from_secs(90)) {
        // Up, and a moment more for it to take links.
        std::thread::sleep(Duration::from_secs(3));
    }
    Ok(())
}

/// Starts the shortcut `appid` through Steam, starting Steam first when it doesn't run and
/// waiting (up to two minutes) until it is up: a link Steam gets before then starts a
/// second client, or gets lost. `on` hears what it waits for. `Ok(false)`: `cancel` was
/// set first.
pub fn run_when_up(
    root: &Path,
    appid: u32,
    cancel: &std::sync::atomic::AtomicBool,
    on: &mut dyn FnMut(&str),
) -> Result<bool> {
    use std::sync::atomic::Ordering;
    let cancelled = || cancel.load(Ordering::Relaxed);
    let mut started = false;
    if !running(root) {
        on("Starting Steam");
        steam_command(root)
            .arg("-silent")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Couldn't start Steam")?;
        started = true;
    }
    let since = std::time::Instant::now();
    while !is_ready(root) {
        if cancelled() {
            return Ok(false);
        }
        if since.elapsed() > Duration::from_secs(120) {
            bail!("Steam didn't come up within two minutes. Start Steam, then press PLAY again.");
        }
        on("Waiting for Steam");
        std::thread::sleep(Duration::from_millis(500));
    }
    if started {
        // Up, and a moment more for it to take links.
        for _ in 0..6 {
            if cancelled() {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    if cancelled() {
        return Ok(false);
    }
    on("Starting Echo VR");
    open(root, &format!("steam://rungameid/{}", game_id(appid)))
        .context("Couldn't ask Steam to start Echo VR")?;
    Ok(true)
}

/// Hands Steam a `steam://` link (say, to start SteamVR).
pub fn open(root: &Path, link: &str) -> Result<()> {
    steam_command(root)
        .arg(link)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("Couldn't ask Steam to open {link}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_is_up_with_its_client_and_web_helper() {
        let alive = |pid: u32| pid == 42;
        assert!(ready(Some(42), alive, true));
        assert!(!ready(Some(42), alive, false));
        assert!(!ready(Some(7), alive, true));
        assert!(!ready(None, alive, true));
        assert!(!ready(Some(0), |_| true, true));
    }

    #[test]
    fn knows_a_client_by_the_pid_it_noted() {
        // As read on a PC with Flatpak Steam: the host's pid, then the sandbox's.
        let flatpak = ns_pids("Name:\tsteam\nNSpid:\t1254266\t1239\nNSgid:\t1254266\t1239\n");
        assert_eq!(flatpak, vec![1_254_266, 1239]);
        assert!(is_client(&flatpak, 1239));
        assert!(!is_client(&flatpak, 1_254_266));
        let native = ns_pids("Name:\tsteam\nNSpid:\t3790601\n");
        assert!(is_client(&native, 3_790_601));
        assert!(ns_pids("Name:\tsteam\n").is_empty());
        assert!(!is_client(&[], 1));
    }

    #[test]
    fn lists_the_steam_libraries() {
        let t = vdf::parse_text(
            "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/u/.local/share/Steam\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"250820\"\t\t\"5000\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/mnt/games/SteamLibrary\"\n\t}\n}\n",
        )
        .unwrap();
        assert_eq!(
            library_paths(&t),
            vec![
                PathBuf::from("/home/u/.local/share/Steam"),
                PathBuf::from("/mnt/games/SteamLibrary")
            ]
        );
        assert!(library_paths(&vdf::parse_text("\"x\" {}").unwrap()).is_empty());
    }

    #[test]
    fn the_flatpak_hands_the_start_to_the_launcher() {
        let text = play_script_text(Path::new("/home/u/Apps/Echo VR's Launcher.AppImage"));
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(text.ends_with(
            "exec flatpak-spawn --host --env=HOME=\"$HOME\" '/home/u/Apps/Echo VR'\\''s Launcher.AppImage' --play \"$@\"\n"
        ));
    }

    #[test]
    fn reads_whether_the_flatpak_may_spawn() {
        // As `flatpak info --show-permissions com.valvesoftware.Steam` prints it.
        let stock = "[Context]\nshared=ipc;network;\n\n[Session Bus Policy]\ncom.steampowered.*=own\norg.freedesktop.Notifications=talk\n\n[System Bus Policy]\norg.freedesktop.UPower=talk\n";
        assert!(!talks_to_spawn(stock));
        let allowed = stock.replace(
            "org.freedesktop.Notifications=talk",
            "org.freedesktop.Flatpak=talk\norg.freedesktop.Notifications=talk",
        );
        assert!(talks_to_spawn(&allowed));
        assert!(!talks_to_spawn(
            &allowed.replace("Flatpak=talk", "Flatpak=none")
        ));
        // Only the session bus counts.
        assert!(!talks_to_spawn(
            "[System Bus Policy]\norg.freedesktop.Flatpak=talk\n"
        ));
    }

    #[test]
    fn picks_the_players_steam() {
        let t = |s: u64| Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(s));
        let native = Found {
            root: PathBuf::from("/home/u/.local/share/Steam"),
            running: false,
            used: t(100),
            program: true,
        };
        let flatpak = Found {
            root: PathBuf::from("/home/u/.var/app/com.valvesoftware.Steam/.local/share/Steam"),
            running: false,
            used: t(50),
            program: true,
        };
        let pick = |n: &Found, f: &Found| is_flatpak(&choose(n.clone(), f.clone()));
        // Signed in to last, by default.
        assert!(!pick(&native, &flatpak));
        assert!(pick(
            &native,
            &Found {
                used: t(200),
                ..flatpak.clone()
            }
        ));
        // The one running wins.
        assert!(pick(
            &native,
            &Found {
                running: true,
                ..flatpak.clone()
            }
        ));
        assert!(!pick(
            &Found {
                running: true,
                ..native.clone()
            },
            &Found {
                used: t(200),
                ..flatpak.clone()
            }
        ));
        // A native install whose program is gone: its files are only left behind.
        assert!(pick(
            &Found {
                program: false,
                ..native.clone()
            },
            &flatpak
        ));
        assert!(pick(
            &Found {
                used: None,
                ..native.clone()
            },
            &flatpak
        ));
    }

    fn shortcut() -> Shortcut {
        Shortcut {
            exe: PathBuf::from(
                "/home/u/EchoVR/pc-latest/ready-at-dawn-echo-arena/bin/win10/echovr.exe",
            ),
            launch_options: "PROTON_LOG=1 %command% -windowed".into(),
            icon: None,
            openvr: true,
        }
    }

    #[test]
    fn ids_follow_steam() {
        // Steam's id for a non-Steam game: CRC32 of exe + name, top bit set.
        let id = shortcut_appid("\"/usr/bin/game\"", "Game");
        assert!(id & 0x8000_0000 != 0);
        assert_eq!(id, crc32fast::hash(b"\"/usr/bin/game\"Game") | 0x8000_0000);
        assert_eq!(game_id(0x8000_0001), 0x8000_0001_0200_0000);
    }

    #[test]
    fn takes_its_shortcut_out() {
        let other = Bin::Map(vec![("AppName".into(), Bin::Str("Other".into()))]);
        let pairs = vec![("shortcuts".to_string(), Bin::Map(vec![("0".into(), other)]))];
        let (pairs, _) = with_shortcut(pairs, &shortcut());
        let (pairs, found) = without_shortcut(pairs);
        assert!(found);
        let Bin::Map(games) = &pairs[0].1 else {
            panic!()
        };
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].0, "0");
        assert_eq!(
            games[0].1.get("AppName").and_then(Bin::as_str),
            Some("Other")
        );
        assert!(!without_shortcut(pairs).1);
    }

    #[test]
    fn knows_its_shortcut_is_in_place() {
        let (pairs, appid) = with_shortcut(Vec::new(), &shortcut());
        assert_eq!(shortcut_in(&pairs, &shortcut()), Some(appid));
        // Steam dropped the launch options: still in place.
        let mut other = shortcut();
        other.launch_options.clear();
        assert_eq!(shortcut_in(&pairs, &other), Some(appid));
        // The launcher moved: not its shortcut any more.
        other.exe = PathBuf::from("/opt/EchoVR_Launcher");
        assert_eq!(shortcut_in(&pairs, &other), None);
        assert_eq!(shortcut_in(&[], &shortcut()), None);
    }

    #[test]
    fn adds_and_replaces_the_shortcut() {
        let other = Bin::Map(vec![("AppName".into(), Bin::Str("Other".into()))]);
        let pairs = vec![("shortcuts".to_string(), Bin::Map(vec![("0".into(), other)]))];
        let (pairs, appid) = with_shortcut(pairs, &shortcut());
        let Bin::Map(games) = &pairs[0].1 else {
            panic!()
        };
        assert_eq!(games.len(), 2);
        assert_eq!(games[1].0, "1");
        let ours = &games[1].1;
        assert_eq!(ours.get("appid"), Some(&Bin::Int(appid)));
        assert_eq!(
            ours.get("StartDir").and_then(Bin::as_str),
            Some("\"/home/u/EchoVR/pc-latest/ready-at-dawn-echo-arena/bin/win10\"")
        );
        // Again: replaced, not added twice; and it survives the file format.
        let bytes = vdf::write_bin(&with_shortcut(pairs, &shortcut()).0);
        let back = vdf::parse_bin(&bytes).unwrap();
        let Bin::Map(games) = &back[0].1 else {
            panic!()
        };
        assert_eq!(games.len(), 2);
        // No shortcuts.vdf yet: one is made.
        let (fresh, _) = with_shortcut(Vec::new(), &shortcut());
        assert_eq!(fresh[0].0, "shortcuts");
    }

    #[test]
    fn most_recent_user() {
        let t = vdf::parse_text(
            "\"users\"\n{\n\t\"76561197960265738\"\n\t{\n\t\t\"MostRecent\"\t\t\"0\"\n\t}\n\t\"76561197960265829\"\n\t{\n\t\t\"MostRecent\"\t\t\"1\"\n\t}\n}\n",
        )
        .unwrap();
        assert_eq!(most_recent_account(&t), Some(101));
    }
}
