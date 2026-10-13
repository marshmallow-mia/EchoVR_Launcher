//! An easter egg: "uwu" typed into any text field uwufies the launcher, its text and its
//! colours, until the Unuwufy button (in the top bar) or the next start. Only what is drawn
//! changes: what is typed into fields, and the data behind the text, stay as they are.
//! The launcher's own images (its UI art) turn pink too, and the background video gets a
//! pink tint; real pictures (the news', anything downloaded) stay as they are.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use egui::ecolor::HsvaGamma;
use egui::epaint::Shape;
use egui::{Color32, ColorImage, TextureId};

static ON: AtomicBool = AtomicBool::new(false);

/// Textures already pink (the UI art's uwu copies): drawn as they are.
static PINK: Mutex<Option<HashSet<TextureId>>> = Mutex::new(None);
/// Textures that are UI but not the art's (the background video): tinted pink.
static TINT: Mutex<Option<HashSet<TextureId>>> = Mutex::new(None);

fn mark(set: &Mutex<Option<HashSet<TextureId>>>, id: TextureId) {
    if let Ok(mut s) = set.lock() {
        s.get_or_insert_with(HashSet::new).insert(id);
    }
}

/// `id` is an image's uwu copy (see [`pinkify`]).
pub fn mark_pink(id: TextureId) {
    mark(&PINK, id);
}

/// `id` is UI that gets a pink tint while it's on (the background video).
pub fn mark_tint(id: TextureId) {
    mark(&TINT, id);
}

/// The built-in images that are pictures, not UI art: the news banner shown when there is
/// no news. They stay as they are, as the news' own pictures do.
const PICTURES: [&str; 2] = ["news_fallback_bg.jpg", "news_fallback_text.png"];

/// Whether the built-in image `name` is drawn as its pink copy now.
pub fn pink_image(name: &str) -> bool {
    on() && !PICTURES.contains(&name)
}

/// An image's pixels turned pink, as [`color`] turns a colour.
pub fn pinkify(img: &mut ColorImage) {
    for p in &mut img.pixels {
        color(p);
    }
}

