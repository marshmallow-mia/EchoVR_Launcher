//! A plugin's settings as a form, drawn from its description
//! ([`crate::core::launcher::plugin_settings`]): its sections with their captions, and a
//! row per setting with its label on the left, its control on the right and its help under
//! them. The form only reports changes; the caller writes them.

use std::collections::BTreeMap;

use crate::core::launcher::plugin_settings::{self as ps, Field, Kind, Section, Settings, Style};
use crate::ui::controls::SWITCH_W;
use crate::ui::design::{self, dz};
use crate::ui::kit::Kit;
use crate::ui::widgets::{MenuItem, Tone};

/// A control's height.
const CTRL_H: f32 = 30.0;
/// A line of a list of choices (radio buttons, checks).
const LINE_H: f32 = 28.0;
/// Between two settings.
const GAP: f32 = 16.0;
/// Help under a setting (design size).
const HELP: f32 = 13.5;
const LABEL: f32 = 16.0;

/// What the form keeps between frames: text being typed, values that didn't check out,
/// whether advanced settings show, the scroll, an action waiting for its confirmation.
#[derive(Debug, Clone, Default)]
pub(super) struct FormState {
    pub advanced: bool,
    pub scroll: f32,
    drafts: BTreeMap<String, String>,
    errors: BTreeMap<String, String>,
    /// An action asked to be confirmed: its values, written on yes.
    pub pending: Option<Vec<(String, String)>>,
}

/// What the form asks of its caller.
#[derive(Debug, Default)]
pub(super) struct FormOut {
    /// Settings changed (key, value): to write.
    pub changes: Vec<(String, String)>,
    /// An action to confirm first: (its label, the question).
    pub confirm: Option<(String, String)>,
}

