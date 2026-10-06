//! The launcher's controls in the design's language (tokens in `design.rs`): flat buttons
//! in PLAY's green, CHECK FOR UPDATES' blue or the PCVR|QUEST switch's dark side with
//! DMCAPS labels, answer tiles, chips like the Quest chip, build tags, progress
//! bars, text fields, checkboxes, dropdown menus, panels with the news cards' rim, header
//! strips like COMMUNITY NEWS, modal cards and a scroll thumb. Coordinates are logical
//! pixels from the kit's origin, as in `kit.rs`.

use std::sync::Arc;

use egui::epaint::CornerRadiusF32;
use egui::text::LayoutJob;
use egui::{pos2, vec2, Color32, CursorIcon, Galley, Id, Order, Rect, Sense, Stroke, StrokeKind};

use super::design::{self, dz};
use super::kit::Kit;
use super::style::{icon_at, mix, Icon, Resp};

/// Button height.
pub const BTN_H: f32 = 34.0;
/// Corner radius of buttons, fields and chips.
pub const R: f32 = 3.0;
/// Letter spacing of DMCAPS labels.
const SPACING: f32 = dz(1.0);
const PANEL_BUTTON: Color32 = Color32::from_rgb(36, 20, 77);
/// Popup menu rows.
const ROW_H: f32 = 30.0;
/// A menu choice: its label over a note.
const PICK_H: f32 = 44.0;

/// A button's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// PLAY's green: the main action.
    Go,
    /// CHECK FOR UPDATES' blue: a second action.
    Blue,
    /// The switch's dark side: everything else.
    Dark,
    /// The concept's navy, for buttons on the library panel.
    Panel,
    /// Deleting things.
    Danger,
}

impl Tone {
    pub fn fill(self) -> Color32 {
        match self {
            Tone::Go => design::GREEN,
            Tone::Blue => design::BLUE,
            Tone::Dark => design::DARK,
            Tone::Panel => PANEL_BUTTON,
            Tone::Danger => design::RED,
        }
    }
}

/// One entry of a dropdown menu.
pub enum MenuItem {
    Row {
        label: String,
        tip: String,
    },
    /// A choice: its label, a note on the right, and a check on the current one.
    Pick {
        label: String,
        detail: String,
        detail_color: Color32,
        checked: bool,
        tip: String,
    },
    Divider,
}

impl MenuItem {
    pub fn row(label: &str, tip: &str) -> MenuItem {
        MenuItem::Row {
            label: label.into(),
            tip: tip.into(),
        }
    }

    fn height(&self) -> f32 {
        match self {
            MenuItem::Row { .. } => ROW_H,
            MenuItem::Pick { .. } => PICK_H,
            MenuItem::Divider => 9.0,
        }
    }
}

/// Where a menu button remembers that its menu is open.
pub fn menu_id(key: &str) -> Id {
    Id::new(("widgets", key)).with("open")
}

/// One line of DMCAPS caps with the labels' spacing, cut with "…" at `max_w`.
fn caps_line(
    ctx: &egui::Context,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_w: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: font,
            color,
            extra_letter_spacing: SPACING,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
    ctx.fonts_mut(|f| f.layout_job(job))
}

/// A disabled control's fill: the colour sunk into the page's violet.
pub(crate) fn disabled(c: Color32) -> Color32 {
    mix(c, Color32::from_rgb(44, 38, 60), 0.6)
}

/// The label font of a button `h` high.
fn label_font(h: f32) -> egui::FontId {
    super::theme::din((h * 0.39).clamp(10.0, 16.0))
}

/// Text colour on a control.
pub(crate) fn fg(enabled: bool) -> Color32 {
    if enabled {
        design::TEXT
    } else {
        Color32::from_gray(160)
    }
}

