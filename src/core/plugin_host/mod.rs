//! Launcher plugins that are code (`plugin.json` schema 2): WebAssembly components
//! (`wit/launcher.wit`, `docs/plugins/v2.md`) run by wasmtime, each in a sandbox of its own.
//!
//! A plugin gets no files, no network and no clock: only the imports in
//! [`imports`], and each of them checks what the player allowed it
//! ([`crate::core::launcher::plugin_permissions`]). Secrets never enter the sandbox: the
//! classic lobbies password is filled into a request by the launcher, a password typed into
//! a plugin's secret field stays here, and the services that need a sign-in
//! ([`services`]) run in the launcher.
//!
//! Each plugin runs on a thread of its own ([`spawn`]) that owns its instance: the page
//! sends it what happened ([`Cmd`]) and draws what it answers ([`Out`]). A call that runs
//! too long (an endless loop) or a plugin that uses too much memory is stopped by
//! wasmtime, and the launcher keeps running: the page offers to start it again.

mod imports;
pub mod services;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::Value;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use crate::core::launcher::plugin_page::PluginPage;
use crate::core::launcher::plugin_permissions::Permissions;

wasmtime::component::bindgen!({
    world: "plugin",
    path: "wit",
});

pub use echovr::launcher::types::{self, Node, Tone};

/// How often wasmtime's clock ticks: a call's time budget is counted in these.
const TICK: Duration = Duration::from_millis(10);
/// `init` may take this long, every other call [`CALL_BUDGET`].
const INIT_BUDGET: Duration = Duration::from_secs(2);
const CALL_BUDGET: Duration = Duration::from_millis(500);
/// A plugin's memory.
pub const MEMORY: usize = 64 << 20;
/// How often a crashed plugin may be started again in a run of the launcher.
pub const RESTARTS: u32 = 3;
/// What a page may hold, at most.
const MAX_NODES: usize = 4000;
const MAX_TEXT: usize = 20_000;

/// The engine all plugins share, and the thread that makes its clock tick.
fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut c = Config::new();
        c.wasm_component_model(true);
        c.epoch_interruption(true);
        c.max_wasm_stack(1 << 20);
        let e = Engine::new(&c).expect("the plugins' WebAssembly engine");
        let ticking = e.clone();
        let _ = std::thread::Builder::new()
            .name("plugin-clock".into())
            .spawn(move || loop {
                std::thread::sleep(TICK);
                ticking.increment_epoch();
            });
        e
    })
}

fn ticks(d: Duration) -> u64 {
    (d.as_millis() / TICK.as_millis()).max(1) as u64
}

/// The compiled component of `wasm`: from the plugin's cache (`<plugin>/.compiled/`) when
/// it was compiled from exactly these bytes by this launcher, else compiled now and kept.
pub fn component(wasm: &Path) -> Result<Component> {
    let bytes = std::fs::read(wasm).with_context(|| format!("read {}", wasm.display()))?;
    let sha = sha256_hex(&bytes);
    let cache = wasm
        .parent()
        .map(|d| d.join(".compiled"))
        .unwrap_or_else(|| PathBuf::from(".compiled"));
    let name = format!(
        "{}-{}-{}.cwasm",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        &sha[..32]
    );
    let cached = cache.join(&name);
    let check = cache.join(format!("{name}.sha256"));
    if let (Ok(c), Ok(want)) = (std::fs::read(&cached), std::fs::read_to_string(&check)) {
        if sha256_hex(&c) == want.trim() {
            // SAFETY: the file is the launcher's own serialization of this very component
            // (its name holds the component's checksum, its own checksum is beside it),
            // and wasmtime checks that it was made by this engine's build.
            match unsafe { Component::deserialize(engine(), &c) } {
                Ok(comp) => return Ok(comp),
                Err(e) => tracing::info!("plugin cache {}: {e:#}", cached.display()),
            }
        }
    }
    let comp = Component::new(engine(), &bytes).map_err(|e| {
        anyhow::anyhow!(
            "{} isn't a launcher plugin component: {e:#}",
            wasm.display()
        )
    })?;
    if let Ok(ser) = comp.serialize() {
        let _ = std::fs::remove_dir_all(&cache);
        if std::fs::create_dir_all(&cache).is_ok() {
            let _ = std::fs::write(&cached, &ser);
            let _ = std::fs::write(&check, sha256_hex(&ser));
        }
    }
    Ok(comp)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Snapshots: a made-up answer (status, body) to a request to an address.
pub type FakeNet = fn(&str) -> (u16, String);

/// What the launcher tells a running plugin, kept up to date by the page while it runs.
#[derive(Debug, Clone, Default)]
pub struct Shared {
    /// What `host.launcher-info` answers (JSON, no secrets).
    pub info: Value,
    /// The classic lobbies server (`host:port`) and the player's account there: the
    /// password is filled into requests to that server only, never handed to the plugin.
    pub relay_server: String,
    pub relay_name: String,
    pub relay_password: String,
    /// What the player allowed it.
    pub granted: Permissions,
    /// Snapshots: made-up answers to its requests (by address) instead of the network.
    pub fake_net: Option<FakeNet>,
}

/// What a plugin is, for its thread.
#[derive(Clone)]
pub struct Spec {
    pub page: Arc<PluginPage>,
    pub dir: PathBuf,
    /// Its files are signed by the launcher's makers (services need that).
    pub first_party: bool,
}

impl Spec {
    pub fn id(&self) -> &str {
        &self.page.id
    }
}

/// What the page sends a plugin.
#[derive(Debug, Clone)]
pub enum Cmd {
    Event(UiEvent),
    /// Start it again after it stopped.
    Restart,
    Stop,
}

/// Something that happened on the page, or an answer for the plugin.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Clicked(String),
    Changed(String, String),
    Toggled(String, bool),
    Tab(String, u32),
    Dialog(String, u32),
    NetDone(u32, std::result::Result<(u16, String), String>),
    ServiceDone(u32, std::result::Result<String, String>),
    SettingsChanged,
    LauncherChanged,
    Shown,
    Hidden,
}

