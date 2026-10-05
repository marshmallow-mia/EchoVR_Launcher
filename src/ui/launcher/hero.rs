//! The concept's top block, shared by the Play page and the Install page's hero: the
//! ECHO VR logo, the info line, the green PLAY-shaped button, the blue CHECK FOR
//! UPDATES-shaped one and the PCVR|QUEST switch, plus the news cards' frame. Coordinates
//! are the Play page's (design pixels); the Install page draws the same parts further
//! down and right with `Kit::offset`.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{pos2, Color32, Galley, Rect};

use super::{settings, Dashboard, JobKind};
use crate::core::launcher::catalog::Platform;
use crate::core::platform;
use crate::ui::design::{self, dz, Dr};
use crate::ui::kit::Kit;
use crate::ui::style::{self, Icon};

// Geometry in design pixels. The image rects put each button's shape where the concept
// has it: play_button_blank.png's shape sits at (61, 61) in its 821×380 glow,
// update_button.png and blue_button.png are just their shape, and the hardware switch's body starts 74 px under its labels.
/// Vertical centre of the info line, its left end and its size.
pub(super) const INFO_Y: f32 = 217.0;
pub(super) const INFO_X: f32 = 144.0;
const INFO_SIZE: f32 = 15.2;
const PLAY_S: f32 = 0.2597;
const PLAY_IMG: Dr = Dr::new(123.2, 227.2, 821.0 * PLAY_S, 380.0 * PLAY_S);
const PLAY_SHAPE: [(f32, f32); 4] = [
    (139.0, 243.0),
    (269.4, 243.0),
    (332.0, 309.8),
    (139.0, 309.8),
];
/// Where PLAY reacts: its box, cut where CHECK FOR UPDATES' slant begins.
const PLAY_AREA: Dr = Dr::new(139.0, 243.0, 186.0, 67.0);
const PLAY_COMPACT_MAIN: Dr = Dr::new(139.0, 88.0, 160.999, 66.8);
const PLAY_COMPACT_ARROW: Dr = Dr::new(300.001, 88.0, 144.998, 66.8);
const PLAY_COMPACT_MAIN_FULL: Dr = Dr::new(139.0, 88.0, 305.999, 66.8);
const PLAY_COMPACT_UPDATE: Dr = Dr::new(445.001, 88.0, 337.999, 66.0);
// The compact Play asset is shifted into the version row using the approved split-control geometry.
const COMPACT_PLAY_EXTRA_X: f32 = 120.0;
const PLAY_SHAPE_SHIFT_START_X: f32 = 200.0;
const COMPACT_PLAY_Y_OFFSET: f32 = -155.0;

fn compact_play_regions(has_arrow: bool) -> (Dr, Option<Dr>, Dr) {
    (
        if has_arrow {
            PLAY_COMPACT_MAIN
        } else {
            PLAY_COMPACT_MAIN_FULL
        },
        has_arrow.then_some(PLAY_COMPACT_ARROW),
        PLAY_COMPACT_UPDATE,
    )
}

/// Active polygon used by the compact Play main/arrow controls.
pub(super) fn compact_play_button_shape() -> [(f32, f32); 4] {
    design::shifted(PLAY_SHAPE, COMPACT_PLAY_EXTRA_X, PLAY_SHAPE_SHIFT_START_X)
        .map(|(x, y)| (x, y + COMPACT_PLAY_Y_OFFSET))
}

/// Arrow hit region in the compact Play split control.
pub(super) fn compact_play_arrow_region() -> Dr {
    PLAY_COMPACT_ARROW
}

/// Progress is clipped to the active Play main target: the PC main segment when the
/// arrow is present, and the full green button on Quest or with no PC choices.
fn compact_job_progress_body(has_arrow: bool) -> Dr {
    compact_play_regions(has_arrow).0
}

// Shared by the wide Play job row and compact split-button row so their indeterminate
// highlights keep the same design width and sweep timing.
// The band spans 35% of the button and advances 0.6 full sweeps per second.
const INDETERMINATE_BAND_WIDTH_FRACTION: f32 = 0.35;
const INDETERMINATE_CYCLES_PER_SECOND: f32 = 0.6;

fn job_progress_clip(body: Rect, fraction: Option<f32>, time: f32) -> Rect {
    if let Some(fraction) = fraction {
        return Rect::from_min_max(
            body.min,
            pos2(
                body.min.x + body.width() * fraction.clamp(0.0, 1.0),
                body.max.y,
            ),
        );
    }
    let band = body.width() * INDETERMINATE_BAND_WIDTH_FRACTION;
    let x = body.min.x - band
        + (time * INDETERMINATE_CYCLES_PER_SECOND).fract() * (body.width() + band);
    Rect::from_min_max(
        pos2(x.max(body.min.x), body.min.y),
        pos2((x + band).min(body.max.x), body.max.y),
    )
}
/// PLAY's native size, and the rounded left end and slanted right end (native pixels)
/// that keep their shape when the green in between is stretched.
const PLAY_NATIVE: (f32, f32) = (821.0, 380.0);
/// The green button's images, every state.
const PLAY_STATES: [&str; 4] = [
    "play_button_blank.png",
    "play_button_blank_hover.png",
    "play_button_blank_grey.png",
    "play_button_blank_grey_hover.png",
];
const PLAY_CAPS: (f32, f32) = (300.0, 521.0);
/// Where the concept's "PLAY" lettering is centred (design pixels): every label is.
const LABEL_CX: f32 = 212.5;
const LABEL_CY: f32 = 278.6;
/// The width of the concept's "PLAY", and the room a label gets at the concept's size.
const PLAY_WORD_W: f32 = 102.0;
const LABEL_W: f32 = 122.0;
/// Conthrax at the width of the concept's "PLAY" has 32 px tall letters; the concept's are
/// 22.
const PLAY_CAP_SCALE: f32 = 22.0 / 32.0;
const UPDATE_IMG: Dr = Dr::new(320.1, 243.0, 342.9, 66.0);
const UPDATE_SHAPE: [(f32, f32); 4] = [
    (320.1, 243.0),
    (663.0, 243.0),
    (663.0, 309.0),
    (383.2, 309.0),
];
const UPDATE_AREA: Dr = Dr::new(325.0, 243.0, 338.0, 66.0);
/// The centre of update_button.png's icon and label, for the blue button's own labels.
/// The blue button's images, every state.
const BLUE_STATES: [&str; 4] = [
    "blue_button.png",
    "blue_button_hover.png",
    "blue_button_grey.png",
    "blue_button_grey_hover.png",
];
const UPDATE_LABEL: (f32, f32) = (500.4, 276.8);
const SWITCH_IMG: Dr = Dr::new(697.0, 248.4, 147.0, 60.1);
const SWITCH_PC: Dr = Dr::new(697.0, 265.0, 73.5, 43.0);
const SWITCH_QUEST: Dr = Dr::new(770.5, 265.0, 73.5, 43.0);
/// RUNNING's lettering in play_button_running.png (native pixels, with its shadow).
const RUNNING_WORD: Rect = Rect {
    min: pos2(80.0, 150.0),
    max: pos2(625.0, 245.0),
};
/// What the green button shows.
pub(super) enum Face<'a> {
    /// A label in Conthrax where the concept has its "PLAY".
    Label(&'a str),
    /// The concept's grey RUNNING.
    Running,
}

