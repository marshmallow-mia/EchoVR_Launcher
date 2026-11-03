//! Echo VR on the Quest: nEVR's build, from nEVR's signed releases, installed and
//! updated over adb; which version is installed, and starting/stopping it.
//!
//! Which release goes onto the headset is the player's pick of nEVR's channels (stable or
//! beta, [`QuestChannel`], Play's picker), straight from nEVR's signed manifest the way
//! Crafter's Quest installer does it ([`nevr_release::quest_offer`]): nothing of ours in
//! between. The APK is the same for every player of a channel.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use super::versions::Step;
use crate::core::adb::{self, PACKAGE};
use crate::core::download::{self, Progress};
use crate::core::error::UiError;
use crate::core::nevr_release::{self, QuestRelease};
use crate::core::paths;
use crate::core::quest_install::{self, ApkInstall, MAIN_USER};

pub const DATA_ZIP: &str = "_data.zip";
/// How an install or update ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// nEVR `version` is on the headset now.
    Installed(String),
    /// It already was.
    UpToDate(String),
}

/// nEVR's channel a Quest follows: Play's picker, launcher.json's `quest_channel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestChannel {
    /// nEVR's tested releases; what Crafter's installer installs too.
    #[default]
    Stable,
    Beta,
}

/// Unknown names (a channel a later launcher knows) are stable.
impl<'de> Deserialize<'de> for QuestChannel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Ok(QuestChannel::ALL
            .into_iter()
            .find(|c| c.name() == name)
            .unwrap_or_default())
    }
}

impl QuestChannel {
    pub const ALL: [QuestChannel; 2] = [QuestChannel::Stable, QuestChannel::Beta];

    /// nEVR's name for it ("stable"), as in its manifest and launcher.json.
    pub fn name(self) -> &'static str {
        match self {
            QuestChannel::Stable => "stable",
            QuestChannel::Beta => "beta",
        }
    }

    /// "Stable"
    pub fn label(self) -> &'static str {
        match self {
            QuestChannel::Stable => "Stable",
            QuestChannel::Beta => "Beta",
        }
    }

    /// What following it means.
    pub fn note(self) -> &'static str {
        match self {
            QuestChannel::Stable => "nEVR's tested releases",
            QuestChannel::Beta => "nEVR's newest builds. They can have bugs",
        }
    }
}

static CHANNEL: Mutex<QuestChannel> = Mutex::new(QuestChannel::Stable);

/// Takes launcher.json's Quest channel; called whenever the settings are loaded or saved.
pub fn set_channel(channel: QuestChannel) {
    *CHANNEL.lock().unwrap_or_else(|p| p.into_inner()) = channel;
}

/// The channel the Quest follows now.
pub fn channel() -> QuestChannel {
    *CHANNEL.lock().unwrap_or_else(|p| p.into_inner())
}

fn staging() -> PathBuf {
    paths::downloads_dir()
}

/// What nEVR's manifest offers the Quest now: its channels' releases, and the followed
/// one's ([`channel`]). An `Err` is the manifest refused.
pub fn offer(cancel: &AtomicBool) -> Result<nevr_release::QuestOffer> {
    nevr_release::quest_offer(channel().name(), cancel).map_err(|e| refused(e).into_error())
}

/// The release the followed channel offers, checked against nEVR's signed manifest.
pub fn offered(cancel: &AtomicBool) -> Result<QuestRelease> {
    offer(cancel)?.release.map_err(|e| {
        let e = refused(e);
        if e.step != 7 {
            return e.into_error();
        }
        UiError::new(
            "Not on this channel",
            format!(
                "nEVR has no Echo VR for the Quest on its {} channel right now. Pick the other one under VERSION on the Play page, or try again later.",
                channel().label()
            ),
        )
        .into()
    })
}

fn refused(e: nevr_release::Refused) -> nevr_release::Refused {
    if e.step != nevr_release::CANCELLED {
        tracing::warn!(
            "Quest: nEVR's {} release refused at step {}: {}",
            channel().name(),
            e.step,
            e.detail
        );
    }
    e
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

/// How the release goes onto the headset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Over what the main profile has (`adb install -r`): its settings and app data stay.
    InPlace,
    /// Settings copied off, uninstalled, installed, settings put back: Reinstall, the
    /// fallback when the update in place didn't go through, or Echo VR only in another profile.
    Clean,
    /// Nothing on the headset: installed, and a copy of settings still waiting goes back.
    Fresh,
}