impl Kit<'_> {
    fn wid(&self, key: &str) -> Id {
        Id::new(("widgets", key))
    }

    // ---- text ----

    /// DMCAPS caps with the labels' letter spacing, cut with "…" at `max_w`.
    pub fn label_galley(
        &self,
        text: &str,
        font: egui::FontId,
        color: Color32,
        max_w: f32,
    ) -> Arc<Galley> {
        self.spaced_fit(&text.to_uppercase(), font, color, SPACING, false, max_w)
    }

    /// A title in Conthrax with wide letter spacing, as the design's headings.
    pub fn title(&self, x: f32, y: f32, text: &str, size: f32) -> Rect {
        let g = self.spaced_galley(
            &text.to_uppercase(),
            design::conthrax(size),
            design::TEXT,
            dz(size * 0.2),
            false,
        );
        self.put(x, y, g)
    }

    /// A small grey caption over a group, like PCVR and QUEST over the switch.
    pub fn caption(&self, x: f32, y: f32, text: &str) -> Rect {
        let g = self.label_galley(text, design::din(14.0), design::GREY, f32::INFINITY);
        self.put(x, y, g)
    }

    /// Paragraphs (split at blank lines) in DMCAPS caps, paths and file names in Myriad,
    /// wrapped at `w`: one galley each.
    pub fn caps_block(&self, text: &str, size: f32, color: Color32, w: f32) -> Vec<Arc<Galley>> {
        text.trim_end()
            .split("\n\n")
            .map(|para| {
                let mut job = LayoutJob::default();
                for (j, line) in para.split('\n').enumerate() {
                    if j > 0 {
                        design::caps_append(&mut job, "\n", size, color, false);
                    }
                    design::caps_append(&mut job, line, size, color, false);
                }
                job.wrap.max_width = w;
                self.ui.ctx().fonts_mut(|f| f.layout_job(job))
            })
            .collect()
    }

    /// [`Kit::caps_block`] drawn with `gap` between paragraphs; returns its height.
    #[allow(clippy::too_many_arguments)]
    pub fn caps_text(
        &self,
        x: f32,
        y: f32,
        w: f32,
        text: &str,
        size: f32,
        color: Color32,
        gap: f32,
    ) -> f32 {
        let mut y0 = y;
        for (i, g) in self
            .caps_block(text, size, color, w)
            .into_iter()
            .enumerate()
        {
            if i > 0 {
                y0 += gap;
            }
            y0 += self.put(x, y0, g).height();
        }
        y0 - y
    }

    /// How wide [`Kit::link`] sets `text`.
    pub fn link_width(&self, text: &str, size: f32) -> f32 {
        self.spaced_galley(
            &text.to_uppercase(),
            design::din(size),
            design::TEXT,
            dz(0.5),
            true,
        )
        .size()
        .x
    }

    /// An underlined DMCAPS link, as the cards' "JOIN THE DISCORD"; returns the click.
    pub fn link(&mut self, key: &str, x: f32, y: f32, text: &str, size: f32, tip: &str) -> Resp {
        let g = self.spaced_galley(
            &text.to_uppercase(),
            design::din(size),
            design::TEXT,
            dz(0.5),
            true,
        );
        let r = Rect::from_min_size(self.origin + vec2(x, y), g.size());
        self.a11y_name = Some(text.to_string());
        let (resp, t, _) = self.hot(key, r, true, tip);
        let g = if t > 0.01 {
            // Galleys keep their colour: lay the hovered one out again, tinted.
            let color = mix(design::TEXT, Color32::from_rgb(150, 190, 255), t);
            self.spaced_galley(
                &text.to_uppercase(),
                design::din(size),
                color,
                dz(0.5),
                true,
            )
        } else {
            g
        };
        self.ui.painter().galley(r.min, g, design::TEXT);
        resp
    }

    // ---- buttons ----

    /// A button's body: its colour, lighter while hovered (`t`), darker while pressed.
    pub fn button_face(&self, r: Rect, fill: Color32, enabled: bool, t: f32, pressed: bool) {
        let p = self.ui.painter();
        p.rect_filled(r, R, if enabled { fill } else { disabled(fill) });
        if pressed {
            p.rect_filled(r, R, Color32::from_black_alpha(50));
        } else if t > 0.01 {
            p.rect_filled(r, R, Color32::from_white_alpha((34.0 * t) as u8));
        }
    }

    /// An icon and a DMCAPS label centred in `r`.
    fn button_label(&self, r: Rect, icon: Option<Icon>, label: &str, color: Color32) {
        let h = r.height();
        let isz = (h * 0.46).round();
        let gap = if icon.is_some() && !label.is_empty() {
            (h * 0.26).round()
        } else {
            0.0
        };
        let icon_w = icon.map_or(0.0, |_| isz + gap);
        let g = self.label_galley(label, label_font(h), color, r.width() - h * 0.5 - icon_w);
        let mut x = r.center().x - (g.size().x + icon_w) / 2.0;
        if let Some(i) = icon {
            icon_at(
                self.ui.painter(),
                i,
                pos2(x, r.center().y - isz / 2.0),
                isz,
                color,
            );
            x += isz + gap;
        }
        let y = r.center().y - g.size().y / 2.0;
        self.ui.painter().galley(pos2(x, y), g, color);
    }

    /// The width a button needs for its label (and icon) at height `h`.
    pub fn button_width(&self, label: &str, icon: Option<Icon>, h: f32) -> f32 {
        let text = self
            .label_galley(label, label_font(h), design::TEXT, f32::INFINITY)
            .size()
            .x;
        let icon = icon.map_or(0.0, |_| (h * 0.46).round() + (h * 0.26).round());
        (text + icon + h * 1.2).ceil()
    }

    /// A flat button: `tone`'s colour, an optional icon and a DMCAPS label.
    #[allow(clippy::too_many_arguments)]
    pub fn button(
        &mut self,
        key: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tone: Tone,
        icon: Option<Icon>,
        label: &str,
        enabled: bool,
        tip: &str,
    ) -> Resp {
        let r = self.rect(x, y, w, h);
        // Icon-only buttons go by their tooltip.
        self.a11y_name = Some(label.to_string()).filter(|l| !l.is_empty());
        let (resp, t, pressed) = self.hot(key, r, enabled, tip);
        self.button_face(r, tone.fill(), enabled, t, pressed);
        self.button_label(r, icon, label, fg(enabled));
        resp
    }

    /// Buttons in a row ending at `right`, each as wide as its label (`BTN_H` high);
    /// returns the index of the clicked one. The first is `first`, the others dark.
    pub fn button_row(
        &mut self,
        key: &str,
        right: f32,
        y: f32,
        labels: &[&str],
        first: Tone,
    ) -> Option<usize> {
        let widths: Vec<f32> = labels
            .iter()
            .map(|l| self.button_width(l, None, BTN_H).max(96.0))
            .collect();
        let mut x = right - widths.iter().sum::<f32>() - 8.0 * (labels.len() as f32 - 1.0);
        let mut picked = None;
        for (i, (label, w)) in labels.iter().zip(&widths).enumerate() {
            let tone = if i == 0 { first } else { Tone::Dark };
            let key = format!("{key}-{i}");
            if self
                .button(&key, x, y, *w, BTN_H, tone, None, label, true, "")
                .clicked
            {
                picked = Some(i);
            }
            x += w + 8.0;
        }
        picked
    }

    /// A big choice: a dark tile (blue when chosen) with a DMCAPS title and a note under
    /// it.
    #[allow(clippy::too_many_arguments)]
    pub fn tile(
        &mut self,
        key: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        title: &str,
        note: &str,
        selected: bool,
    ) -> Resp {
        let r = self.rect(x, y, w, h);
        self.a11y_name = Some(title.to_string());
        let (resp, t, pressed) = self.hot(key, r, true, "");
        let fill = if selected {
            design::BLUE
        } else {
            Color32::from_rgba_unmultiplied(20, 20, 20, 235)
        };
        self.button_face(r, fill, true, t, pressed);
        if !selected {
            let rim = Color32::from_white_alpha((24.0 + 60.0 * t) as u8);
            self.ui
                .painter()
                .rect_stroke(r, R, Stroke::new(1.0, rim), StrokeKind::Inside);
        }
        // The title and the note, centred in the tile.
        let pad = dz(22.0);
        // A long title in a narrow tile (the rail unfolded) gets smaller before it is cut.
        let upper = title.to_uppercase();
        let size = [23.0, 21.0, 19.0, 17.5]
            .into_iter()
            .find(|&s| {
                self.spaced_galley(&upper, design::din(s), design::TEXT, SPACING, false)
                    .size()
                    .x
                    <= w - 2.0 * pad
            })
            .unwrap_or(17.5);
        let g = self.label_galley(title, design::din(size), design::TEXT, w - 2.0 * pad);
        let color = if selected { design::TEXT } else { design::BODY };
        let notes = self.caps_block(note, 15.0, color, w - 2.0 * pad);
        let gap = dz(8.0);
        let content = g.size().y + gap + notes.iter().map(|n| n.size().y).sum::<f32>();
        let mut ty = y + ((h - content) / 2.0).max(pad);
        ty += self.put(x + pad, ty, g).height() + gap;
        for n in notes {
            ty += self.put(x + pad, ty, n).height();
        }
        resp
    }

    // ---- small parts ----

    /// A status chip like the Quest chip; returns its width.
    pub fn chip(&self, x: f32, y: f32, text: &str, color: Color32) -> f32 {
        let g = self.spaced_galley(
            &text.to_uppercase(),
            design::din(12.0),
            design::TEXT,
            dz(0.5),
            false,
        );
        let r = self.rect(x, y, g.size().x + dz(20.0), dz(27.0));
        self.ui.painter().rect_filled(r, dz(4.0), color);
        self.ui
            .painter()
            .galley(r.center() - g.size() / 2.0, g, design::TEXT);
        r.width()
    }

    /// A tag like LIVE BUILD: a dot and Conthrax caps (`size` design pixels) in `color`,
    /// centred on `cy`; returns its width.
    pub fn dot_tag(&self, x: f32, cy: f32, label: &str, size: f32, color: Color32) -> f32 {
        let g = self.dot_tag_galley(label, size, color);
        let d = dz(size * 0.75);
        self.ui
            .painter()
            .circle_filled(self.origin + vec2(x + d / 2.0, cy), d / 2.0, color);
        let gx = x + d + dz(size * 0.55);
        let w = gx - x + g.size().x;
        self.put(gx, cy - g.size().y / 2.0, g);
        w
    }

    pub fn dot_tag_width(&self, label: &str, size: f32) -> f32 {
        dz(size * 1.3) + self.dot_tag_galley(label, size, design::TEXT).size().x
    }

    fn dot_tag_galley(&self, label: &str, size: f32, color: Color32) -> Arc<Galley> {
        self.spaced_galley(
            &label.to_uppercase(),
            design::conthrax(size),
            color,
            dz(size * 0.08),
            false,
        )
    }

    pub fn chip_width(&self, text: &str) -> f32 {
        let g = self.spaced_galley(
            &text.to_uppercase(),
            design::din(12.0),
            design::TEXT,
            dz(0.5),
            false,
        );
        g.size().x + dz(20.0)
    }

    /// A progress bar: green over dark, with a moving sheen while the amount is unknown,
    /// and its label in DMCAPS.
    #[allow(clippy::too_many_arguments)]
    pub fn progress_bar(&self, x: f32, y: f32, w: f32, h: f32, fraction: Option<f32>, label: &str) {
        let r = self.rect(x, y, w, h);
        let p = self.ui.painter();
        p.rect_filled(r, R, Color32::from_rgba_unmultiplied(20, 20, 20, 225));
        match fraction {
            Some(f) => {
                let fill = Rect::from_min_size(r.min, vec2(w * f.clamp(0.0, 1.0), h));
                p.rect_filled(fill, R, design::GREEN);
            }
            None => {
                let t = self.ui.input(|i| i.time) as f32;
                let seg = w * 0.3;
                let pos = (t * 0.7).fract() * (w + seg) - seg;
                let a = (r.min.x + pos).max(r.min.x);
                let b = (r.min.x + pos + seg).min(r.max.x);
                if b > a {
                    p.rect_filled(
                        Rect::from_x_y_ranges(a..=b, r.y_range()),
                        R,
                        design::GREEN.gamma_multiply(0.7),
                    );
                }
                self.ui.ctx().request_repaint();
            }
        }
        let g = self.label_galley(label, label_font(h.max(26.0)), design::TEXT, w - 16.0);
        self.ui
            .painter()
            .galley(r.center() - g.size() / 2.0, g, design::TEXT);
    }

    /// A text field (Myriad, so paths keep their case): dark, with the card rim when
    /// focused and red when `invalid`. Returns true when editing ended.
    #[allow(clippy::too_many_arguments)]
    pub fn field(
        &mut self,
        key: &str,
        text: &mut String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        placeholder: &str,
        invalid: bool,
        tip: &str,
    ) -> bool {
        self.field_with(key, text, x, y, w, h, placeholder, invalid, tip, false)
    }

    /// [`Kit::field`] showing dots instead of what is typed (a password).
    #[allow(clippy::too_many_arguments)]
    pub fn secret_field(
        &mut self,
        key: &str,
        text: &mut String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        placeholder: &str,
        invalid: bool,
        tip: &str,
    ) -> bool {
        self.field_with(key, text, x, y, w, h, placeholder, invalid, tip, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn field_with(
        &mut self,
        key: &str,
        text: &mut String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        placeholder: &str,
        invalid: bool,
        tip: &str,
        secret: bool,
    ) -> bool {
        let r = self.rect(x, y, w, h);
        let id = self.wid(key);
        let focused = self.ui.memory(|m| m.has_focus(id));
        let p = self.ui.painter();
        p.rect_filled(r, R, Color32::from_rgba_unmultiplied(10, 4, 22, 215));
        if invalid {
            p.rect_stroke(r, R, Stroke::new(1.5, design::DANGER), StrokeKind::Inside);
        } else if focused {
            self.gradient_frame(r, R, 1.5, design::RIM_TOP, design::RIM_BOTTOM);
        } else {
            p.rect_stroke(
                r,
                R,
                Stroke::new(1.0, Color32::from_white_alpha(40)),
                StrokeKind::Inside,
            );
        }
        let font = design::myriad(20.0);
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
            let mut job = LayoutJob::default();
            // A secret is laid out as dots, one per character, so the cursor still fits.
            let shown = if secret {
                "•".repeat(buf.as_str().chars().count())
            } else {
                buf.as_str().to_string()
            };
            design::append_text(
                &mut job,
                &shown,
                egui::TextFormat::simple(font.clone(), design::TEXT),
            );
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let edit = egui::TextEdit::singleline(text)
            .id(id)
            .layouter(&mut layouter)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .vertical_align(egui::Align::Center)
            .desired_width(w - 20.0)
            .password(secret)
            .interactive(!self.blocked);
        let resp = self.ui.put(r.shrink2(vec2(10.0, 2.0)), edit);
        let resp = if tip.is_empty() || self.blocked {
            resp
        } else {
            resp.on_hover_text(tip)
        };
        if text.is_empty() && !placeholder.is_empty() {
            self.ui.painter().with_clip_rect(r).text(
                pos2(r.min.x + 10.0, r.center().y),
                egui::Align2::LEFT_CENTER,
                placeholder,
                design::myriad(20.0),
                design::SUBTLE,
            );
        }
        resp.lost_focus()
    }

    /// How wide [`Kit::check`] is with `label`: its box, the gap and the label.
    pub fn check_width(&self, label: &str) -> f32 {
        let g = self.label_galley(label, design::din(18.0), design::TEXT, f32::INFINITY);
        16.0 + 10.0 + g.size().x
    }

    /// A checkbox: blue with a white tick when on, and a DMCAPS label. Returns true when
    /// it was flipped.
    #[allow(clippy::too_many_arguments)]
    pub fn check(
        &mut self,
        key: &str,
        on: &mut bool,
        label: &str,
        x: f32,
        y: f32,
        enabled: bool,
        tip: &str,
    ) -> bool {
        let (s, h, gap) = (16.0, 24.0, 10.0);
        let g = self.label_galley(label, design::din(18.0), fg(enabled), f32::INFINITY);
        let r = self.rect(x, y, s + gap + g.size().x, h);
        self.a11y_name = Some(label.to_string());
        let (resp, t, pressed) = self.hot(key, r, enabled, tip);
        let b = self.rect(x, y + (h - s) / 2.0, s, s);
        let p = self.ui.painter();
        if *on {
            let fill = if enabled {
                design::BLUE
            } else {
                disabled(design::BLUE)
            };
            p.rect_filled(b, R, fill);
        } else {
            p.rect_filled(b, R, Color32::from_rgba_unmultiplied(20, 20, 20, 225));
            p.rect_stroke(
                b,
                R,
                Stroke::new(1.0, Color32::from_white_alpha((70.0 + 80.0 * t) as u8)),
                StrokeKind::Inside,
            );
        }
        if pressed {
            p.rect_filled(b, R, Color32::from_black_alpha(50));
        } else if t > 0.01 && *on {
            p.rect_filled(b, R, Color32::from_white_alpha((34.0 * t) as u8));
        }
        if *on {
            self.mark(true, fg(enabled), s - 4.0, x + 2.0, y + (h - s) / 2.0 + 2.0);
        }
        let ty = r.center().y - g.size().y / 2.0;
        self.ui
            .painter()
            .galley(pos2(b.max.x + gap, ty), g, fg(enabled));
        if resp.clicked {
            *on = !*on;
            return true;
        }
        false
    }

    // ---- menus ----

    /// A dark button with a chevron that opens a menu under it; returns the picked row.
    #[allow(clippy::too_many_arguments)]
    pub fn menu_button(
        &mut self,
        key: &str,
        label: &str,
        items: &[MenuItem],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tip: &str,
    ) -> Option<usize> {
        let r = self.rect(x, y, w, h);
        self.a11y_name = Some(label.to_string());
        let (resp, t, pressed) = self.hot(key, r, true, tip);
        self.button_face(r, design::DARK, true, t, pressed);
        let chevron = (h * 0.34).round();
        let text_r = Rect::from_min_max(r.min, pos2(r.max.x - chevron - 6.0, r.max.y));
        self.button_label(text_r, None, label, design::TEXT);
        icon_at(
            self.ui.painter(),
            Icon::ChevronDown,
            pos2(r.max.x - chevron - h * 0.4, r.center().y - chevron / 2.0),
            chevron,
            design::TEXT,
        );
        let open_id = menu_id(key);
        if resp.clicked {
            let open = self.ui.ctx().data(|d| d.get_temp::<bool>(open_id));
            self.ui
                .ctx()
                .data_mut(|d| d.insert_temp(open_id, !open.unwrap_or(false)));
            return None;
        }
        let pw = 230.0f32.max(w);
        let anchor = Rect::from_min_size(pos2(r.max.x - pw, r.min.y), vec2(pw, h));
        self.menu_popup(open_id, anchor, items)
    }

    /// Whether the menu of `key` is open.
    pub fn menu_open(&self, key: &str) -> bool {
        let id = menu_id(key);
        self.ui
            .ctx()
            .data(|d| d.get_temp::<bool>(id).unwrap_or(false))
    }

    /// Opens or closes the menu of `key`, for a control that draws its own button.
    pub fn toggle_menu(&self, key: &str) {
        let (id, open) = (menu_id(key), self.menu_open(key));
        self.ui.ctx().data_mut(|d| d.insert_temp(id, !open));
    }

    /// The menu of `key`, if open, under `anchor` (logical, the menu takes its width);
    /// returns the picked row.
    pub fn menu_at(&mut self, key: &str, anchor: Rect, items: &[MenuItem]) -> Option<usize> {
        self.menu_popup(menu_id(key), anchor, items)
    }

    /// The open menu of `open_id` next to `anchor`: violet with the card rim, rows turning
    /// blue under the pointer. Closes on a pick, a click outside or Escape.
    fn menu_popup(&mut self, open_id: Id, anchor: Rect, items: &[MenuItem]) -> Option<usize> {
        let open = self
            .ui
            .ctx()
            .data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
        if !open || self.blocked {
            return None;
        }
        let w = anchor.width();
        let h = items.iter().map(MenuItem::height).sum::<f32>() + 8.0;
        let screen = self.ui.ctx().content_rect();
        let pos = if anchor.max.y + 6.0 + h <= screen.max.y {
            pos2(anchor.min.x, anchor.max.y + 6.0)
        } else {
            pos2(anchor.min.x, anchor.min.y - 6.0 - h)
        };
        let popup = Rect::from_min_size(pos, vec2(w, h));
        let ctx = self.ui.ctx().clone();
        let mut picked = None;
        egui::Area::new(open_id.with("area"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .show(&ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
                let p = ui.painter();
                p.rect_filled(
                    rect.translate(vec2(0.0, 4.0)).expand(2.0),
                    dz(6.0),
                    Color32::from_black_alpha(90),
                );
                p.rect_filled(rect, dz(6.0), design::POPUP);
                let mut y = rect.min.y + 4.0;
                for (i, item) in items.iter().enumerate() {
                    let rr = Rect::from_min_size(
                        pos2(rect.min.x + 4.0, y),
                        vec2(w - 8.0, item.height()),
                    );
                    y += item.height();
                    match item {
                        MenuItem::Divider => {
                            ui.painter().hline(
                                rr.x_range().shrink(8.0),
                                rr.center().y,
                                Stroke::new(1.0, Color32::from_white_alpha(40)),
                            );
                        }
                        MenuItem::Row { label, tip } => {
                            let mut resp = ui.interact(rr, open_id.with(i), Sense::click());
                            if !tip.is_empty() {
                                resp = resp.on_hover_text(tip);
                            }
                            if resp.hovered() {
                                ui.painter().rect_filled(rr, R, design::BLUE);
                                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                            }
                            let g = ui.ctx().fonts_mut(|f| {
                                let mut job = LayoutJob::default();
                                job.append(
                                    &label.to_uppercase(),
                                    0.0,
                                    egui::TextFormat {
                                        font_id: design::din(18.0),
                                        color: design::TEXT,
                                        extra_letter_spacing: SPACING,
                                        ..Default::default()
                                    },
                                );
                                job.wrap =
                                    egui::text::TextWrapping::truncate_at_width(rr.width() - 24.0);
                                f.layout_job(job)
                            });
                            let gy = rr.center().y - g.size().y / 2.0;
                            ui.painter()
                                .galley(pos2(rr.min.x + 12.0, gy), g, design::TEXT);
                            if resp.clicked() {
                                picked = Some(i);
                            }
                        }
                        MenuItem::Pick {
                            label,
                            detail,
                            detail_color,
                            checked,
                            tip,
                        } => {
                            let mut resp = ui.interact(rr, open_id.with(i), Sense::click());
                            if !tip.is_empty() {
                                resp = resp.on_hover_text(tip);
                            }
                            let hot = resp.hovered();
                            if hot {
                                ui.painter().rect_filled(rr, R, design::BLUE);
                                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                            }
                            // The label over its note, and a check on the right.
                            let cs = 14.0;
                            let right = rr.max.x - 12.0 - cs;
                            if *checked {
                                let o = pos2(right, rr.center().y - cs / 2.0);
                                let p = |fx: f32, fy: f32| o + vec2(cs * fx, cs * fy);
                                ui.painter().add(egui::Shape::line(
                                    vec![p(0.15, 0.5), p(0.4, 0.78), p(0.88, 0.2)],
                                    Stroke::new(2.0, design::TEXT),
                                ));
                            }
                            let tw = right - 10.0 - rr.min.x - 12.0;
                            let g = caps_line(ui.ctx(), label, design::din(17.0), design::TEXT, tw);
                            let note = caps_line(
                                ui.ctx(),
                                detail,
                                design::din(13.0),
                                if hot { design::TEXT } else { *detail_color },
                                tw,
                            );
                            let note_h = if detail.is_empty() {
                                0.0
                            } else {
                                note.size().y
                            };
                            let top = rr.center().y - (g.size().y + note_h) / 2.0;
                            let x = rr.min.x + 12.0;
                            if note_h > 0.0 {
                                ui.painter()
                                    .galley(pos2(x, top + g.size().y), note, design::TEXT);
                            }
                            ui.painter().galley(pos2(x, top), g, design::TEXT);
                            if resp.clicked() {
                                picked = Some(i);
                            }
                        }
                    }
                }
            });
        let rim = Kit::rim_shape(popup, dz(6.0), 1.5);
        ctx.layer_painter(egui::LayerId::new(Order::Foreground, open_id.with("area")))
            .add(rim);
        let outside_click = ctx.input(|i| {
            i.pointer.any_click()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !popup.contains(p) && !anchor.contains(p))
        });
        if picked.is_some() || outside_click || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.data_mut(|d| d.insert_temp(open_id, false));
        }
        picked
    }

    // ---- surfaces ----

    /// A panel like the news cards: the violet card with its violet-to-pink rim.
    pub fn panel(&self, x: f32, y: f32, w: f32, h: f32) {
        self.image("card_bg.png", x, y, w, h);
        self.gradient_frame(
            self.rect(x, y, w, h),
            dz(6.0),
            dz(2.0),
            design::RIM_TOP,
            design::RIM_BOTTOM,
        );
    }

    /// A title bar: the concept's translucent violet bar with a Conthrax title. It has
    /// two, the full-width pages' and a shorter one (the Install hero, cards).
    pub fn header_strip(&self, x: f32, y: f32, w: f32, h: f32, title: &str) {
        let r = self.rect(x, y, w, h);
        let (name, native) = if w <= dz(1300.0) {
            ("header_strip_small.png", (1142.0, 44.0))
        } else {
            ("header_strip.png", (1731.0, 45.0))
        };
        let tw = (native.0 * h / native.1).round() as u32;
        let tex = self.assets.tex(self.ui.ctx(), name, tw, h.round() as u32);
        super::style::nine_h(
            self.ui.painter(),
            tex.id(),
            native,
            r,
            (12.0, 12.0),
            Color32::WHITE,
        );
        let size = h / dz(1.0) * 0.43;
        let g = self.spaced_galley(
            &title.to_uppercase(),
            design::conthrax(size),
            design::TEXT,
            dz(size * 0.26),
            false,
        );
        let gy = r.center().y - g.size().y / 2.0;
        self.ui
            .painter()
            .galley(pos2(r.min.x + h * 0.55, gy), g, design::TEXT);
    }

    /// Mouse-wheel scrolling plus a thin draggable thumb at the right edge of the rect.
    #[allow(clippy::too_many_arguments)]
    pub fn scroll_area(
        &mut self,
        key: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        content_h: f32,
        offset: &mut f32,
    ) {
        self.scroll(x, y, w, h, content_h, offset);
        if content_h <= h {
            return;
        }
        let track = self.rect(x + w - 5.0, y + 4.0, 4.0, h - 8.0);
        let thumb_h = (track.height() * h / content_h).max(24.0);
        let span = track.height() - thumb_h;
        let max = content_h - h;
        let ty = track.min.y + span * (*offset / max);
        let thumb = Rect::from_min_size(pos2(track.min.x, ty), vec2(4.0, thumb_h));
        let id = self.wid(key);
        let mut t = 0.0;
        if !self.blocked {
            let resp = self
                .ui
                .interact(thumb.expand2(vec2(4.0, 0.0)), id, Sense::drag());
            if resp.dragged() && span > 0.0 {
                *offset = (*offset + resp.drag_delta().y * max / span).clamp(0.0, max);
            }
            t = self.ui.ctx().animate_bool_with_time(
                id.with("hover"),
                resp.hovered() || resp.dragged(),
                super::style::ANIM,
            );
        }
        let p = self.ui.painter();
        p.rect_filled(track, 2.0, Color32::from_white_alpha(16));
        p.rect_filled(
            thumb,
            2.0,
            mix(design::RIM_TOP, design::RIM_BOTTOM, 0.3 + 0.5 * t),
        );
    }

    /// A card over the whole window: dims the page, swallows its clicks, and runs `f` with
    /// a kit on top whose origin is the window's (`key` names the layer; later ones are
    /// drawn above earlier ones).
    pub fn modal<T>(&mut self, key: &str, blocked: bool, f: impl FnOnce(&mut Kit) -> T) -> T {
        let ctx = self.ctx();
        let screen = self.window();
        let assets = self.assets;
        let id = Id::new(("modal", key));
        let out = egui::Area::new(id)
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(&ctx, |ui| {
                ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter().rect_filled(screen, 0.0, design::SCRIM);
                let mut k = Kit::new(ui, assets, blocked);
                k.origin = screen.min;
                f(&mut k)
            });
        ctx.move_to_top(out.response.layer_id);
        out.inner
    }

    /// An opaque panel for modal cards (the page mustn't show through).
    pub fn solid_panel(&self, x: f32, y: f32, w: f32, h: f32) {
        let r = self.rect(x, y, w, h);
        self.ui.painter().rect_filled(
            r.translate(vec2(0.0, 6.0)).expand(4.0),
            dz(10.0),
            Color32::from_black_alpha(80),
        );
        self.ui
            .painter()
            .rect_filled(r, dz(6.0), Color32::from_rgb(20, 8, 42));
        self.panel(x, y, w, h);
    }

    /// The card rim as a shape, for painters other than the kit's.
    fn rim_shape(r: Rect, radius: f32, width: f32) -> egui::Shape {
        let mut points = Vec::new();
        egui::epaint::tessellator::path::rounded_rectangle(
            &mut points,
            r.shrink(width / 2.0),
            CornerRadiusF32::same(radius),
        );
        let (y0, h) = (r.min.y, r.height().max(1.0));
        let stroke = egui::epaint::PathStroke::new_uv(width, move |_, p| {
            mix(design::RIM_TOP, design::RIM_BOTTOM, (p.y - y0) / h)
        });
        egui::Shape::Path(egui::epaint::PathShape {
            points,
            closed: true,
            fill: Color32::TRANSPARENT,
            stroke,
        })
    }
}
