//! The launcher (root window): the design's icon rail, the status bar and pages over the
//! purple backdrop. See `ui/design.rs` and `ui/style.rs` for the widgets. Installing,
//! patching, SteamVR setup and the Quest all run inline (`setup.rs`).

mod echovrce;
mod friends;
mod hero;
mod install;
mod install_panel;
mod mods;
mod now;
mod panel;
mod play;
mod plugin_form;
mod plugins;
mod servers;
mod settings;
mod setup;
mod versions;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::design::{self, dz, Dr, RailIcon};
use super::dialogs::DialogHost;
use super::kit::Kit;
use super::parts::{QuestConn, Worker};
use super::style::{self, Icon};
use crate::core::adb::devices::Status;
use crate::core::error::UiError;

use crate::core::launcher::catalog::{Catalog, Platform, VersionEntry};
use crate::core::launcher::feed;
use crate::core::launcher::game::{self, GameState, Local, Monitor};
use crate::core::launcher::quest::QuestInfo;
use crate::core::launcher::store::{InstalledVersion, LauncherState, Runtime, SteamVrVia, Target};
use crate::core::launcher::update_check;
use crate::core::launcher::versions::Step;
use crate::core::links;

/// Settings → Credits: who made what the launcher is and brings along.
pub struct Credit {
    pub name: &'static str,
    pub by: &'static str,
    pub what: &'static str,
    pub licence: &'static str,
    pub url: &'static str,
    /// Its own credits file, everyone and everything it builds on (or "").
    pub credits: &'static str,
}

pub const CREDITS: &[Credit] = &[
    Credit {
        name: "Echo VR Launcher",
        by: "marshmallow-mia (development) and sickmave (design)",
        what: "Installs, updates and starts Echo VR, its mods and VR on Windows and Linux.",
        licence: "GPL-3.0",
        url: env!("CARGO_PKG_REPOSITORY"),
        credits: "",
    },
    Credit {
        name: "EchoXR",
        by: crate::core::echoxr::AUTHORS,
        what: "Echo VR on OpenXR: SteamVR without Revive, and VR on Linux.",
        licence: "MIT (Revive), Apache-2.0 (OpenXR SDK)",
        url: "https://github.com/marshmallow-mia/EchoXR",
        credits: "https://github.com/marshmallow-mia/EchoXR/blob/main/CREDITS.md",
    },
    Credit {
        name: "EchoXR Hands",
        by: crate::core::echoxr_hands::AUTHORS,
        what: "Your own fingers on Echo VR's hands, from OpenXR hand tracking.",
        licence: "",
        url: "https://github.com/EchoTools/EchoXR-Hands",
        credits: "",
    },
    Credit {
        name: "nEVR runtime",
        by: "EchoTools",
        what: "The mod loader in Echo VR: plugins, Discord sign-in, friends and parties.",
        licence: "Apache-2.0",
        url: "https://github.com/EchoTools/nevr-runtime",
        credits: "",
    },
    Credit {
        name: "EchoVRCE",
        by: "the EchoVRCE team",
        what: "The community servers, accounts and matchmaking Echo VR plays on.",
        licence: "",
        url: "https://echovrce.com",
        credits: "",
    },
    Credit {
        name: "Revive",
        by: "CrossVR (Jules Blok) and contributors",
        what: "Runs Oculus games on SteamVR; EchoXR's OpenXR side is built on it.",
        licence: "MIT",
        url: "https://github.com/LibreVR/Revive",
        credits: "",
    },
];

/// Under the credits.
pub const CREDITS_NOTE: &str = "Echo VR is by Ready at Dawn and Meta, who have nothing to do with this launcher. Problems? Ask on Discord: marshmallow_mia.";

pub const W: f32 = 1280.0;
pub const H: f32 = 720.0;
/// Width of the left navigation rail.
const RAIL: f32 = dz(RAIL_W);
/// The rail's width, and how much wider it gets unfolded (design pixels).
const RAIL_W: f32 = 91.0;
const RAIL_EXTRA: f32 = 150.0;
/// How long the rail takes to unfold or fold (seconds).
const RAIL_SLIDE: f32 = 0.22;
/// Left edge and width of page content.
const X0: f32 = dz(138.0);
const CW: f32 = W - X0 - dz(48.0);
/// The page's header strip under the status bar (design pixels).
pub(super) const HEADER: Dr = Dr::new(138.0, 80.0, 1734.0, 46.0);
/// Echo VR on Quest. The first release is PCVR only: the switch's Quest side says it's
/// coming soon, and nothing looks for a headset.
pub(super) const QUEST: bool = false;
/// How long the switch's "coming soon" stays up.
const QUEST_SOON_FOR: std::time::Duration = std::time::Duration::from_millis(2500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Page {
    #[default]
    Play,
    /// Installing and managing versions (the rail's download icon).
    Install,
    Mods,
    Servers,
    EchoVrce,
    Friends,
    /// Launcher plugins: getting, updating and removing them (the rail's +).
    Plugins,
    /// An installed launcher plugin's tab (its index in the installed list).
    Plugin(u8),
    Settings,
}

impl Page {
    pub const ALL: [Page; 8] = [
        Page::Play,
        Page::Install,
        Page::Settings,
        Page::Servers,
        Page::Mods,
        Page::EchoVrce,
        Page::Friends,
        Page::Plugins,
    ];

    fn title(self) -> &'static str {
        match self {
            Page::Play => "Play",
            Page::Install => "Install",
            Page::Mods => "Mods",
            Page::Servers => "Servers",
            Page::EchoVrce => "EchoVRCE",
            Page::Friends => "Friends",
            Page::Plugins => "Plugins",
            Page::Plugin(_) => "Plugin",
            Page::Settings => "Settings",
        }
    }
}

enum JobResult {
    /// Installed; with why its update failed when it did (installed, not up to date).
    Installed(InstalledVersion, Option<String>),
    /// Reinstalled: checked, the broken files fetched again.
    Reinstalled(crate::core::launcher::versions::Reinstalled),
    Updated,
    Verified(Vec<String>),
    /// `None` = cancelled.
    Failed(Option<UiError>),
    /// The licence patch was applied.
    Patched,
    /// The licence patch was taken off again.
    Unpatched,
    /// A new player's patch is in the download folder, waiting for its version's install.
    LicenceFetched(PathBuf),
    /// Discord authorization for the patch failed.
    OAuthFailed(crate::core::oauth::OAuthError),
    /// Revive (SteamVR) is installed; with what of the rest (artwork, library entry)
    /// couldn't be done.
    ReviveReady(Vec<String>),
    /// The game artwork for SteamVR's library is in place.
    ArtworkInstalled,
    /// Echo VR was put into SteamVR's library (`true`) or taken out.
    LibraryEntry(bool),
    /// Linux: set up, with the Steam shortcut's appid.
    LinuxReady(u32),
    /// SteamVR through EchoXR is set up (Windows).
    EchoXrReady,
    /// An event build is set up for the classic lobbies (EchoLoader, EchoRelay's patch).
    EventBuildReady,
    QuestInstalled(crate::core::launcher::quest::Installed),
    QuestUpdated,
    /// The headset's APK doesn't match the update: offer a reinstall (the text says why).
    QuestNeedsReinstall(String),
    /// Mods were installed, added or removed: what to say.
    ModsChanged(String),
    /// EchoXR Hands is installed: it's on now (and EchoXR, which it needs, on Windows).
    HandsInstalled,
    /// Parts of the launcher's things were uninstalled.
    Uninstalled(crate::core::uninstall::Outcome),
    /// Version (its id)'s folder is deleted: it leaves the library.
    Removed(String),
    /// The launcher's new version is in place: start it (the executable) and end this one.
    LauncherUpdated(PathBuf),
}

enum Msg {
    Catalog(Catalog),
    JobStep(String, Step),
    JobDone(String, JobResult),
    QuestInfo(Result<QuestInfo, UiError>),
    QuestAction(Result<(), UiError>),
    CacheDeleted(Vec<PathBuf>),
    /// A job needs administrator rights: ask, then answer on the channel.
    Consent(std::sync::mpsc::SyncSender<bool>),
    /// A version's mod loader went or is blocked right after a job put it in place (its
    /// id, what happened).
    SlotGone(String, String),
    FeedStatus(Option<feed::Servers>),
    FeedNews(Option<feed::News>),
    /// A feed image by file name (`None`: it couldn't be loaded).
    FeedImage(String, Option<image::RgbaImage>),
    /// Free bytes in a library folder.
    FreeSpace(String, Option<u64>),
    /// Where Revive is installed.
    Revive(Option<String>),
    /// What the dev code's folder holds (Advanced settings).
    DevCode(crate::core::launcher::dev_code::Status),
    /// EchoXR (and what it needs) is fetched.
    EchoXr(bool),
    /// Echo VR copies on this PC the library doesn't have yet (for the library and the
    /// number of versions it had).
    Found((String, usize), Vec<String>),
    /// Bytes "Delete cache" would free.
    CacheSize(u64),
    /// How far a scan for the Quest on the network is (0 to 1).
    QuestScanProgress(f32),
    /// What it found.
    QuestScanDone(Vec<crate::core::launcher::quest_net::Found>),
    /// ADB over the network was turned on, or why not.
    QuestNetwork(Result<(), UiError>),
    /// The launcher's latest release, if newer (`Err`: couldn't look).
    /// A launcher update check's answer, for that channel.
    LauncherUpdate(
        update_check::Channel,
        Result<Option<update_check::Release>, String>,
    ),
    /// An update check is done.
    Updates(crate::core::updates::Check),
    /// Who opens spark:// links now (`Err`: registering failed, and why).
    LinkHandler(links::Handler, Option<String>),
    /// The headset's logs were saved into this folder, or why not.
    QuestLogs(Result<PathBuf, UiError>),
    /// The logs were uploaded: the service's reference, or why not.
    LogsUploaded(Result<String, String>),
    /// Linux: what PLAY's start through Steam waits for.
    LaunchNote(String),
    /// Linux: Steam has the link (true), the start was cancelled (false), or it failed.
    LinuxStarted {
        result: Result<bool, String>,
        version: String,
        bin: PathBuf,
        root: String,
    },
    /// A version's mods, read (for the Mods page's read number `gen`).
    ModView(u64, String, crate::core::launcher::mods::ModView),
    ModCatalog(crate::core::launcher::mods::ModCatalog),
}

/// Whether a newer launcher is out (Settings shows it, the rail marks it).
/// What the update checks found (`core::updates`).
#[derive(Default)]
struct UpdatesFound {
    findings: Vec<crate::core::updates::Finding>,
    /// When the last check was done.
    checked: Option<std::time::Instant>,
    checking: bool,
    /// What the status bar has said this run (each finding once).
    shown: std::collections::BTreeSet<String>,
}

impl UpdatesFound {
    /// Look again right away (something was installed or updated).
    fn check_soon(&mut self) {
        self.checked = None;
    }

    /// An update of installed version `id` is ready.
    fn game(&self, id: &str) -> bool {
        self.findings
            .iter()
            .any(|f| matches!(f, crate::core::updates::Finding::Game { id: g, .. } if g == id))
    }

    /// How many plugin updates there are.
    fn plugins(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| matches!(f, crate::core::updates::Finding::Plugin { .. }))
            .count()
    }

    /// How many plugin updates installed version `id` has.
    fn plugins_for(&self, id: &str) -> usize {
        self.findings
            .iter()
            .filter(|f| {
                matches!(f, crate::core::updates::Finding::Plugin { version_id, .. } if version_id == id)
            })
            .count()
    }
}

/// What Play's update button does after the game's update, one job after the other.
#[derive(Debug, Clone, PartialEq, Eq)]
enum QueuedUpdate {
    /// The catalogue plugins of installed version `id` that have a newer version.
    Plugins(String),
    /// The launcher (last: it restarts).
    Launcher,
}

#[derive(Default)]
enum LauncherUpdate {
    #[default]
    Checking,
    Latest,
    Available(update_check::Release),
    Failed,
}

/// A value measured on a worker thread because measuring can block (disks, the
/// registry): the pages read the last result, and ask again once it is old.
struct Probe<K, V> {
    /// What was measured for which key, and when.
    last: Option<(K, V, std::time::Instant)>,
    /// The key a page asked about since the last `due`.
    wanted: Option<K>,
    pending: bool,
}

impl<K, V> Default for Probe<K, V> {
    fn default() -> Self {
        Probe {
            last: None,
            wanted: None,
            pending: false,
        }
    }
}

impl<K: PartialEq + Clone, V: Clone> Probe<K, V> {
    /// The last result for `key` (`None`: not measured yet).
    fn get(&mut self, key: K) -> Option<V> {
        let v = self
            .last
            .as_ref()
            .filter(|(k, ..)| *k == key)
            .map(|(_, v, _)| v.clone());
        self.wanted = Some(key);
        v
    }

    /// The key to measure now: asked about, not measuring, and not measured within `every`.
    fn due(&mut self, every: std::time::Duration) -> Option<K> {
        let key = self.wanted.take()?;
        let fresh = self
            .last
            .as_ref()
            .is_some_and(|(k, _, at)| *k == key && at.elapsed() < every);
        if self.pending || fresh {
            return None;
        }
        self.pending = true;
        Some(key)
    }

    fn done(&mut self, key: K, v: V) {
        self.pending = false;
        self.last = Some((key, v, std::time::Instant::now()));
    }
}

/// The Play page's feed (RIGHT NOW's numbers and Community News) and its images.
#[derive(Default)]
struct Feed {
    status: Option<feed::Servers>,
    news: Option<feed::News>,
    /// A fetch failed and there is nothing to show.
    status_failed: bool,
    news_failed: bool,
    status_at: Option<std::time::Instant>,
    news_at: Option<std::time::Instant>,
    status_loading: bool,
    news_loading: bool,
    textures: HashMap<String, egui::TextureHandle>,
    /// Images being downloaded, or that failed (retried when the data changes).
    images_pending: std::collections::HashSet<String>,
}

impl Feed {
    const STATUS_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
    const NEWS_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

    /// The image file names the news refers to.
    fn wanted(&self) -> Vec<String> {
        self.news
            .iter()
            .flat_map(|n| [&n.slots.main, &n.slots.community])
            .flatten()
            .filter_map(|i| i.image.clone())
            .collect()
    }

    /// Status and news have arrived, with every image they refer to.
    #[cfg(test)]
    fn ready(&self) -> bool {
        self.status.is_some()
            && self.news.is_some()
            && self.wanted().iter().all(|n| self.textures.contains_key(n))
    }

    /// The texture of a feed image, once downloaded.
    fn texture(&self, name: Option<&str>) -> Option<&egui::TextureHandle> {
        self.textures.get(name?)
    }
}

/// What a job does: its progress wording, whether it can be cancelled, and which
/// failures count as a failed update.
/// The launcher's own update, as a job.
pub(super) const LAUNCHER_JOB: &str = "launcher-update";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Install,
    /// Checking an installed version's files and fetching the broken ones again.
    Reinstall,
    Update,
    Verify,
    Patch,
    Unpatch,
    /// Fetching a new player's patch while their version installs.
    Licence,
    Revive,
    QuestInstall,
    QuestUpdate,
    /// Installing, adding or removing a plugin.
    Mods,
    /// Taking what the launcher put on this PC off it again.
    Uninstall,
    /// Deleting an installed version's folder.
    Remove,
    /// Updating the launcher itself.
    LauncherUpdate,
}

/// What a job changes: an action waits only for the jobs on the same thing, so a game
/// download doesn't hold up playing or modding another version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Res {
    /// An installed version's folder (its id).
    Version(String),
    /// The VR setup: Revive, EchoXR, SteamVR's library, Linux's Steam and GE-Proton.
    Vr,
    /// EchoRelay's files for event builds (EchoLoader and its patch, downloaded once).
    Relay,
    /// The headset.
    Quest,
    /// The mods' shared download folder.
    Mods,
    /// A new player's licence patch on its way.
    Licence,
    /// Everything: the launcher's own update (it restarts at the end) and the uninstall.
    All,
}

/// Whether jobs on `a` and on `b` would get in each other's way.
fn clash(a: &[Res], b: &[Res]) -> bool {
    a.contains(&Res::All) || b.contains(&Res::All) || a.iter().any(|r| b.contains(r))
}

/// Why something waits for `job`, as a disabled control's tip.
fn busy_tip(job: &Job) -> String {
    format!("Busy: {}. Wait until it's done.", job.title)
}

struct Job {
    kind: JobKind,
    /// What it changes.
    res: Vec<Res>,
    /// CANCEL can stop it (an administrator step or a deletion can't be).
    cancellable: bool,
    /// What runs, for the status bar ("Installing Echo VR (PC, latest)").
    title: String,
    /// The latest progress line.
    label: String,
    fraction: Option<f32>,
    cancel: Arc<AtomicBool>,
}