/// What the blue button is.
pub(super) enum Side<'a> {
    /// CHECK FOR UPDATES (the image), with the orange "!" after a failed update.
    Updates { alert: bool },
    /// The blank blue button with an icon and a label.
    Blue { icon: Icon, label: &'a str },
}

/// The two buttons. `grey` draws the green one in greys even when it can be clicked
/// (nothing to play yet, or STOP while the game runs); off, it is grey anyway.
pub(super) struct Row<'a> {
    pub face: Face<'a>,
    pub grey: bool,
    pub enabled: bool,
    pub tip: &'a str,
    pub side: Side<'a>,
    pub side_enabled: bool,
    pub side_tip: &'a str,
}

/// A job running for what the row is about.
#[derive(Clone)]
pub(super) struct JobView {
    pub id: String,
    pub kind: JobKind,
    pub title: String,
    pub label: String,
    pub fraction: Option<f32>,
    pub cancelling: bool,
}

pub(super) fn job_view(d: &Dashboard, id: &str) -> Option<JobView> {
    let j = d.jobs.get(id)?;
    Some(JobView {
        id: id.to_string(),
        kind: j.kind,
        title: j.title.clone(),
        label: j.label.clone(),
        fraction: j.fraction,
        cancelling: j.cancel.load(std::sync::atomic::Ordering::Relaxed),
    })
}

impl JobView {
    /// An install (on this PC or the Quest), as opposed to work on something installed.
    pub fn installs(&self) -> bool {
        matches!(
            self.kind,
            JobKind::Install | JobKind::Reinstall | JobKind::QuestInstall
        )
    }

    /// The job's progress line, "…" instead of "...".
    pub fn step(&self) -> String {
        self.label.replace("...", "…")
    }
}

// ---- the info line ----

/// What clicking the path on the info line does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PathClick {
    /// Open the folder.
    Open,
    /// Pick another library folder before installing.
    ChooseLibrary,
    Nothing,
}

/// The line under the logo: the state and details, a folder, and a problem.
pub(super) struct InfoLine {
    /// The selected target and its state stay on the first line, before optional details.
    pub primary: Vec<String>,
    pub primary_tip: Option<String>,
    pub parts: Vec<String>,
    pub path: Option<String>,
    pub path_click: PathClick,
    pub problem: Option<String>,
    pub color: Color32,
    /// Where the line has to end (design pixels, in the row's coordinates).
    pub right: f32,
}

impl InfoLine {
    pub fn new(right: f32) -> InfoLine {
        InfoLine {
            primary: Vec::new(),
            primary_tip: None,
            parts: Vec::new(),
            path: None,
            path_click: PathClick::Nothing,
            problem: None,
            color: design::GREY,
            right,
        }
    }

    /// A problem the player has to fix, in red at the end.
    pub fn problem(&mut self, text: impl Into<String>) {
        self.problem = Some(text.into());
        self.color = design::DANGER;
    }
}

/// The info line: the state and details in DMCAPS, the folder in full in Myriad
/// (clickable), and a problem in red. A line too long for its room is set smaller.
pub(super) fn info_line(d: &mut Dashboard, kit: &mut Kit, key: &str, line: &InfoLine) {
    if !line.primary.is_empty() {
        info_line_primary(d, kit, key, line);
        return;
    }
    let sep = "  ·  ";
    let color = line.color;
    let layout = |scale: f32| {
        let size = INFO_SIZE * scale;
        let mut before = LayoutJob::default();
        design::caps_append(&mut before, &line.parts.join(sep), size, color, false);
        let mut path = LayoutJob::default();
        let mut after = LayoutJob::default();
        if let Some(p) = &line.path {
            if !line.parts.is_empty() {
                design::caps_append(&mut before, sep, size, color, false);
            }
            design::caps_append(&mut path, p, size, color, true);
        }
        if let Some(problem) = &line.problem {
            design::caps_append(&mut after, sep, size, color, false);
            design::caps_append(&mut after, problem, size, design::DANGER, false);
        }
        [before, path, after].map(|job| kit.ui.ctx().fonts_mut(|f| f.layout_job(job)))
    };
    // The column is as much wider as the window is.
    let right = line.right + kit.dx();
    let max_w = dz(right - INFO_X);
    let width = |g: &[Arc<Galley>; 3]| g.iter().map(|g| g.size().x).sum::<f32>();
    let mut parts = layout(1.0);
    if width(&parts) > max_w {
        let scale = (max_w / width(&parts)).max(0.8);
        parts = layout(scale * 0.995);
    }
    let h = parts.iter().map(|g| g.size().y).fold(0.0, f32::max);
    let (x0, y) = (dz(INFO_X), dz(INFO_Y) - h / 2.0);
    let [before, path, after] = parts;
    let path_w = path.size().x;
    let path_x = x0 + before.size().x;
    kit.clipped(x0, y - 2.0, max_w, h + 4.0, |kit| {
        kit.put(x0, y, before);
        kit.put(path_x, y, path);
        kit.put(path_x + path_w, y, after);
    });
    let Some(p) = line.path.clone() else {
        return;
    };
    let r = kit.rect(path_x, y, path_w.min(dz(right) - path_x), h);
    let (tip, enabled) = match line.path_click {
        PathClick::Open => (format!("{p}\nClick to open this folder"), true),
        PathClick::ChooseLibrary => (
            format!("Echo VR will be installed into {p}\nClick to choose another folder"),
            !d.any_job(),
        ),
        PathClick::Nothing => (p.clone(), false),
    };
    let (resp, t, _) = kit.hot(key, r, enabled, &tip);
    if t > 0.01 {
        let c = color.gamma_multiply(t);
        kit.ui
            .painter()
            .hline(r.x_range(), r.max.y - 1.0, egui::Stroke::new(1.0, c));
    }
    if resp.clicked {
        match line.path_click {
            PathClick::Open => open_folder(d, &p),
            PathClick::ChooseLibrary => choose_library(d),
            PathClick::Nothing => {}
        }
    }
}

