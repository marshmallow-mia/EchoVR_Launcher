//! Shared drawing basics: hover/click areas with an animated hover amount, 3-slice
//! stretched images, colour mixing and the vector icons. The controls themselves are in
//! `widgets.rs`, the design's tokens in `design.rs`.

use egui::{
    pos2, vec2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextureId,
};

use super::kit::Kit;
use super::theme;

pub const ANIM: f32 = 0.12;

/// Conthrax.
pub fn display(size: f32) -> egui::FontId {
    theme::conthrax(size)
}

/// `a` → `b` by `t` (0..=1), per channel including alpha.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

/// A slow pulse for busy text (`phase += 0.15` per 50 ms), 0..1.
pub fn pulse(ctx: &egui::Context) -> f32 {
    let t = ctx.input(|i| i.time) as f32;
    (t * 3.0).sin() * 0.5 + 0.5
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Download,
    Mods,
    Globe,
    Gear,
    Folder,
    Refresh,
    Warning,
    Headset,
    ChevronDown,
    Info,
    Close,
    Copy,
}

/// What a control reports back.
#[derive(Default, Clone, Copy)]
pub struct Resp {
    pub clicked: bool,
    pub hovered: bool,
}

// ---- 9-slice ----

/// Draws `tex` into `r`, keeping the left/right caps (in texture pixels) unstretched
/// relative to the height and stretching only the middle horizontally.
pub fn nine_h(
    p: &egui::Painter,
    tex: TextureId,
    tex_size: (f32, f32),
    r: Rect,
    caps: (f32, f32),
    tint: Color32,
) {
    let (tw, th) = tex_size;
    let s = r.height() / th;
    let (mut l, mut rr) = (caps.0 * s, caps.1 * s);
    if l + rr > r.width() {
        let k = r.width() / (l + rr);
        l *= k;
        rr *= k;
    }
    let (ul, ur) = (caps.0 / tw, 1.0 - caps.1 / tw);
    let parts = [
        (r.min.x, r.min.x + l, 0.0, ul),
        (r.min.x + l, r.max.x - rr, ul, ur),
        (r.max.x - rr, r.max.x, ur, 1.0),
    ];
    for (x0, x1, u0, u1) in parts {
        if x1 > x0 {
            p.image(
                tex,
                Rect::from_min_max(pos2(x0, r.min.y), pos2(x1, r.max.y)),
                Rect::from_min_max(pos2(u0, 0.0), pos2(u1, 1.0)),
                tint,
            );
        }
    }
}

/// An interactive area's accessible name when it has no label: its tooltip, else its
/// key ("rail-mods" -> "rail mods").
pub(crate) fn a11y_fallback(key: &str, tip: &str) -> String {
    if tip.is_empty() {
        key.replace(['-', '_'], " ")
    } else {
        tip.to_string()
    }
}

impl Kit<'_> {
    fn sid(&self, key: &str) -> Id {
        Id::new(("style", key))
    }

    /// A hover/click area with an animated hover amount (0..1) and an egui tooltip.
    pub fn hot(&mut self, key: &str, r: Rect, enabled: bool, tip: &str) -> (Resp, f32, bool) {
        let id = self.sid(key);
        let name = self.a11y_name.take();
        let mut out = Resp::default();
        let mut pressed = false;
        if !self.blocked {
            let sense = if enabled {
                Sense::click()
            } else {
                Sense::hover()
            };
            let mut resp = self.ui.interact(r, id, sense);
            let name = name.unwrap_or_else(|| a11y_fallback(key, tip));
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name.clone())
            });
            if !tip.is_empty() {
                resp = resp.on_hover_text(tip);
            }
            out.hovered = resp.hovered();
            if enabled {
                out.clicked = resp.clicked();
                pressed = resp.is_pointer_button_down_on();
                if out.hovered {
                    self.ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
            }
        }
        let t =
            self.ui
                .ctx()
                .animate_bool_with_time(id.with("hover"), out.hovered && enabled, ANIM);
        (out, t, pressed)
    }

    /// How far the pointer is over `r` (0..1, animated), without taking clicks.
    pub fn hover_t(&self, key: &str, r: Rect) -> f32 {
        let over = !self.blocked && self.ui.rect_contains_pointer(r);
        self.ui
            .ctx()
            .animate_bool_with_time(self.sid(key).with("hover"), over, ANIM)
    }

    pub fn text_width(&self, text: &str, font: egui::FontId) -> f32 {
        self.ui.ctx().fonts_mut(|f| {
            f.layout_no_wrap(text.to_string(), font, Color32::WHITE)
                .size()
                .x
        })
    }
}

