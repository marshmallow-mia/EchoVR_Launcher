//! The launcher design (a 1920×1080 concept, drawn at 2/3 in the 1280×720 window):
//! geometry in design pixels, image buttons with shaped hit areas, gradient frames,
//! downloaded textures, letter-spaced text and the icon rail.

use std::sync::Arc;

use egui::epaint::{CornerRadiusF32, PathShape, PathStroke, RectShape};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    pos2, vec2, Color32, CornerRadius, CursorIcon, Galley, Id, Pos2, Rect, Sense, Shape, Vec2,
};

use super::kit::Kit;
use super::style::{self, icon_at, mix, Icon, Resp, ANIM};
use super::theme;

/// Design pixels to logical pixels.
pub const fn dz(px: f32) -> f32 {
    px * (2.0 / 3.0)
}

/// A rect in design pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dr {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Dr {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Dr {
        Dr { x, y, w, h }
    }

    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub fn shrink(self, d: f32) -> Dr {
        Dr::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }

    /// Moved right by `dx`.
    pub fn moved(self, dx: f32) -> Dr {
        Dr::new(self.x + dx, self.y, self.w, self.h)
    }

    /// `dx` wider.
    pub fn wider(self, dx: f32) -> Dr {
        Dr::new(self.x, self.y, self.w + dx, self.h)
    }

    /// `dy` taller.
    pub fn taller(self, dy: f32) -> Dr {
        Dr::new(self.x, self.y, self.w, self.h + dy)
    }
}

/// A polygon moved right by `dx` (all points, or only those right of `from_x`).
pub fn shifted<const N: usize>(shape: [(f32, f32); N], dx: f32, from_x: f32) -> [(f32, f32); N] {
    shape.map(|(x, y)| if x > from_x { (x + dx, y) } else { (x, y) })
}

// ---- colours ----

/// Main text on the dark panels.
pub const TEXT: Color32 = Color32::WHITE;
/// Status bar and info line grey.
pub const GREY: Color32 = Color32::from_rgb(124, 122, 128);
/// Discord's body text on the panel.
pub const BODY: Color32 = Color32::from_rgb(190, 188, 202);
/// Discord's `-#` subtext and inactive labels.
pub const SUBTLE: Color32 = Color32::from_rgb(150, 145, 166);
/// Headings and field names on the panel.
pub const HEADING: Color32 = Color32::from_rgb(228, 228, 235);
pub const LINK: Color32 = Color32::from_rgb(82, 112, 222);
/// The status bar: translucent violet over the art.
pub const BAR: Color32 = Color32::from_rgba_unmultiplied_const(40, 9, 138, 107);
pub const QUEST_ON: Color32 = Color32::from_rgb(26, 178, 26);
pub const QUEST_WARN: Color32 = Color32::from_rgb(214, 150, 20);
pub const QUEST_OFF: Color32 = Color32::from_rgb(70, 64, 92);
/// The card rim: violet at the top to pink at the bottom.
pub const RIM_TOP: Color32 = Color32::from_rgb(122, 38, 232);
pub const RIM_BOTTOM: Color32 = Color32::from_rgb(250, 112, 255);
pub const DANGER: Color32 = Color32::from_rgb(255, 96, 96);
/// PLAY's green, CHECK FOR UPDATES' blue and the PCVR|QUEST switch's dark side: the
/// fills of every button.
pub const GREEN: Color32 = Color32::from_rgb(27, 189, 27);
pub const BLUE: Color32 = Color32::from_rgb(0, 102, 255);
pub const DARK: Color32 = Color32::from_rgb(20, 20, 20);
/// Destructive buttons (Remove, Delete).
pub const RED: Color32 = Color32::from_rgb(214, 44, 64);
/// Popups and tooltips: the card's violet, opaque.
pub const POPUP: Color32 = Color32::from_rgba_unmultiplied_const(30, 16, 56, 248);
/// The dimmed page behind a dialog or overlay card.
pub const SCRIM: Color32 = Color32::from_rgba_unmultiplied_const(6, 2, 16, 170);

/// Even-odd point-in-polygon.
fn inside(p: Pos2, poly: &[Pos2]) -> bool {
    let mut hit = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            hit = !hit;
        }
        j = i;
    }
    hit
}