/// Draw the selected target in the first line and let optional details use the second.
fn info_line_primary(d: &mut Dashboard, kit: &mut Kit, key: &str, line: &InfoLine) {
    let sep = "  ·  ";
    let right = line.right + kit.dx();
    let max_w = dz(right - INFO_X);
    let color = line.color;
    let (primary_full, required_full) = primary_info_lines(&line.primary);
    let primary_text = fit_info_text(kit, &primary_full, max_w);
    let primary = kit.spaced_galley(&primary_text, design::din(INFO_SIZE), color, dz(0.9), false);

    // Optional details give up room before the target group. The path stays clickable
    // whenever any part of it remains visible, and its tooltip always carries the full path.
    let required = fit_info_text(kit, &required_full, max_w);
    let mut optional = line.parts.clone();
    let mut shown_path = line.path.clone();
    let mut shown_problem = line.problem.clone();
    let build_details =
        |required: &str, optional: &[String], path: Option<&str>, problem: Option<&str>| {
            let mut pieces = Vec::with_capacity(optional.len() + 3);
            if !required.is_empty() {
                pieces.push(required.to_string());
            }
            pieces.extend(optional.iter().cloned());
            if let Some(path) = path {
                pieces.push(path.to_string());
            }
            if let Some(problem) = problem {
                pieces.push(problem.to_string());
            }
            pieces.join(sep)
        };
    let mut details = build_details(
        &required,
        &optional,
        shown_path.as_deref(),
        shown_problem.as_deref(),
    );
    let measure = |text: &str, kit: &Kit<'_>| {
        kit.spaced_galley(text, design::din(INFO_SIZE), color, dz(0.9), false)
            .size()
            .x
    };
    while measure(&details, kit) > max_w && !optional.is_empty() {
        optional.pop();
        details = build_details(
            &required,
            &optional,
            shown_path.as_deref(),
            shown_problem.as_deref(),
        );
    }
    if measure(&details, kit) > max_w && shown_path.is_some() {
        let prefix = build_details(&required, &optional, None, None);
        let prefix_w = measure(&prefix, kit)
            + if prefix.is_empty() {
                0.0
            } else {
                measure(sep, kit)
            };
        let path_room = (max_w - prefix_w).max(1.0);
        shown_path = shown_path
            .as_deref()
            .map(|path| fit_info_text(kit, path, path_room));
        details = build_details(
            &required,
            &optional,
            shown_path.as_deref(),
            shown_problem.as_deref(),
        );
    }
    while measure(&details, kit) > max_w && shown_problem.is_some() {
        shown_problem = None;
        details = build_details(&required, &optional, shown_path.as_deref(), None);
    }
    let secondary = kit.spaced_galley(&details, design::din(INFO_SIZE), color, dz(0.9), false);
    let first_y = dz(184.0);
    let second_y = dz(201.0);
    let x = dz(INFO_X);
    kit.put(x, first_y, primary);
    let primary_rect = kit.rect(x, first_y, max_w, dz(INFO_SIZE + 4.0));
    if primary_text != primary_full {
        let name = line.primary_tip.as_deref().unwrap_or(&primary_full);
        let response = kit.ui.interact(
            primary_rect,
            egui::Id::new((key, "primary-tip")),
            egui::Sense::hover(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, false, name.to_string())
        });
        response.on_hover_text(name);
    }
    kit.put(x, second_y, secondary);
    let (Some(path), Some(shown_path)) = (line.path.clone(), shown_path) else {
        return;
    };
    let path_text = shown_path.as_str();
    let prefix = build_details(&required, &optional, None, None);
    let prefix = if prefix.is_empty() {
        prefix
    } else {
        format!("{prefix}{sep}")
    };
    let prefix_g = kit.spaced_galley(&prefix, design::din(INFO_SIZE), color, dz(0.9), false);
    let path_g = kit.spaced_galley(path_text, design::din(INFO_SIZE), color, dz(0.9), true);
    let px = x + prefix_g.size().x;
    let pr = Rect::from_min_size(pos2(px, second_y), path_g.size());
    let (tip, enabled) = match line.path_click {
        PathClick::Open => (format!("{path}\nClick to open this folder"), true),
        PathClick::ChooseLibrary => (
            format!("Echo VR will be installed into {path}\nClick to choose another folder"),
            !d.any_job(),
        ),
        PathClick::Nothing => (path.clone(), false),
    };
    let (resp, t, _) = kit.hot(key, pr, enabled, &tip);
    if t > 0.01 {
        kit.ui.painter().hline(
            pr.x_range(),
            pr.max.y - 1.0,
            egui::Stroke::new(1.0, color.gamma_multiply(t)),
        );
    }
    if resp.clicked && line.path_click == PathClick::Open {
        open_folder(d, &path);
    }
}

fn fit_info_text(kit: &Kit<'_>, text: &str, max_w: f32) -> String {
    fit_info_text_by(text, max_w, |s| {
        kit.spaced_galley(s, design::din(INFO_SIZE), design::TEXT, dz(0.9), false)
            .size()
            .x
    })
}

fn fit_info_text_by(text: &str, max_w: f32, measure: impl Fn(&str) -> f32) -> String {
    if measure(text) <= max_w {
        return text.to_string();
    }
    let mut prefix = text.to_string();
    while !prefix.is_empty() {
        prefix.pop();
        let candidate = format!("{}…", prefix.trim_end());
        if measure(&candidate) <= max_w {
            return candidate;
        }
    }
    String::new()
}