/// Snapshot mode: extra states to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapVariant {
    /// Nothing installed yet.
    Fresh,
    /// ...and the recommended version is being installed (downloading).
    Installing,
    /// ...and extracted (no percentage).
    Extracting,
    /// A success message in the status bar.
    Notice,
    /// An error with a help link.
    DialogError,
    /// An install error with a path.
    DialogInstallError,
    /// Removing a version (a red button).
    DialogConfirm,
    /// Discord's authorization page opened: continue in the browser.
    DialogBrowser,
    /// A version's Manage menu, open.
    MenuOpen,
    /// The Install card's questions, as an owner (prefilled).
    InstallAsk,
    /// ...as a new player, with a patch link.
    InstallAskNew,
    /// ...with Echo VR found in the Meta library.
    InstallAskFound,
    /// ...not installed, nothing found (choose echovr.exe).
    InstallAskChoose,
    /// ...for the Quest, as a new player.
    InstallAskQuest,
    /// PLAY of a version not installed here: the licence question first.
    Owner,
    /// Installed; the new player's patch is still on its way.
    LicenceWaiting,
    /// An event build installed and selected, an account on the relay.
    EventSelected,
    /// The Install card of an event build.
    InstallAskEvent,
    /// The classic lobbies account card, before an event build's first PLAY.
    RelayAccount,
    /// Settings: "Delete cache" asks.
    DeleteCache,
    /// Settings: "Upload logs" says what the logs contain.
    UploadLogs,
    /// ...and they were sent: the reference.
    LogsSent,
    /// Settings: the credits.
    Credits,
    /// Settings: Advanced settings, on beta, with a beta out.
    AdvancedSettings,
    /// Play while the launcher follows beta: the chip in the status bar.
    BetaChannel,
    /// The rail unfolded.
    RailOpen,
    /// Updates found: the rail's dots, Play's "Update ready".
    UpdatesFound,
    /// Settings: what to uninstall.
    Uninstall,
    /// Mods: local plugins off (an unverified plugin left out, Add DLL locked).
    ModsLocked,
    /// The Quest side, with Echo VR installed on the headset.
    QuestSide,
    /// The Quest side, a headset without Echo VR.
    QuestFresh,
    /// The Quest side, Echo VR running on the headset (seen through its API).
    QuestRunning,
    /// PLAY was clicked; the game isn't up yet.
    Launching,
    /// Echo VR runs, started elsewhere (RUNNING).
    Running,
    /// Echo VR runs, started by the launcher (STOP).
    RunningOurs,
    /// A dedicated server runs on this PC; no game.
    ServerHere,
    /// The Play page's version picker, open.
    VersionMenu,
    /// A placeholder version chosen on the Install page.
    Placeholder,
    /// The "Join a lobby" card with a pasted link.
    JoinLobby,
    /// Settings with SteamVR chosen (its artwork and library options).
    SettingsSteamVr,
    /// ...through EchoXR.
    SettingsEchoXr,
    /// Settings with Virtual Desktop chosen, through SteamVR (EchoXR): its routes.
    SettingsVirtualDesktop,
    /// Play: Virtual Desktop's Oculus mode couldn't start VR, EchoXR instead?
    DialogVdSwitch,
    /// Play: Windows blocked the mod loader, allow it?
    DialogNevrBlocked,
    /// Servers: signed in, the live list (and you in a party queueing).
    ServersLive,
    /// Servers: your match history.
    ServersHistory,
    /// Servers: starting a server.
    ServersStart,
    /// Servers: you in a match, an invite to you, a friend invited.
    ServersInvites,
    /// Play signed in: RIGHT NOW with friends and an invite.
    PlayFriends,
    /// Friends signed in: a search, requests, friends and the players of your matches.
    FriendsPage,
    /// EchoVRCE asks to confirm this location: the code card.
    LoginCode,
    /// EchoVRCE turned the login down with a message.
    LoginMessage,
    /// Servers: the card of a match you just started.
    ServersShare,
    /// Settings with Spark opening spark:// links.
    SettingsLinks,
    /// EchoVRCE: signed in.
    VrceSignedIn,
    /// EchoVRCE: waiting for the sign-in code to be approved.
    VrceSigning,
    /// EchoVRCE: the code being approved in the window.
    VrceSigningHere,
    /// A new player: PATCH on Play.
    NewPlayer,
    /// The licence card for the selected version.
    Licence,
    /// ...with a patch link pasted.
    LicenceLink,
    /// Mods: the version has no mod loader yet.
    ModsNoLoader,
    /// Mods: a plugin's options open.
    ModsOptions,
    /// Mods: the loader's longest text (a plugin skipped, your own config.json).
    ModsLongText,
    /// EchoVRCE isn't answering (502): signed in, the session kept.
    VrceDown,
    /// ...not signed in, after a sign-in that failed on it.
    VrceSignInFailed,
    /// Mods: EchoXR Hands installed, its settings on its row.
    ModsHands,
    /// Mods: EchoXR Hands' settings open.
    ModsSettings,
    /// Mods: a made-up plugin's settings with every kind of control, advanced shown.
    ModsSettingsAll,
    /// Another build downloads while the installed one stays chosen (its menu open: the
    /// `…Menu` one).
    InstallingAnother,
    InstallingAnotherMenu,
    /// Mods while the chosen build is still downloading.
    ModsInstalling,
    /// Settings with EchoCombat installed and on its own server (and that server's card).
    SettingsPackServer,
    PackServerCard,
    /// The Game server plugin installed (its tab second): filled in, asking to play there,
    /// and playing there.
    PluginGameServer,
    PluginGameServerAsk,
    PluginGameServerSet,
}

/// Snapshots: the game as the monitor would see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapGame {
    Launching,
    Ours,
    Elsewhere,
    /// A dedicated server, from another folder.
    Server,
    /// Echo VR runs on the Quest (its API answers over the network).
    QuestRunning,
}

/// PLAY was clicked: the game is starting until the monitor finds it (it knows the game
/// for the launcher's own from then on) or it never shows up.
#[derive(Clone, Copy)]
struct Launched {
    at: std::time::Instant,
    /// The game has been seen running since.
    seen: bool,
}

impl Launched {
    fn now() -> Launched {
        Launched {
            at: std::time::Instant::now(),
            seen: false,
        }
    }
}

#[derive(Default)]
pub struct Dashboard {
    pub page: Page,
    state: LauncherState,
    catalog: Option<Catalog>,
    catalog_loading: bool,
    jobs: HashMap<String, Job>,
    pub dialogs: DialogHost,
    worker: Worker<Msg>,
    monitor: Option<Monitor>,
    child: Option<std::process::Child>,
    /// That is EchoXR.exe, whose exit codes say why the game didn't start.
    child_echoxr: bool,
    /// The game PLAY started, until it has ended (or never showed up).
    launched: Option<Launched>,
    /// Linux: PLAY's start through Steam, until Steam has the link (STOP sets it).
    launch_cancel: Option<Arc<AtomicBool>>,
    /// What that start waits for ("Starting Steam"...), for the info line.
    launch_note: Option<String>,
    /// The game's log of the start PLAY made, for what EchoVRCE says to a login.
    login_watch: Option<play::LoginWatching>,
    /// The lobby to join once "Launch anyway" is answered.
    pending_lobby: Option<play::Join>,
    /// The version whose mod loader the "Windows blocked" question is about.
    nevr_blocked: Option<String>,
    /// PC or Quest, on the Play and the Install page alike.
    platform: Platform,
    quest_conn: QuestConn,
    quest_info: Option<QuestInfo>,
    quest_busy: bool,
    /// The headset's Echo VR was a patched build when last read (its marker says so).
    quest_was_patched: bool,
    /// The Install page's list of installed versions.
    library_scroll: f32,
    /// The version the Install page's hero shows (a row of its version list).
    install_pick: Option<String>,
    /// The Install page's version list.
    versions_scroll: f32,
    /// Snapshots: the game's state instead of the monitor's.
    snap_game: Option<SnapGame>,
    library_field: String,
    /// The Quest's address as typed on the YOUR QUEST card.
    quest_ip_field: String,
    /// Settings' classic lobbies server, as typed.
    relay_server_field: String,
    /// Advanced settings' dev code, as typed.
    dev_code_field: String,
    /// What the dev code's folder held when last read.
    dev_status: crate::core::launcher::dev_code::Status,
    /// The EchoVRCE session (its page).
    vrce: echovrce::Vrce,
    /// The game's own EchoVRCE sign-in (nEVR).
    game_sign_in: echovrce::GameSignIn,
    /// Linux: GE-Proton and EchoXR are in place (checked at start).
    linux_set_up: bool,
    /// PLAY started a preparation (Revive, EchoXR, Linux): it starts the game when that's done.
    play_after_prep: bool,
    /// Linux: when `--play`'s word on a failed start was last looked for.
    linux_outcome_at: Option<std::time::Instant>,
    /// The job whose "continue in your browser" dialog is up (Discord's authorization).
    browser_job: Option<String>,
    /// SteamVR's library entry being added (`true`) or taken out: Settings' box goes back
    /// if that fails.
    library_wanted: Option<bool>,
    /// A spark:// link that came while another card was open: it opens once that closes.
    held_link: Option<String>,
    /// The content packs installed in each live version (its id, the pack), and when
    /// that was read: Settings shows their servers, Play where a start goes.
    packs_seen: Option<(
        std::time::Instant,
        Vec<(String, crate::core::launcher::packs::PackRecord)>,
    )>,
    /// Versions being installed for the first time (they were selected when it started),
    /// with what was selected before: back to it if the install fails or is cancelled.
    select_back: HashMap<String, Option<String>>,
    /// echovrce.com inside the window, on the EchoVRCE page.
    web: crate::ui::web::WebPane,
    /// The game service's servers, party, friends and history (the Servers page).
    servers: servers::Servers,
    /// The selected version's mods and the mods catalogue (the Mods page).
    mods: mods::Mods,
    /// The launcher plugins: their tabs and the Plugins page.
    plugins: plugins::PluginsUi,
    /// Who opens spark:// links (Windows, Linux), once read.
    link_handler: Option<links::Handler>,
    /// When a link handed over by another launcher was last looked for.
    link_checked: Option<std::time::Instant>,
    /// A scan for the Quest on the network is running: how far.
    quest_scan: Option<f32>,
    /// ADB over the network was set up over USB in this session (or tried).
    quest_net_tried: bool,
    pending_remove: Option<String>,
    pending_repair: Option<String>,
    /// Last update result per version, shown on the Play page's Updates card.
    update_note: HashMap<String, String>,
    /// Snapshot mode: made-up state, never saved.
    pub demo: bool,
    /// Snapshots: fetch the real feed instead of the made-up one.
    pub feed_live: bool,
    pub snap_variant: Option<SnapVariant>,
    /// The variant applied last (`Some(None)`: the plain page).
    applied_variant: Option<Option<SnapVariant>>,
    /// When the game was first seen running.
    game_since: Option<std::time::Instant>,
    /// The Quest was checked quietly once already.
    quest_auto_checked: bool,
    /// A full-window card on top (the install questions, the licence patch, joining…).
    overlay: Option<setup::Overlay>,
    /// A job waiting for the administrator-rights answer.
    consent: Option<std::sync::mpsc::SyncSender<bool>>,
    /// Where Revive is installed.
    revive: Probe<(), Option<String>>,
    /// EchoXR and Meta's loader are fetched (SteamVR through EchoXR).
    echoxr: Probe<(), bool>,
    /// Free space in the library.
    free: Probe<String, Option<u64>>,
    /// What "Delete cache" would free.
    cache: Probe<(), u64>,
    /// Echo VR copies on this PC the library doesn't have yet.
    found: Probe<(String, usize), Vec<String>>,
    /// Snapshots: the copies found.
    snap_found: Vec<String>,
    launcher_update: LauncherUpdate,
    /// The updates Play's update button started that wait for the job before them.
    update_queue: Vec<QueuedUpdate>,
    /// What the uninstall card asked to remove, until its confirmation is answered.
    uninstall_parts: Vec<crate::core::uninstall::Part>,
    /// What can be updated, as the last check found it.
    updates: UpdatesFound,
    started: bool,
    deleting_cache: bool,
    /// The logs are on their way to the upload service.
    uploading_logs: bool,
    /// The logs "Upload logs" asked about.
    upload_sources: Vec<crate::core::logs::Source>,
    feed: Feed,
    /// Something that went well, shown in the status bar for a few seconds.
    notice: Option<(String, std::time::Instant)>,
    /// A new player's patch on its way into the version being installed.
    pending_patch: Option<setup::PendingPatch>,
    /// Pages drawn at least once, seen or not (`prewarm`).
    warmed: std::collections::HashSet<Page>,
    /// The Quest side of the switch was clicked while Quest is off (`QUEST`): where, and
    /// when.
    quest_soon: Option<(egui::Rect, std::time::Instant)>,
    video: super::video::BackgroundVideo,
}

impl Dashboard {
    fn save(&self) {
        if self.demo || crate::core::uninstall::finished() {
            return;
        }
        if let Err(e) = self.state.save() {
            tracing::error!("saving launcher state failed: {e:#}");
        }
    }

    /// First frame: load state, import existing installs, start the monitor and catalogue.
    fn start(&mut self, ctx: &egui::Context) {
        self.started = true;
        self.state = if self.demo {
            demo_state()
        } else {
            LauncherState::load()
        };
        if !self.state.imported {
            let n = self.state.import_existing();
            tracing::info!("imported {n} existing install(s)");
            self.save();
        }
        // Debug builds: ECHOVR_PAGE=<title> opens on that page (for trying a page out).
        #[cfg(debug_assertions)]
        if let Ok(name) = std::env::var("ECHOVR_PAGE") {
            if let Some(p) = Page::ALL
                .into_iter()
                .find(|p| p.title().eq_ignore_ascii_case(&name))
            {
                self.page = p;
            } else {
                // A plugin's id: its tab, once the plugins are read.
                self.plugins.open_when_read = Some(name.to_lowercase());
            }
        }
        self.library_field = self.state.library.clone();
        self.quest_ip_field = self.state.quest_ip.clone().unwrap_or_default();
        self.relay_server_field = self.state.relay_server.clone();
        self.dev_code_field = self
            .state
            .dev_code
            .as_deref()
            .and_then(crate::core::launcher::dev_code::normalize)
            .map(|c| crate::core::launcher::dev_code::display(&c))
            .unwrap_or_default();
        self.linux_set_up = cfg!(target_os = "linux") && crate::core::linux::echoxr::is_set_up();
        if cfg!(target_os = "linux")
            && !self.demo
            && setup::linux_runtime(&mut self.state.profile.runtime)
        {
            self.save();
        }
        self.check_launcher_update(ctx);
        // The tray looks for updates while the window is closed (and starts at login if
        // its autostart entry is still there).
        if !self.demo {
            self.state.tray_at_login = crate::core::tray::autostart_on();
        }
        if !self.demo && self.state.tray {
            std::thread::spawn(crate::core::tray::start);
        }
        // The custom background is gone; so is what an older launcher converted for it.
        let old_background = crate::core::paths::data_dir().join("background");
        if !self.demo && old_background.is_dir() {
            let _ = std::fs::remove_dir_all(&old_background);
        }
        if self.demo {
            self.catalog = Some(demo_catalog());
            return;
        }
        if cfg!(any(windows, target_os = "linux")) {
            let off = self.state.spark_links_off;
            self.worker.spawn(ctx, move |tx| {
                // Only when no other app (Spark) has them, and not turned off.
                let h = links::handler();
                let r = (h == links::Handler::None && !off).then(links::register);
                let err = r.and_then(Result::err).map(|e| format!("{e:#}"));
                tx.send(Msg::LinkHandler(links::handler(), err));
            });
        }
        let c = ctx.clone();
        let monitor = Monitor::start(move || c.request_repaint());
        monitor.set_quest_ip(self.quest_ip());
        self.monitor = Some(monitor);
        self.refresh_catalog(ctx);
    }

