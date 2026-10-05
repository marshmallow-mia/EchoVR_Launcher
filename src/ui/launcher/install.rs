//! Install page, laid out like the Play page: a hero panel with the chosen version (a big
//! logo, the Play page's info line, buttons and switch), every version under it, and,
//! where RIGHT NOW sits on Play, your library with adding one you already have. On the Quest
//! side the hero installs Echo VR on the headset, two cards explain how, and the panel
//! shows the headset. That right-hand card is `install_panel`.

use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, Galley};

use super::hero::{self, Face, InfoLine, JobView, PathClick, Row, Side};
use super::{install_panel, play, setup, versions, Dashboard, Page};
use crate::core::adb::devices::Status;
use crate::core::launcher::catalog::{Hosted, Platform, VersionEntry};
use crate::core::launcher::store::InstalledVersion;
use crate::core::platform;
use crate::ui::design::{self, dz, Dr};
use crate::ui::dialogs::{DEV_MODE_URL, USB_DEBUGGING_URL};
use crate::ui::kit::Kit;
use crate::ui::style::Icon;

// Geometry in design pixels.
const HERO: Dr = Dr::new(138.0, 80.0, 1144.0, 520.0);
const HEADER_H: f32 = 46.0;
/// The Play page's info line and buttons move into the hero: this far right and `ROW_DY`
/// lower than on Play.
const DX: f32 = 32.0;
const ROW_DY: f32 = 256.0;
/// The logo: centred, and larger than on Play.
const LOGO_W: f32 = 1047.0;
const LOGO: Dr = Dr::new(
    HERO.x + (HERO.w - LOGO_W) / 2.0,
    153.0,
    LOGO_W,
    LOGO_W * 126.5 / 747.7,
);
/// The version's name with its tags, and its notes, between the logo and the info line.
const TEXT_X: f32 = hero::INFO_X + DX;
const NAME_Y: f32 = 352.0;
const NAME_SIZE: f32 = 32.0;
const TAG_SIZE: f32 = 16.0;
const NOTES_Y: f32 = 404.0;
const NOTES_W: f32 = HERO.w - 2.0 * (TEXT_X - HERO.x);
/// VERSIONS under the hero (PC); the Quest side has How it works there.
const VERSIONS: Dr = Dr::new(137.0, 630.0, 1146.0, 416.0);
/// One row of the VERSIONS card, how far its highlight reaches past the text, the room
/// right of its chip, and the gap with the line between the main servers' builds and the
/// rest.
const ROW_H: f32 = 64.0;
const ROW_PAD: f32 = 12.0;
const CHIP_INSET: f32 = 10.0;
const DIVIDER_H: f32 = 32.0;
/// The tags of the main servers' builds: LIVE BUILD red, EVENT BUILD orange.
const LIVE: Color32 = Color32::from_rgb(255, 0, 0);
const EVENT: Color32 = Color32::from_rgb(255, 174, 0);
/// The line under the main servers' builds, and the rows' summaries.
const DIVIDER: Color32 = Color32::from_rgba_unmultiplied_const(214, 210, 230, 190);
const SUMMARY: Color32 = Color32::from_rgb(150, 146, 166);

pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    // A wider window widens the hero and VERSIONS (the right panel moves right); a
    // taller one makes VERSIONS and the Quest cards taller.
    let h = HERO.wider(kit.dx());
    kit.panel(dz(h.x), dz(h.y), dz(h.w), dz(h.h));
    let title = match d.platform {
        Platform::Pc => "Install",
        Platform::Quest => "Echo VR on Quest",
    };
    kit.header_strip(dz(h.x), dz(h.y), dz(h.w), dz(HEADER_H), title);
    let hr = match d.platform {
        Platform::Pc => pc_hero(d),
        Platform::Quest => quest_hero(d),
    };
    // Smaller when the hero is narrower than the design's (the rail unfolded).
    let lw = LOGO_W.min(HERO.w + kit.dx() - 120.0);
    let s = lw / LOGO_W;
    let logo = Dr::new(
        LOGO.x + (LOGO_W - lw) / 2.0 + kit.dx() / 2.0,
        LOGO.y + LOGO.h * (1.0 - s) / 2.0,
        lw,
        LOGO.h * s,
    );
    kit.image_d(hr.logo, logo);
    hero_text(kit, &hr);
    // One width for every label here, so the button never changes size on this page.
    let extra = hero::extra_for(kit, "INSTALL");
    kit.offset(DX, ROW_DY, |k| {
        hero::info_line(d, k, "hero-path", &hr.line);
        match &hr.job {
            Some(job) => {
                if hero::job_row(k, "hero", extra, job) {
                    d.cancel_job(&job.id);
                }
            }
            None => {
                let row = Row {
                    face: hr.face,
                    grey: hr.grey,
                    enabled: hr.enabled,
                    tip: &hr.tip,
                    side: hr.side,
                    side_enabled: hr.side_enabled,
                    side_tip: &hr.side_tip,
                };
                let (main, side) = hero::row(k, "hero", extra, &row);
                if main {
                    do_main(d, ctx, hr.main);
                } else if side {
                    do_side(d, ctx, hr.side_act);
                }
            }
        }
        if let Some(r) = hero::switch(k, "hero-switch", extra, &mut d.platform) {
            d.quest_soon = Some((r, std::time::Instant::now()));
        }
    });

    match d.platform {
        Platform::Pc => {
            version_list(d, kit, ctx);
            install_panel::pc(d, kit, ctx);
        }
        Platform::Quest => {
            how_it_works(kit, VERSIONS.wider(kit.dx()).taller(kit.dy()));
            install_panel::quest(d, kit, ctx);
        }
    }
}

// ---- the hero ----

/// What the green button does.
enum Main {
    /// Install, or reinstall (check and repair) an installed version.
    Install(Box<VersionEntry>),
    QuestConnect,
    QuestInstall,
    QuestPlay,
    Nothing,
}

/// What the blue button does.
#[derive(Clone)]
enum SideAct {
    ChangeFolder,
    UpdatePc(Box<InstalledVersion>),
    Help(&'static str),
    QuestLink,
    QuestCheck,
    QuestUpdate,
    Nothing,
}

/// Everything the hero shows.
struct Hero {
    logo: &'static str,
    /// Its state (Installed, Files missing…), after the name.
    chips: Vec<(&'static str, Color32)>,
    name: String,
    /// Which main-server build it is, if it is one.
    tag: Option<Hosted>,
    notes: String,
    line: InfoLine,
    face: Face<'static>,
    grey: bool,
    enabled: bool,
    tip: String,
    main: Main,
    side: Side<'static>,
    side_enabled: bool,
    side_tip: String,
    side_act: SideAct,
    job: Option<JobView>,
}

impl Hero {
    fn new() -> Hero {
        Hero {
            logo: hero_logo(None),
            chips: Vec::new(),
            name: String::new(),
            tag: None,
            notes: String::new(),
            line: InfoLine::new(HERO.right() - 32.0 - DX),
            face: Face::Label("INSTALL"),
            grey: false,
            enabled: false,
            tip: String::new(),
            main: Main::Nothing,
            side: Side::Updates { alert: false },
            side_enabled: false,
            side_tip: String::new(),
            side_act: SideAct::Nothing,
            job: None,
        }
    }

