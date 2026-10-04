//! Echo VR on the Quest: which version is installed, and starting/stopping it over adb.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use anyhow::{anyhow, bail, Result};

use super::versions::Step;
use crate::core::adb::{self, PACKAGE};
use crate::core::download::{self, Progress};
use crate::core::error::UiError;
use crate::core::manifest::Manifest;
use crate::core::oauth::{self, OAuthError};
use crate::core::quest_update::{self, Marker, VersionCheck};
use crate::core::{paths, quest_install};

/// Used when the update manifest (which names the current APK) can't be fetched and the
/// versions catalogue names none.
pub const FALLBACK_APK: &str = "echo_quest_27-08-2026.001.apk";
pub const DATA_ZIP: &str = "_data.zip";

/// Where the Quest APK comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApkSource {
    /// The stock APK (owners).
    Stock,
    /// A personal patched APK through Discord (new players).
    Discord,
    /// A patched APK link the user already has.
    Url(String),
}

#[derive(Debug)]
pub enum JobError {
    OAuth(OAuthError),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for JobError {
    fn from(e: anyhow::Error) -> Self {
        JobError::Other(e)
    }
}

/// How an install ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// With the latest update.
    UpToDate,
    /// The update manifest couldn't be fetched: installed, but not checked for updates.
    NotChecked,
}

/// What an update found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    Updated,
    /// The headset's APK is not the one the update is built for: reinstall. The text
    /// says why.
    NeedsReinstall(String),
}

/// A personal patched APK is saved apart from the stock one, so a resumed download can
/// never mix the two.
pub fn patched_name(base: &str) -> String {
    format!("{}.patched.apk", base.trim_end_matches(".apk"))
}

fn staging() -> PathBuf {
    paths::downloads_dir()
}