    /// Once a second: a spark:// link this launcher was started with, or that one started
    /// by a click handed over, opens the "Join a lobby" card (the window coming to the
    /// front).
    fn take_link(&mut self, ctx: &egui::Context) {
        if self.demo {
            return;
        }
        let second = std::time::Duration::from_secs(1);
        if self.link_checked.is_some_and(|t| t.elapsed() < second) {
            return;
        }
        self.link_checked = Some(std::time::Instant::now());
        if cfg!(any(windows, target_os = "linux")) {
            ctx.request_repaint_after(second);
        }
        // The tray's "Open Echo VR Launcher" while this one is open.
        if crate::core::tray::take_window_forward() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if let Some(link) = links::take_incoming().filter(|l| links::parse(l).is_some()) {
            self.held_link = Some(link);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // Another card is open, maybe half filled in: the link waits until it's closed.
        let other_card = self
            .overlay
            .as_ref()
            .is_some_and(|o| !matches!(o, setup::Overlay::JoinLobby { .. }));
        if other_card {
            return;
        }
        let Some(link) = self.held_link.take() else {
            return;
        };
        tracing::info!("opening a spark:// link");
        self.page = Page::Play;
        self.overlay = Some(setup::Overlay::JoinLobby { input: link });
    }

    /// Settings: open spark:// links with the launcher (taking them over from Spark), or
    /// stop.
    fn set_spark_links(&mut self, ctx: &egui::Context, on: bool) {
        self.state.spark_links_off = !on;
        self.save();
        self.worker.spawn(ctx, move |tx| {
            let err = if on {
                links::register().err().map(|e| format!("{e:#}"))
            } else {
                links::unregister();
                None
            };
            tx.send(Msg::LinkHandler(links::handler(), err));
        });
    }

    /// After the frame: the embedded site where its page placed it (hidden under dialogs
    /// and cards, which it would cover).
    pub fn sync_web(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let blocked = self.dialogs.is_open() || self.overlay.is_some();
        self.web.sync(ctx, frame, blocked);
    }

    /// Where the Quest is on the network, when known.
    fn quest_ip(&self) -> Option<std::net::Ipv4Addr> {
        self.state
            .quest_ip
            .as_deref()
            .and_then(crate::core::launcher::quest_net::parse_ip)
    }

    /// Remembers where the Quest is on the network (and watches its game there).
    fn set_quest_ip(&mut self, ip: Option<std::net::Ipv4Addr>) {
        self.state.quest_ip = ip.map(|i| i.to_string());
        self.quest_ip_field = self.state.quest_ip.clone().unwrap_or_default();
        self.save();
        if let Some(m) = &self.monitor {
            m.set_quest_ip(ip);
        }
    }

    /// Echo VR on the Quest, from its API over the network.
    fn quest_game(&self) -> GameState {
        match self.snap_game {
            Some(SnapGame::QuestRunning) => GameState::Running,
            _ => self
                .monitor
                .as_ref()
                .map(Monitor::quest)
                .unwrap_or_default(),
        }
    }

    /// Looks for the Quest on this PC's local network (its game's API, or ADB).
    fn scan_quest(&mut self, ctx: &egui::Context) {
        use crate::core::launcher::quest_net;
        let Some(own) = quest_net::local_ipv4() else {
            self.dialogs.error(
                "No network",
                "This PC doesn't seem to be on a local network.",
                Default::default(),
            );
            return;
        };
        self.quest_scan = Some(0.0);
        self.worker.spawn(ctx, move |tx| {
            let cancel = AtomicBool::new(false);
            let hosts = quest_net::subnet_hosts(own);
            let found =
                quest_net::scan(&hosts, &cancel, &mut |p| tx.send(Msg::QuestScanProgress(p)));
            tx.send(Msg::QuestScanDone(found));
        });
    }

    /// Turns on ADB over the network while the Quest is on USB.
    fn enable_quest_network(&mut self, ctx: &egui::Context, ip: std::net::Ipv4Addr) {
        self.quest_net_tried = true;
        self.worker.spawn(ctx, move |tx| {
            let r = crate::core::launcher::quest_net::enable_adb_network(ip)
                .map_err(|e| UiError::from_anyhow(&e, "ADB over the network"));
            tx.send(Msg::QuestNetwork(r));
        });
    }

    /// Free bytes where new versions are installed (measured on a worker, at most every
    /// 10 s: it can take seconds on Windows with a sleeping drive).
    fn free_bytes(&mut self) -> Option<u64> {
        if self.demo {
            return Some(120_000_000_000);
        }
        self.free.get(self.state.library.clone()).flatten()
    }

    /// Windows: 3 s after a job put version `id`'s files in place, whether its mod loader
    /// (`BugSplat64.dll`) is still there and readable: antivirus takes it away right after,
    /// as a rule.
    fn check_slot_soon(&mut self, ctx: &egui::Context, id: &str) {
        use crate::core::launcher::mods::{slot_state, SlotState};
        if !cfg!(windows) || self.demo {
            return;
        }
        let Some(v) = self
            .state
            .version(id)
            .filter(|v| v.publisher_lock.is_none())
        else {
            return;
        };
        let (id, bin) = (id.to_string(), v.bin_dir());
        self.worker.spawn(ctx, move |tx| {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let lead = match slot_state(&bin) {
                SlotState::Ok => return,
                SlotState::Missing => "BugSplat64.dll, Echo VR's mod loader (nEVR), was removed right after it was put in place.".to_string(),
                SlotState::Blocked(e) => format!("BugSplat64.dll, Echo VR's mod loader (nEVR), was blocked right after it was put in place ({e})."),
            };
            tx.send(Msg::SlotGone(id, lead));
        });
    }

    /// Revive (for SteamVR) is known not to be installed.
    fn revive_missing(&mut self) -> bool {
        self.demo || self.revive.get(()).is_some_and(|dir| dir.is_none())
    }

    /// EchoXR (for SteamVR) is known not to be fetched.
    fn echoxr_missing(&mut self) -> bool {
        self.demo || self.echoxr.get(()).is_some_and(|ready| !ready)
    }

    /// On Windows, what the choice runs through isn't set up yet: EchoXR (SteamVR or Virtual
    /// Desktop through it) or Revive's injector (SteamVR).
    fn steamvr_missing(&mut self) -> bool {
        if !(cfg!(windows) || self.demo) {
            return false;
        }
        if self.state.profile.through_echoxr() {
            return self.echoxr_missing();
        }
        self.state.profile.runtime == Runtime::Revive && self.revive_missing()
    }

    /// Echo VR copies on this PC the library doesn't have yet (looked for on a worker,
    /// again when the library changes), most likely first.
    fn found_installs(&mut self) -> Vec<String> {
        if self.demo {
            return self.snap_found.clone();
        }
        let key = (self.state.library.clone(), self.state.versions.len());
        let found = self.found.get(key).unwrap_or_default();
        found
            .into_iter()
            .filter(|r| !self.state.has_root(r))
            .collect()
    }

    /// What "Delete cache" would free (`None`: not measured yet).
    fn cache_bytes(&mut self) -> Option<u64> {
        if self.demo {
            return Some(1_240_000_000);
        }
        self.cache.get(())
    }

    /// The version folders a cancelled install may have left its zip in.
    fn cache_roots(&self) -> Vec<String> {
        let lib = &self.state.library;
        let mut roots: Vec<String> = self
            .state
            .versions
            .iter()
            .filter(|v| !v.external)
            .map(|v| v.root.clone())
            .collect();
        if let Some(c) = &self.catalog {
            roots.extend(
                c.pc()
                    .map(|e| crate::core::launcher::versions::root_for(lib, &e.id)),
            );
        }
        roots
    }

    /// Updates the launcher to the newer release that is out (see `self_update`): a job,
    /// then it starts again.
    pub(super) fn update_launcher(&mut self, ctx: &egui::Context) {
        let LauncherUpdate::Available(r) = &self.launcher_update else {
            return;
        };
        let v = r.version.clone();
        let tray = self.state.tray && !self.demo;
        self.start_job(
            ctx,
            JobKind::LauncherUpdate,
            LAUNCHER_JOB,
            vec![Res::All],
            &format!("Updating the launcher to {v}"),
            "Downloading...",
            move |cancel, on| match crate::core::launcher::self_update::install(&v, cancel, on) {
                Ok(exe) => JobResult::LauncherUpdated(exe),
                Err(e) => {
                    // The update ended the tray before it replaced the files: it's back.
                    if tray {
                        crate::core::tray::start();
                    }
                    versions::job_err(
                        e.context("You can also download it yourself: github.com/marshmallow-mia/EchoVR_Launcher/releases"),
                        "Launcher Update Failed",
                    )
                }
            },
        );
    }

    /// A newer launcher is out on the channel followed, and this one can update itself to
    /// it (not the way back from a beta: that is Advanced settings' to start).
    fn launcher_ready(&self) -> bool {
        matches!(&self.launcher_update, LauncherUpdate::Available(r) if !r.back)
            && crate::core::launcher::self_update::supported()
    }

    /// Play's update button: `queue` runs after the job it started (if any), one update
    /// after the other.
    fn queue_updates(&mut self, ctx: &egui::Context, queue: Vec<QueuedUpdate>) {
        self.update_queue = queue;
        self.run_update_queue(ctx);
    }

    /// Starts the next queued update once no job runs (a step with nothing to do is
    /// skipped).
    fn run_update_queue(&mut self, ctx: &egui::Context) {
        while let Some(next) = self.update_queue.first().cloned() {
            // A version's mods wait for the jobs on it; the launcher (it restarts) for all.
            let waits = match &next {
                QueuedUpdate::Plugins(id) => self
                    .blocker(&[Res::Version(id.clone()), Res::Mods])
                    .is_some(),
                QueuedUpdate::Launcher => self.any_job(),
            };
            if waits {
                break;
            }
            self.update_queue.remove(0);
            match next {
                QueuedUpdate::Plugins(id) => mods::update_all(self, ctx, &id),
                QueuedUpdate::Launcher if self.launcher_ready() => self.update_launcher(ctx),
                QueuedUpdate::Launcher => {}
            }
        }
    }

    /// Looks for a newer launcher release in the background.
    fn check_launcher_update(&mut self, ctx: &egui::Context) {
        if self.demo {
            self.launcher_update = LauncherUpdate::Latest;
            return;
        }
        self.launcher_update = LauncherUpdate::Checking;
        let channel = self.state.launcher_channel;
        self.worker.spawn(ctx, move |tx| {
            let r = update_check::newer(channel).map_err(|e| format!("{e:#}"));
            tx.send(Msg::LauncherUpdate(channel, r));
        });
    }

    /// At the start and every 15 minutes: looks for updates of the launcher, the installed
    /// Echo VRs and their catalogue plugins in the background, announcing new ones on the
    /// desktop (once each; the tray waits while the window is open).
    fn check_updates(&mut self, ctx: &egui::Context) {
        if self.demo || self.updates.checking {
            return;
        }
        if self
            .updates
            .checked
            .is_some_and(|t| t.elapsed() < crate::core::updates::INTERVAL)
        {
            return;
        }
        self.updates.checking = true;
        let state = self.state.clone();
        self.worker.spawn(ctx, move |tx| {
            let check = crate::core::updates::check(&state);
            let fresh = crate::core::updates::not_yet_announced(&check.findings);
            if state.desktop_notifications {
                crate::core::updates::notify_desktop(&fresh);
            }
            tx.send(Msg::Updates(check));
        });
    }

    /// An update check is done: what it learned is kept, what it found shown.
    fn updates_done(&mut self, check: crate::core::updates::Check) {
        use crate::core::updates::Finding;
        self.updates.checking = false;
        self.updates.checked = Some(std::time::Instant::now());
        if crate::core::updates::apply_baselines(&mut self.state, &check) {
            self.save();
        }
        if let Some(c) = check.catalog.clone() {
            self.mods.catalog_done(c);
        }
        for f in &check.findings {
            // From the channel followed when the check began: another one is picked since.
            if check.launcher_channel != self.state.launcher_channel {
                break;
            }
            if let Finding::Launcher { version, url } = f {
                self.launcher_update = LauncherUpdate::Available(update_check::Release {
                    version: version.clone(),
                    url: url.clone(),
                    back: false,
                });
            }
        }
        // What wasn't there before in this run: in the status bar.
        let new: Vec<&Finding> = check
            .findings
            .iter()
            .filter(|f| self.updates.shown.insert(f.key()))
            .collect();
        match new.as_slice() {
            [] => {}
            [one] => self.notify(&one.text()),
            more => self.notify(&format!("{} updates available", more.len())),
        }
        self.updates.findings = check.findings;
    }

    /// Starts the measurements the pages asked for that are due.
    fn probe(&mut self, ctx: &egui::Context) {
        if self.demo {
            return;
        }
        if let Some(lib) = self.free.due(std::time::Duration::from_secs(10)) {
            self.worker.spawn(ctx, move |tx| {
                let b = crate::core::platform::free_space(std::path::Path::new(&lib));
                tx.send(Msg::FreeSpace(lib, b));
            });
        }
        if self.cache.due(std::time::Duration::from_secs(30)).is_some() {
            let roots = self.cache_roots();
            self.worker.spawn(ctx, move |tx| {
                tx.send(Msg::CacheSize(crate::core::cache::size(&roots)));
            });
        }
        if self.revive.due(std::time::Duration::from_secs(5)).is_some() {
            self.worker.spawn(ctx, |tx| {
                tx.send(Msg::Revive(crate::core::revive::find_revive_dir()));
            });
        }
        if self.echoxr.due(std::time::Duration::from_secs(5)).is_some() {
            self.worker.spawn(ctx, |tx| {
                tx.send(Msg::EchoXr(crate::core::echoxr::is_fetched()));
            });
        }
        if let Some(key) = self.found.due(std::time::Duration::from_secs(30)) {
            let state = self.state.clone();
            self.worker.spawn(ctx, move |tx| {
                let found = crate::core::launcher::discover::find_installs(&state);
                tx.send(Msg::Found(key, found));
            });
        }
    }

    /// What PLAY acts on.
    fn target(&self) -> Target {
        let demo = self.demo;
        self.state
            .target(self.catalog.as_ref(), |v| demo || v.present())
    }

    /// Snapshot mode: puts the dashboard into `snap_variant`'s state.
    fn apply_snap_variant(&mut self, ctx: &egui::Context) {
        if self.applied_variant == Some(self.snap_variant) {
            return;
        }
        self.applied_variant = Some(self.snap_variant);
        self.jobs.clear();
        self.overlay = None;
        self.notice = None;
        self.dialogs = DialogHost::default();
        self.state.owner = Some(true);
        self.state.selected = Some("pc-latest".into());
        self.platform = Platform::Pc;
        // A connected headset with Echo VR on it, as in the design concept.
        self.quest_conn.status = Some(Status::Ready);
        self.quest_info = Some(QuestInfo {
            device: Some("Meta Quest 3 (2G0YC5ZF8R0123)".into()),
            installed: true,
            marker: Some(crate::core::quest_update::Marker {
                base_apk: Some("r15_26-06-23.apk".into()),
                ..Default::default()
            }),
            wifi_ip: Some(std::net::Ipv4Addr::new(192, 168, 178, 45)),
            over_network: false,
        });
        self.update_note.clear();
        self.vrce = echovrce::Vrce::default();
        self.servers = servers::Servers::default();
        self.mods = mods::Mods::default();
        self.state.profile = demo_state().profile;
        self.state.launcher_channel = update_check::Channel::Main;
        self.snap_game = None;
        self.install_pick = None;
        self.snap_found.clear();
        self.state.versions = demo_state().versions;
        self.state.game_server = None;
        plugins::snap_game_server(
            self,
            match self.snap_variant {
                Some(SnapVariant::PluginGameServer) => Some(plugins::SnapServer::Filled),
                Some(SnapVariant::PluginGameServerAsk) => Some(plugins::SnapServer::Asking),
                Some(SnapVariant::PluginGameServerSet) => Some(plugins::SnapServer::Set),
                _ => None,
            },
        );
        if matches!(
            self.snap_variant,
            Some(SnapVariant::Fresh | SnapVariant::Installing | SnapVariant::Extracting)
        ) {
            self.state.versions.clear();
        }
        match self.snap_variant {
            Some(SnapVariant::Fresh) => self.state.selected = None,
            Some(v @ (SnapVariant::Installing | SnapVariant::Extracting)) => {
                self.state.selected = None;
                let (label, fraction) = if v == SnapVariant::Installing {
                    ("Downloading... 42.0%", Some(0.42))
                } else {
                    ("Extracting...", None)
                };
                self.jobs.insert(
                    "pc-latest".into(),
                    Job {
                        kind: JobKind::Install,
                        res: vec![Res::Version("pc-latest".into())],
                        cancellable: true,
                        title: "Installing Echo VR (PC, latest)".into(),
                        label: label.into(),
                        fraction,
                        cancel: Arc::new(AtomicBool::new(false)),
                    },
                );
            }
            Some(SnapVariant::Notice) => {
                self.notify("Echo VR 34.4 (PC) is installed. Have fun!");
            }
            Some(SnapVariant::DialogError) => self.dialogs.error(
                "Allow your PC on the Quest",
                crate::core::error::QUEST_UNAUTHORIZED,
                crate::core::error::HelpLink::UsbDebugging,
            ),
            Some(SnapVariant::DialogInstallError) => self.dialogs.error(
                "Install Failed",
                "Couldn't create the folder C:/EchoVR/versions/pc-34.4: Access is denied. (os error 5)\n\nPick another library in Settings, or check the folder's permissions.",
                Default::default(),
            ),
            Some(SnapVariant::DialogConfirm) => self.dialogs.confirm_danger(
                "snap",
                "Remove",
                "Delete Echo VR (PC, latest)?\n\nThis removes C:/EchoVR/versions/pc-latest from disk.",
                "Delete",
            ),
            Some(SnapVariant::DialogBrowser) => {
                let (title, body) = browser_dialog(setup::LICENCE_JOB);
                self.dialogs.browser(
                    BROWSER_KEY,
                    title,
                    &body,
                    "https://discord.com/oauth2/authorize?client_id=1",
                );
            }
            Some(SnapVariant::MenuOpen) => {
                let id = crate::ui::widgets::menu_id("menu-pc-latest");
                ctx.data_mut(|d| d.insert_temp(id, true));
            }
            Some(SnapVariant::DeleteCache) => settings::ask_delete_cache(self),
            Some(SnapVariant::UploadLogs) => settings::ask_upload(self),
            Some(SnapVariant::RailOpen) => self.state.rail_open = true,
            Some(SnapVariant::UpdatesFound) => {
                use crate::core::updates::Finding;
                self.updates.findings = vec![
                    Finding::Game {
                        id: "pc-latest".into(),
                        name: "Echo VR (PC, latest)".into(),
                        hash: "ab".into(),
                    },
                    Finding::Plugin {
                        version_id: "pc-latest".into(),
                        version_name: "Echo VR (PC, latest)".into(),
                        file: "CombatStats.dll".into(),
                        name: "Combat Stats".into(),
                        to: "0.3.1".into(),
                    },
                ];
                self.launcher_update = LauncherUpdate::Available(update_check::Release {
                    version: "0.11.0".into(),
                    url: concat!(env!("CARGO_PKG_REPOSITORY"), "/releases").into(),
                    back: false,
                });
                self.state.rail_open = true;
            }
            Some(SnapVariant::Uninstall) => settings::ask_uninstall(self),
            Some(SnapVariant::ModsLocked) => {}
            Some(SnapVariant::Credits) => {
                self.overlay = Some(setup::Overlay::Credits { scroll: 0.0 })
            }
            Some(SnapVariant::AdvancedSettings) => {
                self.state.launcher_channel = update_check::Channel::Beta;
                self.launcher_update = LauncherUpdate::Available(update_check::Release {
                    version: "0.11.10-beta.1".into(),
                    url: concat!(env!("CARGO_PKG_REPOSITORY"), "/releases").into(),
                    back: false,
                });
                self.overlay = Some(setup::Overlay::Advanced);
            }
            Some(SnapVariant::BetaChannel) => {
                self.state.launcher_channel = update_check::Channel::Beta;
                self.launcher_update = LauncherUpdate::Latest;
            }
            Some(SnapVariant::LogsSent) => settings::logs_sent(self, ctx, "K7Q4MZ2A"),
            Some(
                v @ (SnapVariant::InstallAsk
                | SnapVariant::InstallAskNew
                | SnapVariant::InstallAskFound
                | SnapVariant::InstallAskChoose),
            ) => {
                let entry = self
                    .catalog
                    .as_ref()
                    .and_then(|c| c.versions.iter().find(|e| e.id == "pc-latest").cloned());
                if v == SnapVariant::InstallAskNew {
                    self.state.owner = Some(false);
                }
                // Nothing installed by the launcher yet; (found) the Meta app's copy.
                if matches!(v, SnapVariant::InstallAskFound | SnapVariant::InstallAskChoose) {
                    self.state.versions.clear();
                }
                if v == SnapVariant::InstallAskFound {
                    self.snap_found = vec!["C:/Program Files/Oculus/Software/Software".into()];
                }
                if let Some(e) = entry {
                    setup::ask_install(self, e);
                }
                if let Some(setup::Overlay::Install(ask)) = &mut self.overlay {
                    if v == SnapVariant::InstallAskNew {
                        ask.link = true;
                        ask.url = "https://files.echovr.de/dlls/1727000000/pnsovr.dll".into();
                    }
                }
            }
            Some(SnapVariant::QuestSide) => self.platform = Platform::Quest,
            Some(SnapVariant::QuestRunning) => {
                self.platform = Platform::Quest;
                self.snap_game = Some(SnapGame::QuestRunning);
            }
            Some(SnapVariant::Launching) => self.snap_game = Some(SnapGame::Launching),
            Some(SnapVariant::Running) => self.snap_game = Some(SnapGame::Elsewhere),
            Some(SnapVariant::RunningOurs) => self.snap_game = Some(SnapGame::Ours),
            Some(SnapVariant::ServerHere) => self.snap_game = Some(SnapGame::Server),
            Some(SnapVariant::VersionMenu) => {
                let id = crate::ui::widgets::menu_id(play::VERSION_MENU);
                ctx.data_mut(|d| d.insert_temp(id, true));
            }
            Some(v @ (SnapVariant::SettingsPackServer | SnapVariant::PackServerCard)) => {
                let server = crate::core::launcher::store::PackServer {
                    address: "192.168.178.126:7350".into(),
                    server_key: "defaultkey".into(),
                    discord_id: "900000000000000001".into(),
                    password: "test".into(),
                };
                self.state
                    .pack_servers
                    .insert("echocombat".into(), server.clone());
                if v == SnapVariant::PackServerCard {
                    self.overlay = Some(setup::Overlay::PackServer {
                        pack: "echocombat".into(),
                        name: "EchoCombat".into(),
                        server,
                    });
                }
            }
            Some(
                v @ (SnapVariant::InstallingAnother
                | SnapVariant::InstallingAnotherMenu
                | SnapVariant::ModsInstalling),
            ) => {
                self.jobs.insert(
                    "pc-34.4".into(),
                    Job {
                        kind: JobKind::Install,
                        res: vec![Res::Version("pc-34.4".into())],
                        cancellable: true,
                        title: "Installing Echo VR 34.4 (PC)".into(),
                        label: "Downloading... 42.0%".into(),
                        fraction: Some(0.42),
                        cancel: Arc::new(AtomicBool::new(false)),
                    },
                );
                match v {
                    SnapVariant::InstallingAnotherMenu => {
                        let id = crate::ui::widgets::menu_id(play::VERSION_MENU);
                        ctx.data_mut(|d| d.insert_temp(id, true));
                    }
                    SnapVariant::ModsInstalling => self.state.selected = Some("pc-34.4".into()),
                    _ => {}
                }
            }
            Some(SnapVariant::SettingsSteamVr) => self.state.profile.runtime = Runtime::Revive,
            Some(SnapVariant::SettingsVirtualDesktop) => {
                self.state.profile.runtime = Runtime::VirtualDesktop;
                self.state.profile.vd_via = crate::core::launcher::store::VdVia::SteamVr;
            }
            Some(SnapVariant::DialogVdSwitch) => play::ask_vd_switch(self),
            Some(SnapVariant::DialogNevrBlocked) => {
                let id = self.state.versions.first().map(|v| v.id.clone()).unwrap_or_default();
                play::ask_nevr_blocked(
                    self,
                    &id,
                    "BugSplat64.dll, Echo VR's mod loader (nEVR), was removed right after it was put in place.",
                );
            }
            Some(SnapVariant::SettingsEchoXr) => {
                self.state.profile.runtime = Runtime::Revive;
                self.state.profile.steamvr_via = SteamVrVia::EchoXr;
            }
            Some(SnapVariant::SettingsLinks) => {
                self.link_handler = Some(links::Handler::Other("Spark".into()))
            }
            Some(SnapVariant::VrceSignedIn) => self.vrce.demo(true),
            Some(SnapVariant::VrceDown) => self.vrce.demo_down(true),
            Some(SnapVariant::VrceSignInFailed) => self.vrce.demo_down(false),
            Some(SnapVariant::LoginCode) => {
                self.overlay = Some(setup::Overlay::LoginNotice(
                    crate::core::launcher::login_watch::LoginNotice {
                        code: Some("58".into()),
                        message: "Please authorize this new location.\nCheck your Discord DMs from @EchoVRCE.\nSelect code >>> 58 <<<".into(),
                    },
                ))
            }
            Some(SnapVariant::LoginMessage) => {
                self.overlay = Some(setup::Overlay::LoginNotice(
                    crate::core::launcher::login_watch::LoginNotice {
                        code: None,
                        message: "Your account is suspended until 2026-10-11.\nReason: unsportsmanlike conduct.".into(),
                    },
                ))
            }
            Some(
                v @ (SnapVariant::ServersLive
                | SnapVariant::ServersHistory
                | SnapVariant::ServersStart
                | SnapVariant::ServersInvites
                | SnapVariant::PlayFriends
                | SnapVariant::FriendsPage
                | SnapVariant::ServersShare),
            ) => {
                self.vrce.demo(true);
                let tab = if v == SnapVariant::ServersHistory {
                    servers::Tab::History
                } else {
                    servers::Tab::Live
                };
                self.servers.demo(tab);
                if v == SnapVariant::ServersStart {
                    self.overlay = Some(setup::Overlay::StartServer {
                        mode: 2,
                        region: Some("eu-west".into()),
                        guild: None,
                        level: 1,
                    });
                }
                if matches!(
                    v,
                    SnapVariant::ServersInvites | SnapVariant::PlayFriends | SnapVariant::FriendsPage
                ) {
                    self.servers.demo_social(true);
                }
                if v == SnapVariant::FriendsPage {
                    self.servers.demo_search();
                }
                if v == SnapVariant::ServersShare {
                    let id = self.servers.demo_social(false);
                    self.overlay = Some(setup::Overlay::ShareMatch {
                        match_id: id,
                        started: true,
                    });
                }
            }
            Some(SnapVariant::VrceSigning) => self.vrce.demo(false),
            Some(SnapVariant::VrceSigningHere) => self.vrce.demo_here(),
            Some(SnapVariant::NewPlayer) => self.state.owner = Some(false),
            Some(SnapVariant::Licence) => {
                self.state.owner = Some(false);
                self.overlay = Some(setup::licence("pc-latest"));
            }
            Some(SnapVariant::LicenceLink) => {
                self.state.owner = Some(false);
                self.overlay = Some(setup::Overlay::Licence {
                    id: "pc-latest".into(),
                    url: "https://files.echovr.de/dlls/1727000000/pnsovr.dll".into(),
                    link: true,
                });
            }
            Some(SnapVariant::InstallAskQuest) => {
                self.state.owner = Some(false);
                self.platform = Platform::Quest;
                setup::ask_quest_install(self, false);
            }
            Some(SnapVariant::EventSelected) => {
                self.state.versions.push(InstalledVersion {
                    id: "pc-halloween-2018".into(),
                    name: "Halloween 2018".into(),
                    root: "C:/EchoVR/versions/pc-halloween-2018".into(),
                    catalog_id: Some("pc-halloween-2018".into()),
                    publisher_lock: Some("rad15_halloween".into()),
                    ..Default::default()
                });
                self.state.selected = Some("pc-halloween-2018".into());
                self.state.relay_account = Some(crate::core::launcher::store::RelayAccount {
                    name: "Pebbles".into(),
                    password: "secret".into(),
                });
            }
            Some(SnapVariant::InstallAskEvent) => {
                let entry = self.catalog.as_ref().and_then(|c| {
                    c.versions
                        .iter()
                        .find(|e| e.id == "pc-summer-2019")
                        .cloned()
                });
                if let Some(e) = entry {
                    self.install_pick = Some(e.id.clone());
                    setup::ask_install(self, e);
                }
            }
            Some(SnapVariant::RelayAccount) => {
                self.state.relay_account = None;
                self.overlay = Some(setup::Overlay::RelayAccount {
                    name: "Pebbles".into(),
                    password: "hunter22".into(),
                    play: true,
                });
            }
            Some(SnapVariant::Owner) => {
                self.state.owner = None;
                self.overlay = Some(setup::Overlay::Owner);
            }
            Some(SnapVariant::LicenceWaiting) => {
                self.state.owner = Some(false);
                self.pending_patch = Some(setup::PendingPatch {
                    version: "pc-latest".into(),
                    dll: None,
                });
                self.jobs.insert(
                    setup::LICENCE_JOB.into(),
                    Job {
                        kind: JobKind::Licence,
                        res: vec![Res::Licence],
                        cancellable: true,
                        title: "Getting your licence patch".into(),
                        label: "Discord authorization opened in your browser.".into(),
                        fraction: None,
                        cancel: Arc::new(AtomicBool::new(false)),
                    },
                );
            }
            Some(SnapVariant::JoinLobby) => {
                self.overlay = Some(setup::Overlay::JoinLobby {
                    input: "spark://c/0F5C1A2B-3C4D-5E6F-7A8B-9C0D1E2F3A4B".into(),
                })
            }
            Some(SnapVariant::Placeholder) => {
                self.install_pick = Some("pc-halloween-2017".into())
            }
            // The Mods page makes its own made-up state (`mods::show`).
            Some(
                SnapVariant::ModsNoLoader | SnapVariant::ModsOptions | SnapVariant::ModsLongText,
            ) => {}
            Some(SnapVariant::ModsHands | SnapVariant::ModsSettings | SnapVariant::ModsSettingsAll) => {
                self.state.echoxr_hands = true;
            }
            Some(SnapVariant::QuestFresh) => {
                self.platform = Platform::Quest;
                if let Some(i) = &mut self.quest_info {
                    i.installed = false;
                    i.marker = None;
                }
            }
            // Set up with the plugin list above.
            Some(
                SnapVariant::PluginGameServer
                | SnapVariant::PluginGameServerAsk
                | SnapVariant::PluginGameServerSet,
            ) => {}
            // The concept's orange "!" on CHECK FOR UPDATES.
            None => {
                self.update_note
                    .insert("pc-latest".into(), "Last update failed".into());
            }
        }
    }

    fn refresh_catalog(&mut self, ctx: &egui::Context) {
        self.catalog_loading = true;
        self.worker
            .spawn(ctx, |tx| tx.send(Msg::Catalog(Catalog::load())));
    }

    /// Fetches RIGHT NOW's numbers every minute and the news every ten, plus any image they
    /// refer to that isn't loaded yet. Snapshots use made-up data.
    fn poll_feed(&mut self, ctx: &egui::Context) {
        if self.demo && !self.feed_live {
            if self.feed.status.is_none() {
                self.feed.status = Some(feed::mock_servers());
                self.feed.news = Some(feed::mock_news());
            }
            return;
        }
        let due = |at: Option<std::time::Instant>, every| at.is_none_or(|t| t.elapsed() >= every);
        if !self.feed.status_loading && due(self.feed.status_at, Feed::STATUS_EVERY) {
            self.feed.status_loading = true;
            self.feed.status_at = Some(std::time::Instant::now());
            self.worker.spawn(ctx, |tx| {
                let s = feed::fetch_servers()
                    .inspect_err(|e| tracing::warn!("status feed unavailable: {e:#}"))
                    .ok();
                tx.send(Msg::FeedStatus(s));
            });
        }
        if !self.feed.news_loading && due(self.feed.news_at, Feed::NEWS_EVERY) {
            self.feed.news_loading = true;
            self.feed.news_at = Some(std::time::Instant::now());
            self.worker.spawn(ctx, |tx| {
                let n = feed::fetch_news()
                    .inspect_err(|e| tracing::warn!("community news unavailable: {e:#}"))
                    .ok();
                tx.send(Msg::FeedNews(n));
            });
        }
        let wanted = self.feed.wanted();
        self.feed.textures.retain(|name, _| wanted.contains(name));
        for name in wanted {
            if self.feed.textures.contains_key(&name) || self.feed.images_pending.contains(&name) {
                continue;
            }
            self.feed.images_pending.insert(name.clone());
            self.worker.spawn(ctx, move |tx| {
                let img = feed::fetch_image(&name)
                    .inspect_err(|e| tracing::warn!("feed image {name}: {e:#}"))
                    .ok();
                tx.send(Msg::FeedImage(name, img));
            });
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(10));
    }

    /// Snapshots: the live feed has fully arrived (or failed for good).
    #[cfg(test)]
    pub fn feed_settled(&self) -> bool {
        self.feed.ready() || self.feed.status_failed || self.feed.news_failed
    }

    /// Echo VR's clients on this PC (servers don't count).
    fn game(&self) -> GameState {
        self.local().state
    }

    /// Echo on this PC: the clients' state, the launcher's own game, servers.
    fn local(&self) -> Local {
        let ours = || self.state.selected.clone();
        match self.snap_game {
            Some(SnapGame::Ours) => Local {
                state: GameState::Running,
                ours: ours(),
                ..Local::default()
            },
            Some(SnapGame::Elsewhere) => Local {
                state: GameState::Running,
                others: 1,
                ..Local::default()
            },
            Some(SnapGame::Server) => Local {
                servers: vec![Some(
                    "D:/EchoServer/ready-at-dawn-echo-arena/bin/win10/echovr.exe".into(),
                )],
                ..Local::default()
            },
            Some(SnapGame::Launching | SnapGame::QuestRunning) => Local::default(),
            None => self
                .monitor
                .as_ref()
                .map(Monitor::local)
                .unwrap_or_default(),
        }
    }

    /// The launcher started the game that runs (or is starting).
    fn ours(&self) -> bool {
        self.child.is_some()
            || self.launched.is_some()
            || self.local().ours.is_some()
            || matches!(self.snap_game, Some(SnapGame::Launching))
    }

    /// The content packs installed in each live version (version id, pack), read again
    /// every few seconds (small files, while a page shows them).
    fn packs(&mut self) -> Vec<(String, crate::core::launcher::packs::PackRecord)> {
        if self.demo {
            let shown = matches!(
                self.snap_variant,
                Some(SnapVariant::SettingsPackServer | SnapVariant::PackServerCard)
            );
            if !shown {
                return Vec::new();
            }
            let pack = crate::core::launcher::packs::PackRecord {
                id: "echocombat".into(),
                name: "EchoCombat".into(),
                enabled: true,
                ..Default::default()
            };
            return vec![("pc-latest".to_string(), pack)];
        }
        let fresh = self
            .packs_seen
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < std::time::Duration::from_secs(5));
        if !fresh {
            let seen = self
                .state
                .versions
                .iter()
                .filter(|v| v.publisher_lock.is_none() && (self.demo || v.present()))
                .flat_map(|v| {
                    crate::core::launcher::mods::installed_packs(v)
                        .into_iter()
                        .map(|p| (v.id.clone(), p))
                })
                .collect();
            self.packs_seen = Some((std::time::Instant::now(), seen));
        }
        self.packs_seen
            .as_ref()
            .map(|(_, p)| p.clone())
            .unwrap_or_default()
    }

