//! Launcher plugins in the UI: each installed plugin's tab (its page, drawn from its
//! description: see `core::launcher::plugin_page`), and the Plugins page (the rail's puzzle piece),
//! the library where plugins are got from the catalogue, pinned, updated and removed.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use eframe::egui;
use serde_json::{json, Map, Value};

use super::plugin_form::{self, FormState};
use super::{empty_state, hero, panel, play, Dashboard, Page};
use crate::core::launcher::echoxr_settings;
use crate::core::launcher::plugin_page::{
    self as pp, Block, Button, Card, Method, PluginPage, Request, Step, Tone as TextTone,
};
use crate::core::launcher::plugin_settings::{Settings, ECHOXR_FILE};
use crate::core::launcher::plugins::{self as core, Installed, PluginCatalog};
use crate::core::launcher::presets::{self, Match, UserPreset};
use crate::core::launcher::relay;
use crate::core::launcher::store::{GameServer, PackServer, ServerKind};
use crate::ui::design::{self, dz, Dr};
use crate::ui::dialogs::Icon as DlgIcon;
use crate::ui::kit::Kit;
use crate::ui::parts::Worker;
use crate::ui::style::{icon_at, Icon};
use crate::ui::widgets::{MenuItem, Tone, BTN_H};

/// The page's two columns: the wide left one and the right-hand panel's.
const LEFT: Dr = Dr::new(137.0, 156.0, 1146.0, 890.0);
const RIGHT: Dr = Dr::new(1318.0, 156.0, 555.0, 890.0);
/// One column across the page.
const WHOLE: Dr = Dr::new(137.0, 156.0, 1736.0, 890.0);
const CARD_GAP: f32 = 30.0;
const ROW_H: f32 = 68.0;
const ROW_GAP: f32 = 10.0;
const REMOVE_KEY: &str = "plugins-remove";
/// "Play on another server?" (a plugin's `server` step).
const SERVER_KEY: &str = "plugins-server";
/// A settings form's button that asks first.
const FORM_ACTION_KEY: &str = "plugins-form-action";
/// "Replace your preset?" and "Delete this preset?".
const PRESET_REPLACE_KEY: &str = "plugins-preset-replace";
const PRESET_DELETE_KEY: &str = "plugins-preset-delete";

/// The plugins, their catalogue, and each plugin page's state.
#[derive(Default)]
pub(super) struct PluginsUi {
    /// The installed plugins, by name (their tabs, in this order).
    pub list: Vec<Installed>,
    catalog: Option<PluginCatalog>,
    /// When the catalogue was read (the built-in one is replaced when it can be).
    catalog_at: Option<std::time::Instant>,
    started: bool,
    /// The library, scrolled (logical pixels), and which plugins it shows.
    library_scroll: f32,
    filter: Filter,
    /// The plugin whose card menu is open, and where (its ⋯'s bottom right).
    card_menu: Option<String>,
    card_menu_at: (f32, f32),
    views: HashMap<String, View>,
    worker: Worker<Msg>,
    /// The plugin being installed, updated or removed.
    busy: Option<String>,
    /// The catalogue is being read again (Refresh).
    loading: bool,
    /// Debug builds: the plugin ECHOVR_PAGE named, opened once it is read.
    pub open_when_read: Option<String>,
    remove_asked: Option<String>,
    /// The server a plugin's `server` step asked to play on (its id, the server), while the
    /// question is up.
    server_asked: Option<(String, GameServer)>,
    /// A plugin page's question that is up (its id, what was asked).
    asked: Option<(String, Asked)>,
    /// The plugin whose rail menu (right click) is open.
    pub rail_menu: Option<String>,
    /// How far the rail's plugin tabs are scrolled (design pixels).
    pub rail_scroll: f32,
    /// The page the rail last kept in view (its tab scrolls into view when it changes).
    pub rail_seen: Option<Page>,
    /// The plugin the Plugins page scrolls to and marks (Plugin details).
    pub focus: Option<String>,
}

/// What a plugin page's settings blocks asked.
#[derive(Clone)]
enum Asked {
    /// A form's button confirms its values (the form keeps them).
    FormAction,
    /// Replace the player's preset of this name.
    Replace(String),
    /// Delete the player's preset at this index.
    Delete(usize),
}

impl PluginsUi {
    /// The plugins and their catalogue are read again (a dev code came or went).
    pub fn reload(&mut self) {
        self.started = false;
        self.catalog = None;
    }
}

enum Msg {
    Read(Vec<Installed>),
    Catalog(PluginCatalog),
    Fetched {
        plugin: String,
        source: String,
        result: Result<Value, Failure>,
    },
    Answered {
        plugin: String,
        result: Result<Value, String>,
    },
    Done(String, Result<(), String>),
}

/// A plugin page while the launcher runs.
#[derive(Default)]
struct View {
    /// Its `page.` values.
    page: Value,
    data: Map<String, Value>,
    fetched: HashMap<String, Instant>,
    in_flight: HashSet<String>,
    settings: BTreeMap<String, String>,
    settings_read: bool,
    /// Fields being typed in, by what they bind to.
    drafts: HashMap<String, String>,
    runner: Option<Runner>,
    scroll: [f32; 2],
    /// Its settings forms: Show advanced, what is typed, a button waiting for its yes.
    form: FormState,
    /// The player's presets, read when the page first shows.
    user_presets: Option<Vec<UserPreset>>,
    /// What "Save" keeps the settings as.
    preset_name: String,
    /// What the presets card last did ("Applied: …").
    preset_note: String,
}

/// An action running: its steps, the next one, and the list row it was started on.
struct Runner {
    steps: Vec<Step>,
    next: usize,
    item: Value,
    /// A request is out.
    waiting: bool,
}

// ---- reading, fetching and acting (each frame) ----

pub(super) fn tick(d: &mut Dashboard, ctx: &egui::Context) {
    if d.page != Page::Plugins {
        d.plugins.focus = None;
    }
    if d.demo {
        if !d.plugins.started {
            d.plugins = demo();
        }
        // The plugin's tab shows what an installed event build gets.
        if matches!(d.page, Page::Plugin(_))
            && !d.state.versions.iter().any(|v| v.publisher_lock.is_some())
        {
            d.state
                .versions
                .push(crate::core::launcher::store::InstalledVersion {
                    id: "pc-halloween-2018".into(),
                    name: "Halloween 2018".into(),
                    catalog_id: Some("pc-halloween-2018".into()),
                    publisher_lock: Some("rad15_halloween".into()),
                    ..Default::default()
                });
            d.state.relay_account = Some(crate::core::launcher::store::RelayAccount {
                name: "Alice".into(),
                password: "demo".into(),
            });
        }
        return;
    }
    if !d.plugins.started {
        d.plugins.started = true;
        reread(d, ctx);
        d.plugins
            .worker
            .spawn(ctx, |tx| tx.send(Msg::Catalog(PluginCatalog::load())));
    }
    // The built-in list (offline at the start): the published one is looked for again
    // every five minutes.
    let retry = d.plugins.catalog.as_ref().is_some_and(|c| c.builtin)
        && d.plugins
            .catalog_at
            .is_some_and(|t| t.elapsed() >= std::time::Duration::from_secs(5 * 60));
    if retry {
        d.plugins.catalog_at = None;
        d.plugins
            .worker
            .spawn(ctx, |tx| tx.send(Msg::Catalog(PluginCatalog::load())));
    }
    for m in d.plugins.worker.drain() {
        match m {
            Msg::Read(list) => {
                d.plugins.list = list;
                d.plugins
                    .views
                    .retain(|id, _| d.plugins.list.iter().any(|p| &p.page.id == id));
                if let Some(id) = d.plugins.open_when_read.take() {
                    if let Some(i) = d.plugins.list.iter().position(|p| p.page.id == id) {
                        d.page = Page::Plugin(i as u8);
                    }
                }
                if let Page::Plugin(i) = d.page {
                    if i as usize >= d.plugins.list.len() {
                        d.page = Page::Plugins;
                    }
                }
                // The plugin that chose the game's server is gone: EchoVRCE again (nothing
                // else would show the way back).
                let gone = d
                    .state
                    .game_server
                    .as_ref()
                    .is_some_and(|g| !d.plugins.list.iter().any(|p| p.page.id == g.plugin));
                if gone {
                    if let Some(g) = d.state.game_server.take() {
                        tracing::info!(
                            "plugin {} is gone: Echo VR plays on EchoVRCE again",
                            g.plugin
                        );
                    }
                    d.save();
                    back_to_nevr(d);
                    d.notify("Echo VR plays on EchoVRCE again");
                }
            }
            Msg::Catalog(c) => catalog_done(d, c),
            Msg::Fetched {
                plugin,
                source,
                result,
            } => {
                let v = d.plugins.views.entry(plugin).or_default();
                v.in_flight.remove(&source);
                match result {
                    Ok(value) => {
                        v.data.insert(source, value);
                    }
                    Err(f) => {
                        tracing::info!("plugin data {source} ({}): {}", f.kind, f.message);
                        v.data
                            .insert(source, json!({"error": f.message, "failed": f.kind}));
                    }
                }
            }
            Msg::Answered { plugin, result } => answered(d, ctx, &plugin, result),
            Msg::Done(what, result) => {
                d.plugins.busy = None;
                match result {
                    Ok(()) => d.notify(&what),
                    Err(e) => d.dialogs.error("Plugins", &e, Default::default()),
                }
                reread(d, ctx);
            }
        }
    }
    if let Some(ans) = d.dialogs.take(REMOVE_KEY) {
        if let (true, Some(id)) = (ans.is_yes(), d.plugins.remove_asked.take()) {
            // Got again later, it starts on the rail.
            d.state.rail_order.retain(|o| *o != id);
            d.state.rail_unpinned.remove(&id);
            d.save();
            d.plugins.busy = Some(id.clone());
            d.plugins.worker.spawn(ctx, move |tx| {
                let r = core::remove(&id).map_err(|e| format!("{e:#}"));
                tx.send(Msg::Done("Plugin removed".into(), r));
            });
        }
    }
    if let Some(ans) = d.dialogs.take(SERVER_KEY) {
        if let Some((id, server)) = d.plugins.server_asked.take() {
            let p = d.plugins.list.iter().find(|p| p.page.id == id).cloned();
            match p {
                Some(p) if ans.is_yes() => {
                    tracing::info!(
                        "plugin {id}: the live build plays on {} (the player said yes)",
                        server.server.address
                    );
                    let to_nakama = server.kind == ServerKind::Nakama;
                    d.state.game_server = Some(server);
                    d.save();
                    if to_nakama {
                        back_to_nevr(d);
                    }
                    d.notify("Echo VR's server changed");
                    if let Some(r) = d.plugins.views.get_mut(&id).and_then(|v| v.runner.as_mut()) {
                        r.waiting = false;
                    }
                    advance(d, ctx, &p);
                }
                _ => fail(d, &id, "Echo VR's server wasn't changed.".into()),
            }
        }
    }
    // The shown plugin page's data, when due.
    if let Page::Plugin(i) = d.page {
        if let Some(p) = d.plugins.list.get(i as usize).cloned() {
            fetch_due(d, ctx, &p);
        }
    }
}

/// The catalogues read (at the start, by Refresh, every 30 seconds, or with an update
/// check): the Plugins page offers what they list. Advanced settings' line about the
/// dev folder is read again too.
pub(super) fn catalog_done(d: &mut Dashboard, c: PluginCatalog) {
    d.plugins.catalog = Some(c);
    d.plugins.catalog_at = Some(std::time::Instant::now());
    d.plugins.loading = false;
    if d.dev_status != crate::core::launcher::dev_code::Status::Checking {
        d.dev_status = crate::core::launcher::dev_code::Status::Off;
    }
}

/// Refresh: the installed plugins and the catalogues (with the dev folder's) read again now.
fn refresh(d: &mut Dashboard, ctx: &egui::Context) {
    if d.plugins.loading {
        return;
    }
    d.plugins.loading = true;
    reread(d, ctx);
    d.plugins
        .worker
        .spawn(ctx, |tx| tx.send(Msg::Catalog(PluginCatalog::load())));
}

fn reread(d: &mut Dashboard, ctx: &egui::Context) {
    d.plugins
        .worker
        .spawn(ctx, |tx| tx.send(Msg::Read(core::installed())));
}

/// The context a plugin's text is filled from.
fn context(d: &Dashboard, p: &Installed) -> Value {
    let v = d.plugins.views.get(&p.page.id);
    let settings: Map<String, Value> = v
        .map(|v| {
            v.settings
                .iter()
                .map(|(k, x)| (k.clone(), Value::String(x.clone())))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "settings": settings,
        "page": v.map(|v| v.page.clone()).unwrap_or_else(|| json!({})),
        "data": v.map(|v| Value::Object(v.data.clone())).unwrap_or_else(|| json!({})),
        "launcher": core::launcher_context(&d.state),
    })
}