/// An image of `size` scaled to cover `r`: its scaled size, and the centred part of it
/// that shows (texture coordinates).
fn cover(size: Vec2, r: Rect) -> (Vec2, Rect) {
    let size = size.max(vec2(1.0, 1.0));
    let s = (r.width() / size.x).max(r.height() / size.y);
    let (uw, uh) = (r.width() / (size.x * s), r.height() / (size.y * s));
    let uv = Rect::from_min_size(pos2((1.0 - uw) / 2.0, (1.0 - uh) / 2.0), vec2(uw, uh));
    (size * s, uv)
}

/// A tint that draws an image at `t` of its opacity: hover images fade in over the
/// normal ones.
pub fn fade(t: f32) -> Color32 {
    Color32::from_white_alpha((255.0 * t.clamp(0.0, 1.0)).round() as u8)
}

/// Something to draw in a rail slot.
#[derive(Debug, Clone, Copy)]
pub enum RailIcon {
    /// An embedded image and its height in design pixels.
    Image(&'static str, f32),
    /// A vector icon and its size in design pixels.
    Vector(Icon, f32),
}

impl Kit<'_> {
    /// Runs `f` with everything drawn `(dx, dy)` design pixels further right and down.
    /// The window's room beyond the design's 1280×720 (design pixels): the main column
    /// is this much wider, panels and lists this much taller.
    pub fn dx(&self) -> f32 {
        self.ex / dz(1.0)
    }

    pub fn dy(&self) -> f32 {
        self.ey / dz(1.0)
    }

    /// An image drawn into `r` as the design has it, `extra` wider: the part between the
    /// `caps` (design pixels at its ends) is stretched.
    pub fn image_wider(&self, name: &str, r: Dr, extra: f32, caps: (f32, f32), tint: Color32) {
        let base = self.drect(r);
        let (w, h) = (base.width().round() as u32, base.height().round() as u32);
        let tex = self.assets.tex(self.ui.ctx(), name, w, h);
        let rect = self.drect(r.wider(extra));
        style::nine_h(self.ui.painter(), tex.id(), (r.w, r.h), rect, caps, tint);
    }

    pub fn offset<R>(&mut self, dx: f32, dy: f32, f: impl FnOnce(&mut Kit) -> R) -> R {
        let saved = self.origin;
        self.origin += vec2(dz(dx), dz(dy));
        let out = f(self);
        self.origin = saved;
        out
    }

    pub fn drect(&self, r: Dr) -> Rect {
        self.rect(dz(r.x), dz(r.y), dz(r.w), dz(r.h))
    }

    fn dpos(&self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(dz(x), dz(y))
    }

    pub fn image_d(&self, name: &str, r: Dr) {
        self.image(name, dz(r.x), dz(r.y), dz(r.w), dz(r.h));
    }

    /// Has `name` scaled for `r` in the background: a state image (hover, pressed) that
    /// isn't drawn yet, so it is ready when it is.
    pub fn prefetch_d(&self, name: &str, r: Dr) {
        let rect = self.drect(r);
        let (w, h) = (rect.width().round() as u32, rect.height().round() as u32);
        self.assets.prefetch(self.ui.ctx(), name, w, h);
    }

    /// `prefetch_d` for an image drawn with `image_stretched`.
    pub fn prefetch_stretched(&self, name: &str, r: Dr, native: (f32, f32)) {
        let rect = self.drect(r);
        let w = native.0 * rect.height() / native.1;
        let (w, h) = (w.round() as u32, rect.height().round() as u32);
        self.assets.prefetch(self.ui.ctx(), name, w, h);
    }

    /// An embedded image multiplied by `tint` (grey = dimmed, alpha = faded).
    pub fn image_tinted(&self, name: &str, r: Dr, tint: Color32) {
        let rect = self.drect(r);
        let tex = self.assets.tex(
            self.ui.ctx(),
            name,
            rect.width().round() as u32,
            rect.height().round() as u32,
        );
        self.paint_tex(&tex, rect, tint);
    }

    /// An embedded image stretched to `r`'s width without distorting its ends: `caps` are
    /// the left and right parts (in the image's `native` pixels) that keep their shape; the
    /// column between them is stretched.
    pub fn image_stretched(
        &self,
        name: &str,
        r: Dr,
        native: (f32, f32),
        caps: (f32, f32),
        tint: Color32,
    ) {
        let rect = self.drect(r);
        let w = native.0 * rect.height() / native.1;
        let tex = self.assets.tex(
            self.ui.ctx(),
            name,
            w.round() as u32,
            rect.height().round() as u32,
        );
        style::nine_h(self.ui.painter(), tex.id(), native, rect, caps, tint);
    }

