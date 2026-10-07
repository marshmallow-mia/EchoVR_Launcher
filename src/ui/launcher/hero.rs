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
pub(super) const LOGO: Dr = Dr::new(119.9, 74.4, 747.7, 126.5);
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
    let sep = "  ·  ";
    let color = line.color;
    let layout = |scale: f32, shown: Option<&str>| {
        let size = INFO_SIZE * scale;
        let mut before = LayoutJob::default();
        design::caps_append(&mut before, &line.parts.join(sep), size, color, false);
        let mut path = LayoutJob::default();
        let mut after = LayoutJob::default();
        if let Some(p) = shown {
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
    let mut parts = layout(1.0, line.path.as_deref());
    if width(&parts) > max_w {
        let scale = (max_w / width(&parts)).max(0.8);
        parts = layout(scale * 0.995, line.path.as_deref());
        // Still too long (a narrow column): the path loses its middle, its start and its
        // folder's name stay (the tip has all of it).
        if let Some(p) = &line.path {
            let chars: Vec<char> = p.chars().collect();
            let mut keep = chars.len();
            while width(&parts) > max_w && keep > 16 {
                keep -= 3;
                let head = keep / 3;
                let tail = keep - head;
                let short: String = chars[..head]
                    .iter()
                    .chain(std::iter::once(&'…'))
                    .chain(&chars[chars.len() - tail..])
                    .collect();
                parts = layout(scale * 0.995, Some(&short));
            }
        }
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
    // The fill runs along the bar with a front slanted like its right end: measured where
    // each point's slant meets the bottom, the bar spans x0..x1.
    let (x0, x1) = (PLAY_SHAPE[0].0, PLAY_SHAPE[2].0 + extra);
    let lit = match job.fraction {
        Some(f) => (x0, x0 + (x1 - x0) * f.clamp(0.0, 1.0)),
        None => {
            let t = kit.ui.input(|i| i.time) as f32;
            let band = (x1 - x0) * 0.35;
            let pos = x0 - band + (t * 0.6).fract() * (x1 - x0 + band);
            kit.ui.ctx().request_repaint();
            (pos, pos + band)
        }
    };
    if lit.1 > lit.0 {
        fill_bar(kit, extra, lit);
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

/// How far right a point's slant meets the bar's bottom, per design pixel above it.
const PLAY_SLANT: f32 = (PLAY_SHAPE[2].0 - PLAY_SHAPE[1].0) / (PLAY_SHAPE[2].1 - PLAY_SHAPE[1].1);
/// The rounding of the green button's corners (design pixels), and the fill's shadow.
const PLAY_ROUND: f32 = 3.0;
const FILL_SHADOW: f32 = 7.0;

/// The bar's progress in green between `lit` (bottom x, see [`job_row`]), inside the
/// button's shape and slanted like its end, with a small shadow ahead of it.
fn fill_bar(kit: &Kit, extra: f32, lit: (f32, f32)) {
    let shape = play_outline(extra);
    let u = |(x, y): (f32, f32)| x + (PLAY_SHAPE[2].1 - y) * PLAY_SLANT;
    let band = |a: f32, b: f32| clip(&clip(&shape, |p| u(p) - a), |p| b - u(p));
    let at = |p: (f32, f32)| kit.drect(Dr::new(p.0, p.1, 0.0, 0.0)).min;
    let mut mesh = egui::Mesh::default();
    let mut add = |poly: &[(f32, f32)], color: &dyn Fn((f32, f32)) -> Color32| {
        if poly.len() < 3 {
            return;
        }
        let first = mesh.vertices.len() as u32;
        for &p in poly {
            mesh.colored_vertex(at(p), color(p));
        }
        for i in 1..poly.len() as u32 - 1 {
            mesh.add_triangle(first, first + i, first + i + 1);
        }
    };
    let (a, b) = lit;
    add(&band(b, b + FILL_SHADOW), &|p| {
        let t = ((u(p) - b) / FILL_SHADOW).clamp(0.0, 1.0);
        Color32::from_black_alpha((90.0 * (1.0 - t) * (1.0 - t)) as u8)
    });
    add(&band(a, b), &|_| design::GREEN);
    kit.ui.painter().add(mesh);
}

/// The green button's shape (design pixels), `extra` wider, its left corners rounded
/// as in its image.
fn play_outline(extra: f32) -> Vec<(f32, f32)> {
    let s = design::shifted(PLAY_SHAPE, extra, 200.0);
    let (left, top, bottom) = (s[0].0, s[0].1, s[3].1);
    let r = PLAY_ROUND;
    let arc = |cx: f32, cy: f32, from: f32| {
        (0..=4).map(move |i| {
            let a = (from + 90.0 * i as f32 / 4.0).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
    };
    let mut out: Vec<(f32, f32)> = arc(left + r, top + r, 180.0).collect();
    out.extend([s[1], s[2]]);
    out.extend(arc(left + r, bottom - r, 90.0));
    out
}

/// The part of convex polygon `poly` where `side` is not negative (`side` linear).
fn clip(poly: &[(f32, f32)], side: impl Fn((f32, f32)) -> f32) -> Vec<(f32, f32)> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    for (i, &p) in poly.iter().enumerate() {
        let q = poly[(i + 1) % poly.len()];
        let (sp, sq) = (side(p), side(q));
        if sp >= 0.0 {
            out.push(p);
        }
        if (sp >= 0.0) != (sq >= 0.0) {
            let t = sp / (sp - sq);
            out.push((p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t));
        }
    }
    out
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
/// the other half picks its side. While Quest is off (`super::QUEST`) its side stays
/// unpicked: the Quest half's rectangle comes back when it was clicked, for the "coming
/// soon".
pub(super) fn switch(
    kit: &mut Kit,
    key: &str,
    extra: f32,
    platform: &mut Platform,
) -> Option<egui::Rect> {
    let name = match platform {
        Platform::Pc => "hardware_pc",
        Platform::Quest => "hardware_quest",
    };
    let at = SWITCH_IMG.moved(extra);
    let body = kit
        .drect(SWITCH_PC.moved(extra))
        .union(kit.drect(SWITCH_QUEST.moved(extra)));
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
        (Platform::Pc, SWITCH_PC.moved(extra), "Echo VR on this PC"),
        (
            Platform::Quest,
            SWITCH_QUEST.moved(extra),
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