fn view<'a>(d: &'a mut Dashboard, p: &Installed) -> &'a mut View {
    let v = d.plugins.views.entry(p.page.id.clone()).or_default();
    if !v.settings_read {
        v.settings_read = true;
        v.settings = core::settings_values(p);
        if !v.page.is_object() {
            v.page = json!({});
        }
    }
    v
}

/// Why a request failed: its message, and what kind of failure it was, as a page sees it
/// (`data.<name>.failed`): `unreachable` (no connection to the server, or no answer in
/// time), `missing` (the server doesn't have that address: 404, 405 or 501) or `server`
/// (any other error answer).
struct Failure {
    message: String,
    kind: &'static str,
}

impl Failure {
    /// A failed GET: `http::get_text` keeps reqwest's error when nothing came back, and
    /// says "server responded with <status>" for an error answer.
    fn of_get(e: &anyhow::Error) -> Failure {
        let message = format!("{e:#}");
        let no_answer = e
            .chain()
            .any(|c| c.downcast_ref::<reqwest::Error>().is_some());
        let kind = if no_answer {
            "unreachable"
        } else if ["404", "405", "501"]
            .iter()
            .any(|code| message.contains(&format!("responded with {code}")))
        {
            "missing"
        } else {
            "server"
        };
        Failure { message, kind }
    }
}

/// Sends a request on the plugin worker; `f` makes the message from its answer.
fn send(
    d: &Dashboard,
    ctx: &egui::Context,
    r: &Request,
    c: &Value,
    f: impl FnOnce(Result<Value, Failure>) -> Msg + Send + 'static,
) {
    let url = pp::render(&r.url, c);
    let body = r.body.as_ref().map(|b| pp::render_body(b, c));
    let method = r.method;
    d.plugins.worker.spawn(ctx, move |tx| {
        let result = match method {
            Method::Get => crate::core::http::get_text(&url).map_err(|e| Failure::of_get(&e)),
            // post_json fails only when no answer came back.
            Method::Post => crate::core::http::post_json(&url, &body.unwrap_or(Value::Null))
                .map_err(|e| Failure {
                    message: format!("{e:#}"),
                    kind: "unreachable",
                })
                .and_then(|(status, text)| {
                    if status >= 500 {
                        Err(Failure {
                            message: format!("the server answered {status}"),
                            kind: "server",
                        })
                    } else {
                        Ok(text)
                    }
                }),
        }
        .map(|text| serde_json::from_str(&text).unwrap_or(Value::String(text)));
        tx.send(f(result));
    });
}

fn fetch_due(d: &mut Dashboard, ctx: &egui::Context, p: &Installed) {
    view(d, p);
    let c = context(d, p);
    let mut due = Vec::new();
    {
        let v = d.plugins.views.get(&p.page.id).expect("its view");
        for s in &p.page.data {
            if v.in_flight.contains(&s.name) || !pp::truthy(&s.when, &c) {
                continue;
            }
            let ready = match v.fetched.get(&s.name) {
                None => true,
                Some(at) => s.every > 0 && at.elapsed().as_secs() >= s.every,
            };
            if ready {
                due.push(s.clone());
            }
        }
    }
    // Drawn again when the next one is due, so the page refetches (and an error goes once
    // the server answers again) with nobody touching the window.
    if let Some(next) = p
        .page
        .data
        .iter()
        .filter(|s| s.every > 0)
        .map(|s| s.every)
        .min()
    {
        ctx.request_repaint_after(std::time::Duration::from_secs(next));
    }
    for s in due {
        let v = d.plugins.views.get_mut(&p.page.id).expect("its view");
        v.in_flight.insert(s.name.clone());
        v.fetched.insert(s.name.clone(), Instant::now());
        let (plugin, source) = (p.page.id.clone(), s.name.clone());
        send(d, ctx, &s.request, &c, move |result| Msg::Fetched {
            plugin,
            source,
            result,
        });
    }
}

/// Starts action `name` of plugin `p` (on list row `item`, if any).
fn act(d: &mut Dashboard, ctx: &egui::Context, p: &Installed, name: &str, item: Value) {
    let Some(steps) = p.page.actions.get(name).cloned() else {
        return;
    };
    let v = view(d, p);
    if v.runner.is_some() {
        return;
    }
    if let Some(m) = v.page.as_object_mut() {
        m.remove("error");
    }
    v.runner = Some(Runner {
        steps,
        next: 0,
        item,
        waiting: false,
    });
    advance(d, ctx, p);
}

fn fail(d: &mut Dashboard, id: &str, why: String) {
    if let Some(v) = d.plugins.views.get_mut(id) {
        v.runner = None;
        pp::set_path(&mut v.page, "error", Value::String(why));
    }
}

/// Runs the action's steps until one waits for an answer, or it ends.
fn advance(d: &mut Dashboard, ctx: &egui::Context, p: &Installed) {
    let id = p.page.id.clone();
    loop {
        let Some((step, item)) = d.plugins.views.get(&id).and_then(|v| {
            let r = v.runner.as_ref()?;
            Some((r.steps.get(r.next).cloned(), r.item.clone()))
        }) else {
            return;
        };
        let Some(step) = step else {
            d.plugins.views.get_mut(&id).expect("its view").runner = None;
            return;
        };
        let c = pp::with_item(&context(d, p), &item);
        if let Some(r) = d.plugins.views.get_mut(&id).and_then(|v| v.runner.as_mut()) {
            r.next += 1;
        }
        match step {
            Step::Request { request, .. } => {
                if let Some(r) = d.plugins.views.get_mut(&id).and_then(|v| v.runner.as_mut()) {
                    r.waiting = true;
                }
                let plugin = id.clone();
                send(d, ctx, &request, &c, move |result| Msg::Answered {
                    plugin,
                    result: result.map_err(|f| f.message),
                });
                return;
            }
            Step::Require { when, otherwise } => {
                if !pp::truthy(&when, &c) {
                    let why = pp::render(&otherwise, &c);
                    return fail(
                        d,
                        &id,
                        if why.is_empty() {
                            "That didn't work.".into()
                        } else {
                            why
                        },
                    );
                }
            }
            Step::Set { path, to } => {
                let value = match &to {
                    Value::String(t) => pp::render_value(t, &c),
                    other => pp::render_body(other, &c),
                };
                set_bound(d, p, &path, value);
            }
            Step::Play { version, .. } => {
                let name = pp::render(&version, &c);
                let event = |vid: &str| {
                    d.state
                        .versions
                        .iter()
                        .any(|v| v.id == vid && v.publisher_lock.is_some())
                };
                match core::version_for(&d.state, &name) {
                    // As PLAY: on Linux they start in VR only.
                    Some(vid)
                        if cfg!(target_os = "linux")
                            && event(&vid)
                            && d.state.profile.runtime
                                == crate::core::launcher::store::Runtime::Flat =>
                    {
                        return fail(
                            d,
                            &id,
                            "Event builds always start in VR: choose SteamVR or WiVRn in Settings."
                                .into(),
                        );
                    }
                    Some(vid) => {
                        d.state.selected = Some(vid);
                        d.save();
                        play::try_start(d, ctx, None);
                    }
                    None => {
                        return fail(
                            d,
                            &id,
                            format!("{name} isn't installed: the Install page has it."),
                        )
                    }
                }
            }
            Step::Copy { text } => {
                ctx.copy_text(pp::render(&text, &c));
                d.notify("Copied");
            }
            Step::Refresh { sources } => {
                if let Some(v) = d.plugins.views.get_mut(&id) {
                    for s in sources {
                        v.fetched.remove(&s);
                    }
                }
            }
            Step::Host {
                fields,
                mods,
                version,
                save,
            } => {
                let field = |k: &str| fields.get(k).map(|t| pp::render(t, &c)).unwrap_or_default();
                let host = crate::core::launcher::host::Host {
                    folder: field("folder").trim().to_string(),
                    socket_uri: field("socket_uri"),
                    serverdb_uri: field("serverdb_uri"),
                    http_uri: field("http_uri"),
                    http_key: field("http_key"),
                    server_key: field("server_key"),
                    discord_id: field("discord_id"),
                    password: field("password"),
                    regions: field("regions"),
                    guilds: field("guilds"),
                    port: field("port"),
                    mods,
                };
                let name = pp::render(&version, &c);
                let v = if name.trim().is_empty() {
                    match d.state.target(None, |v| v.present()) {
                        crate::core::launcher::store::Target::Installed(v)
                            if v.publisher_lock.is_none() =>
                        {
                            Some(v)
                        }
                        _ => d
                            .state
                            .versions
                            .iter()
                            .find(|v| v.publisher_lock.is_none() && v.present())
                            .cloned(),
                    }
                } else {
                    core::version_for(&d.state, name.trim())
                        .and_then(|vid| d.state.version(&vid).cloned())
                };
                let Some(v) = v else {
                    return fail(
                        d,
                        &id,
                        "Install the live build first: a game server runs from it.".into(),
                    );
                };
                match crate::core::launcher::host::write(&v, &host) {
                    Ok(written) => {
                        tracing::info!(
                            "plugin {id}: a game server's setup in {}",
                            written.dir.display()
                        );
                        if !save.is_empty() {
                            set_bound(d, p, &save, written.to_value());
                        }
                    }
                    Err(e) => return fail(d, &id, format!("{e:#}")),
                }
            }
            Step::Server {
                kind,
                name,
                address,
                key,
                discord_id,
                display_name,
                password,
            } => {
                let kind = if pp::render(&kind, &c).trim() == "echorelay" {
                    ServerKind::EchoRelay
                } else {
                    ServerKind::Nakama
                };
                let server = GameServer {
                    name: pp::render(&name, &c).trim().to_string(),
                    kind,
                    server: PackServer {
                        address: pp::render(&address, &c).trim().to_string(),
                        server_key: pp::render(&key, &c).trim().to_string(),
                        discord_id: pp::render(&discord_id, &c).trim().to_string(),
                        password: pp::render(&password, &c),
                    },
                    display_name: pp::render(&display_name, &c).trim().to_string(),
                    plugin: id.clone(),
                };
                if server.server.address.is_empty() {
                    // Back to EchoVRCE: nothing to ask.
                    if let Some(g) = d.state.game_server.take() {
                        tracing::info!(
                            "plugin {id}: Echo VR plays on EchoVRCE again (was {})",
                            g.server.address
                        );
                        d.save();
                        back_to_nevr(d);
                        d.notify("Echo VR plays on EchoVRCE again");
                    }
                    continue;
                }
                if let Some((address, account)) = server.echorelay() {
                    if !relay::valid_server(&address) {
                        return fail(
                            d,
                            &id,
                            format!(
                                "{address} isn't an EchoRelay address: host:port, e.g. {}.",
                                relay::DEFAULT_SERVER
                            ),
                        );
                    }
                    if !relay::valid_account(&account) {
                        return fail(
                            d,
                            &id,
                            format!(
                                "EchoRelay needs a display name (up to {} characters) and a password (up to {}).",
                                relay::NAME_MAX,
                                relay::PASSWORD_MAX
                            ),
                        );
                    }
                } else if crate::core::launcher::mods::backend_of(&server.server).is_none() {
                    return fail(
                        d,
                        &id,
                        format!(
                            "{} isn't a server address: host[:port], or a ws:// or wss:// address.",
                            server.server.address
                        ),
                    );
                }
                if d.state.game_server.as_ref() == Some(&server) {
                    continue;
                }
                let message = server_question(&p.page.name, &server);
                if let Some(r) = d.plugins.views.get_mut(&id).and_then(|v| v.runner.as_mut()) {
                    r.waiting = true;
                }
                d.plugins.server_asked = Some((id.clone(), server));
                d.dialogs.options(
                    SERVER_KEY,
                    "Play on another server?",
                    &message,
                    crate::ui::dialogs::Icon::Warning,
                    &["Play there", "Cancel"],
                );
                return;
            }
        }
    }
}

/// The live builds set up for an EchoRelay server get nEVR back now (the next start would
/// otherwise), unless Echo VR runs from them.
fn back_to_nevr(d: &mut Dashboard) {
    let local = d.local();
    for v in d
        .state
        .versions
        .iter()
        .filter(|v| v.publisher_lock.is_none())
    {
        if local.runs(&v.id, &v.root) {
            continue;
        }
        match relay::leave_live(v) {
            Ok(true) => tracing::info!("{}: nEVR is back in its place (EchoVRCE again)", v.id),
            Ok(false) => {}
            Err(e) => tracing::warn!("{}: {e:#}", v.id),
        }
    }
}

