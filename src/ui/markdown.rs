//! Discord-flavoured markdown, the subset status embeds and announcements use: `-`/`*`/`•`
//! bullets, `-#` subtext, `#` headings, `>` quotes, `**bold**`, `__underline__`,
//! `*italics*`, `~~strike~~`, `` `code` `` chips, `[masked](links)` and `<t:…>`
//! timestamps (shown in local time). Links are coloured, not clickable.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{pos2, vec2, Color32, FontId, Galley, Pos2, Rect, Stroke, StrokeKind};

use super::design;
use super::kit::Kit;
use crate::core::launcher::feed;

/// Fonts and colours for one block of text.
#[derive(Debug, Clone)]
pub struct Look {
    pub font: FontId,
    pub bold: FontId,
    pub mono: FontId,
    /// `-#` subtext.
    pub small: FontId,
    pub color: Color32,
    /// Bold text and headings.
    pub strong: Color32,
    pub subtle: Color32,
    pub link: Color32,
    /// Behind code and timestamps, and their rim.
    pub chip: Color32,
    pub chip_rim: Color32,
    /// Extra space after each line.
    pub line_gap: f32,
    /// Space for a paragraph break (an empty line).
    pub paragraph_gap: f32,
    pub bullet_indent: f32,
    /// For caps-only fonts.
    pub uppercase: bool,
    /// Keep "- " lines as written (a dash, no indent) instead of drawing bullets.
    pub dash_bullets: bool,
}

#[derive(Default, Clone, Copy)]
struct Marks {
    bold: bool,
    underline: bool,
    italic: bool,
    strike: bool,
}

/// One laid-out line: where it goes (relative to the block) and whether it has a bullet.
struct Line {
    x: f32,
    y: f32,
    galley: Arc<Galley>,
    bullet: bool,
    quote: bool,
}

/// Draws `text` at logical (x, y), wrapped at `w`; returns the height used.
pub fn draw(kit: &Kit, x: f32, y: f32, w: f32, text: &str, look: &Look) -> f32 {
    paint(kit, x, y, look, layout(kit, w, text, look))
}

fn paint(kit: &Kit, x: f32, y: f32, look: &Look, laid: (Vec<Line>, f32)) -> f32 {
    let (lines, h) = laid;
    let p = kit.ui.painter();
    for l in lines {
        let at = kit.origin + vec2(x + l.x, y + l.y);
        if l.bullet {
            let row_h = l.galley.rows.first().map_or(0.0, |r| r.height());
            let r = (look.font.size * 0.15).max(1.5);
            p.circle_filled(
                pos2(at.x - look.bullet_indent * 0.75, at.y + row_h * 0.5),
                r,
                look.color,
            );
        }
        if l.quote {
            let bar = egui::Rect::from_min_size(
                pos2(at.x - look.bullet_indent * 0.7, at.y),
                vec2(2.0, l.galley.size().y),
            );
            p.rect_filled(bar, 1.0, look.subtle);
        }
        let galley = l.galley;
        p.galley(at, galley.clone(), look.color);
        chip_rims(p, at, &galley, look);
    }
    h
}

/// Rims around code and timestamp chips: their backgrounds are the `look.chip` quads in
/// each row's mesh.
fn chip_rims(p: &egui::Painter, at: Pos2, galley: &Galley, look: &Look) {
    if look.chip_rim == Color32::TRANSPARENT {
        return;
    }
    for placed in &galley.rows {
        let v = &placed.row.visuals.mesh.vertices;
        let mut i = 0;
        while i + 4 <= v.len() {
            if v[i..i + 4].iter().all(|q| q.color == look.chip) {
                let corners: Vec<Pos2> = v[i..i + 4].iter().map(|q| q.pos).collect();
                let r = Rect::from_points(&corners).translate(at.to_vec2() + placed.pos.to_vec2());
                p.rect_stroke(r, 2.0, Stroke::new(1.0, look.chip_rim), StrokeKind::Inside);
                i += 4;
            } else {
                i += 1;
            }
        }
    }
}

fn layout(kit: &Kit, w: f32, text: &str, look: &Look) -> (Vec<Line>, f32) {
    let mut out = Vec::new();
    let mut y = 0.0;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            y += look.paragraph_gap;
            continue;
        }
        let t = line.trim_start();
        let (mut body, mut indent, mut bullet, mut quote) = (t, 0.0, false, false);
        let mut font = look.font.clone();
        let mut color = look.color;
        let mut bold = false;
        if let Some(rest) = t.strip_prefix("-# ") {
            body = rest;
            font = look.small.clone();
            color = look.subtle;
        } else if let Some((level, rest)) = heading(t) {
            body = rest;
            bold = true;
            color = look.strong;
            font = FontId::new(
                look.bold.size * [1.5, 1.25, 1.1][level - 1],
                look.bold.family.clone(),
            );
        } else if let Some(rest) = t.strip_prefix("> ") {
            body = rest;
            indent = look.bullet_indent;
            quote = true;
        } else if let Some(rest) = ["- ", "* ", "• "]
            .iter()
            .find_map(|p| t.strip_prefix(p))
            .filter(|_| !look.dash_bullets)
        {
            body = rest;
            // Nested bullets: two leading spaces per level.
            let depth = (line.len() - t.len()) / 2;
            indent = look.bullet_indent * (1 + depth) as f32;
            bullet = true;
        }
        let mut job = LayoutJob::default();
        job.wrap.max_width = (w - indent).max(10.0);
        inline(&mut job, body, &font, color, bold, look);
        let galley = kit.ui.ctx().fonts_mut(|f| f.layout_job(job));
        let h = galley.size().y;
        out.push(Line {
            x: indent,
            y,
            galley,
            bullet,
            quote,
        });
        y += h + look.line_gap;
    }
    (out, (y - look.line_gap).max(0.0))
}

