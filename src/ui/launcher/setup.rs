//! Setting up, inline: the questions before an install (own Echo VR or not, how you
//! play), the licence patch (Discord or a link, fetched while the game
//! downloads) and the SteamVR (Revive) setup.

use std::sync::mpsc::sync_channel;

use std::path::PathBuf;

use super::hero::{self, JobView};
use super::versions::job_err;
use super::{Dashboard, JobKind, JobResult, Msg, H, W};
use crate::core::error::UiError;
use crate::core::launcher::catalog::VersionEntry;
use crate::core::launcher::launch;
use crate::core::launcher::patch::{self, FetchError, Source};
use crate::core::launcher::quest::{self as quest_core, ApkSource, JobError, UpdateOutcome};
use crate::core::launcher::relay;
use crate::core::launcher::store::{InstalledVersion, RelayAccount, Runtime, SteamVrVia, Target};
use crate::core::launcher::versions::Step;
use crate::core::{download, echoxr, elevation, oauth, paths, platform, revive};
use crate::ui::design::{self, dz};
use crate::ui::kit::Kit;
use crate::ui::parts::Tx;
use crate::ui::widgets::{Tone, BTN_H};

pub(super) const REVIVE_JOB: &str = "revive";
/// SteamVR through EchoXR (Windows).
pub(super) const ECHOXR_JOB: &str = "echoxr";
/// Putting Echo VR into SteamVR's library (or taking it out) on its own.
const LIBRARY_JOB: &str = "steamvr-library";
pub(super) const CONSENT_KEY: &str = "admin-consent";
pub(super) const JOIN_KEY: &str = "join-server";
pub(super) const QUEST_JOB: &str = "quest";
pub(super) const QUEST_REINSTALL_KEY: &str = "quest-reinstall";
/// A new player's patch, fetched while their version installs.
pub(super) const LICENCE_JOB: &str = "licence";

/// A card over the whole window.
#[derive(Clone)]
pub(super) enum Overlay {
    /// Before an install: your licence and (PC) how you play, then Install.
    Install(Box<InstallAsk>),
    /// Before the first PLAY of a version that wasn't installed here: your licence.
    Owner,
    /// Your account on the classic lobbies server (`play`: PLAY goes on once it's saved).
    RelayAccount {
        name: String,
        password: String,
        play: bool,
    },
    /// The licence patch for installed version `id`: authorize with Discord, or (`link`)
    /// use a patch link.
    Licence { id: String, url: String, link: bool },
    /// "Join a lobby": a lobby link or ID to start Echo VR into.
    JoinLobby { input: String },
    /// EchoVRCE: sign in with pasted tokens.
    VrceTokens { input: String },
    /// Servers: start a new match (mode, place, guild, Combat map).
    StartServer {
        mode: usize,
        region: Option<String>,
        guild: Option<String>,
        level: usize,
    },
    /// Servers: a match's link to copy, and invites for friends (`started`: you just
    /// started it).
    ShareMatch { match_id: String, started: bool },
}

/// The Install card's answers, prefilled with the last ones.
#[derive(Clone)]
pub(super) struct InstallAsk {
    pub target: InstallFor,
    pub owner: Option<bool>,
    pub runtime: Runtime,
    /// A new player's patch from a link they have, instead of Discord.
    pub link: bool,
    pub url: String,
    /// PC: a copy of Echo VR already on this PC (found, or chosen), as its install root.
    pub copy: Option<String>,
    /// Take that copy in instead of downloading.
    pub use_copy: bool,
    /// The copy was chosen or turned down: a copy found later isn't offered over it.
    pub copy_decided: bool,
}

/// What the Install card installs.
#[derive(Clone)]
pub(super) enum InstallFor {
    Pc(Box<VersionEntry>),
    Quest,
}

/// A new player's patch on its way into version `version`: fetched while it installs,
/// put in once both are done.
pub(super) struct PendingPatch {
    pub version: String,
    pub dll: Option<PathBuf>,
}

// ---- jobs ----

/// The licence card for installed version `id` (PATCH, MANAGE, the first PLAY's answer).
pub(super) fn licence(id: &str) -> Overlay {
    Overlay::Licence {
        id: id.to_string(),
        url: String::new(),
        link: false,
    }
}

/// Asks before installing `e` on this PC: your licence and how you play, prefilled with
/// your last answers.
pub(super) fn ask_install(d: &mut Dashboard, e: VersionEntry) {
    // Event builds always start in VR.
    let runtime = match d.state.profile.runtime {
        Runtime::Flat if e.publisher_lock.is_some() => Runtime::MetaLink,
        rt => rt,
    };
    let copy = offers_copy(d, &e)
        .then(|| d.found_installs().into_iter().next())
        .flatten();
    d.overlay = Some(Overlay::Install(Box::new(InstallAsk {
        target: InstallFor::Pc(Box::new(e)),
        owner: d.state.owner,
        runtime,
        link: false,
        url: String::new(),
        use_copy: copy.is_some(),
        copy,
        copy_decided: false,
    })));
}

/// Whether the Install card of `e` offers a copy found on this PC on its own: a live
/// build (found copies are the Meta app's, or the old installer's), not installed yet.
fn offers_copy(d: &Dashboard, e: &VersionEntry) -> bool {
    e.publisher_lock.is_none() && e.exe.is_none() && d.state.installed_from(&e.id).is_none()
}

/// Asks before installing on the Quest: your licence (`link`: a new player's patched
/// build from a link). Unanswered, it starts from what the headset has.
pub(super) fn ask_quest_install(d: &mut Dashboard, link: bool) {
    let owner = if link {
        Some(false)
    } else {
        d.state.owner.or(d.quest_was_patched.then_some(false))
    };
    d.overlay = Some(Overlay::Install(Box::new(InstallAsk {
        target: InstallFor::Quest,
        owner,
        runtime: d.state.profile.runtime,
        link,
        url: String::new(),
        copy: None,
        use_copy: false,
        copy_decided: false,
    })));
}