    /// An embedded image stretched over `r` with rounded corners, leaving out `inset`
    /// native pixels at its edges (a panel image's own square border, which a rim
    /// replaces).
    pub fn image_rounded(&self, name: &str, r: Dr, radius: f32, inset: f32) {
        let rect = self.drect(r);
        let tex = self.assets.tex(
            self.ui.ctx(),
            name,
            rect.width().round() as u32,
            rect.height().round() as u32,
        );
        let (nw, nh) = super::assets::native_size(name);
        let (ix, iy) = (inset / nw.max(1) as f32, inset / nh.max(1) as f32);
        let uv = Rect::from_min_max(pos2(ix, iy), pos2(1.0 - ix, 1.0 - iy));
        self.ui.painter().add(
            RectShape::filled(rect, CornerRadius::from(radius), Color32::WHITE)
                .with_texture(tex.id(), uv),
        );
    }

    /// A downloaded image covering `r` (cropped to its aspect), with rounded corners.
    pub fn texture_cover(&self, tex: &egui::TextureHandle, r: Rect, radius: f32) {
        let [tw, th] = tex.size();
        let (_, uv) = cover(vec2(tw as f32, th as f32), r);
        self.ui.painter().add(
            RectShape::filled(r, CornerRadius::from(radius), Color32::WHITE)
                .with_texture(tex.id(), uv),
        );
    }

    /// An embedded image covering `r` (cropped to its aspect), with rounded corners.
    pub fn image_cover(&self, name: &str, r: Dr, radius: f32) {
        let rect = self.drect(r);
        let (nw, nh) = super::assets::native_size(name);
        let (size, uv) = cover(vec2(nw as f32, nh as f32), rect);
        let tex = self.assets.tex(
            self.ui.ctx(),
            name,
            size.x.round() as u32,
            size.y.round() as u32,
        );
        self.ui.painter().add(
            RectShape::filled(rect, CornerRadius::from(radius), Color32::WHITE)
                .with_texture(tex.id(), uv),
        );
    }

    /// A rounded rim whose colour runs from `top` to `bottom`.
    pub fn gradient_frame(&self, r: Rect, radius: f32, width: f32, top: Color32, bottom: Color32) {
        self.rim(r, radius, width, move |t| mix(top, bottom, t));
    }

    /// A rounded rim coloured by `color(t)`, `t` running from 0 at the top to 1 at the
    /// bottom.
    pub fn rim(
        &self,
        r: Rect,
        radius: f32,
        width: f32,
        color: impl Fn(f32) -> Color32 + Send + Sync + 'static,
    ) {
        let mut points = Vec::new();
        egui::epaint::tessellator::path::rounded_rectangle(
            &mut points,
            r.shrink(width / 2.0),
            CornerRadiusF32::same(radius),
        );
        let (y0, h) = (r.min.y, r.height().max(1.0));
        let stroke = PathStroke::new_uv(width, move |_, p| color((p.y - y0) / h));
        self.ui.painter().add(Shape::Path(PathShape {
            points,
            closed: true,
            fill: Color32::TRANSPARENT,
            stroke,
        }));
    }

    /// Single-line text with extra letter spacing (and an optional underline).
    pub fn spaced_galley(
        &self,
        text: &str,
        font: egui::FontId,
        color: Color32,
        spacing: f32,
        underline: bool,
    ) -> Arc<Galley> {
        let mut job = LayoutJob::default();
        job.append(
            text,
            0.0,
            TextFormat {
                font_id: font,
                color,
                extra_letter_spacing: spacing,
                underline: if underline {
                    egui::Stroke::new(1.0, color)
                } else {
                    egui::Stroke::NONE
                },
                ..Default::default()
            },
        );
        self.ui.ctx().fonts_mut(|f| f.layout_job(job))
    }

