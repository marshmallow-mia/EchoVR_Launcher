//! OS integration: registry lookups, shortcuts, opening folders, admin detection.
//!
//! Registry values are read with the registry API instead of scraping PowerShell output
//! (which returned the string "null" on any failure), and `.lnk` files are written with
//! `mslnk` instead of a PowerShell script whose quoting broke on the Revive arguments.

use std::path::Path;

use anyhow::Result;

/// `HKLM\SOFTWARE\WOW6432Node\Oculus VR, LLC\Oculus` -> `Base` (e.g. `C:\Program Files\Oculus\`).
/// Reading it does not need admin.
#[cfg(windows)]
pub fn oculus_base_path() -> Option<String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let key = winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\WOW6432Node\\Oculus VR, LLC\\Oculus")
        .ok()?;
    let base: String = key.get_value("Base").ok()?;
    let base = base.trim().to_string();
    (!base.is_empty()).then_some(base)
}

#[cfg(not(windows))]
pub fn oculus_base_path() -> Option<String> {
    None
}

/// Every Meta (Oculus) app library folder set up in the Meta Quest app:
/// `HKCU\Software\Oculus VR, LLC\Oculus\Libraries\{id}` -> `OriginalPath`. Games are in
/// its `Software` folder.
#[cfg(windows)]
pub fn meta_libraries() -> Vec<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    let Ok(libraries) = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Oculus VR, LLC\\Oculus\\Libraries")
    else {
        return Vec::new();
    };
    libraries
        .enum_keys()
        .filter_map(Result::ok)
        .filter_map(|id| libraries.open_subkey(id).ok())
        .filter_map(|lib| lib.get_value::<String, _>("OriginalPath").ok())
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

#[cfg(not(windows))]
pub fn meta_libraries() -> Vec<String> {
    Vec::new()
}

/// InstallLocation of an uninstall entry whose DisplayName contains "Revive".
#[cfg(windows)]
pub fn revive_install_location() -> Option<String> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
    let hklm = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        let Ok(uninstall) = hklm.open_subkey_with_flags(
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            KEY_READ | view,
        ) else {
            continue;
        };
        for name in uninstall.enum_keys().filter_map(Result::ok) {
            let Ok(k) = uninstall.open_subkey_with_flags(&name, KEY_READ | view) else {
                continue;
            };
            let display: String = k.get_value("DisplayName").unwrap_or_default();
            if display.to_lowercase().contains("revive") {
                let loc: String = k.get_value("InstallLocation").unwrap_or_default();
                if !loc.trim().is_empty() {
                    return Some(loc.trim().to_string());
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn revive_install_location() -> Option<String> {
    None
}

/// Creates a desktop shortcut (Windows `.lnk`) or an application entry (Linux `.desktop`).
pub fn create_shortcut(
    name: &str,
    target: &Path,
    args: Option<&str>,
    working_dir: Option<&Path>,
    icon: Option<&Path>,
) -> Result<()> {
    #[cfg(windows)]
    {
        use anyhow::Context;
        let desktop = dirs::desktop_dir().context("Couldn't find your Desktop folder")?;
        let lnk = desktop.join(format!("{name}.lnk"));
        let mut sl = mslnk::ShellLink::new(target)
            .map_err(|e| anyhow::anyhow!("Couldn't create the shortcut: {e:?}"))?;
        sl.set_arguments(args.map(str::to_string));
        sl.set_working_dir(working_dir.map(|p| p.to_string_lossy().into_owned()));
        sl.set_icon_location(icon.map(|p| p.to_string_lossy().into_owned()));
        sl.create_lnk(&lnk)
            .map_err(|e| anyhow::anyhow!("Couldn't write {}: {e:?}", lnk.display()))?;
        tracing::info!("shortcut: {} -> {}", lnk.display(), target.display());
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use anyhow::Context;
        let _ = icon;
        let quote = |p: &str| format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\""));
        let exec = match args {
            Some(a) if !a.is_empty() => format!("{} {a}", quote(&target.to_string_lossy())),
            _ => quote(&target.to_string_lossy()),
        };
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\nPath={}\nTerminal=false\n",
            working_dir.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
        );
        let dir = dirs::data_local_dir()
            .context("no data dir")?
            .join("applications");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(format!("{}.desktop", name.to_lowercase().replace(' ', "-")));
        std::fs::write(&file, entry)?;
        tracing::info!("shortcut: {}", file.display());
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (name, target, args, working_dir, icon);
        anyhow::bail!("Desktop shortcuts are only supported on Windows and Linux.")
    }
}

/// Free bytes on the disk holding `path`. Windows asks about that folder only: listing
/// every disk can wait seconds on a sleeping network or optical drive.
#[cfg(windows)]
pub fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path = std::path::absolute(path).ok()?;
    // A new library folder may not exist yet: ask about its nearest existing parent.
    let dir = path.ancestors().find(|p| p.is_dir())?;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the totals may be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

/// Free bytes on the disk holding `path` (the longest matching mount point).
#[cfg(not(windows))]
pub fn free_space(path: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let path = std::path::absolute(path).ok()?;
    disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

pub fn open_folder(path: &Path) -> Result<()> {
    tracing::info!("opening folder {}", path.display());
    open::that_detached(path).map_err(|e| {
        tracing::warn!("couldn't open {}: {e}", path.display());
        anyhow::anyhow!("Couldn't open {}: {e}", path.display())
    })
}

/// Opens `url` in the browser. Err when no opener could be started; Ok only means one
/// started (xdg-open can still fail quietly, see core::linux::prepare_desktop_env).
pub fn try_open_url(url: &str) -> std::io::Result<()> {
    let shown = crate::core::download::redact(url);
    match open::that_detached(url) {
        Ok(()) => {
            tracing::info!("opened {shown} in the browser");
            Ok(())
        }
        Err(e) => {
            tracing::warn!("couldn't open {shown} in the browser: {e}");
            Err(e)
        }
    }
}

pub fn open_url(url: &str) {
    let _ = try_open_url(url);
}
