//! Fonts, and egui's own look (tooltips, text cursor, selection) in the design's colours.

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId};

pub const CONTHRAX: &str = "conthrax";
pub const ARIAL: &str = "arial";
pub const ARIAL_BOLD: &str = "arial-bold";
/// The launcher design's condensed DIN caps (status bar, info line, card text).
pub const DIN: &str = "din";
/// The launcher design's Myriad (the right-hand panels).
pub const MYRIAD: &str = "myriad";
pub const MYRIAD_BOLD: &str = "myriad-bold";

/// The Swing UI's own font.
pub fn conthrax(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(CONTHRAX.into()))
}

/// Java's logical "Arial"; Liberation Sans is metric-compatible and freely licensed.
pub fn arial(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ARIAL.into()))
}

pub fn arial_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ARIAL_BOLD.into()))
}

pub fn din(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(DIN.into()))
}

pub fn myriad(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MYRIAD.into()))
}

pub fn myriad_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MYRIAD_BOLD.into()))
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    defs.font_data.insert(
        CONTHRAX.into(),
        FontData::from_static(include_bytes!("../../assets/fonts/conthrax-sb.otf")).into(),
    );
    defs.font_data.insert(
        ARIAL.into(),
        FontData::from_static(include_bytes!(
            "../../assets/fonts/LiberationSans-Regular.ttf"
        ))
        .into(),
    );
    defs.font_data.insert(
        ARIAL_BOLD.into(),
        FontData::from_static(include_bytes!("../../assets/fonts/LiberationSans-Bold.ttf")).into(),
    );
    defs.font_data.insert(
        DIN.into(),
        FontData::from_static(include_bytes!("../../assets/fonts/dmcaps.ttf")).into(),
    );
    defs.font_data.insert(
        MYRIAD.into(),
        FontData::from_static(include_bytes!("../../assets/fonts/myriad-medium.ttf")).into(),
    );
    defs.font_data.insert(
        MYRIAD_BOLD.into(),
        FontData::from_static(include_bytes!("../../assets/fonts/myriad-bold.ttf")).into(),
    );
    // egui's bundled fonts stay behind ours as glyph fallbacks (✓, arrows, emoji).
    let fallbacks: Vec<String> = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for name in [CONTHRAX, ARIAL, ARIAL_BOLD, DIN, MYRIAD, MYRIAD_BOLD] {
        let mut chain = vec![name.to_string()];
        match name {
            // These lack symbols like "…", "·" or "•".
            CONTHRAX | DIN | MYRIAD => chain.push(ARIAL.into()),
            MYRIAD_BOLD => chain.push(ARIAL_BOLD.into()),
            _ => {}
        }
        chain.extend(fallbacks.iter().cloned());
        defs.families.insert(FontFamily::Name(name.into()), chain);
    }
    // Plain egui widgets (TextEdit caret metrics etc.) default to Arial too.
    if let Some(p) = defs.families.get_mut(&FontFamily::Proportional) {
        p.insert(0, ARIAL.into());
    }
    ctx.set_fonts(defs);
}

pub fn install_style(ctx: &egui::Context) {
    use super::design::{self, dz};
    // The zoom follows the window (`ui::fit_zoom`).
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    ctx.all_styles_mut(|style| {
        style.interaction.selectable_labels = false;
        style.visuals = egui::Visuals::dark();
        let v = &mut style.visuals;
        v.text_cursor.stroke = egui::Stroke::new(1.5, Color32::WHITE);
        v.selection.bg_fill = design::BLUE.gamma_multiply(0.7);
        v.selection.stroke = egui::Stroke::new(1.0, Color32::WHITE);
        // Tooltips: the popup violet with the card rim's top colour.
        v.window_fill = design::POPUP;
        v.panel_fill = design::POPUP;
        v.window_stroke = egui::Stroke::new(1.0, design::RIM_TOP);
        v.window_corner_radius = egui::CornerRadius::same(dz(6.0) as u8);
        v.menu_corner_radius = egui::CornerRadius::same(dz(6.0) as u8);
        v.popup_shadow = egui::Shadow {
            offset: [0, 4],
            blur: 10,
            spread: 0,
            color: Color32::from_black_alpha(110),
        };
        v.override_text_color = Some(design::TEXT);
        style.spacing.item_spacing = egui::vec2(0.0, 0.0);
        style.spacing.tooltip_width = 360.0;
        style.spacing.menu_margin = egui::Margin::symmetric(10, 7);
        style
            .text_styles
            .insert(egui::TextStyle::Body, myriad(14.0));
    });
}