/// Pure: what the player is asked before plugin `plugin` sets the game's server.
fn server_question(plugin: &str, s: &GameServer) -> String {
    let address = &s.server.address;
    let place = if s.name.is_empty() || s.name == *address {
        address.clone()
    } else {
        format!("{} ({address})", s.name)
    };
    if s.kind == ServerKind::EchoRelay {
        return format!(
            "{plugin} wants Echo VR to play on {place}, an EchoRelay server, instead of \
             EchoVRCE. From the next start the live build signs in there as {}. On EchoRelay \
             it starts with EchoLoader in nEVR's place, with the mods that are on; EchoVRCE's \
             friends and parties are off there. Only say yes to a server you trust. Back to \
             EchoVRCE: in {plugin}, or remove the plugin.",
            s.display_name
        );
    }
    let account = if s.server.discord_id.is_empty() {
        String::new()
    } else {
        format!(" as {}", s.server.discord_id)
    };
    format!(
        "{plugin} wants Echo VR to play on {place} instead of EchoVRCE. From the next start \
         the live build signs in{account} and finds matches there. Only say yes to a server \
         you trust. Back to EchoVRCE: in {plugin}, or remove the plugin."
    )
}

/// A request of a running action answered.
fn answered(d: &mut Dashboard, ctx: &egui::Context, id: &str, result: Result<Value, String>) {
    let Some(p) = d.plugins.list.iter().find(|p| p.page.id == id).cloned() else {
        return;
    };
    let save = d.plugins.views.get(id).and_then(|v| {
        let r = v.runner.as_ref()?;
        match r.steps.get(r.next.checked_sub(1)?) {
            Some(Step::Request { save, .. }) => Some(save.clone()),
            _ => None,
        }
    });
    match result {
        Ok(value) => {
            if let Some(save) = save.filter(|s| !s.is_empty()) {
                set_bound(d, &p, &save, value);
            }
            if let Some(r) = d.plugins.views.get_mut(id).and_then(|v| v.runner.as_mut()) {
                r.waiting = false;
            }
            advance(d, ctx, &p);
        }
        Err(e) => fail(d, id, format!("The server didn't answer: {e}")),
    }
}

/// Sets what `bind` names: a `page.` value, or a setting (kept in the plugin's folder).
fn set_bound(d: &mut Dashboard, p: &Installed, bind: &str, value: Value) {
    if let Some(key) = bind.strip_prefix("settings.") {
        let text = pp::text_of(&value);
        if let Err(e) = core::write_settings(p, &[(key.to_string(), text.clone())]) {
            tracing::warn!("plugin {}: {e:#}", p.page.id);
        }
        view(d, p).settings.insert(key.to_string(), text);
        keep_echoxr(d, p);
    } else if let Some(path) = bind.strip_prefix("page.") {
        pp::set_path(&mut view(d, p).page, path, value);
    }
}

// ---- the rail's plugin tabs ----

/// The plugins on the rail, in its order (indices into the installed list): the ones the
/// player ordered, then the ones installed since. Unpinned ones are left out.
pub(super) fn pinned(d: &Dashboard) -> Vec<u8> {
    let ids: Vec<&str> = d.plugins.list.iter().map(|p| p.page.id.as_str()).collect();
    rail_order(&ids, &d.state.rail_order, &d.state.rail_unpinned)
}

/// [`pinned`] for these installed ids.
fn rail_order(
    installed: &[&str],
    order: &[String],
    unpinned: &std::collections::BTreeSet<String>,
) -> Vec<u8> {
    let index = |id: &str| installed.iter().position(|i| *i == id);
    let mut out: Vec<u8> = Vec::new();
    let ordered = order.iter().filter_map(|id| index(id));
    for i in ordered.chain(0..installed.len()) {
        let Ok(b) = u8::try_from(i) else { continue };
        if !out.contains(&b) && !unpinned.contains(installed[i]) {
            out.push(b);
        }
    }
    out
}

/// Whether the plugin is on the rail.
pub(super) fn is_pinned(d: &Dashboard, id: &str) -> bool {
    !d.state.rail_unpinned.contains(id)
}

/// Puts the plugin on the rail (last) or takes it off.
pub(super) fn set_pinned(d: &mut Dashboard, id: &str, on: bool) {
    let mut order = pinned_ids(d);
    order.retain(|o| o != id);
    if on {
        d.state.rail_unpinned.remove(id);
        order.push(id.to_string());
    } else {
        d.state.rail_unpinned.insert(id.to_string());
    }
    d.state.rail_order = order;
    d.save();
}

/// Moves the plugin's tab one up (`-1`) or down (`1`) the rail.
pub(super) fn move_tab(d: &mut Dashboard, id: &str, by: isize) {
    let mut order = pinned_ids(d);
    let Some(at) = order.iter().position(|o| o == id) else {
        return;
    };
    let to = at as isize + by;
    if to < 0 || to as usize >= order.len() {
        return;
    }
    order.swap(at, to as usize);
    d.state.rail_order = order;
    d.save();
}

fn pinned_ids(d: &Dashboard) -> Vec<String> {
    pinned(d)
        .into_iter()
        .map(|i| d.plugins.list[i as usize].page.id.clone())
        .collect()
}

// ---- a plugin's page ----

/// Its tab's title.
pub(super) fn title(d: &Dashboard, i: u8) -> String {
    d.plugins
        .list
        .get(i as usize)
        .map_or_else(|| "Plugin".into(), |p| p.page.name.clone())
}

pub(super) fn rail_icon(p: &PluginPage) -> Icon {
    icon_named(&p.icon)
}

pub(super) fn show_page(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context, i: u8) {
    let Some(p) = d.plugins.list.get(i as usize).cloned() else {
        return empty_state(kit, Icon::Plus, "Plugins", "This plugin was removed.");
    };
    view(d, &p);
    answers(d, &p);
    let c = context(d, &p);
    let f = forms(d, &p);
    let (dx, dy) = (kit.dx(), kit.dy());
    let cols = &p.page.columns;
    match cols.len() {
        0 => {}
        1 => column(
            d,
            kit,
            ctx,
            &p,
            &c,
            &f,
            0,
            &cols[0],
            WHOLE.wider(dx).taller(dy),
        ),
        _ => {
            column(
                d,
                kit,
                ctx,
                &p,
                &c,
                &f,
                0,
                &cols[0],
                LEFT.wider(dx).taller(dy),
            );
            panel::at_right(kit, |k| {
                column(d, k, ctx, &p, &c, &f, 1, &cols[1], RIGHT.taller(dy))
            });
        }
    }
}

/// A column of cards in `r`, scrolled when it is taller.
#[allow(clippy::too_many_arguments)]
fn column(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    p: &Installed,
    c: &Value,
    f: &Forms,
    n: usize,
    cards: &[Card],
    r: Dr,
) {
    let w = r.w;
    // A card with nothing to show (no title, every block hidden) isn't drawn.
    let shown: Vec<&Card> = cards
        .iter()
        .filter(|card| pp::truthy(&card.when, c))
        .filter(|card| {
            !card.title.is_empty()
                || card
                    .blocks
                    .iter()
                    .any(|b| block_height(kit, b, c, f, dz(w - 44.0)) > 0.0)
        })
        .collect();
    let heights: Vec<f32> = shown
        .iter()
        .map(|card| card_height(kit, card, c, f, dz(w - 44.0)))
        .collect();
    let total = heights.iter().sum::<f32>() + dz(CARD_GAP) * shown.len().saturating_sub(1) as f32;
    let (x, y, cw, ch) = (dz(r.x), dz(r.y), dz(r.w), dz(r.h));
    let mut scroll = d
        .plugins
        .views
        .get(&p.page.id)
        .map_or(0.0, |v| v.scroll[n.min(1)]);
    kit.scroll_area(
        &format!("plugin-{}-col{n}", p.page.id),
        x,
        y,
        cw + dz(14.0),
        ch,
        total,
        &mut scroll,
    );
    if let Some(v) = d.plugins.views.get_mut(&p.page.id) {
        v.scroll[n.min(1)] = scroll;
    }
    kit.clipped(x - dz(4.0), y, cw + dz(8.0), ch, |k| {
        let mut top = y - scroll;
        for (ci, (card, h)) in shown.iter().zip(&heights).enumerate() {
            let cr = Dr::new(r.x, top / dz(1.0), r.w, h / dz(1.0));
            draw_card(d, k, ctx, p, c, f, card, cr, &format!("{n}-{ci}"));
            top += h + dz(CARD_GAP);
        }
    });
}

fn tone_color(t: TextTone) -> egui::Color32 {
    match t {
        TextTone::Normal => design::BODY,
        TextTone::Muted => design::GREY,
        TextTone::Good => design::QUEST_ON,
        TextTone::Warn => design::QUEST_WARN,
    }
}

const TEXT_SIZE: f32 = 16.0;
const LABEL_H: f32 = 28.0;
const BLOCK_GAP: f32 = 16.0;

fn text_height(kit: &Kit, text: &str, w: f32) -> f32 {
    kit.caps_block(text, TEXT_SIZE, design::BODY, w)
        .iter()
        .map(|g| g.size().y)
        .sum::<f32>()
}

/// A card's height (logical pixels) with content `w` wide.
fn card_height(kit: &Kit, card: &Card, c: &Value, f: &Forms, w: f32) -> f32 {
    let mut h = if card.title.is_empty() {
        dz(22.0)
    } else {
        dz(72.0)
    };
    if !card.help.is_empty() {
        h += text_height(kit, &pp::render(&card.help, c), w) + dz(BLOCK_GAP);
    }
    for b in &card.blocks {
        h += block_height(kit, b, c, f, w);
    }
    h + dz(10.0)
}

fn block_height(kit: &Kit, b: &Block, c: &Value, f: &Forms, w: f32) -> f32 {
    let gap = dz(BLOCK_GAP);
    match b {
        Block::Form { section, when } => match &f.settings {
            Some(s) if pp::truthy(when, c) => {
                let part = form_part(s, section);
                let h = plugin_form::height(kit, &part, &f.values, &f.state, w);
                if h > 0.0 {
                    h + gap
                } else {
                    0.0
                }
            }
            _ => 0.0,
        },
        Block::Presets { when } => {
            if f.settings.is_some() && pp::truthy(when, c) {
                presets_height(kit, f, w) + gap
            } else {
                0.0
            }
        }
        Block::Text { text, when, .. } => {
            if !pp::truthy(when, c) {
                return 0.0;
            }
            text_height(kit, &pp::render(text, c), w) + gap
        }
        Block::Value { when, .. } | Block::Field { when, .. } | Block::Choice { when, .. } => {
            if pp::truthy(when, c) {
                dz(LABEL_H) + BTN_H + gap
            } else {
                0.0
            }
        }
        Block::List {
            items,
            filter,
            empty,
            when,
            ..
        } => {
            if !pp::truthy(when, c) {
                return 0.0;
            }
            let n = list_items(c, items, filter).len();
            if n == 0 {
                text_height(kit, &pp::render(empty, c), w) + gap
            } else {
                n as f32 * dz(ROW_H + ROW_GAP) + gap
            }
        }
        Block::Button(b) => {
            if pp::truthy(&b.when, c) {
                BTN_H + gap
            } else {
                0.0
            }
        }
    }
}

