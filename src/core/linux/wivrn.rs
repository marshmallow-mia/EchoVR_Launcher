//! Echo VR in WiVRn's app list: WiVRn's headset app lists the desktop entries whose
//! categories name `X-WiVRn-VR`, and starts one when picked. Echo VR's runs the launcher
//! with `--play --runtime wivrn` (see `core::launcher::headless`). It's kept out of the
//! desktop's own menus (`NoDisplay`, which WiVRn doesn't read).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The entry's file, in the user's applications folder.
const FILE: &str = "de.echovr.launcher.wivrn.desktop";

fn path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("applications").join(FILE))
}

/// Whether Echo VR is in WiVRn's list.
pub fn listed() -> bool {
    path().is_some_and(|p| p.is_file())
}

/// Pure: an argument quoted for a desktop entry's `Exec` (its rules: `"` `` ` `` `$` `\`
/// escaped inside the quotes, `%` doubled).
fn exec_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Pure: the entry for the launcher at `exe`, with the icon at `icon`.
fn entry(exe: &Path, icon: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Echo VR\n\
         Comment=Echo VR on WiVRn, through the Echo VR Launcher\n\
         Exec={} {} {} wivrn\n\
         Icon={}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         Categories=Game;X-WiVRn-VR;\n",
        exec_arg(&exe.to_string_lossy()),
        crate::core::launcher::headless::FLAG,
        crate::core::launcher::headless::RUNTIME_FLAG,
        icon.to_string_lossy(),
    )
}

/// The launcher's icon as a file the entry can name.
fn icon() -> Result<PathBuf> {
    let path = crate::core::paths::data_dir().join("icon.png");
    if !path.is_file() {
        std::fs::create_dir_all(crate::core::paths::data_dir())?;
        std::fs::write(&path, crate::core::desktop_notify::ICON_PNG)
            .with_context(|| format!("write {}", path.display()))?;
    }
    Ok(path)
}

/// Puts Echo VR into WiVRn's list (again, when the launcher moved).
pub fn list() -> Result<()> {
    let exe = super::launcher_exe().context("the launcher's own path is unknown")?;
    let path = path().context("no applications folder")?;
    let text = entry(&exe, &icon()?);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    tracing::info!("Echo VR is in WiVRn's list ({})", path.display());
    Ok(())
}

/// Takes Echo VR out of WiVRn's list.
pub fn unlist() {
    if let Some(p) = path() {
        if std::fs::remove_file(&p).is_ok() {
            tracing::info!("Echo VR is out of WiVRn's list");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_echo_vr_for_wivrn() {
        let text = entry(
            Path::new("/home/u/Apps/Echo VR $1 100%.AppImage"),
            Path::new("/home/u/.local/share/EchoVR_Launcher/icon.png"),
        );
        assert!(text.contains(
            "\nExec=\"/home/u/Apps/Echo VR \\$1 100%%.AppImage\" --play --runtime wivrn\n"
        ));
        assert!(text.contains("\nCategories=Game;X-WiVRn-VR;\n"));
        assert!(text.contains("\nIcon=/home/u/.local/share/EchoVR_Launcher/icon.png\n"));
        assert!(text.starts_with("[Desktop Entry]\n"));
    }
}