    /// A job for this hero: in the buttons, and its step on the info line.
    fn show_job(&mut self, job: Option<JobView>) {
        let Some(job) = job else {
            return;
        };
        match self.line.parts.first_mut() {
            Some(first) => *first = play::job_state(&job),
            None => self.line.parts.push(play::job_state(&job)),
        }
        self.line.parts.truncate(1);
        self.line.parts.push(job.step());
        self.line.problem = None;
        self.line.color = design::GREY;
        self.job = Some(job);
    }
}

/// The version recommended to everyone: the first stable PC version in the catalogue.
fn recommended(d: &Dashboard) -> Option<VersionEntry> {
    let c = d.catalog.as_ref()?;
    c.pc()
        .find(|e| e.channel == "stable")
        .or_else(|| c.pc().next())
        .cloned()
}

/// Every PC version to choose from: first the main servers' (live, then events), then
/// the others, each in catalogue order.
fn listed(d: &Dashboard) -> Vec<VersionEntry> {
    let Some(c) = &d.catalog else {
        return Vec::new();
    };
    let (mut hosted, others): (Vec<_>, Vec<_>) = c.pc().cloned().partition(VersionEntry::is_hosted);
    hosted.sort_by_key(|e| e.hosted != Some(Hosted::Live));
    hosted.extend(others);
    hosted
}

/// The logo over the hero. One for every build for now; variants (say, Echo Combat's) go
/// here.
fn hero_logo(_e: Option<&VersionEntry>) -> &'static str {
    "logo_echovr.png"
}

/// A version's name under its logo: without the "Echo VR" the logo already shows.
fn hero_name(name: &str, logo: &str) -> String {
    match name.get(..8) {
        Some(p) if logo == "logo_echovr.png" && p.eq_ignore_ascii_case("echo vr ") => {
            name[8..].trim().to_string()
        }
        _ => name.to_string(),
    }
}

/// A main-server build's tag: its words and colour.
fn tag_look(tag: Hosted) -> Option<(&'static str, Color32)> {
    match tag {
        Hosted::Live => Some(("Live build", LIVE)),
        Hosted::Event => Some(("Event build", EVENT)),
        Hosted::Unknown => None,
    }
}

/// The version the hero shows: the row chosen in VERSIONS, else the recommended one.
fn chosen(d: &Dashboard) -> Option<VersionEntry> {
    d.install_pick
        .as_ref()
        .and_then(|id| listed(d).into_iter().find(|e| &e.id == id))
        .or_else(|| recommended(d))
}

fn busy_text(d: &Dashboard) -> Option<String> {
    d.jobs
        .values()
        .next()
        .map(|j| format!("Busy: {}. Wait until it's done.", j.title))
}

fn pc_hero(d: &mut Dashboard) -> Hero {
    // Look for copies already on this PC now, so the Install card can offer one at once.
    let _ = d.found_installs();
    let mut h = Hero::new();
    let busy = busy_text(d);
    h.side = Side::Blue {
        icon: Icon::Folder,
        label: "Change folder",
    };
    h.side_act = SideAct::ChangeFolder;
    h.side_enabled = busy.is_none();
    h.side_tip = busy
        .clone()
        .unwrap_or_else(|| "Install into another folder".into());
    let Some(e) = chosen(d) else {
        h.line.parts.push("Loading the version list".into());
        (h.grey, h.tip) = (true, "The version list is still loading".into());
        return h;
    };
    h.logo = hero_logo(Some(&e));
    h.name = hero_name(&e.name, h.logo);
    h.tag = e.hosted;
    h.notes = e.notes.clone();
    let offered = d.state.offers(&e);
    if !offered {
        h.chips.push(("Coming soon", design::QUEST_OFF));
    }
    let installed = d.state.installed_from(&e.id).cloned();
    let job = setup::job_for(d, installed.as_ref().map_or(&e.id, |v| &v.id));
    let present = |v: &InstalledVersion| d.demo || v.present();
    match installed {
        Some(v) if present(&v) => {
            h.chips.push(("Installed", design::QUEST_ON));
            h.line.parts.push("Installed".into());
            h.line.path = Some(v.root.clone());
            h.line.path_click = PathClick::Open;
            // Installed: REINSTALL checks it (the card asks first, as for an install).
            let in_use = d.files_in_use(&v);
            h.face = Face::Label("REINSTALL");
            h.main = Main::Install(Box::new(e.clone()));
            h.enabled = in_use.is_none() && busy.is_none();
            h.grey = !h.enabled;
            h.tip = match (&busy, in_use) {
                (Some(b), _) => b.clone(),
                (None, Some(why)) => why.into(),
                (None, None) => "Check every game file against the server's checksums and fetch only the broken ones again".into(),
            };
            h.side = Side::Updates {
                alert: d
                    .update_note
                    .get(&v.id)
                    .is_some_and(|n| n.contains("failed")),
            };
            let updates = crate::core::launcher::versions::has_updates(&v);
            h.side_enabled = updates && in_use.is_none() && busy.is_none();
            h.side_tip = match (&busy, in_use, updates) {
                (_, _, false) => {
                    "Event builds don't get updates: REINSTALL checks their files".into()
                }
                (Some(b), _, _) => b.clone(),
                (None, Some(why), _) => why.into(),
                (None, None, _) => "Download any changed game files".into(),
            };
            h.side_act = SideAct::UpdatePc(Box::new(v));
        }
        gone => {
            let missing = gone.is_some();
            let external = gone.as_ref().is_some_and(|v| v.external);
            if missing {
                h.chips.push(("Files missing", design::RED));
                h.line.parts.push("Game files missing".into());
                h.line.color = design::DANGER;
            }
            let free = d.free_bytes();
            let version = e.version_line();
            h.line
                .parts
                .extend((!version.is_empty()).then_some(version));
            h.line
                .parts
                .extend(free.map(|f| format!("{} free", play::gb(f))));
            h.line
                .parts
                .extend(e.size.map(|s| format!("{} download", play::gb(s))));
            h.line.path = Some(crate::core::launcher::versions::root_for(
                &d.state.library,
                &e.id,
            ));
            h.line.path_click = PathClick::ChooseLibrary;
            let short = matches!((e.size, free), (Some(need), Some(free)) if need > free);
            h.tip = if missing {
                "Download this version again".into()
            } else {
                "Download and install Echo VR into the folder shown above".into()
            };
            if short {
                h.line.problem("Not enough space");
                h.tip = "Not enough space here: change the folder".into();
            }
            if external {
                h.line
                    .problem("Its folder is gone: forget it in your library");
            }
            if let Some(b) = &busy {
                h.tip = b.clone();
            }
            if !offered {
                h.tip = if e.downloadable() {
                    "Event builds are coming soon".into()
                } else {
                    "This build isn't on the download servers yet".into()
                };
            }
            h.enabled = offered && !short && !external && busy.is_none();
            h.grey = !h.enabled;
            if missing {
                h.face = Face::Label("REINSTALL");
            }
            h.main = Main::Install(Box::new(e));
        }
    }
    if job.as_ref().is_some_and(JobView::installs) {
        h.chips = vec![("Installing", design::BLUE)];
    }
    h.show_job(job);
    h
}

fn quest_hero(d: &mut Dashboard) -> Hero {
    let mut h = Hero::new();
    let status = d.quest_conn.status;
    let ready = status == Some(Status::Ready);
    let installed = ready && d.quest_info.as_ref().is_some_and(|i| i.installed);
    let known = ready && d.quest_info.is_some();
    let busy = d.quest_conn.checking || d.quest_busy || d.any_job();
    h.chips.push(match (ready, installed) {
        (true, true) => ("Installed", design::QUEST_ON),
        (true, false) => ("Not installed", design::QUEST_OFF),
        _ => d.quest_chip(),
    });
    h.name = hero_name("Echo VR (Quest)", h.logo);
    h.notes =
        "Installed over USB. The headset holds one version; updates only copy what changed.".into();
    h.line.parts = play::quest_info(d);
    let busy_tip = busy_text(d).unwrap_or_else(|| "Wait a moment".into());
    if !ready {
        (h.face, h.main) = (Face::Label("CONNECT"), Main::QuestConnect);
        h.enabled = !d.quest_conn.checking && !d.quest_busy;
        h.tip = "Look for your Quest over USB".into();
        let url = if status == Some(Status::Unauthorized) {
            USB_DEBUGGING_URL
        } else {
            DEV_MODE_URL
        };
        h.side = Side::Blue {
            icon: Icon::Info,
            label: "How to connect",
        };
        (h.side_act, h.side_enabled) = (SideAct::Help(url), true);
        h.side_tip = "How to turn on developer mode and USB debugging".into();
    } else if !installed {
        h.main = Main::QuestInstall;
        h.enabled = known && !busy;
        h.grey = !h.enabled;
        h.tip = if busy {
            busy_tip.clone()
        } else {
            "Download Echo VR and install it on your Quest".into()
        };
        // New players may have a patched build's link already.
        if d.state.owner == Some(false) {
            h.side = Side::Blue {
                icon: Icon::Download,
                label: "From a link",
            };
            (h.side_act, h.side_enabled) = (SideAct::QuestLink, !busy);
            h.side_tip = if busy {
                busy_tip
            } else {
                "Install a patched build from a link you already have".into()
            };
        } else {
            h.side = Side::Blue {
                icon: Icon::Refresh,
                label: "Check again",
            };
            (h.side_act, h.side_enabled) = (SideAct::QuestCheck, !busy);
            h.side_tip = if busy {
                busy_tip
            } else {
                "Look at the headset again".into()
            };
        }
    } else {
        (h.face, h.main, h.enabled) = (Face::Label("PLAY"), Main::QuestPlay, true);
        h.tip = "Go to Play".into();
        (h.side_act, h.side_enabled) = (SideAct::QuestUpdate, !busy);
        h.side_tip = if busy {
            busy_tip
        } else {
            "Copy the latest game files to your Quest".into()
        };
    }
    h.show_job(hero::job_view(d, setup::QUEST_JOB));
    h
}

fn do_main(d: &mut Dashboard, ctx: &egui::Context, main: Main) {
    match main {
        Main::Install(e) => {
            setup::ask_install(d, *e);
        }
        Main::QuestConnect => d.check_quest(ctx, true),
        Main::QuestInstall => setup::ask_quest_install(d, false),
        Main::QuestPlay => d.page = Page::Play,
        Main::Nothing => {}
    }
}

fn do_side(d: &mut Dashboard, ctx: &egui::Context, act: SideAct) {
    match act {
        SideAct::ChangeFolder => hero::choose_library(d),
        SideAct::UpdatePc(v) => versions::update(d, ctx, *v),
        SideAct::Help(url) => platform::open_url(url),
        SideAct::QuestLink => setup::ask_quest_install(d, true),
        SideAct::QuestCheck => d.check_quest(ctx, true),
        SideAct::QuestUpdate => setup::quest_update(d, ctx),
        SideAct::Nothing => {}
    }
}

/// Select an installed version and go to Play.
pub(super) fn play_version(d: &mut Dashboard, id: &str) {
    d.state.selected = Some(id.to_string());
    d.save();
    d.page = Page::Play;
    d.platform = Platform::Pc;
}

/// The version's name in Conthrax, its tags (LIVE BUILD, then its state) and its notes.
fn hero_text(kit: &mut Kit, h: &Hero) {
    let x = dz(TEXT_X);
    let gap = dz(14.0);
    let tag = h.tag.and_then(tag_look);
    let tags_w = tag.map_or(0.0, |(label, _)| kit.dot_tag_width(label, TAG_SIZE) + gap)
        + h.chips
            .iter()
            .map(|(c, _)| kit.chip_width(c) + dz(8.0))
            .sum::<f32>();
    let name = kit.spaced_fit(
        &h.name.to_uppercase(),
        design::conthrax(NAME_SIZE),
        design::TEXT,
        dz(NAME_SIZE * 0.1),
        false,
        (dz(NOTES_W + kit.dx()) - tags_w).max(dz(120.0)),
    );
    let r = kit.put(x, dz(NAME_Y), name);
    let cy = r.center().y - kit.origin.y;
    let mut tx = if h.name.is_empty() {
        x
    } else {
        r.max.x - kit.origin.x + gap
    };
    if let Some((label, color)) = tag {
        tx += kit.dot_tag(tx, cy, label, TAG_SIZE, color) + gap;
    }
    for (text, color) in &h.chips {
        tx += kit.chip(tx, cy - dz(13.5), text, *color) + dz(8.0);
    }
    let notes_w = dz(NOTES_W + kit.dx());
    if let Some(g) = kit
        .caps_block(&h.notes, 17.0, design::BODY, notes_w)
        .into_iter()
        .next()
    {
        kit.clipped(x, dz(NOTES_Y), notes_w, dz(48.0), |k| {
            k.put(x, dz(NOTES_Y), g);
        });
    }
}

// ---- the cards ----

/// VERSIONS: every PC version, one row each, the main servers' builds above a line, and
/// where the list came from with Refresh on the title row. Clicking a row shows it in the
/// hero above; the chosen row is marked blue.
fn version_list(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let r = VERSIONS.wider(kit.dx()).taller(kit.dy());
    let (x, y, w, bottom) = hero::card_frame(kit, r, "Versions");
    install_panel::list_source(d, kit, ctx, r);
    let entries = listed(d);
    if entries.is_empty() {
        kit.caps_text(
            x,
            y,
            w,
            "Loading the version list…",
            15.8,
            design::TEXT,
            dz(18.0),
        );
        return;
    }
    let chosen = chosen(d).map(|e| e.id);
    let recommended = recommended(d).map(|e| e.id);
    let (row_h, pad) = (dz(ROW_H), dz(ROW_PAD));
    let hosted = entries.iter().take_while(|e| e.is_hosted()).count();
    let divider = if hosted > 0 && hosted < entries.len() {
        dz(DIVIDER_H)
    } else {
        0.0
    };
    let list_h = bottom - y;
    kit.scroll_area(
        "versions",
        x,
        y,
        w + pad + dz(4.0),
        list_h,
        entries.len() as f32 * row_h + divider,
        &mut d.versions_scroll,
    );
    let scroll = d.versions_scroll;
    kit.clipped(x - pad, y, w + 2.0 * pad, list_h, |k| {
        if divider > 0.0 {
            let ly = k.origin.y + y - scroll + hosted as f32 * row_h + (divider - dz(8.0)) / 2.0;
            let lx = k.origin.x + x;
            k.ui.painter().hline(
                lx + dz(24.0)..=lx + w - dz(24.0),
                ly.round(),
                egui::Stroke::new(dz(1.5), DIVIDER),
            );
        }
        for (i, e) in entries.iter().enumerate() {
            let below = if i >= hosted { divider } else { 0.0 };
            let ry = y - scroll + i as f32 * row_h + below;
            let r = k.rect(x - pad, ry, w + 2.0 * pad - dz(8.0), row_h - dz(8.0));
            let is_chosen = chosen.as_deref() == Some(e.id.as_str());
            let tip = if is_chosen {
                "Shown above"
            } else {
                "Show this version above"
            };
            let (resp, t, _) = k.hot(&format!("ver-{}", e.id), r, !is_chosen, tip);
            let p = k.ui.painter();
            if is_chosen {
                p.rect_filled(r, dz(6.0), design::BLUE.gamma_multiply(0.28));
                let bar = egui::Rect::from_min_size(r.min, egui::vec2(dz(4.0), r.height()));
                p.rect_filled(bar, dz(2.0), design::BLUE);
            } else if t > 0.01 {
                p.rect_filled(r, dz(6.0), Color32::from_white_alpha((14.0 * t) as u8));
            }
            if resp.clicked {
                d.install_pick = Some(e.id.clone());
            }

            // The chip on the right: what this version is to you.
            let (chip, color) = match hero::job_view(d, &e.id) {
                Some(job) if job.installs() => (
                    job.fraction.map_or_else(
                        || "Installing".to_string(),
                        |f| format!("{:.0}%", f * 100.0),
                    ),
                    design::BLUE,
                ),
                _ if !d.state.offers(e) => ("Coming soon".into(), design::QUEST_OFF),
                _ if d.state.installed_from(&e.id).is_some() => {
                    ("Installed".into(), design::QUEST_ON)
                }
                _ if recommended.as_deref() == Some(e.id.as_str()) => {
                    ("Recommended".into(), design::QUEST_ON)
                }
                _ => (String::new(), design::QUEST_OFF),
            };
            let chip_x = if chip.is_empty() {
                x + w
            } else {
                x + w - dz(CHIP_INSET) - k.chip_width(&chip)
            };
            let cy = ry + (row_h - dz(8.0)) / 2.0;
            if !chip.is_empty() {
                k.chip(chip_x, cy - dz(13.5), &chip, color);
            }

            // Left: the name and its tag over size and channel. Middle: the summary, up
            // to the chip.
            let tx = x + dz(6.0);
            let room = chip_x - dz(24.0) - tx;
            let tag = e.hosted.and_then(tag_look);
            let tag_w = tag.map_or(0.0, |(l, _)| k.dot_tag_width(l, 13.0) + dz(14.0));
            let name = k.label_galley(&e.name, design::din(18.0), design::TEXT, room - tag_w);
            let nr = k.put(tx, ry + dz(7.0), name);
            if let Some((label, color)) = tag {
                let nx = nr.max.x - k.origin.x + dz(14.0);
                k.dot_tag(nx, nr.center().y - k.origin.y, label, 13.0, color);
            }
            // Under the name: its size, version and date, and channel.
            let sub = [
                e.size.map(play::gb),
                Some(e.version_line()),
                Some(e.channel.clone()),
            ]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("  ·  ");
            let g = k.label_galley(&sub, design::din(15.0), design::GREY, room);
            k.put(tx, ry + dz(32.0), g);
            let left_end = (nr.max.x - k.origin.x + tag_w).max(tx + dz(200.0));
            let summary_w = chip_x - dz(28.0) - left_end - dz(24.0);
            if !e.summary.is_empty() && summary_w > dz(60.0) {
                let g = k.label_galley(&e.summary, design::din(14.0), SUMMARY, summary_w);
                let gx = chip_x - dz(28.0) - g.size().x;
                k.put(gx, cy - g.size().y / 2.0, g);
            }
        }
    });
}

/// The Quest side's steps, in `r`.
fn how_it_works(kit: &mut Kit, r: Dr) {
    let (x, y, w, bottom) = hero::card_frame(kit, r, "How it works");
    kit.caps_text(
        x,
        y,
        w,
        "1 · Turn on developer mode for your Quest in the Meta Horizon app.\n\n\
         2 · Plug the Quest into this PC by USB and allow USB debugging in the headset.\n\n\
         3 · Press INSTALL. Echo VR downloads and goes straight onto the headset.",
        15.8,
        design::TEXT,
        dz(18.0),
    );
    let ly = bottom - dz(22.0);
    if kit
        .link(
            "dev-mode-help",
            x,
            ly,
            "Developer mode help",
            16.5,
            DEV_MODE_URL,
        )
        .clicked
    {
        platform::open_url(DEV_MODE_URL);
    }
}

// ---- the right panel ----

/// Myriad text (paths keep their case), wrapped at `w` or cut with "…" on one line.
pub(super) fn myriad(
    kit: &Kit,
    text: &str,
    font: egui::FontId,
    color: Color32,
    w: f32,
    one_line: bool,
) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    design::append_text(
        &mut job,
        text,
        egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = if one_line {
        TextWrapping::truncate_at_width(w)
    } else {
        TextWrapping {
            max_width: w,
            ..Default::default()
        }
    };
    kit.ui.ctx().fonts_mut(|f| f.layout_job(job))
}

/// A path in Myriad on one line, `w` wide at most: too long, it loses its middle, so its
/// start and its folder's name stay.
pub(super) fn myriad_path(
    kit: &Kit,
    path: &str,
    font: egui::FontId,
    color: Color32,
    w: f32,
) -> Arc<Galley> {
    let chars: Vec<char> = path.chars().collect();
    let mut keep = chars.len();
    let mut g = myriad(kit, path, font.clone(), color, f32::INFINITY, true);
    while g.size().x > w && keep > 12 {
        keep -= 2;
        let head = keep / 3;
        let short: String = chars[..head]
            .iter()
            .chain(std::iter::once(&'…'))
            .chain(&chars[chars.len() - (keep - head)..])
            .collect();
        g = myriad(kit, &short, font.clone(), color, f32::INFINITY, true);
    }
    if g.size().x > w {
        g = myriad(kit, path, font, color, w, true);
    }
    g
}

/// Clickable Myriad text in the link colour, underlined on hover; returns the click.
#[allow(clippy::too_many_arguments)]
pub(super) fn text_link(
    kit: &mut Kit,
    key: &str,
    x: f32,
    y: f32,
    text: &str,
    size: f32,
    enabled: bool,
    tip: &str,
) -> bool {
    let g = myriad(
        kit,
        text,
        design::myriad(size),
        design::LINK,
        f32::INFINITY,
        true,
    );
    let r = kit.rect(x, y, g.size().x, g.size().y);
    let (resp, t, _) = kit.hot(key, r, enabled, tip);
    let color = if enabled {
        design::LINK
    } else {
        design::SUBTLE
    };
    kit.ui
        .painter()
        .galley_with_override_text_color(r.min, g, color);
    if t > 0.01 {
        kit.ui.painter().hline(
            r.x_range(),
            r.max.y - 1.0,
            egui::Stroke::new(1.0, color.gamma_multiply(t)),
        );
    }
    resp.clicked
}
