//! Text cut with "…", noted while snapshots are taken (`ECHOVR_SNAPSHOTS`): which shot
//! cut which text, so labels that don't fit are found on every page, not only the ones
//! someone happens to see. The headless snapshots write them to `cuts.txt`.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use egui::Galley;

/// The shot being taken.
static SHOT: Mutex<String> = Mutex::new(String::new());
/// (shot, text) of every cut.
static FOUND: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ECHOVR_SNAPSHOTS").is_some())
}

/// Notes `text` when `g`, laid out from it, was cut.
pub(crate) fn note(g: &Galley, text: &str) {
    if g.elided {
        note_text(text);
    }
}

/// Notes that `text` was cut.
pub(crate) fn note_text(text: &str) {
    if on() {
        let shot = SHOT.lock().map(|s| s.clone()).unwrap_or_default();
        if let Ok(mut found) = FOUND.lock() {
            found.insert((shot, text.to_string()));
        }
    }
}

/// The shot whose cuts come next.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn set_shot(name: &str) {
    if let Ok(mut s) = SHOT.lock() {
        *s = name.to_string();
    }
}

/// Every cut so far, by shot.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn take() -> Vec<(String, String)> {
    FOUND
        .lock()
        .map(|mut f| std::mem::take(&mut *f).into_iter().collect())
        .unwrap_or_default()
}
