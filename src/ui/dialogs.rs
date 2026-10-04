//! Modal dialogs, drawn in the launcher window as cards over the dimmed page: errors (with
//! help links and a copy button), message/confirm/option boxes and the device picker.
//!
//! Screens queue a dialog under a string key and later `take` the answer. While any dialog
//! is open, the page underneath stops reacting (`is_open`). Enter picks the first button,
//! Escape closes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::Galley;

use super::design::{self, dz};
use super::kit::Kit;
use super::style::{self, Icon as Glyph};
use super::widgets::{Tone, BTN_H};
use crate::core::adb::devices::Device;
use crate::core::error::{HelpLink, UiError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Info,
    Warning,
    Question,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Index of the pressed button (0 = Yes / first option), or the picked device.
    Button(usize),
    /// Closed with Escape (or Cancel in the device picker).
    Closed,
}

impl Answer {
    pub fn is_yes(&self) -> bool {
        *self == Answer::Button(0)
    }
}

enum Kind {
    Error(HelpLink),
    /// The icon, the buttons, and whether the first one deletes something.
    Message(Icon, Vec<String>, bool),
    Picker(Vec<Device>),
    /// Waiting on the player in their browser, at this page.
    Browser(String),
}

struct Dialog {
    key: &'static str,
    title: String,
    message: String,
    kind: Kind,
    scroll: f32,
    copied: Option<Instant>,
}

#[derive(Default)]
pub struct DialogHost {
    stack: Vec<Dialog>,
    answers: Vec<(&'static str, Answer)>,
}

/// Card width, padding and the body's text size (design pixels).
const W: f32 = 560.0;
/// The browser dialog's card: three buttons in a row.
const W_BROWSER: f32 = 720.0;
const PAD: f32 = 26.0;
const BODY: f32 = 17.0;
const ICON: f32 = 34.0;

pub(crate) const DEV_MODE_URL: &str =
    "https://learn.adafruit.com/sideloading-on-oculus-quest/enable-developer-mode";
pub(crate) const USB_DEBUGGING_URL: &str =
    "https://developers.meta.com/horizon/documentation/native/android/mobile-device-setup/";

impl DialogHost {
    pub fn is_open(&self) -> bool {
        !self.stack.is_empty()
    }

    fn push(&mut self, key: &'static str, title: &str, message: &str, kind: Kind) {
        self.stack.push(Dialog {
            key,
            title: title.into(),
            message: message.into(),
            kind,
            scroll: 0.0,
            copied: None,
        });
    }

    /// An error with an optional help link.
    pub fn error(&mut self, title: &str, message: &str, link: HelpLink) {
        self.push("", title, message, Kind::Error(link));
    }

    pub fn error_ui(&mut self, e: &UiError) {
        self.error(&e.title, &e.message, e.link);
    }

    pub fn message(&mut self, key: &'static str, title: &str, message: &str, icon: Icon) {
        self.push(
            key,
            title,
            message,
            Kind::Message(icon, vec!["OK".into()], false),
        );
    }

    pub fn info(&mut self, title: &str, message: &str) {
        self.message("", title, message, Icon::Info);
    }

    /// Yes/No; `Answer::Button(0)` is Yes.
    pub fn confirm(&mut self, key: &'static str, title: &str, message: &str, icon: Icon) {
        self.options(key, title, message, icon, &["Yes", "No"]);
    }

    /// A question whose first answer deletes something (a red button).
    pub fn confirm_danger(&mut self, key: &'static str, title: &str, message: &str, yes: &str) {
        self.push(
            key,
            title,
            message,
            Kind::Message(Icon::Warning, vec![yes.into(), "Cancel".into()], true),
        );
    }

    pub fn options(
        &mut self,
        key: &'static str,
        title: &str,
        message: &str,
        icon: Icon,
        buttons: &[&str],
    ) {
        self.push(
            key,
            title,
            message,
            Kind::Message(icon, buttons.iter().map(|b| b.to_string()).collect(), false),
        );
    }

    /// "Which one is your Quest?" -- `Answer::Button(i)` is `devices[i]`.
    pub fn device_picker(&mut self, key: &'static str, devices: Vec<Device>) {
        self.push(
            key,
            "Which one is your Quest?",
            "More than one device is plugged in.",
            Kind::Picker(devices),
        );
    }

