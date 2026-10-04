//! The bundled Android platform-tools (just `adb` and what it links against).
//!
//! Extracted once per process into a per-user cache directory. The Java version copied
//! the files into the shared temp dir on every call -- on Linux that is the world-writable
//! `/tmp`, where another local user could plant their own `adb` -- and several threads
//! rewrote the binary concurrently.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};

pub const VERSION: &str = "37.0.1";

#[cfg(windows)]
const FILES: &[(&str, &[u8])] = &[
    (
        "adb.exe",
        include_bytes!("../../../assets/platform-tools/windows/adb.exe"),
    ),
    (
        "AdbWinApi.dll",
        include_bytes!("../../../assets/platform-tools/windows/AdbWinApi.dll"),
    ),
    (
        "AdbWinUsbApi.dll",
        include_bytes!("../../../assets/platform-tools/windows/AdbWinUsbApi.dll"),
    ),
    (
        "libwinpthread-1.dll",
        include_bytes!("../../../assets/platform-tools/windows/libwinpthread-1.dll"),
    ),
    (
        "NOTICE.txt",
        include_bytes!("../../../assets/platform-tools/windows/NOTICE.txt"),
    ),
];
#[cfg(windows)]
const BINARY: &str = "adb.exe";

#[cfg(target_os = "macos")]
const FILES: &[(&str, &[u8])] = &[
    (
        "adb",
        include_bytes!("../../../assets/platform-tools/macos/adb"),
    ),
    (
        "NOTICE.txt",
        include_bytes!("../../../assets/platform-tools/macos/NOTICE.txt"),
    ),
];
#[cfg(target_os = "macos")]
const BINARY: &str = "adb";

#[cfg(not(any(windows, target_os = "macos")))]
const FILES: &[(&str, &[u8])] = &[
    (
        "adb",
        include_bytes!("../../../assets/platform-tools/linux/adb"),
    ),
    (
        "NOTICE.txt",
        include_bytes!("../../../assets/platform-tools/linux/NOTICE.txt"),
    ),
];
#[cfg(not(any(windows, target_os = "macos")))]
const BINARY: &str = "adb";

/// ChromeOS' Linux container ships its own adb that is wired to the host's USB.
fn is_chrome_os() -> bool {
    Path::new("/opt/google/cros-containers/etc/lsb-release").exists()
}

pub fn dir() -> PathBuf {
    crate::core::paths::cache_dir().join(format!("platform-tools-{VERSION}"))
}

/// Path to a ready-to-run adb, extracting the bundle on first use.
pub fn binary() -> Result<PathBuf> {
    static READY: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    READY
        .get_or_init(|| {
            if cfg!(target_os = "linux") && is_chrome_os() {
                tracing::info!("adb: ChromeOS detected, using the system adb");
                return Ok(PathBuf::from("adb"));
            }
            extract().map_err(|e| format!("{e:#}"))
        })
        .clone()
        .map_err(|e| anyhow::anyhow!(e))
}

fn extract() -> Result<PathBuf> {
    let dir = dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    for (name, bytes) in FILES {
        let path = dir.join(name);
        let up_to_date = std::fs::read(&path).is_ok_and(|existing| existing == *bytes);
        if !up_to_date {
            // Write beside, then rename: a running adb server keeps the old inode.
            let tmp = dir.join(format!(".{name}.tmp"));
            std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
            }
            if let Err(e) = std::fs::rename(&tmp, &path) {
                // Windows refuses to replace an exe that a running adb server holds open.
                let _ = std::fs::remove_file(&tmp);
                if !path.exists() {
                    return Err(e).with_context(|| format!("install {}", path.display()));
                }
                tracing::warn!(
                    "adb: could not refresh {} ({e}); using the existing copy",
                    path.display()
                );
            }
        }
    }
    let bin = dir.join(BINARY);
    tracing::info!("adb: binary {}", bin.display());
    Ok(bin)
}
