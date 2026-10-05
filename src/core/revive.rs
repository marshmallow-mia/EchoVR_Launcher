//! SteamVR support through Revive: locating/installing Revive, the Revive-injector desktop
//! shortcut, the Meta Horizon store artwork, and Echo VR's entry in SteamVR's library
//! (in Revive's `revive.vrmanifest`).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};

pub const REVIVE_INSTALLER_URL: &str =
    "https://github.com/LibreVR/Revive/releases/download/3.1.1/ReviveInstaller.exe";
/// Pinned: the installer runs elevated, so only this exact build is ever executed.
pub const REVIVE_INSTALLER_SHA256: &str =
    "f4832a6193d55c2477c8da19179457c4f777600f48c67e670015103d7189c091";
pub const DEFAULT_REVIVE_DIR: &str = "C:\\Program Files\\Revive";
pub const REVIVE_INJECTOR: &str = "ReviveInjector.exe";
pub const APP_ID: &str = "ready-at-dawn-echo-arena";
/// Echo VR's key in SteamVR's library.
pub const APP_KEY: &str = "revive.app.ready-at-dawn-echo-arena";
/// Revive's app manifest, in its folder: SteamVR's library lists what it holds.
pub const MANIFEST: &str = "revive.vrmanifest";
/// Where the Meta app keeps Echo's store artwork when its install path can't be read.
const DEFAULT_META_DIR: &str = "C:\\Program Files\\Meta Horizon";
const STORE_ASSETS_SUBDIR: &str =
    "CoreData\\Software\\StoreAssets\\ready-at-dawn-echo-arena_assets";
pub const ARTWORK_ZIP_URL: &str =
    "https://release.echovr.de/stuff/patches/ready-at-dawn-echo-arena_assets.zip";

/// The directory if it contains ReviveInjector.exe, trailing separators trimmed.
fn verify_revive_dir(dir: &str) -> Option<String> {
    let trimmed = dir.trim().trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        return None;
    }
    Path::new(trimmed)
        .join(REVIVE_INJECTOR)
        .is_file()
        .then(|| trimmed.to_string())
}

/// Locates an installed Revive: the uninstall registry's InstallLocation first, then the
/// default folder. The candidate is always verified, since the registry can be stale.
pub fn find_revive_dir() -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    if let Some(dir) = super::platform::revive_install_location() {
        tracing::info!("Revive: registry InstallLocation = '{dir}'");
        if let Some(v) = verify_revive_dir(&dir) {
            return Some(v);
        }
    }
    verify_revive_dir(DEFAULT_REVIVE_DIR)
}