    /// Where a start of the live version `id` plays when that isn't EchoVRCE: a pack that
    /// is on there with a server of its own ("EchoCombat on 192.168.1.5:7350"), else the
    /// server a launcher plugin set ("Server: EchoCombat test").
    fn server_for(&mut self, id: &str) -> Option<String> {
        let packs = self.packs();
        let pack = packs
            .iter()
            .filter(|(v, p)| v == id && p.enabled)
            .find_map(|(_, p)| {
                let s = self.state.pack_servers.get(&p.id)?;
                let address = s.address.trim();
                (!address.is_empty()).then(|| format!("{} on {address}", p.name))
            });
        pack.or_else(|| {
            let g = self.state.game_server.as_ref()?;
            let address = g.server.address.trim();
            let name = g.name.trim();
            (!address.is_empty())
                .then(|| format!("Server: {}", if name.is_empty() { address } else { name }))
        })
    }

    /// Why version `v`'s files can't be changed now (`None`: they can): Echo VR runs, or
    /// a server runs from its folder (both hold its files).
    fn files_in_use(&self, v: &InstalledVersion) -> Option<&'static str> {
        let local = self.local();
        if local.state.is_running() {
            Some("Close Echo VR first: it holds the game's files")
        } else if local.server_in(&v.root) {
            Some("An Echo VR server runs from this folder and holds its files: stop it first")
        } else {
            None
        }
    }

    fn check_quest(&mut self, ctx: &egui::Context, interactive: bool) {
        self.quest_info = None;
        self.quest_conn.network = self.quest_ip().filter(|_| self.state.quest_adb_network);
        self.quest_conn.check(ctx, interactive);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        // Linux: Steam's start (`--play`) failed: what it found, once.
        let due = self
            .linux_outcome_at
            .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(2));
        if cfg!(target_os = "linux") && !self.demo && due {
            self.linux_outcome_at = Some(std::time::Instant::now());
            if let Some(o) = crate::core::linux::take_outcome() {
                self.dialogs.error(&o.title, &o.message, Default::default());
            }
        }
        // Forget our child once it exited, and the game we started once it has ended
        // (or never showed up).
        if let Some(c) = self.child.as_mut() {
            match c.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    let code = status.code();
                    if let Some(c) = code {
                        tracing::info!("the start PLAY made ended with code {:#x}", c as u32);
                    }
                    // Windows' loader turned the game down (EchoXR passes the game's code
                    // on). Not for Virtual Desktop's streamer, which only hands the game on.
                    let vd_streamer = self.state.profile.runtime == Runtime::VirtualDesktop
                        && !self.state.profile.through_echoxr();
                    let loader = code
                        .and_then(crate::core::launcher::game::loader_failure)
                        .filter(|_| !vd_streamer);
                    // EchoXR.exe ended before the game showed up: it says why (later on,
                    // its exit code is Echo's own).
                    let starting = self.launched.is_some_and(|l| !l.seen);
                    let why = code
                        .and_then(|c| crate::core::echoxr::exit_message_for(c, &self.state.profile))
                        .filter(|_| self.child_echoxr && starting);
                    self.child = None;
                    if let (Some(l), Target::Installed(v)) = (loader, self.target()) {
                        self.launched = None;
                        let c = code.unwrap_or_default() as u32;
                        play::ask_nevr_blocked(
                            self,
                            &v.id,
                            &format!("Echo VR didn't start: {l} (code {c:#x}), as a rule BugSplat64.dll, its mod loader (nEVR)."),
                        );
                    } else if let Some(why) = why {
                        self.launched = None;
                        self.dialogs.error(
                            "Echo VR didn't start through EchoXR",
                            why,
                            Default::default(),
                        );
                    }
                }
                Err(_) => self.child = None,
            }
        }
        let ours = self.local().ours.is_some();
        if let Some(l) = self.launched.as_mut() {
            if ours {
                l.seen = true;
            } else if self.launch_cancel.is_some() {
                // Still handing the start to Steam: the wait begins once Steam has it.
            } else if l.seen || l.at.elapsed() > game::LAUNCH_WAIT {
                self.launched = None;
            }
        }
        for m in self.worker.drain() {
            match m {
                Msg::Catalog(c) => {
                    self.catalog = Some(c);
                    self.catalog_loading = false;
                }
                Msg::JobStep(id, step) => {
                    // Discord answered (the job moved on): the browser dialog has done its part.
                    if self.browser_job.as_deref() == Some(id.as_str())
                        && !matches!(step, Step::Browser(_))
                    {
                        self.browser_job = None;
                        self.dialogs.dismiss(BROWSER_KEY);
                    }
                    if let Some(j) = self.jobs.get_mut(&id) {
                        match step {
                            Step::Browser(url) => {
                                j.label = "Waiting for Discord in your browser...".into();
                                let (title, body) = browser_dialog(&id);
                                self.dialogs.browser(BROWSER_KEY, title, &body, &url);
                                self.browser_job = Some(id.clone());
                            }
                            Step::Status(s) => {
                                // The downloader reports progress as "12.34%" status lines too.
                                j.fraction = s
                                    .strip_suffix('%')
                                    .and_then(|p| p.parse::<f32>().ok())
                                    .map(|p| p / 100.0);
                                j.label = s;
                            }
                            Step::Percent(p) => {
                                j.fraction = Some(p as f32 / 100.0);
                                let verb = match j.kind {
                                    JobKind::Verify => "Verifying",
                                    _ => "Downloading",
                                };
                                j.label = format!("{verb}... {p:.1}%");
                            }
                            Step::Checking(p) => {
                                j.fraction = Some(p as f32 / 100.0);
                                j.label = format!("Checking game files... {p:.0}%");
                            }
                        }
                    }
                }
                Msg::JobDone(id, r) => {
                    if self.browser_job.as_deref() == Some(id.as_str()) {
                        self.browser_job = None;
                        self.dialogs.dismiss(BROWSER_KEY);
                    }
                    let job = self.jobs.remove(&id);
                    let kind = job.as_ref().map(|j| j.kind);
                    self.job_done(ctx, &id, kind, job.map(|j| j.title), r);
                }
                Msg::QuestInfo(r) => {
                    self.quest_busy = false;
                    match r {
                        Ok(i) => {
                            // Over USB: learn where it is on the network, and set up ADB
                            // there when asked to.
                            if let (Some(ip), false) = (i.wifi_ip, i.over_network) {
                                if self.quest_ip() != Some(ip) {
                                    self.set_quest_ip(Some(ip));
                                }
                                if self.state.quest_adb_network && !self.quest_net_tried {
                                    self.enable_quest_network(ctx, ip);
                                }
                            }
                            if i.installed {
                                self.quest_was_patched =
                                    i.marker.as_ref().is_some_and(|m| m.patched);
                            }
                            self.quest_info = Some(i)
                        }
                        Err(e) => {
                            self.quest_info = None;
                            self.dialogs.error_ui(&e);
                        }
                    }
                }
                Msg::QuestAction(r) => {
                    self.quest_busy = false;
                    if let Err(e) = r {
                        self.dialogs.error_ui(&e);
                    }
                }
                Msg::Consent(tx) => {
                    self.consent = Some(tx);
                    self.dialogs.confirm(
                        setup::CONSENT_KEY,
                        "Administrator rights required",
                        "This step needs administrator rights.\n\nStart the privileged helper now? Windows will ask you to confirm.",
                        crate::ui::dialogs::Icon::Question,
                    );
                }
                Msg::LinkHandler(h, err) => {
                    self.link_handler = Some(h);
                    if let Some(e) = err {
                        self.notify(&format!("Couldn't take over spark:// links: {e}"));
                    }
                }
                Msg::CacheDeleted(failed) => {
                    self.deleting_cache = false;
                    self.cache = Probe::default();
                    if failed.is_empty() {
                        self.notify("The cached files are deleted");
                    } else {
                        let mut msg = String::from(
                            "The other cached files are deleted. These could not be deleted (in use or no permission):\n",
                        );
                        for f in failed.iter().take(6) {
                            msg.push_str(&format!("\n{}", f.display()));
                        }
                        self.dialogs
                            .error("Some files are still there", &msg, Default::default());
                    }
                }
                Msg::FeedStatus(s) => {
                    self.feed.status_loading = false;
                    self.feed.status_failed = s.is_none() && self.feed.status.is_none();
                    if s.is_some() {
                        self.feed.status = s;
                        self.feed.images_pending.clear();
                    }
                }
                Msg::FeedNews(n) => {
                    self.feed.news_loading = false;
                    self.feed.news_failed = n.is_none() && self.feed.news.is_none();
                    if n.is_some() {
                        self.feed.news = n;
                        self.feed.images_pending.clear();
                    }
                }
                Msg::FreeSpace(lib, b) => self.free.done(lib, b),
                Msg::CacheSize(b) => self.cache.done((), b),
                Msg::QuestScanProgress(p) => {
                    if self.quest_scan.is_some() {
                        self.quest_scan = Some(p);
                    }
                }
                Msg::QuestScanDone(found) => {
                    self.quest_scan = None;
                    match found.first() {
                        Some(f) => {
                            self.set_quest_ip(Some(f.ip));
                            let what = if f.api { "Echo VR's API" } else { "ADB" };
                            self.notify(&format!("Found your Quest at {} ({what})", f.ip));
                        }
                        None => self.dialogs.info(
                            "No Quest found",
                            "Nothing on your network answered as a Quest.\n\n\
                             The scan finds it while Echo VR runs with API access on, or \
                             once ADB over the network is on. You can also type its address, \
                             or plug it in by USB once so the launcher can read it.",
                        ),
                    }
                }
                Msg::QuestLogs(r) => {
                    self.quest_busy = false;
                    match r {
                        Ok(dir) => {
                            self.notify("Saved your Quest's logs");
                            if let Err(e) = crate::core::platform::open_folder(&dir) {
                                tracing::warn!("opening {}: {e:#}", dir.display());
                            }
                        }
                        Err(e) => self.dialogs.error_ui(&e),
                    }
                }
                Msg::LaunchNote(n) => self.launch_note = Some(n),
                Msg::LinuxStarted {
                    result,
                    version,
                    bin,
                    root,
                } => {
                    self.launch_cancel = None;
                    self.launch_note = None;
                    match result {
                        Ok(true) => {
                            if let Some(m) = &self.monitor {
                                m.launched(None, &version, &bin);
                            }
                            // The wait for the game starts now that Steam has the link.
                            self.launched = Some(Launched::now());
                            self.login_watch =
                                Some(play::LoginWatching::new(&root, &self.state.profile));
                        }
                        Ok(false) => self.launched = None,
                        Err(e) => {
                            self.launched = None;
                            self.dialogs
                                .error("Couldn't start Echo VR", &e, Default::default());
                        }
                    }
                }
                Msg::SlotGone(id, lead) => play::ask_nevr_blocked(self, &id, &lead),
                Msg::LogsUploaded(r) => {
                    self.uploading_logs = false;
                    match r {
                        Ok(code) => {
                            tracing::info!("logs: uploaded");
                            settings::logs_sent(self, ctx, &code)
                        }
                        Err(why) => {
                            tracing::warn!("logs: upload failed: {why}");
                            self.dialogs.error(
                                "Couldn't upload your logs",
                                &why,
                                Default::default(),
                            )
                        }
                    }
                }
                Msg::QuestNetwork(r) => match r {
                    Ok(()) => self.notify("ADB over the network is on: you can unplug your Quest"),
                    Err(e) => self.dialogs.error_ui(&e),
                },
                // A channel picked since then gets its own answer.
                Msg::LauncherUpdate(channel, _) if channel != self.state.launcher_channel => {}
                Msg::LauncherUpdate(_, r) => {
                    self.launcher_update = match r {
                        Ok(Some(release)) => LauncherUpdate::Available(release),
                        Ok(None) => LauncherUpdate::Latest,
                        Err(e) => {
                            tracing::info!("launcher update check failed: {e}");
                            LauncherUpdate::Failed
                        }
                    }
                }
                Msg::Updates(check) => self.updates_done(check),
                Msg::ModView(gen, id, view) => self.mods.read_done(gen, id, view),
                Msg::ModCatalog(c) => self.mods.catalog_done(c),
                Msg::Revive(dir) => self.revive.done((), dir),
                Msg::DevCode(status) => self.dev_status = status,
                Msg::EchoXr(ready) => self.echoxr.done((), ready),
                Msg::Found(key, found) => self.found.done(key, found),
                Msg::FeedImage(name, img) => {
                    if let Some(img) = img {
                        let size = [img.width() as usize, img.height() as usize];
                        let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                        let options = egui::TextureOptions::LINEAR
                            .with_mipmap_mode(Some(egui::TextureFilter::Linear));
                        let tex = ctx.load_texture(format!("feed:{name}"), color, options);
                        self.feed.images_pending.remove(&name);
                        self.feed.textures.insert(name, tex);
                    }
                }
            }
        }
        if self.dialogs.take(BROWSER_KEY).is_some() {
            if let Some(id) = self.browser_job.take() {
                tracing::info!("job {id}: browser wait cancelled by the player");
                self.cancel_job(&id);
            }
        }
        if let Some(a) = self.dialogs.take(setup::CONSENT_KEY) {
            if let Some(tx) = self.consent.take() {
                let _ = tx.send(a.is_yes());
            }
        }
        if self
            .dialogs
            .take(setup::JOIN_KEY)
            .is_some_and(|a| a.is_yes())
        {
            crate::core::platform::open_url(crate::core::oauth::PATCHER_INVITE);
        }
        self.quest_conn.poll(&mut self.dialogs);
        if let Some(notice) = self.vrce.tick(ctx, self.demo) {
            self.notify(&notice);
        }
        echovrce::site_linked(self);
        let session = self.vrce.session();
        self.game_sign_in
            .tick(ctx, session.clone(), &self.state.versions, self.demo);
        let shown = match self.page {
            Page::Servers => servers::Shown::Servers,
            Page::Play => servers::Shown::Play,
            Page::Friends => servers::Shown::Friends,
            _ => servers::Shown::Neither,
        };
        if let Some(notice) = self.servers.tick(ctx, session, shown) {
            self.notify(&notice);
        }
        servers::follow_up(self, ctx);
        plugins::tick(self, ctx);
        play::watch_login(self, ctx);
        self.take_link(ctx);
        self.check_updates(ctx);
        // Read the headset's version once it is connected.
        // A reinstall asks the install's questions again.
        if self
            .dialogs
            .take(setup::QUEST_REINSTALL_KEY)
            .is_some_and(|a| a.is_yes())
        {
            setup::ask_quest_install(self, false);
        }
        let ready = self.quest_conn.status == Some(Status::Ready);
        // Probing while a Quest job runs would trip over its adb restarts.
        let quest_job = self.jobs.contains_key(setup::QUEST_JOB);
        if ready && self.quest_info.is_none() && !self.quest_busy && !quest_job {
            self.quest_busy = true;
            self.worker.spawn(ctx, |tx| {
                let r = crate::core::launcher::quest::info()
                    .map_err(|e| UiError::from_anyhow(&e, "Quest"));
                tx.send(Msg::QuestInfo(r));
            });
        }
    }

    /// Shows `text` in the status bar for a few seconds.
    /// "Quest support is coming soon" above the switch's Quest side, fading out.
    fn quest_soon(&mut self, kit: &mut Kit, ctx: &egui::Context) {
        let Some((at, since)) = self.quest_soon else {
            return;
        };
        let age = since.elapsed();
        if age >= QUEST_SOON_FOR {
            self.quest_soon = None;
            return;
        }
        ctx.request_repaint();
        let fade = (QUEST_SOON_FOR - age).as_secs_f32().min(0.4) / 0.4;
        let g = kit.spaced_galley(
            "QUEST SUPPORT IS COMING SOON",
            design::din(13.0),
            design::TEXT.gamma_multiply(fade),
            dz(0.5),
            false,
        );
        let pad = egui::vec2(dz(14.0), dz(9.0));
        let size = g.size() + pad * 2.0;
        let min = egui::pos2(at.center().x - size.x / 2.0, at.top() - size.y - dz(10.0));
        let r = egui::Rect::from_min_size(min, size);
        let p = kit.ui.painter();
        p.rect_filled(r, dz(6.0), design::BLUE.gamma_multiply(fade));
        p.galley(r.min + pad, g, design::TEXT);
    }

    fn notify(&mut self, text: &str) {
        self.notice = Some((text.to_string(), std::time::Instant::now()));
    }

    fn job_done(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        kind: Option<JobKind>,
        title: Option<String>,
        r: JobResult,
    ) {
        let cancelled = matches!(r, JobResult::Failed(None));
        match &r {
            JobResult::Failed(None) => tracing::info!("job {id}: cancelled"),
            JobResult::Failed(Some(e)) => {
                tracing::error!(
                    "job {id} failed: {}: {}",
                    e.title,
                    e.message.replace('\n', " ")
                )
            }
            JobResult::OAuthFailed(e) => {
                tracing::warn!("job {id}: Discord authorization failed: {e:?}")
            }
            _ => tracing::info!("job {id}: done"),
        }
        // A new player's patch: gone with its fetch, or with its version's install.
        if id == setup::LICENCE_JOB && !matches!(r, JobResult::LicenceFetched(_)) {
            self.pending_patch = None;
        }
        let install_failed = matches!(kind, Some(JobKind::Install | JobKind::Reinstall))
            && matches!(r, JobResult::Failed(_));
        // A first install that didn't make it: the selection goes back to what it was, not
        // to a version that isn't there.
        if let Some(before) = self.select_back.remove(id) {
            let gone = self.state.version(id).is_none();
            if install_failed && gone && self.state.selected.as_deref() == Some(id) {
                self.state.selected = before
                    .filter(|b| self.state.version(b).is_some())
                    .or_else(|| self.state.versions.first().map(|v| v.id.clone()));
                self.save();
            }
        }
        if install_failed && self.pending_patch.as_ref().is_some_and(|p| p.version == id) {
            self.pending_patch = None;
            self.cancel_job(setup::LICENCE_JOB);
        }
        if id == setup::QUEST_JOB {
            // Read the headset again once the job is over.
            self.quest_info = None;
        }
        if id == setup::LIBRARY_JOB {
            if let (Some(want), JobResult::Failed(_)) = (self.library_wanted.take(), &r) {
                self.state.revive_library = !want;
                self.save();
            }
        }
        // PLAY prepared VR first: the game starts once that's done (not after a failure).
        let play_after = [
            setup::REVIVE_JOB,
            setup::ECHOXR_JOB,
            setup::LINUX_JOB,
            setup::EVENT_JOB,
        ]
        .contains(&id)
            && std::mem::take(&mut self.play_after_prep);
        let start_now = play_after
            && matches!(
                r,
                JobResult::ReviveReady(_)
                    | JobResult::EchoXrReady
                    | JobResult::LinuxReady(_)
                    | JobResult::EventBuildReady
            );
        match r {
            JobResult::Installed(mut v, update_failed) => {
                crate::core::updates::after_update(&mut v, update_failed.is_none());
                self.updates.check_soon();
                let name = v.name.clone();
                if self.state.selected.is_none() {
                    self.state.selected = Some(v.id.clone());
                }
                // Another version was chosen meanwhile (played while this one downloaded).
                let elsewhere = self.state.selected.as_deref() != Some(v.id.as_str());
                let installed = v.id.clone();
                self.state.upsert(v);
                self.save();
                self.check_slot_soon(ctx, &installed);
                // A new player's patch goes in now, if it is already here.
                setup::apply_pending(self, ctx);
                if update_failed.is_none() {
                    self.update_note.remove(id);
                }
                match update_failed {
                    None if elsewhere => self.notify(&format!(
                        "{name} is installed: choose it next to PLAY to play it"
                    )),
                    None => self.notify(&format!("{name} is installed. Have fun!")),
                    Some(why) => {
                        self.update_note
                            .insert(id.to_string(), "Last update failed".into());
                        self.dialogs.error(
                            "Installed, but not updated",
                            &format!(
                                "{name} is installed, but its update failed:\n\n{why}\n\n\
                                 Use Update in its MANAGE menu on the Install page to try again."
                            ),
                            Default::default(),
                        );
                    }
                }
            }
            JobResult::Reinstalled(mut r) => {
                let name = r.version.name.clone();
                crate::core::updates::after_update(&mut r.version, r.update_failed.is_none());
                self.updates.check_soon();
                self.state.upsert(r.version);
                self.save();
                self.check_slot_soon(ctx, id);
                setup::apply_pending(self, ctx);
                if r.update_failed.is_none() {
                    self.update_note.remove(id);
                }
                match (r.update_failed, r.repaired.len()) {
                    (Some(why), _) => {
                        self.update_note
                            .insert(id.to_string(), "Last update failed".into());
                        self.dialogs.error(
                            "Reinstalled, but not updated",
                            &format!(
                                "{name}'s files are all right again, but its update failed:\n\n{why}\n\n\
                                 Use Update in its MANAGE menu on the Install page to try again."
                            ),
                            Default::default(),
                        );
                    }
                    (None, 0) => self.notify(&format!("{name}: every game file was intact")),
                    (None, 1) => self.notify(&format!("{name}: 1 broken game file was fetched again")),
                    (None, n) => {
                        self.notify(&format!("{name}: {n} broken game files were fetched again"))
                    }
                }
            }
            JobResult::Updated => {
                if let Some(v) = self.state.versions.iter_mut().find(|v| v.id == id) {
                    crate::core::updates::after_update(v, true);
                    self.save();
                }
                self.check_slot_soon(ctx, id);
                self.updates.check_soon();
                self.mods.changed();
                self.update_note.insert(id.to_string(), "Up to date".into());
                self.notify("Echo VR is up to date");
            }
            JobResult::Verified(bad) if bad.is_empty() => {
                self.update_note
                    .insert(id.to_string(), "All files intact".into());
                self.notify("All game files are intact");
            }
            JobResult::Verified(bad) => {
                let mut msg = format!("{} file(s) are missing or modified:\n", bad.len());
                for b in bad.iter().take(8) {
                    msg.push_str(&format!("\n  {b}"));
                }
                if bad.len() > 8 {
                    msg.push_str("\n  ...");
                }
                msg.push_str("\n\nRepair them now?");
                versions::ask_repair(self, id, &msg);
            }
            JobResult::Patched => {
                if let Some(v) = self.state.versions.iter_mut().find(|v| v.id == id) {
                    v.patched = true;
                }
                self.save();
                self.notify("Your licence patch is in place. Have fun!");
            }
            JobResult::LicenceFetched(dll) => {
                if let Some(p) = &mut self.pending_patch {
                    p.dll = Some(dll);
                    setup::apply_pending(self, ctx);
                }
            }
            JobResult::Unpatched => {
                if let Some(v) = self.state.versions.iter_mut().find(|v| v.id == id) {
                    v.patched = false;
                }
                self.save();
                self.notify("The licence patch is removed: the original pnsovr.dll is back");
            }
            JobResult::OAuthFailed(e) => {
                use crate::core::oauth::OAuthError;
                match (&e, e.dialog()) {
                    (OAuthError::NotInGuild(_), Some((title, msg))) => self.dialogs.options(
                        setup::JOIN_KEY,
                        title,
                        &msg,
                        crate::ui::dialogs::Icon::Info,
                        &["Join Server", "Close"],
                    ),
                    (_, Some((title, msg))) => self.dialogs.error(title, &msg, Default::default()),
                    (_, None) => {}
                }
            }
            JobResult::Uninstalled(o) => settings::uninstalled(self, o),
            JobResult::Removed(id) => {
                if let Some(v) = self.state.version(&id).cloned() {
                    versions::forget(self, ctx, &v);
                    self.notify(&format!("{} is removed", v.name));
                }
            }
            JobResult::ModsChanged(notice) => {
                self.mods.changed();
                self.notify(&notice);
                self.updates.check_soon();
            }
            JobResult::HandsInstalled => {
                self.state.echoxr_hands = true;
                let echoxr = crate::core::launcher::store::SteamVrVia::EchoXr;
                if !cfg!(target_os = "linux") && self.state.profile.steamvr_via != echoxr {
                    self.state.profile.steamvr_via = echoxr;
                    self.notify("EchoXR Hands is installed, and EchoXR is on: hand tracking needs it");
                } else {
                    self.notify("EchoXR Hands is installed: it plays along through EchoXR");
                }
                self.save();
                self.mods.changed();
            }
            JobResult::ReviveReady(notes) => {
                self.revive = Probe::default();
                if !play_after {
                    self.notify("SteamVR is ready: PLAY starts Echo VR through it");
                }
                if !notes.is_empty() {
                    self.dialogs.info(
                        "SteamVR is ready",
                        &format!(
                            "Echo VR plays through SteamVR now, but not everything worked:\n\n{}",
                            notes.join("\n\n")
                        ),
                    );
                }
            }
            JobResult::LauncherUpdated(exe) => {
                // The new one starts (it waits for this one to end), the old tray goes.
                self.save();
                crate::core::tray::quit();
                match crate::core::launcher::self_update::restart(&exe) {
                    Ok(()) => {
                        tracing::info!("launcher updated: restarting");
                        crate::core::elevation::shutdown();
                        std::process::exit(0);
                    }
                    Err(e) => self.dialogs.error(
                        "Launcher updated",
                        &format!("The new launcher is in place, but it couldn't be started ({e:#}). Close this one and start it again."),
                        Default::default(),
                    ),
                }
            }
            JobResult::EventBuildReady => {
                if !play_after {
                    self.notify("The event build is set up: PLAY starts it");
                }
            }
            JobResult::EchoXrReady => {
                self.echoxr = Probe::default();
                if !play_after {
                    self.notify(if self.state.profile.runtime == Runtime::VirtualDesktop {
                        "EchoXR is ready: PLAY starts Echo VR through it on Virtual Desktop"
                    } else {
                        "SteamVR through EchoXR is ready: PLAY starts Echo VR through it"
                    });
                }
            }
            JobResult::LinuxReady(appid) => {
                self.state.linux_appid = Some(appid);
                self.linux_set_up = true;
                self.save();
                if !play_after {
                    self.notify("Echo VR is ready on Linux: PLAY starts it through Steam");
                }
            }
            JobResult::ArtworkInstalled => {
                self.notify("Echo VR's artwork is installed (restart SteamVR to see it)")
            }
            JobResult::LibraryEntry(true) => {
                self.notify("Echo VR is in SteamVR's library (restart SteamVR to see it)")
            }
            JobResult::LibraryEntry(false) => {
                self.notify("Echo VR is out of SteamVR's library (restart SteamVR to see it)")
            }
            JobResult::QuestInstalled(crate::core::launcher::quest::Installed::UpToDate) => {
                self.notify("Echo VR is installed on your Quest and up to date")
            }
            JobResult::QuestInstalled(crate::core::launcher::quest::Installed::NotChecked) => {
                self.dialogs.info(
                    "Installed, but not checked for updates",
                    "Echo VR is installed on your Quest, but the update server couldn't be reached, so it may not be the latest version.\n\nUse Update on the Quest side of the Install page later.",
                )
            }
            JobResult::QuestUpdated => self.notify("Your Quest has the latest update"),
            JobResult::QuestNeedsReinstall(detail) => self.dialogs.options(
                setup::QUEST_REINSTALL_KEY,
                "Echo VR version mismatch",
                &format!("{detail}\n\nReinstall Echo VR on your Quest to continue."),
                crate::ui::dialogs::Icon::Warning,
                &["Reinstall Echo VR", "Cancel"],
            ),
            JobResult::Failed(None) => {
                if let Some(t) = title {
                    self.notify(&format!("Cancelled: {t}"));
                }
            }
            JobResult::Failed(Some(e)) => {
                if kind == Some(JobKind::Update) {
                    self.update_note
                        .insert(id.to_string(), "Last update failed".into());
                }
                self.dialogs.error_ui(&e);
            }
        }
        // The match a JOIN was for, when that waited for the preparation.
        let lobby = if play_after {
            self.pending_lobby.take()
        } else {
            None
        };
        if start_now {
            play::try_start(self, ctx, lobby);
        }
        // Cancelled: the updates queued after it don't go on either.
        if cancelled {
            self.update_queue.clear();
        }
        self.run_update_queue(ctx);
    }

    /// Starts job `id` on a worker, changing `res` (see [`Res`]).
    #[allow(clippy::too_many_arguments)]
    fn start_job(
        &mut self,
        ctx: &egui::Context,
        kind: JobKind,
        id: &str,
        res: Vec<Res>,
        title: &str,
        label: &str,
        f: impl FnOnce(&AtomicBool, &mut dyn FnMut(Step)) -> JobResult + Send + 'static,
    ) {
        tracing::info!("job {id}: {title}");
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.insert(
            id.to_string(),
            Job {
                kind,
                res,
                cancellable: !matches!(
                    kind,
                    JobKind::Unpatch | JobKind::Uninstall | JobKind::Remove
                ) && id != setup::LIBRARY_JOB,
                title: title.to_string(),
                label: label.to_string(),
                fraction: None,
                cancel: cancel.clone(),
            },
        );
        let id = id.to_string();
        self.worker.spawn(ctx, move |tx| {
            let mut on = |s: Step| tx.send(Msg::JobStep(id.clone(), s));
            let r = f(&cancel, &mut on);
            tx.send(Msg::JobDone(id, r));
        });
    }

    fn cancel_job(&mut self, id: &str) {
        if let Some(j) = self.jobs.get(id) {
            tracing::info!("job {id}: cancel requested");
            j.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn any_job(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// The running jobs, the one to name first: real work before a licence patch waiting
    /// for its version, then by title (the map's order changes).
    fn jobs_in_order(&self) -> Vec<&Job> {
        let mut jobs: Vec<&Job> = self.jobs.values().collect();
        jobs.sort_by(|a, b| {
            (a.kind == JobKind::Licence, &a.title).cmp(&(b.kind == JobKind::Licence, &b.title))
        });
        jobs
    }

    /// The running job an action on `res` has to wait for (`None`: it can go ahead).
    fn blocker(&self, res: &[Res]) -> Option<&Job> {
        self.jobs_in_order()
            .into_iter()
            .find(|j| clash(&j.res, res))
    }

    /// Why an action on `res` can't be done now: the job it waits for.
    pub(super) fn busy_with(&self, res: &[Res]) -> Option<String> {
        self.blocker(res).map(busy_tip)
    }

    /// "Busy: …" for whatever runs (an action that waits for every job).
    fn busy_any(&self) -> Option<String> {
        self.busy_with(&[Res::All])
    }

    /// Draws the launcher.
    pub fn show(&mut self, kit: &mut Kit) {
        let ctx = kit.ctx();
        let page = self.page;
        if !self.started {
            self.start(&ctx);
        }
        if kit.assets.track_window(&ctx) {
            self.warmed.clear();
        }
        self.poll(&ctx);
        self.poll_feed(&ctx);
        if self.demo {
            self.apply_snap_variant(&ctx);
        }
        // Track how long the game has been running.
        match (self.game().is_running(), self.game_since) {
            (true, None) => self.game_since = Some(std::time::Instant::now()),
            (false, Some(_)) => self.game_since = None,
            _ => {}
        }

        self.background(kit, &ctx);

        // The dashboard stays visible but inert under an overlay card.
        let blocked = kit.blocked;
        if self.overlay.is_some() {
            kit.blocked = true;
        }
        versions::handle_answers(self, &ctx);
        settings::uninstall_answers(self, &ctx);
        play::answers(self, &ctx);
        // The page right of the unfolded rail: laid out that much narrower (as for a
        // narrower window), at the same height.
        let shift = self.rail_extra(&ctx);
        kit.origin.x += shift;
        kit.ex -= shift;
        self.page_body(kit, &ctx, self.page);
        self.warmed.insert(self.page);
        if !(self.page == Page::EchoVrce && echovrce::fills_page(self)) {
            self.top_bar(kit, &ctx);
        }
        self.quest_soon(kit, &ctx);
        kit.origin.x -= shift;
        kit.ex += shift;
        self.rail(kit, &ctx, shift);
        kit.blocked = blocked;
        setup::draw_overlay(self, kit, &ctx);
        self.prewarm(kit, &ctx, shift);
        self.probe(&ctx);
        if self.any_job() || self.quest_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        // A click switched pages after the old one was drawn: draw the new one now, not
        // on the next mouse move.
        if self.page != page {
            ctx.request_repaint();
        }
    }

    /// The designer's video, or its first frame (snapshots, and until the first frame is
    /// decoded), covering the window. It holds while the window isn't focused and while
    /// Echo VR runs.
    fn background(&mut self, kit: &mut Kit, ctx: &egui::Context) {
        let window = kit.window();
        if !self.demo {
            let focused = ctx.input(|i| i.viewport().focused) != Some(false);
            let play = focused && !self.game().is_running();
            if let Some(tex) = self.video.frame(ctx, play) {
                kit.texture_cover(tex, window, 0.0);
                return;
            }
        }
        let s = (window.width() / W).max(window.height() / H);
        let (w, h) = ((W * s).round() as u32, (H * s).round() as u32);
        let tex = kit.assets.tex(ctx, "main_background.jpg", w, h);
        kit.texture_cover(&tex, window, 0.0);
    }

    /// A page under the status bar.
    fn page_body(&mut self, kit: &mut Kit, ctx: &egui::Context, page: Page) {
        // The other pages start with their header strip.
        // (The EchoVRCE site draws its own where the status bar is elsewhere.)
        if !matches!(
            page,
            Page::Play | Page::Install | Page::Settings | Page::Servers | Page::Friends
        ) && !(page == Page::EchoVrce && echovrce::fills_page(self))
        {
            let h = HEADER.wider(kit.dx());
            let title = match page {
                Page::Plugin(i) => plugins::title(self, i),
                _ => page.title().to_string(),
            };
            kit.header_strip(dz(h.x), dz(h.y), dz(h.w), dz(h.h), &title);
        }
        match page {
            Page::Play => play::show(self, kit, ctx),
            Page::Install => install::show(self, kit, ctx),
            Page::Settings => settings::show(self, kit, ctx),
            Page::Mods => mods::show(self, kit, ctx),
            Page::Servers => servers::show(self, kit, ctx),
            Page::EchoVrce => echovrce::show(self, kit, ctx),
            Page::Friends => friends::show(self, kit, ctx),
            Page::Plugins => plugins::show_manage(self, kit, ctx),
            Page::Plugin(i) => plugins::show_page(self, kit, ctx, i),
        }
    }

    /// From the second frame on, draws one page that hasn't been shown yet per frame,
    /// unseen and inert, so its images are scaled in the background before it is first
    /// opened.
    fn prewarm(&mut self, kit: &mut Kit, ctx: &egui::Context, shift: f32) {
        if kit.assets.sync.get() || ctx.cumulative_frame_nr() == 0 {
            return;
        }
        let Some(page) = Page::ALL.into_iter().find(|p| !self.warmed.contains(p)) else {
            return;
        };
        self.warmed.insert(page);
        let rect = kit.ui.max_rect();
        let mut ui = kit
            .ui
            .new_child(egui::UiBuilder::new().max_rect(rect).invisible());
        let mut ghost = Kit::new(&mut ui, kit.assets, true);
        ghost.ghost = true;
        ghost.origin.x += shift;
        ghost.ex = (ghost.ex - shift).max(0.0);
        kit.assets.ghost.set(true);
        self.page_body(&mut ghost, ctx, page);
        kit.assets.ghost.set(false);
        ctx.request_repaint();
    }

    /// The design's rail: pages, a divider, EchoVRCE, Friends and the plugins' +, and
    /// Settings at the bottom. Positions are the icons' centres in design pixels.
    /// Why a page's rail icon has a dot, if it has one (its colour too).
    fn rail_badges(&self) -> Vec<(Page, String, egui::Color32)> {
        use crate::core::echovrce::game::FriendState;
        let mut out = Vec::new();
        if self.vrce.ended {
            out.push((
                Page::EchoVrce,
                "Signed out: sign in again".to_string(),
                design::QUEST_WARN,
            ));
        }
        let requests = self
            .servers
            .requests
            .iter()
            .filter(|f| f.state == FriendState::Received)
            .count();
        if requests > 0 {
            let s = if requests == 1 { "" } else { "s" };
            out.push((
                Page::Friends,
                format!("{requests} friend request{s}"),
                design::QUEST_ON,
            ));
        }
        let invites = self.servers.open_invites().len();
        if invites > 0 {
            let s = if invites == 1 { "" } else { "s" };
            out.push((
                Page::Servers,
                format!("{invites} match invite{s}"),
                design::QUEST_ON,
            ));
        }
        let games: Vec<(&str, &str)> = self
            .updates
            .findings
            .iter()
            .filter_map(|f| match f {
                crate::core::updates::Finding::Game { id, name, .. } => {
                    Some((id.as_str(), name.as_str()))
                }
                _ => None,
            })
            .collect();
        // Short: the reason fits under the name in the unfolded rail. Play shows the
        // chosen version's: another one's is named (the version menu marks it too).
        match games.as_slice() {
            [] => {}
            [(id, _)] if self.state.selected.as_deref() == Some(*id) => {
                out.push((Page::Play, "Update ready".into(), design::QUEST_ON))
            }
            [(_, name)] => out.push((Page::Play, format!("Update for {name}"), design::QUEST_ON)),
            more => out.push((
                Page::Play,
                format!("{} updates ready", more.len()),
                design::QUEST_ON,
            )),
        }
        match self.updates.plugins() {
            0 => {}
            1 => out.push((Page::Mods, "1 mod update".into(), design::QUEST_ON)),
            n => out.push((Page::Mods, format!("{n} mod updates"), design::QUEST_ON)),
        }
        if matches!(self.launcher_update, LauncherUpdate::Available(_)) {
            out.push((
                Page::Settings,
                "Launcher update".to_string(),
                design::QUEST_WARN,
            ));
        }
        out
    }

    /// How much wider than its icons the rail is right now (logical pixels): the unfolded
    /// rail's room, sliding in and out.
    fn rail_extra(&self, ctx: &egui::Context) -> f32 {
        let open = self.state.rail_open;
        let t = if self.demo {
            f32::from(u8::from(open))
        } else {
            ctx.animate_bool_with_time(egui::Id::new("rail-open"), open, RAIL_SLIDE)
        };
        // Eased: quick out, gentle landing.
        let t = 1.0 - (1.0 - t).powi(3);
        t * dz(RAIL_EXTRA)
    }

    /// The rail: the pages' icons, and the ≡ button that unfolds it (`shift` wider than its
    /// icons) to show each page's name, and why its icon has a dot, beside it.
    fn rail(&mut self, kit: &mut Kit, ctx: &egui::Context, shift: f32) {
        // Settings stays at the bottom.
        let settings_y = 1035.0 + kit.dy();
        let mut items = vec![
            (Page::Play, RailIcon::Image("icon_play.png", 24.0), 232.5),
            (Page::Install, RailIcon::Vector(Icon::Download, 30.0), 335.0),
            (Page::Mods, RailIcon::Vector(Icon::Mods, 30.0), 437.0),
            (Page::Servers, RailIcon::Vector(Icon::Globe, 33.0), 538.0),
            (
                Page::EchoVrce,
                RailIcon::Image("icon_echovrce.png", 38.0),
                692.0,
            ),
            (
                Page::Friends,
                RailIcon::Image("icon_community.png", 38.0),
                753.0,
            ),
        ];
        // Each plugin's tab, then the +, as many as fit above Settings.
        let room = ((settings_y - 61.0 - 814.0) / 61.0).floor().max(0.0) as usize;
        let mut cy = 814.0;
        for (i, p) in self
            .plugins
            .list
            .iter()
            .enumerate()
            .take(room.min(u8::MAX as usize))
        {
            let icon = RailIcon::Vector(plugins::rail_icon(&p.page), 30.0);
            items.push((Page::Plugin(i as u8), icon, cy));
            cy += 61.0;
        }
        items.push((Page::Plugins, RailIcon::Vector(Icon::Plus, 30.0), cy));
        items.push((
            Page::Settings,
            RailIcon::Vector(Icon::Gear, 34.0),
            settings_y,
        ));
        let badges = self.rail_badges();
        let badge = |page: Page| badges.iter().find(|(p, ..)| *p == page);
        // How far it is unfolded, 0..1.
        let t = (shift / dz(RAIL_EXTRA)).clamp(0.0, 1.0);
        let width = RAIL + shift;
        self.rail_background(kit, ctx, width);
        let open = self.state.rail_open;
        let tip = if open {
            "Fold the tabs' names away"
        } else {
            "Show the tabs' names"
        };
        if kit.rail_item("rail-fold", RailIcon::Menu(28.0, t), 130.0, false, tip) {
            self.state.rail_open = !open;
            self.save();
        }
        // The names, fading in as the rail opens, cut off at its edge.
        let names =
            (t > 0.01).then(|| (kit.window().min.x + width, (t * 1.6 - 0.6).clamp(0.0, 1.0)));
        let titles: Vec<String> = items
            .iter()
            .map(|(page, ..)| match page {
                Page::Plugin(i) => plugins::title(self, *i),
                _ => page.title().to_string(),
            })
            .collect();
        for ((page, icon, cy), title) in items.iter().cloned().zip(titles) {
            let tip = match badge(page) {
                Some((_, why, _)) => format!("{title}: {why}"),
                None => title.clone(),
            };
            let mut go = kit.rail_item(&format!("rail-{title}"), icon, cy, self.page == page, &tip);
            if let Some((edge, alpha)) = names {
                let x = 98.0;
                let room = dz(RAIL_EXTRA + RAIL_W - x - 14.0);
                let fade = |c: egui::Color32| c.gamma_multiply(alpha);
                let selected = self.page == page;
                let color = if selected {
                    design::TEXT
                } else {
                    egui::Color32::from_gray(200)
                };
                let lines = rail_name(kit, &title, fade(color), room);
                let why = badge(page).map(|(_, why, color)| {
                    kit.label_galley(why, design::din(13.0), fade(*color), room)
                });
                let name_h: f32 = lines.iter().map(|g| g.size().y).sum();
                let why_h = why.as_ref().map_or(0.0, |g| g.size().y + dz(2.0));
                let top = dz(cy) - (name_h + why_h) / 2.0;
                let clip = kit.ui.clip_rect();
                let mut cut = clip;
                cut.max.x = cut.max.x.min(edge);
                kit.ui.set_clip_rect(cut);
                let mut ly = top;
                for g in lines {
                    let h = g.size().y;
                    kit.put(dz(x), ly, g);
                    ly += h;
                }
                if let Some(g) = why {
                    kit.put(dz(x), top + name_h + dz(2.0), g);
                }
                kit.ui.set_clip_rect(clip);
                // The whole row answers, the name as the icon.
                let row = egui::Rect::from_min_max(
                    kit.drect(Dr::new(73.0, cy - 26.5, 0.0, 0.0)).min,
                    egui::pos2(
                        edge - dz(8.0),
                        kit.drect(Dr::new(0.0, cy + 26.5, 0.0, 0.0)).min.y,
                    ),
                );
                if row.width() > 0.0 {
                    go |= kit.click_area(&format!("rail-name-{title}"), row, &tip);
                }
            }
            if go {
                self.page = page;
            }
        }
        // The divider, as wide as the rail.
        kit.ui.painter().rect_filled(
            kit.drect(Dr::new(17.0, 624.0, 59.0, 3.0))
                .with_max_x(kit.window().min.x + width - dz(17.0)),
            dz(1.5),
            egui::Color32::from_rgb(142, 144, 143),
        );
        // A bright "!" on an icon whose page has news (its reason in the tip and under its
        // name), with a soft shadow so it still reads on a hovered or selected icon.
        for (page, _, color) in &badges {
            let Some((_, _, cy)) = items.iter().find(|(p, ..)| p == page) else {
                continue;
            };
            // Conthrax's "!", as on CHECK FOR UPDATES, centred on the icon's corner.
            let c = kit.drect(Dr::new(64.0, cy - 21.0, 0.0, 0.0)).min;
            let font = design::conthrax(34.0);
            let painter = kit.ui.painter();
            let mark = |off: egui::Vec2, color: egui::Color32| {
                painter.text(
                    c + off,
                    egui::Align2::CENTER_CENTER,
                    "!",
                    font.clone(),
                    color,
                );
            };
            for (dx, dy, a) in [(2.5, 3.0, 60), (1.5, 2.0, 100), (1.0, 1.0, 140)] {
                mark(
                    egui::vec2(dz(dx), dz(dy)),
                    egui::Color32::from_black_alpha(a),
                );
            }
            mark(
                egui::Vec2::ZERO,
                color.lerp_to_gamma(egui::Color32::WHITE, 0.3),
            );
        }
    }

    /// The rail's backdrop, `width` wide: the design's sidebar image, uncovered as it
    /// unfolds; a shadow on the page side while it is unfolded.
    fn rail_background(&self, kit: &mut Kit, ctx: &egui::Context, width: f32) {
        let h = H + kit.ey;
        // The image is as wide as the rail ever unfolds (450×1080): only its left part
        // shows, `width` of it.
        let tw = (h * 450.0 / 1080.0).max(width);
        let tex = kit
            .assets
            .tex(ctx, "left_sidebar.jpg", tw.round() as u32, h.round() as u32);
        let o = kit.window().min;
        let rect = egui::Rect::from_min_size(o, egui::vec2(width, h));
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(width / tw, 1.0));
        kit.ui
            .painter()
            .image(tex.id(), rect, uv, egui::Color32::WHITE);
        if width > RAIL + 0.5 {
            // A soft shadow on the page side, and a thin light edge.
            let edge = o.x + width;
            let a = ((width - RAIL) / dz(RAIL_EXTRA)).clamp(0.0, 1.0);
            let shadow = egui::Rect::from_min_max(
                egui::pos2(edge, o.y),
                egui::pos2(edge + dz(24.0), o.y + h),
            );
            let mut mesh = egui::Mesh::default();
            let dark = egui::Color32::from_black_alpha((90.0 * a) as u8);
            mesh.colored_vertex(shadow.left_top(), dark);
            mesh.colored_vertex(shadow.right_top(), egui::Color32::TRANSPARENT);
            mesh.colored_vertex(shadow.right_bottom(), egui::Color32::TRANSPARENT);
            mesh.colored_vertex(shadow.left_bottom(), dark);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            kit.ui.painter().add(mesh);
            kit.ui.painter().vline(
                edge,
                o.y..=o.y + h,
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(150, 110, 230, (70.0 * a) as u8),
                ),
            );
        }
    }

    /// The Quest chip's text and colour.
    fn quest_chip(&self) -> (&'static str, egui::Color32) {
        match (self.quest_conn.checking, self.quest_conn.status) {
            (true, _) => ("Quest: checking...", design::QUEST_OFF),
            (_, Some(Status::Ready)) => ("Quest: connected", design::QUEST_ON),
            (_, Some(Status::Unauthorized)) => ("Quest: allow this PC", design::QUEST_WARN),
            (_, Some(Status::Ambiguous)) => ("Quest: pick a device", design::QUEST_WARN),
            (_, Some(Status::None)) => ("Quest: not connected", design::QUEST_OFF),
            (_, None) => ("Quest: not checked", design::QUEST_OFF),
        }
    }

    /// The PCVR side's state, for the status bar's chip.
    fn pc_chip(&mut self) -> (&'static str, egui::Color32) {
        // The version PLAY starts being installed (another one's download is the status
        // line's to say).
        let target = match self.target() {
            Target::Installed(v) | Target::Missing(v) => Some(v.id),
            Target::Available(e) => Some(e.id),
            Target::None => None,
        };
        if target
            .and_then(|id| self.jobs.get(&id))
            .is_some_and(|j| matches!(j.kind, JobKind::Install | JobKind::Reinstall))
        {
            return ("PCVR: installing", design::BLUE);
        }
        match self.target() {
            Target::Installed(v) => {
                let needs_steamvr = self.steamvr_missing();
                if setup::needs_patch(self, &v) {
                    ("PCVR: needs the patch", design::QUEST_WARN)
                } else if needs_steamvr && self.state.profile.runtime == Runtime::VirtualDesktop {
                    ("PCVR: set up EchoXR", design::QUEST_WARN)
                } else if needs_steamvr {
                    ("PCVR: set up SteamVR", design::QUEST_WARN)
                } else if cfg!(target_os = "macos") && !self.demo {
                    ("PCVR: not on macOS", design::QUEST_OFF)
                } else {
                    ("PCVR: ready", design::QUEST_ON)
                }
            }
            Target::Missing(_) => ("PCVR: files missing", design::RED),
            Target::Available(_) | Target::None => ("PCVR: not installed", design::QUEST_OFF),
        }
    }

    /// The status bar: a chip for the side in use (PCVR: the install, click to install;
    /// Quest: the connection, click to check it) and what the game or the running job is
    /// doing. On Play and Install it spans the main column only.
    /// The status bar's chip while the launcher isn't on main: the channel followed, else
    /// (main picked again, not switched back yet) the running build's own. Its text, colour
    /// and tip.
    /// Takes `code` as the dev code (`None`: none): saved, both catalogues read again, and
    /// its folder checked for Advanced settings' line.
    pub(super) fn set_dev_code(&mut self, ctx: &egui::Context, code: Option<String>) {
        use crate::core::launcher::dev_code;
        self.state.dev_code = code.clone();
        self.save();
        self.dev_code_field = code.as_deref().map(dev_code::display).unwrap_or_default();
        self.mods.reload_catalog();
        self.plugins.reload();
        self.updates.check_soon();
        self.dev_status = dev_code::Status::Off;
        self.check_dev_code(ctx);
    }

    /// Reads the dev code's folder (for Advanced settings' line), once per code.
    pub(super) fn check_dev_code(&mut self, ctx: &egui::Context) {
        use crate::core::launcher::dev_code::{self, Status};
        let Some(code) = self.state.dev_code.as_deref().and_then(dev_code::normalize) else {
            self.dev_status = Status::Off;
            return;
        };
        if self.dev_status != Status::Off || self.demo {
            return;
        }
        self.dev_status = Status::Checking;
        self.worker
            .spawn(ctx, move |tx| tx.send(Msg::DevCode(dev_code::check(&code))));
    }

    fn channel_chip(&self) -> Option<(String, egui::Color32, String)> {
        use update_check::Channel;
        let version = env!("CARGO_PKG_VERSION");
        let followed = self.state.launcher_channel;
        // Screenshots show the channel picked, not the build they were made with.
        let built = if self.demo {
            Channel::Main
        } else {
            update_check::running_channel()
        };
        let (text, channel, tip) = if followed != Channel::Main {
            (
                format!("{} channel", followed.name()),
                followed,
                format!(
                    "Launcher {version} on the {} channel. Click to change it.",
                    followed.name()
                ),
            )
        } else if built != Channel::Main {
            (
                format!("{} build", built.name()),
                built,
                format!(
                    "Launcher {version} is a {} build. Click to switch back to main's.",
                    built.name()
                ),
            )
        } else {
            return None;
        };
        let color = match channel {
            Channel::Alpha => design::RED,
            _ => design::QUEST_WARN,
        };
        Some((text, color, tip))
    }

    fn top_bar(&mut self, kit: &mut Kit, ctx: &egui::Context) {
        let bar = if matches!(self.page, Page::Play | Page::Install | Page::Settings) {
            Dr::new(138.0, 15.0, 1144.0, 43.0)
        } else {
            Dr::new(138.0, 15.0, 1734.0, 43.0)
        }
        .wider(kit.dx());
        kit.ui
            .painter()
            .rect_filled(kit.drect(bar), dz(6.0), design::BAR);

        let (qtext, qcolor) = match self.platform {
            Platform::Pc => self.pc_chip(),
            Platform::Quest => self.quest_chip(),
        };
        let g = kit.spaced_galley(
            &qtext.to_uppercase(),
            design::din(12.0),
            design::TEXT,
            dz(0.5),
            false,
        );
        let chip = Dr::new(149.0, 23.0, g.size().x / dz(1.0) + 20.0, 27.0);
        let r = kit.drect(chip);
        kit.ui.painter().rect_filled(r, dz(4.0), qcolor);
        kit.ui
            .painter()
            .galley(r.center() - g.size() / 2.0, g, design::TEXT);
        match self.platform {
            Platform::Pc => {
                let to_install = !matches!(self.target(), Target::Installed(_));
                let tip = if to_install {
                    "Open the Install page"
                } else {
                    "Echo VR on this PC"
                };
                if kit.hot("pc-chip", r, to_install, tip).0.clicked {
                    self.page = Page::Install;
                }
            }
            Platform::Quest => {
                if kit
                    .hot(
                        "quest-chip",
                        r,
                        !self.quest_conn.checking,
                        "Check the USB connection to your Quest",
                    )
                    .0
                    .clicked
                {
                    self.check_quest(ctx, true);
                }
            }
        }

        // Not on main: the launcher's channel, at the bar's right end.
        let mut status_end = bar.right() - 12.0;
        if let Some((text, color, tip)) = self.channel_chip() {
            let g = kit.spaced_galley(
                &text.to_uppercase(),
                design::din(12.0),
                design::TEXT,
                dz(0.5),
                false,
            );
            let w = g.size().x / dz(1.0) + 20.0;
            let chip = Dr::new(bar.right() - 11.0 - w, 23.0, w, 27.0);
            let r = kit.drect(chip);
            kit.ui.painter().rect_filled(r, dz(4.0), color);
            kit.ui
                .painter()
                .galley(r.center() - g.size() / 2.0, g, design::TEXT);
            if kit.hot("channel-chip", r, true, &tip).0.clicked {
                self.overlay = Some(setup::Overlay::Advanced);
            }
            status_end = chip.x - 12.0;
        }
        // A dev code: unpublished mods and plugins are listed too.
        if self.state.dev_code.is_some() {
            let g = kit.spaced_galley("DEV CODE", design::din(12.0), design::TEXT, dz(0.5), false);
            let w = g.size().x / dz(1.0) + 20.0;
            let chip = Dr::new(status_end + 1.0 - w, 23.0, w, 27.0);
            let r = kit.drect(chip);
            kit.ui.painter().rect_filled(r, dz(4.0), design::RIM_TOP);
            kit.ui
                .painter()
                .galley(r.center() - g.size() / 2.0, g, design::TEXT);
            let tip = "A dev code is set: your dev folder's unpublished mods and plugins are listed too. Click to change it.";
            if kit.hot("dev-code-chip", r, true, tip).0.clicked {
                self.overlay = Some(setup::Overlay::Advanced);
            }
            status_end = chip.x - 12.0;
        }

        let game = self.game();
        // On the Quest side: the headset's game, as its API says over the network.
        let quest_game = (self.platform == Platform::Quest)
            .then(|| self.quest_game())
            .filter(GameState::is_running);
        let busy = self.any_job() || self.quest_busy || self.quest_conn.checking;
        const NOTICE_FOR: std::time::Duration = std::time::Duration::from_secs(8);
        let notice = self
            .notice
            .as_ref()
            .filter(|(_, at)| at.elapsed() < NOTICE_FOR)
            .map(|(text, at)| {
                ctx.request_repaint_after(NOTICE_FOR.saturating_sub(at.elapsed()));
                text.clone()
            });
        let status = if let Some(text) = &notice {
            text.clone()
        } else if let Some((j, more)) = self
            .jobs_in_order()
            .split_first()
            .map(|(j, rest)| (*j, rest.len()))
        {
            // The install before the patch it waits for; the others as a count.
            let line = match j.fraction {
                Some(f) => format!("{}   ·   {:.0}%", j.title, f * 100.0),
                None => format!("{}   ·   {}", j.title, j.label.replace("...", "…")),
            };
            match more {
                0 => line,
                n => format!("{line}   ·   +{n} more"),
            }
        } else if let Some(q) = &quest_game {
            match q {
                GameState::InMatch { .. } => "In a match on your Quest".to_string(),
                _ => "Echo VR is running on your Quest".to_string(),
            }
        } else if let (false, Some(queue)) = (game.is_running(), self.servers.queue_line()) {
            queue
        } else if let (true, Some(since)) = (game.is_running(), self.game_since) {
            let mins = since.elapsed().as_secs() / 60;
            ctx.request_repaint_after(std::time::Duration::from_secs(20));
            if mins == 0 {
                game.label()
            } else {
                format!("{}   ·   {mins} min", game.label())
            }
        } else if self.ours() && !game.is_running() {
            // PLAY was clicked: as Play's line says, not "not running".
            match &self.launch_note {
                Some(n) => format!("{n}…"),
                None => "Starting Echo VR…".into(),
            }
        } else {
            game.label()
        };
        let color = if notice.is_some() || game.is_running() || quest_game.is_some() {
            design::QUEST_ON
        } else if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            style::mix(design::GREY, design::TEXT, style::pulse(ctx))
        } else {
            design::GREY
        };
        let x = dz(chip.right() + 12.0);
        let g = kit.spaced_fit(
            &status.to_uppercase(),
            design::din(13.8),
            color,
            dz(0.5),
            false,
            dz(status_end) - x,
        );
        let y = dz(bar.y + bar.h / 2.0) - g.size().y / 2.0;
        let max_w = dz(status_end) - x;
        kit.clipped(x, 0.0, max_w, dz(bar.bottom()), |kit| {
            kit.put(x, y, g);
        });
    }
}

