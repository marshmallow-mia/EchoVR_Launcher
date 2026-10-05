//! Desktop notifications: the freedesktop notification service on Linux (D-Bus), toasts
//! on Windows. Nothing on macOS (the launcher isn't packaged there). Best effort: a
//! notification that can't be shown is only logged.

// No tray on macOS: much of this is for Windows and Linux only.
#![cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]

/// The launcher's icon, for the notification (and the tray).
pub const ICON_PNG: &[u8] = include_bytes!("../../assets/img/icon.png");

/// Shows `summary` with `body`.
pub fn show(summary: &str, body: &str) {
    #[cfg(any(windows, target_os = "linux"))]
    {
        let mut n = notify_rust::Notification::new();
        n.appname("Echo VR Launcher").summary(summary).body(body);
        #[cfg(target_os = "linux")]
        if let Some(icon) = icon_file() {
            n.icon(&icon.to_string_lossy());
        }
        if let Err(e) = n.show() {
            tracing::info!("desktop notification not shown: {e}");
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    tracing::info!("desktop notification (not on this OS): {summary}: {body}");
}

/// The icon as a file the notification service can read (the data folder's copy).
#[cfg(target_os = "linux")]
fn icon_file() -> Option<std::path::PathBuf> {
    let path = super::paths::data_dir().join("icon.png");
    if std::fs::metadata(&path).map(|m| m.len()).ok() != Some(ICON_PNG.len() as u64) {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, ICON_PNG).ok()?;
    }
    Some(path)
}