/// Installs as the Install card was answered: keeps the answers, then installs. A new
/// player's patch comes meanwhile (PC), or first (the Quest's patched build).
fn confirm_install(d: &mut Dashboard, ctx: &egui::Context, ask: InstallAsk) {
    // Event builds don't ask: EchoRelay's patch replaces the licence check.
    let event = matches!(&ask.target, InstallFor::Pc(e) if e.publisher_lock.is_some());
    let own = match (event, ask.owner) {
        (true, _) => true,
        (false, Some(own)) => own,
        (false, None) => return,
    };
    if !event {
        d.state.owner = Some(own);
    }
    let link = ask.link.then(|| ask.url.trim().to_string());
    match ask.target {
        // A copy already on this PC: taken in (checked first) instead of a download. Its
        // licence patch, if any, stays; a new player without one gets PATCH on Play.
        InstallFor::Pc(e) if ask.use_copy && ask.copy.is_some() => {
            d.state.profile.runtime = ask.runtime;
            let root = ask.copy.unwrap_or_default();
            let probe = InstalledVersion {
                exe: paths::find_exe(&root).map(str::to_string),
                root: root.clone(),
                ..Default::default()
            };
            let patched = patch::is_applied(&probe.bin_dir());
            let id = super::versions::adopt(d, ctx, *e, root);
            if !own && !patched && d.jobs.contains_key(&id) {
                fetch_licence(d, ctx, &id, link.map_or(Source::Discord, Source::Url));
            }
        }
        InstallFor::Pc(e) => {
            // Already installed: a reinstall, which checks the files and fetches only the
            // broken ones. A new player's patch stays; an owner gets the original back.
            let installed = d.state.installed_from(&e.id).cloned();
            let id = installed.as_ref().map_or(e.id.clone(), |v| v.id.clone());
            let patched = installed.as_ref().is_some_and(|v| v.patched);
            d.state.profile.runtime = ask.runtime;
            d.state.selected = Some(id.clone());
            d.save();
            match installed {
                Some(v) => super::versions::reinstall(d, ctx, *e, v, !own && patched),
                None => super::versions::install(d, ctx, *e),
            }
            if !own && !patched && d.jobs.contains_key(&id) {
                fetch_licence(d, ctx, &id, link.map_or(Source::Discord, Source::Url));
            }
        }
        InstallFor::Quest => {
            d.save();
            let source = match (own, link) {
                (true, _) => ApkSource::Stock,
                (false, None) => ApkSource::Discord,
                (false, Some(url)) => ApkSource::Url(url),
            };
            quest_install(d, ctx, source);
        }
    }
}

/// Gets a new player's patch for version `id` while it installs: Discord opens right
/// away, and the patch waits in the download folder until the game is in place
/// ([`apply_pending`]).
fn fetch_licence(d: &mut Dashboard, ctx: &egui::Context, id: &str, source: Source) {
    d.pending_patch = Some(PendingPatch {
        version: id.to_string(),
        dll: None,
    });
    // One patch is yours for every version: one already on its way will do.
    if d.jobs.contains_key(LICENCE_JOB) {
        return;
    }
    let first = match source {
        Source::Discord => "Opening Discord in your browser...",
        _ => "Downloading your patch...",
    };
    d.start_job(
        ctx,
        JobKind::Licence,
        LICENCE_JOB,
        "Getting your licence patch",
        first,
        move |cancel, on| {
            let staged = patch::staged();
            if source == Source::Discord && staged.is_file() {
                return JobResult::LicenceFetched(staged);
            }
            match patch::fetch(&source, cancel, on) {
                Ok(dll) => JobResult::LicenceFetched(dll),
                Err(FetchError::OAuth(e)) => JobResult::OAuthFailed(e),
                Err(FetchError::Other(e)) => job_err(e, "Licence Patch Failed"),
            }
        },
    );
}

/// Pure: a pending patch goes in once it is fetched and its version is installed, with
/// no install of it still running.
fn pending_ready(fetched: bool, installed: bool, installing: bool) -> bool {
    fetched && installed && !installing
}

/// Puts a new player's patch into its version once both are there: called when the
/// patch arrives and when the install finishes, whichever is last.
pub(super) fn apply_pending(d: &mut Dashboard, ctx: &egui::Context) {
    let ready = d.pending_patch.as_ref().is_some_and(|p| {
        pending_ready(
            p.dll.is_some(),
            d.state.version(&p.version).is_some(),
            d.jobs.contains_key(&p.version),
        )
    });
    if let Some(PendingPatch {
        version,
        dll: Some(dll),
    }) = ready.then(|| d.pending_patch.take()).flatten()
    {
        patch(d, ctx, &version, Source::Staged(dll));
    }
}

/// The account card, filled with the saved account (`play`: PLAY goes on after Save).
pub(super) fn relay_account(d: &Dashboard, play: bool) -> Overlay {
    let saved = d.state.relay_account.clone().unwrap_or_default();
    Overlay::RelayAccount {
        name: saved.name,
        password: saved.password,
        play,
    }
}

/// Points every installed event build at the classic lobbies server as your account, so
/// a desktop shortcut starts them right too. PLAY writes the started one again.
pub(super) fn write_relay_configs(d: &Dashboard) {
    let Some(account) = &d.state.relay_account else {
        return;
    };
    for v in d
        .state
        .versions
        .iter()
        .filter(|v| v.publisher_lock.is_some() && v.present())
    {
        if let Err(e) = relay::write_config(v, &d.state.relay_server, account) {
            tracing::warn!("classic lobbies config for {}: {e:#}", v.id);
        }
    }
}

/// The job to show for version `id`: its own, or the patch it waits for.
pub(super) fn job_for(d: &Dashboard, id: &str) -> Option<JobView> {
    hero::job_view(d, id).or_else(|| {
        d.pending_patch
            .as_ref()
            .filter(|p| p.version == id)
            .and_then(|_| hero::job_view(d, LICENCE_JOB))
    })
}

/// A job's way to ask for administrator rights: the dialog answers on a channel.
fn consent_asker(tx: Tx<Msg>) -> impl FnMut() -> bool + Send + 'static {
    move || {
        let (s, r) = sync_channel(1);
        tx.send(Msg::Consent(s));
        r.recv().unwrap_or(false)
    }
}