impl UiEvent {
    fn to_wit(&self) -> Event {
        match self.clone() {
            UiEvent::Clicked(id) => Event::Clicked(id),
            UiEvent::Changed(id, v) => Event::Changed((id, v)),
            UiEvent::Toggled(id, on) => Event::Toggled((id, on)),
            UiEvent::Tab(id, i) => Event::Tab((id, i)),
            UiEvent::Dialog(id, i) => Event::Dialog((id, i)),
            UiEvent::NetDone(t, r) => {
                Event::NetDone((t, r.map(|(status, body)| types::Response { status, body })))
            }
            UiEvent::ServiceDone(t, r) => Event::ServiceDone((t, r)),
            UiEvent::SettingsChanged => Event::SettingsChanged,
            UiEvent::LauncherChanged => Event::LauncherChanged,
            UiEvent::Shown => Event::Shown,
            UiEvent::Hidden => Event::Hidden,
        }
    }

    /// The player clicked (what lets a plugin copy).
    fn is_click(&self) -> bool {
        matches!(
            self,
            UiEvent::Clicked(_) | UiEvent::Toggled(..) | UiEvent::Tab(..) | UiEvent::Dialog(..)
        )
    }
}

/// A server a plugin asked Echo VR to play on (`game.set-server`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerAsk {
    pub kind: String,
    pub name: String,
    pub address: String,
    pub key: String,
    pub discord_id: String,
    pub display_name: String,
    pub password: String,
}

/// What a plugin asks of the page.
#[derive(Debug, Clone)]
pub enum Out {
    /// Its page, to draw until the next one.
    Page(Page),
    Notify(String, Tone),
    Ask {
        id: String,
        title: String,
        text: String,
        buttons: Vec<String>,
        warn: bool,
    },
    Copy(String),
    OpenUrl(String),
    Play {
        version: String,
        args: Vec<String>,
    },
    /// The player is asked; the answer goes back as `ServiceDone(ticket, …)`.
    SetServer {
        ticket: u32,
        server: ServerAsk,
    },
    ResetServer,
    /// Its settings changed (forms read them again).
    SettingsChanged,
    /// A service operation that runs in the launcher ([`services::start`]); its answer
    /// goes back as `ServiceDone(ticket, …)`.
    Service {
        ticket: u32,
        op: String,
        args: Value,
    },
    /// A log stream for a `log-view`: a server's console.
    OpenLog {
        log: u32,
        server: String,
    },
    CopyLog {
        log: u32,
        filter: String,
    },
    /// It stopped: why, and whether it may be started again.
    Stopped {
        why: String,
        restart: bool,
    },
}

/// Wakes the page when a plugin has something for it (egui's repaint).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A running plugin, as its page holds it. Dropping it stops the plugin.
pub struct Running {
    tx: Sender<Cmd>,
    pub rx: Receiver<Out>,
    pub shared: Arc<Mutex<Shared>>,
}

impl Running {
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    pub fn event(&self, e: UiEvent) {
        self.send(Cmd::Event(e));
    }