/// Polls [`find_revive_dir`] until Revive shows up or the timeout elapses.
pub fn wait_for_revive_dir(timeout: std::time::Duration) -> Option<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(d) = find_revive_dir() {
            return Some(d);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Downloads the pinned Revive installer into the per-user download dir and verifies it.
pub fn download_installer(
    cancel: &AtomicBool,
    on: &mut dyn FnMut(super::download::Progress),
) -> Result<PathBuf> {
    let job = super::download::Job {
        url: REVIVE_INSTALLER_URL.into(),
        dir: super::paths::downloads_dir().join("revive"),
        filename: "ReviveInstaller.exe".into(),
        use_mirror: false,
        fresh: false,
        extract: false,
    };
    let path = super::download::run(&job, cancel, on)?;
    if !super::download::sha256_matches(&path, REVIVE_INSTALLER_SHA256) {
        let _ = std::fs::remove_file(&path);
        bail!("The downloaded Revive installer failed its integrity check. Please try again.");
    }
    Ok(path)
}

/// Arguments of the injector shortcut, in the order PLAY passes them: the game, the
/// "can't press any buttons in-game" fix, the launch options (`game_args`, already joined
/// as a command line), and the app.
pub fn injector_arguments(exe: &str, game_args: &str) -> String {
    let args = if game_args.is_empty() {
        String::new()
    } else {
        format!(" {game_args}")
    };
    format!("\"{exe}\" -nosymbollookup{args} /app {APP_ID}")
}

/// Creates the desktop shortcut `name` launching Echo VR through the Revive injector, with
/// the launch options (`game_args`, joined as a command line).
pub fn create_injector_shortcut(
    name: &str,
    revive_dir: &str,
    exe: &Path,
    game_args: &str,
) -> Result<()> {
    let injector = Path::new(revive_dir).join(REVIVE_INJECTOR);
    let exe_abs = std::path::absolute(exe).unwrap_or_else(|_| exe.to_path_buf());
    let exe_str = exe_abs.to_string_lossy().replace('/', "\\");
    super::platform::create_shortcut(
        name,
        &injector,
        Some(&injector_arguments(&exe_str, game_args)),
        Some(Path::new(revive_dir)),
        Some(&injector),
    )
}

/// Where Echo's store artwork goes: inside the Meta app's install (from the registry,
/// which only administrators can change), else its default folder.
pub fn store_assets_dir() -> PathBuf {
    let base = super::platform::oculus_base_path().unwrap_or_else(|| DEFAULT_META_DIR.into());
    Path::new(base.trim_end_matches(['\\', '/'])).join(STORE_ASSETS_SUBDIR)
}

/// Whether the game artwork is in place (its folder has files).
pub fn artwork_installed() -> bool {
    std::fs::read_dir(store_assets_dir()).is_ok_and(|mut d| d.next().is_some())
}

/// Downloads the game artwork and extracts it into the Meta Horizon store assets.
pub fn install_artwork(cancel: &AtomicBool) -> Result<()> {
    let dir = super::paths::downloads_dir().join("revive");
    std::fs::create_dir_all(&dir)?;
    let zip = dir.join(format!("{APP_ID}_assets.zip"));
    super::http::download_to(ARTWORK_ZIP_URL, &zip, Some(cancel))
        .context("Downloading the artwork failed")?;
    let dest = store_assets_dir();
    std::fs::create_dir_all(&dest).with_context(|| format!("create {}", dest.display()))?;
    super::zip::extract(&zip, &dest, cancel)?;
    let _ = std::fs::remove_file(&zip);
    Ok(())
}

/// True for errors that elevation would fix.
pub fn needs_elevation(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>().is_some_and(|io| {
            io.kind() == std::io::ErrorKind::PermissionDenied
                || io.raw_os_error() == Some(5)   // ERROR_ACCESS_DENIED
                || io.raw_os_error() == Some(740) // ERROR_ELEVATION_REQUIRED
        })
    })
}

// ---- SteamVR's library (revive.vrmanifest) ----

/// Echo VR's entry for SteamVR's library: Revive's injector starting `exe` with the
/// launch options (`game_args`, joined as a command line), as PLAY does.
pub fn library_entry(exe: &Path, game_args: &str) -> serde_json::Value {
    let exe = exe.to_string_lossy().replace('/', "\\");
    let image = store_assets_dir().join("cover_landscape_image_large.png");
    serde_json::json!({
        "app_key": APP_KEY,
        "launch_type": "binary",
        "binary_path_windows": REVIVE_INJECTOR,
        "arguments": injector_arguments(&exe, game_args),
        "action_manifest_path": "Input/action_manifest.json",
        "image_path": image.to_string_lossy(),
        "strings": { "en_us": { "name": "Echo VR" } }
    })
}

/// The manifest text with Echo VR's entry put in (or `None`: taken out), replacing any
/// earlier one; every other app stays as it was.
pub fn manifest_with(text: &str, entry: Option<serde_json::Value>) -> Result<String> {
    let mut doc: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({ "source": "builtin", "applications": [] })
    } else {
        serde_json::from_str(text).context("revive.vrmanifest isn't valid JSON")?
    };
    let apps = doc
        .get_mut("applications")
        .and_then(serde_json::Value::as_array_mut)
        .context("revive.vrmanifest has no application list")?;
    apps.retain(|a| a.get("app_key").and_then(serde_json::Value::as_str) != Some(APP_KEY));
    apps.extend(entry);
    Ok(serde_json::to_string_pretty(&doc)?)
}

/// Pure: whether the manifest `text`'s Echo VR entry starts the game in folder `root`.
pub fn entry_points_into(text: &str, root: &str) -> bool {
    let norm = |s: &str| s.replace('/', "\\").to_ascii_lowercase();
    let root = norm(root);
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|doc| {
            doc.get("applications")?.as_array()?.iter().find_map(|a| {
                (a.get("app_key")?.as_str()? == APP_KEY)
                    .then(|| {
                        a.get("arguments")
                            .and_then(serde_json::Value::as_str)
                            .map(norm)
                    })
                    .flatten()
            })
        })
        .is_some_and(|args| !root.is_empty() && args.contains(&root))
}

