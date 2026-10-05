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
        url: "https://github.com/EchoTools/EchoXR",
        credits: "https://github.com/EchoTools/EchoXR/blob/main/CREDITS.md",
    },
    Credit {
        name: "EchoXR Hands",
        by: crate::core::echoxr_hands::AUTHORS,
        what: "Your own fingers on Echo VR's hands, from OpenXR hand tracking.",
        licence: "",
        url: "https://github.com/EchoTools/EchoXR-Hands",
        credits: "https://github.com/EchoTools/EchoXR-Hands/blob/main/CREDITS.md",
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
const RAIL_EXTRA: f32 = 190.0;
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
    /// Community plugins (the rail's +), not built yet.
    Plugins,
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
    QuestInstalled(crate::core::launcher::quest::Installed),
    QuestUpdated,
    /// The headset's APK doesn't match the update: offer a reinstall (the text says why).
    QuestNeedsReinstall(String),
    /// Mods were installed, added or removed: what to say.
    ModsChanged(String),
    /// Parts of the launcher's things were uninstalled.
    Uninstalled(crate::core::uninstall::Outcome),
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
    FeedStatus(Option<feed::Servers>),
    FeedNews(Option<feed::News>),
    /// A feed image by file name (`None`: it couldn't be loaded).
    FeedImage(String, Option<image::RgbaImage>),
    /// Free bytes in a library folder.
    FreeSpace(String, Option<u64>),
    /// Where Revive is installed.
    Revive(Option<String>),
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
    LauncherUpdate(Result<Option<update_check::Release>, String>),
    /// Who opens spark:// links now (`Err`: registering failed, and why).
    LinkHandler(links::Handler, Option<String>),
    /// The headset's logs were saved into this folder, or why not.
    QuestLogs(Result<PathBuf, UiError>),
    /// The logs were uploaded: the service's reference, or why not.
    LogsUploaded(Result<String, String>),
    /// A version's mods, read (for the Mods page's read number `gen`).
    ModView(u64, String, crate::core::launcher::mods::ModView),
    ModCatalog(crate::core::launcher::mods::ModCatalog),
}

/// Whether a newer launcher is out (Settings shows it, the rail marks it).
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
}

