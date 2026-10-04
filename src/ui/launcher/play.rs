//! Play page, as the design concept has it: the ECHO VR logo over an info line, PLAY,
//! CHECK FOR UPDATES and the PCVR|QUEST switch, Community News (a banner and two cards)
//! and RIGHT NOW on the right (what is going on, and your friends). PLAY does what the selected version needs (patch, set up
//! SteamVR, play, stop); on the Quest side it connects or starts Echo VR on the headset.
//! With nothing installed it is grey and opens the Install page. While a job runs on the
//! installed game, PLAY fills up with its progress and CHECK FOR UPDATES becomes CANCEL.

use egui::Color32;

use super::hero::{self, Face, InfoLine, JobView, PathClick, Row, Side};
use super::{now, panel, setup, versions, Dashboard, Msg, Page};
use crate::core::adb::devices::Status;
use crate::core::error::UiError;
use crate::core::launcher::catalog::{Platform, VersionEntry};
use crate::core::launcher::feed::NewsItem;
use crate::core::launcher::store::{InstalledVersion, Runtime, SteamVrVia, Target};
use crate::core::launcher::{launch, quest, relay};
use crate::core::revive;
use crate::ui::design::{self, dz, Dr};
use crate::ui::dialogs::Icon as DlgIcon;
use crate::ui::kit::Kit;
use crate::ui::markdown::{self, Look};
use crate::ui::style::{self, Icon};
use crate::ui::widgets::{MenuItem, Tone, BTN_H};
use egui::pos2;