fn primary_info_lines(parts: &[String]) -> (String, String) {
    let name = parts.first().cloned().unwrap_or_default();
    let status = parts
        .iter()
        .skip(1)
        .cloned()
        .collect::<Vec<_>>()
        .join("  ·  ");
    (name, status)
}

pub(super) fn open_folder(d: &mut Dashboard, path: &str) {
    if let Err(e) = platform::open_folder(std::path::Path::new(path)) {
        d.dialogs.error(
            "Couldn't open folder",
            &format!("{e:#}"),
            Default::default(),
        );
    }
}

/// Asks for another folder to install new versions into.
pub(super) fn choose_library(d: &mut Dashboard) {
    if let Some(dir) = crate::ui::parts::choose_folder() {
        d.library_field = dir;
        settings::set_library(d);
    }
}

// ---- the buttons ----

/// How much wider (design pixels) than the concept the green button must be for `label`.
pub(super) fn extra_for(kit: &Kit, label: &str) -> f32 {
    let w = kit.text_width(label, play_font(kit)) / dz(1.0);
    (w - LABEL_W).max(0.0).ceil()
}

/// The green and the blue button (`extra` wider than the concept, everything right of
/// the green one moved along); returns which was clicked. Under the pointer they show
/// the concept's hover images, held down its "clicked" greys; off (or `grey`) they are
/// grey too.
pub(super) fn row(kit: &mut Kit, key: &str, extra: f32, r: &Row) -> (bool, bool) {
    let play_shape = design::shifted(PLAY_SHAPE, extra, 200.0);
    let update_shape = design::shifted(UPDATE_SHAPE, extra, 0.0);
    kit.a11y_name = Some(match r.face {
        Face::Label(label) => label.to_string(),
        Face::Running => "Running".into(),
    });
    let (resp, t, pressed) = kit.hot_shape(
        &format!("{key}-main"),
        PLAY_AREA.wider(extra),
        &play_shape,
        r.enabled,
        r.tip,
    );
    let off = r.grey || !r.enabled;
    // Held down: the concept's grey "clicked" PLAY.
    let grey = off || pressed;
    let lit_t = if grey { 0.0 } else { t };
    play_body(kit, extra, grey, t);
    match r.face {
        Face::Running => word(
            kit,
            "play_button_running.png",
            RUNNING_WORD,
            extra,
            Color32::WHITE,
        ),
        Face::Label(label) => play_label(kit, label, !grey, lit_t, extra),
    }
    kit.shape_veil(&play_shape, 0.0, off && pressed);
    let main = resp.clicked;

    kit.a11y_name = Some(match r.side {
        Side::Updates { .. } => "Check for updates".into(),
        Side::Blue { label, .. } => label.to_string(),
    });
    let (resp, t, pressed) = kit.hot_shape(
        &format!("{key}-side"),
        UPDATE_AREA.moved(extra),
        &update_shape,
        r.side_enabled,
        r.side_tip,
    );
    match r.side {
        Side::Updates { alert } => {
            // Held down it is the concept's "clicked + hover" grey.
            let name = match (alert, r.side_enabled && !pressed) {
                (false, true) => "update_button",
                (true, true) => "update_button_alert",
                (false, false) => "update_button_grey",
                (true, false) => "update_button_alert_grey",
            };
            let at = UPDATE_IMG.moved(extra);
            let base = if alert {
                "update_button_alert"
            } else {
                "update_button"
            };
            for state in ["", "_hover", "_grey", "_grey_hover"] {
                kit.prefetch_d(&format!("{base}{state}.png"), at);
            }
            kit.image_d(&format!("{name}.png"), at);
            if t > 0.01 {
                kit.image_tinted(&format!("{name}_hover.png"), at, design::fade(t));
            }
        }
        Side::Blue { icon, label } => {
            blue_face(kit, extra, r.side_enabled && !pressed, t, icon, label);
        }
    }
    (main, resp.clicked)
}

/// The Play page's compact row. Install deliberately keeps using `row` above.
pub(super) fn play_row(
    kit: &mut Kit,
    key: &str,
    r: &Row,
    arrow: bool,
    arrow_enabled: bool,
    arrow_tip: &str,
) -> (bool, bool, bool) {
    const EXTRA: f32 = COMPACT_PLAY_EXTRA_X;
    const UP: f32 = COMPACT_PLAY_Y_OFFSET;
    let green = compact_play_button_shape();
    let blue = design::shifted(UPDATE_SHAPE, COMPACT_PLAY_EXTRA_X, 0.0)
        .map(|(x, y)| (x, y + COMPACT_PLAY_Y_OFFSET));

    // The blue and green assets overlap by five design pixels. Paint and ownership use
    // the same seam so the two controls never both respond to one pointer position.
    let (main, arrow_clicked) = kit.clipped(dz(0.0), dz(88.0), dz(445.0), dz(67.0), |kit| {
        kit.a11y_name = Some(match r.face {
            Face::Label(label) => label.to_string(),
            Face::Running => "Running".into(),
        });
        let (main_area, arrow_area, update_area) = compact_play_regions(arrow);
        let _ = update_area;
        let (resp, t, pressed) =
            kit.hot_shape(&format!("{key}-main"), main_area, &green, r.enabled, r.tip);
        let off = r.grey || !r.enabled;
        play_body_at(kit, EXTRA, off || pressed, if off { 0.0 } else { t }, UP);
        let label = match r.face {
            Face::Running => "RUNNING",
            Face::Label(label) => label,
        };
        play_label_at(
            kit,
            label,
            !off,
            if off { 0.0 } else { t },
            219.5,
            121.4,
            130.0,
        );
        kit.shape_veil(&green, 0.0, off && pressed);
        let main = resp.clicked;

        let mut arrow_clicked = false;
        if arrow {
            kit.a11y_name = Some("Choose PC version".into());
            let (resp, t, pressed) = kit.hot_shape(
                &format!("{key}-version"),
                arrow_area.expect("arrow area exists when shown"),
                &green,
                arrow_enabled,
                arrow_tip,
            );
            arrow_clicked = resp.clicked;
            let color = if arrow_enabled {
                style::mix(Color32::WHITE, Color32::from_gray(180), t)
            } else {
                Color32::from_gray(145)
            };
            let x = dz(300.0);
            kit.ui.painter().vline(
                x,
                dz(96.0)..=dz(146.0),
                egui::Stroke::new(dz(1.5), Color32::from_white_alpha(110)),
            );
            if pressed {
                kit.ui.painter().rect_filled(
                    kit.rect(dz(301.0), dz(89.0), dz(143.0), dz(64.0)),
                    0.0,
                    Color32::from_black_alpha(22),
                );
            }
            style::icon_at(
                kit.ui.painter(),
                Icon::ChevronDown,
                pos2(dz(355.0 - 10.0), dz(121.0 - 10.0)),
                dz(20.0),
                color,
            );
        }
        (main, arrow_clicked)
    });
    let blue_row = kit.clipped(dz(445.0), dz(88.0), dz(338.0), dz(67.0), |kit| {
        kit.a11y_name = Some(match r.side {
            Side::Updates { .. } => "Check for updates".into(),
            Side::Blue { label, .. } => label.to_string(),
        });
        let (resp, t, pressed) = kit.hot_shape(
            &format!("{key}-side"),
            compact_play_regions(arrow).2,
            &blue,
            r.side_enabled,
            r.side_tip,
        );
        match r.side {
            Side::Updates { alert } => {
                let name = match (alert, r.side_enabled && !pressed) {
                    (false, true) => "update_button",
                    (true, true) => "update_button_alert",
                    (false, false) => "update_button_grey",
                    (true, false) => "update_button_alert_grey",
                };
                let at = UPDATE_IMG.moved(EXTRA).lower(UP);
                kit.image_d(&format!("{name}.png"), at);
                if t > 0.01 {
                    kit.image_tinted(&format!("{name}_hover.png"), at, design::fade(t));
                }
            }
            Side::Blue { icon, label } => {
                blue_face_at(kit, EXTRA, r.side_enabled && !pressed, t, icon, label, UP);
            }
        }
        resp.clicked
    });
    (main, blue_row, arrow_clicked)
}