enum Item<'a> {
    Section(&'a Section, bool),
    Field(&'a Field),
}

/// Where each visible part goes (y from the form's top), and the form's height.
fn layout<'a>(
    k: &Kit,
    s: &'a Settings,
    values: &BTreeMap<String, String>,
    st: &FormState,
    w: f32,
) -> (Vec<(Item<'a>, f32, f32)>, f32) {
    let mut out = Vec::new();
    let mut y = 0.0;
    for sec in &s.sections {
        let shown: Vec<&Field> = sec
            .fields
            .iter()
            .filter(|f| (st.advanced || !f.advanced) && ps::holds(&f.visible_if, values))
            .collect();
        if shown.is_empty() {
            continue;
        }
        let first = out.is_empty();
        let h = section_height(k, sec, w, first);
        if h > 0.0 {
            out.push((Item::Section(sec, first), y, h));
            y += h;
        }
        for f in shown {
            let h = field_height(k, f, st, w);
            out.push((Item::Field(f), y, h));
            y += h;
        }
    }
    (out, y)
}

/// The form's height `w` wide.
pub(super) fn height(
    k: &Kit,
    s: &Settings,
    values: &BTreeMap<String, String>,
    st: &FormState,
    w: f32,
) -> f32 {
    layout(k, s, values, st, w).1
}

fn text_height(k: &Kit, text: &str, size: f32, w: f32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    k.caps_block(text, size, design::GREY, w)
        .iter()
        .map(|g| g.size().y)
        .sum()
}

fn section_height(k: &Kit, sec: &Section, w: f32, first: bool) -> f32 {
    if sec.title.is_empty() && sec.help.is_empty() {
        return if first { 0.0 } else { dz(10.0) };
    }
    let top = if first { 0.0 } else { dz(18.0) };
    let title = if sec.title.is_empty() { 0.0 } else { dz(36.0) };
    let help = text_height(k, &sec.help, HELP, w);
    top + title + if help > 0.0 { help + dz(12.0) } else { 0.0 }
}

/// Where a check's label starts (its box and gap).
const CHECK_INDENT: f32 = 26.0;

/// A setting's control, and how far its help is indented.
fn control(f: &Field, w: f32, k: &Kit) -> (f32, f32) {
    let col = control_w(w);
    match &f.kind {
        Kind::Bool { .. } if f.style != Style::Switch => (CTRL_H, CHECK_INDENT),
        Kind::Choice { choices } if style_of(f, k, col) == Style::Radio => {
            (LINE_H * choices.len() as f32, 0.0)
        }
        Kind::Multi { choices } if style_of(f, k, col) == Style::Checks => {
            (LINE_H * choices.len() as f32, 0.0)
        }
        Kind::Text { .. } if f.style == Style::Area => (CTRL_H * 3.0, 0.0),
        Kind::Note { .. } => (0.0, 0.0),
        Kind::Link { .. } => (dz(30.0), 0.0),
        _ => (CTRL_H, 0.0),
    }
}

fn label_w(w: f32) -> f32 {
    (w * 0.4).min(dz(420.0))
}

fn control_w(w: f32) -> f32 {
    w - label_w(w) - dz(24.0)
}

/// The style a setting is drawn with `col` wide: segmented buttons and chips that don't
/// fit become a dropdown and checks.
fn style_of(f: &Field, k: &Kit, col: f32) -> Style {
    let fits = |choices: &[ps::Choice]| {
        let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
        k.segmented_width(&labels, CTRL_H) <= col
    };
    match (&f.kind, f.style) {
        (Kind::Choice { choices }, Style::Segmented) if !fits(choices) => Style::Dropdown,
        (Kind::Choice { .. }, Style::Default) => Style::Dropdown,
        (Kind::Multi { choices }, Style::Chips) if !fits(choices) => Style::Checks,
        (Kind::Multi { .. }, Style::Default) => Style::Checks,
        (Kind::Number { .. }, Style::Default) => Style::Field,
        (Kind::Bool { .. }, Style::Default) => Style::Check,
        (_, s) => s,
    }
}

fn under(f: &Field, st: &FormState) -> String {
    match st.errors.get(&f.key) {
        Some(e) => e.clone(),
        None => match &f.kind {
            Kind::Note { text } => text.clone(),
            _ => f.help.clone(),
        },
    }
}

fn field_height(k: &Kit, f: &Field, st: &FormState, w: f32) -> f32 {
    let (ch, indent) = control(f, w, k);
    let text = under(f, st);
    let help = text_height(k, &text, HELP, w - indent);
    let gap_help = if help > 0.0 && ch > 0.0 { dz(6.0) } else { 0.0 };
    ch + gap_help + help + GAP
}

/// Draws the form at (`x`, `y`) `w` wide, clipped to `top..bottom` (the parts outside
/// aren't drawn); returns what changed.
#[allow(clippy::too_many_arguments)]
pub(super) fn form(
    k: &mut Kit,
    key: &str,
    s: &Settings,
    values: &BTreeMap<String, String>,
    st: &mut FormState,
    x: f32,
    y: f32,
    w: f32,
    (top, bottom): (f32, f32),
    enabled: bool,
) -> FormOut {
    let mut out = FormOut::default();
    let (items, _) = layout(k, s, values, st, w);
    let any_live = s.store.live();
    for (item, iy, ih) in items {
        let ry = y + iy;
        if ry + ih < top || ry > bottom {
            continue;
        }
        match item {
            Item::Section(sec, first) => {
                let mut sy = ry + if first { 0.0 } else { dz(18.0) };
                if !sec.title.is_empty() {
                    let g = k.label_galley(&sec.title, design::din(19.0), design::TEXT, w);
                    let r = k.put(x, sy, g);
                    hover_tip(k, &format!("{key}-section-{}", sec.title), r, &sec.tooltip);
                    k.ui.painter().hline(
                        (k.origin.x + x)..=(k.origin.x + x + w),
                        k.origin.y + sy + dz(30.0),
                        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(28)),
                    );
                    sy += dz(36.0);
                }
                k.caps_text(x, sy, w, &sec.help, HELP, design::GREY, 0.0);
            }
            Item::Field(f) => {
                let on = enabled && ps::holds(&f.enabled_if, values);
                // A setting that applies at the next start among live ones says so.
                let next_start = any_live && f.has_value() && !f.live(s);
                field(k, key, s, f, values, st, x, ry, w, on, next_start, &mut out);
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn field(
    k: &mut Kit,
    key: &str,
    _s: &Settings,
    f: &Field,
    values: &BTreeMap<String, String>,
    st: &mut FormState,
    x: f32,
    y: f32,
    w: f32,
    enabled: bool,
    next_start: bool,
    out: &mut FormOut,
) {
    let col = control_w(w);
    let cx = x + w - col;
    let id = format!("{key}-{}", if f.key.is_empty() { &f.label } else { &f.key });
    let value = values.get(&f.key).cloned().unwrap_or_default();
    let tip = f.hover();
    let (ch, indent) = control(f, w, k);
    let style = style_of(f, k, col);
    let set = |out: &mut FormOut, st: &mut FormState, v: String| {
        st.errors.remove(&f.key);
        st.drafts.remove(&f.key);
        if v != value {
            out.changes.push((f.key.clone(), v));
        }
    };

    // The label on the left (a check carries its own).
    let own_label = matches!(
        (&f.kind, style),
        (Kind::Bool { .. }, Style::Check) | (Kind::Note { .. }, _) | (Kind::Link { .. }, _)
    ) || matches!(f.kind, Kind::Action { .. });
    if !own_label {
        let g = k.label_galley(&f.label, design::din(LABEL), design::TEXT, label_w(w));
        let lr = k.put(x, y + (CTRL_H - g.size().y) / 2.0, g);
        hover_tip(k, &format!("{id}-label"), lr, tip);
        if next_start {
            let tx = lr.max.x - k.origin.x + dz(12.0);
            if tx + k.dot_tag_width("Next start", 11.0) < cx - dz(8.0) {
                k.dot_tag(tx, y + CTRL_H / 2.0, "Next start", 11.0, design::QUEST_OFF);
            }
        }
    } else if next_start && f.has_value() {
        // A check carries its label: the tag goes to the row's right end.
        let tw = k.dot_tag_width("Next start", 11.0);
        k.dot_tag(
            cx + col - tw,
            y + CTRL_H / 2.0,
            "Next start",
            11.0,
            design::QUEST_OFF,
        );
    }

    let invalid = st.errors.contains_key(&f.key);
    match &f.kind {
        Kind::Bool { on, off } => {
            let mut b = value == *on;
            let flipped = if style == Style::Switch {
                k.switch(
                    &id,
                    &mut b,
                    &f.label,
                    cx + col - SWITCH_W,
                    y + 3.0,
                    enabled,
                    tip,
                )
            } else {
                k.check(&id, &mut b, &f.label, x, y + 3.0, enabled, tip)
            };
            if flipped {
                set(out, st, if b { on.clone() } else { off.clone() });
            }
        }
        Kind::Text { placeholder, .. } | Kind::Secret { placeholder } => {
            let mut text = st
                .drafts
                .get(&f.key)
                .cloned()
                .unwrap_or_else(|| value.clone());
            let secret = matches!(f.kind, Kind::Secret { .. });
            let ended = if style == Style::Area {
                k.area(&id, &mut text, cx, y, col, ch, invalid, tip)
            } else if secret {
                k.secret_field(
                    &id,
                    &mut text,
                    cx,
                    y,
                    col,
                    CTRL_H,
                    placeholder,
                    invalid,
                    tip,
                )
            } else {
                k.field(
                    &id,
                    &mut text,
                    cx,
                    y,
                    col,
                    CTRL_H,
                    placeholder,
                    invalid,
                    tip,
                )
            };
            commit_text(k, f, st, out, &value, text, ended && enabled);
        }
        Kind::Number {
            min,
            max,
            step,
            unit,
            integer,
        } => {
            let unit_w = if unit.is_empty() {
                0.0
            } else {
                k.label_galley(unit, design::din(LABEL), design::GREY, f32::INFINITY)
                    .size()
                    .x
                    + dz(10.0)
            };
            match style {
                Style::Slider => {
                    let (lo, hi) = (min.unwrap_or(0.0), max.unwrap_or(1.0));
                    let shown = st.drafts.get(&f.key).unwrap_or(&value);
                    let mut n: f64 = shown.parse().unwrap_or(lo);
                    let text_w = dz(110.0);
                    let done = k.slider(
                        &id,
                        &mut n,
                        (lo, hi, *step),
                        &f.label,
                        cx,
                        y + 3.0,
                        col - text_w - dz(12.0),
                        enabled,
                        tip,
                    );
                    let now = ps::snap(n, *min, *max, *step, *integer);
                    if now != *shown {
                        st.drafts.insert(f.key.clone(), now.clone());
                    }
                    let label = if unit.is_empty() {
                        now.clone()
                    } else {
                        format!("{now} {unit}")
                    };
                    let g = k.label_galley(&label, design::din(LABEL), design::TEXT, text_w);
                    k.put(cx + col - text_w, y + (CTRL_H - g.size().y) / 2.0, g);
                    if done {
                        set(out, st, now);
                    }
                }
                Style::Stepper => {
                    let bw = CTRL_H;
                    let fw = dz(110.0);
                    let sx = cx + col - unit_w - bw * 2.0 - fw - 4.0;
                    let n: f64 = value.parse().unwrap_or(min.unwrap_or(0.0));
                    let by = step.unwrap_or(1.0);
                    let mut nudge = None;
                    if k.button(
                        &format!("{id}-dn"),
                        sx,
                        y,
                        bw,
                        CTRL_H,
                        Tone::Dark,
                        None,
                        "",
                        enabled && min.is_none_or(|m| n > m),
                        "Less",
                    )
                    .clicked
                    {
                        nudge = Some(-by);
                    }
                    sign(k, sx, y, bw, false);
                    let mut text = st
                        .drafts
                        .get(&f.key)
                        .cloned()
                        .unwrap_or_else(|| value.clone());
                    let ended = k.field(
                        &id,
                        &mut text,
                        sx + bw + 2.0,
                        y,
                        fw,
                        CTRL_H,
                        "",
                        invalid,
                        tip,
                    );
                    if k.button(
                        &format!("{id}-up"),
                        sx + bw + fw + 4.0,
                        y,
                        bw,
                        CTRL_H,
                        Tone::Dark,
                        None,
                        "",
                        enabled && max.is_none_or(|m| n < m),
                        "More",
                    )
                    .clicked
                    {
                        nudge = Some(by);
                    }
                    sign(k, sx + bw + fw + 4.0, y, bw, true);
                    if let Some(dn) = nudge {
                        let next = ps::snap(n + dn, *min, *max, *step, *integer);
                        set(out, st, next);
                    } else {
                        commit_text(k, f, st, out, &value, text, ended && enabled);
                    }
                    unit_label(k, unit, cx + col - unit_w + dz(10.0), y);
                }
                _ => {
                    let fw = (col - unit_w).min(dz(160.0));
                    let fx = cx + col - unit_w - fw;
                    let mut text = st
                        .drafts
                        .get(&f.key)
                        .cloned()
                        .unwrap_or_else(|| value.clone());
                    let range = match (min, max) {
                        (Some(a), Some(b)) => format!("{} to {}", ps::num(*a), ps::num(*b)),
                        _ => String::new(),
                    };
                    let ended = k.field(&id, &mut text, fx, y, fw, CTRL_H, &range, invalid, tip);
                    commit_text(k, f, st, out, &value, text, ended && enabled);
                    unit_label(k, unit, cx + col - unit_w + dz(10.0), y);
                }
            }
        }
        Kind::Choice { choices } => {
            let current = choices.iter().position(|c| c.value == value);
            match style {
                Style::Segmented => {
                    let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
                    let sel: Vec<bool> = (0..choices.len()).map(|i| Some(i) == current).collect();
                    let tips: Vec<&str> = choices.iter().map(|c| c.hover(f)).collect();
                    let sw = k.segmented_width(&labels, CTRL_H);
                    if let Some(i) =
                        k.segmented(&id, &labels, &sel, cx + col - sw, y, CTRL_H, enabled, &tips)
                    {
                        set(out, st, choices[i].value.clone());
                    }
                }
                Style::Radio => {
                    for (i, c) in choices.iter().enumerate() {
                        let ctip = c.hover(f);
                        if k.radio(
                            &format!("{id}-{i}"),
                            Some(i) == current,
                            &c.label,
                            cx,
                            y + i as f32 * LINE_H + 2.0,
                            enabled,
                            ctip,
                        ) {
                            set(out, st, c.value.clone());
                        }
                    }
                }
                _ => {
                    let label = current.map_or_else(|| value.clone(), |i| choices[i].label.clone());
                    let bw = col.min(dz(360.0));
                    if enabled {
                        let items: Vec<MenuItem> = choices
                            .iter()
                            .enumerate()
                            .map(|(i, c)| MenuItem::Pick {
                                label: c.label.clone(),
                                detail: c.help.clone(),
                                detail_color: design::GREY,
                                checked: Some(i) == current,
                                tip: c.tooltip.clone(),
                            })
                            .collect();
                        if let Some(i) =
                            k.menu_button(&id, &label, &items, cx + col - bw, y, bw, CTRL_H, tip)
                        {
                            set(out, st, choices[i].value.clone());
                        }
                    } else {
                        k.button(
                            &id,
                            cx + col - bw,
                            y,
                            bw,
                            CTRL_H,
                            Tone::Dark,
                            None,
                            &label,
                            false,
                            tip,
                        );
                    }
                }
            }
        }
        Kind::Multi { choices } => {
            let picked: Vec<&str> = value.split(',').map(str::trim).collect();
            let sel: Vec<bool> = choices
                .iter()
                .map(|c| picked.contains(&c.value.as_str()))
                .collect();
            let toggle = |i: usize| -> String {
                choices
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| if *j == i { !sel[i] } else { sel[*j] })
                    .map(|(_, c)| c.value.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            if style == Style::Chips {
                let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
                let tips: Vec<&str> = choices.iter().map(|c| c.hover(f)).collect();
                let sw = k.segmented_width(&labels, CTRL_H);
                if let Some(i) =
                    k.segmented(&id, &labels, &sel, cx + col - sw, y, CTRL_H, enabled, &tips)
                {
                    set(out, st, toggle(i));
                }
            } else {
                for (i, c) in choices.iter().enumerate() {
                    let mut b = sel[i];
                    let ctip = c.hover(f);
                    if k.check(
                        &format!("{id}-{i}"),
                        &mut b,
                        &c.label,
                        cx,
                        y + i as f32 * LINE_H + 2.0,
                        enabled,
                        ctip,
                    ) {
                        set(out, st, toggle(i));
                    }
                }
            }
        }
        Kind::Color { .. } => {
            let mut text = st
                .drafts
                .get(&f.key)
                .cloned()
                .unwrap_or_else(|| value.clone());
            let fw = dz(170.0);
            let fx = cx + col - fw;
            k.swatch(fx - CTRL_H - 8.0, y, CTRL_H, &text);
            let ended = k.field(&id, &mut text, fx, y, fw, CTRL_H, "#rrggbb", invalid, tip);
            commit_text(k, f, st, out, &value, text, ended && enabled);
        }
        Kind::Key { modifiers } => {
            let bw = col.min(dz(300.0));
            if let Some(pressed) = k.key_capture(
                &id,
                &value,
                *modifiers,
                cx + col - bw,
                y,
                bw,
                CTRL_H,
                enabled,
                tip,
            ) {
                match f.check(&pressed) {
                    Ok(v) => set(out, st, v),
                    Err(e) => {
                        st.errors.insert(f.key.clone(), e);
                    }
                }
            }
        }
        Kind::Action {
            sets,
            danger,
            confirm,
        } => {
            let tone = if *danger { Tone::Danger } else { Tone::Dark };
            let bw = k.button_width(&f.label, None, CTRL_H).max(dz(180.0));
            if k.button(&id, x, y, bw, CTRL_H, tone, None, &f.label, enabled, tip)
                .clicked
            {
                if confirm.is_empty() {
                    out.changes.extend(sets.iter().cloned());
                } else {
                    st.pending = Some(sets.clone());
                    out.confirm = Some((f.label.clone(), confirm.clone()));
                }
            }
        }
        Kind::Note { .. } => {}
        Kind::Link { url } => {
            let ltip = if tip.is_empty() { url.as_str() } else { tip };
            if k.link(&id, x, y + dz(4.0), &f.label, 15.0, ltip).clicked {
                crate::core::platform::open_url(url);
            }
        }
    }

    // Under it: what went wrong, else its help.
    let text = under(f, st);
    if !text.is_empty() {
        let color = if st.errors.contains_key(&f.key) {
            design::DANGER
        } else {
            design::GREY
        };
        let hy = y + ch + if ch > 0.0 { dz(6.0) } else { 0.0 };
        k.caps_text(x + indent, hy, w - indent, &text, HELP, color, 0.0);
    }
}

/// `tip` while the pointer is on `r` (a label, a title: not clickable).
fn hover_tip(k: &mut Kit, id: &str, r: egui::Rect, tip: &str) {
    if tip.is_empty() || k.blocked {
        return;
    }
    let id = egui::Id::new(("plugin-form-hover", id));
    let _ =
        k.ui.interact(r, id, egui::Sense::hover())
            .on_hover_text(tip);
}

/// A stepper button's − (or +, `plus`), drawn: the font's minus is a speck at this size.
fn sign(k: &Kit, x: f32, y: f32, s: f32, plus: bool) {
    let c = k.rect(x, y, s, s).center();
    let (half, stroke) = (s * 0.2, egui::Stroke::new(2.0, design::TEXT));
    let p = k.ui.painter();
    p.line_segment(
        [c - egui::vec2(half, 0.0), c + egui::vec2(half, 0.0)],
        stroke,
    );
    if plus {
        p.line_segment(
            [c - egui::vec2(0.0, half), c + egui::vec2(0.0, half)],
            stroke,
        );
    }
}

fn unit_label(k: &Kit, unit: &str, x: f32, y: f32) {
    if unit.is_empty() {
        return;
    }
    let g = k.label_galley(unit, design::din(LABEL), design::GREY, f32::INFINITY);
    k.put(x, y + (CTRL_H - g.size().y) / 2.0, g);
}

/// A text-like field's typing: kept while it differs, checked when editing ends (Escape
/// puts it back).
fn commit_text(
    k: &Kit,
    f: &Field,
    st: &mut FormState,
    out: &mut FormOut,
    value: &str,
    text: String,
    ended: bool,
) {
    if !ended {
        if text != value {
            st.drafts.insert(f.key.clone(), text);
        } else if !st.errors.contains_key(&f.key) {
            st.drafts.remove(&f.key);
        }
        return;
    }
    if k.ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        st.drafts.remove(&f.key);
        st.errors.remove(&f.key);
        return;
    }
    match f.check(&text) {
        Ok(v) => {
            st.drafts.remove(&f.key);
            st.errors.remove(&f.key);
            if v != value {
                out.changes.push((f.key.clone(), v));
            }
        }
        Err(e) => {
            st.drafts.insert(f.key.clone(), text);
            st.errors.insert(f.key.clone(), e);
        }
    }
}

/// The value a row control shows for setting `f` (a bool: on).
pub(super) fn is_on(f: &Field, value: &str) -> bool {
    matches!(&f.kind, Kind::Bool { on, .. } if on == value)
}
