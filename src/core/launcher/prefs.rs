//! The player's per-mod choices from launcher.json, for code without the state at hand (the
//! catalogues are read on workers and before a start): set whenever the settings are loaded
//! or saved, like [`super::dev_code::set_all`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// What launcher.json says about single mods.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModPrefs {
    /// Mods whose prereleases are offered (catalogue ids).
    pub prereleases: BTreeSet<String>,
    /// Catalogue id → where it comes from: `catalogue` or a dev code's
    /// [`super::dev_code::source_tag`].
    pub sources: BTreeMap<String, String>,
    /// Unlisted mods added by id.
    pub unlisted: Vec<String>,
}

static PREFS: Mutex<Option<ModPrefs>> = Mutex::new(None);

/// Takes launcher.json's per-mod choices.
pub fn set(state: &super::store::LauncherState) {
    *PREFS.lock().unwrap_or_else(|p| p.into_inner()) = Some(ModPrefs {
        prereleases: state.mod_prereleases.clone(),
        sources: state.mod_sources.clone(),
        unlisted: state.unlisted_mods.clone(),
    });
}

/// The per-mod choices in use (none before launcher.json was read, and in tests).
pub fn get() -> ModPrefs {
    PREFS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .unwrap_or_default()
}
