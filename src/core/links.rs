//! `spark://` links -- Spark's, and echo.taxi's (which only open them): joining a match from
//! one, the launcher registering itself for them (Windows, Linux) when no other app has,
//! and a launcher started by a link handing it to the one already running. macOS isn't
//! supported: it delivers links as an Apple event, which the window toolkit doesn't pass
//! on, and Echo VR for PC doesn't run there.

use std::path::PathBuf;

use crate::core::launcher::launch;
use crate::core::paths;

#[cfg(any(windows, target_os = "linux", test))]
const SCHEME: &str = "spark";

/// A match to join from a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The lobby id the game takes (`-lobbyid`).
    pub lobby: String,
    /// `spark://s/`: watch as a spectator.
    pub spectate: bool,
}

/// Reads `spark://c/<id>` (join), `spark://j/<id>` and `spark://s/<id>` (spectate), also
/// behind `https://echo.taxi/`. Anything else isn't a link.
pub fn parse(url: &str) -> Option<Link> {
    let s = url.trim().trim_matches(['"', '\'', '<', '>']);
    let s = ["https://echo.taxi/", "http://echo.taxi/", "echo.taxi/"]
        .iter()
        .find_map(|p| s.strip_prefix(p))
        .unwrap_or(s);
    let rest = s
        .strip_prefix("spark://")
        .or_else(|| s.strip_prefix("spark:%2F%2F"))?;
    let (kind, id) = rest.split_once('/')?;
    let spectate = match kind.to_ascii_lowercase().as_str() {
        "c" | "j" => false,
        "s" => true,
        _ => return None,
    };
    let lobby = launch::lobby_uuid(id.trim_end_matches('/'))?;
    Some(Link { lobby, spectate })
}

/// Who opens `spark://` links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handler {
    /// Nobody yet.
    None,
    Ours,
    /// Another app (Spark): its command.
    Other(String),
    /// Not something this launcher can tell or change here (macOS).
    Unsupported,
}

fn own_exe() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        crate::core::linux::launcher_exe()
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe().ok()
    }
}

/// The desktop entry that makes the launcher open `spark://` links (Linux).
#[cfg(any(target_os = "linux", test))]
fn desktop_entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Echo VR Launcher\nComment=Joins Echo VR matches from spark:// links\nExec=\"{exe}\" %u\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/{SCHEME};\n"
    )
}

#[cfg(any(target_os = "linux", test))]
const DESKTOP_FILE: &str = "echovr-launcher.desktop";

#[cfg(target_os = "linux")]
fn desktop_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("applications").join(DESKTOP_FILE))
}

/// Who opens `spark://` links now.
pub fn handler() -> Handler {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CLASSES_ROOT;
        let command: Option<String> = winreg::RegKey::predef(HKEY_CLASSES_ROOT)
            .open_subkey(format!("{SCHEME}\\shell\\open\\command"))
            .and_then(|k| k.get_value(""))
            .ok();
        let own = own_exe().map(|p| p.to_string_lossy().to_ascii_lowercase());
        match command {
            None => Handler::None,
            Some(c) if own.is_some_and(|o| c.to_ascii_lowercase().contains(&o)) => Handler::Ours,
            // The launcher under its old name (the Echo VR Installer): taken over again.
            Some(c) if c.to_ascii_lowercase().contains("echovr_installer.exe") => Handler::None,
            Some(c) => Handler::Other(c),
        }
    }
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("xdg-mime")
            .args(["query", "default", &format!("x-scheme-handler/{SCHEME}")])
            .output();
        match out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()) {
            Ok(d) if d.is_empty() => Handler::None,
            // Still named after its file is gone (unregistered): nobody.
            Ok(d) if d == DESKTOP_FILE => match desktop_path() {
                Some(p) if p.exists() => Handler::Ours,
                _ => Handler::None,
            },
            Ok(d) => Handler::Other(d),
            Err(_) => Handler::Unsupported,
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Handler::Unsupported
    }
}

