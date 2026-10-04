//! The right-hand panel the Play and Settings pages draw at the window's right edge: its
//! frame (panel_bg.png with a violet rim) and title, its text look, and where its body and
//! footer go. Other pages put their cards in the same column with [`at_right`].

use egui::Color32;

use crate::ui::design::{self, dz, Dr};
use crate::ui::kit::Kit;
use crate::ui::markdown::Look;
use crate::ui::style;

pub(super) const PANEL: Dr = Dr::new(1318.0, 15.0, 555.0, 1031.0);
/// The panel's rim at the top and at the bottom (translucent violet, as the concept's).
const RIM_FAINT: egui::Color32 = egui::Color32::from_rgba_premultiplied(18, 4, 36, 60);
const RIM_GLOW: egui::Color32 = egui::Color32::from_rgba_premultiplied(150, 44, 235, 235);
/// The content column.
pub(super) const X: f32 = 1349.0;
const TITLE_Y: f32 = 48.0;
pub(super) const BODY_Y: f32 = 102.0;
const FOOTER_Y: f32 = 1000.0;

/// Where the panel's footer line is: at its bottom, however tall the window makes it.
pub(super) fn footer_y(kit: &Kit) -> f32 {
    FOOTER_Y + kit.dy()
}

/// Draws a right-hand panel's content (`f`) at the window's right edge.
pub(super) fn at_right<R>(kit: &mut Kit, f: impl FnOnce(&mut Kit) -> R) -> R {
    let dx = kit.dx();
    kit.offset(dx, 0.0, f)
}

/// The panel's text, in Myriad.
pub(super) fn look() -> Look {
    Look {
        font: design::myriad(20.0),
        bold: design::myriad_bold(20.0),
        mono: egui::FontId::monospace(dz(14.5)),
        small: design::myriad(19.0),
        color: design::BODY,
        strong: design::HEADING,
        subtle: design::SUBTLE,
        link: design::LINK,
        chip: Color32::from_rgb(16, 4, 44),
        chip_rim: Color32::from_rgb(46, 32, 84),
        line_gap: dz(9.0),
        paragraph_gap: dz(10.0),
        bullet_indent: dz(20.0),
        uppercase: false,
        dash_bullets: false,
    }
}

/// The right-hand panel with its title.
pub(super) fn frame(kit: &Kit, title: &str) {
    // As in the concept: rounded, with a violet rim that is faint at the top and glows at
    // the bottom. panel_bg.png's own square border (2 px) is left out for it.
    let radius = dz(10.0);
    let panel = PANEL.taller(kit.dy());
    kit.image_rounded("panel_bg.png", panel, radius, 3.0);
    kit.rim(kit.drect(panel), radius, dz(2.0), |t| {
        let glow = ((t - 0.45) / 0.55).clamp(0.0, 1.0);
        let glow = glow * glow;
        style::mix(RIM_FAINT, RIM_GLOW, glow)
    });
    let g = kit.spaced_galley(
        &title.to_uppercase(),
        design::din(24.0),
        design::TEXT,
        dz(1.2),
        false,
    );
    kit.put(dz(1342.0), dz(TITLE_Y), g);
}