struct Job {
    kind: JobKind,
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
    /// The rail unfolded.
    RailOpen,
    /// Settings: what to uninstall.
    Uninstall,
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
    /// The game's log of the start PLAY made, for what EchoVRCE says to a login.
    login_watch: Option<play::LoginWatching>,
    /// The lobby to join once "Launch anyway" is answered.
    pending_lobby: Option<play::Join>,
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
    /// echovrce.com inside the window, on the EchoVRCE page.
    web: crate::ui::web::WebPane,
    /// The game service's servers, party, friends and history (the Servers page).
    servers: servers::Servers,
    /// The selected version's mods and the mods catalogue (the Mods page).
    mods: mods::Mods,
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
    /// What the uninstall card asked to remove, until its confirmation is answered.
    uninstall_parts: Vec<crate::core::uninstall::Part>,
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
            }
        }
        self.library_field = self.state.library.clone();
        self.quest_ip_field = self.state.quest_ip.clone().unwrap_or_default();
        self.relay_server_field = self.state.relay_server.clone();
        self.linux_set_up = cfg!(target_os = "linux") && crate::core::linux::echoxr::is_set_up();
        if cfg!(target_os = "linux")
            && !self.demo
            && setup::linux_runtime(&mut self.state.profile.runtime)
        {
            self.save();
        }
        self.check_launcher_update(ctx);
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
        let Some(link) = links::take_incoming().filter(|l| links::parse(l).is_some()) else {
            return;
        };
        tracing::info!("opening a spark:// link");
        self.page = Page::Play;
        self.overlay = Some(setup::Overlay::JoinLobby { input: link });
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
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

    /// Revive (for SteamVR) is known not to be installed.
    fn revive_missing(&mut self) -> bool {
        self.demo || self.revive.get(()).is_some_and(|dir| dir.is_none())
    }

    /// EchoXR (for SteamVR) is known not to be fetched.
    fn echoxr_missing(&mut self) -> bool {
        self.demo || self.echoxr.get(()).is_some_and(|ready| !ready)
    }

    /// SteamVR is the choice, on Windows, and isn't set up the way it runs yet (Revive's
    /// injector, or EchoXR).
    fn steamvr_missing(&mut self) -> bool {
        if self.state.profile.runtime != Runtime::Revive || !(cfg!(windows) || self.demo) {
            return false;
        }
        match self.state.profile.steamvr_via {
            SteamVrVia::Revive => self.revive_missing(),
            SteamVrVia::EchoXr => self.echoxr_missing(),
        }
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

    /// Looks for a newer launcher release in the background.
    fn check_launcher_update(&mut self, ctx: &egui::Context) {
        if self.demo {
            self.launcher_update = LauncherUpdate::Latest;
            return;
        }
        self.launcher_update = LauncherUpdate::Checking;
        self.worker.spawn(ctx, |tx| {
            let r = update_check::newer().map_err(|e| format!("{e:#}"));
            tx.send(Msg::LauncherUpdate(r));
        });
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
        self.snap_game = None;
        self.install_pick = None;
        self.snap_found.clear();
        self.state.versions = demo_state().versions;
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
            Some(SnapVariant::Uninstall) => settings::ask_uninstall(self),
            Some(SnapVariant::Credits) => {
                self.overlay = Some(setup::Overlay::Credits { scroll: 0.0 })
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
            Some(SnapVariant::SettingsSteamVr) => self.state.profile.runtime = Runtime::Revive,
            Some(SnapVariant::SettingsEchoXr) => {
                self.state.profile.runtime = Runtime::Revive;
                self.state.profile.steamvr_via = SteamVrVia::EchoXr;
            }
            Some(SnapVariant::SettingsLinks) => {
                self.link_handler = Some(links::Handler::Other("Spark".into()))
            }
            Some(SnapVariant::VrceSignedIn) => self.vrce.demo(true),
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
            Some(SnapVariant::ModsNoLoader | SnapVariant::ModsOptions) => {}
            Some(SnapVariant::QuestFresh) => {
                self.platform = Platform::Quest;
                if let Some(i) = &mut self.quest_info {
                    i.installed = false;
                    i.marker = None;
                }
            }
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
                    // EchoXR.exe ended before the game showed up: it says why (later on,
                    // its exit code is Echo's own).
                    let starting = self.launched.is_some_and(|l| !l.seen);
                    let why = status
                        .code()
                        .and_then(crate::core::echoxr::exit_message)
                        .filter(|_| self.child_echoxr && starting);
                    self.child = None;
                    if let Some(why) = why {
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
                    let kind = self.jobs.remove(&id).map(|j| j.kind);
                    self.job_done(ctx, &id, kind, r);
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
                        "This step needs administrator rights (it installs into Program Files).\n\nStart the privileged helper now? Windows will ask you to confirm.",
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
                Msg::LogsUploaded(r) => {
                    self.uploading_logs = false;
                    match r {
                        Ok(code) => settings::logs_sent(self, ctx, &code),
                        Err(why) => self.dialogs.error(
                            "Couldn't upload your logs",
                            &why,
                            Default::default(),
                        ),
                    }
                }
                Msg::QuestNetwork(r) => match r {
                    Ok(()) => self.notify("ADB over the network is on: you can unplug your Quest"),
                    Err(e) => self.dialogs.error_ui(&e),
                },
                Msg::LauncherUpdate(r) => {
                    self.launcher_update = match r {
                        Ok(Some(release)) => LauncherUpdate::Available(release),
                        Ok(None) => LauncherUpdate::Latest,
                        Err(e) => {
                            tracing::info!("launcher update check failed: {e}");
                            LauncherUpdate::Failed
                        }
                    }
                }
                Msg::ModView(gen, id, view) => self.mods.read_done(gen, id, view),
                Msg::ModCatalog(c) => self.mods.catalog_done(c),
                Msg::Revive(dir) => self.revive.done((), dir),
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
        play::watch_login(self, ctx);
        self.take_link(ctx);
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

    fn job_done(&mut self, ctx: &egui::Context, id: &str, kind: Option<JobKind>, r: JobResult) {
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
        if install_failed && self.pending_patch.as_ref().is_some_and(|p| p.version == id) {
            self.pending_patch = None;
            self.cancel_job(setup::LICENCE_JOB);
        }
        if id == setup::QUEST_JOB {
            // Read the headset again once the job is over.
            self.quest_info = None;
        }
        // PLAY prepared VR first: the game starts once that's done (not after a failure).
        let play_after = [setup::REVIVE_JOB, setup::ECHOXR_JOB, setup::LINUX_JOB].contains(&id)
            && std::mem::take(&mut self.play_after_prep);
        let start_now = play_after
            && matches!(
                r,
                JobResult::ReviveReady(_) | JobResult::EchoXrReady | JobResult::LinuxReady(_)
            );
        match r {
            JobResult::Installed(v, update_failed) => {
                let name = v.name.clone();
                if self.state.selected.is_none() {
                    self.state.selected = Some(v.id.clone());
                }
                self.state.upsert(v);
                self.save();
                // A new player's patch goes in now, if it is already here.
                setup::apply_pending(self, ctx);
                match update_failed {
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
            JobResult::Reinstalled(r) => {
                let name = r.version.name.clone();
                self.state.upsert(r.version);
                self.save();
                setup::apply_pending(self, ctx);
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
            JobResult::ModsChanged(notice) => {
                self.mods.changed();
                self.notify(&notice);
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
            JobResult::EchoXrReady => {
                self.echoxr = Probe::default();
                if !play_after {
                    self.notify("SteamVR through EchoXR is ready: PLAY starts Echo VR through it");
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
            JobResult::Failed(None) => {}
            JobResult::Failed(Some(e)) => {
                if kind == Some(JobKind::Update) {
                    self.update_note
                        .insert(id.to_string(), "Last update failed".into());
                }
                self.dialogs.error_ui(&e);
            }
        }
        if start_now {
            play::try_start(self, ctx, None);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_job(
        &mut self,
        ctx: &egui::Context,
        kind: JobKind,
        id: &str,
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
        // The page right of the unfolded rail, in the room the zoom made for it.
        let shift = self.rail_extra(&ctx).min(kit.ex);
        kit.origin.x += shift;
        kit.ex -= shift;
        self.page_body(kit, &ctx, self.page);
        self.warmed.insert(self.page);
        self.top_bar(kit, &ctx);
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
        if !matches!(
            page,
            Page::Play | Page::Install | Page::Settings | Page::Servers | Page::Friends
        ) {
            let h = HEADER.wider(kit.dx());
            kit.header_strip(dz(h.x), dz(h.y), dz(h.w), dz(h.h), page.title());
        }
        match page {
            Page::Play => play::show(self, kit, ctx),
            Page::Install => install::show(self, kit, ctx),
            Page::Settings => settings::show(self, kit, ctx),
            Page::Mods => mods::show(self, kit, ctx),
            Page::Servers => servers::show(self, kit, ctx),
            Page::EchoVrce => echovrce::show(self, kit, ctx),
            Page::Friends => friends::show(self, kit, ctx),
            Page::Plugins => empty_state(
                kit,
                Icon::Plus,
                "Plugins",
                "Community plugins will be listed here. Until then, Mods → Additional Plugins has the ones you can get.",
            ),
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
        if matches!(self.launcher_update, LauncherUpdate::Available(_)) {
            out.push((
                Page::Settings,
                "Launcher update available".to_string(),
                design::QUEST_WARN,
            ));
        }
        out
    }

    /// How much wider than its icons the rail is right now (logical pixels): the unfolded
    /// rail's room, sliding in and out.
    pub fn rail_extra(&self, ctx: &egui::Context) -> f32 {
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
        let items = [
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
            (Page::Plugins, RailIcon::Vector(Icon::Plus, 30.0), 814.0),
            (
                Page::Settings,
                RailIcon::Vector(Icon::Gear, 34.0),
                settings_y,
            ),
        ];
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
        for (page, icon, cy) in items {
            let tip = match badge(page) {
                Some((_, why, _)) => format!("{}: {why}", page.title()),
                None => page.title().to_string(),
            };
            let mut go = kit.rail_item(
                &format!("rail-{}", page.title()),
                icon,
                cy,
                self.page == page,
                &tip,
            );
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
                let name = kit.label_galley(page.title(), design::din(19.0), fade(color), room);
                let why = badge(page).map(|(_, why, color)| {
                    kit.label_galley(why, design::din(13.0), fade(*color), room)
                });
                let name_h = name.size().y;
                let why_h = why.as_ref().map_or(0.0, |g| g.size().y + dz(2.0));
                let top = dz(cy) - (name_h + why_h) / 2.0;
                let clip = kit.ui.clip_rect();
                let mut cut = clip;
                cut.max.x = cut.max.x.min(edge);
                kit.ui.set_clip_rect(cut);
                kit.put(dz(x), top, name);
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
                    go |= kit.click_area(&format!("rail-name-{}", page.title()), row, &tip);
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
        // A dot on an icon whose page has news (its reason in the tip and under its name).
        for (page, _, color) in &badges {
            let Some((_, _, cy)) = items.iter().find(|(p, ..)| p == page) else {
                continue;
            };
            let c = kit.drect(Dr::new(63.0, cy - 21.0, 0.0, 0.0)).min;
            kit.ui.painter().circle_filled(c, dz(6.0), *color);
        }
    }

    /// The rail's backdrop, `width` wide: the design's sidebar image, stretched as it
    /// unfolds; a shadow on the page side while it is unfolded.
    fn rail_background(&self, kit: &mut Kit, ctx: &egui::Context, width: f32) {
        let h = H + kit.ey;
        let tex = kit.assets.tex(
            ctx,
            "left_sidebar.jpg",
            RAIL.round() as u32,
            h.round() as u32,
        );
        let o = kit.window().min;
        let rect = egui::Rect::from_min_size(o, egui::vec2(width, h));
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
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
        if self
            .jobs
            .values()
            .any(|j| matches!(j.kind, JobKind::Install | JobKind::Reinstall))
        {
            return ("PCVR: installing", design::BLUE);
        }
        match self.target() {
            Target::Installed(v) => {
                let needs_steamvr = self.steamvr_missing();
                if setup::needs_patch(self, &v) {
                    ("PCVR: needs the patch", design::QUEST_WARN)
                } else if needs_steamvr {
                    ("PCVR: set up SteamVR", design::QUEST_WARN)
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
        } else if let Some(j) = self
            .jobs
            .values()
            .find(|j| j.kind != JobKind::Licence)
            .or_else(|| self.jobs.values().next())
        {
            // The install before the patch it waits for.
            match j.fraction {
                Some(f) => format!("{}   ·   {:.0}%", j.title, f * 100.0),
                None => format!("{}   ·   {}", j.title, j.label),
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
            dz(bar.right() - 12.0) - x,
        );
        let y = dz(bar.y + bar.h / 2.0) - g.size().y / 2.0;
        let max_w = dz(bar.right() - 12.0) - x;
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
        rail_open: false,
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
    s.profile.windowed = true;
    s.quest_ip = Some("192.168.178.45".into());
    s
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