    /// Like [`Kit::spaced_galley`], but cut with "…" at `max_w` (logical pixels).
    pub fn spaced_fit(
        &self,
        text: &str,
        font: egui::FontId,
        color: Color32,
        spacing: f32,
        underline: bool,
        max_w: f32,
    ) -> Arc<Galley> {
        let mut job = LayoutJob::default();
        job.append(
            text,
            0.0,
            TextFormat {
                font_id: font,
                color,
                extra_letter_spacing: spacing,
                underline: if underline {
                    egui::Stroke::new(1.0, color)
                } else {
                    egui::Stroke::NONE
                },
                ..Default::default()
            },
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
        self.ui.ctx().fonts_mut(|f| f.layout_job(job))
    }

    /// Draws a galley with its top-left at logical (x, y); returns where it went.
    pub fn put(&self, x: f32, y: f32, g: Arc<Galley>) -> Rect {
        let r = Rect::from_min_size(self.origin + vec2(x, y), g.size());
        self.ui.painter().galley(r.min, g, TEXT);
        r
    }

    /// Hover/click inside `shape` (a polygon in design pixels), reacting only within
    /// `area`. Buttons whose images overlap (PLAY's slant under CHECK FOR UPDATES') get
    /// areas that don't, and their shapes keep the empty corners inert.
    pub fn hot_shape(
        &mut self,
        key: &str,
        area: Dr,
        shape: &[(f32, f32)],
        enabled: bool,
        tip: &str,
    ) -> (Resp, f32, bool) {
        let id = Id::new(("design", key));
        let poly: Vec<Pos2> = shape.iter().map(|&(x, y)| self.dpos(x, y)).collect();
        let name = self.a11y_name.take();
        let mut out = Resp::default();
        let mut pressed = false;
        if !self.blocked {
            let sense = if enabled {
                Sense::click()
            } else {
                Sense::hover()
            };
            let resp = self.ui.interact(self.drect(area), id, sense);
            let name = name.unwrap_or_else(|| super::style::a11y_fallback(key, tip));
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name.clone())
            });
            let within = |p: Option<Pos2>| p.is_some_and(|p| inside(p, &poly));
            out.hovered = resp.hovered() && within(resp.hover_pos());
            if enabled {
                // A click without a pointer (keyboard, accessibility tools) is in the shape.
                let at = resp.interact_pointer_pos();
                out.clicked = resp.clicked() && (at.is_none() || within(at));
                pressed = out.hovered && resp.is_pointer_button_down_on();
                if out.hovered {
                    self.ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
            }
            if out.hovered && !tip.is_empty() {
                resp.on_hover_text(tip);
            }
        }
        // Disabled buttons have hover images too (their tooltip says why they are off).
        let t = self
            .ui
            .ctx()
            .animate_bool_with_time(id.with("hover"), out.hovered, ANIM);
        (out, t, pressed)
    }

    /// Feedback over an image button without a hover image of its own: a light veil in
    /// its shape (`t`, as the grey buttons lighten from 92 to 101), dark while pressed.
    pub fn shape_veil(&self, shape: &[(f32, f32)], t: f32, pressed: bool) {
        let color = if pressed {
            Color32::from_black_alpha(50)
        } else {
            Color32::from_white_alpha((16.0 * t) as u8)
        };
        if pressed || t > 0.01 {
            let poly = shape.iter().map(|&(x, y)| self.dpos(x, y)).collect();
            self.ui
                .painter()
                .add(Shape::convex_polygon(poly, color, egui::Stroke::NONE));
        }
    }

    /// A rail slot centred at design y `cy`: the blue glow square when selected (lighter
    /// under the pointer), the violet square on hover (lighter while pressed), and the icon.
    pub fn rail_item(
        &mut self,
        key: &str,
        icon: RailIcon,
        cy: f32,
        selected: bool,
        tip: &str,
    ) -> bool {
        let sq = Dr::new(21.0, cy - 26.5, 52.0, 53.0);
        let (resp, t, pressed) = self.hot(key, self.drect(sq), true, tip);
        if selected {
            // sidebar_selected(_hover).png: a 264 px square at (74, 74) in its 412×416 glow.
            let s = sq.w / 264.0;
            let glow = Dr::new(sq.x - 74.0 * s, sq.y - 74.0 * s, 412.0 * s, 416.0 * s);
            self.image_d("sidebar_selected.png", glow);
            self.image_tinted("sidebar_selected_hover.png", glow, fade(t));
        } else if pressed {
            self.image_d("sidebar_pressed.png", sq);
        } else if t > 0.01 {
            self.image_tinted("sidebar_hover.png", sq, fade(t));
        }
        let (cx, cy) = (47.0, cy);
        match icon {
            RailIcon::Image(name, h) => {
                let (nw, nh) = super::assets::native_size(name);
                let w = h * nw as f32 / nh.max(1) as f32;
                self.image_d(name, Dr::new(cx - w / 2.0, cy - h / 2.0, w, h));
            }
            RailIcon::Vector(i, s) => {
                let o = self.dpos(cx - s / 2.0, cy - s / 2.0);
                let color = if selected {
                    TEXT
                } else {
                    mix(Color32::from_gray(225), TEXT, t)
                };
                icon_at(self.ui.painter(), i, o, dz(s), color);
            }
        }
        resp.clicked
    }

    /// Clicks on something already drawn at `r` (links): returns the click.
    pub fn click_area(&mut self, key: &str, r: Rect, tip: &str) -> bool {
        self.hot(key, r, true, tip).0.clicked
    }
}

