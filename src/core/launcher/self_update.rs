//! Updating the launcher itself, as its games and plugins update: the release's zip for
//! this system (from release.echovr.de, else the GitHub release), checked against that
//! release's `SHA256SUMS`, unpacked beside the running executable. Each file it replaces
//! is moved aside first, as `<name>.old` (Windows can't overwrite a running exe or a loaded
//! DLL, but can rename them), and the old ones go at the next start. Then the launcher
//! starts again: the new one waits for this one to end ([`AFTER_FLAG`]).
//!
//! Not on macOS (no build of it) nor for an AppImage (one file to replace, not a folder):
//! there the release page opens, as before.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use super::versions::Step;
use crate::core::{download, http, paths};

/// Where the release downloads are, by file name (with their `SHA256SUMS`).
const MIRROR: &str = "https://release.echovr.de/launcher/download";
/// The GitHub release's downloads, `{v}` its version.
const GITHUB: &str = "https://github.com/marshmallow-mia/EchoVR_Launcher/releases/download/v{v}";
/// The checksums file beside the downloads.
const SUMS: &str = "SHA256SUMS";
/// The folder the zips have the launcher's files in.
const ZIP_FOLDER: &str = "EchoVR_Launcher/";
/// What a replaced file is renamed to until the next start.
const OLD: &str = ".old";
/// Where an update is unpacked before it goes in, beside the executable.
const STAGING: &str = ".launcher-update";
/// The new launcher's first argument after an update, then the old one's process id: it
/// waits for that one to end before it opens.
pub const AFTER_FLAG: &str = "--after-update";

/// The release download for this system, for version `v` ("0.11.6").
fn asset(v: &str) -> Option<String> {
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        return None;
    };
    Some(format!("Echo_VR_Launcher-{v}-{os}.zip"))
}

/// The running launcher's executable, the file an update replaces.
fn executable() -> Option<PathBuf> {
    if std::env::var_os("APPIMAGE").is_some() {
        return None;
    }
    std::env::current_exe().ok()
}

/// Whether this launcher can update itself (else: the release page).
pub fn supported() -> bool {
    asset("0").is_some() && executable().is_some()
}