    /// Something for the plugin from elsewhere (a worker's answer).
    pub fn sender(&self) -> Sender<Cmd> {
        self.tx.clone()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

/// Starts plugin `spec` on a thread of its own.
pub fn spawn(spec: Spec, shared: Shared, wake: Wake) -> Running {
    let (tx, cmd_rx) = mpsc::channel();
    let (out_tx, rx) = mpsc::channel();
    let shared = Arc::new(Mutex::new(shared));
    let s = shared.clone();
    let own = tx.clone();
    let name = format!("plugin-{}", spec.id());
    let started = std::thread::Builder::new().name(name).spawn(move || {
        run(spec, s, own, cmd_rx, out_tx, wake);
    });
    if let Err(e) = started {
        tracing::warn!("plugin thread: {e}");
    }
    Running { tx, rx, shared }
}

/// The store's data: what the imports see of the plugin and the launcher.
pub struct HostState {
    spec: Spec,
    shared: Arc<Mutex<Shared>>,
    out: Sender<Out>,
    wake: Wake,
    /// Answers for the plugin come back through its own queue.
    own: Sender<Cmd>,
    limits: StoreLimits,
    /// It asked to be drawn again.
    dirty: bool,
    last_click: Option<Instant>,
    /// Requests sent right after a click: their answer counts as that click (a Join that
    /// asks the server first, then starts the game).
    clicked_tickets: std::collections::HashSet<u32>,
    timers: BTreeMap<u32, (Instant, Option<Duration>)>,
    next_ticket: u32,
    in_flight: usize,
    notices: Vec<Instant>,
    storage: Option<BTreeMap<String, String>>,
    logs: HashMap<u32, String>,
}

impl HostState {
    fn new(
        spec: Spec,
        shared: Arc<Mutex<Shared>>,
        out: Sender<Out>,
        wake: Wake,
        own: Sender<Cmd>,
    ) -> Self {
        HostState {
            spec,
            shared,
            out,
            wake,
            own,
            limits: StoreLimitsBuilder::new()
                .memory_size(MEMORY)
                .instances(4)
                .tables(8)
                .memories(2)
                .trap_on_grow_failure(true)
                .build(),
            dirty: true,
            last_click: None,
            clicked_tickets: Default::default(),
            timers: BTreeMap::new(),
            next_ticket: 1,
            in_flight: 0,
            notices: Vec::new(),
            storage: None,
            logs: HashMap::new(),
        }
    }

    fn send(&self, o: Out) {
        let _ = self.out.send(o);
        (self.wake)();
    }

    fn ticket(&mut self) -> u32 {
        let t = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1);
        t
    }

    /// What it may do now: what it declared and the player allowed.
    fn may(&self) -> Permissions {
        let granted = self
            .shared
            .lock()
            .map(|s| s.granted.clone())
            .unwrap_or_default();
        crate::core::launcher::plugin_permissions::Permissions::of(&self.spec.page).within(&granted)
    }
}

/// The plugin's thread: its instance, started again when asked after a crash.
fn run(
    spec: Spec,
    shared: Arc<Mutex<Shared>>,
    own: Sender<Cmd>,
    rx: Receiver<Cmd>,
    out: Sender<Out>,
    wake: Wake,
) {
    let say = |o: Out| {
        let _ = out.send(o);
        (wake)();
    };
    let comp = match component(&spec.dir.join(&spec.page.wasm)) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("plugin {}: {e:#}", spec.id());
            return say(Out::Stopped {
                why: format!("{e:#}"),
                restart: false,
            });
        }
    };
    let mut linker: Linker<HostState> = Linker::new(engine());
    if let Err(e) = Plugin::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s) {
        return say(Out::Stopped {
            why: format!("{e:#}"),
            restart: false,
        });
    }
    let mut restarts = 0;
    loop {
        let state = HostState::new(
            spec.clone(),
            shared.clone(),
            out.clone(),
            wake.clone(),
            own.clone(),
        );
        let why = match instance(&comp, &linker, state, &rx) {
            Ended::Stop => return,
            Ended::Crashed(why) => why,
        };
        tracing::warn!("plugin {} stopped: {why}", spec.id());
        let restart = restarts < RESTARTS;
        say(Out::Stopped { why, restart });
        if !restart {
            return;
        }
        // Until the page asks for it to start again.
        loop {
            match rx.recv() {
                Ok(Cmd::Restart) => break,
                Ok(Cmd::Stop) | Err(_) => return,
                Ok(Cmd::Event(_)) => {}
            }
        }
        restarts += 1;
    }
}

enum Ended {
    Stop,
    Crashed(String),
}