/// Draw a Play job row with progress limited to the main segment and a disabled arrow.
pub(super) fn play_job_row(
    kit: &mut Kit,
    key: &str,
    job: &JobView,
    arrow: bool,
    arrow_tip: &str,
) -> (bool, bool) {
    const EXTRA: f32 = 120.0;
    const UP: f32 = -155.0;
    let green = design::shifted(PLAY_SHAPE, EXTRA, 200.0).map(|(x, y)| (x, y + UP));
    let blue = design::shifted(UPDATE_SHAPE, EXTRA, 0.0).map(|(x, y)| (x, y + UP));
    kit.clipped(dz(0.0), dz(88.0), dz(445.0), dz(67.0), |kit| {
        kit.a11y_name = Some(job.step());
        kit.hot_shape(
            &format!("{key}-main"),
            compact_play_regions(arrow).0,
            &green,
            false,
            &job.step(),
        );
        let body = kit.drect(compact_job_progress_body(arrow));
        play_body_at(kit, EXTRA, true, 0.0, UP);
        let time = kit.ui.input(|i| i.time) as f32;
        let clip = job_progress_clip(body, job.fraction, time);
        let saved = kit.ui.clip_rect();
        kit.ui.set_clip_rect(saved.intersect(clip));
        play_body_at(kit, EXTRA, false, 0.0, UP);
        kit.ui.set_clip_rect(saved);
        if let Some(f) = job.fraction {
            play_label_at(
                kit,
                &format!("{:.0}%", f * 100.0),
                true,
                0.0,
                219.5,
                121.4,
                130.0,
            );
        } else {
            kit.ui.ctx().request_repaint();
        }
        if arrow {
            kit.a11y_name = Some("Choose PC version".into());
            kit.hot_shape(
                &format!("{key}-version"),
                compact_play_regions(arrow)
                    .1
                    .expect("arrow area exists when shown"),
                &green,
                false,
                arrow_tip,
            );
            kit.ui.painter().vline(
                dz(300.0),
                dz(96.0)..=dz(146.0),
                egui::Stroke::new(dz(1.5), Color32::from_white_alpha(110)),
            );
            style::icon_at(
                kit.ui.painter(),
                Icon::ChevronDown,
                pos2(dz(345.0), dz(111.0)),
                dz(20.0),
                Color32::from_gray(145),
            );
        }
    });
    let cancel = kit.clipped(dz(445.0), dz(88.0), dz(338.0), dz(67.0), |kit| {
        kit.a11y_name = Some(
            if job.cancelling {
                "Stopping…"
            } else {
                "Cancel"
            }
            .into(),
        );
        let can = !job.cancelling;
        let tip = if can {
            format!("Stop {}", job.title.to_lowercase())
        } else {
            "Stopping…".into()
        };
        let (resp, t, pressed) = kit.hot_shape(
            &format!("{key}-side"),
            compact_play_regions(arrow).2,
            &blue,
            can,
            &tip,
        );
        blue_face_at(
            kit,
            EXTRA,
            can && !pressed,
            t,
            Icon::Close,
            if can { "Cancel" } else { "Stopping…" },
            UP,
        );
        resp.clicked
    });
    (cancel, false)
}

fn play_body_at(kit: &mut Kit, extra: f32, grey: bool, t: f32, dy: f32) {
    let at = PLAY_IMG.wider(extra).lower(dy);
    let (body, hover) = if grey {
        (
            "play_button_blank_grey.png",
            "play_button_blank_grey_hover.png",
        )
    } else {
        ("play_button_blank.png", "play_button_blank_hover.png")
    };
    kit.image_stretched(body, at, PLAY_NATIVE, PLAY_CAPS, Color32::WHITE);
    if t > 0.01 {
        kit.image_stretched(hover, at, PLAY_NATIVE, PLAY_CAPS, design::fade(t));
    }
}