fn list_items(c: &Value, items: &str, filter: &str) -> Vec<Value> {
    pp::items(c, items)
        .into_iter()
        .filter(|it| pp::truthy(filter, &pp::with_item(c, it)))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn draw_card(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    p: &Installed,
    c: &Value,
    f: &Forms,
    card: &Card,
    r: Dr,
    key: &str,
) {
    let (x, mut y, w) = if card.title.is_empty() {
        hero::card_frame(k, r, "");
        (dz(r.x + 22.0), dz(r.y + 22.0), dz(r.w - 44.0))
    } else {
        let (x, y, w, _) = hero::card_frame(k, r, &pp::render(&card.title, c));
        (x, y, w)
    };
    if !card.help.is_empty() {
        y += k.caps_text(
            x,
            y,
            w,
            &pp::render(&card.help, c),
            TEXT_SIZE,
            design::GREY,
            0.0,
        ) + dz(BLOCK_GAP);
    }
    let busy = d
        .plugins
        .views
        .get(&p.page.id)
        .is_some_and(|v| v.runner.is_some());
    for (bi, b) in card.blocks.iter().enumerate() {
        let bkey = format!("plugin-{}-{key}-{bi}", p.page.id);
        y += draw_block(d, k, ctx, p, c, f, b, x, y, w, &bkey, busy);
    }
}

/// Draws a block at `y`; its height.
#[allow(clippy::too_many_arguments)]
fn draw_block(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    p: &Installed,
    c: &Value,
    f: &Forms,
    b: &Block,
    x: f32,
    y: f32,
    w: f32,
    key: &str,
    busy: bool,
) -> f32 {
    let gap = dz(BLOCK_GAP);
    let h = block_height(k, b, c, f, w);
    if h == 0.0 {
        return 0.0;
    }
    match b {
        Block::Form { section, .. } => {
            if let Some(s) = &f.settings {
                let part = form_part(s, section);
                let out = {
                    let v = view(d, p);
                    let values = v.settings.clone();
                    plugin_form::form(
                        k,
                        key,
                        &part,
                        &values,
                        &mut v.form,
                        x,
                        y,
                        w,
                        (f32::NEG_INFINITY, f32::INFINITY),
                        !busy,
                    )
                };
                if let Some((label, question)) = out.confirm {
                    d.dialogs
                        .confirm(FORM_ACTION_KEY, &label, &question, DlgIcon::Warning);
                    d.plugins.asked = Some((p.page.id.clone(), Asked::FormAction));
                }
                write_form(d, p, out.changes);
            }
        }
        Block::Presets { .. } => draw_presets(d, k, p, f, x, y, w, key),
        Block::Text { text, tone, .. } => {
            k.caps_text(
                x,
                y,
                w,
                &pp::render(text, c),
                TEXT_SIZE,
                tone_color(*tone),
                0.0,
            );
        }
        Block::Value {
            label, value, copy, ..
        } => {
            k.caption(x, y, &pp::render(label, c));
            let fy = y + dz(LABEL_H);
            let text = pp::render(value, c);
            let bw = if *copy {
                k.button_width("Copy", Some(Icon::Copy), BTN_H)
                    .max(dz(120.0))
            } else {
                0.0
            };
            let box_w = if *copy { w - bw - dz(12.0) } else { w };
            let r = k.rect(x, fy, box_w, BTN_H);
            k.ui.painter()
                .rect_filled(r, dz(4.0), egui::Color32::from_black_alpha(90));
            let g = k.label_galley(
                &text,
                egui::FontId::monospace(dz(17.0)),
                design::TEXT,
                box_w - dz(20.0),
            );
            let gy = fy + (BTN_H - g.size().y) / 2.0;
            k.put(x + dz(10.0), gy, g);
            if *copy
                && k.button(
                    &format!("{key}-copy"),
                    x + w - bw,
                    fy,
                    bw,
                    BTN_H,
                    Tone::Dark,
                    Some(Icon::Copy),
                    "Copy",
                    !text.is_empty(),
                    "Copy it to give it to friends",
                )
                .clicked
            {
                ctx.copy_text(text);
                d.notify("Copied");
            }
        }
        Block::Field {
            bind,
            label,
            hint,
            secret,
            ..
        } => {
            k.caption(x, y, &pp::render(label, c));
            let current = pp::lookup(c, bind).map(pp::text_of).unwrap_or_default();
            let v = view(d, p);
            let draft = v.drafts.entry(bind.clone()).or_insert(current);
            let placeholder = pp::render(hint, c);
            let fy = y + dz(LABEL_H);
            let done = if *secret {
                k.secret_field(key, draft, x, fy, w, BTN_H, &placeholder, false, "")
            } else {
                k.field(key, draft, x, fy, w, BTN_H, &placeholder, false, "")
            };
            let typed = draft.trim().to_string();
            if bind.starts_with("page.") {
                // The page sees what is typed at once.
                set_bound(d, p, bind, Value::String(typed));
            } else if done {
                set_bound(d, p, bind, Value::String(typed));
                view(d, p).drafts.remove(bind);
            }
        }
        Block::Choice {
            bind,
            label,
            options,
            value,
            text,
            ..
        } => {
            k.caption(x, y, &pp::render(label, c));
            let opts: Vec<(String, String)> = pp::items(c, options)
                .iter()
                .map(|it| {
                    let ic = pp::with_item(c, it);
                    let v = if value.is_empty() {
                        pp::text_of(it)
                    } else {
                        pp::render(value, &ic)
                    };
                    let t = if text.is_empty() {
                        v.clone()
                    } else {
                        pp::render(text, &ic)
                    };
                    (v, t)
                })
                .collect();
            let current = pp::lookup(c, bind).map(pp::text_of).unwrap_or_default();
            // Nothing chosen yet: the first option.
            if !opts.iter().any(|(v, _)| *v == current) {
                if let Some((first, _)) = opts.first() {
                    set_bound(d, p, bind, Value::String(first.clone()));
                }
            }
            let shown = opts
                .iter()
                .find(|(v, _)| *v == current)
                .or(opts.first())
                .map_or_else(|| "None".to_string(), |(_, t)| t.clone());
            let items: Vec<MenuItem> = opts.iter().map(|(_, t)| MenuItem::row(t, "")).collect();
            if let Some(i) = k.menu_button(key, &shown, &items, x, y + dz(LABEL_H), w, BTN_H, "") {
                set_bound(d, p, bind, Value::String(opts[i].0.clone()));
            }
        }
        Block::List {
            items,
            filter,
            title,
            detail,
            button,
            empty,
            ..
        } => {
            let rows = list_items(c, items, filter);
            if rows.is_empty() {
                k.caps_text(x, y, w, &pp::render(empty, c), TEXT_SIZE, design::GREY, 0.0);
            }
            let mut ry = y;
            for (i, it) in rows.iter().enumerate() {
                let ic = pp::with_item(c, it);
                hero::tile(k, x, ry, w, dz(ROW_H), false);
                let pad = dz(16.0);
                let bw = button.as_ref().map_or(0.0, |b| {
                    k.button_width(&pp::render(&b.label, &ic), None, BTN_H)
                        .max(dz(110.0))
                });
                let tw = w - bw - pad * 3.0;
                let t =
                    k.label_galley(&pp::render(title, &ic), design::din(20.0), design::TEXT, tw);
                k.put(x + pad, ry + dz(10.0), t);
                let dt = k.label_galley(
                    &pp::render(detail, &ic),
                    design::din(15.0),
                    design::GREY,
                    tw,
                );
                k.put(x + pad, ry + dz(40.0), dt);
                if let Some(b) = button.as_ref().filter(|b| pp::truthy(&b.when, &ic)) {
                    let by = ry + (dz(ROW_H) - BTN_H) / 2.0;
                    if draw_button(
                        k,
                        &format!("{key}-row{i}"),
                        b,
                        &ic,
                        x + w - pad - bw,
                        by,
                        bw,
                        busy,
                    ) {
                        act(d, ctx, p, &b.action, it.clone());
                    }
                }
                ry += dz(ROW_H + ROW_GAP);
            }
        }
        Block::Button(b) => {
            let label = pp::render(&b.label, c);
            let bw = k.button_width(&label, None, BTN_H).max(dz(160.0));
            if draw_button(k, key, b, c, x, y, bw, busy) {
                act(d, ctx, p, &b.action, Value::Null);
            }
            let _ = gap;
        }
    }
    h
}

#[allow(clippy::too_many_arguments)]
fn draw_button(
    k: &mut Kit,
    key: &str,
    b: &Button,
    c: &Value,
    x: f32,
    y: f32,
    w: f32,
    busy: bool,
) -> bool {
    let enabled = !busy && pp::truthy(&b.enabled, c);
    let tone = if b.primary { Tone::Go } else { Tone::Dark };
    let tip = if busy {
        "Working on the last one…"
    } else {
        ""
    };
    k.button(
        key,
        x,
        y,
        w,
        BTN_H,
        tone,
        None,
        &pp::render(&b.label, c),
        enabled,
        tip,
    )
    .clicked
}

// ---- a plugin's settings forms and presets ----

/// What the settings blocks need besides the page's context, copied for this frame: the
/// settings, their values, the forms' state and the player's presets.
#[derive(Default)]
struct Forms {
    settings: Option<Settings>,
    values: BTreeMap<String, String>,
    state: FormState,
    user: Vec<UserPreset>,
    note: String,
}

fn forms(d: &mut Dashboard, p: &Installed) -> Forms {
    let v = view(d, p);
    if v.user_presets.is_none() && p.page.settings.is_some() {
        v.user_presets = Some(presets::load(&p.dir));
    }
    Forms {
        settings: p.page.settings.clone(),
        values: v.settings.clone(),
        state: v.form.clone(),
        user: v.user_presets.clone().unwrap_or_default(),
        note: v.preset_note.clone(),
    }
}

/// The part of `s` a form block shows: one section, its title left to the card (`*`:
/// every section, with their titles).
fn form_part(s: &Settings, section: &str) -> Settings {
    let mut part = Settings {
        store: s.store.clone(),
        ..Default::default()
    };
    if section == "*" {
        part.sections = s.sections.clone();
    } else if let Some(i) = s.section(section) {
        let mut sec = s.sections[i].clone();
        sec.title.clear();
        sec.tooltip.clear();
        part.sections.push(sec);
    }
    part
}

/// Writes changes a form or a preset made, and keeps EchoXR's copies with them.
fn write_form(d: &mut Dashboard, p: &Installed, changes: Vec<(String, String)>) {
    if changes.is_empty() {
        return;
    }
    if !d.demo {
        if let Err(e) = core::write_settings(p, &changes) {
            d.dialogs.error(
                "Couldn't change the settings",
                &format!("{e:#}"),
                Default::default(),
            );
            return;
        }
    }
    view(d, p).settings.extend(changes);
    keep_echoxr(d, p);
}

/// When `p` keeps EchoXR's settings: its file beside EchoXR in every installed version, so
/// a change reaches a game that is running.
fn keep_echoxr(d: &Dashboard, p: &Installed) {
    if d.demo
        || !p
            .page
            .settings
            .as_ref()
            .is_some_and(echoxr_settings::is_echoxr)
    {
        return;
    }
    let text = std::fs::read_to_string(p.dir.join(ECHOXR_FILE)).unwrap_or_default();
    echoxr_settings::apply_all(&d.state.versions, Some(&text));
}

/// The answer to a question `p`'s page asked.
fn answers(d: &mut Dashboard, p: &Installed) {
    let Some((id, asked)) = d.plugins.asked.clone() else {
        return;
    };
    if id != p.page.id {
        return;
    }
    let key = match asked {
        Asked::FormAction => FORM_ACTION_KEY,
        Asked::Replace(_) => PRESET_REPLACE_KEY,
        Asked::Delete(_) => PRESET_DELETE_KEY,
    };
    let Some(answer) = d.dialogs.take(key) else {
        return;
    };
    d.plugins.asked = None;
    let yes = answer.is_yes();
    match asked {
        Asked::FormAction => {
            let pending = view(d, p).form.pending.take();
            if yes {
                write_form(d, p, pending.unwrap_or_default());
            }
        }
        Asked::Replace(name) => {
            if let (true, Some(s)) = (yes, &p.page.settings) {
                let values = view(d, p).settings.clone();
                save_preset(d, p, s, &values, name, true);
            }
        }
        Asked::Delete(i) => {
            let mut users = view(d, p).user_presets.clone().unwrap_or_default();
            if yes && i < users.len() {
                let gone = users.remove(i);
                if store_presets(d, p, users) {
                    view(d, p).preset_note = format!("Deleted: {}.", gone.name);
                }
            }
        }
    }
}

/// What the presets card says about the settings as they are.
fn preset_status(f: &Forms) -> String {
    let Some(s) = &f.settings else {
        return String::new();
    };
    let now = match presets::matching(s, &f.values, &f.user) {
        Some(Match::Builtin(i)) => {
            let p = &s.presets[i];
            if p.help.is_empty() {
                format!("These are {}.", p.name)
            } else {
                format!("These are {}: {}", p.name, p.help)
            }
        }
        Some(Match::User(i)) => format!("Your preset {}.", f.user[i].name),
        None => "Your own settings: save them to keep them as a preset.".into(),
    };
    if f.note.is_empty() {
        now
    } else {
        format!("{} {now}", f.note)
    }
}

/// The name a preset of the current settings goes by: the preset they are, else what is
/// typed, else a plain one.
fn current_name(f: &Forms, typed: &str) -> String {
    let matched =
        f.settings
            .as_ref()
            .and_then(|s| match presets::matching(s, &f.values, &f.user)? {
                Match::Builtin(i) => Some(s.presets[i].name.clone()),
                Match::User(i) => Some(f.user[i].name.clone()),
            });
    matched
        .or_else(|| presets::check_name(typed).ok())
        .unwrap_or_else(|| "My settings".into())
}

fn presets_height(k: &Kit, f: &Forms, w: f32) -> f32 {
    let gap = dz(BLOCK_GAP);
    let advanced = f.settings.as_ref().is_some_and(Settings::has_advanced);
    dz(LABEL_H)
        + BTN_H
        + gap
        + text_height(k, &preset_status(f), w)
        + gap
        + dz(LABEL_H)
        + BTN_H
        + gap
        + BTN_H
        + if advanced { gap + dz(24.0) } else { 0.0 }
}

/// The presets card's block: the preset picker, what the settings are, Save under a name,
/// Rename, Delete, Export… and Import…, and Show advanced.
#[allow(clippy::too_many_arguments)]
fn draw_presets(
    d: &mut Dashboard,
    k: &mut Kit,
    p: &Installed,
    f: &Forms,
    x: f32,
    y: f32,
    w: f32,
    key: &str,
) {
    let Some(s) = &f.settings else {
        return;
    };
    let gap = dz(BLOCK_GAP);
    let current = presets::matching(s, &f.values, &f.user);

    k.caption(x, y, "Preset");
    let pick = |label: &str, detail: &str, checked: bool, tip: &str| MenuItem::Pick {
        label: label.into(),
        detail: detail.into(),
        detail_color: design::GREY,
        checked,
        tip: tip.into(),
    };
    let mut items: Vec<MenuItem> = s
        .presets
        .iter()
        .enumerate()
        .map(|(i, pr)| pick(&pr.name, "", current == Some(Match::Builtin(i)), &pr.help))
        .collect();
    if !f.user.is_empty() {
        items.push(MenuItem::Divider);
        items.extend(
            f.user
                .iter()
                .enumerate()
                .map(|(i, u)| pick(&u.name, "Yours", current == Some(Match::User(i)), "")),
        );
    }
    let shown = match current {
        Some(Match::Builtin(i)) => s.presets[i].name.clone(),
        Some(Match::User(i)) => f.user[i].name.clone(),
        None => "Your own".into(),
    };
    let my = y + dz(LABEL_H);
    let picked = k.menu_button(
        &format!("{key}-pick"),
        &shown,
        &items,
        x,
        my,
        w,
        BTN_H,
        "A preset sets every setting it covers at once",
    );
    if let Some(i) = picked {
        let n = s.presets.len();
        let chosen = if i < n {
            Some((
                s.presets[i].name.clone(),
                s.presets[i].values.clone(),
                false,
            ))
        } else {
            f.user
                .get(i.wrapping_sub(n + 1))
                .map(|u| (u.name.clone(), u.values.clone(), true))
        };
        if let Some((name, values, mine)) = chosen {
            write_form(d, p, presets::changes_to(s, &f.values, &values));
            let v = view(d, p);
            v.preset_note = format!("Applied: {name}.");
            if mine {
                v.preset_name = name;
            }
        }
    }

    let mut yy = my + BTN_H + gap;
    let status = preset_status(f);
    k.caps_text(x, yy, w, &status, TEXT_SIZE, design::GREY, 0.0);
    yy += text_height(k, &status, w) + gap;

    // Keep the settings under a name.
    k.caption(x, yy, "Save as a preset");
    let fy = yy + dz(LABEL_H);
    let save_w = k.button_width("Save", None, BTN_H).max(dz(120.0));
    let named = {
        let v = view(d, p);
        k.field(
            &format!("{key}-name"),
            &mut v.preset_name,
            x,
            fy,
            w - save_w - dz(12.0),
            BTN_H,
            "Its name",
            false,
            "",
        );
        presets::check_name(&v.preset_name)
    };
    let save_tip = match &named {
        Ok(_) => "Keep the settings as they are now under this name",
        Err(e) => e.as_str(),
    };
    if k.button(
        &format!("{key}-save"),
        x + w - save_w,
        fy,
        save_w,
        BTN_H,
        Tone::Go,
        None,
        "Save",
        named.is_ok(),
        save_tip,
    )
    .clicked
    {
        if let Ok(name) = named.clone() {
            save_preset(d, p, s, &f.values, name, false);
        }
    }
    yy = fy + BTN_H + gap;

    // Rename and Delete the player's preset the settings are; Export… and Import….
    let mine = match current {
        Some(Match::User(i)) => Some(i),
        _ => None,
    };
    let bw = (w - dz(12.0) * 3.0) / 4.0;
    let bx = |i: usize| x + i as f32 * (bw + dz(12.0));
    let rename_to = named
        .as_ref()
        .ok()
        .filter(|n| mine.is_some_and(|i| f.user[i].name != **n));
    let (rename_tip, delete_tip) = if mine.is_some() {
        (
            "Give your preset the name typed above",
            "Delete your preset (the settings stay as they are)",
        )
    } else {
        (
            "Pick one of your presets first",
            "Pick one of your presets first",
        )
    };
    if k.button(
        &format!("{key}-rename"),
        bx(0),
        yy,
        bw,
        BTN_H,
        Tone::Dark,
        None,
        "Rename",
        rename_to.is_some(),
        rename_tip,
    )
    .clicked
    {
        if let (Some(i), Some(name)) = (mine, rename_to.cloned()) {
            let mut users = f.user.clone();
            let others: Vec<UserPreset> = users
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, u)| u.clone())
                .collect();
            let name = presets::unique_name(&others, &name);
            users[i].name = name.clone();
            if store_presets(d, p, users) {
                view(d, p).preset_note = format!("Renamed: {name}.");
            }
        }
    }
    if k.button(
        &format!("{key}-delete"),
        bx(1),
        yy,
        bw,
        BTN_H,
        Tone::Dark,
        None,
        "Delete",
        mine.is_some(),
        delete_tip,
    )
    .clicked
    {
        if let Some(i) = mine {
            d.dialogs.confirm_danger(
                PRESET_DELETE_KEY,
                "Delete this preset?",
                &format!(
                    "Your preset {} goes. The settings stay as they are.",
                    f.user[i].name
                ),
                "Delete",
            );
            d.plugins.asked = Some((p.page.id.clone(), Asked::Delete(i)));
        }
    }
    if k.button(
        &format!("{key}-export"),
        bx(2),
        yy,
        bw,
        BTN_H,
        Tone::Dark,
        None,
        "Export…",
        true,
        "Save the settings as a file to pass on",
    )
    .clicked
    {
        let typed = view(d, p).preset_name.clone();
        export_preset(d, p, s, &f.values, &current_name(f, &typed));
    }
    if k.button(
        &format!("{key}-import"),
        bx(3),
        yy,
        bw,
        BTN_H,
        Tone::Dark,
        None,
        "Import…",
        true,
        "Add a preset from a file and use it",
    )
    .clicked
    {
        import_preset(d, p, s, &f.values);
    }
    yy += BTN_H + gap;

    if s.has_advanced() {
        let v = view(d, p);
        k.check(
            &format!("{key}-advanced"),
            &mut v.form.advanced,
            "Show advanced",
            x,
            yy,
            true,
            "Settings for fine-tuning",
        );
    }
}