const LAUNCH_ANYWAY: &str = "launch-anyway";

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
    // Look for a headset quietly once, so the Quest chip has something to say.
    if !d.quest_auto_checked && !d.demo && !kit.ghost {
        d.quest_auto_checked = true;
        if d.quest_conn.status.is_none() && !d.quest_conn.checking {
            d.check_quest(ctx, false);
        }
    }
    if !kit.ghost {
        if let Some(answer) = d.dialogs.take(LAUNCH_ANYWAY) {
            let lobby = d.pending_lobby.take();
            if answer.is_yes() {
                start(d, ctx, lobby);
            }
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

/// What PLAY does.
enum Main {
    Play,
    Stop,
    Patch(String),
    SetUpRevive,
    /// SteamVR through EchoXR (Windows).
    SetUpEchoXr,
    /// Linux: GE-Proton, EchoXR and the Steam shortcut.
    SetUpLinux,
    QuestConnect,
    QuestPlay,
    /// Close Echo VR on the headset.
    QuestStop,
    /// Nothing to play yet: open the Install page.
    ToInstall,
    Nothing,
}

/// What CHECK FOR UPDATES does.
enum Update {
    Pc(InstalledVersion),
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
    /// The last update failed: the button shows its orange "!".
    update_alert: bool,
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
            update_alert: false,
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

/// Why PLAY and CHECK FOR UPDATES can't be used right now, in their tooltips.
fn explain_disabled(d: &Dashboard, a: &mut Action) {
    let busy = d
        .jobs
        .values()
        .next()
        .map(|j| format!("Busy: {}. Wait until it's done.", j.title));
    if !a.enabled && !matches!(a.main, Main::Nothing) {
        if let Some(b) = &busy {
            a.tip = b.clone();
        }
    }
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
        .or_else(|| hero::job_view(d, setup::ECHOXR_JOB));
    let mut a = Action::new();
    let needs_steamvr = d.steamvr_missing();
    let echoxr = d.state.profile.runtime == Runtime::Revive
        && d.state.profile.steamvr_via == SteamVrVia::EchoXr;
    let local = d.local();
    // Echo VR clients: the launcher's own game, or one started elsewhere. Servers on this
    // PC don't count (they never block PLAY).
    let running = local.state.is_running();
    let ours_running = local.ours.is_some();
    let ours = d.ours();
    match target {
        Target::Installed(v) => {
            a.job = job;
            let size = v.catalog_id.as_ref().and_then(|cid| {
                let c = d.catalog.as_ref()?;
                c.pc().find(|e| &e.id == cid)?.size.map(gb)
            });
            let event = v.publisher_lock.is_some();
            let state = if running {
                "Running"
            } else if ours {
                "Starting"
            } else if setup::needs_patch(d, &v) {
                "Needs the licence patch"
            } else if needs_steamvr {
                "SteamVR is not set up"
            } else if event {
                "Classic lobby"
            } else {
                "Installed"
            };
            a.line.parts.push(state.into());
            a.line.parts.push(v.name.clone());
            // An event build plays on the classic lobbies server.
            if event {
                a.line.parts.push(d.state.relay_server.clone());
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
                // Clicked: the grey PLAY until the game shows up (or its process ends).
                a.tip = "Starting Echo VR…".into();
            } else if setup::needs_patch(d, &v) {
                (a.label, a.main) = ("PATCH", Main::Patch(v.id.clone()));
                a.enabled = !d.any_job();
                a.tip =
                    "New players need a personal licence patch: get yours through Discord".into();
            } else if needs_steamvr && echoxr {
                (a.label, a.main) = ("SET UP", Main::SetUpEchoXr);
                a.enabled = !d.any_job();
                a.tip =
                    "Set up SteamVR through EchoXR: its OpenXR runtime goes into the game's folder"
                        .into();
            } else if needs_steamvr {
                (a.label, a.main) = ("SET UP", Main::SetUpRevive);
                a.enabled = !d.any_job();
                a.tip = "Set up SteamVR: installs Revive, which runs Echo VR on SteamVR (asks for administrator rights)".into();
            } else if echoxr && cfg!(windows) && v.publisher_lock.is_some() {
                (a.main, a.enabled, a.grey) = (Main::Play, false, true);
                a.tip =
                    "Event builds don't run through EchoXR: choose Revive for SteamVR in Settings"
                        .into();
            } else if cfg!(target_os = "linux") && v.publisher_lock.is_some() {
                (a.main, a.enabled, a.grey) = (Main::Play, false, true);
                a.tip =
                    "Event builds don't run on Linux yet: EchoXR runs only the live build".into();
            } else if cfg!(target_os = "linux") && !setup::pc_play_supported(d) {
                (a.label, a.main) = ("SET UP", Main::SetUpLinux);
                a.enabled = !d.any_job();
                a.tip = "Set up Echo VR for Linux: GE-Proton and EchoXR's OpenXR runtime (about 0.5 GB of downloads), then a shortcut in Steam that starts it (Steam restarts)".into();
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
            a.update_enabled = updates && in_use.is_none() && !ours && !d.any_job();
            if ours && !running {
                a.update_tip = "Echo VR is starting".into();
            }
            if let Some(why) = in_use {
                a.update_tip = why.into();
            }
            if !updates {
                a.update_tip = "Event builds don't get updates: REINSTALL on the Install page checks their files".into();
            }
            a.update_alert = d
                .update_note
                .get(&v.id)
                .is_some_and(|n| n.contains("failed"));
            a.update = Update::Pc(v);
        }
        Target::Missing(_) => {
            a.not_installed("Game files missing", job.as_ref());
            if !job.as_ref().is_some_and(JobView::installs) {
                a.line.color = design::DANGER;
                a.tip =
                    "The game files are gone. Click to install Echo VR again or add its new folder"
                        .into();
            }
        }
        Target::Available(_) | Target::None => a.not_installed("Not installed", job.as_ref()),
    }
    a
}

// ---- Quest ----

fn quest_action(d: &mut Dashboard) -> Action {
    let ready = d.quest_conn.status == Some(Status::Ready);
    let installed = ready && d.quest_info.as_ref().is_some_and(|i| i.installed);
    let known = ready && d.quest_info.is_some();
    let busy = d.quest_conn.checking || d.quest_busy || d.any_job();
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
        a.tip = "Look for your Quest over USB".into();
    }
    a.update = Update::Quest;
    a.update_enabled = installed && !busy;
    a.update_tip = "Copy the latest game files to your Quest".into();
    a
}

/// What the headset's connection and install are, for an info line.
pub(super) fn quest_info(d: &Dashboard) -> Vec<String> {
    let device = d.quest_info.as_ref().and_then(|i| i.device.clone());
    match (d.quest_conn.checking, d.quest_conn.status) {
        (true, _) => vec!["Looking for your headset over USB".into()],
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
        JobKind::Reinstall => "Reinstalling".into(),
        JobKind::Update => "Updating".into(),
        JobKind::Verify => "Verifying".into(),
        JobKind::Patch => "Patching".into(),
        JobKind::Unpatch => "Removing the patch".into(),
        JobKind::Licence => "Licence patch".into(),
        JobKind::Background => "Converting".into(),
        JobKind::Revive | JobKind::QuestUpdate | JobKind::Mods => job.title.clone(),
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
        side: Side::Updates {
            alert: a.update_alert,
        },
        side_enabled: a.update_enabled,
        side_tip: &a.update_tip,
    };
    let (main, update) = hero::row(kit, "play", 0.0, &row);
    if main {
        match a.main {
            Main::Play => try_start(d, ctx, None),
            Main::Stop => stop(d),
            Main::Patch(id) => d.overlay = Some(setup::licence(&id)),
            Main::SetUpRevive => setup::revive(d, ctx),
            Main::SetUpEchoXr => setup::echoxr_windows(d, ctx),
            Main::SetUpLinux => setup::linux_setup(d, ctx),
            Main::QuestConnect => d.check_quest(ctx, true),
            Main::QuestPlay => quest_launch(d, ctx),
            Main::QuestStop => quest_stop(d, ctx),
            Main::ToInstall => {
                // The Install page opens on the version picked here.
                d.install_pick = match d.target() {
                    Target::Available(e) => Some(e.id),
                    Target::Missing(v) => v.catalog_id,
                    _ => None,
                };
                d.page = Page::Install;
            }
            Main::Nothing => {}
        }
    }
    if update {
        match a.update {
            Update::Pc(v) => versions::update(d, ctx, v),
            Update::Quest => setup::quest_update(d, ctx),
            Update::Nothing => {}
        }
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
        || ctx.input(|i| i.key_pressed(egui::Key::Escape))
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
        || (id.is_some()
            && (direct || signed_in)
            && ctx.input(|i| i.key_pressed(egui::Key::Enter)));
    if let (true, Some(id)) = (join, id) {
        d.overlay = None;
        d.state.last_lobby = text;
        d.save();
        if direct {
            try_start(d, ctx, Some(id));
        } else {
            super::servers::queue_lobby(d, ctx, &id.lobby);
        }
    }
}

// ---- the version picker ----

/// VERSION, right of the switch: the version PLAY starts, and a menu to switch to
/// another. Installed versions come first, then the catalogue's others: picking one of
/// those greys PLAY, which then opens the Install page on it.
fn version_picker(d: &mut Dashboard, kit: &mut Kit) {
    let installed = d.state.versions.clone();
    let available: Vec<VersionEntry> = d
        .catalog
        .as_ref()
        .map(|c| {
            c.pc()
                .filter(|e| d.state.installed_from(&e.id).is_none())
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
    let busy = d
        .jobs
        .values()
        .next()
        .map(|j| format!("Busy: {}. Wait until it's done.", j.title));
    let (enabled, tip) = match (running, busy) {
        (true, _) => (false, "Close Echo VR to switch versions".to_string()),
        (false, Some(b)) => (false, b),
        (false, None) => (true, "The version PLAY starts: click to switch".to_string()),
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

    let r = kit.drect(PICKER);
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
    let g = kit.spaced_fit(
        &current_name.to_uppercase(),
        design::din(17.0),
        fg,
        dz(1.2),
        false,
        r.width() - 2.0 * pad - chevron - dz(10.0),
    );
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
            let (detail, detail_color) = if !present {
                ("Files missing".to_string(), design::DANGER)
            } else if v.external {
                ("Existing folder".to_string(), design::GREY)
            } else {
                let size = v.catalog_id.as_ref().and_then(|cid| {
                    let c = d.catalog.as_ref()?;
                    c.pc().find(|e| &e.id == cid)?.size.map(gb)
                });
                (size.unwrap_or_default(), design::GREY)
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
    items.extend(available.iter().map(|e| MenuItem::Pick {
        label: e.name.clone(),
        detail: match e.size {
            Some(s) => format!("Not installed  ·  {}", gb(s)),
            None => "Not installed".into(),
        },
        detail_color: design::GREY,
        checked: checked(&e.id),
        tip: "Not installed yet: PLAY takes you to the Install page".into(),
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
    let tex = main
        .as_ref()
        .and_then(|m| d.feed.texture(m.image.as_deref()))
        .cloned();
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
    let link = match (&news, &main) {
        _ if fallback => Some((
            "How to play".to_string(),
            crate::core::LOUNGE_INVITE.to_string(),
        )),
        (_, Some(m)) if !m.link_label.is_empty() && !m.link_url.is_empty() => {
            Some((m.link_label.clone(), m.link_url.clone()))
        }
        _ => None,
    };
    if let Some((label, url)) = link {
        let (right, base) = NEWS_LINK;
        let g = kit.spaced_fit(
            &label.to_uppercase(),
            design::conthrax(21.0),
            design::TEXT,
            dz(5.5),
            true,
            dz(700.0),
        );
        let r = kit.put(dz(right + dx) - g.size().x, dz(base) - g.size().y, g);
        if kit.click_area("news-link", r, &url) {
            crate::core::platform::open_url(&url);
        }
    }

    let (main_card, community_card) = cards(news.as_ref().map(|n| n.slots.clone()), main);
    // Side by side, sharing the extra width; taller in a taller window.
    let (half, dy) = (dx / 2.0, kit.dy());
    card(kit, 0, CARDS[0].wider(half).taller(dy), &main_card, false);
    let second = CARDS[1].moved(half).wider(half).taller(dy);
    card(kit, 1, second, &community_card, true);
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
            "Your launcher for Echo VR, on PC and on your Quest.\n\n\
             - Install, update and play from one place\n\
             - Switch between PCVR and Quest next to PLAY\n\
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
    // A version not installed here: the licence question first, once.
    let unpatched = matches!(d.target(), Target::Installed(v) if !v.patched);
    if d.state.owner.is_none() && unpatched && !d.demo {
        d.pending_lobby = lobby;
        d.overlay = Some(setup::Overlay::Owner);
        return;
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
fn stop(d: &mut Dashboard) {
    if let Some(mut c) = d.child.take() {
        let _ = c.kill();
    }
    if let Some(m) = &d.monitor {
        m.stop_ours();
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
    // An event build: pointed at the classic lobbies server as your account each time,
    // so a changed account or server applies.
    if v.publisher_lock.is_some() {
        let Some(account) = d.state.relay_account.clone() else {
            return;
        };
        if let Err(e) = relay::write_config(&v, &d.state.relay_server, &account) {
            d.dialogs.error(
                "Couldn't set up the classic lobby",
                &format!("{e:#}"),
                Default::default(),
            );
            return;
        }
    }
    // The live build with nEVR: its plugins list (config.yaml) as the Mods page has it.
    if v.publisher_lock.is_none() {
        if let Err(e) = crate::core::launcher::mods::before_start(&v) {
            d.dialogs.error(
                "Couldn't prepare the mods",
                &format!("{e:#}"),
                Default::default(),
            );
            return;
        }
    }
    // Linux: through Steam's shortcut, which runs the launcher with --play.
    if cfg!(target_os = "linux") {
        let Some((root, appid)) = crate::core::linux::steam::root().zip(d.state.linux_appid) else {
            return;
        };
        crate::core::linux::set_next_lobby(lobby.as_ref().map(|j| (j.lobby.as_str(), j.spectate)));
        match crate::core::linux::steam::run(&root, appid) {
            Ok(()) => {
                if let Some(m) = &d.monitor {
                    m.launched(None, &v.id, &v.bin_dir());
                }
                d.launched = Some(super::Launched::now());
            }
            Err(e) => d.dialogs.error(
                "Couldn't start Echo VR",
                &format!("{e:#}"),
                Default::default(),
            ),
        }
        return;
    }
    let revive_dir = match (d.state.profile.runtime, d.state.profile.steamvr_via) {
        (Runtime::Revive, SteamVrVia::Revive) => revive::find_revive_dir(),
        _ => None,
    };
    // SteamVR through EchoXR: EchoXR into the game's folder and its copy of the game
    // current. Where that needs administrator rights (the Meta library's), SET UP's job
    // does it.
    if cfg!(windows)
        && d.state.profile.runtime == Runtime::Revive
        && d.state.profile.steamvr_via == SteamVrVia::EchoXr
        && v.publisher_lock.is_none()
    {
        let bin = v.bin_dir();
        let platform = crate::core::echoxr::platform_dir_for(&bin);
        if let Err(e) = crate::core::echoxr::prepare(&bin, platform.as_deref()) {
            if revive::needs_elevation(&e) {
                d.notify("EchoXR needs administrator rights for this folder: PLAY again once it's set up");
                setup::echoxr_windows(d, ctx);
            } else {
                d.dialogs.error(
                    "Couldn't set up EchoXR",
                    &format!("{e:#}"),
                    Default::default(),
                );
            }
            return;
        }
    }
    // Watching: on the monitor, as the spectator stream.
    let mut profile = d.state.profile.clone();
    if lobby.as_ref().is_some_and(|j| j.spectate) {
        profile.runtime = Runtime::Flat;
        profile.spectator = true;
    }
    let command = if v.publisher_lock.is_some() {
        launch::build_relay(&profile, &exe, revive_dir.as_deref())
    } else {
        launch::build(
            &profile,
            &exe,
            revive_dir.as_deref(),
            lobby.as_ref().map(|j| j.lobby.as_str()),
        )
    };
    let result = command.and_then(|c| launch::spawn(&c));
    match result {
        Ok(child) => {
            if let Some(m) = &d.monitor {
                m.launched(Some(child.id()), &v.id, &v.bin_dir());
            }
            d.child = Some(child);
            d.child_echoxr = profile.runtime == Runtime::Revive
                && profile.steamvr_via == SteamVrVia::EchoXr
                && v.publisher_lock.is_none();
            d.launched = Some(super::Launched::now());
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
