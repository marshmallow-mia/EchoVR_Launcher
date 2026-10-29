//! The steps that put nEVR's Echo VR onto a Quest over adb: what is installed, the APK
//! (in place, or after an uninstall the player agreed to), the game data, permissions.
//!
//! Every step that matters is checked: the Java installer ignored the result of
//! `adb install`, so a failed install still ended in "Installation is complete!".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

use super::adb::{self, devices::Status, PACKAGE};
use super::error::{quest_not_found, quest_unauthorized, UiError};
use super::launcher::quest_net;
use super::nevr_release::Member;

/// Game data lives in the app-owned external media dir. Unlike `/sdcard/readyatdawn`,
/// this needs no storage permission, so it works on secondary Quest accounts. It must be
/// staged after the APK is installed, because Android wipes `/sdcard/Android/media/<pkg>`
/// when the app is uninstalled.
pub const DATA_DIR: &str = "/sdcard/Android/media/com.readyatdawn.r15/files";
/// Where older installers put the game data; nEVR's build reads it there too.
pub const LEGACY_DATA_DIR: &str = "/sdcard/readyatdawn";

/// Message for "several usable devices", listing what adb saw.
pub fn multi_device_message() -> String {
    let mut s = String::from(
        "More than one device is plugged in, so the installer can't tell\nwhich one is your Quest:\n\n",
    );
    for d in adb::last_selection().pickable() {
        s.push_str(&format!("  \u{2022} {}\n", d.label()));
    }
    s.push_str("\nUnplug the others (phone, second headset, emulator) and try again.");
    s
}

/// Maps a non-ready probe to the dialog the user should see.
pub fn status_error(status: Status) -> Option<UiError> {
    match status {
        Status::Ready => None,
        Status::Unauthorized => Some(quest_unauthorized()),
        Status::Ambiguous => Some(UiError::new(
            "Several devices connected",
            multi_device_message(),
        )),
        Status::None => Some(quest_not_found()),
    }
}

/// Runs `action`; if it fails because the device dropped, restarts the adb server and
/// retries once. A failure with the device still connected is not retried.
pub fn with_reconnect(
    desc: &str,
    status: &mut dyn FnMut(String),
    mut action: impl FnMut() -> bool,
) -> bool {
    if action() {
        return true;
    }
    if adb::probe().status == Status::Ready {
        tracing::warn!("{desc} failed but the device is connected -- not retrying");
        return false;
    }
    tracing::warn!("device disconnected during {desc}, retrying...");
    status("Device disconnected - retrying...".into());
    if !reconnect() {
        tracing::error!("device still disconnected after reconnecting during {desc}");
        return false;
    }
    status("Device reconnected, continuing...".into());
    action()
}

/// Restarts adb's server, and connects again to a headset it reached over the network.
fn reconnect() -> bool {
    let network = adb::last_selection()
        .serial
        .filter(|s| quest_net::is_network_serial(s));
    adb::exec(&["kill-server"]);
    adb::exec(&["start-server"]);
    if let Some(s) = network {
        adb::exec(&["connect", &s]);
    }
    std::thread::sleep(Duration::from_secs(2));
    adb::probe().status == Status::Ready
}

fn shell_ok(script: &str) -> bool {
    adb::shell(script).success()
}

/// Last non-empty line of adb output, for error messages.
fn last_line(output: &str) -> &str {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("(no output)")
}

/// Echo VR as the headset has it installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The installed APK's path on the headset.
    pub path: String,
    pub version_code: Option<u64>,
}

/// What is installed, read from `pm path` and `dumpsys package`; `None` when nothing is.
pub fn installed() -> Option<Installed> {
    let out = adb::exec(&["shell", "pm", "path", PACKAGE]);
    let path = parse_pm_path(&out.output)?;
    let dump = adb::shell(&format!("dumpsys package {PACKAGE} | grep versionCode="));
    Some(Installed {
        path,
        version_code: parse_version_code(&dump.output),
    })
}

/// Pure: the APK's path from `pm path <pkg>` (keyed off a `package:` line rather than the
/// exit code: builds disagree on what "not installed" exits with).
pub fn parse_pm_path(output: &str) -> Option<String> {
    output
        .lines()
        .filter_map(|l| l.trim().strip_prefix("package:"))
        .map(str::trim)
        .find(|p| !p.is_empty())
        .map(str::to_string)
}