/// Pure: the checksum `sums` (as `sha256sum` writes it) gives for `name`.
fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// Downloads version `v` for this system, checks it, and puts it in place of the running
/// launcher's files. Returns the executable to start.
pub fn install(v: &str, cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<PathBuf> {
    let (Some(name), Some(exe)) = (asset(v), executable()) else {
        bail!("This launcher can't update itself here: download it from the release page.");
    };
    let dir = exe
        .parent()
        .context("the launcher's folder is unknown")?
        .to_path_buf();
    on(Step::Status(format!("Downloading launcher {v}...")));
    let zip = fetch(v, &name, cancel, on)?;
    on(Step::Status(format!("Installing launcher {v}...")));
    let staging = dir.join(STAGING);
    let _ = std::fs::remove_dir_all(&staging);
    let result = crate::core::zip::extract_root(&zip, Some(ZIP_FOLDER), &staging, cancel)
        .with_context(|| {
            format!(
                "Couldn't unpack the update into {}. Is the launcher's folder writable?",
                dir.display()
            )
        })
        .and_then(|_| {
            let file = exe
                .file_name()
                .context("the launcher's file name is unknown")?;
            if !staging.join(file).is_file() {
                bail!("The update has no {}", file.to_string_lossy());
            }
            // The tray runs from these files: it goes first, while it still has their
            // names (after the update the window starts it again).
            crate::core::tray::quit();
            replace(&dir, &staging)?;
            executable_bit(&exe)
        });
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(&zip);
    result?;
    tracing::info!("launcher {v} is in {}", dir.display());
    installed_version(&dir, v);
    Ok(exe)
}

/// Windows: when the installer put the launcher into `dir`, the version Windows' Apps list
/// shows becomes `v`.
fn installed_version(dir: &Path, v: &str) {
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
        let key = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall\EchoVR_Launcher",
            KEY_READ | KEY_WRITE,
        );
        let Ok(key) = key else {
            return;
        };
        let at: String = key.get_value("InstallLocation").unwrap_or_default();
        if Path::new(&at) == dir {
            if let Err(e) = key.set_value("DisplayVersion", &v) {
                tracing::warn!("Apps list version: {e}");
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (dir, v);
}

/// The zip `name` of version `v`, checked against its release's checksums: from the
/// mirror, else from GitHub.
fn fetch(v: &str, name: &str, cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<PathBuf> {
    let github = GITHUB.replace("{v}", v);
    // Debug builds: `ECHOVR_UPDATE_MIRROR=<url>` fetches the update from a test server.
    let mirror = std::env::var("ECHOVR_UPDATE_MIRROR")
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .unwrap_or_else(|| MIRROR.to_string());
    let mut last = None;
    for base in [mirror.as_str(), github.as_str()] {
        let attempt = http::get_text(&format!("{base}/{SUMS}"))
            .and_then(|sums| {
                sum_for(&sums, name).with_context(|| format!("{base}/{SUMS} doesn't list {name}"))
            })
            .and_then(|sha| {
                download::fetch_pinned(
                    &format!("{base}/{name}"),
                    &paths::downloads_dir(),
                    name,
                    &sha,
                    cancel,
                    &mut |p| {
                        if let download::Progress::Percent(x) = p {
                            on(Step::Percent(x));
                        }
                    },
                )
            });
        match attempt {
            Ok(zip) => return Ok(zip),
            Err(e) if http::is_cancelled(&e) => return Err(e),
            Err(e) => {
                tracing::warn!("launcher update from {base}: {e:#}");
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no download")))
        .context("Couldn't download the launcher update")
}

/// Unix: makes `exe` executable (a file the update added has no mode from the zip).
fn executable_bit(exe: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(exe)?.permissions();
        p.set_mode(p.mode() | 0o755);
        std::fs::set_permissions(exe, p)
            .with_context(|| format!("make {} executable", exe.display()))?;
    }
    #[cfg(not(unix))]
    let _ = exe;
    Ok(())
}

/// Moves every file in `staging` into `dir`, each file it replaces renamed `<name>.old`
/// first. On a failure what was moved goes back.
fn replace(dir: &Path, staging: &Path) -> Result<()> {
    let mut files = Vec::new();
    collect(staging, Path::new(""), &mut files)?;
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let undo = |done: &[(PathBuf, Option<PathBuf>)]| {
        for (target, old) in done.iter().rev() {
            let _ = std::fs::remove_file(target);
            if let Some(old) = old {
                let _ = std::fs::rename(old, target);
            }
        }
    };
    for rel in files {
        let (from, target) = (staging.join(&rel), dir.join(&rel));
        let step = (|| -> Result<Option<PathBuf>> {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let old = if target.exists() {
                // The new file takes over the old one's permissions (the zip doesn't carry
                // them: the executable would lose its executable bit).
                if let Ok(meta) = std::fs::metadata(&target) {
                    let _ = std::fs::set_permissions(&from, meta.permissions());
                }
                let old = aside_name(&target);
                std::fs::rename(&target, &old)
                    .with_context(|| format!("move {} aside", target.display()))?;
                Some(old)
            } else {
                None
            };
            if let Err(e) = std::fs::rename(&from, &target) {
                if let Some(old) = &old {
                    let _ = std::fs::rename(old, &target);
                }
                return Err(e).with_context(|| format!("put {} in", target.display()));
            }
            Ok(old)
        })();
        match step {
            Ok(old) => done.push((target, old)),
            Err(e) => {
                undo(&done);
                return Err(e.context(format!(
                    "Couldn't replace the launcher's files in {}",
                    dir.display()
                )));
            }
        }
    }
    Ok(())
}

/// The files under `root/rel`, relative to `root`.
fn collect(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(root.join(rel))? {
        let e = e?;
        let r = rel.join(e.file_name());
        if e.file_type()?.is_dir() {
            collect(root, &r, out)?;
        } else {
            out.push(r);
        }
    }
    Ok(())
}

/// `<file>.old` beside `file`.
fn old_name(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(OLD);
    file.with_file_name(name)
}

/// Where `file` goes aside: `<file>.old`, or when an older one is still there and can't go
/// (a process started from it still runs: a tray from before the last update), the first
/// free `<file>.<n>.old`. Every one ends in `.old`, so the next start removes it.
fn aside_name(file: &Path) -> PathBuf {
    let free = |p: &Path| {
        let _ = std::fs::remove_file(p);
        !p.exists()
    };
    let first = old_name(file);
    if free(&first) {
        return first;
    }
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    (1..=9)
        .map(|n| file.with_file_name(format!("{name}.{n}{OLD}")))
        .find(|p| free(p))
        .unwrap_or(first)
}

/// At a start: removes what an update left beside the executable (the replaced files, an
/// unpacking that didn't finish).
pub fn clean_up() {
    let Some(dir) = executable().and_then(|e| e.parent().map(Path::to_path_buf)) else {
        return;
    };
    let _ = std::fs::remove_dir_all(dir.join(STAGING));
    let mut old = Vec::new();
    if collect(&dir, Path::new(""), &mut old).is_err() {
        return;
    }
    old.retain(|r| r.to_string_lossy().ends_with(OLD));
    let remove = |old: &mut Vec<PathBuf>, last: bool| {
        old.retain(|rel| match std::fs::remove_file(dir.join(rel)) {
            Ok(()) => {
                tracing::info!("removed {} (replaced by an update)", rel.display());
                false
            }
            Err(e) => {
                if last {
                    tracing::info!("{} stays for now: {e}", rel.display());
                }
                true
            }
        });
    };
    remove(&mut old, false);
    if old.is_empty() {
        return;
    }
    // Still in use: most likely by a tray started before the update, running from the old
    // executable. It goes (the window starts a new one), then they can.
    crate::core::tray::quit();
    let start = Instant::now();
    loop {
        let last = start.elapsed() >= Duration::from_secs(2);
        remove(&mut old, last);
        if old.is_empty() || last {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Starts the launcher at `exe` (the updated one), which waits for this process to end.
pub fn restart(exe: &Path) -> Result<()> {
    crate::core::process::command(exe)
        .arg(AFTER_FLAG)
        .arg(std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("couldn't start {}", exe.display()))?;
    Ok(())
}

/// After an update: waits (up to a minute) for the old launcher, process `pid`, to end.
pub fn wait_for(pid: &str) {
    let Ok(pid) = pid.parse::<u32>() else {
        return;
    };
    let pid = sysinfo::Pid::from_u32(pid);
    let start = Instant::now();
    let mut sys = sysinfo::System::new();
    while start.elapsed() < Duration::from_secs(60) {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        if sys.process(pid).is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    tracing::warn!("the old launcher (pid {pid}) is still running; starting anyway");
}

/// Tests against the live mirror: version `v`'s Windows and Linux zips, downloaded into
/// `dir` and checked against the mirror's `SHA256SUMS` as an update checks them.
#[cfg(test)]
pub(super) fn check_on_mirror(v: &str, dir: &Path) -> Result<()> {
    let sums = http::get_text(&format!("{MIRROR}/{SUMS}"))?;
    for os in ["windows", "linux"] {
        let name = format!("Echo_VR_Launcher-{v}-{os}.zip");
        let sha = sum_for(&sums, &name).with_context(|| format!("{SUMS} doesn't list {name}"))?;
        let zip = download::fetch_pinned(
            &format!("{MIRROR}/{name}"),
            dir,
            &name,
            &sha,
            &AtomicBool::new(false),
            &mut |_| {},
        )?;
        if crate::core::zip::top_folder(&zip)?.as_deref() != Some(ZIP_FOLDER) {
            bail!("{name} doesn't have everything in {ZIP_FOLDER}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_by_name() {
        let sums = "347ba925597f0c9cc025e92de771b4d55461fcb866835e4de60f919fed7f916d  Echo_VR_Launcher-0.11.4-linux.tar.gz\n\
                    9D4571A66EA558D30725106BCC2E72AFA606A35DC218BE041971F129EFF20776 *Echo_VR_Launcher-0.11.4-linux.zip\n\
                    nothexnothexnothexnothexnothexnothexnothexnothexnothexnothexnoth  Echo_VR_Launcher-0.11.4-windows.zip\n";
        assert_eq!(
            sum_for(sums, "Echo_VR_Launcher-0.11.4-linux.zip").as_deref(),
            Some("9d4571a66ea558d30725106bcc2e72afa606a35dc218be041971f129eff20776")
        );
        assert!(sum_for(sums, "Echo_VR_Launcher-0.11.4-windows.zip").is_none());
        assert!(sum_for(sums, "Echo_VR_Launcher-0.11.4").is_none());
        assert_eq!(
            asset("0.11.6").is_some(),
            cfg!(any(windows, target_os = "linux"))
        );
    }

    /// Windows: the running executable can be renamed (it can't be overwritten), which is
    /// what an update does with it.
    #[test]
    #[cfg(windows)]
    fn the_running_exe_can_be_moved_aside() {
        let exe = std::env::current_exe().unwrap();
        let old = old_name(&exe);
        std::fs::rename(&exe, &old).unwrap();
        std::fs::copy(&old, &exe).unwrap();
        assert!(exe.is_file());
        // The renamed one is still in use: it goes at the next start (here: when it can).
        let _ = std::fs::remove_file(&old);
    }

    #[test]
    fn an_old_one_that_cant_go_doesnt_block_the_update() {
        let dir = tempfile::tempdir().unwrap();
        let (app, staging) = (dir.path().join("app"), dir.path().join("stage"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("EchoVR_Launcher.exe"), "0.11.9").unwrap();
        std::fs::write(staging.join("EchoVR_Launcher.exe"), "beta.3").unwrap();
        // The last update's .old can't be removed (as one a running tray holds): here a
        // folder in its place.
        std::fs::create_dir_all(app.join("EchoVR_Launcher.exe.old/held")).unwrap();
        replace(&app, &staging).unwrap();
        assert_eq!(
            std::fs::read_to_string(app.join("EchoVR_Launcher.exe")).unwrap(),
            "beta.3"
        );
        assert_eq!(
            std::fs::read_to_string(app.join("EchoVR_Launcher.exe.1.old")).unwrap(),
            "0.11.9"
        );
        // And with that one held too, the next number.
        std::fs::write(staging.join("EchoVR_Launcher.exe"), "beta.4").unwrap();
        std::fs::remove_file(app.join("EchoVR_Launcher.exe.1.old")).unwrap();
        std::fs::create_dir_all(app.join("EchoVR_Launcher.exe.1.old/held")).unwrap();
        replace(&app, &staging).unwrap();
        assert_eq!(
            std::fs::read_to_string(app.join("EchoVR_Launcher.exe.2.old")).unwrap(),
            "beta.3"
        );
    }

    #[test]
    fn files_are_replaced_and_the_old_ones_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let (app, staging) = (dir.path().join("app"), dir.path().join("app").join(STAGING));
        std::fs::create_dir_all(staging.join("sub")).unwrap();
        std::fs::write(app.join("EchoVR_Launcher"), "old").unwrap();
        std::fs::write(app.join("LICENSE"), "old licence").unwrap();
        std::fs::write(staging.join("EchoVR_Launcher"), "new").unwrap();
        std::fs::write(staging.join("LICENSE"), "new licence").unwrap();
        std::fs::write(staging.join("sub/extra.dll"), "added").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |m| std::fs::Permissions::from_mode(m);
            std::fs::set_permissions(app.join("EchoVR_Launcher"), mode(0o755)).unwrap();
            std::fs::set_permissions(staging.join("EchoVR_Launcher"), mode(0o644)).unwrap();
        }
        replace(&app, &staging).unwrap();
        assert_eq!(
            std::fs::read_to_string(app.join("EchoVR_Launcher")).unwrap(),
            "new"
        );
        // Still executable: the old one's permissions, not the zip's lack of them.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(app.join("EchoVR_Launcher"))
                .unwrap()
                .permissions();
            assert_eq!(m.mode() & 0o777, 0o755);
        }
        assert_eq!(
            std::fs::read_to_string(app.join("EchoVR_Launcher.old")).unwrap(),
            "old"
        );
        assert_eq!(
            std::fs::read_to_string(app.join("sub/extra.dll")).unwrap(),
            "added"
        );
        assert!(!app.join("sub/extra.dll.old").exists());

        // A file that can't be put in (a read-only folder): what was moved goes back.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let (app, staging) = (dir.path().join("app"), dir.path().join("stage"));
            std::fs::create_dir_all(staging.join("locked")).unwrap();
            std::fs::create_dir_all(app.join("locked")).unwrap();
            std::fs::write(app.join("a"), "old a").unwrap();
            std::fs::write(staging.join("a"), "new a").unwrap();
            std::fs::write(app.join("locked/b"), "old b").unwrap();
            std::fs::write(staging.join("locked/b"), "new b").unwrap();
            std::fs::set_permissions(app.join("locked"), std::fs::Permissions::from_mode(0o555))
                .unwrap();
            let r = replace(&app, &staging);
            std::fs::set_permissions(app.join("locked"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            assert!(r.is_err());
            assert_eq!(std::fs::read_to_string(app.join("a")).unwrap(), "old a");
            assert!(!app.join("a.old").exists());
            assert_eq!(
                std::fs::read_to_string(app.join("locked/b")).unwrap(),
                "old b"
            );
        }
    }
}