/// The "continue in your browser" dialog while a job waits on Discord's authorization.
const BROWSER_KEY: &str = "browser-wait";

/// What that dialog says, for job `id`: why the page opened and what to do there.
fn browser_dialog(id: &str) -> (&'static str, String) {
    let what = if id == setup::QUEST_JOB {
        "a patched Echo VR for your Quest"
    } else {
        "your personal licence patch, which new players need to play Echo VR on PC"
    };
    (
        "Continue in your browser",
        format!(
            "Discord's authorization page just opened in your browser. The Echo VR Patcher bot uses it to build {what} for your Discord account.\n\nAuthorize it there (you need to be a member of its Discord server), then come back here: the launcher picks it up by itself.\n\nNo page in your browser? Open it again, or copy the link into your browser yourself."
        ),
    )
}

/// A centred card for pages that are not built yet.
fn empty_state(kit: &mut Kit, icon: Icon, title: &str, text: &str) {
    let (w, h) = (dz(780.0), dz(336.0));
    let x = X0 + (CW + kit.ex - w) / 2.0;
    let y = dz(300.0) + kit.ey / 2.0;
    kit.panel(x, y, w, h);
    let s = dz(60.0);
    let top = kit.rect(x + (w - s) / 2.0, y + dz(40.0), s, s).min;
    style::icon_at(kit.ui.painter(), icon, top, s, design::TEXT);
    let g = kit.spaced_galley(
        &title.to_uppercase(),
        design::conthrax(27.0),
        design::TEXT,
        dz(4.0),
        false,
    );
    let tx = x + (w - g.size().x) / 2.0;
    kit.put(tx, y + dz(122.0), g);
    let pad = dz(60.0);
    let paras = kit.caps_block(text, 17.0, design::BODY, w - 2.0 * pad);
    let mut ty = y + dz(180.0);
    for g in paras {
        let gx = x + (w - g.size().x) / 2.0;
        ty += kit.put(gx, ty, g).height();
    }
    let cw = kit.chip_width("Coming soon");
    kit.chip(
        x + (w - cw) / 2.0,
        y + h - dz(62.0),
        "Coming soon",
        design::BLUE,
    );
}

