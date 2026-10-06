//! Controls for settings the launcher draws from a description (a plugin's settings): a
//! switch, a slider, segmented buttons, radio buttons, a colour swatch with its field, a
//! key capture box and a multi-line field. They speak the widgets' language
//! (`widgets.rs`): the same fills, rims, caps and hover, in logical pixels from the kit's
//! origin.

use egui::{pos2, vec2, Color32, CursorIcon, Id, Rect, Sense, Stroke, StrokeKind};

use super::design::{self};
use super::kit::Kit;
use super::style::mix;
use super::widgets::{disabled, fg, Tone, R};

/// A switch's track.
pub const SWITCH_W: f32 = 38.0;
const SWITCH_H: f32 = 20.0;
/// The controls' row height (a check's).
const ROW: f32 = 24.0;

fn id(key: &str) -> Id {
    Id::new(("controls", key))
}

impl Kit<'_> {
    /// A switch, `SWITCH_W` wide and centred in a 24-high row at `y`: blue with its knob on
    /// the right when on. Returns true when it was flipped.
    pub fn switch(
        &mut self,
        key: &str,
        on: &mut bool,
        label: &str,
        x: f32,
        y: f32,
        enabled: bool,
        tip: &str,
    ) -> bool {
        let r = self.rect(x, y + (ROW - SWITCH_H) / 2.0, SWITCH_W, SWITCH_H);
        self.a11y_name = Some(label.to_string());
        let (resp, t, pressed) = self.hot(key, r.expand(3.0), enabled, tip);
        if resp.clicked {
            *on = !*on;
        }
        let pos = self
            .ui
            .ctx()
            .animate_bool_with_time(id(key).with("pos"), *on, 0.12);
        let off_fill = Color32::from_rgba_unmultiplied(20, 20, 20, 225);
        let fill = mix(off_fill, design::BLUE, pos);
        let p = self.ui.painter();
        let round = SWITCH_H / 2.0;
        p.rect_filled(r, round, if enabled { fill } else { disabled(fill) });
        if pos < 0.99 {
            p.rect_stroke(
                r,
                round,
                Stroke::new(
                    1.0,
                    Color32::from_white_alpha(((70.0 + 80.0 * t) * (1.0 - pos)) as u8),
                ),
                StrokeKind::Inside,
            );
        }
        let knob = SWITCH_H - 6.0;
        let kx = r.min.x + 3.0 + (r.width() - 6.0 - knob) * pos;
        let kc = pos2(kx + knob / 2.0, r.center().y);
        p.circle_filled(kc, knob / 2.0, fg(enabled));
        if pressed {
            p.rect_filled(r, round, Color32::from_black_alpha(40));
        }
        resp.clicked
    }

    /// A radio button with its label (as a check, round): returns true when it was picked.
    #[allow(clippy::too_many_arguments)]
    pub fn radio(
        &mut self,
        key: &str,
        on: bool,
        label: &str,
        x: f32,
        y: f32,
        enabled: bool,
        tip: &str,
    ) -> bool {
        let s = 16.0;
        let g = self.label_galley(label, design::din(16.0), fg(enabled), f32::INFINITY);
        let r = self.rect(x, y, s + 10.0 + g.size().x, ROW);
        self.a11y_name = Some(label.to_string());
        let (resp, t, _) = self.hot(key, r, enabled, tip);
        let c = pos2(r.min.x + s / 2.0, r.center().y);
        let p = self.ui.painter();
        p.circle_filled(c, s / 2.0, Color32::from_rgba_unmultiplied(20, 20, 20, 225));
        p.circle_stroke(
            c,
            s / 2.0 - 0.5,
            Stroke::new(
                1.0,
                if on {
                    design::BLUE
                } else {
                    Color32::from_white_alpha((70.0 + 80.0 * t) as u8)
                },
            ),
        );
        if on {
            let dot = if enabled {
                design::BLUE
            } else {
                disabled(design::BLUE)
            };
            p.circle_filled(c, s / 2.0 - 4.0, dot);
        }
        let ty = r.center().y - g.size().y / 2.0;
        p.galley(pos2(r.min.x + s + 10.0, ty), g, fg(enabled));
        resp.clicked && !on
    }

    /// How wide [`Kit::segmented`] is with `labels` at height `h`.
    pub fn segmented_width(&self, labels: &[&str], h: f32) -> f32 {
        labels
            .iter()
            .map(|l| self.button_width(l, None, h).max(h * 2.0))
            .sum::<f32>()
            + 2.0 * labels.len().saturating_sub(1) as f32
    }

    /// Buttons side by side, the `selected` one blue, each with its tooltip in `tips`:
    /// returns the one clicked (several may be blue, so a blue one may be clicked too).
    #[allow(clippy::too_many_arguments)]
    pub fn segmented(
        &mut self,
        key: &str,
        labels: &[&str],
        selected: &[bool],
        x: f32,
        y: f32,
        h: f32,
        enabled: bool,
        tips: &[&str],
    ) -> Option<usize> {
        let mut cx = x;
        let mut picked = None;
        for (i, label) in labels.iter().enumerate() {
            let w = self.button_width(label, None, h).max(h * 2.0);
            let on = selected.get(i).copied().unwrap_or(false);
            let tone = if on { Tone::Blue } else { Tone::Dark };
            if self
                .button(
                    &format!("{key}-{i}"),
                    cx,
                    y,
                    w,
                    h,
                    tone,
                    None,
                    label,
                    enabled,
                    tips.get(i).copied().unwrap_or_default(),
                )
                .clicked
            {
                picked = Some(i);
            }
            cx += w + 2.0;
        }
        picked
    }

    /// A slider from `min` to `max` (`value` moves while dragged, on the step's grid):
    /// returns true when a change is done (let go, a click, an arrow key while hovered).
    #[allow(clippy::too_many_arguments)]
    pub fn slider(
        &mut self,
        key: &str,
        value: &mut f64,
        (min, max, step): (f64, f64, Option<f64>),
        label: &str,
        x: f32,
        y: f32,
        w: f32,
        enabled: bool,
        tip: &str,
    ) -> bool {
        let r = self.rect(x, y, w, ROW);
        let knob = 14.0;
        let track = Rect::from_min_max(
            pos2(r.min.x + knob / 2.0, r.center().y - 2.0),
            pos2(r.max.x - knob / 2.0, r.center().y + 2.0),
        );
        let span = (max - min).max(f64::EPSILON);
        let snap = |v: f64| -> f64 {
            let v = match step.filter(|s| *s > 0.0) {
                Some(s) => min + ((v - min) / s).round() * s,
                None => v,
            };
            v.clamp(min, max)
        };
        let mut done = false;
        let mut hovered = false;
        if !self.blocked {
            let sense = if enabled {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            };
            let mut resp = self.ui.interact(r, id(key), sense);
            resp.widget_info(|| egui::WidgetInfo::slider(enabled, *value, label.to_string()));
            if !tip.is_empty() {
                resp = resp.on_hover_text(tip);
            }
            hovered = resp.hovered();
            if enabled {
                if hovered || resp.dragged() {
                    self.ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if let Some(p) = resp.interact_pointer_pos() {
                    if resp.dragged() || resp.clicked() || resp.drag_started() {
                        let f = ((p.x - track.min.x) / track.width()).clamp(0.0, 1.0) as f64;
                        *value = snap(min + f * span);
                    }
                }
                if resp.drag_stopped() || resp.clicked() {
                    done = true;
                }
                if hovered {
                    let nudge = step.unwrap_or(span / 100.0);
                    let (left, right) = self.ui.input(|i| {
                        (
                            i.key_pressed(egui::Key::ArrowLeft),
                            i.key_pressed(egui::Key::ArrowRight),
                        )
                    });
                    if left || right {
                        *value = snap(*value + if right { nudge } else { -nudge });
                        done = true;
                    }
                }
            }
        }
        let t =
            self.ui
                .ctx()
                .animate_bool_with_time(id(key).with("hover"), hovered && enabled, 0.12);
        let f = ((*value - min) / span).clamp(0.0, 1.0) as f32;
        let kx = track.min.x + track.width() * f;
        let p = self.ui.painter();
        p.rect_filled(track, 2.0, Color32::from_rgba_unmultiplied(20, 20, 20, 225));
        let blue = if enabled {
            design::BLUE
        } else {
            disabled(design::BLUE)
        };
        p.rect_filled(
            Rect::from_min_max(track.min, pos2(kx, track.max.y)),
            2.0,
            blue,
        );
        let c = pos2(kx, r.center().y);
        p.circle_filled(c, knob / 2.0 + 1.5 * t, fg(enabled));
        p.circle_stroke(c, knob / 2.0 + 1.5 * t, Stroke::new(1.0, blue));
        done
    }

    /// A colour swatch (`#rrggbb` text, shown when it reads).
    pub fn swatch(&self, x: f32, y: f32, s: f32, hex: &str) {
        let r = self.rect(x, y, s, s);
        let p = self.ui.painter();
        p.rect_filled(r, R, Color32::from_rgba_unmultiplied(20, 20, 20, 225));
        if let Some(c) = parse_hex(hex) {
            p.rect_filled(r.shrink(3.0), R, c);
        }
        p.rect_stroke(
            r,
            R,
            Stroke::new(1.0, Color32::from_white_alpha(40)),
            StrokeKind::Inside,
        );
    }

    /// A box that takes the next key pressed while it is focused (click it first):
    /// returns it as `Ctrl+Alt+C` (`modifiers`: with Ctrl, Alt, Shift), or `Some("")`
    /// for Delete or Backspace. Escape lets go.
    #[allow(clippy::too_many_arguments)]
    pub fn key_capture(
        &mut self,
        key: &str,
        current: &str,
        modifiers: bool,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        enabled: bool,
        tip: &str,
    ) -> Option<String> {
        let r = self.rect(x, y, w, h);
        let listening_id = id(key).with("listening");
        let ctx = self.ui.ctx().clone();
        let mut listening = ctx.data(|d| d.get_temp::<bool>(listening_id).unwrap_or(false));
        self.a11y_name = Some(current.to_string());
        let (resp, t, _) = self.hot(key, r, enabled, tip);
        if resp.clicked {
            listening = !listening;
        }
        let mut out = None;
        if listening && enabled && !self.blocked {
            let events = ctx.input(|i| i.events.clone());
            for e in events {
                if let egui::Event::Key {
                    key: k,
                    pressed: true,
                    modifiers: m,
                    ..
                } = e
                {
                    match k {
                        egui::Key::Escape => {}
                        egui::Key::Backspace | egui::Key::Delete => out = Some(String::new()),
                        _ => {
                            let mut parts = Vec::new();
                            if modifiers {
                                if m.ctrl || m.command && !cfg!(target_os = "macos") {
                                    parts.push("Ctrl");
                                }
                                if m.alt {
                                    parts.push("Alt");
                                }
                                if m.shift {
                                    parts.push("Shift");
                                }
                            }
                            let mut s = parts.join("+");
                            if !s.is_empty() {
                                s.push('+');
                            }
                            s.push_str(k.name());
                            out = Some(s);
                        }
                    }
                    listening = false;
                    break;
                }
            }
            // A click elsewhere lets go too.
            let away = ctx.input(|i| {
                i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| !r.contains(p))
            });
            if away {
                listening = false;
            }
        }
        ctx.data_mut(|d| d.insert_temp(listening_id, listening));
        let p = self.ui.painter();
        p.rect_filled(r, R, Color32::from_rgba_unmultiplied(10, 4, 22, 215));
        if listening {
            self.gradient_frame(r, R, 1.5, design::RIM_TOP, design::RIM_BOTTOM);
        } else {
            self.ui.painter().rect_stroke(
                r,
                R,
                Stroke::new(1.0, Color32::from_white_alpha((40.0 + 40.0 * t) as u8)),
                StrokeKind::Inside,
            );
        }
        let (text, color) = if listening {
            ("Press a key", design::SUBTLE)
        } else if current.is_empty() {
            ("Not set", design::SUBTLE)
        } else {
            (current, fg(enabled))
        };
        let g = self.label_galley(text, design::din(16.0), color, r.width() - 20.0);
        self.ui.painter().galley(
            pos2(r.min.x + 10.0, r.center().y - g.size().y / 2.0),
            g,
            color,
        );
        out
    }

    /// A text field over several lines, as [`Kit::field`]: returns true when editing
    /// ended.
    #[allow(clippy::too_many_arguments)]
    pub fn area(
        &mut self,
        key: &str,
        text: &mut String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        invalid: bool,
        tip: &str,
    ) -> bool {
        let r = self.rect(x, y, w, h);
        let fid = id(key);
        let focused = self.ui.memory(|m| m.has_focus(fid));
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
        let edit = egui::TextEdit::multiline(text)
            .id(fid)
            .font(design::myriad(20.0))
            .text_color(design::TEXT)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::same(6))
            .desired_width(w - 20.0)
            .interactive(!self.blocked);
        let resp = self.ui.put(r.shrink2(vec2(4.0, 2.0)), edit);
        let resp = if tip.is_empty() || self.blocked {
            resp
        } else {
            resp.on_hover_text(tip)
        };
        resp.lost_focus()
    }
}

/// `#rrggbb` or `#rrggbbaa` as a colour.
pub fn parse_hex(hex: &str) -> Option<Color32> {
    let h = hex.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(
            byte(0)?,
            byte(2)?,
            byte(4)?,
            byte(6)?,
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hex_colours() {
        assert_eq!(parse_hex("#ff8000"), Some(Color32::from_rgb(255, 128, 0)));
        assert_eq!(
            parse_hex("00000080"),
            Some(Color32::from_rgba_unmultiplied(0, 0, 0, 128))
        );
        assert_eq!(parse_hex("#ff80"), None);
        assert_eq!(parse_hex("#gg0000"), None);
    }
}