/// Appends `text`, taking "-" and "_" from Liberation Sans when the font is Myriad: Myriad's
/// thin dash and underscore vanish at small sizes.
pub fn append_text(job: &mut LayoutJob, text: &str, format: TextFormat) {
    let myriad = match &format.font_id.family {
        egui::FontFamily::Name(n) if &**n == theme::MYRIAD => Some(false),
        egui::FontFamily::Name(n) if &**n == theme::MYRIAD_BOLD => Some(true),
        _ => None,
    };
    let Some(bold) = myriad else {
        job.append(text, 0.0, format);
        return;
    };
    let size = format.font_id.size;
    let thin = |c: char| c == '-' || c == '_';
    let mut rest = text;
    while !rest.is_empty() {
        let is_thin = rest.starts_with(thin);
        let end = rest
            .find(|c: char| thin(c) != is_thin)
            .unwrap_or(rest.len());
        let mut f = format.clone();
        if is_thin {
            f.font_id = if bold {
                theme::arial_bold(size)
            } else {
                theme::arial(size)
            };
        }
        job.append(&rest[..end], 0.0, f);
        rest = &rest[end..];
    }
}

/// A word that has to keep its case: a path, a file name or a link.
pub fn keeps_case(word: &str) -> bool {
    let w = word.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | '"' | '\'' | ':'));
    if w.contains('/') || w.contains('\\') || w.starts_with("http") {
        return true;
    }
    // "echovr.exe", "r15_26-06-23.apk", but not "4.3" or the end of a sentence.
    w.rsplit_once('.').is_some_and(|(stem, ext)| {
        stem.chars().any(|c| c.is_alphabetic())
            && (2..=4).contains(&ext.len())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            && ext.chars().any(|c| c.is_ascii_alphabetic())
    })
}

/// Text in the design's two faces, as the info line sets it: DMCAPS caps, and Myriad for
/// the words that have to keep their case (see [`keeps_case`]). `keep` forces Myriad for
/// all of it (a path with spaces).
pub fn caps_append(job: &mut LayoutJob, text: &str, size: f32, color: Color32, keep: bool) {
    let format = |font| TextFormat {
        font_id: font,
        color,
        extra_letter_spacing: dz(0.5),
        valign: egui::Align::Center,
        ..Default::default()
    };
    // Myriad a little larger, so its capitals stand as tall as DMCAPS'.
    let myriad = || myriad(size * 1.09);
    if keep {
        append_text(job, text, format(myriad()));
        return;
    }
    let mut rest = text;
    while !rest.is_empty() {
        let space = rest.starts_with(char::is_whitespace);
        let end = rest
            .find(|c: char| c.is_whitespace() != space)
            .unwrap_or(rest.len());
        let part = &rest[..end];
        if !space && keeps_case(part) {
            append_text(job, part, format(myriad()));
        } else {
            job.append(&part.to_uppercase(), 0.0, format(din(size)));
        }
        rest = &rest[end..];
    }
}

/// DIN caps (status bar, info line, card text).
pub fn din(size_design: f32) -> egui::FontId {
    theme::din(dz(size_design))
}

pub fn myriad(size_design: f32) -> egui::FontId {
    theme::myriad(dz(size_design))
}

pub fn myriad_bold(size_design: f32) -> egui::FontId {
    theme::myriad_bold(dz(size_design))
}

pub fn conthrax(size_design: f32) -> egui::FontId {
    style::display(dz(size_design))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_hit_test() {
        let tri = [pos2(0.0, 0.0), pos2(10.0, 0.0), pos2(0.0, 10.0)];
        assert!(inside(pos2(2.0, 2.0), &tri));
        assert!(!inside(pos2(8.0, 8.0), &tri));
        assert!(!inside(pos2(-1.0, 2.0), &tri));
    }
}