    /// Something to finish in the browser, at `url` (just opened there). Open again and
    /// Copy link keep it up; Cancel or Escape close it with `Answer::Closed`. The screen
    /// takes it away with [`DialogHost::dismiss`] once the wait is over.
    pub fn browser(&mut self, key: &'static str, title: &str, message: &str, url: &str) {
        self.dismiss(key);
        self.push(key, title, message, Kind::Browser(url.to_string()));
    }

    /// Closes the dialog under `key`, if it is up, without an answer.
    pub fn dismiss(&mut self, key: &'static str) {
        self.stack.retain(|d| d.key != key);
    }

    pub fn take(&mut self, key: &'static str) -> Option<Answer> {
        let i = self.answers.iter().position(|(k, _)| *k == key)?;
        Some(self.answers.remove(i).1)
    }

    /// Draws the top-most dialog over the window.
    pub fn show(&mut self, kit: &mut Kit) {
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        let answer = kit.modal("dialog", false, |k| draw(k, top));
        let keys = kit.ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        let answer = answer.or(match (&top.kind, keys) {
            (_, (_, true)) => Some(Answer::Closed),
            (Kind::Error(_) | Kind::Message(..), (true, _)) => Some(Answer::Button(0)),
            _ => None,
        });
        if let Some(a) = answer {
            let d = self.stack.pop().expect("top exists");
            if !d.key.is_empty() {
                self.answers.push((d.key, a));
            }
        }
    }
}

/// The body text, one galley per paragraph.
fn body(k: &Kit, text: &str, w: f32) -> Vec<Arc<Galley>> {
    k.caps_block(text, BODY, design::BODY, w)
}

fn draw(k: &mut Kit, d: &mut Dialog) -> Option<Answer> {
    let w = dz(if matches!(d.kind, Kind::Browser(_)) {
        W_BROWSER
    } else {
        W
    });
    let pad = dz(PAD);
    let text_w = w - 2.0 * pad;
    let paras = body(k, &d.message, text_w);
    let gap = dz(12.0);
    let text_h: f32 =
        paras.iter().map(|g| g.size().y).sum::<f32>() + gap * paras.len().saturating_sub(1) as f32;
    let link = match &d.kind {
        Kind::Error(HelpLink::DeveloperMode) => {
            Some(("How to enable Developer Mode on your Quest", DEV_MODE_URL))
        }
        Kind::Error(HelpLink::UsbDebugging) => Some((
            "How to allow USB debugging on your Quest",
            USB_DEBUGGING_URL,
        )),
        _ => None,
    };
    let devices = match &d.kind {
        Kind::Picker(devs) => devs.len().min(5),
        _ => 0,
    };
    // The title wraps under itself when it is long.
    let title_w = w - 2.0 * pad - dz(ICON) - dz(16.0);
    let tg = {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &d.title.to_uppercase(),
            0.0,
            egui::TextFormat {
                font_id: design::conthrax(21.0),
                color: design::TEXT,
                extra_letter_spacing: dz(2.5),
                ..Default::default()
            },
        );
        job.wrap.max_width = title_w;
        job.wrap.max_rows = 2;
        k.ui.ctx().fonts_mut(|f| f.layout_job(job))
    };
    let head = tg.size().y.max(dz(ICON)) + dz(20.0);
    let list_h = devices as f32 * (BTN_H + 8.0);
    let link_h = if link.is_some() { dz(40.0) } else { 0.0 };
    let max_text = (super::launcher::H + k.ey) * 0.6 - head - list_h;
    let shown_h = text_h.min(max_text);
    let h = pad + head + shown_h + link_h + list_h + dz(26.0) + BTN_H + pad;
    let x = ((super::launcher::W + k.ex - w) / 2.0).round();
    let y = ((super::launcher::H + k.ey - h) / 2.0).round();
    k.solid_panel(x, y, w, h);

    // Header: the icon on a tinted square and the title.
    let (glyph, color) = match &d.kind {
        Kind::Error(_) | Kind::Message(Icon::Warning, ..) => (Glyph::Warning, design::QUEST_WARN),
        Kind::Picker(_) => (Glyph::Headset, design::BLUE),
        Kind::Message(..) | Kind::Browser(_) => (Glyph::Info, design::BLUE),
    };
    let ib = k.rect(x + pad, y + pad, dz(ICON), dz(ICON));
    k.ui.painter().rect_filled(ib, dz(4.0), color);
    let is = dz(ICON) * 0.62;
    style::icon_at(
        k.ui.painter(),
        glyph,
        ib.center() - egui::vec2(is, is) / 2.0,
        is,
        design::TEXT,
    );
    let title_x = x + pad + dz(ICON) + dz(16.0);
    let ty = y + pad + (dz(ICON) - tg.size().y).max(0.0) / 2.0;
    k.put(title_x, ty, tg);