/// Keeps `values` as the player's preset `name`; asks first when it would replace one
/// (unless `replace`).
fn save_preset(
    d: &mut Dashboard,
    p: &Installed,
    s: &Settings,
    values: &BTreeMap<String, String>,
    name: String,
    replace: bool,
) {
    let mut users = view(d, p).user_presets.clone().unwrap_or_default();
    let values = s.preset_values(values);
    match users
        .iter()
        .position(|u| u.name.eq_ignore_ascii_case(&name))
    {
        Some(i) if !replace => {
            d.dialogs.confirm(
                PRESET_REPLACE_KEY,
                "Replace your preset?",
                &format!(
                    "You have a preset called {}: replace it with the settings as they are now?",
                    users[i].name
                ),
                DlgIcon::Question,
            );
            d.plugins.asked = Some((p.page.id.clone(), Asked::Replace(name)));
            return;
        }
        Some(i) => users[i].values = values,
        None => users.push(UserPreset {
            name: name.clone(),
            values,
        }),
    }
    if store_presets(d, p, users) {
        view(d, p).preset_note = format!("Saved: {name}.");
    }
}

/// Keeps `users` as the player's presets; false (and says why) when they can't be.
fn store_presets(d: &mut Dashboard, p: &Installed, users: Vec<UserPreset>) -> bool {
    if !d.demo {
        if let Err(e) = presets::save(&p.dir, &users) {
            d.dialogs.error(
                "Couldn't keep the presets",
                &format!("{e:#}"),
                Default::default(),
            );
            return false;
        }
    }
    view(d, p).user_presets = Some(users);
    true
}

fn export_preset(
    d: &mut Dashboard,
    p: &Installed,
    s: &Settings,
    values: &BTreeMap<String, String>,
    name: &str,
) {
    let Some(path) = crate::ui::parts::choose_save_json(name) else {
        return;
    };
    let text = presets::export_text(s, &p.page.id, name, values);
    match std::fs::write(&path, text) {
        Ok(()) => {
            let file = path
                .file_name()
                .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
            view(d, p).preset_note = format!("Exported: {file}.");
        }
        Err(e) => d.dialogs.error(
            "Couldn't export the preset",
            &format!("{}: {e}", path.display()),
            Default::default(),
        ),
    }
}

fn import_preset(
    d: &mut Dashboard,
    p: &Installed,
    s: &Settings,
    values: &BTreeMap<String, String>,
) {
    let Some(path) = crate::ui::parts::choose_open_json() else {
        return;
    };
    let read = std::fs::metadata(&path)
        .map_err(|e| e.to_string())
        .and_then(|m| {
            if m.len() as usize > presets::MAX_FILE {
                Err("This file is too big to be a preset.".to_string())
            } else {
                std::fs::read_to_string(&path).map_err(|e| e.to_string())
            }
        });
    let imported = read.and_then(|text| presets::import_text(&text, &p.page.id, s));
    let imp = match imported {
        Ok(imp) => imp,
        Err(why) => {
            d.dialogs
                .error("Couldn't import the preset", &why, Default::default());
            return;
        }
    };
    let mut users = view(d, p).user_presets.clone().unwrap_or_default();
    let name = presets::unique_name(&users, &imp.name);
    users.push(UserPreset {
        name: name.clone(),
        values: s.preset_values(&imp.values),
    });
    if !store_presets(d, p, users) {
        return;
    }
    write_form(d, p, presets::changes_to(s, values, &imp.values));
    let mut note = format!("Imported and applied: {name}.");
    if !imp.clamped.is_empty() {
        note += &format!(" Moved into range: {}.", imp.clamped.join(", "));
    }
    if !imp.dropped.is_empty() {
        note += &format!(" Left out: {}.", imp.dropped.join(", "));
    }
    let v = view(d, p);
    v.preset_note = note;
    v.preset_name = name;
}

// ---- the Plugins page: the library ----

/// Which plugins the library shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Filter {
    #[default]
    All,
    Installed,
    Available,
}

/// A card in the library: an installed plugin (its index), or one of the catalogue's
/// others (its index among them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibCard {
    Installed(u8),
    Offered(usize),
}

/// The library's cards: the installed plugins in the rail's order (the pinned ones, then
/// the others as installed), then the catalogue's others; `filter` keeps some of them.
fn library_cards(pinned: &[u8], installed: usize, offered: usize, filter: Filter) -> Vec<LibCard> {
    let mut out = Vec::new();
    if filter != Filter::Available {
        let rest = (0..installed)
            .filter_map(|i| u8::try_from(i).ok())
            .filter(|i| !pinned.contains(i));
        out.extend(pinned.iter().copied().chain(rest).map(LibCard::Installed));
    }
    if filter != Filter::Installed {
        out.extend((0..offered).map(LibCard::Offered));
    }
    out
}

/// The grid's columns in `w` (logical pixels): as many cards at least `LIB_CARD_W` wide as
/// fit, `LIB_GAP` apart, stretched to fill the row. Their count and width.
fn library_grid(w: f32) -> (usize, f32) {
    let (min, gap) = (dz(LIB_CARD_W), dz(LIB_GAP));
    let cols = (((w + gap) / (min + gap)).floor() as usize).max(1);
    (cols, (w - gap * (cols - 1) as f32) / cols as f32)
}