/// Whether SteamVR's library entry for Echo VR starts the game in folder `root`.
pub fn library_points_into(root: &str) -> bool {
    find_revive_dir()
        .and_then(|dir| std::fs::read_to_string(Path::new(&dir).join(MANIFEST)).ok())
        .is_some_and(|text| entry_points_into(&text, root))
}

/// Puts Echo VR into SteamVR's library (`entry`), or takes it out (`None`). Needs
/// administrator rights where Revive lives in Program Files.
pub fn set_library_entry(entry: Option<serde_json::Value>) -> Result<()> {
    let dir = find_revive_dir().context("Revive is not installed")?;
    let path = Path::new(&dir).join(MANIFEST);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let out = manifest_with(&text, entry)?;
    let tmp = path.with_extension("vrmanifest.tmp");
    std::fs::write(&tmp, out).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_entry_shape() {
        let exe =
            Path::new("C:/EchoVR/versions/pc-latest/ready-at-dawn-echo-arena/bin/win10/echovr.exe");
        let e = library_entry(exe, "-windowed");
        assert_eq!(e["app_key"], APP_KEY);
        assert_eq!(e["binary_path_windows"], REVIVE_INJECTOR);
        let args = e["arguments"].as_str().unwrap();
        assert!(args.starts_with("\"C:\\EchoVR\\versions\\pc-latest\\"));
        assert!(args.ends_with("-nosymbollookup -windowed /app ready-at-dawn-echo-arena"));
        assert_eq!(e["strings"]["en_us"]["name"], "Echo VR");
    }

    #[test]
    fn knows_where_its_entry_points() {
        let exe =
            Path::new("C:/EchoVR/versions/pc-latest/ready-at-dawn-echo-arena/bin/win10/echovr.exe");
        let text = manifest_with("", Some(library_entry(exe, ""))).unwrap();
        assert!(entry_points_into(&text, "C:/EchoVR/versions/pc-latest"));
        assert!(entry_points_into(&text, "c:\\echovr\\versions\\PC-LATEST"));
        assert!(!entry_points_into(&text, "C:/EchoVR/versions/echo-2019"));
        assert!(!entry_points_into("", "C:/EchoVR"));
    }

    #[test]
    fn manifest_keeps_other_apps() {
        let text = r#"{"source":"builtin","applications":[
            {"app_key":"revive.app.other","arguments":"/app other"},
            {"app_key":"revive.app.ready-at-dawn-echo-arena","arguments":"old"}]}"#;
        let entry = serde_json::json!({"app_key": APP_KEY, "arguments": "new"});
        let out: serde_json::Value =
            serde_json::from_str(&manifest_with(text, Some(entry)).unwrap()).unwrap();
        let apps = out["applications"].as_array().unwrap();
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0]["app_key"], "revive.app.other");
        assert_eq!(apps[1]["arguments"], "new");
        let removed: serde_json::Value =
            serde_json::from_str(&manifest_with(text, None).unwrap()).unwrap();
        assert_eq!(removed["applications"].as_array().unwrap().len(), 1);
        // No manifest yet: one is made.
        let fresh: serde_json::Value = serde_json::from_str(
            &manifest_with("", Some(serde_json::json!({"app_key": APP_KEY}))).unwrap(),
        )
        .unwrap();
        assert_eq!(fresh["applications"].as_array().unwrap().len(), 1);
        assert!(manifest_with("not json", None).is_err());
    }

    #[test]
    fn injector_args() {
        let exe = "C:\\EchoVR\\ready-at-dawn-echo-arena\\bin\\win10\\echovr.exe";
        assert_eq!(
            injector_arguments(exe, ""),
            format!("\"{exe}\" -nosymbollookup /app ready-at-dawn-echo-arena")
        );
        assert_eq!(
            injector_arguments(exe, "-windowed -lobbyid X"),
            format!("\"{exe}\" -nosymbollookup -windowed -lobbyid X /app ready-at-dawn-echo-arena")
        );
    }

    #[test]
    fn elevation_detection() {
        let e = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            .context("x");
        assert!(needs_elevation(&e));
        assert!(!needs_elevation(&anyhow::anyhow!("network")));
    }
}