/// Fetches the personal licence patch and puts it into version `id`.
pub(super) fn patch(d: &mut Dashboard, ctx: &egui::Context, id: &str, source: Source) {
    let Some(v) = d.state.version(id).cloned() else {
        return;
    };
    if let Some(why) = d.files_in_use(&v) {
        d.notify(why);
        return;
    }
    let mut consent = consent_asker(d.worker.tx(ctx));
    let first = match source {
        Source::Discord => "Opening Discord in your browser...",
        Source::Url(_) => "Downloading your patch...",
        Source::Staged(_) => "Applying the patch...",
    };
    d.start_job(
        ctx,
        JobKind::Patch,
        id,
        &format!("Patching {}", v.name),
        first,
        move |cancel, on| {
            // A patch from Discord earlier this session is yours too: reuse it. One from
            // a link goes once it is in (it may not be yours).
            let staged = patch::staged();
            let dll = if source == Source::Discord && staged.is_file() {
                staged
            } else {
                match patch::fetch(&source, cancel, on) {
                    Ok(p) => p,
                    Err(FetchError::OAuth(e)) => return JobResult::OAuthFailed(e),
                    Err(FetchError::Other(e)) => return job_err(e, "Licence Patch Failed"),
                }
            };
            on(Step::Status("Applying the patch...".into()));
            let r = elevation::apply_patch(&v.bin_dir(), &dll, &mut consent);
            if patch::from_link(&dll) {
                let _ = std::fs::remove_file(&dll);
            }
            match r {
                Ok(()) => JobResult::Patched,
                Err(e) => job_err(e, "Couldn't write patch"),
            }
        },
    );
}

/// Takes the licence patch off version `id` again.
pub(super) fn unpatch(d: &mut Dashboard, ctx: &egui::Context, id: &str) {
    let Some(v) = d.state.version(id).cloned() else {
        return;
    };
    if let Some(why) = d.files_in_use(&v) {
        d.notify(why);
        return;
    }
    let mut consent = consent_asker(d.worker.tx(ctx));
    d.start_job(
        ctx,
        JobKind::Unpatch,
        id,
        &format!("Removing the patch from {}", v.name),
        "Restoring the original pnsovr.dll...",
        move |_, _| match elevation::remove_patch(&v.bin_dir(), &mut consent) {
            Ok(()) => JobResult::Unpatched,
            Err(e) => job_err(e, "Couldn't remove patch"),
        },
    );
}

/// What SteamVR's library entry starts: the version PLAY starts, with the launch options
/// (executable, arguments joined as a command line).
fn library_target(d: &Dashboard) -> Option<(std::path::PathBuf, String)> {
    let Target::Installed(v) = d.target() else {
        return None;
    };
    Some((v.exe_path(), launch_args(d, &v)))
}

/// The launch options `v` starts with, as a command line: none for an event build (the
/// 2019 one quits on flags it doesn't know).
fn launch_args(d: &Dashboard, v: &InstalledVersion) -> String {
    if v.publisher_lock.is_some() {
        String::new()
    } else {
        launch::join_args(&launch::game_args(&d.state.profile, None))
    }
}

/// Puts Echo VR into SteamVR's library (`add`) or takes it out, asking for administrator
/// rights when Revive's folder needs them.
pub(super) fn steamvr_library(d: &mut Dashboard, ctx: &egui::Context, add: bool) {
    let target = if add { library_target(d) } else { None };
    if add && target.is_none() {
        d.notify("Install Echo VR first: the SteamVR library entry starts it");
        return;
    }
    let mut consent = consent_asker(d.worker.tx(ctx));
    let title = if add {
        "Adding Echo VR to SteamVR"
    } else {
        "Removing Echo VR from SteamVR"
    };
    d.start_job(
        ctx,
        JobKind::Revive,
        LIBRARY_JOB,
        title,
        "Updating the SteamVR library...",
        move |_, _| {
            let (exe, args) = match &target {
                Some((exe, args)) => (Some(exe.as_path()), args.as_str()),
                None => (None, ""),
            };
            match elevation::set_library_entry(exe, args, &mut consent) {
                Ok(()) => JobResult::LibraryEntry(add),
                Err(e) => job_err(e, "SteamVR Library"),
            }
        },
    );
}

/// Sets up SteamVR through EchoXR (Windows): EchoXR and, without the Meta app, Meta's
/// Platform SDK loader; then EchoXR into the selected version's folder (asking for
/// administrator rights for the Meta library's).
pub(super) fn echoxr_windows(d: &mut Dashboard, ctx: &egui::Context) {
    let mut consent = consent_asker(d.worker.tx(ctx));
    let bin = match d.target() {
        Target::Installed(v) if v.publisher_lock.is_none() => Some(v.bin_dir()),
        _ => None,
    };
    d.start_job(
        ctx,
        JobKind::Revive,
        ECHOXR_JOB,
        "Setting up SteamVR through EchoXR",
        "Downloading EchoXR...",
        move |cancel, on| {
            let r = echoxr::fetch(cancel, on).and_then(|()| match &bin {
                Some(bin) => {
                    on(Step::Status(
                        "Putting EchoXR into the game's folder...".into(),
                    ));
                    let platform = echoxr::platform_dir_for(bin);
                    elevation::prepare_echoxr(bin, platform.as_deref(), &mut consent)
                }
                None => Ok(()),
            });
            match r {
                Ok(()) => JobResult::EchoXrReady,
                Err(e) => job_err(e, "SteamVR Setup Failed"),
            }
        },
    );
}

/// Downloads and installs Revive (asking for administrator rights), then the artwork and
/// Echo VR's entry in SteamVR's library.
pub(super) fn revive(d: &mut Dashboard, ctx: &egui::Context) {
    let mut consent = consent_asker(d.worker.tx(ctx));
    let artwork = d.state.revive_artwork;
    let library = d.state.revive_library.then(|| library_target(d)).flatten();
    d.start_job(
        ctx,
        JobKind::Revive,
        REVIVE_JOB,
        "Setting up SteamVR",
        "Downloading Revive...",
        move |cancel, on| {
            let installer = match revive::download_installer(cancel, &mut |p| {
                if let download::Progress::Percent(v) = p {
                    on(Step::Percent(v));
                }
            }) {
                Ok(p) => p,
                Err(e) => return job_err(e, "SteamVR Setup Failed"),
            };
            on(Step::Status("Installing Revive...".into()));
            if let Err(e) = elevation::run_revive_installer(&installer, &mut consent) {
                return job_err(
                    e.context("Installing Revive failed"),
                    "SteamVR Setup Failed",
                );
            }
            on(Step::Status("Waiting for Revive...".into()));
            if revive::wait_for_revive_dir(std::time::Duration::from_secs(8)).is_none() {
                return JobResult::Failed(Some(UiError::new(
                    "SteamVR Setup Failed",
                    "Revive does not appear to be installed (was the installer cancelled?).",
                )));
            }
            // The artwork and the library entry are niceties: Revive works without them.
            let mut notes = Vec::new();
            if artwork {
                on(Step::Status("Installing the game artwork...".into()));
                if let Err(e) = elevation::install_artwork(&mut consent) {
                    tracing::warn!("artwork: {e:#}");
                    notes.push(format!("Its artwork couldn't be installed: {e:#}"));
                }
            }
            if let Some((exe, args)) = &library {
                on(Step::Status(
                    "Adding Echo VR to the SteamVR library...".into(),
                ));
                if let Err(e) = elevation::set_library_entry(Some(exe), args, &mut consent) {
                    tracing::warn!("steamvr library: {e:#}");
                    notes.push(format!("It couldn't be added to SteamVR's library: {e:#}"));
                }
            }
            JobResult::ReviveReady(notes)
        },
    );
}

