//! Starting the servers with the PC: the server host (`--server-host`) at sign-in
//! (Windows' Run key, Linux' autostart folder), which starts the servers set to run.

use anyhow::Result;

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "Echo VR Launcher (game servers)";
#[cfg(target_os = "linux")]
const AUTOSTART_FILE: &str = "echovr-launcher-servers.desktop";

/// Pure: the Linux autostart entry starting `exe` as the server host.
#[cfg(any(target_os = "linux", test))]
fn entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Echo VR Launcher (game servers)\nComment=Starts the Echo VR game servers set to run\nExec=\"{exe}\" {}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        super::FLAG
    )
}

#[cfg(target_os = "linux")]
fn path() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("autostart").join(AUTOSTART_FILE))
}

/// Starts the server host at sign-in (`on`), or not any more.
pub fn set(on: bool) -> Result<()> {
    #[cfg(windows)]
    {
        use anyhow::Context;
        use winreg::enums::HKEY_CURRENT_USER;
        let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(RUN_KEY)
            .context("the Run key")?;
        if on {
            let exe = crate::core::tray::exe_path().context("no executable path")?;
            key.set_value(RUN_VALUE, &format!("\"{}\" {}", exe.display(), super::FLAG))
                .context("writing the Run key")?;
        } else {
            let _ = key.delete_value(RUN_VALUE);
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        use anyhow::Context;
        let path = path().context("no config folder")?;
        if on {
            let exe = crate::core::tray::exe_path().context("no executable path")?;
            std::fs::create_dir_all(path.parent().context("no autostart folder")?)?;
            std::fs::write(&path, entry(&exe.to_string_lossy()))
                .with_context(|| format!("writing {}", path.display()))?;
        } else {
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = on;
        anyhow::bail!("Game servers run on Windows and Linux.")
    }
}

/// Whether the server host starts at sign-in.
pub fn on() -> bool {
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
        path().is_some_and(|p| p.exists())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn autostart_runs_the_server_host() {
        let e = super::entry("/opt/Echo VR/EchoVR_Launcher");
        assert!(e.contains("Exec=\"/opt/Echo VR/EchoVR_Launcher\" --server-host\n"));
    }
}