thread_local! {
    /// Text drawn now stays as it is (the Unuwufy button's own label).
    static RAW: Cell<bool> = const { Cell::new(false) };
    /// Text drawn now gets its letters changed only: no stutters, no faces (a button's,
    /// a tile's, a chip's, which must still fit).
    static LETTERS: Cell<bool> = const { Cell::new(false) };
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn set(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// Whether a field's text, as typed, turns it on: "uwu" and nothing else.
pub fn is_trigger(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case("uwu")
}

/// Runs `f` with text left as it is.
pub fn raw<R>(f: impl FnOnce() -> R) -> R {
    let was = RAW.with(|r| r.replace(true));
    let out = f();
    RAW.with(|r| r.set(was));
    out
}

/// Runs `f` with text getting its letters changed only (see `LETTERS`).
pub fn letters<R>(f: impl FnOnce() -> R) -> R {
    let was = LETTERS.with(|r| r.replace(true));
    let out = f();
    LETTERS.with(|r| r.set(was));
    out
}

/// `s` as it is drawn: uwufied while it's on.
pub fn text(s: &str) -> Cow<'_, str> {
    if !on() || RAW.with(Cell::get) || s.trim().is_empty() {
        Cow::Borrowed(s)
    } else if LETTERS.with(Cell::get) {
        Cow::Owned(uwufy_letters(s))
    } else {
        Cow::Owned(uwufy(s))
    }
}

/// Pure: `s` with its words' letters changed and nothing added.
pub fn uwufy_letters(s: &str) -> String {
    s.split(' ')
        .map(|w| if keep(w) { w.to_string() } else { word(w) })
        .collect::<Vec<_>>()
        .join(" ")
}

const FACES: [&str; 6] = ["uwu", "owo", ">w<", "^w^", ":3", "x3"];

/// FNV-1a: the same choices for the same text, every frame and every start.
fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A word that says what something is, not how: a version, path, address, file name,
/// command-line option or number. It stays as it is.
fn keep(word: &str) -> bool {
    let inner = word.trim_end_matches(['.', ',', '!', '?', ';', ')', '"', '\'']);
    word.starts_with('-')
        || inner.contains(['/', '\\', '@', ':', '_', '=', '.'])
        || word.chars().any(|c| c.is_ascii_digit())
        || !word.chars().any(char::is_alphabetic)
}

fn vowel(c: char) -> bool {
    matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u')
}

/// A word's letters: ove → uv, r and l → w, n before a vowel → ny.
fn word(w: &str) -> String {
    let c: Vec<char> = w.chars().collect();
    let mut out = String::with_capacity(w.len() + 2);
    let mut i = 0;
    while i < c.len() {
        let low = |k: usize| c.get(i + k).map(|c| c.to_ascii_lowercase());
        if low(0) == Some('o') && low(1) == Some('v') && low(2) == Some('e') {
            out.push(if c[i].is_uppercase() { 'U' } else { 'u' });
            out.push(c[i + 1]);
            i += 3;
            continue;
        }
        match c[i] {
            'r' | 'l' => out.push('w'),
            'R' | 'L' => out.push('W'),
            n @ ('n' | 'N') if c.get(i + 1).is_some_and(|&v| vowel(v)) => {
                out.push(n);
                out.push(if c[i + 1].is_uppercase() { 'Y' } else { 'y' });
            }
            ch => out.push(ch),
        }
        i += 1;
    }
    out
}

/// Pure: `s` uwufied. Labels of one or two words get their letters only (they must still
/// fit); sentences sometimes start with a stutter, may get a face in the middle, and end
/// with one or two.
pub fn uwufy(s: &str) -> String {
    let words: Vec<&str> = s.split(' ').collect();
    if words.iter().filter(|w| !w.is_empty()).count() < 3 {
        return uwufy_letters(s);
    }
    // Sentences: up to a word ending in . ! or ? (not a kept one: a version, a path).
    let mut sentences: Vec<Vec<&str>> = vec![Vec::new()];
    for w in &words {
        sentences.last_mut().unwrap().push(w);
        let ends = w.ends_with(['.', '!', '?']) && !keep(w.trim_end_matches(['.', '!', '?']));
        if ends {
            sentences.push(Vec::new());
        }
    }
    let mut out: Vec<String> = Vec::new();
    for sentence in sentences.into_iter().filter(|s| !s.is_empty()) {
        let h = hash(&sentence.join(" "));
        let real = sentence.iter().filter(|w| !w.is_empty()).count();
        let mid = (real >= 6 && h % 6 == 1).then_some(sentence.len() / 2);
        let last = sentence.iter().rposition(|w| !w.is_empty());
        for (i, w) in sentence.iter().enumerate() {
            let mut t = if keep(w) { w.to_string() } else { word(w) };
            if i == 0 && h.is_multiple_of(4) && !keep(w) {
                if let Some(first) = t.chars().next().filter(|c| c.is_alphabetic()) {
                    t = format!("{first}-{t}");
                }
            }
            if Some(i) == mid {
                t = format!("{t} {}", FACES[(h >> 8) as usize % FACES.len()]);
            }
            if Some(i) == last && real >= 2 {
                // A full stop gives way to the face; ! and ? stay.
                let base = if keep(w) {
                    t.as_str()
                } else {
                    t.trim_end_matches('.')
                };
                let mut end = format!("{base} {}", FACES[(h >> 16) as usize % FACES.len()]);
                if h.is_multiple_of(3) {
                    end = format!("{end} {}", FACES[(h >> 24) as usize % FACES.len()]);
                }
                t = end;
            }
            out.push(t);
        }
    }
    out.join(" ")
}

/// A colour turned pastel pink: its hue most of the way to pink, greys tinted, a little
/// lighter. Alpha stays; so do placeholders and additive colours.
pub fn color(c: &mut Color32) {
    if *c == Color32::PLACEHOLDER || c.a() == 0 {
        return;
    }
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let mut hsv = HsvaGamma::from(Color32::from_rgb(r, g, b));
    const PINK: f32 = 330.0 / 360.0;
    // The shorter way round the hue circle.
    let d = (PINK - hsv.h + 1.5).rem_euclid(1.0) - 0.5;
    hsv.h = (hsv.h + d * 0.9).rem_euclid(1.0);
    hsv.s = (hsv.s * 0.85 + 0.3 * (1.0 - hsv.s)).min(1.0);
    hsv.v = hsv.v * 0.9 + 0.1 * hsv.v.sqrt();
    let rgb = Color32::from(hsv);
    *c = Color32::from_rgba_unmultiplied(rgb.r(), rgb.g(), rgb.b(), a);
}

/// Recolours everything a pass painted (tooltips and menus too) while it's on.
pub struct UwuPlugin;

impl egui::Plugin for UwuPlugin {
    fn debug_name(&self) -> &'static str {
        "uwu"
    }

    fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
        if !on() {
            return;
        }
        let set = |s: &Mutex<Option<HashSet<TextureId>>>| {
            s.lock().ok().and_then(|s| s.clone()).unwrap_or_default()
        };
        let (pink, tint) = (set(&PINK), set(&TINT));
        for c in &mut output.shapes {
            recolor(&mut c.shape, &pink, &tint);
        }
    }
}

