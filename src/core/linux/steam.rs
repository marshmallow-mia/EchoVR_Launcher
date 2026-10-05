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

/// Where Steam is: the usual install, its `~/.steam` link, or the Flatpak's.
pub fn root() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
    .into_iter()
    .find(|p| p.join("config").is_dir())
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
        ("OpenVR".into(), Bin::Int(1)),
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

/// Whether Steam's client is running.
pub fn running() -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes()
        .values()
        .any(|p| matches!(p.name().to_str(), Some("steam") | Some("steamwebhelper")))
}

/// The command that talks to Steam: `steam`, or the Flatpak's.
fn steam_command(root: &Path) -> std::process::Command {
    if root.to_string_lossy().contains("com.valvesoftware.Steam") {
        let mut c = std::process::Command::new("flatpak");
        c.args(["run", "com.valvesoftware.Steam"]);
        c
    } else {
        std::process::Command::new("steam")
    }
}

/// Closes Steam and waits (up to a minute) until it is gone.
pub fn shutdown(root: &Path) -> Result<()> {
    if !running() {
        return Ok(());
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
        if !running() {
            // Give it a moment to finish writing its files.
            std::thread::sleep(Duration::from_secs(1));
            return Ok(());
        }
    }
    bail!("Steam didn't close. Close it yourself and try again.")
}

/// Starts Steam again, in the background.
pub fn start(root: &Path) -> Result<()> {
    steam_command(root)
        .arg("-silent")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Couldn't start Steam")?;
    Ok(())
}

/// Starts the shortcut `appid` through Steam.
pub fn run(root: &Path, appid: u32) -> Result<()> {
    open(root, &format!("steam://rungameid/{}", game_id(appid)))
        .context("Couldn't ask Steam to start Echo VR")
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

    fn shortcut() -> Shortcut {
        Shortcut {
            exe: PathBuf::from(
                "/home/u/EchoVR/pc-latest/ready-at-dawn-echo-arena/bin/win10/echovr.exe",
            ),
            launch_options: "PROTON_LOG=1 %command% -windowed".into(),
            icon: None,
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