/// Pure: the first `versionCode=<n>` in `dumpsys package` output.
pub fn parse_version_code(output: &str) -> Option<u64> {
    let rest = &output[output.find("versionCode=")? + "versionCode=".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// SHA-256 of the installed APK at `path`, hashed on the headset; falls back to pulling it.
pub fn installed_sha(path: &str) -> Option<String> {
    if let Some(h) = first_hash(&adb::exec(&["shell", "sha256sum", path]).output) {
        return Some(h);
    }
    tracing::info!("on-device sha256sum unusable, pulling the APK to hash it locally");
    let dir = super::paths::temp_dir().join(format!("quest-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).ok()?;
    let local = dir.join("installed.apk");
    let sha = adb::pull(path, &local)
        .then(|| super::download::sha256_file(&local).ok())
        .flatten();
    let _ = std::fs::remove_dir_all(&dir);
    sha
}

/// Pure: the first token of `sha256sum` output, when it is a hash.
pub fn first_hash(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .next()
        .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

/// How `adb install` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApkInstall {
    Done,
    /// The installed Echo VR is signed by someone else (another build): Android only
    /// replaces it after an uninstall.
    OtherSigner,
    /// The installed Echo VR is newer than this APK: Android doesn't go back in place.
    Newer,
    /// Refused for another reason (the last line says which).
    Failed(String),
}

/// Pure: what `adb install` answered. Modern adb exits non-zero on failure, older ones
/// only print `Failure [...]`.
pub fn install_result(code: Option<i32>, output: &str) -> ApkInstall {
    if code == Some(0) && output.contains("Success") && !output.contains("Failure") {
        ApkInstall::Done
    } else if output.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE")
        || output.contains("INSTALL_FAILED_SHARED_USER_INCOMPATIBLE")
    {
        ApkInstall::OtherSigner
    } else if output.contains("INSTALL_FAILED_VERSION_DOWNGRADE") {
        ApkInstall::Newer
    } else {
        ApkInstall::Failed(last_line(output).to_string())
    }
}

/// Installs the APK at `apk`, in place of what is installed (`-r`: its data stays).
pub fn install_apk(apk: &Path) -> ApkInstall {
    let r = adb::exec(&["install", "-r", "-g", &apk.to_string_lossy()]);
    install_result(r.code, &r.output)
}

/// Uninstalls Echo VR (the player agreed: Android removes its app data with it).
pub fn uninstall() -> Result<()> {
    let r = adb::exec(&["uninstall", PACKAGE]);
    if !r.success() && parse_pm_path(&adb::exec(&["shell", "pm", "path", PACKAGE]).output).is_some()
    {
        return Err(UiError::new(
            "Uninstall failed",
            format!(
                "Your Quest didn't uninstall Echo VR:\n{}",
                last_line(&r.output)
            ),
        )
        .into());
    }
    Ok(())
}

/// Whether the game data is on the headset: every file of `contents` with its size, in
/// [`DATA_DIR`] or [`LEGACY_DATA_DIR`]. (Its hash was checked when it was downloaded; 930 MB
/// aren't hashed again on every check.)
pub fn data_present(contents: &[Member]) -> bool {
    [DATA_DIR, LEGACY_DATA_DIR].iter().any(|root| {
        // The paths are checked against nEVR's manifest: no shell metacharacters.
        let paths: Vec<&str> = contents.iter().map(|m| m.path.as_str()).collect();
        let out = adb::shell(&format!(
            "cd {root} 2>/dev/null && stat -c '%s %n' {} 2>/dev/null",
            paths.join(" ")
        ));
        let sizes = parse_stat(&out.output);
        contents
            .iter()
            .all(|m| sizes.get(m.path.as_str()) == Some(&m.size))
    })
}

/// Pure: `stat -c '%s %n'` output as path -> size.
pub fn parse_stat(output: &str) -> HashMap<&str, u64> {
    output
        .lines()
        .filter_map(|l| {
            let (size, path) = l.trim().split_once(' ')?;
            Some((path.trim(), size.parse().ok()?))
        })
        .collect()
}

/// Copies the game data zip at `zip` onto the headset and unpacks it into [`DATA_DIR`].
/// The player's settings in the game's media folder. Android deletes that folder with the
/// app, so replacing another build keeps a copy and puts it back.
const SETTINGS: &str = "RAD";

/// Copies the player's settings off the headset into `dir`: the copy, or `None` when there
/// are none (or they couldn't be copied).
pub fn save_settings(dir: &Path) -> Option<PathBuf> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).ok()?;
    let remote = format!("{DATA_DIR}/{SETTINGS}");
    if !shell_ok(&format!("test -d {remote}")) {
        return None;
    }
    let saved = dir.join(SETTINGS);
    (adb::pull(&remote, dir) && saved.is_dir()).then_some(saved)
}

/// Puts settings [`save_settings`] kept back on the headset.
pub fn restore_settings(saved: &Path) -> bool {
    shell_ok(&format!("mkdir -p {DATA_DIR}"));
    let ok = adb::exec(&["push", &saved.to_string_lossy(), &format!("{DATA_DIR}/")]).success();
    shell_ok(&format!("chmod -R 777 {DATA_DIR}/{SETTINGS}"));
    ok
}

pub fn push_data(zip: &Path, status: &mut dyn FnMut(String)) -> Result<()> {
    let name = zip
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    shell_ok(&format!("mkdir -p {DATA_DIR}/_local"));
    shell_ok(&format!("chmod -R 777 {DATA_DIR}"));

    status("Copying the game data to your Quest...".into());
    let remote_zip = format!("/data/local/tmp/{name}");
    if !adb::push_file(zip, &remote_zip) {
        return Err(UiError::new(
            "Transfer Failed",
            "Copying the game data to your Quest failed.",
        )
        .into());
    }

    status("Verifying transfer...".into());
    if adb::probe().status != Status::Ready {
        tracing::warn!("device disconnected after the data push");
        status("Device disconnected - retrying...".into());
        if !reconnect() {
            return Err(UiError::new(
                "Device Disconnected",
                "Device disconnected during data transfer and could not be reconnected.",
            )
            .into());
        }
        status("Device reconnected, continuing...".into());
    }

    status("Unpacking the game data...".into());
    let steps: [(&str, String); 4] = [
        ("mv", format!("mv {remote_zip} {DATA_DIR}/")),
        // -o: never stop at an overwrite prompt when leftovers exist.
        ("unzip", format!("cd {DATA_DIR}/ && unzip -o {name}")),
        ("rm", format!("cd {DATA_DIR}/ && rm {name}")),
        ("chmod", format!("chmod -R 777 {DATA_DIR}")),
    ];
    for (desc, script) in &steps {
        if !with_reconnect(desc, status, || shell_ok(script)) {
            return Err(UiError::new(
                "Installation did not finish",
                format!("Setting up the game data failed at step '{desc}'.\nPlease try again."),
            )
            .into());
        }
    }
    Ok(())
}

/// The permissions Echo VR asks for, granted so it never stops to ask in the headset.
pub fn grant_permissions() {
    shell_ok(&format!("mkdir -p {DATA_DIR}/_local"));
    for script in [
        format!("appops set {PACKAGE} MANAGE_EXTERNAL_STORAGE allow"),
        format!("pm grant {PACKAGE} android.permission.READ_EXTERNAL_STORAGE"),
        format!("pm grant {PACKAGE} android.permission.WRITE_EXTERNAL_STORAGE"),
        format!("pm grant {PACKAGE} android.permission.RECORD_AUDIO"),
    ] {
        let r = adb::shell(&script);
        if !r.success() {
            // Some builds don't declare every permission; the game still runs.
            tracing::warn!("'{script}' failed: {}", last_line(&r.output));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_result_detection() {
        assert_eq!(
            install_result(Some(0), "Performing Streamed Install\nSuccess\n"),
            ApkInstall::Done
        );
        assert_eq!(
            install_result(
                Some(1),
                "adb: failed to install x.apk: Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package com.readyatdawn.r15 signatures do not match newer version; ignoring!]"
            ),
            ApkInstall::OtherSigner
        );
        assert_eq!(
            install_result(
                Some(1),
                "adb: failed to install x.apk: Failure [INSTALL_FAILED_VERSION_DOWNGRADE: Downgrade detected]"
            ),
            ApkInstall::Newer
        );
        assert_eq!(
            install_result(Some(0), "Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE]"),
            ApkInstall::Failed("Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE]".into())
        );
        assert!(matches!(install_result(None, ""), ApkInstall::Failed(_)));
    }

    #[test]
    fn reads_what_is_installed() {
        assert_eq!(
            parse_pm_path("package:/data/app/~~x==/com.readyatdawn.r15-y==/base.apk\n").as_deref(),
            Some("/data/app/~~x==/com.readyatdawn.r15-y==/base.apk")
        );
        assert_eq!(parse_pm_path(""), None);
        assert_eq!(
            parse_version_code("    versionCode=60200999 minSdk=29 targetSdk=29\n"),
            Some(60200999)
        );
        assert_eq!(parse_version_code("Unable to find package"), None);
        let sha = "f75c3d69d017ab34d10668bb7e12eea33bea64bc455331a26605bcf93980e6eb";
        assert_eq!(
            first_hash(&format!("{sha}  /data/app/base.apk")).as_deref(),
            Some(sha)
        );
        assert_eq!(first_hash("sha256sum: not found"), None);
    }

    #[test]
    fn reads_the_data_sizes() {
        let out = "1461 _data/1/rad15/android/manifests/a\n929620177 _data/1/rad15/android/packages/a_0\nstat: x: No such file\n";
        let s = parse_stat(out);
        assert_eq!(s.get("_data/1/rad15/android/manifests/a"), Some(&1461));
        assert_eq!(
            s.get("_data/1/rad15/android/packages/a_0"),
            Some(&929620177)
        );
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn last_line_skips_blank_lines() {
        assert_eq!(last_line("a\nb\n\n  \n"), "b");
        assert_eq!(last_line(""), "(no output)");
    }
}