/// Vector icons in an `s`-sized box at `o`.
pub fn icon_at(p: &egui::Painter, icon: Icon, o: Pos2, s: f32, c: Color32) {
    let st = Stroke::new((s / 11.0).max(1.5), c);
    let pt = |fx: f32, fy: f32| o + vec2(s * fx, s * fy);
    let line = |pts: Vec<Pos2>| {
        p.add(Shape::line(pts, st));
    };
    match icon {
        Icon::Download => {
            line(vec![pt(0.5, 0.1), pt(0.5, 0.64)]);
            line(vec![pt(0.26, 0.42), pt(0.5, 0.66), pt(0.74, 0.42)]);
            line(vec![
                pt(0.14, 0.72),
                pt(0.14, 0.88),
                pt(0.86, 0.88),
                pt(0.86, 0.72),
            ]);
        }
        Icon::Mods => {
            for (fx, fy) in [(0.1, 0.1), (0.56, 0.1), (0.1, 0.56), (0.56, 0.56)] {
                let rr = Rect::from_min_size(pt(fx, fy), vec2(s * 0.34, s * 0.34));
                if (fx, fy) == (0.56, 0.1) {
                    p.rect_filled(rr, 2.0, c);
                } else {
                    p.rect_stroke(rr, 2.0, st, StrokeKind::Inside);
                }
            }
        }
        Icon::Globe => {
            let cen = pt(0.5, 0.5);
            p.circle_stroke(cen, s * 0.4, st);
            line(vec![pt(0.1, 0.5), pt(0.9, 0.5)]);
            p.add(Shape::ellipse_stroke(cen, vec2(s * 0.16, s * 0.4), st));
        }
        Icon::Gear => {
            let cen = pt(0.5, 0.5);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment(
                    [cen + d * s * 0.3, cen + d * s * 0.46],
                    Stroke::new(s / 7.0, c),
                );
            }
            p.circle_stroke(cen, s * 0.28, st);
            p.circle_stroke(cen, s * 0.1, st);
        }
        Icon::Folder => {
            line(vec![
                pt(0.1, 0.84),
                pt(0.1, 0.18),
                pt(0.4, 0.18),
                pt(0.5, 0.3),
                pt(0.9, 0.3),
                pt(0.9, 0.84),
                pt(0.1, 0.84),
            ]);
        }
        Icon::Refresh => {
            let cen = pt(0.5, 0.5);
            let pts: Vec<Pos2> = (0..=20)
                .map(|k| {
                    let a = -0.3 + k as f32 / 20.0 * 4.9;
                    cen + vec2(a.cos(), a.sin()) * s * 0.36
                })
                .collect();
            let end = *pts.last().expect("points");
            line(pts);
            line(vec![
                end + vec2(-s * 0.2, -s * 0.02),
                end,
                end + vec2(-s * 0.02, s * 0.2),
            ]);
        }
        Icon::Warning => {
            p.add(Shape::convex_polygon(
                vec![pt(0.5, 0.08), pt(0.95, 0.9), pt(0.05, 0.9)],
                Color32::TRANSPARENT,
                st,
            ));
            line(vec![pt(0.5, 0.36), pt(0.5, 0.62)]);
            p.circle_filled(pt(0.5, 0.76), s / 16.0, c);
        }
        Icon::Headset => {
            let rr = Rect::from_min_max(pt(0.06, 0.3), pt(0.94, 0.76));
            p.rect_stroke(rr, s * 0.14, st, StrokeKind::Middle);
            line(vec![pt(0.38, 0.76), pt(0.5, 0.62), pt(0.62, 0.76)]);
            line(vec![
                pt(0.2, 0.3),
                pt(0.3, 0.12),
                pt(0.7, 0.12),
                pt(0.8, 0.3),
            ]);
        }
        Icon::ChevronDown => line(vec![pt(0.15, 0.32), pt(0.5, 0.68), pt(0.85, 0.32)]),
        Icon::Close => {
            line(vec![pt(0.2, 0.2), pt(0.8, 0.8)]);
            line(vec![pt(0.8, 0.2), pt(0.2, 0.8)]);
        }
        Icon::Copy => {
            let back = Rect::from_min_max(pt(0.14, 0.1), pt(0.62, 0.62));
            let front = Rect::from_min_max(pt(0.38, 0.36), pt(0.86, 0.9));
            p.rect_stroke(back, s * 0.06, st, StrokeKind::Middle);
            p.rect_stroke(front, s * 0.06, st, StrokeKind::Middle);
        }
        Icon::Info => {
            p.circle_stroke(pt(0.5, 0.5), s * 0.42, st);
            line(vec![pt(0.5, 0.44), pt(0.5, 0.74)]);
            p.circle_filled(pt(0.5, 0.29), s / 16.0, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing() {
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 0.0), Color32::BLACK);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 1.0), Color32::WHITE);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 2.0), Color32::WHITE);
    }
}