/// Recolours `shape`: everything but images, whose tint changes only for those marked
/// [`mark_tint`] (the art's pink copies are pink already; real pictures stay).
fn recolor(shape: &mut Shape, pink: &HashSet<TextureId>, tint: &HashSet<TextureId>) {
    let image = match shape {
        Shape::Vec(shapes) => {
            for s in shapes {
                recolor(s, pink, tint);
            }
            return;
        }
        Shape::Rect(r) => r.brush.as_ref().map(|b| (b.fill_texture_id, r.fill)),
        Shape::Mesh(m) if m.texture_id != TextureId::default() => {
            if tint.contains(&m.texture_id) {
                egui::epaint::shape_transform::adjust_colors(shape, color);
            }
            return;
        }
        _ => None,
    };
    egui::epaint::shape_transform::adjust_colors(shape, color);
    if let (Some((id, fill)), Shape::Rect(r)) = (image, shape) {
        if !tint.contains(&id) || pink.contains(&id) {
            r.fill = fill;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uwufies_words_and_sentences() {
        assert_eq!(uwufy("PLAY"), "PWAY");
        assert_eq!(uwufy("Friends"), "Fwiends");
        assert_eq!(uwufy("love"), "wuv");
        assert_eq!(uwufy("no"), "nyo");
        let s = uwufy("Install, update and play from one place.");
        assert!(s.contains("Instaww"), "{s}");
        assert!(
            s.contains("pway") && s.contains("fwom") && s.contains("pwace"),
            "{s}"
        );
        assert!(
            FACES.iter().any(|f| s.ends_with(f)),
            "ends with a face: {s}"
        );
        assert!(!s.contains("pwace."), "the full stop gives way: {s}");
        // Always the same.
        assert_eq!(s, uwufy("Install, update and play from one place."));
    }

    #[test]
    fn leaves_what_things_are_alone() {
        for w in [
            "C:/EchoVR/versions/pc-latest",
            "v0.11.16",
            "168.119.2.92:6800",
            "BugSplat64.dll",
            "-mp",
            "https://echovrce.com",
        ] {
            assert_eq!(uwufy(w), w);
        }
        let s = uwufy("Played on 168.119.2.92:6800 with BugSplat64.dll loaded.");
        assert!(
            s.contains("168.119.2.92:6800") && s.contains("BugSplat64.dll"),
            "{s}"
        );
    }

    #[test]
    fn some_sentences_stutter() {
        let all: Vec<String> = (0..40)
            .map(|i| uwufy(&format!("Hello there friend {i}.")))
            .collect();
        assert!(all.iter().any(|s| s.contains("H-")), "{all:?}");
        assert!(all.iter().any(|s| !s.contains("H-")), "{all:?}");
    }

    #[test]
    fn only_while_on_and_not_raw() {
        set(false);
        assert_eq!(text("Play"), "Play");
        set(true);
        assert_eq!(text("Play"), "Pway");
        assert_eq!(raw(|| text("Play").into_owned()), "Play");
        set(false);
        assert!(is_trigger("uwu") && is_trigger(" UwU ") && !is_trigger("textuwutext"));
    }

    #[test]
    fn colours_go_pink() {
        let hue = |c: Color32| HsvaGamma::from(c).h * 360.0;
        let mut blue = Color32::from_rgb(0, 102, 255);
        color(&mut blue);
        assert!((280.0..350.0).contains(&hue(blue)), "{}", hue(blue));
        let mut white = Color32::WHITE;
        color(&mut white);
        assert!(white.r() > white.g(), "white gets a pink tint: {white:?}");
        let mut see_through = Color32::from_rgba_unmultiplied(0, 102, 255, 100);
        color(&mut see_through);
        assert_eq!(see_through.a(), 100);
        let mut placeholder = Color32::PLACEHOLDER;
        color(&mut placeholder);
        assert_eq!(placeholder, Color32::PLACEHOLDER);
    }
}