/// Pure: the route for what is installed (`have`) and whether Reinstall was asked for.
fn route(have: Option<&quest_install::Installed>, reinstall: bool) -> Route {
    match have {
        None => Route::Fresh,
        Some(i) if i.in_main() && !reinstall => Route::InPlace,
        Some(_) => Route::Clean,
    }
}

/// Puts the offered nEVR release onto the headset's main profile: nEVR's APKs share one
/// signing key, so an update goes on in place and keeps the settings and the app's data.
/// Only when that doesn't go through (another signer, a newer build, another refusal), when
/// Echo VR is only in another profile, or when `reinstall` asks for it, is it a clean install:
/// the settings are copied off first (nothing is uninstalled when that fails), then uninstall,
/// install, settings back. Then the game data when it isn't there, then the checks.
/// `reinstall` also does it when the release is already there.
pub fn install(cancel: &AtomicBool, reinstall: bool, on: &mut dyn FnMut(Step)) -> Result<Outcome> {
    ready()?;
    on(Step::Status("Checking nEVR's release...".into()));
    let r = offered(cancel)?;
    let v = r.version.clone();
    let have = quest_install::installed();
    // Echo VR in another profile only isn't this release in the main profile: it goes, and
    // the release comes into the main profile.
    let current = !reinstall
        && have
            .as_ref()
            .is_some_and(|i| i.in_main() && is_release(i, &r, quest_install::installed_sha));

    if !current {
        let apk = staging().join(format!("nevr-runtime-v{v}-quest.apk"));
        if !download::sha256_matches(&apk, &r.apk.sha256) {
            on(Step::Status(format!("Downloading Echo VR (nEVR {v})...")));
            std::fs::create_dir_all(staging())?;
            r.apk
                .download(&apk, cancel, &mut |f| on(Step::Percent(f)))
                .map_err(nevr_release::Refused::into_error)?;
        }
        let settings = staging().join("quest-settings");
        let mut way = route(have.as_ref(), reinstall);
        if way == Route::InPlace {
            on(Step::Status(format!("Updating to nEVR {v}...")));
            match quest_install::install_apk(&apk) {
                ApkInstall::Done => {}
                why => {
                    tracing::warn!(
                        "Quest: the update in place didn't go through ({why:?}); reinstalling"
                    );
                    on(Step::Status(
                        "The update didn't go through. Reinstalling, your settings are kept..."
                            .into(),
                    ));
                    way = Route::Clean;
                }
            }
        }
        if way == Route::Clean {
            on(Step::Status("Keeping your Echo VR settings...".into()));
            // An Err stops here, before anything is uninstalled.
            quest_install::save_settings(&settings)?;
            on(Step::Status(format!("Reinstalling nEVR {v}...")));
            quest_install::uninstall()?;
        }
        if way != Route::InPlace {
            on(Step::Status("Installing Echo VR on your Quest...".into()));
            match quest_install::install_apk(&apk) {
                ApkInstall::Done => {}
                ApkInstall::Failed(why) => bail_refused(&why)?,
                ApkInstall::Newer => bail_refused(&format!(
                    "A newer Echo VR than nEVR {v} is still on it and couldn't be taken off."
                ))?,
                ApkInstall::OtherSigner => {
                    bail_refused("Another Echo VR build is still on it and couldn't be taken off.")?
                }
            }
            // The copy stays until it is back on the headset: a failed install above keeps it
            // for the next one.
            if settings.is_dir() {
                on(Step::Status("Putting your Echo VR settings back...".into()));
                if quest_install::restore_settings(&settings) {
                    let _ = std::fs::remove_dir_all(&settings);
                } else {
                    tracing::warn!(
                        "Quest: settings not put back; a copy is in {}",
                        settings.display()
                    );
                }
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
    quest_install::grant_permissions();

    // What the headset reports now has to be what nEVR's manifest names, in the main profile.
    let now = quest_install::installed().filter(quest_install::Installed::in_main);
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

/// A profile on the headset, and whether Echo VR is installed in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub id: u32,
    pub name: String,
    pub echo: bool,
}

impl Profile {
    /// The main profile: the one Echo VR is installed into.
    pub fn is_main(&self) -> bool {
        self.id == MAIN_USER
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestInfo {
    pub device: Option<String>,
    /// Echo VR is in the main profile.
    pub installed: bool,
    /// The headset's profiles (one per Meta account), the main one first.
    pub profiles: Vec<Profile>,
    /// The profile in use in the headset now.
    pub current_profile: Option<u32>,
    /// The installed APK's versionCode.
    pub version_code: Option<u64>,
    /// The nEVR release the installed APK is, when it is one.
    pub nevr: Option<String>,
    /// The release the followed channel offers, and its versionCode.
    pub offered: Option<(String, u64)>,
    /// nEVR's channels and the release each has now (`None`: no Quest release there), when
    /// its manifest was read.
    pub channels: Vec<(QuestChannel, Option<String>)>,
    /// Its Wi-Fi address, when it was read over USB.
    pub wifi_ip: Option<std::net::Ipv4Addr>,
    /// ADB reaches it over the network rather than USB.
    pub over_network: bool,
}

impl QuestInfo {
    pub fn version_label(&self) -> String {
        if let (false, Some(p)) = (self.installed, self.elsewhere()) {
            return format!("Echo VR is only in the profile \"{}\"", p.name);
        }
        match (self.installed, &self.nevr) {
            (false, _) => "Echo VR is not installed on this Quest".into(),
            (true, Some(v)) => format!("nEVR {v}"),
            (true, None) => "Echo VR (another build)".into(),
        }
    }

    /// The main profile's name, when the headset said.
    pub fn main_profile(&self) -> Option<&str> {
        self.profiles
            .iter()
            .find(|p| p.is_main())
            .map(|p| p.name.as_str())
    }

    /// A profile other than the main one with Echo VR in it.
    pub fn elsewhere(&self) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.echo && !p.is_main())
    }

    /// The release to update to, when Echo VR is installed and is another build or an older
    /// nEVR. A newer one isn't taken back to an older channel's release (Reinstall does that).
    pub fn update_offered(&self) -> Option<&str> {
        match &self.offered {
            Some((v, code))
                if self.installed
                    && (self.nevr.is_none() || self.version_code.is_none_or(|c| c < *code)) =>
            {
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
    let mut profiles: Vec<Profile> = quest_install::users()
        .into_iter()
        .map(|u| Profile {
            echo: have.as_ref().is_some_and(|i| i.users.contains(&u.id)),
            id: u.id,
            name: u.name,
        })
        .collect();
    profiles.sort_by_key(|p| !p.is_main());
    let (offered, channels) = match offer(&AtomicBool::new(false)) {
        Ok(o) => (
            o.release.ok().map(|r| (r.version, r.version_code)),
            QuestChannel::ALL
                .into_iter()
                .map(|c| {
                    let v = o.channels.iter().find(|(n, _)| n == c.name());
                    (c, v.and_then(|(_, v)| v.clone()))
                })
                .collect(),
        ),
        Err(e) => {
            tracing::info!("Quest: no release offer read: {e:#}");
            (None, Vec::new())
        }
    };
    Ok(QuestInfo {
        device,
        installed: have.as_ref().is_some_and(quest_install::Installed::in_main),
        profiles,
        current_profile: quest_install::current_user(),
        version_code,
        nevr: version_code.and_then(nevr_release::quest_version_of),
        offered,
        channels,
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
    // Echo VR is in the main profile: from another one, Android starts nothing there.
    if let Some(now) = quest_install::current_user().filter(|u| *u != MAIN_USER) {
        let names = quest_install::users();
        let name = |id: u32| {
            names
                .iter()
                .find(|u| u.id == id)
                .map_or_else(|| format!("profile {id}"), |u| format!("\"{}\"", u.name))
        };
        return Err(UiError::new(
            "Switch profiles in the headset",
            format!(
                "Your Quest is in the profile {} now. Echo VR is in the profile {}: switch to it in the headset, then press PLAY again.",
                name(now),
                name(MAIN_USER)
            ),
        )
        .into());
    }
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

/// Where Echo VR and nEVR keep their logs on the headset, and the folder each one's are
/// saved in. nEVR has moved its own between builds (6.12 writes `nevr-sentinel.log` and
/// `nevr-crashes/` into the app's `Android/data` folder, 6.14 wrote them into
/// `Android/media`), so its logs are looked for by name in every place it has used.
fn log_roots() -> [(String, &'static str); 5] {
    [
        ("/sdcard/r14logs".into(), "r14logs"),
        (format!("/sdcard/Android/data/{PACKAGE}/files"), "data"),
        (format!("/sdcard/Android/media/{PACKAGE}"), "media"),
        ("/sdcard/readyatdawn".into(), "readyatdawn"),
        // Where nEVR's crash records go when it can't write the others.
        ("/data/local/tmp/nevr-crashes".into(), "tmp-crashes"),
    ]
}

/// Folders the search doesn't go into: the game data, not logs.
const NOT_LOGS: [&str; 3] = ["_data", "RAD", "_temp"];

/// Pure: whether the headset's file `name` is a log to save: a log as Upload logs takes
/// them, or the address map nEVR writes beside a crash dump. Never nEVR's sign-in
/// (`device_login.txt`), the player value beside the game's logcat capture (`user.txt`),
/// nor a crash dump itself (it can hold sign-in tokens).
pub fn is_quest_log(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !matches!(n.as_str(), "device_login.txt" | "user.txt")
        && !n.ends_with(".dmp")
        && (crate::core::logs::is_log(&n) || n.ends_with(".dmp.maps"))
}

/// Pure: the logs in `find`'s `listing` of [`log_roots`], by the folder each goes into
/// (`<label>/<its folder below the root>`), with their paths on the headset.
fn quest_logs(listing: &str) -> Vec<(PathBuf, Vec<String>)> {
    let roots = log_roots();
    let mut by_dir: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for path in listing
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('/'))
    {
        let Some((root, label)) = roots
            .iter()
            .find(|(r, _)| path.starts_with(&format!("{r}/")))
        else {
            continue;
        };
        let rel = &path[root.len() + 1..];
        let mut parts: Vec<&str> = rel.split('/').collect();
        let Some(name) = parts.pop() else { continue };
        if !is_quest_log(name)
            || parts
                .iter()
                .any(|p| p.is_empty() || *p == ".." || p.starts_with('.') || NOT_LOGS.contains(p))
        {
            continue;
        }
        let dir = parts.iter().fold(PathBuf::from(label), |d, p| d.join(p));
        match by_dir.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, files)) => files.push(path.to_string()),
            None => by_dir.push((dir, vec![path.to_string()])),
        }
    }
    by_dir
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

/// Saves Echo VR's and nEVR's logs of the last two weeks from the headset into a new
/// folder beside the launcher's logs, with their times, and returns it. Only logs leave
/// the headset (see [`is_quest_log`]).
pub fn pull_logs() -> Result<PathBuf> {
    ready()?;
    let roots: Vec<String> = log_roots().into_iter().map(|(r, _)| r).collect();
    let prune = NOT_LOGS
        .iter()
        .map(|n| format!("-name {n}"))
        .collect::<Vec<_>>()
        .join(" -o ");
    let find = format!(
        "find {} -maxdepth 4 \\( {prune} \\) -prune -o -type f -mtime -14 -print 2>/dev/null",
        roots.join(" ")
    );
    // find exits 1 for a root that isn't there: what it listed counts.
    let listing = adb::exec(&["shell", &find]).output;
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let dest = logs_folder(now);
    let mut saved = 0;
    for (dir, files) in quest_logs(&listing) {
        let local = dest.join(&dir);
        std::fs::create_dir_all(&local)?;
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        if adb::pull_many(&files, &local) {
            saved += files.len();
        } else {
            tracing::warn!("Quest logs: couldn't save all of {}", dir.display());
        }
    }
    if saved == 0 {
        let _ = std::fs::remove_dir_all(&dest);
        bail!("There are no Echo VR logs on your Quest yet: play it once, then try again.");
    }
    tracing::info!("saved {saved} of the Quest's logs to {}", dest.display());
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_logs_leave_the_headset() {
        for keep in [
            "nevr-sentinel.log",
            "nevr-sentinel.1793471728040.log",
            "nevr-sentinel.session-1793471728040.log",
            "afklog.0.txt",
            "assetpatch.log",
            "[r14(client)]-[11-01-2026]_[11-42-47]_10549.log",
            "6f1c-22.dmp.maps",
        ] {
            assert!(is_quest_log(keep), "{keep}");
        }
        for drop in [
            "device_login.txt",
            "DEVICE_LOGIN.TXT",
            "user.txt",
            "config.yaml",
            "install-id.txt",
            "nevr-data-packages.verified",
            "afklog.on",
            "ffr.off",
            "6f1c-22.dmp",
            "2dfe2e7610506f03",
        ] {
            assert!(!is_quest_log(drop), "{drop}");
        }
    }

    #[test]
    fn quest_logs_keep_their_folders() {
        let listing = format!(
            "/sdcard/r14logs/[r14(client)]-[11-01-2026]_[11-42-47]_10549.log\n\
             /sdcard/r14logs/[r14(client)]-[10-31-2026]_[19-43-05]_14427.log\n\
             /sdcard/Android/data/{PACKAGE}/files/nevr-sentinel.log\n\
             /sdcard/Android/data/{PACKAGE}/files/device_login.txt\n\
             /sdcard/Android/data/{PACKAGE}/files/nevr-crashes/c1.dmp\n\
             /sdcard/Android/data/{PACKAGE}/files/nevr-crashes/c1.dmp.maps\n\
             /sdcard/Android/media/{PACKAGE}/files/afk/afklog.0.txt\n\
             /sdcard/Android/media/{PACKAGE}/files/afk/user.txt\n\
             /sdcard/Android/media/{PACKAGE}/files/afk/afklog.on\n\
             /sdcard/Android/media/{PACKAGE}/files/_data/5932408047/rad15/x.txt\n\
             /sdcard/Android/media/{PACKAGE}/asset_patches/assetpatch.log\n\
             /sdcard/Android/media/{PACKAGE}/asset_patches/489bb35d53ca50e9/2dfe2e7610506f03\n\
             /sdcard/Android/media/{PACKAGE}/files/nevr-sentinel.session-1.log\n\
             /sdcard/Android/media/{PACKAGE}2/files/x.log\n\
             find: /sdcard/readyatdawn: No such file or directory\n"
        );
        let got: Vec<(String, Vec<String>)> = quest_logs(&listing)
            .into_iter()
            .map(|(d, f)| {
                let names = f.iter().map(|p| p.rsplit('/').next().unwrap().to_string());
                (d.to_string_lossy().replace('\\', "/"), names.collect())
            })
            .collect();
        let want = [
            (
                "r14logs",
                vec![
                    "[r14(client)]-[11-01-2026]_[11-42-47]_10549.log",
                    "[r14(client)]-[10-31-2026]_[19-43-05]_14427.log",
                ],
            ),
            ("data", vec!["nevr-sentinel.log"]),
            ("data/nevr-crashes", vec!["c1.dmp.maps"]),
            ("media/files/afk", vec!["afklog.0.txt"]),
            ("media/asset_patches", vec!["assetpatch.log"]),
            ("media/files", vec!["nevr-sentinel.session-1.log"]),
        ];
        let want: Vec<(String, Vec<String>)> = want
            .into_iter()
            .map(|(d, f)| (d.to_string(), f.into_iter().map(String::from).collect()))
            .collect();
        assert_eq!(got, want);
    }

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
    fn the_quest_channel_is_stable_unless_picked() {
        assert_eq!(QuestChannel::default(), QuestChannel::Stable);
        let names: Vec<&str> = QuestChannel::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names, nevr_release::QUEST_CHANNELS);
        for c in QuestChannel::ALL {
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(json, format!("\"{}\"", c.name()));
            assert_eq!(serde_json::from_str::<QuestChannel>(&json).unwrap(), c);
        }
        // A channel a later launcher knows: stable.
        assert_eq!(
            serde_json::from_str::<QuestChannel>("\"nightly\"").unwrap(),
            QuestChannel::Stable
        );
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
        // Beta's newer build stays when Stable is older.
        (i.version_code, i.nevr) = (Some(60300999), Some("6.3.0".into()));
        assert_eq!(i.update_offered(), None);
    }

    #[test]
    fn an_update_goes_on_in_place() {
        let mut i = quest_install::Installed {
            path: "/data/app/base.apk".into(),
            version_code: Some(60100999),
            users: vec![MAIN_USER],
        };
        assert_eq!(route(None, false), Route::Fresh);
        assert_eq!(route(None, true), Route::Fresh);
        assert_eq!(route(Some(&i), false), Route::InPlace);
        assert_eq!(route(Some(&i), true), Route::Clean);
        i.users = vec![10];
        assert_eq!(route(Some(&i), false), Route::Clean);
        i.users = vec![MAIN_USER, 10];
        assert_eq!(route(Some(&i), false), Route::InPlace);
    }

    #[test]
    fn echo_in_another_profile_only_is_named() {
        let mut i = QuestInfo {
            profiles: vec![
                Profile {
                    id: 0,
                    name: "Main".into(),
                    echo: false,
                },
                Profile {
                    id: 10,
                    name: "Second".into(),
                    echo: true,
                },
            ],
            ..QuestInfo::default()
        };
        assert_eq!(i.main_profile(), Some("Main"));
        assert_eq!(
            i.version_label(),
            "Echo VR is only in the profile \"Second\""
        );
        i.installed = true;
        i.nevr = Some("6.12.0".into());
        assert_eq!(i.version_label(), "nEVR 6.12.0");
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
            users: vec![MAIN_USER],
        };
        assert!(is_release(&i, &r, |_| Some("a".repeat(64))));
        assert!(!is_release(&i, &r, |_| Some("b".repeat(64))));
        assert!(!is_release(&i, &r, |_| None));
        i.version_code = Some(4987566);
        assert!(!is_release(&i, &r, |_| panic!("no hash for another code")));
    }
}
