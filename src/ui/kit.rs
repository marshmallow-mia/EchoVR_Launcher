//! Absolute positioning: a `Kit` draws into a `Ui` at logical pixels from its origin
//! (images, the ✓/✗ mark, clipping and wheel scrolling). The controls are in
//! `widgets.rs`, the design's parts in `design.rs`.

use egui::{pos2, vec2, Color32, Pos2, Rect, Shape, Stroke, TextureHandle, Ui};

use super::assets::Assets;
use super::launcher::{H, W};

pub struct Kit<'a> {
    pub ui: &'a mut Ui,
    pub assets: &'a Assets,
    pub origin: Pos2,
    /// A modal dialog/window is open on top: draw, but don't react.
    pub blocked: bool,
    /// Drawing a page nobody sees, only to warm its images (`Dashboard::prewarm`): no
    /// side effects.
    pub ghost: bool,
    /// The window's room beyond the design's `W`×`H` (logical pixels; one of them is 0,
    /// as the launcher is scaled to fit).
    pub ex: f32,
    pub ey: f32,
    /// The name the next interactive area gets for accessibility tools (screen readers,
    /// AT-SPI); without one it is named after its tooltip, else its key.
    pub a11y_name: Option<String>,
}

impl<'a> Kit<'a> {
    pub fn new(ui: &'a mut Ui, assets: &'a Assets, blocked: bool) -> Self {
        let origin = ui.max_rect().min;
        let window = ui.ctx().viewport_rect().size();
        Kit {
            ui,
            assets,
            origin,
            blocked,
            ghost: false,
            ex: (window.x - W).max(0.0),
            ey: (window.y - H).max(0.0),
            a11y_name: None,
        }
    }

    /// Runs `f` with painting and interaction clipped to the rect (for scrolling lists).
    pub fn clipped<R>(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        f: impl FnOnce(&mut Kit) -> R,
    ) -> R {
        let saved = self.ui.clip_rect();
        let r = self.rect(x, y, w, h).intersect(saved);
        self.ui.set_clip_rect(r);
        let out = f(self);
        self.ui.set_clip_rect(saved);
        out
    }

    /// Mouse-wheel scrolling over a rect: updates `offset` within `0..=content_h - h`.
    pub fn scroll(&self, x: f32, y: f32, w: f32, h: f32, content_h: f32, offset: &mut f32) {
        let r = self.rect(x, y, w, h);
        if !self.blocked && self.ui.rect_contains_pointer(r) {
            *offset -= self.ui.input(|i| i.smooth_scroll_delta.y);
        }
        *offset = offset.clamp(0.0, (content_h - h).max(0.0));
    }

    /// Whether `key` was pressed for what this kit draws: not while a dialog over it has
    /// the keys (Escape there closes the dialog, not the card under it).
    pub fn key(&self, key: egui::Key) -> bool {
        !self.blocked && self.ui.input(|i| i.key_pressed(key))
    }

    pub fn ctx(&self) -> egui::Context {
        self.ui.ctx().clone()
    }

    /// The whole window.
    pub fn window(&self) -> Rect {
        self.rect(0.0, 0.0, W + self.ex, H + self.ey)
    }

    pub fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.origin + vec2(x, y), vec2(w, h))
    }

    fn painter(&self) -> &egui::Painter {
        self.ui.painter()
    }

    // ---- painting ----

    pub fn image(&self, name: &str, x: f32, y: f32, w: f32, h: f32) {
        let tex = self
            .assets
            .tex(self.ui.ctx(), name, w.round() as u32, h.round() as u32);
        self.paint_tex(&tex, self.rect(x, y, w, h), Color32::WHITE);
    }

    pub fn paint_tex(&self, tex: &TextureHandle, rect: Rect, tint: Color32) {
        self.painter().image(
            tex.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            tint,
        );
    }

    /// `markIcon`: vector ✓ / ✗ in a `size` square at (x, y).
    pub fn mark(&self, check: bool, color: Color32, size: f32, x: f32, y: f32) {
        let o = self.origin + vec2(x, y);
        let s = size;
        let stroke = Stroke::new((s / 7.0).max(2.0), color);
        let p = |fx: f32, fy: f32| o + vec2((s * fx).floor(), (s * fy).floor());
        if check {
            self.painter().add(Shape::line(
                vec![p(1.0 / 6.0, 0.5), p(0.4, 0.8), p(5.0 / 6.0, 0.2)],
                stroke,
            ));
            // Round the joint/caps like BasicStroke.CAP_ROUND.
            for q in [p(1.0 / 6.0, 0.5), p(0.4, 0.8), p(5.0 / 6.0, 0.2)] {
                self.painter().circle_filled(q, stroke.width / 2.0, color);
            }
        } else {
            for (a, b) in [(p(0.2, 0.2), p(0.8, 0.8)), (p(0.8, 0.2), p(0.2, 0.8))] {
                self.painter().line_segment([a, b], stroke);
                self.painter().circle_filled(a, stroke.width / 2.0, color);
                self.painter().circle_filled(b, stroke.width / 2.0, color);
            }
        }
    }
}