/// A desktop shortcut to version `id` (through Revive when that is the launch mode).
pub(super) fn shortcut(d: &mut Dashboard, id: &str) {
    let Some(v) = d.state.version(id).cloned() else {
        return;
    };
    let exe = v.exe_path();
    let revive = match (d.state.profile.runtime, d.state.profile.steamvr_via) {
        (Runtime::Revive, SteamVrVia::Revive) if !d.demo => revive::find_revive_dir(),
        _ => None,
    };
    // The launch options from Settings go into the shortcut too. An event build's is
    // named after it, as the relay's own installer names them.
    let args = launch_args(d, &v);
    let name = match &v.publisher_lock {
        Some(_) => format!("Echo VR {}", v.name),
        None => "Echo VR".to_string(),
    };
    let echoxr = d.state.profile.runtime == Runtime::Revive
        && d.state.profile.steamvr_via == SteamVrVia::EchoXr
        && v.publisher_lock.is_none();
    let result = match (d.state.profile.runtime, revive) {
        // EchoXR.exe beside the game starts it on SteamVR, with the game's icon.
        _ if echoxr => platform::create_shortcut(
            &name,
            &v.bin_dir().join(echoxr::LAUNCHER),
            (!args.is_empty()).then_some(args.as_str()),
            Some(&v.bin_dir()),
            Some(&exe),
        ),
        (Runtime::Revive, Some(dir)) => revive::create_injector_shortcut(&dir, &exe, &args),
        _ => platform::create_shortcut(
            &name,
            &exe,
            (!args.is_empty()).then_some(args.as_str()),
            Some(&v.bin_dir()),
            Some(&exe),
        ),
    };
    match result {
        Ok(()) => d.notify("The desktop shortcut is ready"),
        Err(e) => d.dialogs.error(
            "Couldn't create shortcut",
            &format!("{e:#}"),
            Default::default(),
        ),
    }
}

/// Version `v` needs the licence patch before PLAY: you're a new player, it isn't patched,
/// and the PC game runs here (macOS can't, so nothing asks for the patch there).
pub(super) fn needs_patch(d: &Dashboard, v: &InstalledVersion) -> bool {
    // Event builds get EchoRelay's patch instead, which skips the licence check.
    v.publisher_lock.is_none()
        && d.state.owner == Some(false)
        && !v.patched
        && (cfg!(any(windows, target_os = "linux")) || d.demo)
}

/// The ways to play this PC offers: SteamVR (through Revive) only on Windows; on Linux
/// VR is one way (OpenXR through EchoXR, "VR" on the Meta Link tile) and Flat. Snapshots
/// show all of them, as on Windows.
pub(super) fn runtimes(d: &Dashboard) -> Vec<Runtime> {
    let linux = cfg!(target_os = "linux") && !d.demo;
    Runtime::ALL
        .into_iter()
        .filter(|rt| *rt != Runtime::Revive || cfg!(windows) || d.demo)
        .filter(|rt| !(linux && *rt == Runtime::VirtualDesktop))
        .collect()
}

/// Whether PLAY can start the PC game here (snapshots: as on Windows). On Linux, once it
/// is set up (`linux_setup`).
pub(super) fn pc_play_supported(d: &Dashboard) -> bool {
    cfg!(windows) || d.demo || linux_ready(d)
}

/// Linux: GE-Proton, EchoXR and the Steam shortcut are in place.
pub(super) fn linux_ready(d: &Dashboard) -> bool {
    cfg!(target_os = "linux") && d.linux_set_up && d.state.linux_appid.is_some()
}

pub(super) const LINUX_JOB: &str = "linux-setup";

/// Sets up Echo VR for Linux: a private GE-Proton, EchoXR's OpenXR runtime and Meta's
/// Platform SDK loader, then (closing Steam meanwhile) the shortcut in Steam that starts it.
pub(super) fn linux_setup(d: &mut Dashboard, ctx: &egui::Context) {
    use crate::core::linux::{self, echoxr, steam};
    let Some(root) = steam::root() else {
        d.dialogs.error(
            "Steam not found",
            "Echo VR runs through Steam on Linux: install Steam, sign in once, and try again.",
            Default::default(),
        );
        return;
    };
    d.start_job(
        ctx,
        JobKind::Revive,
        LINUX_JOB,
        "Setting up Echo VR for Linux",
        "Preparing...",
        move |cancel, on| {
            if let Err(e) = echoxr::setup(&root, cancel, on) {
                return job_err(e, "Linux Setup Failed");
            }
            let Some(exe) = linux::launcher_exe() else {
                return job_err(
                    anyhow::anyhow!("the launcher's own path is unknown"),
                    "Linux Setup Failed",
                );
            };
            on(Step::Status(
                "Adding Echo VR to Steam (Steam restarts)...".into(),
            ));
            let shortcut = steam::Shortcut {
                exe,
                launch_options: linux::PLAY_FLAG.into(),
                icon: None,
            };
            let r = steam::shutdown(&root)
                .and_then(|()| steam::install_shortcut(&root, &shortcut))
                .and_then(|appid| steam::start(&root).map(|()| appid));
            match r {
                Ok(appid) => JobResult::LinuxReady(appid),
                Err(e) => job_err(e, "Linux Setup Failed"),
            }
        },
    );
}

pub(super) fn quest_install(d: &mut Dashboard, ctx: &egui::Context, source: ApkSource) {
    let first = match source {
        ApkSource::Discord => "Opening Discord in your browser...",
        _ => "Checking for the latest version...",
    };
    // The catalogue's APK, for when the update manifest (which names the current one)
    // can't be fetched.
    let fallback = d
        .catalog
        .as_ref()
        .and_then(|c| {
            c.versions
                .iter()
                .find(|e| e.platform == crate::core::launcher::catalog::Platform::Quest)
        })
        .filter(|e| e.uses_mirror() && e.url.ends_with(".apk"))
        .map(|e| e.url.clone());
    d.start_job(
        ctx,
        JobKind::QuestInstall,
        QUEST_JOB,
        "Installing Echo VR on your Quest",
        first,
        move |cancel, on| match quest_core::install(&source, fallback.as_deref(), cancel, on) {
            Ok(outcome) => JobResult::QuestInstalled(outcome),
            Err(JobError::OAuth(e)) => JobResult::OAuthFailed(e),
            Err(JobError::Other(e)) => job_err(e, "Installation Failed"),
        },
    );
}