/// Two made-up versions for UI snapshots.
fn demo_state() -> LauncherState {
    let mut s = LauncherState {
        imported: true,
        // Snapshots: ECHOVR_SNAPSHOTS_RAIL=1 takes every page with the rail unfolded.
        rail_open: std::env::var_os("ECHOVR_SNAPSHOTS_RAIL").is_some(),
        ..Default::default()
    };
    s.versions.push(InstalledVersion {
        id: "pc-latest".into(),
        name: "Echo VR (PC, latest)".into(),
        root: "C:/EchoVR/versions/pc-latest".into(),
        catalog_id: Some("pc-latest".into()),
        ..Default::default()
    });
    s.versions.push(InstalledVersion {
        id: "existing".into(),
        name: "Existing install".into(),
        root: "C:/Program Files/Oculus/Software/Software".into(),
        external: true,
        ..Default::default()
    });
    s.selected = Some("pc-latest".into());
    s.profile.windowed = false;
    s.quest_ip = Some("192.168.178.45".into());
    s
}

/// A tab's name in the unfolded rail, `room` wide: on two lines when it doesn't fit on
/// one (a plugin's name can be long), at the size of the others; smaller only when a
/// word alone is too wide even so.
fn rail_name(
    kit: &Kit,
    title: &str,
    color: egui::Color32,
    room: f32,
) -> Vec<std::sync::Arc<egui::Galley>> {
    let words: Vec<&str> = title.split_whitespace().collect();
    let line =
        |text: &str, size: f32| kit.label_galley(text, design::din(size), color, f32::INFINITY);
    for size in [19.0, 17.0, 15.0] {
        let whole = line(title, size);
        if whole.size().x <= room {
            return vec![whole];
        }
        // Two lines: as many words on the first as fit, the rest on the second.
        for n in (1..words.len()).rev() {
            let first = line(&words[..n].join(" "), size);
            if first.size().x > room {
                continue;
            }
            let second = line(&words[n..].join(" "), size);
            if second.size().x <= room {
                return vec![first, second];
            }
            break;
        }
    }
    vec![kit.label_galley(title, design::din(15.0), color, room)]
}