/// The library's scrolled content: where each heading goes (its top) and each card (its
/// index in `cards`, column, top and height), in logical pixels, and the content's height. A row is as tall
/// as its tallest card (`heights`, in the order of `cards`). With `sections`, the
/// installed plugins and the others each under a heading, on rows of their own.
#[allow(clippy::type_complexity)]
fn library_layout(
    cards: &[LibCard],
    heights: &[f32],
    cols: usize,
    sections: bool,
) -> (Vec<(&'static str, f32)>, Vec<(usize, usize, f32, f32)>, f32) {
    let (gap, head) = (dz(LIB_GAP), dz(LIB_HEAD));
    let all: Vec<usize> = (0..cards.len().min(heights.len())).collect();
    let (installed, offered): (Vec<usize>, Vec<usize>) = all
        .iter()
        .partition(|&&n| matches!(cards[n], LibCard::Installed(_)));
    let groups: Vec<(Option<&'static str>, Vec<usize>)> = if sections {
        vec![
            (Some("Your plugins"), installed),
            (Some("More plugins"), offered),
        ]
    } else {
        vec![(None, all)]
    };
    let (mut headings, mut placed, mut y) = (Vec::new(), Vec::new(), 0.0);
    for (title, group) in groups.into_iter().filter(|(_, g)| !g.is_empty()) {
        if let Some(t) = title {
            headings.push((t, y));
            y += head;
        }
        for row in group.chunks(cols) {
            let row_h = row.iter().map(|&n| heights[n]).fold(0.0, f32::max);
            for (col, &n) in row.iter().enumerate() {
                placed.push((n, col, y, row_h));
            }
            y += row_h + gap;
        }
    }
    (headings, placed, (y - gap).max(0.0))
}

const LIBRARY: Dr = Dr::new(137.0, 156.0, 1736.0, 890.0);
const LIB_CARD_W: f32 = 230.0;
const LIB_CARD_H: f32 = 330.0;
const LIB_GAP: f32 = 28.0;
const LIB_PAD: f32 = 20.0;
/// A section's heading in the library, with the room under it.
const LIB_HEAD: f32 = 34.0;
/// A plugin card's menu (its ⋯ or a right click).
const CARD_MENU: &str = "plugins-card";

/// The icon a plugin names (plugin.json's or the catalogue's `icon`).
fn icon_named(name: &str) -> Icon {
    match name {
        "calendar" => Icon::Calendar,
        "globe" => Icon::Globe,
        "controller" => Icon::Controller,
        // Not the Mods page's: plugins are the launcher's, mods Echo VR's.
        _ => Icon::Plug,
    }
}

/// The Plugins page: every plugin as a card, the installed ones first. Pin the ones you
/// use to the rail; get more, update and remove them here.
pub(super) fn show_manage(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let (dx, dy) = (kit.dx(), kit.dy());
    let r = LIBRARY.wider(dx).taller(dy);
    // The filters are the card's title, as SERVERS and MY MATCHES are on the Servers page.
    let (x, top, w, bottom) = hero::card_frame_bare(kit, r);
    let filters = [
        (Filter::All, "All"),
        (Filter::Installed, "Installed"),
        (Filter::Available, "Available"),
    ];
    let tabs = filters.map(|(f, label)| (label, d.plugins.filter == f));
    if let Some(n) = hero::card_tabs(kit, "plugins-filter", x, dz(r.y + 20.0), &tabs) {
        d.plugins.filter = filters[n].0;
        d.plugins.library_scroll = 0.0;
    }
    let catalog = d.plugins.catalog.clone();
    let list = d.plugins.list.clone();
    let offered: Vec<core::PluginEntry> = catalog
        .as_ref()
        .map(|c| {
            c.plugins
                .iter()
                .filter(|e| !list.iter().any(|p| p.page.id == e.id))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    // The filters and Refresh, in the title's row (the catalogues are also read every 30
    // seconds).
    let ly = dz(r.y + 28.0);
    let link_w = kit
        .spaced_galley("REFRESH", design::din(14.0), design::TEXT, dz(0.5), true)
        .size()
        .x;
    let fx = dz(r.right() - 22.0) - link_w;
    if d.plugins.loading {
        let g = kit.spaced_galley("REFRESH", design::din(14.0), design::GREY, dz(0.5), false);
        kit.put(fx, ly, g);
    } else if kit
        .link(
            "plugins-refresh",
            fx,
            ly,
            "Refresh",
            14.0,
            "Look for new plugins and updates now (the launcher also looks every 30 seconds)",
        )
        .clicked
    {
        refresh(d, ctx);
    }
    let pinned = pinned(d);
    let cards = library_cards(&pinned, list.len(), offered.len(), d.plugins.filter);
    if cards.is_empty() {
        let text = match (d.plugins.filter, catalog.is_some()) {
            (Filter::Installed, _) => {
                "None yet. Plugins add apps to the launcher. Get one under Available, and pin the ones you use to the rail."
            }
            (_, false) => "Reading the plugins list…",
            _ => "You have every plugin there is.",
        };
        kit.caps_text(x, top, w, text, TEXT_SIZE, design::GREY, 0.0);
    }
    // The menu first: the click that opens it isn't taken as a click away from it.
    let menu_was_open = kit.menu_open(CARD_MENU);
    card_menu(d, kit, ctx);
    let (cols, cw) = library_grid(w);
    let gap = dz(LIB_GAP);
    // Each card as tall as its text needs, so its whole summary shows.
    let infos: Vec<CardInfo> = cards
        .iter()
        .map(|c| match *c {
            LibCard::Installed(i) => installed_info(d, i),
            LibCard::Offered(k) => offered_info(&offered[k]),
        })
        .collect();
    let heights: Vec<f32> = infos.iter().map(|c| lib_card_height(kit, c, cw)).collect();
    let (headings, placed, content_h) =
        library_layout(&cards, &heights, cols, d.plugins.filter == Filter::All);
    let list_h = bottom - top;
    // Plugin details (the rail's menu): scrolled to it, marked until another is asked for.
    let focus = d.plugins.focus.clone();
    if let Some(&(_, _, row_top, ch)) = focus.as_ref().and_then(|id| {
        placed.iter().find(|(n, ..)| match &cards[*n] {
            LibCard::Installed(i) => list.get(*i as usize).is_some_and(|p| &p.page.id == id),
            LibCard::Offered(_) => false,
        })
    }) {
        let max = (content_h - list_h).max(0.0);
        let s = &mut d.plugins.library_scroll;
        if row_top < *s || row_top + ch > *s + list_h {
            *s = row_top.min(max);
        }
    }
    kit.scroll_area(
        "plugins-library",
        x,
        top,
        w + dz(16.0),
        list_h,
        content_h,
        &mut d.plugins.library_scroll,
    );
    let scroll = d.plugins.library_scroll;
    kit.clipped(x, top, w + dz(8.0), list_h, |kit| {
        for &(title, hy) in &headings {
            let g = kit.spaced_galley(
                &title.to_uppercase(),
                design::din(14.0),
                design::GREY,
                dz(1.0),
                false,
            );
            kit.put(x, top + hy - scroll, g);
        }
        for &(n, col, cy, ch) in &placed {
            let info = &infos[n];
            let cx = x + col as f32 * (cw + gap);
            let cy = top + cy - scroll;
            if cy + ch < top || cy > bottom {
                continue;
            }
            match cards[n] {
                LibCard::Installed(i) => {
                    installed_card(d, kit, ctx, i, info, (cx, cy, cw, ch), menu_was_open)
                }
                LibCard::Offered(k) => {
                    offered_card(d, kit, ctx, &offered[k], info, (cx, cy, cw, ch))
                }
            }
        }
    });
}

/// What a library card shows: its icon, name, version line, tags and summary, and
/// whether it has two buttons (Update beside or over Open).
struct CardInfo {
    icon: Icon,
    name: String,
    meta: String,
    tags: Vec<(String, egui::Color32)>,
    summary: String,
    two_buttons: bool,
    /// More for its tip.
    note: Option<String>,
}

impl CardInfo {
    /// The card's tip: its name, line and whole summary.
    fn tip(&self) -> String {
        let mut tip = if self.meta.is_empty() {
            format!("{}\n{}", self.name, self.summary)
        } else {
            format!("{} ({})\n{}", self.name, self.meta, self.summary)
        };
        if let Some(note) = &self.note {
            tip.push_str(&format!("\n\n{note}"));
        }
        tip
    }
}

/// An installed plugin's card.
fn installed_info(d: &Dashboard, i: u8) -> CardInfo {
    let Some(p) = d.plugins.list.get(i as usize) else {
        return offered_info(&core::PluginEntry::default());
    };
    let catalog = d.plugins.catalog.clone().unwrap_or_default();
    let update = core::update_for(p, &catalog).is_some();
    let mut meta = Vec::new();
    if !p.page.version.is_empty() {
        meta.push(format!("v{}", p.page.version));
    }
    if !p.page.author.is_empty() {
        meta.push(format!("by {}", p.page.author));
    }
    if p.from_catalog.is_none() {
        meta.push("added by hand".into());
    }
    let mut tags = Vec::new();
    if update {
        tags.push(("Update".to_string(), design::QUEST_ON));
    }
    if !is_pinned(d, &p.page.id) {
        tags.push(("Not in the rail".to_string(), design::GREY));
    }
    if !p.warnings.is_empty() {
        // Short enough for a card's width; the tip says where the details are.
        let problems = match p.warnings.len() {
            1 => "1 problem".to_string(),
            n => format!("{n} problems"),
        };
        tags.push((problems, design::QUEST_WARN));
    }
    CardInfo {
        icon: rail_icon(&p.page),
        name: p.page.name.clone(),
        meta: meta.join(" · "),
        tags,
        summary: p.page.summary.clone(),
        two_buttons: update,
        note: (!p.warnings.is_empty())
            .then(|| "Its description has problems: the launcher's log lists them.".into()),
    }
}

/// A catalogue plugin's card.
fn offered_info(e: &core::PluginEntry) -> CardInfo {
    let mut meta = Vec::new();
    if !e.version.is_empty() {
        meta.push(format!("v{}", e.version));
    }
    if !e.author.is_empty() {
        meta.push(format!("by {}", e.author));
    }
    let mut tags = Vec::new();
    if e.dev {
        // From the dev folder of the dev code in Advanced settings, not published.
        tags.push(("Dev".to_string(), design::RIM_BOTTOM));
    }
    CardInfo {
        icon: icon_named(&e.icon),
        name: e.name.clone(),
        meta: meta.join(" · "),
        tags,
        summary: e.summary.clone(),
        two_buttons: false,
        note: None,
    }
}

/// The most lines of a summary a card shows (its tip has all of it).
const SUMMARY_ROWS: usize = 12;

/// A card's text, laid out `inner` wide: the name (smaller when long), the version line
/// (on more lines at its " · " when long), the tags on rows that fit, and the summary.
struct CardText {
    name: std::sync::Arc<egui::Galley>,
    name_h: f32,
    meta: Vec<std::sync::Arc<egui::Galley>>,
    tag_rows: Vec<Vec<usize>>,
    summary: std::sync::Arc<egui::Galley>,
}

fn card_text(kit: &Kit, c: &CardInfo, inner: f32) -> CardText {
    let name_h = kit
        .label_galley("X", design::din(19.0), design::TEXT, f32::INFINITY)
        .size()
        .y;
    let name = [19.0, 17.0, 15.0]
        .into_iter()
        .map(|size| kit.label_galley(&c.name, design::din(size), design::TEXT, f32::INFINITY))
        .find(|g| g.size().x <= inner)
        .unwrap_or_else(|| kit.label_galley(&c.name, design::din(15.0), design::TEXT, inner));
    let line = |t: &str| kit.label_galley(t, design::din(13.0), design::GREY, inner);
    let mut meta = Vec::new();
    let mut cur = String::new();
    for part in c.meta.split(" · ").filter(|p| !p.is_empty()) {
        let joined = if cur.is_empty() {
            part.to_string()
        } else {
            format!("{cur} · {part}")
        };
        let wide = kit
            .label_galley(&joined, design::din(13.0), design::GREY, f32::INFINITY)
            .size()
            .x;
        if cur.is_empty() || wide <= inner {
            cur = joined;
        } else {
            meta.push(line(&cur));
            cur = part.to_string();
        }
    }
    if !cur.is_empty() {
        meta.push(line(&cur));
    }
    let mut tag_rows: Vec<Vec<usize>> = Vec::new();
    let mut row_w = 0.0;
    for (n, (label, _)) in c.tags.iter().enumerate() {
        let tw = kit.dot_tag_width(label, 12.0);
        match tag_rows.last_mut() {
            Some(row) if row_w + dz(14.0) + tw <= inner => {
                row.push(n);
                row_w += dz(14.0) + tw;
            }
            _ => {
                tag_rows.push(vec![n]);
                row_w = tw;
            }
        }
    }
    let summary = kit.caps_rows(&c.summary, 14.0, design::BODY, inner, SUMMARY_ROWS);
    CardText {
        name,
        name_h,
        meta,
        tag_rows,
        summary,
    }
}

/// How a card's buttons go: one row (Update beside Open) or stacked, when the two
/// labels don't both fit side by side.
fn buttons_stacked(kit: &Kit, c: &CardInfo, inner: f32) -> bool {
    let widest = kit
        .button_width("Update", None, BTN_H)
        .max(kit.button_width("Open", None, BTN_H));
    c.two_buttons && 2.0 * widest + dz(10.0) > inner
}

/// The top of a card's buttons, `h` high at `y`.
fn buttons_top(kit: &Kit, c: &CardInfo, y: f32, h: f32, inner: f32) -> f32 {
    let rows = if buttons_stacked(kit, c, inner) {
        2.0
    } else {
        1.0
    };
    y + h - dz(LIB_PAD) - rows * BTN_H - (rows - 1.0) * dz(10.0)
}

/// The tag rows' height.
fn tags_h(t: &CardText) -> f32 {
    t.tag_rows.len() as f32 * dz(28.0)
}

/// How tall a card `w` wide must be for all of its text: at least `LIB_CARD_H`.
fn lib_card_height(kit: &Kit, c: &CardInfo, w: f32) -> f32 {
    let pad = dz(LIB_PAD);
    let inner = w - pad * 2.0;
    let t = card_text(kit, c, inner);
    let meta_h = (t.meta.len() as f32 * dz(20.0)).max(dz(20.0));
    let text = pad + dz(60.0 + 16.0) + t.name_h + dz(6.0) + meta_h + dz(8.0) + t.summary.size().y;
    let below = if t.tag_rows.is_empty() {
        dz(12.0)
    } else {
        dz(12.0) + tags_h(&t)
    };
    let buttons = buttons_top(kit, c, 0.0, 0.0, inner);
    (text + below - buttons).max(dz(LIB_CARD_H))
}

/// A card's frame, icon, name, line, its summary and its tags over the buttons. The name
/// takes one row on every card, so the summaries of a row start together.
fn card_face(kit: &mut Kit, (x, y, w, h): (f32, f32, f32, f32), marked: bool, c: &CardInfo) {
    hero::tile(kit, x, y, w, h, marked);
    let pad = dz(LIB_PAD);
    // The icon on a tinted square.
    let side = dz(60.0);
    let sq = kit.rect(x + pad, y + pad, side, side);
    kit.ui
        .painter()
        .rect_filled(sq, dz(10.0), design::BLUE.gamma_multiply(0.22));
    let s = dz(36.0);
    icon_at(
        kit.ui.painter(),
        c.icon,
        sq.center() - egui::vec2(s, s) / 2.0,
        s,
        design::TEXT,
    );
    let inner = w - pad * 2.0;
    let t = card_text(kit, c, inner);
    let mut ty = y + pad + side + dz(16.0);
    let gh = t.name.size().y;
    kit.put(x + pad, ty + (t.name_h - gh) / 2.0, t.name.clone());
    ty += t.name_h + dz(6.0);
    let meta_top = ty;
    for g in &t.meta {
        kit.put(x + pad, ty, g.clone());
        ty += dz(20.0);
    }
    ty = ty.max(meta_top + dz(20.0));
    // The summary, then the tags right over the buttons.
    kit.put(x + pad, ty + dz(8.0), t.summary.clone());
    let mut row_cy = buttons_top(kit, c, y, h, inner) - dz(12.0) - tags_h(&t) + dz(14.0);
    for row in &t.tag_rows {
        let mut tx = x + pad;
        for &n in row {
            let (label, color) = &c.tags[n];
            tx += kit.dot_tag(tx, row_cy, label, 12.0, *color) + dz(14.0);
        }
        row_cy += dz(28.0);
    }
}

/// An installed plugin's card: Open (and Update), its pin and ⋯ at the top right.
fn installed_card(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    i: u8,
    info: &CardInfo,
    (x, y, w, h): (f32, f32, f32, f32),
    menu_was_open: bool,
) {
    let Some(p) = d.plugins.list.get(i as usize).cloned() else {
        return;
    };
    let catalog = d.plugins.catalog.clone().unwrap_or_default();
    let update = core::update_for(&p, &catalog).cloned();
    let id = p.page.id.clone();
    let pin = is_pinned(d, &id);
    let busy = d.plugins.busy.clone();
    // The whole card: the summary in its tip, its menu on a right click.
    let card = kit.rect(x, y, w, h);
    let (resp, hover, _) = kit.hot(&format!("plugins-card-{id}"), card, true, &info.tip());
    let marked = d.plugins.focus.as_deref() == Some(id.as_str());
    card_face(kit, (x, y, w, h), marked, info);
    // The whole card opens it (its buttons over it keep their own clicks).
    if hover > 0.01 {
        kit.ui.painter().rect_filled(
            card,
            dz(6.0),
            egui::Color32::from_white_alpha((6.0 * hover) as u8),
        );
    }
    if resp.clicked {
        d.page = Page::Plugin(i);
    }
    let pad = dz(LIB_PAD);
    // Top right: ⋯, and the pin left of it.
    let side = dz(34.0);
    let more = kit.rect(x + w - pad - side + dz(6.0), y + pad - dz(4.0), side, side);
    let pin_r = more.translate(egui::vec2(-side - dz(4.0), 0.0));
    let icon_button = |kit: &mut Kit, key: &str, r: egui::Rect, icon: Icon, bright: bool, tip: &str| {
        let (resp, t, _) = kit.hot(key, r, true, tip);
        if t > 0.01 {
            kit.ui
                .painter()
                .rect_filled(r, dz(6.0), egui::Color32::from_white_alpha((24.0 * t) as u8));
        }
        let s = dz(24.0);
        let rest = if bright {
            egui::Color32::from_gray(225)
        } else {
            design::GREY
        };
        icon_at(
            kit.ui.painter(),
            icon,
            r.center() - egui::vec2(s, s) / 2.0,
            s,
            crate::ui::style::mix(rest, design::TEXT, t),
        );
        resp
    };
    let (pin_icon, pin_tip) = if pin {
        (Icon::Pinned, "In the rail: click to unpin it")
    } else {
        (Icon::Pin, "Pin it to the rail")
    };
    if icon_button(kit, &format!("plugins-pin-{id}"), pin_r, pin_icon, pin, pin_tip).clicked {
        set_pinned(d, &id, !pin);
    }
    let more_resp = icon_button(kit, &format!("plugins-more-{id}"), more, Icon::More, true, "More");
    if (more_resp.clicked && !(menu_was_open && d.plugins.card_menu.as_deref() == Some(&id)))
        || resp.secondary
    {
        d.plugins.card_menu = Some(id.clone());
        d.plugins.card_menu_at = (more.max.x, more.max.y);
        let key = crate::ui::widgets::menu_id(CARD_MENU);
        ctx.data_mut(|dd| dd.insert_temp(key, true));
    }
    // At the bottom: Open, and Update beside it (or over it, when both don't fit).
    let inner = w - pad * 2.0;
    let by = buttons_top(kit, info, y, h, inner);
    if busy.as_deref() == Some(id.as_str()) {
        kit.chip(x + pad, by + dz(6.0), "Working…", design::QUEST_WARN);
        return;
    }
    let idle = busy.is_none();
    let busy_tip = busy
        .as_ref()
        .map(|b| format!("Wait: {b} is being installed or updated"));
    let stacked = buttons_stacked(kit, info, inner);
    let (open_w, open_y) = match (&update, stacked) {
        (Some(_), false) => ((inner - dz(10.0)) / 2.0, by),
        (Some(_), true) => (inner, by + BTN_H + dz(10.0)),
        (None, _) => (inner, by),
    };
    if let Some(e) = &update {
        if kit
            .button(
                &format!("plugins-update-{id}"),
                x + pad,
                by,
                open_w,
                BTN_H,
                Tone::Go,
                None,
                "Update",
                idle,
                busy_tip
                    .as_deref()
                    .unwrap_or(&format!("Get version {}", e.version)),
            )
            .clicked
        {
            get(d, ctx, e.clone(), "Plugin updated");
        }
    }
    if kit
        .button(
            &format!("plugins-open-{id}"),
            x + w - pad - open_w,
            open_y,
            open_w,
            BTN_H,
            Tone::Blue,
            None,
            "Open",
            true,
            "Open its tab",
        )
        .clicked
    {
        d.page = Page::Plugin(i);
    }
}

/// A catalogue plugin you don't have: Get (or Coming soon).
fn offered_card(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    e: &core::PluginEntry,
    info: &CardInfo,
    (x, y, w, h): (f32, f32, f32, f32),
) {
    let card = kit.rect(x, y, w, h);
    kit.hot(&format!("plugins-card-{}", e.id), card, false, &info.tip());
    card_face(kit, (x, y, w, h), false, info);
    let pad = dz(LIB_PAD);
    let by = y + h - pad - BTN_H;
    if d.plugins.busy.as_deref() == Some(e.id.as_str()) {
        kit.chip(x + pad, by + dz(6.0), "Downloading…", design::QUEST_WARN);
        return;
    }
    if !e.downloadable() {
        // As on the Mods page: not a button that can't be pressed.
        let chip = "Coming soon";
        kit.chip(
            x + pad,
            by + (BTN_H - dz(27.0)) / 2.0,
            chip,
            design::QUEST_OFF,
        );
        return;
    }
    let busy_tip = d
        .plugins
        .busy
        .as_ref()
        .map(|id| format!("Wait: {id} is being installed"));
    let tip = busy_tip
        .as_deref()
        .unwrap_or("Download and add it: it gets a tab in the rail");
    if kit
        .button(
            &format!("plugins-get-{}", e.id),
            x + pad,
            by,
            w - pad * 2.0,
            BTN_H,
            Tone::Go,
            Some(Icon::Download),
            "Get",
            d.plugins.busy.is_none(),
            tip,
        )
        .clicked
    {
        get(d, ctx, e.clone(), "Plugin added: it has its own tab now");
    }
}

/// The open card's menu: Update, Pin or Unpin, Remove, Homepage.
fn card_menu(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let Some(id) = d.plugins.card_menu.clone() else {
        return;
    };
    let Some(p) = d.plugins.list.iter().find(|p| p.page.id == id).cloned() else {
        d.plugins.card_menu = None;
        return;
    };
    let catalog = d.plugins.catalog.clone().unwrap_or_default();
    let update = core::update_for(&p, &catalog).cloned();
    let entry = catalog.plugins.iter().find(|e| e.id == id).cloned();
    let homepage = entry.map(|e| e.homepage).unwrap_or_default();
    let pin = is_pinned(d, &id);
    let idle = d.plugins.busy.is_none();
    let wait = (!idle).then_some("Wait: a plugin is being installed or updated");
    let rows = [
        match &update {
            Some(e) => MenuItem::row(&format!("Update to {}", e.version), "Get the catalogue's version")
                .unless(wait),
            None => MenuItem::row("Update", "").unless(Some("It's up to date")),
        },
        if pin {
            MenuItem::row("Unpin from the rail", "Its tab leaves the rail: open it from here")
        } else {
            MenuItem::row("Pin to the rail", "Its tab goes in the rail")
        },
        MenuItem::row("Remove", "Remove it and its settings")
            .unless(wait),
        MenuItem::Divider,
        MenuItem::row("Homepage", "Its page in your browser")
            .unless(homepage.is_empty().then_some("It names no homepage")),
    ];
    let (rx, ry) = d.plugins.card_menu_at;
    let anchor = egui::Rect::from_min_size(egui::pos2(rx - 260.0, ry), egui::vec2(260.0, 0.0));
    match kit.menu_at(CARD_MENU, anchor, &rows) {
        Some(0) => {
            if let Some(e) = update {
                get(d, ctx, e, "Plugin updated");
            }
        }
        Some(1) => set_pinned(d, &id, !pin),
        Some(2) => {
            d.plugins.remove_asked = Some(id.clone());
            d.dialogs.confirm_danger(
                REMOVE_KEY,
                "Remove plugin",
                &format!("Remove {} and its settings?", p.page.name),
                "Remove",
            );
        }
        Some(4) => crate::core::platform::open_url(&homepage),
        _ => {}
    }
    if !kit.menu_open(CARD_MENU) {
        d.plugins.card_menu = None;
    }
}

fn get(d: &mut Dashboard, ctx: &egui::Context, e: core::PluginEntry, done: &str) {
    d.plugins.busy = Some(e.id.clone());
    let done = done.to_string();
    d.plugins.worker.spawn(ctx, move |tx| {
        let r = core::install(&e, &std::sync::atomic::AtomicBool::new(false), &mut |_| {})
            .map_err(|err| format!("{err:#}"));
        tx.send(Msg::Done(done, r));
    });
}

// ---- snapshots ----

/// The Event lobbies plugin with made-up answers.
fn demo() -> PluginsUi {
    let (page, _) = core::read_description(include_str!(
        "../../../docs/plugins/event-lobbies/plugin.json"
    ));
    let mut catalog = PluginCatalog::builtin();
    // The library's other cards: the plugins there are, one from a dev folder.
    for (text, dev) in [
        (include_str!("../../../docs/plugins/game-server/plugin.json"), false),
        (include_str!("../../../docs/plugins/server-host/plugin.json"), false),
        (include_str!("../../../docs/plugins/controllers/plugin.json"), true),
    ] {
        if let Some(p) = core::read_description(text).0 {
            catalog.plugins.push(core::PluginEntry {
                id: p.id.clone(),
                name: p.name.clone(),
                summary: p.summary.clone(),
                author: p.author.clone(),
                version: p.version.clone(),
                url: format!("https://release.echovr.de/launcher/plugins/{}.zip", p.id),
                sha256: "0".repeat(64),
                icon: p.icon.clone(),
                dev,
                ..Default::default()
            });
        }
    }
    let mut ui = PluginsUi {
        started: true,
        catalog: Some(catalog),
        ..Default::default()
    };
    let Some(page) = page else {
        return ui;
    };
    let id = page.id.clone();
    // Installed from the catalogue at its current version, as a player has it.
    let page_version = page.version.clone();
    ui.list.push(Installed {
        page: std::sync::Arc::new(page),
        dir: std::path::PathBuf::new(),
        from_catalog: Some(page_version),
        warnings: Vec::new(),
    });
    let mut v = View {
        page: json!({"build": "halloween", "region": "EU"}),
        settings_read: true,
        ..Default::default()
    };
    v.data.insert(
        "matches".into(),
        json!({"matches": [
            {"id": "34bc3367-bdbf-cd42-0444-fccbe1ca795f", "build": "halloween", "build_name": "Halloween 2018", "gametype": "social_2.0", "mode": "Lobby", "level": "mpl_lobby_b2_spooky", "players": 7, "limit": 16, "joinable": true},
            {"id": "ca51c667-ea0a-ee83-1218-e2d493e30d97", "build": "halloween", "build_name": "Halloween 2018", "gametype": "echo_arena", "mode": "Arena", "level": "mpl_arena_a", "players": 8, "limit": 16, "joinable": true},
            {"id": "f0309645-b344-3d7d-8ff9-25db8a20e9d6", "build": "summer", "build_name": "Summer 2019", "gametype": "social_2.0", "mode": "Lobby", "level": "mpl_lobby_b2_summer", "players": 16, "limit": 16, "joinable": false}
        ]}),
    );
    v.data
        .insert("regions".into(), json!({"regions": ["EU", "US-East"]}));
    v.data.insert(
        "current".into(),
        json!({"ok": true, "match": {"id": "34bc3367-bdbf-cd42-0444-fccbe1ca795f", "gametype": "social_2.0", "mode": "Lobby", "level": "mpl_lobby_b2_spooky", "private": false, "players": 7, "limit": 16}}),
    );
    for s in ["matches", "regions", "current"] {
        v.fetched.insert(s.into(), Instant::now());
    }
    ui.views.insert(id, v);
    ui
}

/// Snapshots: the Game server plugin's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SnapServer {
    /// Filled in, EchoVRCE still.
    Filled,
    /// ...asking to play there.
    Asking,
    /// ...playing there.
    Set,
}

/// Snapshots: the Game server plugin as the second tab in `state`, or not there (`None`).
/// Snapshots: the Controllers plugin installed (its tab second, `on`), its hands tilted a
/// little, advanced settings shown and a preset of the player's own.
/// Snapshots: six more plugins (the catalogue's and two made up), one of them unpinned.
pub(super) fn snap_many(d: &mut Dashboard, on: bool) {
    const PREFIX: &str = "many-";
    d.plugins.list.retain(|p| !p.page.id.starts_with(PREFIX));
    if let Some(c) = d.plugins.catalog.as_mut() {
        c.plugins.retain(|e| !e.id.starts_with(PREFIX));
    }
    if !on {
        return;
    }
    let manifests = [
        include_str!("../../../docs/plugins/game-server/plugin.json"),
        include_str!("../../../docs/plugins/server-host/plugin.json"),
        include_str!("../../../docs/plugins/controllers/plugin.json"),
        include_str!("../../../docs/plugins/game-server/plugin.json"),
        include_str!("../../../docs/plugins/event-lobbies/plugin.json"),
        include_str!("../../../docs/plugins/server-host/plugin.json"),
        include_str!("../../../docs/plugins/controllers/plugin.json"),
        include_str!("../../../docs/plugins/event-lobbies/plugin.json"),
        include_str!("../../../docs/plugins/game-server/plugin.json"),
        include_str!("../../../docs/plugins/server-host/plugin.json"),
    ];
    let renamed = [
        None,
        None,
        None,
        Some(("Tournament tools", "calendar")),
        Some(("Replay viewer", "globe")),
        Some(("Practice drills", "plugin")),
        Some(("Coach mode", "controller")),
        Some(("Map voting", "calendar")),
        Some(("Stats tracker", "plugin")),
        Some(("Clip recorder", "globe")),
    ];
    for (n, (text, rename)) in manifests.iter().zip(renamed).enumerate() {
        let Some(mut page) = core::read_description(text).0 else {
            continue;
        };
        page.id = format!("{PREFIX}{n}-{}", page.id);
        if let Some((name, icon)) = rename {
            page.name = name.into();
            page.icon = icon.into();
        }
        // One with an update, one whose description has a problem: their tags and
        // buttons.
        if n == 3 {
            if let Some(c) = d.plugins.catalog.as_mut() {
                c.plugins.retain(|e| e.id != page.id);
                c.plugins.push(core::PluginEntry {
                    id: page.id.clone(),
                    name: page.name.clone(),
                    version: "9.9.9".into(),
                    url: format!("https://release.echovr.de/launcher/plugins/{}.zip", page.id),
                    sha256: "0".repeat(64),
                    ..Default::default()
                });
            }
        }
        let warnings = if n == 5 {
            vec!["made-up problem".to_string()]
        } else {
            Vec::new()
        };
        d.plugins.list.push(Installed {
            from_catalog: Some(page.version.clone()),
            page: std::sync::Arc::new(page),
            dir: std::path::PathBuf::new(),
            warnings,
        });
    }
    // The second one off the rail.
    if let Some(p) = d.plugins.list.get(2) {
        d.state.rail_unpinned.insert(p.page.id.clone());
    }
}

pub(super) fn snap_controllers(d: &mut Dashboard, on: bool) {
    const ID: &str = "controllers";
    d.plugins.list.retain(|p| p.page.id != ID);
    d.plugins.views.remove(ID);
    if !on {
        return;
    }
    let (page, _) = core::read_description(include_str!(
        "../../../docs/plugins/controllers/plugin.json"
    ));
    let Some(page) = page else {
        return;
    };
    let mut settings = page
        .settings
        .as_ref()
        .map(Settings::defaults)
        .unwrap_or_default();
    settings.insert("Pitch".into(), "-6".into());
    settings.insert("Vibration".into(), "70".into());
    d.plugins.list.push(Installed {
        from_catalog: Some(page.version.clone()),
        page: std::sync::Arc::new(page),
        dir: std::path::PathBuf::new(),
        warnings: Vec::new(),
    });
    let mut view = View {
        page: json!({}),
        settings,
        settings_read: true,
        user_presets: Some(vec![UserPreset {
            name: "Mia's hands".into(),
            values: BTreeMap::from([
                ("Pitch".into(), "-6".into()),
                ("Vibration".into(), "70".into()),
            ]),
        }]),
        preset_name: "Mia's hands".into(),
        preset_note: "Saved: Mia's hands.".into(),
        ..Default::default()
    };
    view.form.advanced = true;
    d.plugins.views.insert(ID.into(), view);
}

pub(super) fn snap_game_server(d: &mut Dashboard, state: Option<SnapServer>) {
    const ID: &str = "game-server";
    d.plugins.list.retain(|p| p.page.id != ID);
    d.plugins.views.remove(ID);
    d.plugins.server_asked = None;
    let Some(state) = state else {
        return;
    };
    let (page, _) = core::read_description(include_str!(
        "../../../docs/plugins/game-server/plugin.json"
    ));
    let Some(page) = page else {
        return;
    };
    d.plugins.list.push(Installed {
        from_catalog: Some(page.version.clone()),
        page: std::sync::Arc::new(page),
        dir: std::path::PathBuf::new(),
        warnings: Vec::new(),
    });
    let settings = [
        ("relay_address", "203.0.113.7:6800"),
        ("relay_name", "Pebbles"),
        ("relay_password", "test"),
    ];
    d.plugins.views.insert(
        ID.into(),
        View {
            page: json!({}),
            settings: settings
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            settings_read: true,
            ..Default::default()
        },
    );
    let server = GameServer {
        kind: ServerKind::EchoRelay,
        display_name: "Pebbles".into(),
        server: PackServer {
            address: "203.0.113.7:6800".into(),
            password: "test".into(),
            ..Default::default()
        },
        plugin: ID.into(),
        ..Default::default()
    };
    match state {
        SnapServer::Filled => {}
        SnapServer::Asking => {
            d.dialogs.options(
                SERVER_KEY,
                "Play on another server?",
                &server_question("Game server", &server),
                crate::ui::dialogs::Icon::Warning,
                &["Play there", "Cancel"],
            );
            d.plugins.server_asked = Some((ID.into(), server));
        }
        SnapServer::Set => d.state.game_server = Some(server),
    }
}

#[cfg(test)]
mod tests {
    use super::Failure;

    /// The library: the pinned plugins in the rail's order, the unpinned ones, then the
    /// catalogue's others; the filters keep either part.
    #[test]
    fn library_cards_in_order() {
        use super::{library_cards, Filter, LibCard::*};
        // Four installed, 2 unpinned, pinned as 3, 0, 1; two to get.
        let pinned = [3, 0, 1];
        assert_eq!(
            library_cards(&pinned, 4, 2, Filter::All),
            [Installed(3), Installed(0), Installed(1), Installed(2), Offered(0), Offered(1)]
        );
        assert_eq!(
            library_cards(&pinned, 4, 2, Filter::Installed),
            [Installed(3), Installed(0), Installed(1), Installed(2)]
        );
        assert_eq!(
            library_cards(&pinned, 4, 2, Filter::Available),
            [Offered(0), Offered(1)]
        );
        assert!(library_cards(&[], 0, 0, Filter::All).is_empty());
    }

    /// The library's sections: each group under its heading on rows of its own, an empty
    /// group left out, each row as tall as its tallest card; without sections one grid
    /// and no headings.
    #[test]
    fn library_layout_sections() {
        use super::{library_layout, LibCard::*, LIB_CARD_H, LIB_GAP, LIB_HEAD};
        use crate::ui::design::dz;
        let (ch, gap, head) = (dz(LIB_CARD_H), dz(LIB_GAP), dz(LIB_HEAD));
        let cards = [Installed(0), Offered(0), Installed(1)];
        let tall = ch + 50.0;
        let heights = [ch, ch, tall];
        let (heads, placed, h) = library_layout(&cards, &heights, 4, true);
        assert_eq!(heads.len(), 2);
        assert_eq!(heads[0].1, 0.0);
        // The installed ones first, side by side, the row as tall as the taller.
        assert_eq!(placed[0], (0, 0, head, tall));
        assert_eq!(placed[1], (2, 1, head, tall));
        assert!((heads[1].1 - (head + tall + gap)).abs() < 1e-3);
        assert_eq!(placed[2].0, 1, "the offered one under its own heading");
        assert_eq!(placed[2].1, 0);
        assert!((placed[2].2 - (heads[1].1 + head)).abs() < 1e-3);
        assert!((h - (placed[2].2 + ch)).abs() < 1e-3);
        let (heads, placed, _) = library_layout(&[Offered(0)], &[ch], 4, true);
        assert_eq!(heads.len(), 1, "no heading over nothing");
        assert_eq!(placed[0].2, head);
        // One grid without sections: two rows of two, each as tall as its cards.
        let (heads, placed, h) = library_layout(&cards, &heights, 2, false);
        assert!(heads.is_empty());
        assert_eq!((placed[2].1, placed[2].2, placed[2].3), (0, ch + gap, tall));
        assert!((h - (ch + gap + tall)).abs() < 1e-3);
        assert_eq!(library_layout(&[], &[], 4, true).2, 0.0);
    }

    /// The library's grid: six columns on the normal window, one at the least, filling the
    /// row.
    #[test]
    fn library_grid_fills_the_row() {
        use super::{library_grid, LIBRARY, LIB_CARD_W, LIB_GAP};
        use crate::ui::design::dz;
        let normal = dz(LIBRARY.w - 44.0);
        let (cols, w) = library_grid(normal);
        assert_eq!(cols, 6);
        assert!(w >= dz(LIB_CARD_W));
        assert!((cols as f32 * w + (cols - 1) as f32 * dz(LIB_GAP) - normal).abs() < 0.01);
        assert_eq!(library_grid(10.0).0, 1);
        assert!(library_grid(normal + 600.0).0 > 6, "a wider window, more columns");
    }

    /// The rail's plugin tabs: the player's order, then ones installed since; unpinned
    /// and uninstalled ones left out.
    #[test]
    fn rail_order_keeps_the_players_order() {
        use super::rail_order;
        let installed = ["a", "b", "c", "d"];
        let none = Default::default();
        assert_eq!(rail_order(&installed, &[], &none), [0, 1, 2, 3]);
        let order = ["c".to_string(), "gone".into(), "a".into()];
        assert_eq!(rail_order(&installed, &order, &none), [2, 0, 1, 3]);
        let unpinned = ["a".to_string(), "d".into()].into_iter().collect();
        assert_eq!(rail_order(&installed, &order, &unpinned), [2, 1]);
    }

    #[test]
    fn failures_by_kind() {
        // Nothing listens there: no answer at all.
        let e = crate::core::http::get_text("http://127.0.0.1:9/api/matches").unwrap_err();
        assert_eq!(Failure::of_get(&e).kind, "unreachable");
        // The server answered, but without the matches API, or with an error.
        let e = anyhow::anyhow!("GET http://x/api/matches: server responded with 404 Not Found");
        assert_eq!(Failure::of_get(&e).kind, "missing");
        let e = anyhow::anyhow!(
            "GET http://x/api/matches: server responded with 500 Internal Server Error"
        );
        assert_eq!(Failure::of_get(&e).kind, "server");
    }
}