fn play_label_at(kit: &mut Kit, label: &str, lit: bool, t: f32, cx: f32, cy: f32, room: f32) {
    let (fg, shadow) = if lit {
        (
            style::mix(Color32::from_rgb(214, 250, 206), Color32::WHITE, t),
            Color32::from_rgb(12, 120, 12),
        )
    } else {
        (Color32::from_gray(225), Color32::from_gray(70))
    };
    let mut font = play_font(kit);
    let width = dz(room);
    let text_w = kit.text_width(label, font.clone());
    if text_w > width {
        font.size *= width / text_w;
    }
    let c = kit.drect(Dr::new(cx, cy, 0.0, 0.0)).min;
    kit.ui.painter().text(
        c + egui::vec2(0.0, 1.0),
        egui::Align2::CENTER_CENTER,
        label,
        font.clone(),
        shadow,
    );
    kit.ui
        .painter()
        .text(c, egui::Align2::CENTER_CENTER, label, font, fg);
}

fn blue_face_at(
    kit: &mut Kit,
    extra: f32,
    enabled: bool,
    t: f32,
    icon: Icon,
    label: &str,
    dy: f32,
) {
    let at = UPDATE_IMG.moved(extra).lower(dy);
    let (body, hover) = if enabled {
        ("blue_button.png", "blue_button_hover.png")
    } else {
        ("blue_button_grey.png", "blue_button_grey_hover.png")
    };
    kit.image_d(body, at);
    if t > 0.01 {
        kit.image_tinted(hover, at, design::fade(t));
    }
    let g = kit.spaced_galley(
        &label.to_uppercase(),
        design::din(20.0),
        design::TEXT,
        dz(1.6),
        false,
    );
    let is = dz(26.0);
    let gap = dz(16.0);
    let c = kit
        .drect(Dr::new(
            UPDATE_LABEL.0 + extra,
            UPDATE_LABEL.1 + dy,
            0.0,
            0.0,
        ))
        .min;
    let left = c.x - (is + gap + g.size().x) / 2.0;
    style::icon_at(
        kit.ui.painter(),
        icon,
        pos2(left, c.y - is / 2.0),
        is,
        design::TEXT,
    );
    kit.ui.painter().galley(
        pos2(left + is + gap, c.y - g.size().y / 2.0),
        g,
        design::TEXT,
    );
}

/// A running job in the buttons' place: the green one fills up with its progress (a
/// sweep while the amount is unknown) over grey, and the blue one becomes CANCEL.
/// Returns true when CANCEL was clicked.
pub(super) fn job_row(kit: &mut Kit, key: &str, extra: f32, job: &JobView) -> bool {
    let play_shape = design::shifted(PLAY_SHAPE, extra, 200.0);
    kit.hot_shape(
        &format!("{key}-main"),
        PLAY_AREA.wider(extra),
        &play_shape,
        false,
        &job.step(),
    );
    play_body(kit, extra, true, 0.0);
    let body = kit.drect(PLAY_AREA.wider(extra));
    let (x0, x1) = (
        body.min.x,
        kit.drect(Dr::new(332.0 + extra, 0.0, 0.0, 0.0)).min.x,
    );
    let lit = match job.fraction {
        Some(f) => Some((x0, x0 + (x1 - x0) * f.clamp(0.0, 1.0))),
        None => {
            let t = kit.ui.input(|i| i.time) as f32;
            let band = (x1 - x0) * INDETERMINATE_BAND_WIDTH_FRACTION;
            let pos = x0 - band + (t * INDETERMINATE_CYCLES_PER_SECOND).fract() * (x1 - x0 + band);
            kit.ui.ctx().request_repaint();
            Some((pos.max(x0), (pos + band).min(x1)))
        }
    };
    if let Some((a, b)) = lit.filter(|(a, b)| b > a) {
        let clip = Rect::from_x_y_ranges(a..=b, body.y_range().expand(20.0));
        let saved = kit.ui.clip_rect();
        kit.ui.set_clip_rect(clip.intersect(saved));
        play_body(kit, extra, false, 0.0);
        kit.ui.set_clip_rect(saved);
    }
    let label = match job.fraction {
        Some(f) => format!("{:.0}%", f * 100.0),
        None => "PREPARING".into(),
    };
    play_label(kit, &label, true, 0.0, extra);

    // CANCEL on the blue button.
    let shape = design::shifted(UPDATE_SHAPE, extra, 0.0);
    let can = !job.cancelling;
    let tip = if can {
        format!("Stop {}", job.title.to_lowercase())
    } else {
        "Stopping…".to_string()
    };
    let (resp, t, pressed) = kit.hot_shape(
        &format!("{key}-side"),
        UPDATE_AREA.moved(extra),
        &shape,
        can,
        &tip,
    );
    let label = if can { "Cancel" } else { "Stopping…" };
    blue_face(kit, extra, can && !pressed, t, Icon::Close, label);
    resp.clicked
}

/// The blank blue button (grey when off, lighter under the pointer, `t`) with an icon and
/// a DMCAPS label where update_button.png has its own.
fn blue_face(kit: &Kit, extra: f32, enabled: bool, t: f32, icon: Icon, label: &str) {
    let at = UPDATE_IMG.moved(extra);
    for name in BLUE_STATES {
        kit.prefetch_d(name, at);
    }
    let (body, hover) = if enabled {
        ("blue_button.png", "blue_button_hover.png")
    } else {
        ("blue_button_grey.png", "blue_button_grey_hover.png")
    };
    kit.image_d(body, at);
    if t > 0.01 {
        kit.image_tinted(hover, at, design::fade(t));
    }
    let g = kit.spaced_galley(
        &label.to_uppercase(),
        design::din(20.0),
        design::TEXT,
        dz(1.6),
        false,
    );
    let is = dz(26.0);
    let gap = dz(16.0);
    let total = is + gap + g.size().x;
    let c = kit
        .drect(Dr::new(UPDATE_LABEL.0 + extra, UPDATE_LABEL.1, 0.0, 0.0))
        .min;
    let left = c.x - total / 2.0;
    style::icon_at(
        kit.ui.painter(),
        icon,
        pos2(left, c.y - is / 2.0),
        is,
        design::TEXT,
    );
    let gp = pos2(left + is + gap, c.y - g.size().y / 2.0);
    kit.ui.painter().galley(gp, g, design::TEXT);
}

/// The font of the green button's labels: letters as tall as the image's own "PLAY" (22
/// of its design pixels). Its lettering is wider than Conthrax, so matching its width
/// would make Conthrax about 45% taller.
fn play_font(kit: &Kit) -> egui::FontId {
    let probe = design::conthrax(40.0);
    let size = 40.0 * dz(PLAY_WORD_W) / kit.text_width("PLAY", probe.clone());
    egui::FontId::new(size * PLAY_CAP_SCALE, probe.family)
}