fn fetch(
    url: &str,
    name: &str,
    mirror: bool,
    fresh: bool,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<PathBuf> {
    let job = download::Job {
        url: url.into(),
        dir: staging(),
        filename: name.into(),
        use_mirror: mirror,
        fresh,
        extract: false,
    };
    download::run(&job, cancel, &mut |p| match p {
        Progress::Percent(v) => on(Step::Percent(v)),
        Progress::Status(s) => on(Step::Status(s)),
        _ => {}
    })
}

/// Downloads the APK (stock or patched) and the game data, installs both over adb,
/// records the install on the headset and applies the latest update. `fallback_apk` is
/// the APK to take when the update manifest (which names the current one) can't be
/// fetched: the versions catalogue's.
pub fn install(
    source: &ApkSource,
    fallback_apk: Option<&str>,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<Installed, JobError> {
    ready()?;
    on(Step::Status("Checking for the latest version...".into()));
    let manifest = Manifest::fetch(quest_update::QUEST_MANIFEST_URL)
        .map_err(|e| tracing::warn!("quest manifest: {e:#}"))
        .ok();
    let base = manifest
        .as_ref()
        .and_then(|m| m.base_apk_name.clone())
        .or_else(|| fallback_apk.map(str::to_string))
        .unwrap_or_else(|| FALLBACK_APK.into());

    let (apk, patched) = match source {
        ApkSource::Stock => {
            on(Step::Status("Downloading Echo VR...".into()));
            let apk = fetch(&base, &base, true, false, cancel, on)?;
            if let Some(sha) = manifest.as_ref().and_then(|m| m.base_apk_sha.as_deref()) {
                on(Step::Status("Verifying the download...".into()));
                if !download::sha256_matches(&apk, sha) {
                    let _ = std::fs::remove_file(&apk);
                    return Err(anyhow!(
                        "The downloaded APK is corrupt (checksum mismatch). Please try again."
                    )
                    .into());
                }
            }
            (apk, false)
        }
        ApkSource::Discord | ApkSource::Url(_) => {
            let url = match source {
                ApkSource::Url(u) => oauth::validate_apk_url(u.trim()).ok_or_else(|| {
                    anyhow!("That link is not a patched APK link. Please check it and try again.")
                })?,
                _ => oauth::run(oauth::FileType::Apk, cancel, on).map_err(JobError::OAuth)?,
            };
            on(Step::Status("Downloading your patched APK...".into()));
            (
                fetch(&url, &patched_name(&base), false, true, cancel, on)?,
                true,
            )
        }
    };
    on(Step::Status("Downloading the game data...".into()));
    fetch(DATA_ZIP, DATA_ZIP, true, false, cancel, on)?;

    let apk_file = apk
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    quest_install::install(&staging(), &apk_file, DATA_ZIP, &mut |s| {
        on(Step::Status(s))
    })?;
    // Record which base version this is before the update runs, so a failed update
    // still leaves a correct marker.
    quest_update::write_marker_after_install(
        Some(base),
        manifest.as_ref().and_then(|m| m.base_apk_sha.clone()),
        download::sha256_file(&apk).ok(),
        patched,
    );
    let _ = std::fs::remove_file(staging().join(DATA_ZIP));
    let outcome = match manifest {
        Some(m) => {
            on(Step::Status("Applying the latest update...".into()));
            quest_update::apply(&m, cancel, &mut |s| on(Step::Status(s))).map_err(|e| {
                e.context("Echo VR is installed, but applying the latest update failed. Use Update to try again.")
            })?;
            Installed::UpToDate
        }
        None => Installed::NotChecked,
    };
    adb::exec(&["kill-server"]);
    Ok(outcome)
}

/// Checks that the headset has the APK the update is built for, then applies it.
pub fn update(cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<UpdateOutcome> {
    let st = quest_update::check_version(quest_update::QUEST_MANIFEST_URL, &mut |s| {
        on(Step::Status(s))
    });
    match st.result {
        VersionCheck::Ok => {
            let m = st
                .manifest
                .ok_or_else(|| anyhow!("The update list could not be loaded."))?;
            quest_update::apply(&m, cancel, &mut |s| on(Step::Status(s)))?;
            Ok(UpdateOutcome::Updated)
        }
        VersionCheck::NotInstalled | VersionCheck::Mismatch => {
            Ok(UpdateOutcome::NeedsReinstall(st.detail))
        }
        VersionCheck::NoDevice => {
            Err(UiError::new(crate::core::error::QUEST_NOT_FOUND_TITLE, st.detail).into())
        }
        VersionCheck::ManifestError => Err(UiError::new("Update check failed", st.detail).into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestInfo {
    pub device: Option<String>,
    pub installed: bool,
    /// From the installer's on-device marker, when present.
    pub marker: Option<Marker>,
    /// Its Wi-Fi address, when it was read over USB.
    pub wifi_ip: Option<std::net::Ipv4Addr>,
    /// ADB reaches it over the network rather than USB.
    pub over_network: bool,
}

impl QuestInfo {
    pub fn version_label(&self) -> String {
        match (&self.installed, &self.marker) {
            (false, _) => "Echo VR is not installed on this Quest".into(),
            (true, Some(m)) => {
                let base = m.base_apk.clone().unwrap_or_else(|| "unknown build".into());
                if m.patched {
                    format!("{base} (patched)")
                } else {
                    base
                }
            }
            (true, None) => "Installed (version unknown)".into(),
        }
    }
}

fn ready() -> Result<()> {
    adb::bundle::binary()?;
    match quest_install::status_error(adb::probe().status) {
        None => Ok(()),
        Some(e) => Err(e.into()),
    }
}

pub fn info() -> Result<QuestInfo> {
    ready()?;
    let target = adb::target_device();
    let device = target.as_ref().map(|d| d.label());
    let over_network = target
        .as_ref()
        .is_some_and(|d| super::quest_net::is_network_serial(&d.serial));
    let installed = quest_update::installed_apk_path().is_some();
    let marker = if installed {
        quest_update::read_marker()
    } else {
        None
    };
    Ok(QuestInfo {
        device,
        installed,
        marker,
        wifi_ip: super::quest_net::ip_over_usb(),
        over_network,
    })
}

/// Pure: the launchable component from `cmd package resolve-activity --brief <pkg>`,
/// whose last line is `pkg/.Activity` (or `No activity found`).
pub fn parse_activity(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with(PACKAGE) && l.contains('/'))
        .map(str::to_string)
}

pub fn launch() -> Result<()> {
    ready()?;
    let out = adb::shell(&format!(
        "cmd package resolve-activity --brief -c android.intent.category.LAUNCHER {PACKAGE}"
    ));
    let Some(component) = parse_activity(&out.output) else {
        return Err(UiError::new(
            "Echo VR not installed",
            "Echo VR is not installed on your Quest.",
        )
        .into());
    };
    let r = adb::exec(&["shell", "am", "start", "-n", &component]);
    if !r.success() || r.output.contains("Error") {
        bail!("The Quest refused to start Echo VR:\n{}", r.output.trim());
    }
    Ok(())
}

/// Closes Echo VR on the headset.
pub fn stop() -> Result<()> {
    ready()?;
    adb::exec(&["shell", "am", "force-stop", PACKAGE]);
    Ok(())
}

/// Where Echo VR keeps its logs on the headset, and the folder each one is saved as.
fn log_dirs() -> [(String, &'static str); 2] {
    [
        ("/sdcard/r14logs".into(), "r14logs"),
        (
            format!("/sdcard/Android/data/{PACKAGE}/files/_local/r14logs"),
            "_local-r14logs",
        ),
    ]
}

/// Pure: the folder one save of the headset's logs goes into, named by when it was made.
pub fn logs_folder(at: time::OffsetDateTime) -> PathBuf {
    let name = format!(
        "{}-{:02}-{:02}_{:02}-{:02}-{:02}",
        at.year(),
        at.month() as u8,
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    );
    paths::log_dir().join("quest").join(name)
}

/// Saves Echo VR's logs from the headset into a new folder beside the launcher's logs,
/// and returns it.
pub fn pull_logs() -> Result<PathBuf> {
    ready()?;
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let dest = logs_folder(now);
    std::fs::create_dir_all(&dest)?;
    let saved = log_dirs()
        .into_iter()
        .filter(|(remote, name)| adb::pull(remote, &dest.join(name)))
        .count();
    if saved == 0 {
        let _ = std::fs::remove_dir_all(&dest);
        bail!("There are no Echo VR logs on your Quest yet: play it once, then try again.");
    }
    tracing::info!("saved the Quest's logs to {}", dest.display());
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quest_logs_go_into_a_dated_folder() {
        let at = time::OffsetDateTime::from_unix_timestamp(1790845503).unwrap(); // 2026-10-01 09:05:03 UTC
        let dir = logs_folder(at);
        assert!(dir.ends_with("quest/2026-10-01_09-05-03"));
        assert!(dir.starts_with(paths::log_dir()));
    }

    #[test]
    fn patched_apk_is_kept_apart() {
        assert_eq!(
            patched_name("echo_quest_27-08-2026.001.apk"),
            "echo_quest_27-08-2026.001.patched.apk"
        );
        assert_ne!(patched_name(FALLBACK_APK), FALLBACK_APK);
    }

    #[test]
    fn activity_parsing() {
        let out = "priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=true\ncom.readyatdawn.r15/com.unity3d.player.UnityPlayerActivity\n";
        assert_eq!(
            parse_activity(out).as_deref(),
            Some("com.readyatdawn.r15/com.unity3d.player.UnityPlayerActivity")
        );
        assert_eq!(parse_activity("No activity found"), None);
    }

    #[test]
    fn version_label() {
        let mut i = QuestInfo {
            device: None,
            installed: false,
            marker: None,
            wifi_ip: None,
            over_network: false,
        };
        assert!(i.version_label().contains("not installed"));
        i.installed = true;
        assert_eq!(i.version_label(), "Installed (version unknown)");
        i.marker = Some(Marker {
            base_apk: Some("echo_quest_27-08-2026.001.apk".into()),
            patched: true,
            ..Default::default()
        });
        assert_eq!(i.version_label(), "echo_quest_27-08-2026.001.apk (patched)");
    }
}
