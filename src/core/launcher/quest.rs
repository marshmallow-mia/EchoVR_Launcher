//! Echo VR on the Quest: nEVR's build, from nEVR's signed releases, installed and
//! updated over adb; which version is installed, and starting/stopping it.
//!
//! Which release goes onto the headset is ours to say (the Quest's `# nEVR quest:` line,
//! [`pc_update::quest_nevr_release`]); what its files are, nEVR's signed manifest says
//! ([`nevr_release::quest_release`]). The APK is the same for every player.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Result};

use super::versions::Step;
use crate::core::adb::{self, PACKAGE};
use crate::core::download::{self, Progress};
use crate::core::error::UiError;
use crate::core::nevr_release::{self, QuestRelease};
use crate::core::quest_install::{self, ApkInstall};
use crate::core::{paths, pc_update};

pub const DATA_ZIP: &str = "_data.zip";
/// How an install or update ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// nEVR `version` is on the headset now.
    Installed(String),
    /// It already was.
    UpToDate(String),
    /// The followed launcher channel offers no Echo VR for the Quest.
    NotOffered,
}

fn staging() -> PathBuf {
    paths::downloads_dir()
}

/// The release the followed launcher channel offers for the Quest, checked against nEVR's
/// signed manifest; `None` when it offers none.
pub fn offered(cancel: &AtomicBool) -> Result<Option<QuestRelease>> {
    let Some(which) = pc_update::quest_nevr_release()? else {
        return Ok(None);
    };
    nevr_release::quest_release(&which, cancel)
        .map(Some)
        .map_err(|e| {
            tracing::warn!(
                "Quest: nEVR's release refused at step {}: {}",
                e.step,
                e.detail
            );
            anyhow::anyhow!(e)
        })
}

/// Pure: whether what is installed is release `r`'s APK (its versionCode, then its bytes:
/// `sha` is asked for only when the code matches).
fn is_release(
    installed: &quest_install::Installed,
    r: &QuestRelease,
    sha: impl FnOnce(&str) -> Option<String>,
) -> bool {
    installed.version_code == Some(r.version_code)
        && sha(&installed.path).as_deref() == Some(r.apk.sha256.as_str())
}

