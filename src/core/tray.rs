//! The tray: the launcher's executable started with `--tray` (`ui::tray`), with no window,
//! looking for updates while the launcher is closed. This side starts and stops it, tells
//! whether it (or a launcher window) runs, passes requests between them as files in the
//! data folder, and starts it at login (Windows' Run key, Linux' autostart folder).

// No tray on macOS: much of this is for Windows and Linux only.
#![cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;

use super::paths;

/// The tray's flag.
pub const FLAG: &str = "--tray";

/// What the processes of this executable (of this user) run as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Tray,
    Window,
    /// Linux' PLAY through Steam, or the elevated helper: no window of their own.
    Other,
}

/// Pure: what a process with these arguments (after the program) is.
fn role(args: &[OsString]) -> Role {
    match args.first().and_then(|a| a.to_str()) {
        Some(FLAG) => Role::Tray,
        Some(a) if a == super::linux::PLAY_FLAG || a == super::elevation::HELPER_FLAG => {
            Role::Other
        }
        _ => Role::Window,
    }
}

/// The other processes of this executable run by this user, with their roles.
fn others() -> Vec<(sysinfo::Pid, Role)> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let Ok(me) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(name) = me.file_name().map(|n| n.to_os_string()) else {
        return Vec::new();
    };
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_user(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet),
    );
    let own = Pid::from_u32(std::process::id());
    let user = sys.process(own).and_then(|p| p.user_id().cloned());
    sys.processes()
        .values()
        .filter(|p| p.pid() != own && p.name() == name && p.user_id().cloned() == user)
        .map(|p| (p.pid(), role(p.cmd().get(1..).unwrap_or_default())))
        .collect()
}

/// Whether a tray runs (another process than this one).
pub fn running() -> bool {
    others().iter().any(|(_, r)| *r == Role::Tray)
}

/// Whether a launcher window runs (another process than this one).
pub fn window_running() -> bool {
    others().iter().any(|(_, r)| *r == Role::Window)
}

fn exe() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        super::linux::launcher_exe()
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe().ok()
    }
}

/// Starts the tray unless it runs (not on macOS: no tray there).
pub fn start() {
    if !cfg!(any(windows, target_os = "linux")) || running() {
        return;
    }
    let Some(exe) = exe() else {
        return;
    };
    let mut cmd = super::process::command(&exe);
    cmd.arg(FLAG)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(_) => tracing::info!("tray started"),
        Err(e) => tracing::warn!("tray: couldn't start it: {e}"),
    }
}

/// Ends every tray of this user.
pub fn quit() {
    use sysinfo::{ProcessesToUpdate, System};
    let trays: Vec<sysinfo::Pid> = others()
        .into_iter()
        .filter(|(_, r)| *r == Role::Tray)
        .map(|(p, _)| p)
        .collect();
    if trays.is_empty() {
        return;
    }
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&trays), true);
    for pid in trays {
        if let Some(p) = sys.process(pid) {
            p.kill();
        }
    }
    tracing::info!("tray quit");
}

// ---- requests between the window and the tray ----

fn request_file(name: &str) -> PathBuf {
    paths::data_dir().join(name)
}

fn leave(name: &str) {
    let f = request_file(name);
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, b"1");
}

fn take(name: &str) -> bool {
    std::fs::remove_file(request_file(name)).is_ok()
}

/// The tray asks the running window to come forward.
pub fn ask_window_forward() {
    leave("show-window");
}

/// The window: whether the tray asked it to come forward (once).
pub fn take_window_forward() -> bool {
    take("show-window")
}

/// The window asks the tray to look for updates now (Settings changed, an update done).
pub fn ask_check() {
    leave("tray-check");
}

/// The tray: whether it was asked to look now (once).
pub fn take_check() -> bool {
    take("tray-check")
}

/// Opens the launcher's window: brings the running one forward, or starts one.
pub fn open_window() {
    if window_running() {
        ask_window_forward();
        return;
    }
    let Some(exe) = exe() else {
        return;
    };
    let mut cmd = super::process::command(&exe);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Err(e) = cmd.spawn() {
        tracing::warn!("tray: couldn't open the launcher: {e}");
    }
}

// ---- starting at login ----

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "Echo VR Launcher (updates)";
#[cfg(any(target_os = "linux", test))]
const AUTOSTART_FILE: &str = "echovr-launcher-tray.desktop";

/// Pure: the Linux autostart entry starting `exe` as the tray.
#[cfg(any(target_os = "linux", test))]
fn autostart_entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Echo VR Launcher (updates)\nComment=Looks for updates of Echo VR, its plugins and the launcher\nExec=\"{exe}\" {FLAG}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
    )
}

#[cfg(target_os = "linux")]
fn autostart_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("autostart").join(AUTOSTART_FILE))
}

/// Starts the tray at login (`on`), or not any more.
pub fn set_autostart(on: bool) -> Result<()> {
    #[cfg(windows)]
    {
        use anyhow::Context;
        use winreg::enums::HKEY_CURRENT_USER;
        let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(RUN_KEY)
            .context("the Run key")?;
        if on {
            let exe = exe().context("no executable path")?;
            key.set_value(RUN_VALUE, &format!("\"{}\" {FLAG}", exe.display()))
                .context("writing the Run key")?;
        } else {
            let _ = key.delete_value(RUN_VALUE);
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        use anyhow::Context;
        let path = autostart_path().context("no config folder")?;
        if on {
            let exe = exe().context("no executable path")?;
            std::fs::create_dir_all(path.parent().context("no autostart folder")?)?;
            std::fs::write(&path, autostart_entry(&exe.to_string_lossy()))
                .with_context(|| format!("writing {}", path.display()))?;
        } else {
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = on;
        Ok(())
    }
}

/// Whether the tray starts at login.
pub fn autostart_on() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .and_then(|k| k.get_value::<String, _>(RUN_VALUE))
            .is_ok()
    }
    #[cfg(target_os = "linux")]
    {
        autostart_path().is_some_and(|p| p.exists())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_trays_from_windows() {
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(role(&args(&["--tray"])), Role::Tray);
        assert_eq!(role(&args(&[])), Role::Window);
        assert_eq!(role(&args(&["spark://c/x"])), Role::Window);
        assert_eq!(role(&args(&[crate::core::linux::PLAY_FLAG])), Role::Other);
        assert_eq!(
            role(&args(&[crate::core::elevation::HELPER_FLAG, "x"])),
            Role::Other
        );
    }

    #[test]
    fn autostart_runs_the_tray() {
        let e = autostart_entry("/opt/Echo VR/EchoVR_Launcher");
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("Exec=\"/opt/Echo VR/EchoVR_Launcher\" --tray\n"));
    }
}