pub(super) fn quest_update(d: &mut Dashboard, ctx: &egui::Context) {
    d.start_job(
        ctx,
        JobKind::QuestUpdate,
        QUEST_JOB,
        "Updating Echo VR on your Quest",
        "Checking your Quest...",
        move |cancel, on| match quest_core::update(cancel, on) {
            Ok(UpdateOutcome::Updated) => JobResult::QuestUpdated,
            Ok(UpdateOutcome::NeedsReinstall(detail)) => JobResult::QuestNeedsReinstall(detail),
            Err(e) => job_err(e, "Update Failed"),
        },
    );
}

// ---- overlay cards ----

pub(super) fn draw_overlay(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    if d.overlay.is_none() {
        return;
    }
    let blocked = kit.blocked;
    kit.modal("overlay", blocked, |k| match &d.overlay {
        Some(Overlay::Install(_)) => install_card(d, k, ctx),
        Some(Overlay::Owner) => owner_card(d, k, ctx),
        Some(Overlay::RelayAccount { .. }) => relay_account_card(d, k, ctx),
        Some(Overlay::Licence { .. }) => licence_card(d, k, ctx),
        Some(Overlay::JoinLobby { .. }) => super::play::lobby_card(d, k, ctx),
        Some(Overlay::VrceTokens { .. }) => super::echovrce::tokens_card(d, k, ctx),
        Some(Overlay::StartServer { .. }) => super::servers::start_card(d, k, ctx),
        Some(Overlay::ShareMatch { .. }) => super::servers::share_card(d, k, ctx),
        None => {}
    });
}

pub(super) const OWN_NOTE: &str =
    "You play with your own licence from the Meta store. The licence patch stays optional.";
pub(super) const NEW_NOTE: &str =
    "No licence yet? You get a personal licence patch through Discord before your first match.";

pub(super) fn runtime_note(r: Runtime) -> &'static str {
    match r {
        // On Linux every headset plays through its OpenXR runtime (EchoXR).
        Runtime::MetaLink if cfg!(target_os = "linux") => {
            "Any headset on SteamVR, WiVRn or Monado: start its OpenXR runtime first."
        }
        Runtime::MetaLink => "Quest over Link or Air Link, or a Rift, with the Meta Quest app.",
        Runtime::VirtualDesktop => "Quest over Virtual Desktop; start its streamer first.",
        Runtime::Revive => "Any SteamVR headset. The launcher sets it up for you.",
        Runtime::Flat => "No headset: play or spectate on the monitor.",
    }
}

/// A runtime's name on its tile (the Install card's, Settings').
pub(super) fn runtime_label(r: Runtime) -> &'static str {
    match r {
        Runtime::MetaLink if cfg!(target_os = "linux") => "VR",
        Runtime::MetaLink => "Meta Link",
        Runtime::VirtualDesktop => "Virtual Desktop",
        Runtime::Revive => "SteamVR",
        Runtime::Flat => "Flat",
    }
}

/// A centred overlay card `w`×`h` with its title strip; returns its content's left edge,
/// top, width and bottom.
pub(super) fn card(k: &Kit, w: f32, h: f32, title: &str) -> (f32, f32, f32, f32) {
    let (x, y) = (
        ((W + k.ex - w) / 2.0).round(),
        ((H + k.ey - h) / 2.0).round(),
    );
    k.solid_panel(x, y, w, h);
    k.header_strip(x, y, w, dz(46.0), title);
    let pad = dz(30.0);
    (x + pad, y + dz(46.0) + pad, w - 2.0 * pad, y + h - pad)
}

/// A question over a card's answers, in the cards' caps.
fn question(k: &Kit, x: f32, y: f32, w: f32, text: &str) {
    let q = k.spaced_fit(
        &text.to_uppercase(),
        design::conthrax(18.0),
        design::TEXT,
        dz(2.0),
        false,
        w,
    );
    k.put(x, y, q);
}

/// Space between a card text's paragraphs.
const PARA: f32 = 10.0;

/// The height of `text` in a card `w` wide.
fn text_height(k: &Kit, text: &str, w: f32) -> f32 {
    let block = k.caps_block(text, 17.0, design::BODY, w);
    block.iter().map(|g| g.size().y).sum::<f32>() + dz(PARA) * block.len().saturating_sub(1) as f32
}

/// The height of [`patch_options`] with `text`, `w` wide.
fn patch_options_h(k: &Kit, text: &str, w: f32, link: bool) -> f32 {
    let field = if link {
        dz(20.0) + BTN_H + dz(34.0)
    } else {
        0.0
    };
    text_height(k, text, w) + dz(14.0 + 24.0 + 26.0 + 24.0) + field
}

/// A patch link the card can use: any, when it doesn't use one.
fn link_ok(quest: bool, link: bool, url: &str) -> bool {
    let validate = if quest {
        oauth::validate_apk_url
    } else {
        oauth::validate_dll_url
    };
    !link || validate(url.trim()).is_some()
}