/// The green button's body at its width (`extra` wider than the concept): green or grey,
/// with its hover image faded in by `t`.
fn play_body(kit: &Kit, extra: f32, grey: bool, t: f32) {
    let at = PLAY_IMG.wider(extra);
    for name in PLAY_STATES {
        kit.prefetch_stretched(name, at, PLAY_NATIVE);
    }
    let draw = |name: &str, tint| kit.image_stretched(name, at, PLAY_NATIVE, PLAY_CAPS, tint);
    let (body, hover) = if grey {
        (
            "play_button_blank_grey.png",
            "play_button_blank_grey_hover.png",
        )
    } else {
        ("play_button_blank.png", "play_button_blank_hover.png")
    };
    draw(body, Color32::WHITE);
    if t > 0.01 {
        draw(hover, design::fade(t));
    }
}

/// `rect` (native pixels) of a green-button image (RUNNING's lettering), moved right by
/// half of `extra` so it stays centred on a wider button.
fn word(kit: &Kit, image: &str, rect: Rect, extra: f32, tint: Color32) {
    let img = kit.drect(PLAY_IMG);
    let tex = kit.assets.tex(
        kit.ui.ctx(),
        image,
        img.width().round() as u32,
        img.height().round() as u32,
    );
    let (nw, nh) = PLAY_NATIVE;
    let s = dz(PLAY_S);
    let dest = Rect::from_min_size(
        img.min + rect.min.to_vec2() * s + egui::vec2(dz(extra / 2.0), 0.0),
        rect.size() * s,
    );
    let uv = Rect::from_min_max(
        pos2(rect.min.x / nw, rect.min.y / nh),
        pos2(rect.max.x / nw, rect.max.y / nh),
    );
    kit.ui.painter().image(tex.id(), dest, uv, tint);
}

/// A label where the image has its "PLAY": green-white on green (whiter under the
/// pointer, `t`), RUNNING's light grey on grey. Set smaller when it is wider than the
/// room there.
fn play_label(kit: &Kit, label: &str, lit: bool, t: f32, extra: f32) {
    let (fg, shadow) = if lit {
        (
            style::mix(Color32::from_rgb(214, 250, 206), Color32::WHITE, t),
            Color32::from_rgb(12, 120, 12),
        )
    } else {
        (Color32::from_gray(225), Color32::from_gray(70))
    };
    let mut font = play_font(kit);
    let room = dz(LABEL_W + extra);
    let w = kit.text_width(label, font.clone());
    if w > room {
        font.size *= room / w;
    }
    let c = kit
        .drect(Dr::new(LABEL_CX + extra / 2.0, LABEL_CY - 1.6, 0.0, 0.0))
        .min;
    let p = kit.ui.painter();
    p.text(
        c + egui::vec2(0.0, 1.0),
        egui::Align2::CENTER_CENTER,
        label,
        font.clone(),
        shadow,
    );
    p.text(c, egui::Align2::CENTER_CENTER, label, font, fg);
}

/// The PCVR | QUEST switch: the image shows the side in use (lighter under the pointer),
/// the other half picks its side.
pub(super) fn switch(
    kit: &mut Kit,
    key: &str,
    extra: f32,
    platform: &mut Platform,
) -> Option<egui::Rect> {
    switch_at(kit, key, extra, 0.0, platform)
}

/// The Play page's compact row moves its platform switch into the logo band.
pub(super) fn play_switch(
    kit: &mut Kit,
    key: &str,
    extra: f32,
    platform: &mut Platform,
) -> Option<egui::Rect> {
    switch_at(kit, key, extra, -155.0, platform)
}

fn switch_at(
    kit: &mut Kit,
    key: &str,
    extra: f32,
    dy: f32,
    platform: &mut Platform,
) -> Option<egui::Rect> {
    let name = match platform {
        Platform::Pc => "hardware_pc",
        Platform::Quest => "hardware_quest",
    };
    let at = SWITCH_IMG.moved(extra).lower(dy);
    let body = kit
        .drect(SWITCH_PC.moved(extra).lower(dy))
        .union(kit.drect(SWITCH_QUEST.moved(extra).lower(dy)));
    let t = kit.hover_t(&format!("{key}-body"), body);
    for other in [
        "hardware_pc.png",
        "hardware_pc_hover.png",
        "hardware_quest.png",
        "hardware_quest_hover.png",
    ] {
        kit.prefetch_d(other, at);
    }
    kit.image_d(&format!("{name}.png"), at);
    if t > 0.01 {
        kit.image_tinted(&format!("{name}_hover.png"), at, design::fade(t));
    }
    let sides = [
        (
            Platform::Pc,
            SWITCH_PC.moved(extra).lower(dy),
            "Echo VR on this PC",
        ),
        (
            Platform::Quest,
            SWITCH_QUEST.moved(extra).lower(dy),
            if super::QUEST {
                "Echo VR on your Quest, over USB"
            } else {
                "Echo VR on Quest: coming soon"
            },
        ),
    ];
    let mut soon = None;
    for (i, (p, area, tip)) in sides.into_iter().enumerate() {
        let r = kit.drect(area);
        if kit
            .hot(&format!("{key}-{i}"), r, *platform != p, tip)
            .0
            .clicked
        {
            if p == Platform::Quest && !super::QUEST {
                soon = Some(r);
            } else {
                *platform = p;
            }
        }
    }
    soon
}

// ---- cards ----

/// A tile's faint fill on a card.
pub(super) const TILE: Color32 = Color32::from_rgba_premultiplied(8, 8, 8, 8);

/// A tile on a card (an installed version, a friend): faint, or VERSIONS' blue with its
/// bar when `selected`.
pub(super) fn tile(kit: &Kit, x: f32, y: f32, w: f32, h: f32, selected: bool) {
    let r = kit.rect(x, y, w, h);
    let p = kit.ui.painter();
    if selected {
        p.rect_filled(r, dz(6.0), design::BLUE.gamma_multiply(0.28));
        let bar = egui::Rect::from_min_size(r.min, egui::vec2(dz(4.0), r.height()));
        p.rect_filled(bar, dz(2.0), design::BLUE);
    } else {
        p.rect_filled(r, dz(6.0), TILE);
    }
}

