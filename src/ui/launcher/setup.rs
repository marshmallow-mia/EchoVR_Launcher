//! Setting up, inline: the questions before an install (how you play, a copy already on
//! this PC), Echo VR onto the Quest, and the SteamVR (Revive) setup.

use std::sync::mpsc::sync_channel;

use super::hero::{self, JobView};
use super::versions::job_err;
use super::{Dashboard, JobKind, JobResult, Msg, Res, H, W};
use crate::core::error::UiError;
use crate::core::launcher::catalog::VersionEntry;
use crate::core::launcher::launch;
use crate::core::launcher::quest as quest_core;
use crate::core::launcher::relay;
use crate::core::launcher::store::{InstalledVersion, PackServer, RelayAccount, Runtime, Target};
use crate::core::launcher::versions::Step;
use crate::core::{download, echoxr, elevation, platform, revive};
use crate::ui::design::{self, dz};
use crate::ui::kit::Kit;
use crate::ui::parts::Tx;
use crate::ui::widgets::{Tone, BTN_H};

pub(super) const REVIVE_JOB: &str = "revive";
/// SteamVR through EchoXR (Windows).
pub(super) const ECHOXR_JOB: &str = "echoxr";
/// Setting an event build up for the classic lobbies before its PLAY (EchoLoader and
/// EchoRelay's patch).
pub(super) const EVENT_JOB: &str = "event-build";
/// Putting Echo VR into SteamVR's library (or taking it out) on its own.
pub(super) const LIBRARY_JOB: &str = "steamvr-library";
pub(super) const CONSENT_KEY: &str = "admin-consent";
pub(super) const QUEST_JOB: &str = "quest";
/// A card over the whole window.
#[derive(Clone)]
pub(super) enum Overlay {
    /// Before an install: (PC) how you play, then Install.
    Install(Box<InstallAsk>),
    /// Your account on the classic lobbies server (`play`: PLAY goes on once it's saved).
    RelayAccount {
        name: String,
        password: String,
        play: bool,
    },
    /// The server content pack `pack` (its name: `name`) plays on instead of EchoVRCE, and
    /// your account there, as typed.
    PackServer {
        pack: String,
        name: String,
        server: PackServer,
    },
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
    /// EchoVRCE turned the login down (the game was closed): what it wants.
    LoginNotice(crate::core::launcher::login_watch::LoginNotice),
    /// Settings: which logs to upload (`off`: the kinds unchecked; all go at first).
    UploadLogs { off: Vec<crate::core::logs::Kind> },
    /// Settings: who made the launcher and what it brings along (`scroll`: the list's).
    Credits { scroll: f32 },
    /// Settings: going back to the version before an update (`scroll`: the list's).
    GoBack { scroll: f32 },
    /// Settings and Install: what to uninstall (`picked`: the parts ticked).
    Uninstall {
        picked: Vec<crate::core::uninstall::Part>,
    },
    /// Mods: a plugin's settings, from its description.
    PluginSettings(Box<super::mods::SettingsSheet>),
    /// Settings: the advanced ones (the launcher's update channel).
    Advanced,
    /// A critical alert, to be acknowledged ([`crate::core::launcher::alerts`]).
    Alert(Box<crate::core::launcher::alerts::Alert>),
    /// More mods → Add by id: an unlisted mod's id, as typed, and what the check of it said.
    AddUnlisted { input: String, note: Option<String> },
}