/// The patch's options, as the installer's patch panel: what it is, the Patcher server
/// (only its members get one), and "Use a patch link instead" with its field.
#[allow(clippy::too_many_arguments)]
fn patch_options(
    k: &mut Kit,
    x: f32,
    y: f32,
    w: f32,
    text: &str,
    quest: bool,
    link: &mut bool,
    url: &mut String,
) {
    let th = k.caps_text(x, y, w, text, 17.0, design::BODY, dz(PARA));
    let mut ry = y + th + dz(14.0);
    let join_tip = "Only its members get a patch: join, then authorize";
    if k.link(
        "licence-join",
        x,
        ry,
        "Join the Echo VR Patcher server",
        16.0,
        join_tip,
    )
    .clicked
    {
        platform::open_url(oauth::PATCHER_INVITE);
    }
    ry += dz(24.0 + 26.0);
    let link_tip = if quest {
        "Install a patched build from a link you already got from the Echo VR Discord"
    } else {
        "Use a patch link you already got from the Echo VR Discord instead of authorizing"
    };
    k.check(
        "licence-link",
        link,
        "Use a patch link instead",
        x,
        ry,
        true,
        link_tip,
    );
    ry += dz(24.0);
    if !*link {
        return;
    }
    let fy = ry + dz(20.0);
    let pw = k.button_width("Paste", None, BTN_H).max(100.0);
    let invalid = !url.trim().is_empty() && !link_ok(quest, true, url);
    k.field(
        "licence-url",
        url,
        x,
        fy,
        w - pw - 10.0,
        BTN_H,
        "https://files.echovr.de/...",
        invalid,
        "The link to your personal patch",
    );
    if k.button(
        "licence-paste",
        x + w - pw,
        fy,
        pw,
        BTN_H,
        Tone::Dark,
        None,
        "Paste",
        true,
        "Paste a link from your clipboard",
    )
    .clicked
    {
        if let Some(clip) = arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
        {
            *url = clip.trim().to_string();
        }
    }
    if invalid {
        let msg = "That doesn't look like a patch link.";
        k.caps_text(x, fy + BTN_H + dz(10.0), w, msg, 16.0, design::DANGER, 0.0);
    }
}

/// The action and Cancel at the bottom right of a card; returns (action, cancel) clicks.
/// Escape cancels too.
#[allow(clippy::too_many_arguments)]
fn card_buttons(
    k: &mut Kit,
    ctx: &egui::Context,
    key: &str,
    right: f32,
    by: f32,
    label: &str,
    enabled: bool,
    tip: &str,
) -> (bool, bool) {
    let aw = k.button_width(label, None, BTN_H).max(140.0);
    let cw = k.button_width("Cancel", None, BTN_H).max(110.0);
    let cancel = k
        .button(
            &format!("{key}-cancel"),
            right - cw,
            by,
            cw,
            BTN_H,
            Tone::Dark,
            None,
            "Cancel",
            true,
            "",
        )
        .clicked
        || ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let go = k
        .button(
            &format!("{key}-go"),
            right - cw - 8.0 - aw,
            by,
            aw,
            BTN_H,
            Tone::Go,
            None,
            label,
            enabled,
            tip,
        )
        .clicked;
    (go && !cancel, cancel)
}