fn heading(t: &str) -> Option<(usize, &str)> {
    ["# ", "## ", "### "]
        .iter()
        .enumerate()
        .find_map(|(i, p)| t.strip_prefix(p).map(|rest| (i + 1, rest)))
}

fn fmt(font: &FontId, color: Color32, m: Marks, look: &Look) -> TextFormat {
    let font_id = if m.bold && font.family == look.font.family {
        FontId::new(font.size, look.bold.family.clone())
    } else {
        font.clone()
    };
    let color = if m.bold { look.strong } else { color };
    TextFormat {
        font_id,
        color,
        italics: m.italic,
        underline: if m.underline {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        strikethrough: if m.strike {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        valign: egui::Align::Center,
        ..Default::default()
    }
}

/// A chip (code or timestamp): the text on `look.chip`, with a little room each side.
fn chip(job: &mut LayoutJob, text: &str, font: FontId, color: Color32, look: &Look) {
    let expand = (font.size * 0.2).round().max(1.0);
    let room = TextFormat {
        font_id: FontId::new(expand * 2.5, font.family.clone()),
        valign: egui::Align::Center,
        ..Default::default()
    };
    job.append(" ", 0.0, room.clone());
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font,
            color,
            background: look.chip,
            expand_bg: expand,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.append(" ", 0.0, room);
}

/// Appends one line's inline markdown to `job`.
fn inline(job: &mut LayoutJob, s: &str, font: &FontId, color: Color32, bold: bool, look: &Look) {
    let mut m = Marks {
        bold,
        ..Default::default()
    };
    let mut lit = String::new();
    let up = |t: &str| {
        if look.uppercase {
            t.to_uppercase()
        } else {
            t.to_string()
        }
    };
    let flush = |job: &mut LayoutJob, lit: &mut String, m: Marks| {
        if !lit.is_empty() {
            design::append_text(job, &up(lit), fmt(font, color, m, look));
            lit.clear();
        }
    };
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if let Some(r) = rest.strip_prefix('\\') {
            if let Some(c) = r.chars().next() {
                lit.push(c);
                i += 1 + c.len_utf8();
                continue;
            }
        }
        if rest.starts_with("**") {
            flush(job, &mut lit, m);
            m.bold = !m.bold;
            i += 2;
            continue;
        }
        if rest.starts_with("__") {
            flush(job, &mut lit, m);
            m.underline = !m.underline;
            i += 2;
            continue;
        }
        if rest.starts_with("~~") {
            flush(job, &mut lit, m);
            m.strike = !m.strike;
            i += 2;
            continue;
        }
        if rest.starts_with('*') && (m.italic || rest[1..].contains('*')) {
            flush(job, &mut lit, m);
            m.italic = !m.italic;
            i += 1;
            continue;
        }
        if let Some(r) = rest.strip_prefix('`') {
            if let Some(end) = r.find('`') {
                flush(job, &mut lit, m);
                let code = r[..end].to_string();
                chip(job, &code, look.mono.clone(), color, look);
                i += end + 2;
                continue;
            }
        }
        if let Some((label, len)) = masked_link(rest) {
            flush(job, &mut lit, m);
            let mut f = fmt(font, look.link, m, look);
            f.color = look.link;
            job.append(&up(label), 0.0, f);
            i += len;
            continue;
        }
        if let Some((unix, style, len)) = timestamp(rest) {
            flush(job, &mut lit, m);
            chip(
                job,
                &feed::discord_time(unix, style),
                font.clone(),
                color,
                look,
            );
            i += len;
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        lit.push(c);
        i += c.len_utf8();
    }
    flush(job, &mut lit, m);
    if job.sections.is_empty() {
        job.append(" ", 0.0, fmt(font, color, m, look));
    }
}

/// `[label](url)` at the start of `s`: the label and the length consumed.
fn masked_link(s: &str) -> Option<(&str, usize)> {
    let r = s.strip_prefix('[')?;
    let close = r.find("](")?;
    let label = &r[..close];
    if label.contains('\n') {
        return None;
    }
    let after = &r[close + 2..];
    let end = after.find(')')?;
    let url = after[..end].trim_matches(|c| c == '<' || c == '>');
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    Some((label, 1 + close + 2 + end + 1))
}

/// `<t:1790410560>` or `<t:1790410560:f>` at the start of `s`.
fn timestamp(s: &str) -> Option<(i64, char, usize)> {
    let r = s.strip_prefix("<t:")?;
    let end = r.find('>')?;
    let inner = &r[..end];
    let (num, style) = match inner.split_once(':') {
        Some((n, st)) if st.len() == 1 => (n, st.chars().next()?),
        None => (inner, 'f'),
        _ => return None,
    };
    Some((num.parse().ok()?, style, 3 + end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_timestamps() {
        assert_eq!(
            masked_link("[7/12](https://x.y/z) rest"),
            Some(("7/12", 21))
        );
        assert_eq!(masked_link("[a](not a url)"), None);
        assert_eq!(timestamp("<t:1790410560:f> x"), Some((1790410560, 'f', 16)));
        assert_eq!(timestamp("<t:1790410560>"), Some((1790410560, 'f', 14)));
        assert_eq!(timestamp("<t:abc>"), None);
    }
}
