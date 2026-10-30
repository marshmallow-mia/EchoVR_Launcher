//! Play page, as the design concept has it: the ECHO VR logo over an info line, PLAY,
//! CHECK FOR UPDATES and the PCVR|QUEST switch, Community News (a banner and two cards)
//! and RIGHT NOW on the right (what is going on, and your friends). PLAY does what the selected version needs (patch, set up
//! SteamVR, play, stop); on the Quest side it connects or starts Echo VR on the headset.
//! With nothing installed it is grey and opens the Install page. While a job runs on the
//! installed game, PLAY fills up with its progress and CHECK FOR UPDATES becomes CANCEL.

use egui::Color32;

use super::hero::{self, Face, InfoLine, JobView, PathClick, Row, Side};
use super::{now, panel, setup, versions, Dashboard, Msg, Page, QueuedUpdate, Res};
use crate::core::adb::devices::Status;
use crate::core::error::UiError;
use crate::core::launcher::catalog::{Platform, VersionEntry};
use crate::core::launcher::feed::NewsItem;
use crate::core::launcher::login_watch::LoginWatch;
use crate::core::launcher::store::{
    InstalledVersion, LaunchProfile, Runtime, SteamVrVia, Target, VdVia,
};
use crate::core::launcher::{launch, quest, ready, relay};
use crate::core::revive;
use crate::ui::design::{self, dz, Dr};
use crate::ui::dialogs::{Answer, Icon as DlgIcon};
use crate::ui::kit::Kit;
use crate::ui::markdown::{self, Look};
use crate::ui::style::{self, Icon};
use crate::ui::widgets::{MenuItem, Tone, BTN_H};
use egui::pos2;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const LAUNCH_ANYWAY: &str = "launch-anyway";
/// A JOIN with an event build picked: the live build instead?
const JOIN_LIVE: &str = "join-live";
/// A JOIN of an EchoVRCE match while a plugin's server is chosen: back to EchoVRCE?
const JOIN_ECHOVRCE: &str = "join-echovrce";
/// Virtual Desktop's Oculus mode failed: through EchoXR instead?
const VD_SWITCH: &str = "vd-switch";
/// An event build on Linux while Settings says "No headset": which VR to play it in.
const EVENT_VR: &str = "event-vr";
/// Echo VR's mod loader (`BugSplat64.dll`) was removed or blocked: allow it and repair?
const NEVR_BLOCKED: &str = "nevr-blocked";
/// PLAY on Linux would add Echo VR to Steam (Steam restarts): asked once.
const PREPARE_LINUX: &str = "prepare-linux";
/// A content pack stays off for changed game files: repair them? (On its own.)
const PACK_DATA: &str = "pack-data";
/// The same at PLAY, which can also go on without the pack.
const PACK_DATA_PLAY: &str = "pack-data-play";

/// The info line's right limit (design pixels).
const INFO_RIGHT: f32 = 1282.0;
const NEWS: Dr = Dr::new(138.0, 350.0, 1144.0, 421.0);
/// news_header.png is the top 75 of CommunityNewsTab's 697 px.
const NEWS_HEADER_H: f32 = 421.0 * 75.0 / 697.0;
/// The news link's right end and baseline, at the banner's bottom right.
const NEWS_LINK: (f32, f32) = (1226.0, 727.0);
/// The design banner's lettering (news_fallback_text.png): its left edge, width and
/// height, at the scale of its background in a 1280×720 window.
const NEWS_LETTERING: (f32, f32, f32) = (NEWS.x + 48.0, 439.0, 275.0);
const CARDS: [Dr; 2] = [
    Dr::new(137.0, 800.0, 555.0, 248.0),
    Dr::new(728.0, 800.0, 555.0, 248.0),
];
/// The version picker right of the PCVR|QUEST switch: its body (as tall as the switch's)
/// and its caption, level with the switch's.
const PICKER: Dr = Dr::new(878.0, 265.0, 260.0, 43.0);
const PICKER_CAPTION_Y: f32 = 248.4;
pub(super) const VERSION_MENU: &str = "version-picker";
const PICKER_RADIUS: f32 = 8.0;
/// The menu reaches from the picker to the column's right edge.
const PICKER_MENU_W: f32 = INFO_RIGHT - PICKER.x;
/// The switch's PCVR / QUEST captions: grey DMCAPS, 8 px tall capitals.
const PICKER_CAPTION_SIZE: f32 = 11.5;
const CAPTION: Color32 = Color32::from_gray(118);
const CAPTION_ASCENT: f32 = 0.2;

pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    // Look for a headset quietly once, so the Quest chip has something to say: only for a
    // player on the Quest side or with a Quest set up before. adb's server would start for
    // everyone else too (on Windows its mDNS brings up a firewall question at first start).
    let quest_player = d.platform == Platform::Quest
        || d.state.quest_ip.is_some()
        || d.state.quest_adb_network;
    if quest_player && !d.quest_auto_checked && !d.demo && !kit.ghost {
        d.quest_auto_checked = true;
        if d.quest_conn.status.is_none() && !d.quest_conn.checking {
            d.check_quest(ctx, false);
        }
    }
    kit.image_d("logo_echovr.png", hero::LOGO);
    easter_egg(d, kit);
    let mut a = match d.platform {
        Platform::Pc => pc_action(d),
        Platform::Quest => quest_action(d),
    };
    if a.job.is_none() {
        explain_disabled(d, &mut a);
    }
    // Starting: look again soon in case the game's process ends before it shows up.
    if d.ours() && !d.game().is_running() {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
    hero::info_line(d, kit, "info-path", &a.line);
    // PLAY keeps the concept's size; longer labels are set smaller.
    match a.job.take() {
        Some(job) => {
            if hero::job_row(kit, "play", 0.0, &job) {
                d.cancel_job(&job.id);
            }
        }
        None => buttons(d, kit, ctx, a),
    }
    hero::switch(kit, "side", 0.0, &mut d.platform);
    if d.platform == Platform::Pc {
        version_picker(d, kit);
    }
    news(d, kit);
    panel::at_right(kit, |k| now::show(d, k, ctx));
}