/// Why a call failed, for the player.
fn why(e: &wasmtime::Error) -> String {
    if let Some(t) = e.downcast_ref::<wasmtime::Trap>() {
        return match t {
            wasmtime::Trap::Interrupt => {
                "it took too long (it doesn't answer: an endless loop?)".to_string()
            }
            wasmtime::Trap::StackOverflow => "it ran out of stack".to_string(),
            wasmtime::Trap::UnreachableCodeReached => "it crashed (a panic)".to_string(),
            other => format!("it crashed ({other})"),
        };
    }
    let text = format!("{e:#}");
    if text.contains("memory") || text.contains("grow") {
        format!("it used too much memory ({} MB at most)", MEMORY >> 20)
    } else {
        format!("it crashed ({text})")
    }
}

/// One instance, until it is stopped or crashes.
fn instance(
    comp: &Component,
    linker: &Linker<HostState>,
    state: HostState,
    rx: &Receiver<Cmd>,
) -> Ended {
    let mut store = Store::new(engine(), state);
    store.limiter(|s| &mut s.limits);
    store.epoch_deadline_trap();
    store.set_epoch_deadline(ticks(INIT_BUDGET));
    let plugin = match Plugin::instantiate(&mut store, comp, linker) {
        Ok(p) => p,
        Err(e) => return Ended::Crashed(why(&e)),
    };
    let info = store
        .data()
        .shared
        .lock()
        .map(|s| imports::info_text(&s, &store.data().spec))
        .unwrap_or_default();
    match plugin.call_init(&mut store, &info) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Ended::Crashed(format!("it didn't start: {e}")),
        Err(e) => return Ended::Crashed(why(&e)),
    }
    loop {
        if store.data().dirty {
            store.data_mut().dirty = false;
            store.set_epoch_deadline(ticks(CALL_BUDGET));
            match plugin.call_render(&mut store) {
                Ok(page) => {
                    let page = checked(page);
                    store.data().send(Out::Page(page));
                }
                Err(e) => return Ended::Crashed(why(&e)),
            }
        }
        let next = store.data().timers.values().map(|(at, _)| *at).min();
        let cmd = match next {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ended::Stop,
            },
            None => match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return Ended::Stop,
            },
        };
        store.set_epoch_deadline(ticks(CALL_BUDGET));
        match cmd {
            Some(Cmd::Stop) => return Ended::Stop,
            Some(Cmd::Restart) => {}
            Some(Cmd::Event(e)) => {
                if e.is_click() {
                    store.data_mut().last_click = Some(Instant::now());
                }
                if let UiEvent::NetDone(ticket, _) = e {
                    let d = store.data_mut();
                    d.in_flight = d.in_flight.saturating_sub(1);
                    if d.clicked_tickets.remove(&ticket) {
                        d.last_click = Some(Instant::now());
                    }
                }
                if let Err(e) = plugin.call_on_event(&mut store, &e.to_wit()) {
                    return Ended::Crashed(why(&e));
                }
                // What it shows may follow from what happened.
                store.data_mut().dirty = true;
            }
            None => {
                let now = Instant::now();
                let due: Vec<u32> = store
                    .data()
                    .timers
                    .iter()
                    .filter(|(_, (at, _))| *at <= now)
                    .map(|(id, _)| *id)
                    .collect();
                for id in due {
                    let d = store.data_mut();
                    match d.timers.get(&id).and_then(|(_, every)| *every) {
                        Some(every) => {
                            d.timers.insert(id, (now + every, Some(every)));
                        }
                        None => {
                            d.timers.remove(&id);
                        }
                    }
                    store.set_epoch_deadline(ticks(CALL_BUDGET));
                    if let Err(e) = plugin.call_on_timer(&mut store, id) {
                        return Ended::Crashed(why(&e));
                    }
                }
            }
        }
    }
}

/// Pure: `page` as the launcher draws it: at most [`MAX_NODES`] nodes, texts cut at
/// [`MAX_TEXT`], children only by indices in range (the drawer stops at a depth limit, so
/// a node that names itself does no harm: see `ui::launcher::plugin_ui`).
pub fn checked(mut page: Page) -> Page {
    page.nodes.truncate(MAX_NODES);
    let n = page.nodes.len() as u32;
    let cut = |s: &mut String| {
        if s.len() > MAX_TEXT {
            let mut end = MAX_TEXT;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
        }
    };
    for node in &mut page.nodes {
        match node {
            Node::Card(c) => {
                cut(&mut c.title);
                cut(&mut c.help);
                c.children.retain(|i| *i < n);
            }
            Node::Tabs(t) => t.children.retain(|i| *i < n),
            Node::Text(t) => cut(&mut t.text),
            Node::Value(v) => cut(&mut v.value),
            Node::Table(t) => t.rows.truncate(500),
            _ => {}
        }
    }
    page.columns.truncate(2);
    for c in &mut page.columns {
        c.retain(|i| *i < n);
    }
    page
}
