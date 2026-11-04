//! Keeps the installed versions' folders ready to play while the launcher runs, so Echo VR
//! started some other way (Steam's library, WiVRn's list, `echovr.exe` itself) plays with
//! the choices made here (see `core::launcher::ready`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::core::launcher::ready;
use crate::core::launcher::store::{InstalledVersion, LauncherState};
use crate::ui::parts::Worker;

/// How often the versions' fingerprints are looked at.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// After a version couldn't be made ready: when to try again (doubling up to the most).
const RETRY_AFTER: Duration = Duration::from_secs(60);
const RETRY_MOST: Duration = Duration::from_secs(30 * 60);

/// A version made ready (`Ok`) or not, for the fingerprint it was made ready for.
type Done = (String, u64, Result<(), String>);

#[derive(Default)]
pub(super) struct Keeper {
    /// The fingerprint each version's folder was last made ready for.
    applied: HashMap<String, u64>,
    /// After a failure, for the fingerprint that failed: not before then, and how long
    /// that wait was.
    retry: HashMap<String, (u64, Instant, Duration)>,
    worker: Worker<Done>,
    busy: bool,
    at: Option<Instant>,
    /// What the watching thread compares with (`None` until it runs).
    watch: Option<Arc<Mutex<Watch>>>,
}

/// The watching thread's copy: the choices as they are now, and the fingerprints made
/// ready (or waiting for a retry).
#[derive(Default)]
struct Watch {
    state: Option<LauncherState>,
    applied: HashMap<String, u64>,
    waiting: HashMap<String, (u64, Instant)>,
}

/// Every [`CHECK_EVERY`] off the window's thread: a frame only when a version's fingerprint
/// is new (so an idle window draws nothing for this). The frame's tick does the rest.
fn watch(ctx: egui::Context, shared: Arc<Mutex<Watch>>) {
    let _ = std::thread::Builder::new()
        .name("ready-keeper".into())
        .spawn(move || loop {
            std::thread::sleep(CHECK_EVERY);
            let (state, applied, waiting) = {
                let w = shared.lock().unwrap_or_else(|e| e.into_inner());
                let Some(state) = w.state.clone() else {
                    continue;
                };
                (state, w.applied.clone(), w.waiting.clone())
            };
            let due = state.versions.iter().filter(|v| v.present()).any(|v| {
                let fingerprint = ready::fingerprint(&state, v);
                applied.get(&v.id) != Some(&fingerprint)
                    && !waiting.get(&v.id).is_some_and(|(failed, until)| {
                        *failed == fingerprint && Instant::now() < *until
                    })
            });
            if due {
                ctx.request_repaint();
            }
        });
}

impl Keeper {
    /// PLAY (or another start) made `v` ready just now.
    pub fn applied(&mut self, state: &LauncherState, v: &InstalledVersion) {
        self.applied
            .insert(v.id.clone(), ready::fingerprint(state, v));
        self.retry.remove(&v.id);
    }

    /// Keeps the watching thread's copy current (and starts it).
    fn share(&mut self, ctx: &egui::Context, state: &LauncherState) {
        let shared = self.watch.get_or_insert_with(|| {
            let shared = Arc::new(Mutex::new(Watch::default()));
            watch(ctx.clone(), shared.clone());
            shared
        });
        let mut w = shared.lock().unwrap_or_else(|e| e.into_inner());
        if w.state.as_ref() != Some(state) {
            w.state = Some(state.clone());
        }
        if w.applied != self.applied {
            w.applied = self.applied.clone();
        }
        let waiting: HashMap<String, (u64, Instant)> = self
            .retry
            .iter()
            .map(|(id, (f, until, _))| (id.clone(), (*f, *until)))
            .collect();
        if w.waiting != waiting {
            w.waiting = waiting;
        }
    }

    /// Every frame: makes one version ready when its fingerprint changed since, on the
    /// worker. `skip` leaves a version alone for now (its game runs, a job has its folder,
    /// a start is on its way); it's done once that's over.
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        state: &LauncherState,
        skip: impl Fn(&InstalledVersion) -> bool,
    ) {
        for (id, fingerprint, result) in self.worker.drain() {
            self.busy = false;
            match result {
                Ok(()) => {
                    self.applied.insert(id.clone(), fingerprint);
                    self.retry.remove(&id);
                }
                Err(e) => {
                    let wait = self
                        .retry
                        .get(&id)
                        .map_or(RETRY_AFTER, |(_, _, w)| (*w * 2).min(RETRY_MOST));
                    tracing::warn!(
                        "{id}: not ready to play: {e} (again in {}s)",
                        wait.as_secs()
                    );
                    self.retry
                        .insert(id, (fingerprint, Instant::now() + wait, wait));
                }
            }
        }
        self.share(ctx, state);
        if self.busy || self.at.is_some_and(|t| t.elapsed() < CHECK_EVERY) {
            return;
        }
        self.at = Some(Instant::now());
        for v in state.versions.iter().filter(|v| v.present()) {
            if skip(v) {
                continue;
            }
            let fingerprint = ready::fingerprint(state, v);
            if self.applied.get(&v.id) == Some(&fingerprint) {
                continue;
            }
            let waiting = self.retry.get(&v.id).is_some_and(|(failed, until, _)| {
                *failed == fingerprint && Instant::now() < *until
            });
            if waiting {
                continue;
            }
            self.busy = true;
            let (state, v) = (state.clone(), v.clone());
            self.worker.spawn(ctx, move |tx| {
                let result = ready::prepare(&state, &v).map_err(|f| f.to_string());
                if result.is_ok() {
                    tracing::info!("{}: ready to play, however it's started", v.id);
                }
                tx.send((v.id.clone(), fingerprint, result));
            });
            return;
        }
    }
}