/// The Install card's answers, prefilled with the last ones.
#[derive(Clone)]
pub(super) struct InstallAsk {
    pub target: InstallFor,
    pub runtime: Runtime,
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

/// Asks before installing `e` on this PC: how you play, prefilled with your last answer.
pub(super) fn ask_install(d: &mut Dashboard, e: VersionEntry) {
    // Event builds always start in VR: the first VR choice this system offers (Meta
    // Link isn't one on Linux).
    let runtime = match d.state.profile.runtime {
        Runtime::Flat if e.publisher_lock.is_some() => runtimes(d)
            .into_iter()
            .find(|rt| *rt != Runtime::Flat)
            .unwrap_or(Runtime::MetaLink),
        rt => rt,
    };
    let copy = offers_copy(d, &e)
        .then(|| d.found_installs().into_iter().next())
        .flatten();
    d.overlay = Some(Overlay::Install(Box::new(InstallAsk {
        target: InstallFor::Pc(Box::new(e)),
        runtime,
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

/// Asks before installing on the Quest.
pub(super) fn ask_quest_install(d: &mut Dashboard) {
    d.overlay = Some(Overlay::Install(Box::new(InstallAsk {
        target: InstallFor::Quest,
        runtime: d.state.profile.runtime,
        copy: None,
        use_copy: false,
        copy_decided: false,
    })));
}

/// Installs as the Install card was answered: keeps the answers, then installs.
fn confirm_install(d: &mut Dashboard, ctx: &egui::Context, ask: InstallAsk) {
    match ask.target {
        // A copy already on this PC: taken in (checked first) instead of a download.
        InstallFor::Pc(e) if ask.use_copy && ask.copy.is_some() => {
            d.state.profile.runtime = ask.runtime;
            let root = ask.copy.unwrap_or_default();
            super::versions::adopt(d, ctx, *e, root);
        }
        InstallFor::Pc(e) => {
            // Already installed: a reinstall, which checks the files and fetches only the
            // broken ones.
            let installed = d.state.installed_from(&e.id).cloned();
            let id = installed.as_ref().map_or(e.id.clone(), |v| v.id.clone());
            d.state.profile.runtime = ask.runtime;
            if installed.is_none() {
                d.select_back.insert(id.clone(), d.state.selected.clone());
            }
            d.state.selected = Some(id.clone());
            d.save();
            match installed {
                Some(v) => super::versions::reinstall(d, ctx, *e, v),
                None => super::versions::install(d, ctx, *e),
            }
        }
        InstallFor::Quest => quest_install(d, ctx),
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

/// The job to show for version `id`.
pub(super) fn job_for(d: &Dashboard, id: &str) -> Option<JobView> {
    hero::job_view(d, id)
}

/// A job's way to ask for administrator rights: the dialog answers on a channel.
pub(super) fn consent_asker(tx: Tx<Msg>) -> impl FnMut() -> bool + Send + 'static {
    move || {
        let (s, r) = sync_channel(1);
        tx.send(Msg::Consent(s));
        r.recv().unwrap_or(false)
    }
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
/// Installs the game artwork for SteamVR's library on its own (Revive was there before
/// the launcher, so PLAY never prepared it), asking for administrator rights if needed.
pub(super) fn revive_artwork(d: &mut Dashboard, ctx: &egui::Context) {
    let mut consent = consent_asker(d.worker.tx(ctx));
    d.start_job(
        ctx,
        JobKind::Revive,
        LIBRARY_JOB,
        vec![Res::Vr],
        "Installing the game artwork",
        "Downloading the artwork...",
        move |_, _| match elevation::install_artwork(&mut consent) {
            Ok(()) => JobResult::ArtworkInstalled,
            Err(e) => job_err(e, "Couldn't install the game artwork"),
        },
    );
}

pub(super) fn steamvr_library(d: &mut Dashboard, ctx: &egui::Context, add: bool) {
    let target = if add { library_target(d) } else { None };
    if add && target.is_none() {
        d.notify("Install Echo VR first: the SteamVR library entry starts it");
        return;
    }
    // What Settings' box should say if this fails: the way it was.
    d.library_wanted = Some(add);
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
        vec![Res::Vr],
        title,
        "Updating the SteamVR library...",
        move |_, _| {
            let (exe, args) = match &target {
                Some((exe, args)) => (Some(exe.as_path()), args.as_str()),
                None => (None, ""),
            };
            match elevation::set_library_entry(exe, args, &mut consent) {
                Ok(()) => JobResult::LibraryEntry(add),
                Err(e) => job_err(e, "Couldn't change SteamVR's library"),
            }
        },
    );
}

/// Sets event build `v` up for the classic lobbies (EchoLoader in its crash reporter's
/// place, EchoRelay's patch as its plugin), as its install does: for a build an older
/// launcher installed, before its PLAY.
pub(super) fn event_build(d: &mut Dashboard, ctx: &egui::Context, v: &InstalledVersion) {
    let v = v.clone();
    d.start_job(
        ctx,
        JobKind::Revive,
        EVENT_JOB,
        vec![Res::Relay, Res::Version(v.id.clone())],
        &format!("Setting up {}", v.name),
        "Adding EchoLoader and EchoRelay's patch...",
        move |cancel, on| match relay::set_up(&v, cancel, on) {
            Ok(()) => JobResult::EventBuildReady,
            Err(e) => job_err(e, "Couldn't set up the classic lobby"),
        },
    );
}

/// Sets the live build `v` up for the EchoRelay server a plugin chose (EchoLoader and
/// EchoRelay's patch in nEVR's place), before its PLAY.
pub(super) fn live_relay(d: &mut Dashboard, ctx: &egui::Context, v: &InstalledVersion) {
    let v = v.clone();
    d.start_job(
        ctx,
        JobKind::Revive,
        EVENT_JOB,
        vec![Res::Relay, Res::Version(v.id.clone())],
        &format!("Setting up {} for EchoRelay", v.name),
        "Adding EchoLoader and EchoRelay's patch...",
        move |cancel, on| match relay::set_up_live(&v, cancel, on) {
            Ok(()) => JobResult::LiveRelayReady,
            Err(e) => job_err(e, "Couldn't set up EchoRelay"),
        },
    );
}

/// Sets up EchoXR on Windows (for SteamVR, or Virtual Desktop through it): EchoXR, then
/// EchoXR into the selected version's folder (asking for administrator rights for the Meta
/// library's).
pub(super) fn echoxr_windows(d: &mut Dashboard, ctx: &egui::Context) {
    let mut consent = consent_asker(d.worker.tx(ctx));
    let (bin, res) = match d.target() {
        Target::Installed(v) => (Some(v.bin_dir()), vec![Res::Vr, Res::Version(v.id.clone())]),
        _ => (None, vec![Res::Vr]),
    };
    let (title, failed) = if d.state.profile.runtime == Runtime::VirtualDesktop {
        (
            "Preparing EchoXR for Virtual Desktop",
            "Couldn't set up EchoXR",
        )
    } else {
        ("Preparing SteamVR (EchoXR)", "Couldn't set up SteamVR")
    };
    d.start_job(
        ctx,
        JobKind::Revive,
        ECHOXR_JOB,
        res,
        title,
        "Downloading EchoXR...",
        move |cancel, on| {
            let r = echoxr::fetch(cancel, on).and_then(|()| match &bin {
                Some(bin) => {
                    on(Step::Status(
                        "Putting EchoXR into the game's folder...".into(),
                    ));
                    elevation::prepare_echoxr(bin, &mut consent)
                }
                None => Ok(()),
            });
            match r {
                Ok(()) => JobResult::EchoXrReady,
                Err(e) => job_err(e, failed),
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
        vec![Res::Vr],
        "Preparing SteamVR (Revive)",
        "Downloading Revive...",
        move |cancel, on| {
            // Installed already (by hand, or before): only the artwork and library entry.
            if revive::find_revive_dir().is_none() {
                let installer = match revive::download_installer(cancel, &mut |p| {
                    if let download::Progress::Percent(v) = p {
                        on(Step::Percent(v));
                    }
                }) {
                    Ok(p) => p,
                    Err(e) => return job_err(e, "Couldn't set up SteamVR"),
                };
                on(Step::Status("Installing Revive...".into()));
                if let Err(e) = elevation::run_revive_installer(&installer, &mut consent) {
                    return job_err(
                        e.context("Installing Revive failed"),
                        "Couldn't set up SteamVR",
                    );
                }
            }
            on(Step::Status("Waiting for Revive...".into()));
            if revive::wait_for_revive_dir(std::time::Duration::from_secs(8)).is_none() {
                return JobResult::Failed(Some(UiError::new(
                    "Couldn't set up SteamVR",
                    "Revive does not appear to be installed (was the installer cancelled?).",
                )));
            }
            // The artwork and the library entry are niceties: Revive works without them.
            let mut notes = Vec::new();
            if artwork && !revive::artwork_installed() {
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

/// A desktop shortcut to version `id`: the launcher with `--play`, which starts it as PLAY
/// does (the route, launch options and mods from Settings at each start), without the
/// window.
pub(super) fn shortcut(d: &mut Dashboard, id: &str) {
    let Some(v) = d.state.version(id).cloned() else {
        return;
    };
    match launcher_shortcut(&v) {
        Ok(()) => d.notify("The desktop shortcut is ready: it starts this version as PLAY does"),
        Err(e) => d.dialogs.error(
            "Couldn't create shortcut",
            &format!("{e:#}"),
            Default::default(),
        ),
    }
}

/// Writes `v`'s desktop shortcut (see [`shortcut`]), and takes out the one through Revive
/// that launchers before made beside it.
fn launcher_shortcut(v: &InstalledVersion) -> anyhow::Result<()> {
    use crate::core::launcher::headless;
    let launcher = crate::core::linux::launcher_exe()
        .ok_or_else(|| anyhow::anyhow!("the launcher's own path is unknown"))?;
    let names = crate::core::uninstall::shortcut_names(v);
    let args = launch::join_args(&[
        headless::FLAG.to_string(),
        headless::VERSION_FLAG.to_string(),
        v.id.clone(),
    ]);
    // The plain name is the last (the one through Revive first).
    let name = names.last().cloned().unwrap_or_else(|| "Echo VR".into());
    platform::create_shortcut(
        &name,
        &launcher,
        Some(&args),
        launcher.parent(),
        Some(&v.exe_path()),
    )?;
    for old in names.iter().filter(|n| **n != name) {
        let _ = platform::remove_shortcut(old);
    }
    Ok(())
}

/// Once (Windows): the desktop shortcuts launchers before made, which started the game
/// directly with the launch options of the day, start it through the launcher now.
pub(super) fn shortcuts_through_launcher(d: &mut Dashboard) {
    if !cfg!(windows) || d.demo || d.state.shortcuts_through_launcher {
        return;
    }
    let desktop = dirs::desktop_dir();
    for v in d.state.versions.clone() {
        let made = crate::core::uninstall::shortcut_names(&v).iter().any(|n| {
            desktop
                .as_ref()
                .is_some_and(|dir| dir.join(format!("{n}.lnk")).is_file())
        });
        if made {
            match launcher_shortcut(&v) {
                Ok(()) => tracing::info!("{}: its desktop shortcut starts the launcher now", v.id),
                Err(e) => tracing::warn!("{}: desktop shortcut: {e:#}", v.id),
            }
        }
    }
    d.state.shortcuts_through_launcher = true;
    d.save();
}

/// The ways to play this PC offers. Windows: Meta Link, Virtual Desktop, SteamVR
/// (through Revive or EchoXR) and Flat. Linux: VR through EchoXR on SteamVR or WiVRn (the
/// ones installed; both when neither is), and Flat. Snapshots show Windows'.
pub(super) fn runtimes(d: &Dashboard) -> Vec<Runtime> {
    if cfg!(target_os = "linux") && !d.demo {
        use crate::core::linux::echoxr::{installed, Xr};
        let xr = installed();
        let mut out: Vec<Runtime> = [(Xr::SteamVr, Runtime::Revive), (Xr::Wivrn, Runtime::Wivrn)]
            .into_iter()
            .filter(|(x, _)| xr.is_empty() || xr.contains(x))
            .map(|(_, rt)| rt)
            .collect();
        out.push(Runtime::Flat);
        return out;
    }
    Runtime::ALL
        .into_iter()
        .filter(|rt| *rt != Runtime::Wivrn)
        .filter(|rt| *rt != Runtime::Revive || cfg!(windows) || d.demo)
        .collect()
}

/// Linux: a choice Linux doesn't offer (Meta Link, Linux's "VR" before) becomes SteamVR,
/// or WiVRn when only that is installed. Returns whether it changed.
pub(super) fn linux_runtime(runtime: &mut Runtime) -> bool {
    use crate::core::linux::echoxr::{installed, Xr};
    if !matches!(runtime, Runtime::MetaLink | Runtime::VirtualDesktop) {
        return false;
    }
    *runtime = if installed() == [Xr::Wivrn] {
        Runtime::Wivrn
    } else {
        Runtime::Revive
    };
    true
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

/// Whether preparing Linux would add Echo VR to Steam (and restart Steam for it): its
/// shortcut isn't in Steam, or not with this launcher; or the Flatpak Steam may not hand
/// its start to the launcher yet.
pub(super) fn linux_needs_shortcut() -> bool {
    use crate::core::linux::{self, steam};
    let Some(root) = steam::root() else {
        return true;
    };
    let Some(shortcut) = linux::steam_shortcut(&root) else {
        return true;
    };
    steam::installed_shortcut(&root, &shortcut).is_none()
        || (steam::is_flatpak(&root) && !steam::flatpak_can_spawn())
}

pub(super) const WINDOWS_STEAM_JOB: &str = "windows-steam";

/// Windows: the shortcut Steam's library gets for Echo VR: the launcher with `--play`
/// (in SteamVR's library too, with SteamVR installed).
fn windows_steam_shortcut(
    steam_root: &std::path::Path,
) -> Option<crate::core::linux::steam::Shortcut> {
    let exe = std::env::current_exe().ok()?;
    let steamvr = crate::core::linux::steam::libraries(steam_root)
        .iter()
        .any(|l| l.join("steamapps/common/SteamVR").is_dir());
    Some(crate::core::linux::steam::Shortcut {
        exe,
        launch_options: crate::core::launcher::headless::FLAG.into(),
        icon: None,
        openvr: steamvr,
    })
}

/// Windows: puts Echo VR into Steam's library (`add`) or takes it out, closing Steam
/// meanwhile (it rewrites the file from memory while it runs) and starting it again.
pub(super) fn windows_steam(d: &mut Dashboard, ctx: &egui::Context, add: bool) {
    use crate::core::linux::steam;
    let Some(root) = steam::root() else {
        d.dialogs.error(
            "Steam not found",
            "Install Steam and sign in once, then try again.",
            Default::default(),
        );
        return;
    };
    d.start_job(
        ctx,
        JobKind::Revive,
        WINDOWS_STEAM_JOB,
        vec![Res::Vr],
        if add {
            "Adding Echo VR to Steam"
        } else {
            "Taking Echo VR out of Steam"
        },
        "Steam restarts...",
        move |_cancel, on| {
            let Some(shortcut) = windows_steam_shortcut(&root) else {
                return job_err(
                    anyhow::anyhow!("the launcher's own path is unknown"),
                    "Steam",
                );
            };
            if add {
                if let Some(appid) = steam::installed_shortcut(&root, &shortcut) {
                    return JobResult::WindowsSteam(Some(appid));
                }
            } else if !steam::has_shortcut(&root) {
                return JobResult::WindowsSteam(None);
            }
            on(Step::Status("Steam restarts...".into()));
            let was_running = steam::running(&root);
            let changed = steam::shutdown(&root).and_then(|()| {
                if add {
                    steam::install_shortcut(&root, &shortcut).map(Some)
                } else {
                    steam::remove_shortcut(&root).map(|_| None)
                }
            });
            if was_running || add {
                let _ = steam::start(&root);
            }
            match changed {
                Ok(appid) => JobResult::WindowsSteam(appid),
                Err(e) => job_err(e, "Couldn't change Echo VR's entry in Steam"),
            }
        },
    );
}

/// Windows, at the start: Echo VR's entry in Steam starts this launcher still (it was
/// moved: the box goes off, and the notice's "Fix it" adds it again for this one).
pub(super) fn windows_steam_current(d: &mut Dashboard) {
    use crate::core::linux::steam;
    if !cfg!(windows) || d.demo || d.state.windows_appid.is_none() {
        return;
    }
    let current = steam::root().is_some_and(|root| {
        windows_steam_shortcut(&root)
            .is_some_and(|s| steam::installed_shortcut(&root, &s).is_some())
    });
    if !current {
        tracing::info!(
            "Echo VR's Steam entry doesn't start this launcher: Settings can add it again"
        );
        d.state.windows_appid = None;
        d.notify_with(
            "Echo VR's entry in Steam starts another copy of the launcher",
            "Fix it",
            super::notices::Action::FixSteamEntry,
        );
    }
}

/// Linux: Echo VR in WiVRn's app list as Settings has it (WiVRn installed), pointing at
/// this launcher (also after it moved). At the start and when the box flips.
pub(super) fn wivrn_list(d: &mut Dashboard) {
    use crate::core::linux::{echoxr, wivrn};
    if !cfg!(target_os = "linux") || d.demo {
        return;
    }
    if d.state.wivrn_list && echoxr::installed().contains(&echoxr::Xr::Wivrn) {
        if let Err(e) = wivrn::list() {
            tracing::warn!("WiVRn's list: {e:#}");
        }
    } else if wivrn::listed() {
        wivrn::unlist();
    }
}

/// What PLAY asks before [`linux_setup`] adds Echo VR to Steam: with the Flatpak Steam, also
/// for the permission its shortcut needs.
pub(super) fn linux_setup_question() -> String {
    use crate::core::linux::steam;
    let mut text = String::from(
        "On Linux, Echo VR starts through Steam: it is added to your Steam library once, and Steam closes and restarts for that.",
    );
    if steam::root().is_some_and(|r| steam::is_flatpak(&r)) && !steam::flatpak_can_spawn() {
        text.push_str(
            "\n\nYour Steam is the Flatpak. Its Echo VR has to start the launcher outside Steam's sandbox, so Steam also gets permission to start programs on this PC (org.freedesktop.Flatpak). That loosens Steam's sandbox; the launcher's uninstall takes it back.",
        );
    }
    text.push_str("\n\nThen Echo VR starts.");
    text
}

/// Prepares Echo VR for Linux: a private GE-Proton, EchoXR's OpenXR runtime and Meta's
/// Platform SDK loader, then (closing Steam meanwhile) the shortcut in Steam that starts it,
/// unless it is there already. The Flatpak Steam's shortcut runs a script that hands the
/// start to the launcher outside its sandbox, which Steam needs permission for.
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
        vec![Res::Vr],
        "Preparing Echo VR for Linux",
        "Preparing...",
        move |cancel, on| {
            if let Err(e) = echoxr::setup(&root, cancel, on) {
                return job_err(e, "Couldn't set up Echo VR on Linux");
            }
            if let Err(e) = linux::refresh_play_script(&root) {
                return job_err(e, "Couldn't set up Echo VR on Linux");
            }
            let Some(shortcut) = linux::steam_shortcut(&root) else {
                return job_err(
                    anyhow::anyhow!("the launcher's own path is unknown"),
                    "Couldn't set up Echo VR on Linux",
                );
            };
            // The Flatpak Steam gets its permission while it is closed: it applies from its
            // next start.
            let allow = steam::is_flatpak(&root) && !steam::flatpak_can_spawn();
            // In Steam already (an update of GE-Proton or EchoXR): Steam stays as it is.
            if !allow {
                if let Some(appid) = steam::installed_shortcut(&root, &shortcut) {
                    return JobResult::LinuxReady {
                        appid,
                        flatpak_override: false,
                    };
                }
            }
            on(Step::Status(
                "Adding Echo VR to Steam (Steam restarts)...".into(),
            ));
            let r = steam::shutdown(&root)
                .and_then(|()| if allow { steam::allow_spawn() } else { Ok(()) })
                .and_then(|()| steam::install_shortcut(&root, &shortcut))
                .and_then(|appid| steam::start(&root).map(|()| appid));
            match r {
                Ok(appid) => JobResult::LinuxReady {
                    appid,
                    flatpak_override: allow,
                },
                Err(e) => job_err(e, "Couldn't set up Echo VR on Linux"),
            }
        },
    );
}

/// Puts the Quest release the launcher channel offers onto the headset, or updates it.
pub(super) fn quest_install(d: &mut Dashboard, ctx: &egui::Context) {
    let installed = d.quest_info.as_ref().is_some_and(|i| i.installed);
    d.start_job(
        ctx,
        JobKind::QuestInstall,
        QUEST_JOB,
        vec![Res::Quest],
        if installed {
            "Updating Echo VR on your Quest"
        } else {
            "Installing Echo VR on your Quest"
        },
        "Checking nEVR's release...",
        move |cancel, on| match quest_core::install(cancel, on) {
            Ok(outcome) => JobResult::Quest(outcome),
            Err(e) => job_err(e, "Couldn't install"),
        },
    );
}

/// The launcher's channel has no Quest release (no `# nEVR quest:` line, or `none`).
pub(super) fn quest_not_offered(d: &mut Dashboard) {
    d.dialogs.info(
        "Not on this channel yet",
        &format!(
            "Echo VR for the Quest isn't offered on the {} channel yet. Once it is, it installs from here. Settings > Advanced shows which channel you follow.",
            d.state.launcher_channel.label()
        ),
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
        Some(Overlay::RelayAccount { .. }) => relay_account_card(d, k, ctx),
        Some(Overlay::PackServer { .. }) => pack_server_card(d, k),
        Some(Overlay::JoinLobby { .. }) => super::play::lobby_card(d, k, ctx),
        Some(Overlay::LoginNotice(_)) => super::play::login_card(d, k, ctx),
        Some(Overlay::VrceTokens { .. }) => super::echovrce::tokens_card(d, k, ctx),
        Some(Overlay::StartServer { .. }) => super::servers::start_card(d, k, ctx),
        Some(Overlay::ShareMatch { .. }) => super::servers::share_card(d, k, ctx),
        Some(Overlay::UploadLogs { .. }) => super::settings::upload_card(d, k, ctx),
        Some(Overlay::Credits { .. }) => super::settings::credits_card(d, k),
        Some(Overlay::GoBack { .. }) => super::goback::card(d, k, ctx),
        Some(Overlay::Uninstall { .. }) => super::settings::uninstall_card(d, k),
        Some(Overlay::PluginSettings(_)) => super::mods::settings_card(d, k, ctx),
        Some(Overlay::Advanced) => super::settings::advanced_card(d, k, ctx),
        Some(Overlay::AddUnlisted { .. }) => super::mods::unlisted_card(d, k, ctx),
        Some(Overlay::Alert(_)) => super::alerts::card(d, k),
        None => {}
    });
}

pub(super) fn runtime_note(r: Runtime) -> &'static str {
    match r {
        Runtime::MetaLink => "Quest over Link or Air Link, or a Rift, with the Meta Quest app.",
        Runtime::VirtualDesktop => {
            "Quest over Virtual Desktop: its Oculus mode, SteamVR or VD's own OpenXR."
        }
        // On Linux every headset plays through EchoXR on its OpenXR runtime.
        Runtime::Revive
            if cfg!(target_os = "linux") && crate::core::linux::echoxr::steamvr_in_flatpak() =>
        {
            crate::core::linux::echoxr::STEAMVR_IN_FLATPAK
        }
        Runtime::Revive if cfg!(target_os = "linux") => {
            "Any SteamVR headset, through EchoXR. PLAY starts SteamVR."
        }
        Runtime::Revive => "Any SteamVR headset. The launcher sets it up for you.",
        Runtime::Wivrn => {
            "Quest and other headsets over WiVRn, through EchoXR. Connect in WiVRn's app."
        }
        Runtime::Flat => "No headset: play or spectate in a window on the monitor.",
    }
}

/// A runtime's name on its tile (the Install card's, Settings').
pub(super) fn runtime_label(r: Runtime) -> &'static str {
    match r {
        Runtime::MetaLink => "Meta Link",
        Runtime::VirtualDesktop => "Virtual Desktop",
        Runtime::Revive => "SteamVR",
        Runtime::Wivrn => "WiVRn",
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

/// The action and Cancel at the bottom right of a card; returns (action, cancel) clicks.
/// Escape cancels too.
#[allow(clippy::too_many_arguments)]
fn card_buttons(
    k: &mut Kit,
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
        || k.key(egui::Key::Escape);
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

/// Before an install: how you play and where it goes (PC), or what goes onto the Quest.
/// Prefilled with your last answers.
fn install_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let offered = runtimes(d);
    let library = d.state.library.clone();
    // Already there (its files gone, or another build on the headset): a reinstall, in
    // the version's own folder.
    let installed = match &d.overlay {
        Some(Overlay::Install(ask)) => match &ask.target {
            InstallFor::Pc(e) => d.state.installed_from(&e.id).cloned(),
            InstallFor::Quest => None,
        },
        _ => None,
    };
    let reinstall = installed.is_some()
        || matches!(&d.overlay, Some(Overlay::Install(ask)) if matches!(ask.target, InstallFor::Quest))
            && d.quest_info.as_ref().is_some_and(|i| i.installed);
    // Only a job on this build (or the headset) holds it up.
    let busy = match &d.overlay {
        Some(Overlay::Install(ask)) => match &ask.target {
            InstallFor::Pc(e) => {
                let id = installed.as_ref().map_or(e.id.clone(), |v| v.id.clone());
                let mut res = vec![Res::Version(id)];
                if e.publisher_lock.is_some() {
                    res.push(Res::Relay);
                }
                d.busy_with(&res)
            }
            InstallFor::Quest => d.busy_with(&[Res::Quest]),
        },
        _ => None,
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
    // Offered, not chosen: the location shown (maybe one just picked) stays the answer.
    if let Some(copy) = found {
        ask.copy = Some(copy);
    }
    let use_copy = ask.use_copy && ask.copy.is_some();
    let (title, root) = match &ask.target {
        InstallFor::Pc(e) => (
            format!("{verb} {}", e.name),
            Some(installed.as_ref().map_or_else(
                || crate::core::launcher::versions::root_for(&library, &e.id),
                |v| v.root.clone(),
            )),
        ),
        InstallFor::Quest => (format!("{verb} Echo VR on your Quest"), None),
    };
    let quest = root.is_none();
    // An event build always starts in VR.
    let event = matches!(&ask.target, InstallFor::Pc(e) if e.publisher_lock.is_some());
    let offered: Vec<Runtime> = offered
        .into_iter()
        .filter(|rt| !(event && *rt == Runtime::Flat))
        .collect();
    let (w, pad, gap) = (dz(1100.0), dz(30.0), dz(16.0));
    let inner = w - 2.0 * pad;
    let (q_h, runtime_h, place_h) = (dz(40.0), dz(130.0), dz(50.0));
    // The line naming a copy on this PC, under the install location.
    let found_h = dz(28.0);

    // Measured first: the card fits what it shows.
    let mut h = if event {
        text_height(k, EVENT_NOTE, inner) + dz(24.0)
    } else {
        0.0
    };
    if quest {
        h += text_height(k, QUEST_NOTE, inner) + dz(24.0);
    } else {
        h += q_h + runtime_h + dz(24.0) + place_h + dz(24.0);
        if ask.copy.is_some() && !use_copy && !reinstall {
            h += found_h;
        }
    }
    let h = dz(46.0) + 2.0 * pad + h + BTN_H;
    let (x, mut y, cw, bottom) = card(k, w, h, &title);

    if event {
        y += k.caps_text(x, y, cw, EVENT_NOTE, 17.0, design::BODY, dz(PARA)) + dz(24.0);
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
        let (caption, path) = match &ask.copy {
            Some(copy) if use_copy => ("Echo VR is in", copy.as_str()),
            _ if reinstall => ("Its folder", root.as_str()),
            _ => ("Install location", root.as_str()),
        };
        let cap = k.caption(x, y, caption);
        // A reinstall stays in the version's folder: nothing to change.
        if !use_copy && !reinstall {
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
        // One choice on this line: the copy found on this PC, or downloading (the
        // location's own); without a copy found, an echovr.exe of your own.
        if !reinstall {
            let (text, tip) = match (&ask.copy, use_copy) {
                (Some(_), true) => (
                    "Download it instead",
                    "Download Echo VR into your library instead of using this copy",
                ),
                (Some(_), false) => (
                    "Use the copy on this PC",
                    "Echo VR is already on this PC: add it instead of downloading it again",
                ),
                (None, _) => (
                    "Already have it? Choose echovr.exe",
                    "Use an Echo VR that is already on this PC: choose its echovr.exe",
                ),
            };
            let lw = k.link_width(text, 14.0);
            if k.link("install-choose", x + cw - lw, y - dz(2.0), text, 14.0, tip)
                .clicked
            {
                match &ask.copy {
                    Some(_) => {
                        ask.use_copy = !use_copy;
                        ask.copy_decided = true;
                    }
                    None => choose = true,
                }
            }
        }
        let path = super::install::myriad(k, path, design::myriad(18.0), design::TEXT, cw, true);
        k.put(x, y + dz(24.0), path);
        if let (Some(copy), false, false) = (&ask.copy, use_copy, reinstall) {
            let line = format!("Echo VR is already on this PC: {copy}");
            let g = super::install::myriad(k, &line, design::myriad(15.0), design::GREY, cw, true);
            k.put(x, y + dz(50.0), g);
        }
    } else {
        k.caps_text(x, y, cw, QUEST_NOTE, 17.0, design::BODY, dz(PARA));
    }

    let tip = match () {
        _ if quest => "Download nEVR's Echo VR and install it on your Quest",
        _ if event && use_copy => {
            "Check this copy against the event build's checksums, add it and EchoRelay's patch"
        }
        _ if event && reinstall => "Check every game file and fetch only the broken ones again",
        _ if event => {
            "Download this event build and add EchoRelay's patch, for the classic lobbies"
        }
        _ if use_copy => {
            "Check this copy against the build's checksums and add it to your library: only broken files are downloaded"
        }
        _ if reinstall => {
            "Check every game file against the server's checksums and fetch only the broken ones again"
        }
        _ => "Download and install Echo VR",
    };
    // Off: what it waits for.
    let tip = busy.as_deref().unwrap_or(tip);
    let (go, cancel) = card_buttons(
        k,
        "install",
        x + cw,
        bottom - BTN_H,
        if use_copy { "Use this copy" } else { verb },
        busy.is_none(),
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
    // Off: what's missing.
    let tip = match (account.name.is_empty(), account.password.is_empty()) {
        _ if ready => tip,
        (true, _) => "Type a display name first",
        (false, true) => "Type a password first",
        (false, false) => "The name or password is too long",
    };
    let play = *play;
    let (go, cancel) = card_buttons(k, "relay", x + cw, bottom - BTN_H, label, ready, tip);
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

/// A content pack's own server (Settings): its address and key, and your account there.
/// Saved with an empty address, the pack plays on EchoVRCE.
fn pack_server_card(d: &mut Dashboard, k: &mut Kit) {
    let Some(Overlay::PackServer { pack, name, server }) = &mut d.overlay else {
        return;
    };
    let text = PACK_SERVER_NOTE.replace("{name}", name);
    let (w, pad, gap) = (dz(1000.0), dz(30.0), dz(20.0));
    let row = dz(30.0) + BTN_H + dz(22.0);
    let h = dz(46.0)
        + 2.0 * pad
        + text_height(k, &text, w - 2.0 * pad)
        + dz(24.0)
        + 2.0 * row
        + dz(10.0)
        + BTN_H;
    let (x, mut y, cw, bottom) = card(k, w, h, &format!("{name} server"));
    y += k.caps_text(x, y, cw, &text, 17.0, design::BODY, dz(PARA)) + dz(24.0);
    let fw = (cw - gap) / 2.0;
    // The address and its key; then your account there.
    let typed = server.address.trim().to_string();
    let bad_address =
        !typed.is_empty() && crate::core::launcher::nevr::socket_uri(&typed, "").is_none();
    k.caption(x, y, "Address");
    k.caption(x + fw + gap, y, "Server key");
    k.field(
        "pack-server-address",
        &mut server.address,
        x,
        y + dz(30.0),
        fw,
        BTN_H,
        "Empty: EchoVRCE",
        bad_address,
        "What the server's host gives you: host:port, or a ws:// or wss:// address",
    );
    k.secret_field(
        "pack-server-key",
        &mut server.server_key,
        x + fw + gap,
        y + dz(30.0),
        fw,
        BTN_H,
        "From the server's host",
        false,
        "The server's key, unless its address carries it (token=…)",
    );
    y += row;
    let bad_id = !server.discord_id.trim().chars().all(|c| c.is_ascii_digit());
    k.caption(x, y, "Discord ID");
    k.caption(x + fw + gap, y, "Password");
    k.field(
        "pack-server-id",
        &mut server.discord_id,
        x,
        y + dz(30.0),
        fw,
        BTN_H,
        "Your account's Discord ID",
        bad_id,
        "Digits only: Discord's Developer Mode → right-click your name → Copy User ID",
    );
    k.secret_field(
        "pack-server-password",
        &mut server.password,
        x + fw + gap,
        y + dz(30.0),
        fw,
        BTN_H,
        "Your password there",
        false,
        "Not one you use anywhere else: it goes into the game's config in plain text",
    );
    let on_echovrce = typed.is_empty();
    let account = !server.discord_id.trim().is_empty() && !server.password.is_empty();
    let ready = on_echovrce || (!bad_address && !bad_id && account);
    let tip = if on_echovrce {
        "Play on EchoVRCE while it's on, as without it"
    } else if bad_address {
        "That isn't an address: host:port, or ws://… / wss://…"
    } else if bad_id {
        "A Discord ID is digits only"
    } else if !account {
        "Your Discord ID and password there first"
    } else {
        "Play on this server while it's on"
    };
    let (go, cancel) = card_buttons(k, "pack-server", x + cw, bottom - BTN_H, "Save", ready, tip);
    if cancel {
        d.overlay = None;
    } else if go {
        let (pack, name) = (pack.clone(), name.clone());
        let mut s = server.clone();
        s.address = typed;
        s.server_key = s.server_key.trim().to_string();
        s.discord_id = s.discord_id.trim().to_string();
        d.overlay = None;
        if s.address.is_empty() {
            d.state.pack_servers.remove(&pack);
            d.notify(&format!("{name} plays on EchoVRCE"));
        } else {
            d.notify(&format!(
                "{name} plays on {} from its next start",
                s.address
            ));
            d.state.pack_servers.insert(pack, s);
        }
        d.save();
    }
}

const PACK_SERVER_NOTE: &str = "While {name} is on, Echo VR plays on this server instead of EchoVRCE: you sign in, find matches and play there, with the others who have {name}. Its host gives you the address and its key. Leave the address empty to play on EchoVRCE.\n\nYour account there is a Discord ID and a password. The password goes into the game's config in plain text: don't reuse a real one.";
const RELAY_NOTE: &str = "Event builds play on the community's classic lobbies server ({server}). Pick a display name and a password: your first login locks the account to that password, and it works in every event build.\n\nDon't reuse a real password: it is sent without encryption.";
const EVENT_NOTE: &str = "An event build: it plays on the community's classic lobbies server, with EchoRelay's patch instead of the licence check. You pick your account there when you first play it.";
/// The Install card's text for the Quest.
const QUEST_NOTE: &str = "Echo VR on your Quest is nEVR's build, the same for every player, checked against nEVR's signed release. It goes on over USB or Wi-Fi, with the game data when the headset doesn't have it yet.\n\nAnother Echo VR build on the headset is updated to nEVR's: your in-game settings stay.";
