//! Shared pieces: a worker channel, the folder picker, and the Quest connection check.

use std::sync::mpsc::{channel, Receiver, Sender};

use super::dialogs::{Answer, DialogHost};
use crate::core::adb::{self, devices::Device, devices::Status};
use crate::core::error;

// ---- worker messages ----

/// Background threads report back through this; every send wakes the UI.
pub struct Worker<M> {
    tx: Sender<M>,
    rx: Receiver<M>,
}

pub struct Tx<M> {
    tx: Sender<M>,
    ctx: egui::Context,
}

impl<M> Clone for Tx<M> {
    fn clone(&self) -> Self {
        Tx {
            tx: self.tx.clone(),
            ctx: self.ctx.clone(),
        }
    }
}

impl<M: Send + 'static> Tx<M> {
    pub fn send(&self, m: M) {
        let _ = self.tx.send(m);
        self.ctx.request_repaint();
    }
}

impl<M: Send + 'static> Default for Worker<M> {
    fn default() -> Self {
        let (tx, rx) = channel();
        Worker { tx, rx }
    }
}

impl<M: Send + 'static> Worker<M> {
    pub fn tx(&self, ctx: &egui::Context) -> Tx<M> {
        Tx {
            tx: self.tx.clone(),
            ctx: ctx.clone(),
        }
    }

    pub fn drain(&self) -> Vec<M> {
        self.rx.try_iter().collect()
    }

    /// Runs `f` on a new thread with a sender.
    pub fn spawn(&self, ctx: &egui::Context, f: impl FnOnce(Tx<M>) + Send + 'static) {
        let tx = self.tx(ctx);
        std::thread::spawn(move || f(tx));
    }
}

// ---- folder picker ----

/// Opens a folder picker; `None` when cancelled.
pub fn choose_folder() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Opens a file picker for the game's executable (`echovr.exe`); `None` when cancelled.
pub fn choose_exe() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Choose echovr.exe")
        .add_filter("Echo VR", &["exe"])
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned())
}

// ---- Quest connection row ----

enum ConnMsg {
    Done(Status, bool),
}

/// "Checking Quest connection..." → ✓ Quest connected (model) / ✗ reason.
#[derive(Default)]
pub struct QuestConn {
    worker: Worker<ConnMsg>,
    pub checking: bool,
    pub status: Option<Status>,
    picker: Option<Vec<Device>>,
    /// With ADB over the network on: where to connect to before looking.
    pub network: Option<std::net::Ipv4Addr>,
}

const PICKER_KEY: &str = "quest-picker";

impl QuestConn {
    pub fn check(&mut self, ctx: &egui::Context, interactive: bool) {
        self.checking = true;
        self.status = None;
        let network = self.network;
        self.worker.spawn(ctx, move |tx| {
            let st = adb::bundle::binary()
                .map(|_| {
                    if let Some(ip) = network {
                        if let Err(e) = crate::core::launcher::quest_net::connect(ip) {
                            tracing::info!("quest over the network: {e:#}");
                        }
                    }
                    adb::connection_status()
                })
                .unwrap_or(Status::None);
            tx.send(ConnMsg::Done(st, interactive));
        });
    }

    /// Returns the final status once a check (and a possible device pick) completes.
    pub fn poll(&mut self, dialogs: &mut DialogHost) -> Option<Status> {
        let mut result = None;
        for ConnMsg::Done(st, interactive) in self.worker.drain() {
            self.checking = false;
            self.status = Some(st);
            if !interactive {
                result = Some(st);
                continue;
            }
            match st {
                Status::Ambiguous => {
                    // Several usable devices and nothing tells them apart: ask, don't guess.
                    let devices = adb::last_selection().pickable();
                    dialogs.device_picker(PICKER_KEY, devices.clone());
                    self.picker = Some(devices);
                }
                Status::Unauthorized => dialogs.error_ui(&error::quest_unauthorized()),
                Status::None => dialogs.error_ui(&error::quest_not_found()),
                Status::Ready => {}
            }
            if st != Status::Ambiguous {
                result = Some(st);
            }
        }
        if let Some(a) = dialogs.take(PICKER_KEY) {
            let devices = self.picker.take().unwrap_or_default();
            match a {
                Answer::Button(i) if i < devices.len() => {
                    adb::set_preferred(&devices[i].serial);
                    self.status = Some(Status::Ready);
                    result = Some(Status::Ready);
                }
                _ => result = self.status,
            }
        }
        result
    }
}