/// Before an install: your licence (with the patch's options for new players), how you
/// play and where it goes (PC). Prefilled with your last answers.
fn install_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let busy = d.any_job();
    let offered = runtimes(d);
    let library = d.state.library.clone();
    // Already there (its files gone, or another build on the headset): a reinstall.
    let reinstall = match &d.overlay {
        Some(Overlay::Install(ask)) => match &ask.target {
            InstallFor::Pc(e) => d.state.installed_from(&e.id).is_some(),
            InstallFor::Quest => d.quest_info.as_ref().is_some_and(|i| i.installed),
        },
        _ => false,
    };
    let verb = if reinstall { "Reinstall" } else { "Install" };
    // A copy found on this PC meanwhile (the search runs in the background) is offered
    // until one is chosen or turned down.
    let look = match &d.overlay {
        Some(Overlay::Install(ask)) => match &ask.target {
            InstallFor::Pc(e) => ask.copy.is_none() && !ask.copy_decided && offers_copy(d, e),
            InstallFor::Quest => false,
        },
        _ => false,
    };
    let found = look
        .then(|| d.found_installs().into_iter().next())
        .flatten();
    let Some(Overlay::Install(ask)) = &mut d.overlay else {
        return;
    };
    let ask = &mut **ask;
    if let Some(copy) = found {
        ask.copy = Some(copy);
        ask.use_copy = true;
    }
    let use_copy = ask.use_copy && ask.copy.is_some();
    let (title, root) = match &ask.target {
        InstallFor::Pc(e) => (
            format!("{verb} {}", e.name),
            Some(crate::core::launcher::versions::root_for(&library, &e.id)),
        ),
        InstallFor::Quest => (format!("{verb} Echo VR on your Quest"), None),
    };
    let quest = root.is_none();
    // An event build: no licence (EchoRelay's patch skips its check), and it always
    // starts in VR.
    let event = matches!(&ask.target, InstallFor::Pc(e) if e.publisher_lock.is_some());
    let offered: Vec<Runtime> = offered
        .into_iter()
        .filter(|rt| !(event && *rt == Runtime::Flat))
        .collect();
    let new = !event && ask.owner == Some(false);
    let options = if quest { QUEST_OPTIONS } else { PC_OPTIONS };
    let (w, pad, gap) = (dz(1100.0), dz(30.0), dz(16.0));
    let inner = w - 2.0 * pad;
    let (q_h, owner_h, runtime_h, place_h) = (dz(40.0), dz(110.0), dz(130.0), dz(50.0));
    let where_h = dz(100.0);

    // Measured first: the card fits what it shows.
    let mut h = if event {
        text_height(k, EVENT_NOTE, inner) + dz(24.0)
    } else {
        q_h + owner_h + dz(20.0)
    };
    if new {
        h += patch_options_h(k, options, inner, ask.link) + dz(22.0);
    }
    if quest {
        h += text_height(k, QUEST_WARNING, inner) + dz(24.0);
    } else {
        h += q_h + runtime_h + dz(24.0) + place_h + dz(24.0);
        if ask.copy.is_some() {
            h += q_h + where_h + dz(24.0);
        }
    }
    let h = dz(46.0) + 2.0 * pad + h + BTN_H;
    let (x, mut y, cw, bottom) = card(k, w, h, &title);

    if event {
        y += k.caps_text(x, y, cw, EVENT_NOTE, 17.0, design::BODY, dz(PARA)) + dz(24.0);
    } else {
        question(k, x, y, cw, "Do you own Echo VR on your Meta account?");
        y += q_h;
        let tw = (cw - gap) / 2.0;
        let answers = [
            (true, "I own Echo VR on Meta", OWN_NOTE),
            (false, "I'm a new player", NEW_NOTE),
        ];
        for (i, (own, label, note)) in answers.into_iter().enumerate() {
            let tx = x + i as f32 * (tw + gap);
            let on = ask.owner == Some(own);
            if k.tile(
                &format!("install-owner-{i}"),
                tx,
                y,
                tw,
                owner_h,
                label,
                note,
                on,
            )
            .clicked
            {
                ask.owner = Some(own);
            }
        }
        y += owner_h + dz(20.0);
    }
    if new {
        patch_options(k, x, y, cw, options, quest, &mut ask.link, &mut ask.url);
        y += patch_options_h(k, options, cw, ask.link) + dz(22.0);
    }

    let mut change = false;
    let mut choose = false;
    if let Some(root) = &root {
        question(k, x, y, cw, "How do you play Echo VR?");
        y += q_h;
        let n = offered.len() as f32;
        let rw = (cw - (n - 1.0) * gap) / n;
        for (i, rt) in offered.iter().enumerate() {
            let rx = x + i as f32 * (rw + gap);
            let on = ask.runtime == *rt;
            if k.tile(
                &format!("install-runtime-{i}"),
                rx,
                y,
                rw,
                runtime_h,
                runtime_label(*rt),
                runtime_note(*rt),
                on,
            )
            .clicked
            {
                ask.runtime = *rt;
            }
        }
        y += runtime_h + dz(24.0);
        // Echo VR already on this PC: take it in, or download it anyway.
        if ask.copy.is_some() {
            question(k, x, y, cw, "Where is Echo VR?");
            y += q_h;
            let tw = (cw - gap) / 2.0;
            let size = match &ask.target {
                InstallFor::Pc(e) => e.size.map(super::play::gb),
                InstallFor::Quest => None,
            };
            let download = size.map_or_else(
                || "Downloads it into your library".to_string(),
                |s| format!("Downloads {s} into your library"),
            );
            let tiles = [
                (false, "Download it", download.as_str()),
                (true, "Use the copy on this PC", COPY_NOTE),
            ];
            for (i, (copy, label, note)) in tiles.into_iter().enumerate() {
                let tx = x + i as f32 * (tw + gap);
                if k.tile(
                    &format!("install-where-{i}"),
                    tx,
                    y,
                    tw,
                    where_h,
                    label,
                    note,
                    use_copy == copy,
                )
                .clicked
                {
                    ask.use_copy = copy;
                    ask.copy_decided = true;
                }
            }
            y += where_h + dz(24.0);
        }
        let (caption, path) = match &ask.copy {
            Some(copy) if use_copy => ("Echo VR is in", copy.as_str()),
            _ => ("Install location", root.as_str()),
        };
        let cap = k.caption(x, y, caption);
        if !use_copy {
            let change_tip = "Install into another folder (your library)";
            change = k
                .link(
                    "install-change",
                    x + cap.width() + dz(14.0),
                    y - dz(2.0),
                    "Change",
                    14.0,
                    change_tip,
                )
                .clicked;
        }
        // Anyone who has Echo VR already says where, instead of downloading it again.
        if !reinstall {
            let text = if ask.copy.is_some() {
                "Not this one? Choose echovr.exe"
            } else {
                "Already have it? Choose echovr.exe"
            };
            let lw = k.link_width(text, 14.0);
            choose = k
                .link(
                    "install-choose",
                    x + cw - lw,
                    y - dz(2.0),
                    text,
                    14.0,
                    "Use an Echo VR that is already on this PC: choose its echovr.exe",
                )
                .clicked;
        }
        let path = super::install::myriad(k, path, design::myriad(18.0), design::TEXT, cw, true);
        k.put(x, y + dz(24.0), path);
    } else {
        k.caps_text(x, y, cw, QUEST_WARNING, 17.0, design::BODY, dz(PARA));
    }

    let ready = event || (ask.owner.is_some() && (!new || link_ok(quest, ask.link, &ask.url)));
    let tip = match (quest, ask.owner, ask.link) {
        (false, Some(true), _) if use_copy => {
            "Check this copy against the build's checksums and add it to your library: only broken files are downloaded"
        }
        (false, Some(false), false) if use_copy => {
            "Add this copy; Discord opens in your browser meanwhile for your patch"
        }
        (false, Some(false), true) if use_copy => "Add this copy and put your patch in",
        _ if event && use_copy => {
            "Check this copy against the event build's checksums, add it and EchoRelay's patch"
        }
        _ if event && reinstall => "Check every game file and fetch only the broken ones again",
        _ if event => {
            "Download this event build and add EchoRelay's patch, for the classic lobbies"
        }
        (_, None, _) => "Answer the licence question first",
        (false, Some(true), _) if reinstall => {
            "Download Echo VR again and install it over this copy"
        }
        (false, Some(true), _) => "Download and install Echo VR",
        (false, Some(false), false) => {
            "Download Echo VR; Discord opens in your browser meanwhile for your patch"
        }
        (false, Some(false), true) => {
            "Download Echo VR and your patch, which goes in once it's installed"
        }
        (true, Some(true), _) => "Install the store build on your Quest",
        (true, Some(false), false) => "Authorize with Discord, then install your patched build",
        (true, Some(false), true) => "Download your patched build and install it",
    };
    let (go, cancel) = card_buttons(
        k,
        ctx,
        "install",
        x + cw,
        bottom - BTN_H,
        if use_copy { "Use this copy" } else { verb },
        ready && !busy,
        tip,
    );
    let ask = ask.clone();
    if cancel {
        d.overlay = None;
    } else if go {
        d.overlay = None;
        confirm_install(d, ctx, ask);
    } else if change {
        hero::choose_library(d);
    } else if choose {
        if let Some(root) = super::versions::choose_copy(d) {
            if let Some(Overlay::Install(ask)) = &mut d.overlay {
                ask.copy = Some(root);
                ask.use_copy = true;
                ask.copy_decided = true;
            }
        }
    }
}

/// Before the first PLAY of a version that wasn't installed here: the licence question.
/// Owners play on; new players get the licence card.
fn owner_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let (w, h) = (dz(1000.0), dz(440.0));
    let (x, y, cw, bottom) = card(k, w, h, "Before you play");
    question(k, x, y, cw, "Do you own Echo VR on your Meta account?");
    let top = y + dz(52.0);
    let gap = dz(20.0);
    let tw = (cw - gap) / 2.0;
    let answers = [
        (true, "I own Echo VR on Meta", OWN_NOTE),
        (false, "I'm a new player", NEW_NOTE),
    ];
    let mut picked = None;
    for (i, (own, label, note)) in answers.into_iter().enumerate() {
        let tx = x + i as f32 * (tw + gap);
        if k.tile(
            &format!("owner-{i}"),
            tx,
            top,
            tw,
            dz(200.0),
            label,
            note,
            false,
        )
        .clicked
        {
            picked = Some(own);
        }
    }
    let cw2 = k.button_width("Cancel", None, BTN_H).max(110.0);
    let cancel = k
        .button(
            "owner-cancel",
            x + cw - cw2,
            bottom - BTN_H,
            cw2,
            BTN_H,
            Tone::Dark,
            None,
            "Cancel",
            true,
            "",
        )
        .clicked
        || ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if cancel {
        d.overlay = None;
        d.pending_lobby = None;
        return;
    }
    let Some(own) = picked else {
        return;
    };
    d.state.owner = Some(own);
    d.save();
    d.overlay = None;
    let lobby = d.pending_lobby.take();
    if own {
        super::play::try_start(d, ctx, lobby);
    } else if let Target::Installed(v) = d.target() {
        d.overlay = Some(licence(&v.id));
    }
}