/// Puts the offered nEVR release onto the headset, as nEVR's install method says (ADR 0023,
/// "Quest plan"): a clean install of its APK (what is installed is uninstalled first, the
/// settings kept), then the game data when it isn't there, then the checks.
pub fn install(cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<Outcome> {
    ready()?;
    on(Step::Status("Checking nEVR's release...".into()));
    let Some(r) = offered(cancel)? else {
        return Ok(Outcome::NotOffered);
    };
    let v = r.version.clone();
    let have = quest_install::installed();
    let current = have
        .as_ref()
        .is_some_and(|i| is_release(i, &r, quest_install::installed_sha));

    if !current {
        let apk = staging().join(format!("nevr-runtime-v{v}-quest.apk"));
        if !download::sha256_matches(&apk, &r.apk.sha256) {
            on(Step::Status(format!("Downloading Echo VR (nEVR {v})...")));
            std::fs::create_dir_all(staging())?;
            r.apk
                .download(&apk, cancel, &mut |f| on(Step::Percent(f)))
                .map_err(anyhow::Error::from)?;
        }
        // A clean install, never one over another build: what is installed goes first (its
        // sign-in with it), and the settings in the game's media folder, which Android takes
        // with the app, come back after.
        let kept = if have.is_some() {
            on(Step::Status("Keeping your Echo VR settings...".into()));
            let kept = quest_install::save_settings(&staging().join("quest-settings"));
            on(Step::Status(format!("Updating to nEVR {v}...")));
            quest_install::uninstall()?;
            kept
        } else {
            None
        };
        on(Step::Status("Installing Echo VR on your Quest...".into()));
        match quest_install::install_apk(&apk) {
            ApkInstall::Done => {}
            ApkInstall::Failed(why) => bail_refused(&why)?,
            other => bail_refused(&format!("{other:?}"))?,
        }
        if let Some(dir) = kept {
            on(Step::Status("Putting your Echo VR settings back...".into()));
            if quest_install::restore_settings(&dir) {
                let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
            } else {
                tracing::warn!(
                    "Quest: settings not put back; a copy is in {}",
                    dir.display()
                );
            }
        }
        let _ = std::fs::remove_file(&apk);
    }

    on(Step::Status("Checking the game data...".into()));
    if !quest_install::data_present(&r.data.contents) {
        on(Step::Status("Downloading the game data...".into()));
        let zip = fetch_data(&r, cancel, on)?;
        quest_install::push_data(&zip, &mut |s| on(Step::Status(s)))?;
        let _ = std::fs::remove_file(&zip);
        if !quest_install::data_present(&r.data.contents) {
            bail!("The game data didn't arrive complete on your Quest. Please try again.");
        }
    }
    // The community content (the Echo Combat update's asset patches), as Crafter's
    // installer adds it.
    crate::core::quest_content::apply(cancel, &staging(), &mut |s| on(Step::Status(s)))?;
    quest_install::grant_permissions();

    // What the headset reports now has to be what nEVR's manifest names.
    let now = quest_install::installed();
    if now.as_ref().and_then(|i| i.version_code) != Some(r.version_code) {
        bail!(
            "Your Quest doesn't report nEVR {v} after the install (versionCode {:?}). Please try again.",
            now.and_then(|i| i.version_code)
        );
    }
    // A headset on USB is let go of; one over the network stays connected.
    let network = adb::last_selection()
        .serial
        .is_some_and(|s| super::quest_net::is_network_serial(&s));
    if !network {
        adb::exec(&["kill-server"]);
    }
    Ok(if current {
        Outcome::UpToDate(v)
    } else {
        Outcome::Installed(v)
    })
}

/// An APK the headset refused for a reason of its own.
fn bail_refused(why: &str) -> Result<()> {
    Err(UiError::new(
        "Install Failed",
        format!("Your Quest refused the APK:\n{why}"),
    )
    .into())
}

/// The game data nEVR's manifest names (from its release folder), checked against its sha256.
fn fetch_data(r: &QuestRelease, cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<PathBuf> {
    let job = download::Job {
        url: r.data.url.clone(),
        dir: staging(),
        filename: DATA_ZIP.into(),
        use_mirror: false,
        extract: false,
        sha256: Some(r.data.sha256.clone()),
    };
    let file = download::run(&job, cancel, &mut |p| match p {
        Progress::Percent(v) => on(Step::Percent(v)),
        Progress::Status(s) => on(Step::Status(s)),
        _ => {}
    })?;
    on(Step::Status("Verifying the download...".into()));
    if !download::sha256_matches(&file, &r.data.sha256) {
        let _ = std::fs::remove_file(&file);
        bail!("The downloaded game data is corrupt (checksum mismatch). Please try again.");
    }
    Ok(file)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestInfo {
    pub device: Option<String>,
    pub installed: bool,
    /// The installed APK's versionCode.
    pub version_code: Option<u64>,
    /// The nEVR release the installed APK is, when it is one.
    pub nevr: Option<String>,
    /// The release the launcher channel offers, and its versionCode.
    pub offered: Option<(String, u64)>,
    /// Its Wi-Fi address, when it was read over USB.
    pub wifi_ip: Option<std::net::Ipv4Addr>,
    /// ADB reaches it over the network rather than USB.
    pub over_network: bool,
}

impl QuestInfo {
    pub fn version_label(&self) -> String {
        match (self.installed, &self.nevr) {
            (false, _) => "Echo VR is not installed on this Quest".into(),
            (true, Some(v)) => format!("nEVR {v}"),
            (true, None) => "Echo VR (another build)".into(),
        }
    }

    /// The release to update to, when Echo VR is installed and isn't the offered one.
    pub fn update_offered(&self) -> Option<&str> {
        match &self.offered {
            Some((v, code)) if self.installed && self.version_code != Some(*code) => {
                Some(v.as_str())
            }
            _ => None,
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
    let have = quest_install::installed();
    let version_code = have.as_ref().and_then(|i| i.version_code);
    let offered = match offered(&AtomicBool::new(false)) {
        Ok(r) => r.map(|r| (r.version, r.version_code)),
        Err(e) => {
            tracing::info!("Quest: no release offer read: {e:#}");
            None
        }
    };
    Ok(QuestInfo {
        device,
        installed: have.is_some(),
        version_code,
        nevr: version_code.and_then(nevr_release::quest_version_of),
        offered,
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
    // nEVR's build lists its activity as Quest apps do (category INFO), older builds as a
    // launcher's.
    let component = [
        "android.intent.category.LAUNCHER",
        "android.intent.category.INFO",
    ]
    .iter()
    .find_map(|c| {
        let out = adb::shell(&format!(
            "cmd package resolve-activity --brief -a android.intent.action.MAIN -c {c} {PACKAGE}"
        ));
        parse_activity(&out.output)
    });
    let Some(component) = component else {
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
    fn activity_parsing() {
        let out = "priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=true\ncom.readyatdawn.r15/com.unity3d.player.UnityPlayerActivity\n";
        assert_eq!(
            parse_activity(out).as_deref(),
            Some("com.readyatdawn.r15/com.unity3d.player.UnityPlayerActivity")
        );
        assert_eq!(parse_activity("No activity found"), None);
    }

    #[test]
    fn version_label_and_offer() {
        let mut i = QuestInfo::default();
        assert!(i.version_label().contains("not installed"));
        i.offered = Some(("6.2.0".into(), 60200999));
        assert_eq!(i.update_offered(), None);
        i.installed = true;
        assert_eq!(i.version_label(), "Echo VR (another build)");
        assert_eq!(i.update_offered(), Some("6.2.0"));
        (i.version_code, i.nevr) = (Some(60100999), Some("6.1.0".into()));
        assert_eq!(i.version_label(), "nEVR 6.1.0");
        assert_eq!(i.update_offered(), Some("6.2.0"));
        i.version_code = Some(60200999);
        assert_eq!(i.update_offered(), None);
    }

    #[test]
    fn the_release_is_installed_only_with_its_code_and_bytes() {
        let r = QuestRelease {
            version: "6.2.0".into(),
            apk: nevr_release::Artifact {
                url: String::new(),
                sha256: "a".repeat(64),
                size: 1,
            },
            version_code: 60200999,
            signer_cert_sha256: String::new(),
            data: nevr_release::GameData {
                url: String::new(),
                sha256: String::new(),
                size: 0,
                contents: Vec::new(),
            },
        };
        let mut i = quest_install::Installed {
            path: "/data/app/base.apk".into(),
            version_code: Some(60200999),
        };
        assert!(is_release(&i, &r, |_| Some("a".repeat(64))));
        assert!(!is_release(&i, &r, |_| Some("b".repeat(64))));
        assert!(!is_release(&i, &r, |_| None));
        i.version_code = Some(4987566);
        assert!(!is_release(&i, &r, |_| panic!("no hash for another code")));
    }
}