    // The message, scrolling when it is long.
    let ty = y + pad + head;
    k.scroll_area(
        "dialog-text",
        x + pad,
        ty,
        text_w + pad / 2.0,
        shown_h,
        text_h,
        &mut d.scroll,
    );
    let scroll = d.scroll;
    k.clipped(x + pad, ty, text_w, shown_h, |k| {
        let mut gy = ty - scroll;
        for g in paras {
            gy += k.put(x + pad, gy, g).height() + gap;
        }
    });

    let mut by = ty + shown_h;
    if let Some((label, url)) = link {
        if k.link("dialog-help", x + pad, by + dz(16.0), label, 16.0, url)
            .clicked
        {
            crate::core::platform::open_url(url);
        }
        by += link_h;
    }
    if let Kind::Picker(devs) = &d.kind {
        let mut dy = by + dz(10.0);
        for (i, dev) in devs.iter().take(5).enumerate() {
            let key = format!("dialog-device-{i}");
            if k.button(
                &key,
                x + pad,
                dy,
                text_w,
                BTN_H,
                Tone::Dark,
                Some(Glyph::Headset),
                &dev.label(),
                true,
                "",
            )
            .clicked
            {
                return Some(Answer::Button(i));
            }
            dy += BTN_H + 8.0;
        }
        by = dy - 8.0;
    }

    // Buttons, right-aligned; errors can copy their text.
    let by = by + dz(26.0);
    let right = x + w - pad;
    match &d.kind {
        Kind::Error(_) => {
            let copied = d
                .copied
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
            let label = if copied { "Copied" } else { "Copy" };
            let cw = k.button_width("Copied", Some(Glyph::Copy), BTN_H);
            let tip = "Copy this message, to paste it when you ask for help";
            if k.button(
                "dialog-copy",
                x + pad,
                by,
                cw,
                BTN_H,
                Tone::Dark,
                Some(Glyph::Copy),
                label,
                true,
                tip,
            )
            .clicked
            {
                let text = format!("{}\n\n{}", d.title, d.message.trim_end());
                if let Ok(mut c) = arboard::Clipboard::new() {
                    let _ = c.set_text(text);
                    d.copied = Some(Instant::now());
                }
            }
            if copied {
                k.ui.ctx().request_repaint_after(Duration::from_millis(200));
            }
            k.button_row("dialog-btn", right, by, &["Close"], Tone::Blue)
                .map(Answer::Button)
        }
        Kind::Message(_, buttons, danger) => {
            let labels: Vec<&str> = buttons.iter().map(String::as_str).collect();
            let tone = if *danger { Tone::Danger } else { Tone::Go };
            k.button_row("dialog-btn", right, by, &labels, tone)
                .map(Answer::Button)
        }
        Kind::Picker(_) => k
            .button_row("dialog-btn", right, by, &["Cancel"], Tone::Dark)
            .map(|_| Answer::Closed),
        Kind::Browser(url) => {
            // Open again (the way on), Copy link beside it, Cancel on the right.
            let ow = k.button_width("Open again", Some(Glyph::Globe), BTN_H);
            if k.button(
                "dialog-open",
                x + pad,
                by,
                ow,
                BTN_H,
                Tone::Blue,
                Some(Glyph::Globe),
                "Open again",
                true,
                "Open the page in your browser again",
            )
            .clicked
            {
                crate::core::platform::open_url(url);
            }
            let copied = d
                .copied
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
            let label = if copied { "Copied" } else { "Copy link" };
            let cw = k.button_width("Copy link", Some(Glyph::Copy), BTN_H);
            if k.button(
                "dialog-copy",
                x + pad + ow + dz(12.0),
                by,
                cw,
                BTN_H,
                Tone::Dark,
                Some(Glyph::Copy),
                label,
                true,
                "Copy the page's address, to open it in a browser yourself",
            )
            .clicked
            {
                if let Ok(mut c) = arboard::Clipboard::new() {
                    let _ = c.set_text(url.clone());
                    d.copied = Some(Instant::now());
                }
            }
            if copied {
                k.ui.ctx().request_repaint_after(Duration::from_millis(200));
            }
            k.button_row("dialog-btn", right, by, &["Cancel"], Tone::Dark)
                .map(|_| Answer::Closed)
        }
    }
}