/// A card like the news cards: card_bg with the violet-to-pink rim and a Conthrax title.
/// Returns the content's left edge, top, width and bottom (logical pixels).
pub(super) fn card_frame(kit: &Kit, r: Dr, title: &str) -> (f32, f32, f32, f32) {
    kit.image_d("card_bg.png", r);
    kit.gradient_frame(
        kit.drect(r),
        dz(6.0),
        dz(2.0),
        design::RIM_TOP,
        design::RIM_BOTTOM,
    );
    let inner = r.shrink(22.0);
    let g = kit.spaced_fit(
        &title.to_uppercase(),
        design::conthrax(24.0),
        design::TEXT,
        dz(1.8),
        false,
        dz(r.w - 44.0),
    );
    let (x, w) = (dz(inner.x), dz(inner.w));
    let y = dz(r.y + 20.0);
    kit.put(x, y, g);
    (x, dz(r.y + 72.0), w, dz(inner.bottom()))
}

#[cfg(test)]
mod play_split_tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Owner {
        Main,
        Arrow,
        Update,
    }

    fn owner(x: f32, y: f32, arrow: bool) -> Option<Owner> {
        let (main, arrow_area, update) = compact_play_regions(arrow);
        let green = design::shifted(PLAY_SHAPE, 120.0, 200.0).map(|(x, y)| (x, y - 155.0));
        let blue = design::shifted(UPDATE_SHAPE, 120.0, 0.0).map(|(x, y)| (x, y - 155.0));
        let contains = |area: Dr, poly: &[(f32, f32)]| {
            x >= area.x
                && x <= area.right()
                && y >= area.y
                && y <= area.bottom()
                && design::inside(
                    pos2(x, y),
                    &poly.iter().map(|&(x, y)| pos2(x, y)).collect::<Vec<_>>(),
                )
        };
        if contains(main, &green) {
            Some(Owner::Main)
        } else if arrow_area.is_some_and(|area| contains(area, &green)) {
            Some(Owner::Arrow)
        } else if contains(update, &blue) {
            Some(Owner::Update)
        } else {
            None
        }
    }

    #[test]
    fn play_split_owns_only_its_polygon_and_keeps_both_seams_inert() {
        assert_eq!(owner(220.0, 120.0, true), Some(Owner::Main));
        assert_eq!(owner(299.9, 120.0, true), Some(Owner::Main));
        assert_eq!(owner(300.0, 120.0, true), None);
        assert_eq!(owner(300.1, 120.0, true), Some(Owner::Arrow));
        assert_eq!(owner(355.0, 120.0, true), Some(Owner::Arrow));
        assert_eq!(owner(300.0, 120.0, false), Some(Owner::Main));
        assert_eq!(owner(444.9, 154.0, true), Some(Owner::Arrow));
        assert_eq!(owner(445.0, 154.0, true), None);
        assert_eq!(owner(445.1, 88.5, true), Some(Owner::Update));
        assert_eq!(owner(600.0, 120.0, true), Some(Owner::Update));
        assert_eq!(owner(390.0, 88.0, true), None);
        assert_eq!(owner(500.0, 154.0, true), None);
    }

    #[test]
    fn long_version_name_is_elided_without_dropping_state_or_progress() {
        let parts = vec!["Beta".repeat(40), "Installing".into(), "35%".into()];
        let (name, state) = primary_info_lines(&parts);
        let visible = fit_info_text_by(&name, 12.0, |s| s.chars().count() as f32);
        assert!(visible.ends_with('…'));
        assert!(visible.len() < name.len());
        assert_eq!(state, "Installing  ·  35%");
    }

    #[test]
    fn job_progress_uses_the_active_main_button_width() {
        // Independent expectations for the approved compact Play design coordinates:
        // origin (139, 88), PC edge x=300, Quest edge x=445, height 66.8. The body
        // endpoints sit 0.001 px before each seam to keep the adjacent regions separate.
        const EXPECTED_PC_PROGRESS_BODY: Dr = Dr::new(139.0, 88.0, 160.999, 66.8);
        const EXPECTED_QUEST_PROGRESS_BODY: Dr = Dr::new(139.0, 88.0, 305.999, 66.8);
        const EXPECTED_PC_PROGRESS_RIGHT: f32 = 299.999;
        const EXPECTED_QUEST_PROGRESS_RIGHT: f32 = 444.999;
        const FULL_PROGRESS_FRACTION: f32 = 1.0;
        const PC_MAIN_RIGHT_EDGE: f32 = 300.0;
        // Near the end of the sweep, the band reaches across the PC/Quest split.
        const INDETERMINATE_SAMPLE_TIME: f32 = 0.99;

        let pc = compact_job_progress_body(true);
        let quest = compact_job_progress_body(false);
        assert_eq!(pc, EXPECTED_PC_PROGRESS_BODY);
        assert_eq!(quest, EXPECTED_QUEST_PROGRESS_BODY);
        assert_eq!(pc.right(), EXPECTED_PC_PROGRESS_RIGHT);
        assert_eq!(quest.right(), EXPECTED_QUEST_PROGRESS_RIGHT);

        let rect = |body: Dr| Rect::from_min_size(pos2(body.x, body.y), egui::vec2(body.w, body.h));
        let pc_determinate = job_progress_clip(rect(pc), Some(FULL_PROGRESS_FRACTION), 0.0);
        let quest_determinate = job_progress_clip(rect(quest), Some(FULL_PROGRESS_FRACTION), 0.0);
        assert_eq!(pc_determinate.max.x, rect(pc).max.x);
        assert!(pc_determinate.max.x < PC_MAIN_RIGHT_EDGE);
        assert_eq!(quest_determinate.max.x, rect(quest).max.x);

        let pc_indeterminate = job_progress_clip(rect(pc), None, INDETERMINATE_SAMPLE_TIME);
        let quest_indeterminate = job_progress_clip(rect(quest), None, INDETERMINATE_SAMPLE_TIME);
        assert!(pc_indeterminate.max.x < PC_MAIN_RIGHT_EDGE);
        assert!(quest_indeterminate.max.x > PC_MAIN_RIGHT_EDGE);
        assert!(quest_indeterminate.max.x <= rect(quest).max.x);
    }
}