/// The licence patch for an installed version (PATCH, MANAGE, the first PLAY's answer):
/// what it is, Authorize with Discord, or (ticked) a patch link you already have.
fn licence_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let busy = d.any_job();
    let name = match &d.overlay {
        Some(Overlay::Licence { id, .. }) => d.state.version(id).map(|v| v.name.clone()),
        _ => None,
    };
    let Some(Overlay::Licence { id, url, link }) = &mut d.overlay else {
        return;
    };
    let text = PC_LICENCE.replace("{version}", name.as_deref().unwrap_or("this version"));
    let (w, pad) = (dz(960.0), dz(30.0));
    let options_h = patch_options_h(k, &text, w - 2.0 * pad, *link);
    let h = dz(46.0) + 2.0 * pad + options_h + dz(34.0) + BTN_H;
    let (x, y, cw, bottom) = card(k, w, h, "Licence patch");
    patch_options(k, x, y, cw, &text, false, link, url);
    let ready = link_ok(false, *link, url);
    let (label, tip) = if *link {
        (
            "Apply patch",
            "Download the patch and put it into this version",
        )
    } else {
        (
            "Authorize with Discord",
            "Opens Discord in your browser: authorize there, and the patch comes by itself",
        )
    };
    let (go, cancel) = card_buttons(
        k,
        ctx,
        "licence",
        x + cw,
        bottom - BTN_H,
        label,
        ready && !busy,
        tip,
    );
    let (id, from) = (id.clone(), link.then(|| url.trim().to_string()));
    if cancel {
        d.overlay = None;
    } else if go {
        d.overlay = None;
        patch(d, ctx, &id, from.map_or(Source::Discord, Source::Url));
    }
}

/// Your account on the classic lobbies server: a display name and a password, the first
/// login locking the account to it. One account plays every event build.
fn relay_account_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let server = d.state.relay_server.clone();
    let Some(Overlay::RelayAccount {
        name,
        password,
        play,
    }) = &mut d.overlay
    else {
        return;
    };
    let text = RELAY_NOTE.replace("{server}", &server);
    let (w, pad, gap) = (dz(960.0), dz(30.0), dz(20.0));
    let fields_h = dz(30.0) + BTN_H + dz(34.0);
    let h = dz(46.0)
        + 2.0 * pad
        + text_height(k, &text, w - 2.0 * pad)
        + dz(24.0)
        + fields_h
        + dz(10.0)
        + BTN_H;
    let (x, mut y, cw, bottom) = card(k, w, h, "Classic lobbies account");
    y += k.caps_text(x, y, cw, &text, 17.0, design::BODY, dz(PARA)) + dz(24.0);
    let fw = (cw - gap) / 2.0;
    k.caption(x, y, "Display name");
    k.caption(x + fw + gap, y, "Password");
    let fy = y + dz(30.0);
    k.field(
        "relay-name",
        name,
        x,
        fy,
        fw,
        BTN_H,
        "The name above your head",
        false,
        "Up to 20 characters",
    );
    k.secret_field(
        "relay-password",
        password,
        x + fw + gap,
        fy,
        fw,
        BTN_H,
        "Not one you use anywhere else",
        false,
        "Up to 64 characters. Sent without encryption.",
    );
    // The relay's limits, kept while typing.
    for (s, max) in [
        (&mut *name, relay::NAME_MAX),
        (&mut *password, relay::PASSWORD_MAX),
    ] {
        if s.chars().count() > max {
            *s = s.chars().take(max).collect();
        }
    }
    let account = RelayAccount {
        name: name.trim().to_string(),
        password: password.clone(),
    };
    let ready = relay::valid_account(&account);
    let (label, tip) = if *play {
        ("Save & play", "Keep this account and start Echo VR")
    } else {
        ("Save", "Keep this account for every event build")
    };
    let play = *play;
    let (go, cancel) = card_buttons(k, ctx, "relay", x + cw, bottom - BTN_H, label, ready, tip);
    if cancel {
        d.overlay = None;
    } else if go {
        d.overlay = None;
        d.state.relay_account = Some(account);
        d.save();
        write_relay_configs(d);
        if play {
            super::play::try_start(d, ctx, None);
        }
    }
}

const RELAY_NOTE: &str = "Event builds play on the community's classic lobbies server ({server}). Pick a display name and a password: your first login locks the account to that password, and it works in every event build.\n\nDon't reuse a real password: it is sent without encryption.";
const EVENT_NOTE: &str = "An event build: it plays on the community's classic lobbies server, with EchoRelay's patch instead of the licence check. You pick your account there when you first play it.";
const PC_LICENCE: &str = "New players need a personal licence patch. Authorize with Discord and the Echo VR Patcher bot builds one for your account; you need to be a member of its server.\n\nIt replaces pnsovr.dll in {version}. The original is kept, so you can take the patch off again in MANAGE.";
const PC_OPTIONS: &str = "New players need a personal licence patch. Authorize with Discord while Echo VR downloads: the Echo VR Patcher bot builds one for your account (you need to be a member of its server), and it goes in once Echo VR is installed.";
const QUEST_OPTIONS: &str = "New players install a personal patched build. Authorize with Discord and the Echo VR Patcher bot builds it for your account; you need to be a member of its server.";
/// The Install card's tile for a copy of Echo VR already on this PC.
const COPY_NOTE: &str = "Already on this PC: nothing to download";
const QUEST_WARNING: &str = "Installing replaces Echo VR on your Quest: the installed app and its local data are removed first.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_patch_waits_for_its_install() {
        assert!(!pending_ready(false, true, false), "not fetched yet");
        assert!(!pending_ready(true, false, true), "still installing");
        assert!(
            !pending_ready(true, true, true),
            "reinstalling over an old copy"
        );
        assert!(!pending_ready(true, false, false), "the install failed");
        assert!(pending_ready(true, true, false));
    }
}
