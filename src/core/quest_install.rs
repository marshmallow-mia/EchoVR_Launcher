//! Installs the Echo VR APK and its `_data.zip` onto a Quest over adb (port of
//! `InstallerQuest`).
//!
//! Every step that matters is checked: the Java version ignored the result of
//! `adb install`, so a failed install still ended in "Installation is complete!".

use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use super::adb::{self, devices::Status, PACKAGE};
use super::error::{quest_not_found, quest_unauthorized, UiError};

/// Game data lives in the app-owned external media dir. Unlike `/sdcard/readyatdawn`,
/// this needs no storage permission, so it works on secondary Quest accounts. It must be
/// staged after the APK is installed, because Android wipes `/sdcard/Android/media/<pkg>`
/// when the app is uninstalled.
pub const DATA_DIR: &str = "/sdcard/Android/media/com.readyatdawn.r15/files";

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

fn reconnect() -> bool {
    adb::exec(&["kill-server"]);
    adb::exec(&["start-server"]);
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

/// Did `adb install` really install? Modern adb exits non-zero on failure, older ones
/// only print `Failure [...]`.
pub fn install_succeeded(code: Option<i32>, output: &str) -> bool {
    code == Some(0) && output.contains("Success") && !output.contains("Failure")
}

pub fn install(
    dir: &Path,
    apk: &str,
    data_zip: &str,
    status: &mut dyn FnMut(String),
) -> Result<()> {
    adb::bundle::binary()?;
    if let Some(e) = status_error(adb::probe().status) {
        return Err(e.into());
    }

    let apk_path = dir.join(apk);
    let data_path = dir.join(data_zip);
    if !apk_path.exists() || !data_path.exists() {
        return Err(UiError::new(
            "File not found",
            "The downloaded game files are gone. Please install again.",
        )
        .into());
    }

    // A fresh server, then re-pin (the user's device choice is kept across this).
    adb::exec(&["kill-server"]);
    if let Some(e) = status_error(adb::probe().status) {
        return Err(e.into());
    }

    status("Removing the old version...".into());
    let un = adb::exec(&["uninstall", PACKAGE]);
    if !un.success() {
        // Not installed is fine; anything else surfaces again at `install`.
        tracing::info!("uninstall: {}", last_line(&un.output));
    }
    adb::shell("rm -rf /sdcard/readyatdawn"); // legacy data location

    status("Installing the APK...".into());
    let apk_arg = apk_path.to_string_lossy();
    let inst = adb::exec(&["install", "-g", &apk_arg]);
    if !install_succeeded(inst.code, &inst.output) {
        return Err(UiError::new(
            "Install Failed",
            format!("Your Quest refused the APK:\n{}", last_line(&inst.output)),
        )
        .into());
    }

    shell_ok(&format!("mkdir -p {DATA_DIR}/_local"));
    shell_ok(&format!("chmod -R 777 {DATA_DIR}"));

    status("Pushing data files...".into());
    let remote_zip = format!("/data/local/tmp/{data_zip}");
    if !adb::push_file(&data_path, &remote_zip) {
        return Err(UiError::new(
            "Transfer Failed",
            "Failed to push data files to the device.",
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

    status("Unpacking data files...".into());
    let steps: [(&str, String); 4] = [
        ("mv", format!("mv {remote_zip} {DATA_DIR}/")),
        // -o: never stop at an overwrite prompt when leftovers exist.
        ("unzip", format!("cd {DATA_DIR}/ && unzip -o {data_zip}")),
        ("rm", format!("cd {DATA_DIR}/ && rm {data_zip}")),
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

    status("Granting permissions...".into());
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_result_detection() {
        assert!(install_succeeded(
            Some(0),
            "Performing Streamed Install\nSuccess\n"
        ));
        assert!(!install_succeeded(
            Some(1),
            "adb: failed to install x.apk: Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE]"
        ));
        assert!(!install_succeeded(
            Some(0),
            "Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE]"
        ));
        assert!(!install_succeeded(None, ""));
    }

    #[test]
    fn last_line_skips_blank_lines() {
        assert_eq!(last_line("a\nb\n\n  \n"), "b");
        assert_eq!(last_line(""), "(no output)");
    }
}