/// Makes the launcher open `spark://` links (for this user).
pub fn register() -> anyhow::Result<()> {
    let exe = own_exe().ok_or_else(|| anyhow::anyhow!("the launcher's own path is unknown"))?;
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        let exe = exe.to_string_lossy();
        let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(format!("Software\\Classes\\{SCHEME}"))?;
        key.set_value("", &format!("URL:{SCHEME}"))?;
        key.set_value("URL Protocol", &"")?;
        let (icon, _) = key.create_subkey("DefaultIcon")?;
        icon.set_value("", &format!("\"{exe}\",0"))?;
        let (cmd, _) = key.create_subkey("shell\\open\\command")?;
        cmd.set_value("", &format!("\"{exe}\" \"%1\""))?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let path = desktop_path().ok_or_else(|| anyhow::anyhow!("no applications folder"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, desktop_entry(&exe.to_string_lossy()))?;
        let status = std::process::Command::new("xdg-mime")
            .args([
                "default",
                DESKTOP_FILE,
                &format!("x-scheme-handler/{SCHEME}"),
            ])
            .status()?;
        if !status.success() {
            anyhow::bail!("xdg-mime couldn't set the link handler");
        }
        if let Some(dir) = path.parent() {
            let _ = std::process::Command::new("update-desktop-database")
                .arg(dir)
                .status();
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = exe;
        anyhow::bail!("opening spark:// links isn't supported on this system")
    }
}

/// Stops the launcher opening `spark://` links (only its own registration).
pub fn unregister() {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        if handler() == Handler::Ours {
            let _ = winreg::RegKey::predef(HKEY_CURRENT_USER)
                .delete_subkey_all(format!("Software\\Classes\\{SCHEME}"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(p) = desktop_path() {
            let _ = std::fs::remove_file(p);
        }
        // xdg-mime can't unset a default: drop it from the user's mimeapps.list.
        if let Some(list) = dirs::config_dir().map(|d| d.join("mimeapps.list")) {
            if let Ok(text) = std::fs::read_to_string(&list) {
                let kept = without_ours(&text);
                if kept != text {
                    let _ = std::fs::write(&list, kept);
                }
            }
        }
    }
}

/// A mimeapps.list without the launcher among `spark://`'s handlers.
#[cfg(any(target_os = "linux", test))]
fn without_ours(list: &str) -> String {
    let key = format!("x-scheme-handler/{SCHEME}=");
    let mut out: String = list
        .lines()
        .filter_map(|line| {
            let Some(apps) = line.strip_prefix(&key) else {
                return Some(line.to_string());
            };
            let rest: Vec<&str> = apps
                .split(';')
                .filter(|a| !a.is_empty() && *a != DESKTOP_FILE)
                .collect();
            (!rest.is_empty()).then(|| format!("{key}{};", rest.join(";")))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if list.ends_with('\n') {
        out.push('\n');
    }
    out
}

// ---- handing a link to the launcher that is running ----

fn incoming_file() -> PathBuf {
    paths::data_dir().join("incoming-link")
}

/// Leaves `link` for the launcher (this one, once it is up, or the one already running);
/// returns whether another launcher of this user is running (then this one can exit).
pub fn hand_over(link: &str) -> bool {
    leave(&incoming_file(), link);
    another_launcher_running()
}

/// A link left for the launcher, taken (so it is acted on once).
pub fn take_incoming() -> Option<String> {
    take(&incoming_file())
}

fn leave(file: &std::path::Path, link: &str) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, link.trim());
}

fn take(file: &std::path::Path) -> Option<String> {
    let link = std::fs::read_to_string(file).ok()?;
    let _ = std::fs::remove_file(file);
    Some(link).filter(|l| !l.trim().is_empty())
}

/// Whether another process of this executable runs for this user.
fn another_launcher_running() -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let Ok(me) = std::env::current_exe() else {
        return false;
    };
    let Some(name) = me.file_name().map(|n| n.to_os_string()) else {
        return false;
    };
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_user(UpdateKind::OnlyIfNotSet),
    );
    let own = Pid::from_u32(std::process::id());
    let user = sys.process(own).and_then(|p| p.user_id().cloned());
    sys.processes()
        .values()
        .any(|p| p.pid() != own && p.name() == name && p.user_id().cloned() == user)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0F5C1A2B-3C4D-5E6F-7A8B-9C0D1E2F3A4B";

    #[test]
    fn reads_links() {
        let join = |spectate| {
            Some(Link {
                lobby: ID.into(),
                spectate,
            })
        };
        assert_eq!(parse(&format!("spark://c/{ID}")), join(false));
        assert_eq!(
            parse(&format!("spark://j/{}", ID.to_lowercase())),
            join(false)
        );
        assert_eq!(parse(&format!("spark://s/{ID}/")), join(true));
        assert_eq!(
            parse(&format!("https://echo.taxi/spark://c/{ID}")),
            join(false)
        );
        assert_eq!(
            parse(&format!(
                "<https://echo.taxi/spark://c/{ID}.nakama1_us-east>"
            )),
            join(false)
        );
        for bad in [
            ID.to_string(),
            "spark://x/abc".into(),
            format!("https://example.com/{ID}"),
            "spark://c/".into(),
            String::new(),
        ] {
            assert_eq!(parse(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_left_link_is_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sub/incoming-link");
        leave(&f, &format!(" spark://c/{ID} "));
        assert_eq!(
            take(&f).as_deref(),
            Some(format!("spark://c/{ID}").as_str())
        );
        assert_eq!(take(&f), None);
    }

    #[test]
    fn unregistering_keeps_other_handlers() {
        let list = "[Default Applications]\nx-scheme-handler/spark=echovr-launcher.desktop\ntext/plain=gedit.desktop\n[Added Associations]\nx-scheme-handler/spark=spark.desktop;echovr-launcher.desktop;\n";
        assert_eq!(
            without_ours(list),
            "[Default Applications]\ntext/plain=gedit.desktop\n[Added Associations]\nx-scheme-handler/spark=spark.desktop;\n"
        );
        let untouched = "[Default Applications]\ntext/plain=gedit.desktop";
        assert_eq!(without_ours(untouched), untouched);
    }

    #[test]
    fn desktop_entry_opens_links() {
        let e = desktop_entry("/opt/Echo VR Launcher.AppImage");
        assert!(e.contains("Exec=\"/opt/Echo VR Launcher.AppImage\" %u"));
        assert!(e.contains("MimeType=x-scheme-handler/spark;"));
        assert!(e.starts_with("[Desktop Entry]\n"));
    }
}
