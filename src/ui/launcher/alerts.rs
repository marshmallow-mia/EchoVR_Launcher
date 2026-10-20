//! Alerts ([`crate::core::launcher::alerts`]) in the window: the most important one in the
//! top bar (blue for info, orange for a warning) with More and ×, and a critical one as a
//! card that has to be acknowledged.

use super::{setup, Dashboard};
use crate::core::launcher::alerts::{Alert, Severity};
use crate::ui::design::{self, dz};
use crate::ui::dialogs::Icon as DlgIcon;
use crate::ui::kit::Kit;
use crate::ui::markdown;
use crate::ui::widgets::{Tone, BTN_H};

const MORE_KEY: &str = "alert-more";

/// The alert the top bar shows: the most important one (critical ones too, once read).
fn shown(d: &Dashboard) -> Option<&Alert> {
    d.alerts.iter().max_by_key(|a| a.severity())
}

/// Draws the top bar's alert into `x..right` (logical, the bar's text area), centred on
/// `cy`; false when there is none.
pub(super) fn strip(d: &mut Dashboard, kit: &mut Kit, x: f32, right: f32, cy: f32) -> bool {
    let Some(a) = shown(d).cloned() else {
        return false;
    };
    if right - x < dz(160.0) {
        return false;
    }
    let color = match a.severity() {
        Severity::Info => design::BLUE,
        _ => design::QUEST_WARN,
    };
    let h = dz(29.0);
    let r = egui::Rect::from_min_max(egui::pos2(x, cy - h / 2.0), egui::pos2(right, cy + h / 2.0));
    let r = r.translate(kit.origin.to_vec2());
    kit.ui.painter().rect_filled(r, dz(4.0), color);
    // Right: ×, then MORE.
    let mut end = right - dz(10.0);
    if a.can_dismiss() {
        let g = kit.spaced_galley("×", design::din(16.0), design::TEXT, 0.0, false);
        let gx = end - g.size().x;
        let gr = kit.put(gx, cy - g.size().y / 2.0, g);
        if kit
            .hot("alert-dismiss", gr.expand(dz(6.0)), true, "Dismiss it")
            .0
            .clicked
        {
            d.state.alerts_seen.insert(a.id.clone());
            d.save();
            d.alerts.retain(|x| x.id != a.id);
            return true;
        }
        end = gx - dz(14.0);
    }
    let more = kit.spaced_galley("MORE", design::din(12.5), design::TEXT, dz(0.5), true);
    let mx = end - more.size().x;
    let mr = kit.put(mx, cy - more.size().y / 2.0, more);
    let clicked_more = kit
        .hot("alert-more", mr.expand(dz(6.0)), true, "Read all of it")
        .0
        .clicked;
    let title = kit.spaced_fit(
        &a.title.to_uppercase(),
        design::din(13.0),
        design::TEXT,
        dz(0.5),
        false,
        mx - dz(14.0) - (x + dz(12.0)),
    );
    let ty = cy - title.size().y / 2.0;
    let tr = kit.put(x + dz(12.0), ty, title);
    let clicked_title = kit.hot("alert-title", tr, true, "Read all of it").0.clicked;
    if clicked_more || clicked_title {
        let buttons: &[&str] = if a.link().is_some() {
            &["Open link", "Close"]
        } else {
            &["Close"]
        };
        let text = format!(
            "{}\n\n{}",
            crate::core::launcher::alerts::plain(&a.body),
            a.link().map(|(_, u)| u).unwrap_or("")
        );
        d.alert_open = Some(a.clone());
        d.dialogs
            .options(MORE_KEY, &a.title, text.trim(), DlgIcon::Warning, buttons);
    }
    true
}

/// The answer to an alert's More dialog: its link.
pub(super) fn answers(d: &mut Dashboard) {
    if let Some(answer) = d.dialogs.take(MORE_KEY) {
        if let (Some(a), true) = (d.alert_open.take(), answer.is_yes()) {
            if let Some((_, url)) = a.link() {
                crate::core::platform::open_url(url);
            }
        }
    }
}

/// A critical alert's card: its text, its link, and "I've read it" (no Escape).
pub(super) fn card(d: &mut Dashboard, k: &mut Kit) {
    let Some(setup::Overlay::Alert(a)) = &d.overlay else {
        return;
    };
    let a = (**a).clone();
    let (w, h) = (dz(900.0), dz(560.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, &a.title);
    let look = markdown::Look {
        font: design::din(17.0),
        bold: design::din(17.0),
        mono: egui::FontId::monospace(dz(15.0)),
        small: design::din(14.5),
        color: design::BODY,
        strong: design::TEXT,
        subtle: design::GREY,
        link: design::TEXT,
        chip: egui::Color32::from_white_alpha(18),
        chip_rim: egui::Color32::TRANSPARENT,
        line_gap: 0.0,
        paragraph_gap: dz(16.0),
        bullet_indent: dz(16.0),
        uppercase: true,
        dash_bullets: true,
    };
    let by = bottom - BTN_H;
    k.clipped(x, y, cw, by - dz(16.0) - y, |k| {
        markdown::draw(k, x, y, cw, &a.body, &look)
    });
    let read_w = k.button_width("I've read it", None, BTN_H).max(170.0);
    if k.button(
        "alert-read",
        x + cw - read_w,
        by,
        read_w,
        BTN_H,
        Tone::Go,
        None,
        "I've read it",
        true,
        "",
    )
    .clicked
    {
        d.alerts_read.insert(a.id.clone());
        if a.can_dismiss() {
            d.state.alerts_seen.insert(a.id.clone());
            d.save();
            d.alerts.retain(|x| x.id != a.id);
        }
        d.overlay = None;
        return;
    }
    if let Some((label, url)) = a.link() {
        let lw = k.button_width(label, None, BTN_H).max(140.0);
        if k.button(
            "alert-link",
            x + cw - read_w - dz(10.0) - lw,
            by,
            lw,
            BTN_H,
            Tone::Dark,
            None,
            label,
            true,
            url,
        )
        .clicked
        {
            crate::core::platform::open_url(url);
        }
    }
}
