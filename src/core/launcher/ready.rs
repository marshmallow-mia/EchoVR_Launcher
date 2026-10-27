//! An installed version's folder ready to play, however Echo VR is started: PLAY, `--play`
//! (Steam's library, WiVRn's list, a desktop entry), or `echovr.exe` / `EchoXR.exe` started
//! by something else. What a start needs on disk (nEVR's `config.yaml` with the mods and
//! the server, the required mods, EchoRelay's or the classic lobbies' files, EchoXR and its
//! copy of the game) is written whenever it changes, not only at PLAY: the launcher's
//! window and tray keep it ([`fingerprint`] says when), and every start writes it once more.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use super::mods;
use super::relay;
use super::store::{InstalledVersion, LauncherState};

/// What could not be made ready, and how bad that is for a start.
#[derive(Debug)]
pub struct Failed {
    pub step: Part,
    pub error: anyhow::Error,
}

/// The parts of a version's readiness (see [`prepare`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// An event build's classic lobbies: EchoLoader, EchoRelay's patch, its `config.json`.
    ClassicLobby,
    /// The live build on an EchoRelay server a plugin chose.
    EchoRelay,
    /// The mods beside EchoRelay's patch there.
    EchoRelayMods,
    /// The live build with nEVR: `config.yaml`, the required mods, the sign-in's place.
    Mods,
    /// EchoXR in the game's folder and its copy of the game current.
    EchoXr,
}

impl Part {
    /// The title a dialog about it gets.
    pub fn title(self) -> &'static str {
        match self {
            Part::ClassicLobby => "Couldn't set up the classic lobby",
            Part::EchoRelay => "Couldn't set up EchoRelay",
            Part::EchoRelayMods => "Couldn't set up the mods for EchoRelay",
            Part::Mods => "Couldn't prepare the mods",
            Part::EchoXr => "Couldn't set up EchoXR",
        }
    }
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {:#}", self.step.title(), self.error)
    }
}

/// One at a time in this process: the window's PLAY and its keeper share the files.
static PREPARING: Mutex<()> = Mutex::new(());

/// Makes `v`'s folder ready for a start with `state`'s choices. Downloads what isn't there
/// yet (EchoLoader, EchoRelay's patch, required mods). Stops at the first part that fails;
/// EchoXR's part is only tried where the start goes through it.
pub fn prepare(state: &LauncherState, v: &InstalledVersion) -> Result<(), Failed> {
    let _one = PREPARING.lock().unwrap_or_else(|e| e.into_inner());
    let fail = |step| move |error| Failed { step, error };
    let cancel = AtomicBool::new(false);
    let hands = v.publisher_lock.is_none() && state.profile.hands(state.echoxr_hands);
    if v.publisher_lock.is_some() {
        // An event build: on the classic lobbies server, as your account there.
        if !relay::in_place(v) {
            relay::set_up(v, &cancel, &mut |_| {}).map_err(fail(Part::ClassicLobby))?;
        }
        if let Some(account) = &state.relay_account {
            relay::write_config(v, &state.relay_server, account)
                .map_err(fail(Part::ClassicLobby))?;
        }
    } else if let Some((server, account)) = mods::echorelay_for(state, v) {
        // The live build on an EchoRelay server: EchoLoader in nEVR's place, and the mods
        // that are on beside EchoRelay's patch.
        relay::set_up_live(v, &cancel, &mut |_| {})
            .and_then(|()| relay::write_live_config(v, &server, &account))
            .map_err(fail(Part::EchoRelay))?;
        mods::before_echorelay_start(v, hands).map_err(fail(Part::EchoRelayMods))?;
    } else {
        mods::before_start(v, hands, &mods::Servers::of(state), &state.holds)
            .map_err(fail(Part::Mods))?;
    }
    // nEVR's log filter, from the Logs menu's choice (where nEVR is in place).
    if mods::nevr_in(&v.bin_dir()) {
        match super::nevr::apply_logging(&v.bin_dir(), &state.nevr_logging) {
            Ok(super::nevr::LoggingStep::Own(own)) => {
                tracing::info!("{}: {} is your own: left as it is", v.id, own.display())
            }
            Ok(super::nevr::LoggingStep::Done) => {}
            // Only logs: a start isn't worth stopping for them.
            Err(e) => tracing::warn!("{}: logging: {e:#}", v.id),
        }
    }
    echoxr(state, v).map_err(fail(Part::EchoXr))
}

/// EchoXR in `v`'s folder where its starts go through it: on Windows the EchoXR routes once
/// EchoXR is got; on Linux every VR start, once Linux is set up (with the Platform SDK
/// stand-in beside the game).
fn echoxr(state: &LauncherState, v: &InstalledVersion) -> anyhow::Result<()> {
    let bin = v.bin_dir();
    if cfg!(windows) {
        if state.profile.through_echoxr() && crate::core::echoxr::is_fetched() {
            crate::core::echoxr::prepare(&bin)?;
        }
    } else if cfg!(target_os = "linux") && crate::core::linux::echoxr::is_set_up() {
        crate::core::echoxr::install_into(&bin)?;
        crate::core::echoxr::platform_beside_game(&bin)?;
        crate::core::echoxr::refresh_openxr_exe(&bin)?;
    }
    // The Controllers plugin's settings beside EchoXR, wherever it is (a removed plugin's
    // copy goes).
    super::echoxr_settings::apply(v, super::echoxr_settings::current(state).as_deref())
}

/// What [`prepare`] writes `v`'s folder from, as one number: when it changes, the folder
/// is due again. The choices in `state` that matter, `v`'s Mods page choices and plugins,
/// the mods catalogue (its required mods), whether EchoXR is there, and the game's own
/// files (an update or Verify replaces them, a repair the data a content pack needs).
pub fn fingerprint(state: &LauncherState, v: &InstalledVersion) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let choices = serde_json::to_string(&(
        &state.profile,
        &state.game_server,
        &state.pack_servers,
        &state.nevr_logging,
        state.echoxr_hands,
        &state.relay_server,
        &state.relay_account,
    ))
    .unwrap_or_default();
    choices.hash(&mut h);
    v.root.hash(&mut h);
    std::fs::read(mods::choices_path(v)).ok().hash(&mut h);
    // The catalogues as kept (read, not parsed: parsing logs what it leaves out).
    std::fs::read(crate::core::paths::data_dir().join("mods.json"))
        .ok()
        .hash(&mut h);
    super::dev_code::kept("mods").hash(&mut h);
    crate::core::echoxr::is_fetched().hash(&mut h);
    super::echoxr_settings::current(state).hash(&mut h);
    (cfg!(target_os = "linux") && crate::core::linux::echoxr::is_set_up()).hash(&mut h);
    let bin = v.bin_dir();
    stamp(&v.exe_path()).hash(&mut h);
    // The game data the packs need as it shipped (a repair puts it back: they're served).
    for pack in mods::installed_packs(v) {
        for (manifest, _) in super::packs::stock_manifests(&bin, &pack.content) {
            stamp(&manifest).hash(&mut h);
        }
    }
    stamp(&bin.join(mods::SLOT)).hash(&mut h);
    stamp(&bin.join(super::nevr::RUNTIME)).hash(&mut h);
    if let Ok(entries) = std::fs::read_dir(bin.join("plugins")) {
        let mut plugins: Vec<_> = entries
            .flatten()
            .map(|e| (e.file_name(), stamp(&e.path())))
            .collect();
        plugins.sort();
        plugins.hash(&mut h);
    }
    h.finish()
}

/// A file's size and modification time, when it's there.
fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}