/// The built-in catalogue plus an older build that is not installed, for snapshots.
fn demo_catalog() -> Catalog {
    let mut c = Catalog::builtin();
    c.versions.push(VersionEntry {
        id: "pc-34.4".into(),
        name: "Echo VR 34.4 (PC)".into(),
        channel: "archive".into(),
        platform: Platform::Pc,
        url: "ready-at-dawn-echo-arena.zip".into(),
        size: Some(4_270_000_000),
        notes: "The last official build.".into(),
        summary: "The last official build".into(),
        ..Default::default()
    });
    c
}

#[cfg(test)]
mod tests {
    use super::{clash, Res};

    fn v(id: &str) -> Res {
        Res::Version(id.into())
    }

    #[test]
    fn a_download_holds_only_its_version() {
        let install_beta = [v("pc-beta")];
        // The live build can be updated, patched and modded meanwhile.
        assert!(!clash(&install_beta, &[v("pc-latest")]));
        assert!(!clash(&install_beta, &[v("pc-latest"), Res::Licence]));
        assert!(!clash(&install_beta, &[v("pc-latest"), Res::Mods]));
        // ...but not the version being installed.
        assert!(clash(&install_beta, &[v("pc-beta"), Res::Mods]));
    }

    #[test]
    fn shared_things_wait_for_each_other() {
        // VR setup, the headset, the mods' downloads.
        assert!(clash(&[Res::Vr], &[Res::Vr, v("pc-latest")]));
        assert!(!clash(&[Res::Vr], &[v("pc-latest")]));
        assert!(clash(&[Res::Quest], &[Res::Quest]));
        assert!(!clash(&[Res::Quest], &[v("pc-latest"), Res::Vr]));
        assert!(clash(&[v("a"), Res::Mods], &[v("b"), Res::Mods]));
        // An event build's install sets up EchoRelay's files: another event build's setup
        // waits, the VR setup doesn't.
        assert!(clash(
            &[v("pc-event"), Res::Relay],
            &[Res::Relay, v("pc-event-2")]
        ));
        assert!(!clash(&[v("pc-event"), Res::Relay], &[Res::Vr]));
    }

    #[test]
    fn the_launcher_update_and_uninstall_wait_for_everything() {
        assert!(clash(&[Res::All], &[Res::Quest]));
        assert!(clash(&[v("pc-beta")], &[Res::All]));
        assert!(clash(&[Res::Licence], &[Res::All]));
    }
}