/// The answers to PLAY's questions (launch anyway, Windows blocked the mod loader, VD's
/// Oculus mode, adding Echo VR to Steam), whichever page is showing: they come up off
/// Play too (after an install, JOIN on Servers).
pub(super) fn answers(d: &mut Dashboard, ctx: &egui::Context) {
    if d.dialogs.take(PREPARE_LINUX).is_some_and(|a| a.is_yes()) {
        setup::linux_setup(d, ctx);
        d.play_after_prep = d.jobs.contains_key(setup::LINUX_JOB);
    }
    if let Some(answer) = d.dialogs.take(JOIN_LIVE) {
        let lobby = d.pending_lobby.take();
        if let (Answer::Button(0), Some(lobby), Some(live)) = (answer, lobby, live_build(d)) {
            d.state.selected = Some(live.id);
            d.save();
            join(d, ctx, lobby);
        }
    }
    if let Some(answer) = d.dialogs.take(JOIN_ECHOVRCE) {
        let lobby = d.pending_lobby.take();
        if let (Answer::Button(0), Some(lobby)) = (answer, lobby) {
            super::plugins::back_to_echovrce(d);
            try_start(d, ctx, Some(lobby));
        }
    }
    if let Some(answer) = d.dialogs.take(LAUNCH_ANYWAY) {
        let lobby = d.pending_lobby.take();
        if answer.is_yes() {
            start(d, ctx, lobby);
        }
    }
    if let Some(Answer::Button(0)) = d.dialogs.take(PACK_DATA) {
        if let Some(id) = d.pack_repair.take() {
            super::versions::repair_game(d, ctx, &id);
        }
    }
    if let Some(answer) = d.dialogs.take(PACK_DATA_PLAY) {
        let lobby = d.pending_lobby.take();
        match (answer, d.pack_repair.take()) {
            (Answer::Button(0), Some(id)) => super::versions::repair_game(d, ctx, &id),
            (Answer::Button(1), Some(id)) => {
                d.pack_play_without.insert(id);
                try_start(d, ctx, lobby);
            }
            _ => {}
        }
    }
    if let Some(answer) = d.dialogs.take(NEVR_BLOCKED) {
        let id = d.nevr_blocked.take();
        match (answer, id) {
            (Answer::Button(0), Some(id)) => allow_and_repair(d, ctx, &id),
            (Answer::Button(1), _) => crate::core::platform::open_url("windowsdefender://threat"),
            _ => {}
        }
    }
    if let Some(Answer::Button(i)) = d.dialogs.take(EVENT_VR) {
        let runtime = match i {
            0 => Some(Runtime::Revive),
            1 => Some(Runtime::Wivrn),
            _ => None,
        };
        if let Some(runtime) = runtime {
            d.state.profile.runtime = runtime;
            d.save();
            let main = pc_action(d).main;
            if !matches!(main, Main::PickVr) {
                press_play(d, ctx, main);
            }
        }
    }
    if let Some(Answer::Button(i)) = d.dialogs.take(VD_SWITCH) {
        let via = match i {
            0 => Some(VdVia::VdXr),
            1 => Some(VdVia::SteamVr),
            _ => None,
        };
        if let Some(via) = via {
            d.state.profile.vd_via = via;
            d.save();
            d.notify(match via {
                VdVia::SteamVr => {
                    "Virtual Desktop plays through SteamVR now: starting Echo VR again"
                }
                _ => "Virtual Desktop plays directly now: starting Echo VR again",
            });
            // Once the game that couldn't start VR has closed.
            d.play_when_closed = Some(std::time::Instant::now());
        }
    }
    if let Some(at) = d.play_when_closed {
        if at.elapsed() > std::time::Duration::from_secs(30) {
            d.play_when_closed = None;
        } else if !d.game().is_running() && d.busy_any().is_none() {
            d.play_when_closed = None;
            let main = pc_action(d).main;
            if !matches!(main, Main::Stop | Main::ToInstall | Main::Nothing) {
                press_play(d, ctx, main);
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

/// What PLAY does.
enum Main {
    Play,
    Stop,
    /// Closes the other build PLAY started, then plays this one.
    SwitchTo,
    /// PLAY, after installing Revive (SteamVR on Windows).
    PrepareRevive,
    /// PLAY, after putting EchoXR into the game's folder (SteamVR on Windows).
    PrepareEchoXr,
    /// PLAY, after GE-Proton, EchoXR and the Steam shortcut (Linux) are in place.
    PrepareLinux,
    QuestConnect,
    QuestPlay,
    /// Close Echo VR on the headset.
    QuestStop,
    /// Nothing to play yet: open the Install page.
    ToInstall,
    /// An event build with "No headset" picked (Linux): asks which VR to play it in.
    PickVr,
    Nothing,
}

/// What CHECK FOR UPDATES does.
enum Update {
    Pc(Box<InstalledVersion>),
    Quest,
    Nothing,
}

/// Everything the top of the page shows for the current side.
struct Action {
    line: InfoLine,
    /// PLAY's label; "PLAY" is the image's own lettering.
    label: &'static str,
    /// Nothing installed: PLAY in greys.
    grey: bool,
    main: Main,
    enabled: bool,
    tip: String,
    /// A job on what PLAY would start: shown in the buttons.
    job: Option<JobView>,
    update: Update,
    update_enabled: bool,
    update_tip: String,
    /// The job CHECK FOR UPDATES waits for ("Busy: …"), when that is why it's off.
    update_busy: Option<String>,
    /// The last update failed: the button shows its orange "!".
    update_alert: bool,
    /// An update is out: the button says so.
    update_ready: bool,
}

impl Action {
    fn new() -> Action {
        Action {
            line: InfoLine::new(INFO_RIGHT),
            label: "PLAY",
            grey: false,
            main: Main::Nothing,
            enabled: false,
            tip: String::new(),
            job: None,
            update: Update::Nothing,
            update_enabled: false,
            update_tip: "Download any changed game files".into(),
            update_busy: None,
            update_alert: false,
            update_ready: false,
        }
    }

    /// PLAY does something on `res`: off, saying why, while a job there runs.
    fn wait_for(&mut self, d: &Dashboard, res: &[Res]) {
        match d.busy_with(res) {
            Some(busy) => (self.enabled, self.tip) = (false, busy),
            None => self.enabled = true,
        }
    }

    /// Nothing to play: a grey PLAY that opens the Install page, and one word on the info
    /// line (`state`, or the install's progress while one runs).
    fn not_installed(&mut self, state: &str, job: Option<&JobView>) {
        self.grey = true;
        (self.main, self.enabled) = (Main::ToInstall, true);
        self.tip = "Echo VR isn't installed yet. Click to install it".into();
        self.update_tip = "Install Echo VR first".into();
        self.line.parts = match job.filter(|j| j.installs()) {
            Some(j) => vec![
                "Installing".into(),
                j.fraction
                    .map_or_else(|| j.step(), |f| format!("{:.0}%", f * 100.0)),
            ],
            None => vec![state.into()],
        };
    }
}

pub(super) fn gb(bytes: u64) -> String {
    let g = bytes as f64 / 1e9;
    if g >= 10.0 || (g - g.round()).abs() < 0.05 {
        format!("{g:.0}GB")
    } else {
        format!("{g:.1}GB")
    }
}

/// Why CHECK FOR UPDATES can't be used right now, in its tooltip (PLAY's own tip says
/// why PLAY is off).
fn explain_disabled(d: &Dashboard, a: &mut Action) {
    let busy = a.update_busy.clone();
    if !a.update_enabled {
        a.update_tip = match (&busy, &a.update) {
            (Some(b), _) => b.clone(),
            (None, Update::Nothing) if d.platform == Platform::Pc => "Install Echo VR first".into(),
            (None, Update::Quest) if d.quest_conn.status != Some(Status::Ready) => {
                "Connect your Quest first".into()
            }
            (None, Update::Quest) => "Install Echo VR on your Quest first".into(),
            (None, Update::Pc(v)) if d.files_in_use(v).is_some() => {
                d.files_in_use(v).unwrap_or_default().into()
            }
            (None, _) => a.update_tip.clone(),
        };
    }
}

/// A hidden spot between ECHO and VR. Nothing gives it away: no cursor, no tip.
fn easter_egg(d: &mut Dashboard, kit: &mut Kit) {
    const SPOT: Dr = Dr::new(574.0, 74.4, 50.0, 126.5);
    if kit.ghost || kit.blocked {
        return;
    }
    let id = egui::Id::new("play-easter-egg");
    if kit
        .ui
        .interact(kit.drect(SPOT), id, egui::Sense::click())
        .clicked()
    {
        d.dialogs
            .info("You found an Easter Egg", "Never divide by 0!");
    }
}

// ---- PC ----

fn pc_action(d: &mut Dashboard) -> Action {
    let target = d.target();
    let id = match &target {
        Target::Installed(v) | Target::Missing(v) => Some(v.id.clone()),
        Target::Available(e) => Some(e.id.clone()),
        Target::None => None,
    };
    let job = id
        .and_then(|id| setup::job_for(d, &id))
        .or_else(|| hero::job_view(d, setup::REVIVE_JOB))
        .or_else(|| hero::job_view(d, setup::ECHOXR_JOB))
        .or_else(|| hero::job_view(d, setup::EVENT_JOB))
        .or_else(|| hero::job_view(d, setup::LINUX_JOB));
    let mut a = Action::new();
    let needs_steamvr = d.steamvr_missing();
    let echoxr = d.state.profile.through_echoxr();
    let vd = d.state.profile.runtime == Runtime::VirtualDesktop;
    let local = d.local();
    // Echo VR clients: the launcher's own game, or one started elsewhere. Servers on this
    // PC don't count (they never block PLAY).
    let running = local.state.is_running();
    let ours_running = local.ours.is_some();
    let ours = d.ours();
    match target {
        Target::Installed(v) => {
            a.job = job;
            // Echo VR running from this build. Another build running isn't this one: it is
            // named, and PLAY waits for it.
            let here = local.runs(&v.id, &v.root);
            let other = (running && !here).then(|| {
                d.running_version()
                    .map_or_else(|| "Another Echo VR build".to_string(), |o| o.name.clone())
            });
            let running = running && here;
            let ours_running = ours_running && local.ours.as_deref() == Some(v.id.as_str());
            let ours = ours && local.ours.as_deref().is_none_or(|id| id == v.id);
            let size = v.catalog_id.as_ref().and_then(|cid| {
                let c = d.catalog.as_ref()?;
                c.pc().find(|e| &e.id == cid)?.size.map(gb)
            });
            let event = v.publisher_lock.is_some();
            let state = if running {
                "Running"
            } else if ours {
                "Starting"
            } else if needs_steamvr && vd {
                "EchoXR is not set up"
            } else if needs_steamvr {
                "SteamVR is not set up"
            } else if event {
                "Classic lobby"
            } else {
                "Installed"
            };
            a.line.parts.push(state.into());
            a.line.parts.push(v.name.clone());
            // An event build plays on the classic lobbies server; a pack with a server of its
            // own on that, and the live build on a server a plugin set on that.
            if event {
                a.line.parts.push(d.state.relay_server.clone());
            } else if let Some(server) = d.server_for(&v.id) {
                a.line.parts.push(server);
            }
            if let Some(o) = &other {
                a.line.parts.push(format!("{o} is running"));
            }
            a.line.parts.extend(size);
            a.line.parts.extend(servers_here(local.servers.len()));
            a.line.path = Some(v.root.clone());
            a.line.path_click = PathClick::Open;
            if let Some(job) = &a.job {
                a.line.parts[0] = job_state(job);
                a.line.parts.push(job.step());
            }
            if ours_running {
                (a.label, a.main, a.enabled, a.grey) = ("STOP", Main::Stop, true, true);
                a.tip = "Close Echo VR".into();
            } else if running {
                a.label = "RUNNING";
                a.tip = "Echo VR was started outside the launcher.".into();
            } else if ours {
                // Clicked: STOP until the game shows up (or its start ends); it cancels it.
                (a.label, a.main, a.enabled, a.grey) = ("STOP", Main::Stop, true, true);
                a.tip = "Echo VR is starting: STOP cancels the start".into();
                if let Some(n) = &d.launch_note {
                    a.line.parts.push(format!("{n}…"));
                }
            } else if let Some(o) = &other {
                // Another build PLAY started: closed first, then this one starts.
                if local.ours.is_some() {
                    (a.main, a.enabled) = (Main::SwitchTo, true);
                    a.tip = format!("Close {o} and play this one");
                } else {
                    (a.main, a.enabled, a.grey) = (Main::Play, false, true);
                    a.tip = format!(
                        "{o} is running (started outside the launcher): close it first to play this one"
                    );
                }
            } else if needs_steamvr && echoxr {
                a.main = Main::PrepareEchoXr;
                a.tip = if vd {
                    "Start Echo VR through Virtual Desktop: the first start sets up VR support in the game's folder"
                } else {
                    "Start Echo VR on SteamVR: the first start sets up VR support in the game's folder"
                }
                .into();
                a.wait_for(d, &[Res::Vr, Res::Version(v.id.clone())]);
            } else if needs_steamvr {
                a.main = Main::PrepareRevive;
                a.tip = "Start Echo VR on SteamVR: the first start installs Revive, which runs it there (asks for administrator rights)".into();
                a.wait_for(d, &[Res::Vr]);
            } else if cfg!(target_os = "linux")
                && v.publisher_lock.is_some()
                && d.state.profile.runtime == Runtime::Flat
            {
                a.main = Main::PickVr;
                a.tip =
                    "Event builds always start in VR: PLAY asks whether through SteamVR or WiVRn"
                        .into();
            } else if cfg!(target_os = "linux") && !setup::pc_play_supported(d) {
                a.main = Main::PrepareLinux;
                a.tip = "Start Echo VR: the first start downloads what it needs to run on Linux (about 0.5 GB) and adds Echo VR to Steam once".into();
                a.wait_for(d, &[Res::Vr]);
            } else if !setup::pc_play_supported(d) {
                (a.main, a.enabled, a.grey) = (Main::Play, false, true);
                a.tip = "Echo VR for PC doesn't run on macOS: play it on Windows, or on your Quest"
                    .into();
            } else {
                (a.main, a.enabled) = (Main::Play, true);
                a.tip = "Start Echo VR".into();
            }
            let updates = crate::core::launcher::versions::has_updates(&v);
            let in_use = d.files_in_use(&v);
            // Besides the game's files, the button brings the version's plugin updates
            // and the launcher's.
            let game = updates && d.updates.game(&v.id);
            let plugins = d.updates.plugins_for(&v.id);
            let launcher = d.launcher_ready();
            // Only the launcher's update (it restarts) waits for every job; the game's and
            // its mods' for the jobs on this version.
            a.update_busy = if updates || plugins > 0 {
                d.busy_with(&[Res::Version(v.id.clone()), Res::Mods])
            } else {
                d.busy_any()
            };
            a.update_enabled = (updates || plugins > 0 || launcher)
                && in_use.is_none()
                && !ours
                && a.update_busy.is_none();
            if ours && !running {
                a.update_tip = "Echo VR is starting".into();
            }
            if let Some(why) = in_use {
                a.update_tip = why.into();
            }
            if !updates && !launcher {
                a.update_tip =
                    "Event builds don't get updates: REPAIR on the Install page checks their files"
                        .into();
            }
            a.update_alert = d
                .update_note
                .get(&v.id)
                .is_some_and(|n| n.contains("failed"));
            a.update_ready = game || plugins > 0 || launcher;
            if a.update_ready && a.update_enabled {
                a.update_tip = ready_tip(d, game, plugins, launcher);
            }
            // The orange "!": what it means, and that a click tries again.
            if a.update_alert && a.update_enabled {
                a.update_tip = format!(
                    "Its last update didn't finish: click to try again. {}",
                    a.update_tip
                );
            }
            a.update = Update::Pc(Box::new(v));
        }
        Target::Missing(_) => {
            a.not_installed("Game files missing", job.as_ref());
            if !job.as_ref().is_some_and(JobView::installs) {
                a.line.color = design::DANGER;
                a.tip =
                    "The game files are gone. Click to install Echo VR again or add its new folder"
                        .into();
            }
            // Being installed again: its progress and CANCEL in the buttons.
            a.job = job.filter(JobView::installs);
        }
        Target::Available(_) | Target::None => {
            a.not_installed("Not installed", job.as_ref());
            a.job = job.filter(JobView::installs);
        }
    }
    a
}

// ---- Quest ----

fn quest_action(d: &mut Dashboard) -> Action {
    let ready = d.quest_conn.status == Some(Status::Ready);
    let installed = ready && d.quest_info.as_ref().is_some_and(|i| i.installed);
    let known = ready && d.quest_info.is_some();
    let quest_busy = d.busy_with(&[Res::Quest]);
    let busy = d.quest_conn.checking || d.quest_busy || quest_busy.is_some();
    let job = hero::job_view(d, setup::QUEST_JOB);
    let mut a = Action::new();
    a.line.parts = quest_info(d);
    let game = d.quest_game();
    if game.is_running() && job.is_none() {
        // Running on the headset, as its API says over the network.
        let state = match game {
            crate::core::launcher::game::GameState::InMatch { .. } => "In a match",
            _ => "Running",
        };
        a.line.parts = vec![state.into(), "On your Quest".into()];
        a.line
            .parts
            .extend(d.quest_info.as_ref().and_then(|i| i.device.clone()));
        if ready {
            (a.label, a.main, a.enabled, a.grey) = ("STOP", Main::QuestStop, !d.quest_busy, true);
            a.tip = "Close Echo VR on the headset".into();
        } else {
            a.label = "RUNNING";
            a.tip =
                "Plug in your Quest or turn on ADB over the network to stop it from here".into();
        }
        a.update = Update::Quest;
        a.update_tip = "Close Echo VR on the headset first".into();
        return a;
    }
    if job.as_ref().is_some_and(JobView::installs) || (known && !installed) {
        a.not_installed("Not installed on this Quest", job.as_ref());
        a.tip = "Echo VR isn't on your Quest yet. Click to install it".into();
        a.line
            .parts
            .extend(d.quest_info.as_ref().and_then(|i| i.device.clone()));
    } else if installed {
        a.job = job;
        if let Some(job) = &a.job {
            a.line.parts[0] = job_state(job);
            a.line.parts.push(job.step());
        }
        (a.main, a.enabled) = (Main::QuestPlay, !d.quest_busy);
        a.tip = "Start Echo VR on the headset".into();
    } else {
        a.label = "CONNECT";
        a.main = Main::QuestConnect;
        a.enabled = !d.quest_conn.checking && !d.quest_busy;
        a.tip = "Look for your Quest over USB or Wi-Fi".into();
    }
    let offered = d.quest_info.as_ref().and_then(|i| i.update_offered());
    a.update = Update::Quest;
    a.update_enabled = installed && !busy;
    a.update_busy = quest_busy;
    a.update_tip = match offered {
        Some(v) => format!("Update to nEVR {v}: install the newer build on your Quest"),
        None => "Check for a newer nEVR build for your Quest".into(),
    };
    // A launcher update comes along (after the Quest's).
    a.update_ready = offered.is_some() || d.launcher_ready();
    if d.launcher_ready() && a.update_enabled {
        a.update_tip = format!("{}, then {}", a.update_tip, ready_tip(d, false, 0, true));
    }
    a
}

/// The orange button's tip: what a click updates.
fn ready_tip(d: &Dashboard, game: bool, plugins: usize, launcher: bool) -> String {
    let mut parts = Vec::new();
    if game {
        parts.push("the changed game files".to_string());
    }
    match plugins {
        0 => {}
        1 => parts.push("a plugin".into()),
        n => parts.push(format!("{n} plugins")),
    }
    if launcher {
        if let super::LauncherUpdate::Available(r) = &d.launcher_update {
            parts.push(format!(
                "the launcher {} (it restarts at the end)",
                r.version
            ));
        }
    }
    let list = match parts.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    format!("Updates are out: this downloads {list}")
}

/// What the headset's connection and install are, for an info line.
pub(super) fn quest_info(d: &Dashboard) -> Vec<String> {
    let device = d.quest_info.as_ref().and_then(|i| i.device.clone());
    match (d.quest_conn.checking, d.quest_conn.status) {
        (true, _) => vec!["Looking for your headset".into()],
        (_, Some(Status::Ready)) => {
            let mut parts = match &d.quest_info {
                Some(i) if i.installed => vec!["Installed".into(), i.version_label()],
                Some(_) => vec!["Not installed on this Quest".into()],
                None => vec!["Reading the installed version".into()],
            };
            parts.extend(device);
            parts
        }
        (_, Some(Status::Unauthorized)) => vec![
            "Allow this PC".into(),
            "Accept the USB debugging prompt in the headset".into(),
        ],
        (_, Some(Status::Ambiguous)) => vec![
            "Several devices connected".into(),
            "Pick your Quest when you connect".into(),
        ],
        (_, Some(Status::None)) => vec![
            "No Quest found".into(),
            "Plug it in by USB with developer mode on".into(),
        ],
        (_, None) => vec!["Plug in your Quest by USB".into()],
    }
}

/// What a running job is doing, as the info line's first word.
pub(super) fn job_state(job: &JobView) -> String {
    use super::JobKind;
    match job.kind {
        JobKind::Install | JobKind::QuestInstall => "Installing".into(),
        JobKind::Reinstall => "Repairing".into(),
        JobKind::Update => "Updating".into(),
        JobKind::Verify => "Checking files".into(),
        JobKind::Uninstall => "Uninstalling".into(),
        JobKind::Remove => "Removing".into(),
        JobKind::GoBack => "Undoing update".into(),
        JobKind::Revive | JobKind::Mods | JobKind::LauncherUpdate => job.title.clone(),
    }
}

// ---- the buttons ----

/// PLAY and CHECK FOR UPDATES.
fn buttons(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context, a: Action) {
    let face = match a.label {
        "RUNNING" => Face::Running,
        label => Face::Label(label),
    };
    let row = Row {
        face,
        grey: a.grey,
        enabled: a.enabled,
        tip: &a.tip,
        side: if a.update_ready && !a.update_alert {
            Side::Blue {
                icon: Icon::Download,
                label: "Update now",
                ready: true,
            }
        } else {
            Side::Updates {
                alert: a.update_alert,
            }
        },
        side_enabled: a.update_enabled,
        side_tip: &a.update_tip,
    };
    let (main, update) = hero::row(kit, "play", 0.0, &row);
    if main {
        press_play(d, ctx, a.main);
    }
    if update {
        run_update(d, ctx, a.update);
    }
}

/// What PLAY's button does now (`main`, from [`pc_action`] or [`quest_action`]).
fn press_play(d: &mut Dashboard, ctx: &egui::Context, main: Main) {
    match main {
        Main::Play => try_start(d, ctx, None),
        Main::Stop => stop(d),
        Main::SwitchTo => {
            stop(d);
            d.play_when_closed = Some(std::time::Instant::now());
        }
        Main::PrepareRevive => {
            d.pending_lobby = None;
            setup::revive(d, ctx);
            d.play_after_prep = d.jobs.contains_key(setup::REVIVE_JOB);
        }
        Main::PrepareEchoXr => {
            d.pending_lobby = None;
            setup::echoxr_windows(d, ctx);
            d.play_after_prep = d.jobs.contains_key(setup::ECHOXR_JOB);
        }
        Main::PrepareLinux if setup::linux_needs_shortcut() => {
            d.pending_lobby = None;
            d.dialogs.confirm(
                PREPARE_LINUX,
                "Add Echo VR to Steam",
                &setup::linux_setup_question(),
                crate::ui::dialogs::Icon::Info,
            )
        }
        Main::PrepareLinux => {
            d.pending_lobby = None;
            setup::linux_setup(d, ctx);
            d.play_after_prep = d.jobs.contains_key(setup::LINUX_JOB);
        }
        Main::QuestConnect => d.check_quest(ctx, true),
        Main::QuestPlay => quest_launch(d, ctx),
        Main::QuestStop => quest_stop(d, ctx),
        Main::ToInstall => {
            // The Install page opens on the version picked here, with its install's
            // questions when it can be installed straight away.
            let target = d.target();
            d.install_pick = match &target {
                Target::Available(e) => Some(e.id.clone()),
                Target::Missing(v) => v.catalog_id.clone(),
                _ => None,
            };
            d.page = Page::Install;
            if let Target::Available(e) = target {
                if d.busy_any().is_none() {
                    setup::ask_install(d, e);
                }
            }
        }
        Main::PickVr => d.dialogs.options(
            EVENT_VR,
            "Play in VR",
            "Event builds always start in VR, and Settings says you play without a headset. Which VR should Echo VR use? Settings keeps your choice, and can change it back.",
            crate::ui::dialogs::Icon::Info,
            &["Use SteamVR", "Use WiVRn", "Cancel"],
        ),
        Main::Nothing => {}
    }
}

/// A notice's "Play": picks installed version `id` on the Play page, and starts it when
/// PLAY would (else its next step, such as the VR setup, waits there).
pub(super) fn play_version(d: &mut Dashboard, ctx: &egui::Context, id: &str) {
    if d.state.version(id).is_none() {
        return;
    }
    d.platform = Platform::Pc;
    d.state.selected = Some(id.to_string());
    d.save();
    d.page = Page::Play;
    let main = pc_action(d).main;
    if matches!(
        main,
        Main::Play | Main::PrepareRevive | Main::PrepareEchoXr | Main::PrepareLinux
    ) {
        press_play(d, ctx, main);
    }
}

/// Play's update button: the game's update, then its mods', then the launcher's.
fn run_update(d: &mut Dashboard, ctx: &egui::Context, update: Update) {
    // The launcher last: it restarts.
    let launcher = d.launcher_ready().then_some(QueuedUpdate::Launcher);
    match update {
        Update::Pc(v) => {
            let plugins =
                (d.updates.plugins_for(&v.id) > 0).then(|| QueuedUpdate::Plugins(v.id.clone()));
            let held = d.state.holds.contains_key(&format!("game:{}", v.id));
            if held {
                // Gone back from its update: only a newer one is offered; this button
                // doesn't bring the one gone back from again.
                d.updates.check_soon();
                let key = format!("game:{}", v.id);
                d.notify_with(
                    "You undid this version's last update, so it isn't installed again",
                    "Install it anyway",
                    super::notices::Action::UseNewest(key),
                );
            } else if crate::core::launcher::versions::has_updates(&v) {
                versions::update(d, ctx, *v);
            }
            d.queue_updates(ctx, plugins.into_iter().chain(launcher).collect());
        }
        Update::Quest => {
            setup::quest_install(d, ctx);
            d.queue_updates(ctx, launcher.into_iter().collect());
        }
        Update::Nothing => {}
    }
}

// ---- joining a lobby ----

/// Opens "Join a lobby" with the link on the clipboard (when it is one) or the last one.
pub(super) fn open_lobby_card(d: &mut Dashboard) {
    let input = arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get_text().ok())
        .map(|t| t.trim().to_string())
        .filter(|t| crate::core::links::parse(t).is_some() || launch::lobby_uuid(t).is_some())
        .unwrap_or_else(|| d.state.last_lobby.clone());
    d.overlay = Some(setup::Overlay::JoinLobby { input });
}

/// "Join a lobby": paste a spark:// or echo.taxi link (or the bare ID), and JOIN starts
/// Echo VR into that lobby.
pub(super) fn lobby_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    // The PC game can be started into it, or (running already, or on the Quest side) the
    // match is queued as the next one.
    let direct = super::servers::starts_pc(d);
    let signed_in = d.vrce.session().is_some();
    let Some(setup::Overlay::JoinLobby { input }) = &mut d.overlay else {
        return;
    };
    let (w, h) = (dz(960.0), dz(310.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, "Join a lobby");
    let hint = match (direct, signed_in) {
        (true, _) => "Paste a spark:// or echo.taxi link, or a lobby ID: Echo VR starts and joins that lobby.",
        (false, true) => "Paste a spark:// or echo.taxi link, or a lobby ID: it becomes your next match, and the terminal in Echo VR takes you there.",
        (false, false) => "Paste a spark:// or echo.taxi link, or a lobby ID. While Echo VR runs (or on the Quest) a match can only be queued as your next one: sign in with EchoVRCE for that.",
    };
    let th = k.caps_text(x, y, cw, hint, 17.0, design::BODY, 0.0);
    let fy = y + th + dz(20.0);
    let pw = k.button_width("Paste", None, BTN_H).max(100.0);
    let id = crate::core::links::parse(input)
        .map(|l| Join {
            lobby: l.lobby,
            spectate: l.spectate,
        })
        .or_else(|| launch::lobby_uuid(input).map(Join::lobby));
    let invalid = !input.trim().is_empty() && id.is_none();
    k.field(
        "lobby-id",
        input,
        x,
        fy,
        cw - pw - 10.0,
        BTN_H,
        "spark://c/…",
        invalid,
        "A lobby link or ID",
    );
    if k.button(
        "lobby-paste",
        x + cw - pw,
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
            *input = clip.trim().to_string();
        }
    }
    if invalid {
        let msg = "That isn't a lobby link or ID.";
        k.caps_text(x, fy + BTN_H + dz(12.0), cw, msg, 16.0, design::DANGER, 0.0);
    }
    let text = input.trim().to_string();
    let by = bottom - BTN_H;
    let jw = k.button_width("Join", None, BTN_H).max(140.0);
    let cw2 = k.button_width("Cancel", None, BTN_H).max(110.0);
    let right = x + cw;
    if k.button(
        "lobby-cancel",
        right - cw2,
        by,
        cw2,
        BTN_H,
        Tone::Dark,
        None,
        "Cancel",
        true,
        "",
    )
    .clicked
        || k.key(egui::Key::Escape)
    {
        d.overlay = None;
        return;
    }
    let watch = id.as_ref().is_some_and(|j| j.spectate);
    let (label, tip) = match (direct, watch) {
        (true, true) => ("Watch", "Start Echo VR on the monitor and watch this match"),
        (true, false) => ("Join", "Start Echo VR and join this lobby"),
        (false, _) => ("Join", "Queue this match as your next one"),
    };
    // Off: what's missing.
    let tip = match (&id, direct || signed_in) {
        (None, _) => "Paste a lobby link or ID first",
        (Some(_), false) => "Sign in with EchoVRCE to queue a match while Echo VR runs",
        _ => tip,
    };
    let join = k
        .button(
            "lobby-join",
            right - cw2 - 8.0 - jw,
            by,
            jw,
            BTN_H,
            Tone::Go,
            None,
            label,
            id.is_some() && (direct || signed_in),
            tip,
        )
        .clicked
        || (id.is_some() && (direct || signed_in) && k.key(egui::Key::Enter));
    if let (true, Some(id)) = (join, id) {
        d.overlay = None;
        d.state.last_lobby = text;
        d.save();
        if direct {
            self::join(d, ctx, id);
        } else {
            super::servers::queue_lobby(d, ctx, &id.lobby);
        }
    }
}

// ---- the version picker ----

/// VERSION, right of the switch: the version PLAY starts, and a menu to switch to
/// another. Installed versions come first, then the catalogue's others: picking one of
/// those greys PLAY, which then opens the Install page on it. Jobs don't lock it: while
/// one version downloads, another can be played (each row says what runs on it).
fn version_picker(d: &mut Dashboard, kit: &mut Kit) {
    let installed = d.state.versions.clone();
    let available: Vec<VersionEntry> = d
        .catalog
        .as_ref()
        .map(|c| {
            c.pc()
                .filter(|e| d.state.installed_from(&e.id).is_none() && d.state.offers(e))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if installed.is_empty() && available.is_empty() {
        return;
    }
    let (current_id, current_name) = match d.target() {
        Target::Installed(v) | Target::Missing(v) => (Some(v.id), v.name),
        Target::Available(e) => (Some(e.id), e.name),
        Target::None => (None, "Choose a version".to_string()),
    };
    let running = d.game().is_running() || d.ours();
    let (enabled, tip) = if running {
        (false, "Close Echo VR to switch versions".to_string())
    } else {
        (true, "The version PLAY starts: click to switch".to_string())
    };

    let caption = kit.spaced_galley(
        "VERSION",
        design::din(PICKER_CAPTION_SIZE),
        CAPTION,
        dz(0.9),
        false,
    );
    let (cx, cy) = (dz(PICKER.x), dz(PICKER_CAPTION_Y) - caption_top(kit));
    kit.put(cx, cy, caption);

    // Narrower with the page (the rail unfolded): to the column's edge.
    let r = kit.drect(PICKER.wider((INFO_RIGHT + kit.dx() - PICKER.x - PICKER.w).min(0.0)));
    let (resp, t, pressed) = kit.hot(VERSION_MENU, r, enabled, &tip);
    let open = enabled && kit.menu_open(VERSION_MENU);
    let fill = if pressed {
        Color32::from_gray(12)
    } else {
        style::mix(design::DARK, Color32::from_gray(40), t)
    };
    let radius = dz(PICKER_RADIUS);
    kit.ui.painter().rect_filled(r, radius, fill);
    if open {
        kit.ui.painter().rect_stroke(
            r,
            radius,
            egui::Stroke::new(dz(2.0), design::BLUE),
            egui::StrokeKind::Inside,
        );
    }
    let fg = if enabled {
        design::TEXT
    } else {
        Color32::from_gray(150)
    };
    let chevron = dz(15.0);
    let pad = dz(18.0);
    // A long name in a narrow picker gets smaller before it is cut.
    let room = r.width() - 2.0 * pad - chevron - dz(10.0);
    let name = current_name.to_uppercase();
    let size = [17.0, 15.5, 14.0]
        .into_iter()
        .find(|&s| {
            kit.spaced_galley(&name, design::din(s), fg, dz(1.2), false)
                .size()
                .x
                <= room
        })
        .unwrap_or(14.0);
    let g = kit.spaced_fit(&name, design::din(size), fg, dz(1.2), false, room);
    let gy = r.center().y - g.size().y / 2.0;
    kit.ui.painter().galley(pos2(r.min.x + pad, gy), g, fg);
    style::icon_at(
        kit.ui.painter(),
        Icon::ChevronDown,
        pos2(r.max.x - pad - chevron, r.center().y - chevron / 2.0),
        chevron,
        fg,
    );
    if resp.clicked {
        kit.toggle_menu(VERSION_MENU);
        return;
    }
    if !enabled {
        return;
    }

    let checked = |id: &str| current_id.as_deref() == Some(id);
    let mut items: Vec<MenuItem> = installed
        .iter()
        .map(|v| {
            let present = d.demo || v.present();
            let (detail, detail_color) = if let Some(j) = hero::job_view(d, &v.id) {
                (job_detail(&j), design::BLUE)
            } else if !present {
                ("Files missing".to_string(), design::DANGER)
            } else if v.external {
                ("Existing folder".to_string(), design::GREY)
            } else {
                let size = v.catalog_id.as_ref().and_then(|cid| {
                    let c = d.catalog.as_ref()?;
                    c.pc().find(|e| &e.id == cid)?.size.map(gb)
                });
                // An update is out for it (the rail's badge counts every version's).
                let ready = d.updates.game(&v.id) || d.updates.plugins_for(&v.id) > 0;
                match (ready, size) {
                    (true, Some(s)) => (format!("Update available  ·  {s}"), design::QUEST_ON),
                    (true, None) => ("Update available".into(), design::QUEST_ON),
                    (false, s) => (s.unwrap_or_default(), design::GREY),
                }
            };
            MenuItem::Pick {
                label: v.name.clone(),
                detail,
                detail_color,
                checked: checked(&v.id),
                tip: v.root.clone(),
            }
        })
        .collect();
    items.extend(available.iter().map(|e| {
        let job = hero::job_view(d, &e.id);
        MenuItem::Pick {
            label: e.name.clone(),
            detail: match (&job, e.size) {
                (Some(j), _) => job_detail(j),
                (None, Some(s)) => format!("Not installed  ·  {}", gb(s)),
                (None, None) => "Not installed".into(),
            },
            detail_color: if job.is_some() {
                design::BLUE
            } else {
                design::GREY
            },
            checked: checked(&e.id),
            tip: if job.is_some() {
                "Installing: PLAY starts it once it's in place".into()
            } else {
                "Not installed yet: PLAY takes you to the Install page".into()
            },
        }
    }));
    items.push(MenuItem::Divider);
    items.push(MenuItem::row(
        "Install another version…",
        "Open the Install page",
    ));
    let w = r.width().max(dz(PICKER_MENU_W));
    let anchor = egui::Rect::from_min_size(r.min, egui::vec2(w, r.height()));
    match kit.menu_at(VERSION_MENU, anchor, &items) {
        Some(i) if i < installed.len() + available.len() => {
            let id = match installed.get(i) {
                Some(v) => v.id.clone(),
                None => available[i - installed.len()].id.clone(),
            };
            d.state.selected = Some(id);
            d.save();
        }
        Some(_) => d.page = Page::Install,
        None => {}
    }
}

/// A version's job in the picker's menu: "Installing  ·  42%".
fn job_detail(j: &JobView) -> String {
    match j.fraction {
        Some(f) => format!("{}  ·  {:.0}%", job_state(j), f * 100.0),
        None => job_state(j),
    }
}

/// How far below its galley's top a DMCAPS capital starts, at the caption's size.
fn caption_top(kit: &Kit) -> f32 {
    let g = kit.spaced_galley("V", design::din(PICKER_CAPTION_SIZE), CAPTION, 0.0, false);
    // DMCAPS' ascent sits above its capitals; measured once from the switch's labels.
    g.size().y * CAPTION_ASCENT
}

// ---- Community News ----

fn news(d: &mut Dashboard, kit: &mut Kit) {
    let dx = kit.dx();
    // A wider window: the header strip stretches (its title stays), the design's banner
    // background fills the wider card, and a downloaded banner keeps its size, centred.
    kit.image_wider(
        "news_header.png",
        Dr::new(NEWS.x, NEWS.y, NEWS.w, NEWS_HEADER_H),
        dx,
        (420.0, 8.0),
        Color32::WHITE,
    );
    let area = Dr::new(
        NEWS.x,
        NEWS.y + NEWS_HEADER_H,
        NEWS.w + dx,
        NEWS.h - NEWS_HEADER_H,
    );
    let banner = Dr::new(area.x + dx / 2.0, area.y, NEWS.w, area.h);
    let news = d.feed.news.clone();
    let main = news.as_ref().and_then(|n| n.slots.main.clone());
    let tex = main.as_ref().and_then(|m| d.feed.texture(m)).cloned();
    let lettering = main.as_ref().and_then(|m| d.feed.lettering(m)).cloned();
    let links = main.as_ref().map(NewsItem::all_links).unwrap_or_default();
    // Nothing loaded (yet), and snapshots: the design's banner, with its HOW TO PLAY.
    let fallback = news.is_none() || (tex.is_none() && d.demo && !d.feed_live);
    match tex {
        // The background covers the card (cropped top and bottom when it's wider); the
        // lettering stays at its left.
        _ if fallback => {
            kit.image_cover("news_fallback_bg.jpg", area, dz(4.0));
            let (x, w, h) = NEWS_LETTERING;
            let y = area.y + (area.h - h) / 2.0;
            kit.image_d("news_fallback_text.png", Dr::new(x, y, w, h));
            banner_rim(kit, area);
        }
        // In two layers, as the design's banner: the background covers the card, the
        // lettering stays whole above the links.
        Some(tex) if main.as_ref().is_some_and(|m| m.lettering.is_some()) => {
            kit.texture_cover(&tex, kit.drect(area), dz(4.0));
            if let Some(l) = &lettering {
                let bottom = if links.is_empty() {
                    area.y + area.h - 14.0
                } else {
                    NEWS_LINK.1 - 36.0
                };
                let fit = Dr::new(
                    banner.x + 40.0,
                    area.y + 14.0,
                    banner.w - 80.0,
                    bottom - area.y - 14.0,
                );
                kit.paint_tex(l, contain(l.size(), kit.drect(fit)), Color32::WHITE);
            }
            banner_rim(kit, area);
        }
        Some(tex) => {
            if dx > 0.0 {
                kit.image_d("card_bg.png", area);
            }
            kit.texture_cover(&tex, kit.drect(banner), dz(4.0));
            banner_rim(kit, area);
        }
        // A message without a picture (or still downloading it): its title on the panel.
        None => {
            kit.image_d("card_bg.png", area);
            if let Some(m) = &main {
                let g = kit.spaced_galley(
                    &m.title.to_uppercase(),
                    design::conthrax(40.0),
                    design::TEXT,
                    dz(4.0),
                    false,
                );
                kit.clipped(
                    dz(banner.x + 40.0),
                    dz(banner.y),
                    dz(banner.w - 80.0),
                    dz(banner.h),
                    |kit| {
                        kit.put(dz(banner.x + 50.0), dz(banner.y + 60.0), g);
                    },
                );
            }
        }
    }
    let links = if fallback {
        vec![(
            "How to play".to_string(),
            crate::core::LOUNGE_INVITE.to_string(),
        )]
    } else {
        links
    };
    // Side by side from the right, the first leftmost.
    let (right, base) = NEWS_LINK;
    let mut x = dz(right + dx);
    for (i, (label, url)) in links.iter().enumerate().rev() {
        let g = kit.spaced_fit(
            &label.to_uppercase(),
            design::conthrax(21.0),
            design::TEXT,
            dz(5.5),
            true,
            dz(700.0),
        );
        // put() takes x within the page, its rect is in the window's: step by the width.
        x -= g.size().x;
        let r = kit.put(x, dz(base) - g.size().y, g);
        x -= dz(48.0);
        if kit.click_area(&format!("news-link-{i}"), r, url) {
            crate::core::platform::open_url(url);
        }
    }

    let (main_card, community_card) = cards(news.as_ref().map(|n| n.slots.clone()), main);
    // Side by side, sharing the extra width; taller in a taller window.
    let (half, dy) = (dx / 2.0, kit.dy());
    card(kit, 0, CARDS[0].wider(half).taller(dy), &main_card, false);
    let second = CARDS[1].moved(half).wider(half).taller(dy);
    card(kit, 1, second, &community_card, true);
}

/// Pure: `size` scaled to fit inside `r`, centred.
fn contain(size: [usize; 2], r: egui::Rect) -> egui::Rect {
    let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
    let s = (r.width() / w).min(r.height() / h);
    egui::Rect::from_center_size(r.center(), egui::vec2(w * s, h * s))
}

/// The rim around a picture banner: violet at the top to pink at the bottom.
fn banner_rim(kit: &Kit, area: Dr) {
    kit.gradient_frame(
        kit.drect(area),
        dz(4.0),
        dz(2.0),
        Color32::from_rgb(120, 60, 200),
        design::RIM_BOTTOM,
    );
}

/// The two cards' contents: the `main` message, and the `community` one (or a pointer to
/// the Discord when it isn't set).
fn cards(
    slots: Option<crate::core::launcher::feed::Slots>,
    main: Option<NewsItem>,
) -> (NewsItem, NewsItem) {
    let text = |title: &str, body: &str| NewsItem {
        title: title.into(),
        body: body.into(),
        ..Default::default()
    };
    let main = main.unwrap_or_else(|| {
        text(
            "Welcome",
            "Your launcher for Echo VR on PC and Quest.\n\n\
             - Install, update and play from one place\n\
             - Friends, servers and matches to join, on the left\n\
             - Server status and this week's best on the right\n\n\
             Community news appears here when there is some.",
        )
    });
    let community = slots.and_then(|s| s.community).unwrap_or_else(|| NewsItem {
        title: "Community".into(),
        body:
            "Matches, events, help and the latest builds: the Echo VR community meets on Discord."
                .into(),
        link_label: "Join the Discord".into(),
        link_url: crate::core::LOUNGE_INVITE.into(),
        ..Default::default()
    });
    (main, community)
}

fn card_look() -> Look {
    Look {
        font: design::din(15.8),
        bold: design::din(15.8),
        mono: egui::FontId::monospace(dz(15.0)),
        small: design::din(14.0),
        color: design::TEXT,
        strong: design::TEXT,
        subtle: design::BODY,
        link: design::TEXT,
        chip: Color32::from_white_alpha(18),
        chip_rim: Color32::TRANSPARENT,
        line_gap: 0.0,
        paragraph_gap: dz(18.0),
        bullet_indent: dz(16.0),
        uppercase: true,
        dash_bullets: true,
    }
}

/// A news card: the title in Conthrax, the text in DIN caps, and its link (when asked).
fn card(kit: &mut Kit, i: usize, r: Dr, item: &NewsItem, with_link: bool) {
    let (x, y, w, bottom) = hero::card_frame(kit, r, &item.title);
    let look = card_look();
    let body_h = kit.clipped(x, y, w, bottom - y, |kit| {
        markdown::draw(kit, x, y, w, &item.body, &look)
    });
    if with_link && !item.link_label.is_empty() && !item.link_url.is_empty() {
        let g = kit.spaced_galley(
            &item.link_label.to_uppercase(),
            design::din(16.5),
            design::TEXT,
            dz(0.5),
            true,
        );
        let ly = (y + body_h + look.paragraph_gap).min(bottom - g.size().y);
        let lr = kit.put(x, ly, g);
        if kit.click_area(&format!("card-link-{i}"), lr, &item.link_url) {
            crate::core::platform::open_url(&item.link_url);
        }
    }
}

// ---- starting ----

fn quest_stop(d: &mut Dashboard, ctx: &egui::Context) {
    d.quest_busy = true;
    d.worker.spawn(ctx, |tx| {
        tx.send(Msg::QuestAction(quest::stop().map_err(|e| {
            UiError::from_anyhow(&e, "Couldn't close Echo VR")
        })))
    });
}

/// Closes the Echo VR PLAY started and starts that version again.
pub(super) fn restart(d: &mut Dashboard) {
    let Some(id) = d.local().ours else {
        return;
    };
    d.platform = Platform::Pc;
    d.state.selected = Some(id);
    d.save();
    stop(d);
    d.play_when_closed = Some(std::time::Instant::now());
}

/// Starts Echo VR on the Quest, from another page: Play shows it.
pub(super) fn play_quest(d: &mut Dashboard, ctx: &egui::Context) {
    d.platform = Platform::Quest;
    d.page = Page::Play;
    quest_launch(d, ctx);
}

fn quest_launch(d: &mut Dashboard, ctx: &egui::Context) {
    d.quest_busy = true;
    d.worker.spawn(ctx, |tx| {
        tx.send(Msg::QuestAction(quest::launch().map_err(|e| {
            UiError::from_anyhow(&e, "Couldn't start Echo VR")
        })))
    });
}

/// A match to start the game into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Join {
    pub lobby: String,
    /// Watch as a spectator (`spark://s/` links).
    pub spectate: bool,
}

impl Join {
    pub fn lobby(lobby: String) -> Join {
        Join {
            lobby,
            spectate: false,
        }
    }
}

/// Starts Echo VR into `lobby` (JOIN on Servers or Friends, Join a lobby, a link) the way
/// PLAY would start it: the VR setup first (the game then starts into the match), or says
/// why it can't.
pub(super) fn join(d: &mut Dashboard, ctx: &egui::Context, lobby: Join) {
    let a = pc_action(d);
    if !a.enabled {
        d.notify(&a.tip);
        return;
    }
    let prepare = |d: &mut Dashboard,
                   ctx: &egui::Context,
                   job: &str,
                   f: fn(&mut Dashboard, &egui::Context)| {
        d.pending_lobby = Some(lobby.clone());
        f(d, ctx);
        d.play_after_prep = d.jobs.contains_key(job);
    };
    match a.main {
        // An event build plays the classic lobbies: these matches are the live build's.
        Main::Play if matches!(d.target(), Target::Installed(v) if v.publisher_lock.is_some()) => {
            match live_build(d) {
                Some(live) => {
                    d.pending_lobby = Some(lobby);
                    d.dialogs.options(
                        JOIN_LIVE,
                        "Join with the live build?",
                        &format!(
                            "The event build picked next to PLAY plays on the classic lobbies \
                             server, not in these matches. {} can join it.",
                            live.name
                        ),
                        DlgIcon::Question,
                        &[&format!("Switch to {} and join", live.name), "Cancel"],
                    );
                }
                None => d.dialogs.info(
                    "Install the live build to join",
                    "Event builds play on the classic lobbies server, not in these matches: \
                     the live build of Echo VR joins them. Install it on the Install page.",
                ),
            }
        }
        Main::Play => try_start(d, ctx, Some(lobby)),
        Main::PrepareRevive => prepare(d, ctx, setup::REVIVE_JOB, setup::revive),
        Main::PrepareEchoXr => prepare(d, ctx, setup::ECHOXR_JOB, setup::echoxr_windows),
        Main::PrepareLinux if setup::linux_needs_shortcut() => {
            d.pending_lobby = Some(lobby);
            d.dialogs.confirm(
                PREPARE_LINUX,
                "Add Echo VR to Steam",
                &setup::linux_setup_question(),
                crate::ui::dialogs::Icon::Info,
            )
        }
        Main::PrepareLinux => prepare(d, ctx, setup::LINUX_JOB, setup::linux_setup),
        Main::ToInstall => {
            d.notify("Install Echo VR first to join matches");
            press_play(d, ctx, Main::ToInstall);
        }
        _ => d.notify(&a.tip),
    }
}

/// The installed live build (no event build), the one picked first.
fn live_build(d: &Dashboard) -> Option<crate::core::launcher::store::InstalledVersion> {
    let live = |v: &&crate::core::launcher::store::InstalledVersion| {
        v.publisher_lock.is_none() && v.present()
    };
    d.state
        .versions
        .iter()
        .filter(live)
        .find(|v| v.id == "pc-latest")
        .or_else(|| d.state.versions.iter().find(live))
        .cloned()
}

/// Starts the PC game (joining a match when given), after warning about anything that
/// looks wrong.
pub(super) fn try_start(d: &mut Dashboard, ctx: &egui::Context, lobby: Option<Join>) {
    let event = matches!(d.target(), Target::Installed(v) if v.publisher_lock.is_some());
    // An event build plays on the classic lobbies server: your account there first, once.
    // It joins no links (those are the live build's matches).
    if event {
        if d.state.relay_account.is_none() {
            d.overlay = Some(setup::relay_account(d, true));
            return;
        }
        // EchoLoader and EchoRelay's patch in its folder first (a build an older launcher
        // installed has the patch as EchoRelay's own installer puts it in); then it starts.
        if let Target::Installed(v) = d.target() {
            if !relay::in_place(&v) {
                // EchoRelay's files are being set up for another event build: after it.
                if let Some(busy) = d.busy_with(&[Res::Relay, Res::Version(v.id.clone())]) {
                    d.notify(&busy);
                    return;
                }
                setup::event_build(d, ctx, &v);
                d.play_after_prep = d.jobs.contains_key(setup::EVENT_JOB);
                return;
            }
        }
        return match launch::preflight(&d.state.profile) {
            Some(w) => {
                d.pending_lobby = None;
                d.dialogs.confirm(
                    LAUNCH_ANYWAY,
                    "Launch Echo VR",
                    &format!("{w}\n\nLaunch anyway?"),
                    DlgIcon::Warning,
                )
            }
            None => start(d, ctx, None),
        };
    }
    // The live build on an EchoRelay server a plugin chose: EchoLoader and EchoRelay's
    // patch in nEVR's place first; then it starts. EchoVRCE's matches aren't there.
    if let Target::Installed(v) = d.target() {
        let echorelay = crate::core::launcher::mods::echorelay_for(&d.state, &v).is_some();
        if echorelay && lobby.is_some() {
            let place = d
                .state
                .game_server
                .as_ref()
                .map(|g| g.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "another server".into());
            d.pending_lobby = lobby;
            d.dialogs.options(
                JOIN_ECHOVRCE,
                "This match is on EchoVRCE",
                &format!(
                    "Echo VR plays on {place} now: a plugin chose it. Switch back to EchoVRCE \
                     to join this match? The plugin can choose {place} again later."
                ),
                DlgIcon::Question,
                &["Back to EchoVRCE and join", "Cancel"],
            );
            return;
        }
        if echorelay && !relay::live_in_place(&v) {
            if let Some(busy) = d.busy_with(&[Res::Relay, Res::Version(v.id.clone())]) {
                d.notify(&busy);
                return;
            }
            setup::live_relay(d, ctx, &v);
            d.pending_lobby = lobby;
            d.play_after_prep = d.jobs.contains_key(setup::EVENT_JOB);
            return;
        }
    }
    // A content pack that would stay off for changed game files: repair them first?
    if let Target::Installed(v) = d.target() {
        if !d.pack_play_without.contains(&v.id) {
            if let Some((pack, _)) = crate::core::launcher::mods::pack_data_changed(&v) {
                d.pending_lobby = lobby;
                ask_pack_repair(d, &v.id, &pack, true);
                return;
            }
        }
    }
    match launch::preflight(&d.state.profile) {
        Some(w) => {
            d.pending_lobby = lobby;
            d.dialogs.confirm(
                LAUNCH_ANYWAY,
                "Launch Echo VR",
                &format!("{w}\n\nLaunch anyway?"),
                DlgIcon::Warning,
            )
        }
        None => start(d, ctx, lobby),
    }
}

/// "1 server running here": dedicated servers on this PC, for the Play line.
fn servers_here(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("1 server running here".into()),
        n => Some(format!("{n} servers running here")),
    }
}

/// STOP: ends what PLAY started -- the starter (Revive's injector, EchoXR.exe) and the
/// game itself, never a game started some other way, nor a server.
/// Ends the game PLAY started, or its start while the game hasn't shown up yet.
fn stop(d: &mut Dashboard) {
    let starting = d.launched.is_some_and(|l| !l.seen) && d.local().ours.is_none();
    // Ended on purpose: the crash safety net doesn't count this run.
    d.game_stopped = true;
    if let Some(c) = d.launch_cancel.take() {
        c.store(true, Ordering::Relaxed);
    }
    if let Some(mut c) = d.child.take() {
        let _ = c.kill();
    }
    if let Some(m) = &d.monitor {
        m.stop_ours();
    }
    // Linux: the launcher Steam ran with --play, and what runs in the game's prefix (Proton
    // and its helpers would keep Steam's start going after the game).
    if cfg!(target_os = "linux") && (starting || crate::core::linux::playing().is_some()) {
        std::thread::spawn(crate::core::linux::cancel_start);
    }
    if starting {
        d.launched = None;
        d.launch_note = None;
        d.notify("Start cancelled");
    }
}

fn start(d: &mut Dashboard, ctx: &egui::Context, lobby: Option<Join>) {
    let Target::Installed(v) = d.target() else {
        return;
    };
    let exe = v.exe_path();
    if !exe.is_file() {
        d.dialogs.error(
            "Echo VR not found",
            &format!(
                "{} is missing in {}.\nRepair or reinstall it on the Install page.",
                v.exe_name(),
                v.root
            ),
            Default::default(),
        );
        return;
    }
    // The live build loads BugSplat64.dll (nEVR, the mod loader) at its start: without it,
    // or blocked, Windows only says "unable to start correctly".
    if cfg!(windows) && v.publisher_lock.is_none() {
        use crate::core::launcher::mods::{loader_problem, SlotState};
        let lead = match loader_problem(&v.bin_dir()) {
            None | Some((_, SlotState::Ok)) => None,
            Some((file, SlotState::Missing)) => Some(format!(
                "{file}, part of Echo VR's mod loader (nEVR), is missing from {}.",
                v.bin_dir().display()
            )),
            Some((file, SlotState::Blocked(e))) => Some(format!(
                "{file}, part of Echo VR's mod loader (nEVR), can't be read ({e})."
            )),
        };
        if let Some(lead) = lead {
            ask_nevr_blocked(d, &v.id, &lead);
            return;
        }
    }
    // An event build plays as your classic lobbies account (asked for before PLAY gets here).
    if v.publisher_lock.is_some() && d.state.relay_account.is_none() {
        return;
    }
    // The folder as a start needs it: mods and the server (nEVR's config.yaml), EchoRelay's
    // or the classic lobbies' files, EchoXR. The keeper writes it whenever it changes; once
    // more now. Where EchoXR needs administrator rights (the Meta library's), PLAY's
    // preparation does it.
    if let Err(f) = ready::prepare_start(&d.state, &v) {
        if f.step == ready::Part::EchoXr && revive::needs_elevation(&f.error) {
            d.notify("EchoXR needs administrator rights for this folder: Echo VR starts once it's set up");
            setup::echoxr_windows(d, ctx);
            d.pending_lobby = lobby;
            d.play_after_prep = d.jobs.contains_key(setup::ECHOXR_JOB);
        } else {
            d.dialogs.error(
                f.step.title(),
                &format!("{:#}", f.error),
                Default::default(),
            );
        }
        return;
    }
    d.ready.applied(&d.state, &v);
    // Linux: through Steam's shortcut, which runs the launcher with --play.
    if cfg!(target_os = "linux") {
        let Some((root, appid)) = crate::core::linux::steam::root().zip(d.state.linux_appid) else {
            d.dialogs.error(
                "Steam not found",
                "Echo VR runs through Steam on Linux: install Steam, sign in once, and press PLAY again.",
                Default::default(),
            );
            return;
        };
        let spectate = lobby.as_ref().is_some_and(|j| j.spectate);
        if d.state.profile.runtime == Runtime::Revive
            && !spectate
            && crate::core::linux::echoxr::steamvr_in_flatpak()
        {
            d.dialogs.error(
                "SteamVR in the Flatpak Steam",
                crate::core::linux::echoxr::STEAMVR_IN_FLATPAK,
                Default::default(),
            );
            return;
        }
        // The Flatpak Steam's shortcut hands the start to this launcher, wherever it is now.
        if let Err(e) = crate::core::linux::refresh_play_script(&root) {
            d.dialogs.error(
                "Couldn't start Echo VR",
                &format!("{e:#}"),
                Default::default(),
            );
            return;
        }
        crate::core::linux::set_next_lobby(lobby.as_ref().map(|j| (j.lobby.as_str(), j.spectate)));
        // Steam may have to start first: on the worker, STOP can cancel it meanwhile.
        let cancel = Arc::new(AtomicBool::new(false));
        d.launch_cancel = Some(cancel.clone());
        d.launch_note = Some("Starting Echo VR".into());
        d.launched = Some(super::Launched::now());
        let (version, bin, root_dir) = (v.id.clone(), v.bin_dir(), v.root.clone());
        d.worker.spawn(ctx, move |tx| {
            let result = crate::core::linux::steam::run_when_up(&root, appid, &cancel, &mut |n| {
                tx.send(Msg::LaunchNote(n.to_string()))
            })
            .map_err(|e| format!("{e:#}"));
            tx.send(Msg::LinuxStarted {
                result,
                version,
                bin,
                root: root_dir,
            });
        });
        return;
    }
    let revive_dir = match (d.state.profile.runtime, d.state.profile.steamvr_via) {
        (Runtime::Revive, SteamVrVia::Revive) => revive::find_revive_dir(),
        _ => None,
    };
    // Watching: on the monitor, as the spectator stream.
    let mut profile = d.state.profile.clone();
    if lobby.as_ref().is_some_and(|j| j.spectate) {
        profile.runtime = Runtime::Flat;
        profile.spectator = true;
    }
    // Virtual Desktop's Oculus mode: its streamer starts the game.
    let vd_streamer = (profile.runtime == Runtime::VirtualDesktop && !profile.through_echoxr())
        .then(launch::vd_streamer)
        .flatten();
    let tools = launch::Tools {
        revive_dir: revive_dir.as_deref(),
        vd_streamer: vd_streamer.as_deref(),
    };
    // What the streamer can't hand the game (event builds start without options anyway).
    let left_out = match v.publisher_lock {
        Some(_) => Vec::new(),
        None => launch::vd_left_out(&profile, lobby.as_ref().map(|j| j.lobby.as_str())),
    };
    let command = if v.publisher_lock.is_some() {
        launch::build_relay(&profile, &exe, &tools)
    } else {
        launch::build(
            &profile,
            &exe,
            &tools,
            lobby.as_ref().map(|j| j.lobby.as_str()),
        )
    };
    // EchoXR Hands: its OpenXR layer in the game's loader, when the live build plays
    // through EchoXR.
    let through_echoxr = profile.through_echoxr();
    let hands = through_echoxr && v.publisher_lock.is_none() && profile.hands(d.state.echoxr_hands);
    let command = command.map(|mut c| {
        if hands {
            c.env
                .extend(crate::core::echoxr_hands::layer_env(&v.bin_dir(), |p| {
                    p.to_string_lossy().replace('/', "\\")
                }));
        }
        c
    });
    let result = command.and_then(|c| launch::spawn(&c));
    match result {
        Ok(child) => {
            if let Some(m) = &d.monitor {
                m.launched(Some(child.id()), &v.id, &v.bin_dir());
            }
            d.child = Some(child);
            d.child_echoxr = through_echoxr;
            d.launched = Some(super::Launched::now());
            d.login_watch = Some(LoginWatching::new(&v.root, &profile));
            if !left_out.is_empty() {
                d.notify(if lobby.is_some() {
                    "VD's Oculus mode can't join matches: Echo VR started without joining"
                } else {
                    "VD's Oculus mode can't pass launch options: Echo VR started without them"
                });
            }
            if d.state.minimize_on_launch {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
        Err(e) => d.dialogs.error(
            "Couldn't start Echo VR",
            &format!("{e:#}"),
            Default::default(),
        ),
    }
}

// ---- what EchoVRCE says to a login ----

/// How long after PLAY the game's log is watched for a login EchoVRCE turned down.
const LOGIN_WATCH_FOR: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// The login watch of the start PLAY made: polled once a second while the game may run.
pub(super) struct LoginWatching {
    watch: LoginWatch,
    at: std::time::Instant,
    since: std::time::Instant,
    /// Virtual Desktop's Oculus mode: its start is watched for VR that didn't come up (no
    /// headset, or no swap chain for it).
    vd_meta: bool,
}

impl LoginWatching {
    pub fn new(root: &str, profile: &LaunchProfile) -> LoginWatching {
        // A little slack: the game may open its log in the same second PLAY ran.
        let since = std::time::SystemTime::now() - std::time::Duration::from_secs(2);
        LoginWatching {
            watch: LoginWatch::new(root, since, crate::core::launcher::nevr::log_dirs()),
            at: std::time::Instant::now(),
            since: std::time::Instant::now(),
            vd_meta: profile.runtime == Runtime::VirtualDesktop && profile.vd_via == VdVia::Meta,
        }
    }
}

/// Every frame: reads the game's log once a second while the launcher's game runs. When
/// EchoVRCE turned the login down, the game is closed and the card says what it wants
/// (the code to pick in its Discord DM). Done when the game is gone.
pub(super) fn watch_login(d: &mut Dashboard, ctx: &egui::Context) {
    let ours = d.ours();
    let Some(w) = d.login_watch.as_mut() else {
        return;
    };
    let starting = w.since.elapsed() < crate::core::launcher::game::LAUNCH_WAIT;
    // A login is turned down around the start; after that the watch rests.
    if (!ours && !starting) || w.since.elapsed() > LOGIN_WATCH_FOR {
        d.login_watch = None;
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_secs(1));
    if w.at.elapsed() < std::time::Duration::from_secs(1) {
        return;
    }
    w.at = std::time::Instant::now();
    // Virtual Desktop's Oculus mode without VR (no headset, or no swap chain for it): the
    // game stops with an error; offer EchoXR instead.
    if w.vd_meta && starting && w.watch.swap_chain_failed() {
        tracing::info!("Virtual Desktop's Oculus mode didn't start VR: closing the game");
        d.login_watch = None;
        stop(d);
        ask_vd_switch(d);
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        return;
    }
    let Some(notice) = w.watch.poll().into_iter().next() else {
        return;
    };
    tracing::info!(
        "EchoVRCE turned the login down{}: closing the game",
        if notice.confirm_location() {
            " (confirm this location)"
        } else {
            ""
        }
    );
    d.login_watch = None;
    stop(d);
    d.overlay = Some(setup::Overlay::LoginNotice(notice));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}

/// Content pack `pack` of version `id` stays off: an older installer changed the game
/// data it needs. Repair the game files? At PLAY (`at_play`) it can also start without
/// the pack (answered in `answers`).
pub(super) fn ask_pack_repair(d: &mut Dashboard, id: &str, pack: &str, at_play: bool) {
    d.pack_repair = Some(id.to_string());
    let body = format!(
        "{pack} can't run on this copy of Echo VR: its game files were changed, usually by an older installer (such as {pack}'s own install.bat).\n\n\
         Repair game files checks every game file against the build's and downloads only the changed ones again. Your settings stay, and so do your mods."
    );
    if at_play {
        let without = format!("Play without {pack}");
        d.dialogs.options(
            PACK_DATA_PLAY,
            "Repair the game files?",
            &body,
            DlgIcon::Warning,
            &["Repair game files", &without, "Cancel"],
        );
    } else {
        d.dialogs.options(
            PACK_DATA,
            "Repair the game files?",
            &body,
            DlgIcon::Warning,
            &["Repair game files", "Not now"],
        );
    }
}

/// Echo VR can't start for its mod loader in version `id`'s folder (`lead`: what happened
/// to it): most likely antivirus. Allow it in Windows Security and repair, or open Windows
/// Security (answered in `show`).
pub(super) fn ask_nevr_blocked(d: &mut Dashboard, id: &str, lead: &str) {
    tracing::warn!("{id}: {lead}");
    d.nevr_blocked = Some(id.to_string());
    let folder = d
        .state
        .version(id)
        .map(|v| v.bin_dir().display().to_string())
        .unwrap_or_else(|| "the game's folder".into());
    d.dialogs.options(
        NEVR_BLOCKED,
        "Windows blocked Echo VR's mod loader",
        &format!(
            "{lead} Echo VR can't start without it.\n\n\
             That is usually antivirus: Windows Security (or another antivirus) takes it for a threat and removes or blocks it. \
             Allow and repair adds {folder} to Windows Security's exclusions (it asks for administrator rights) and puts the file back.\n\n\
             Or by hand: Windows Security, Virus & threat protection, Protection history: allow BugSplat64.dll, then update Echo VR. \
             With another antivirus, allow that folder there."
        ),
        DlgIcon::Warning,
        &["Allow and repair", "Open Windows Security", "Close"],
    );
}

/// Adds version `id`'s game folder to Windows Security's exclusions (asking for
/// administrator rights), then updates it, which puts the mod loader back.
fn allow_and_repair(d: &mut Dashboard, ctx: &egui::Context, id: &str) {
    let Some(v) = d.state.version(id).cloned() else {
        return;
    };
    let mut consent = setup::consent_asker(d.worker.tx(ctx));
    let bin = v.bin_dir();
    d.start_job(
        ctx,
        super::JobKind::Update,
        id,
        vec![super::Res::Version(id.to_string())],
        &format!("Allowing {} in Windows Security", v.name),
        "Asking for administrator rights...",
        move |cancel, on| {
            let r =
                crate::core::elevation::exclude_from_defender(&bin, &mut consent).and_then(|()| {
                    on(crate::core::launcher::versions::Step::Status(
                        "Putting the mod loader back...".into(),
                    ));
                    crate::core::launcher::versions::update(&v, cancel, on)
                });
            match r {
                Ok(()) => super::JobResult::Updated,
                Err(e) => versions::job_err(e, "Couldn't allow Echo VR"),
            }
        },
    );
}

/// Virtual Desktop's Oculus mode couldn't start VR: through EchoXR instead
/// (answered in `show`)?
pub(super) fn ask_vd_switch(d: &mut Dashboard) {
    d.dialogs.options(
        VD_SWITCH,
        "Virtual Desktop couldn't start VR",
        "Echo VR got no headset picture in Virtual Desktop's Oculus mode, so the game was closed. Is Virtual Desktop connected from your headset?\n\nOr play another way: straight through Virtual Desktop (nothing else needed), or through SteamVR (needs SteamVR installed). Echo VR starts again the new way; Settings can change it back.",
        DlgIcon::Warning,
        &["Use Virtual Desktop directly", "Use SteamVR", "Keep Oculus mode"],
    );
}

/// The card for a login EchoVRCE turned down: the code to pick in its Discord DM, or its
/// message; START AGAIN once it's done.
pub(super) fn login_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let Some(setup::Overlay::LoginNotice(n)) = d.overlay.clone() else {
        return;
    };
    let location = n.confirm_location();
    let (w, h) = (dz(900.0), dz(if location { 400.0 } else { 360.0 }));
    let title = if location {
        "Confirm this PC"
    } else {
        "EchoVRCE turned the login down"
    };
    let (x, y, cw, bottom) = setup::card(k, w, h, title);
    let mut ty = y;
    if location {
        ty += k.caps_text(
            x,
            ty,
            cw,
            "EchoVRCE doesn't know this location yet. Open its direct message in Discord (from @EchoVRCE) and pick this code:",
            17.0,
            design::BODY,
            0.0,
        ) + dz(24.0);
        let code = n.code.clone().unwrap_or_else(|| "?".into());
        let g = k.label_galley(&code, design::conthrax(54.0), design::TEXT, cw);
        let gw = g.size().x;
        k.put(x + (cw - gw) / 2.0, ty, g);
        ty += dz(84.0);
        k.caps_text(
            x,
            ty,
            cw,
            "Then start Echo VR again: the login goes through.",
            15.0,
            design::GREY,
            0.0,
        );
    } else {
        ty += k.caps_text(
            x,
            ty,
            cw,
            "Echo VR was closed. EchoVRCE says:",
            17.0,
            design::BODY,
            0.0,
        ) + dz(18.0);
        k.caps_text(x, ty, cw, &n.message, 16.0, design::TEXT, dz(4.0));
    }

    let by = bottom - BTN_H;
    let right = x + cw;
    let close_w = k.button_width("Close", None, BTN_H).max(dz(140.0));
    if k.button(
        "login-close",
        right - close_w,
        by,
        close_w,
        BTN_H,
        Tone::Dark,
        None,
        "Close",
        true,
        "",
    )
    .clicked
        || k.key(egui::Key::Escape)
    {
        d.overlay = None;
        return;
    }
    let again_w = k.button_width("Start again", None, BTN_H).max(dz(200.0));
    if k.button(
        "login-again",
        right - close_w - dz(10.0) - again_w,
        by,
        again_w,
        BTN_H,
        Tone::Go,
        None,
        "Start again",
        !d.game().is_running(),
        "Start Echo VR again",
    )
    .clicked
    {
        d.overlay = None;
        try_start(d, ctx, None);
        return;
    }
    if location {
        let dw = k.button_width("Open Discord", None, BTN_H).max(dz(200.0));
        if k.button(
            "login-discord",
            x,
            by,
            dw,
            BTN_H,
            Tone::Blue,
            None,
            "Open Discord",
            true,
            "Open your direct messages in Discord",
        )
        .clicked
            && !d.demo
        {
            crate::core::platform::open_url("https://discord.com/channels/@me");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::launcher::mods::tests::{live, with_pack, with_pack_data};

    /// A window whose selected version has pack "Test" on, built from game data that an
    /// older installer changed (or not, `stock`).
    fn window(dir: &std::path::Path, stock: bool) -> (Dashboard, egui::Context) {
        let v = live(dir);
        with_pack(&v);
        with_pack_data(&v, stock);
        let mut d = Dashboard::default();
        d.state.selected = Some(v.id.clone());
        d.state.versions = vec![v];
        d.platform = Platform::Pc;
        (d, egui::Context::default())
    }

    #[test]
    fn play_asks_to_repair_game_files_a_pack_needs() {
        let dir = tempfile::tempdir().unwrap();
        let (mut d, ctx) = window(dir.path(), false);
        try_start(&mut d, &ctx, None);
        assert_eq!(d.dialogs.open_keys(), vec![PACK_DATA_PLAY]);
        assert!(d.overlay.is_none());

        // Cancel: nothing starts, and PLAY asks again.
        d.dialogs.press(PACK_DATA_PLAY, Answer::Button(2));
        answers(&mut d, &ctx);
        assert!(d.overlay.is_none() && d.pack_play_without.is_empty());
        try_start(&mut d, &ctx, None);
        assert_eq!(d.dialogs.open_keys(), vec![PACK_DATA_PLAY]);

        // Play without it: the start goes on (to "Echo VR not found" here, the test
        // folder has no game), and isn't asked about again this run.
        d.dialogs.press(PACK_DATA_PLAY, Answer::Button(1));
        answers(&mut d, &ctx);
        assert!(d.pack_play_without.contains("pc-latest"));
        assert!(!d.dialogs.open_keys().contains(&PACK_DATA_PLAY));
        assert!(d.overlay.is_none());

        // Repair: without the catalogue read it says so instead of guessing a build.
        let (mut d, ctx) = window(dir.path(), false);
        try_start(&mut d, &ctx, None);
        d.dialogs.press(PACK_DATA_PLAY, Answer::Button(0));
        answers(&mut d, &ctx);
        assert_eq!(d.dialogs.open_keys().len(), 1);
        assert!(d.overlay.is_none());
    }

    #[test]
    fn play_doesnt_ask_with_game_files_as_they_shipped() {
        let dir = tempfile::tempdir().unwrap();
        let (mut d, ctx) = window(dir.path(), true);
        try_start(&mut d, &ctx, None);
        assert!(!d.dialogs.open_keys().contains(&PACK_DATA_PLAY));
        assert!(d.overlay.is_none());
    }
}
