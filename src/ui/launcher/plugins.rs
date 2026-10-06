//! Launcher plugins in the UI: each installed plugin's tab (its page, drawn from its
//! description: see `core::launcher::plugin_page`), and the Plugins page (the rail's +),
//! where plugins are got from the catalogue, updated and removed.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use eframe::egui;
use serde_json::{json, Map, Value};

use super::{empty_state, hero, panel, play, Dashboard, Page};
use crate::core::launcher::plugin_page::{
    self as pp, Block, Button, Card, Method, PluginPage, Request, Step, Tone as TextTone,
};
use crate::core::launcher::plugins::{self as core, Installed, PluginCatalog};
use crate::ui::design::{self, dz, Dr};
use crate::ui::kit::Kit;
use crate::ui::parts::Worker;
use crate::ui::style::Icon;
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

/// The plugins, their catalogue, and each plugin page's state.
#[derive(Default)]
pub(super) struct PluginsUi {
    /// The installed plugins, by name (their tabs, in this order).
    pub list: Vec<Installed>,
    catalog: Option<PluginCatalog>,
    started: bool,
    views: HashMap<String, View>,
    worker: Worker<Msg>,
    /// The plugin being installed, updated or removed.
    busy: Option<String>,
    /// Debug builds: the plugin ECHOVR_PAGE named, opened once it is read.
    pub open_when_read: Option<String>,
    remove_asked: Option<String>,
}

enum Msg {
    Read(Vec<Installed>),
    Catalog(PluginCatalog),
    Fetched {
        plugin: String,
        source: String,
        result: Result<Value, String>,
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
            }
            Msg::Catalog(c) => d.plugins.catalog = Some(c),
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
                    Err(e) => {
                        tracing::info!("plugin data {source}: {e}");
                        v.data.insert(source, json!({"error": e}));
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
            d.plugins.busy = Some(id.clone());
            d.plugins.worker.spawn(ctx, move |tx| {
                let r = core::remove(&id).map_err(|e| format!("{e:#}"));
                tx.send(Msg::Done("Plugin removed".into(), r));
            });
        }
    }
    // The shown plugin page's data, when due.
    if let Page::Plugin(i) = d.page {
        if let Some(p) = d.plugins.list.get(i as usize).cloned() {
            fetch_due(d, ctx, &p);
        }
    }
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

/// Sends a request on the plugin worker; `f` makes the message from its answer.
fn send(
    d: &Dashboard,
    ctx: &egui::Context,
    r: &Request,
    c: &Value,
    f: impl FnOnce(Result<Value, String>) -> Msg + Send + 'static,
) {
    let url = pp::render(&r.url, c);
    let body = r.body.as_ref().map(|b| pp::render_body(b, c));
    let method = r.method;
    d.plugins.worker.spawn(ctx, move |tx| {
        let result = match method {
            Method::Get => crate::core::http::get_text(&url).map_err(|e| format!("{e:#}")),
            Method::Post => crate::core::http::post_json(&url, &body.unwrap_or(Value::Null))
                .map_err(|e| format!("{e:#}"))
                .and_then(|(status, text)| {
                    if status >= 500 {
                        Err(format!("the server answered {status}"))
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
                    result,
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
                    // As PLAY: they don't start on Linux yet.
                    Some(vid) if cfg!(target_os = "linux") && event(&vid) => {
                        return fail(
                            d,
                            &id,
                            "Event builds don't run on Linux yet: EchoXR runs only the live build."
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
        }
    }
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
    } else if let Some(path) = bind.strip_prefix("page.") {
        pp::set_path(&mut view(d, p).page, path, value);
    }
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
    match p.icon.as_str() {
        "calendar" => Icon::Calendar,
        "globe" => Icon::Globe,
        _ => Icon::Mods,
    }
}

pub(super) fn show_page(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context, i: u8) {
    let Some(p) = d.plugins.list.get(i as usize).cloned() else {
        return empty_state(kit, Icon::Plus, "Plugins", "This plugin was removed.");
    };
    view(d, &p);
    let c = context(d, &p);
    let (dx, dy) = (kit.dx(), kit.dy());
    let cols = &p.page.columns;
    match cols.len() {
        0 => {}
        1 => column(d, kit, ctx, &p, &c, 0, &cols[0], WHOLE.wider(dx).taller(dy)),
        _ => {
            column(d, kit, ctx, &p, &c, 0, &cols[0], LEFT.wider(dx).taller(dy));
            panel::at_right(kit, |k| {
                column(d, k, ctx, &p, &c, 1, &cols[1], RIGHT.taller(dy))
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
                    .any(|b| block_height(kit, b, c, dz(w - 44.0)) > 0.0)
        })
        .collect();
    let heights: Vec<f32> = shown
        .iter()
        .map(|card| card_height(kit, card, c, dz(w - 44.0)))
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
            draw_card(d, k, ctx, p, c, card, cr, &format!("{n}-{ci}"));
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
fn card_height(kit: &Kit, card: &Card, c: &Value, w: f32) -> f32 {
    let mut h = if card.title.is_empty() {
        dz(22.0)
    } else {
        dz(72.0)
    };
    if !card.help.is_empty() {
        h += text_height(kit, &pp::render(&card.help, c), w) + dz(BLOCK_GAP);
    }
    for b in &card.blocks {
        h += block_height(kit, b, c, w);
    }
    h + dz(10.0)
}

fn block_height(kit: &Kit, b: &Block, c: &Value, w: f32) -> f32 {
    let gap = dz(BLOCK_GAP);
    match b {
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
        y += draw_block(d, k, ctx, p, c, b, x, y, w, &bkey, busy);
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
    b: &Block,
    x: f32,
    y: f32,
    w: f32,
    key: &str,
    busy: bool,
) -> f32 {
    let gap = dz(BLOCK_GAP);
    let h = block_height(k, b, c, w);
    if h == 0.0 {
        return 0.0;
    }
    match b {
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

// ---- the Plugins page ----

const INSTALLED_CARD: Dr = Dr::new(137.0, 156.0, 1146.0, 890.0);
const MORE_CARD: Dr = Dr::new(1318.0, 156.0, 555.0, 890.0);
const TILE_H: f32 = 118.0;

pub(super) fn show_manage(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let (dx, dy) = (kit.dx(), kit.dy());
    let (x, mut y, w, _) = hero::card_frame(
        kit,
        INSTALLED_CARD.wider(dx).taller(dy),
        "Installed plugins",
    );
    if d.plugins.list.is_empty() {
        kit.caps_text(
            x,
            y,
            w,
            "None yet. Plugins add apps to the launcher: each gets its own tab in the rail. Get one from the list on the right.",
            TEXT_SIZE,
            design::GREY,
            0.0,
        );
    }
    let busy = d.plugins.busy.clone();
    let catalog = d.plugins.catalog.clone().unwrap_or_default();
    for (i, p) in d.plugins.list.clone().iter().enumerate() {
        hero::tile(kit, x, y, w, dz(TILE_H), false);
        let pad = dz(18.0);
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
        if !p.warnings.is_empty() {
            // The details are in the launcher's log.
            meta.push(format!(
                "{} problem(s) in its plugin.json, see the log",
                p.warnings.len()
            ));
        }
        let name = kit.label_galley(&p.page.name, design::din(22.0), design::TEXT, w * 0.5);
        kit.put(x + pad, y + dz(14.0), name);
        let m = kit.label_galley(&meta.join(" · "), design::din(14.0), design::GREY, w * 0.5);
        kit.put(x + pad, y + dz(48.0), m);
        let s = kit.label_galley(
            &p.page.summary,
            design::myriad(17.0),
            design::BODY,
            w - pad * 2.0,
        );
        kit.put(x + pad, y + dz(76.0), s);
        // Its buttons, right to left.
        let mut right = x + w - pad;
        let by = y + dz(14.0);
        let mut button =
            |kit: &mut Kit, key: &str, label: &str, tone: Tone, enabled: bool, tip: &str| {
                let bw = kit.button_width(label, None, BTN_H).max(dz(110.0));
                right -= bw;
                let hit = kit
                    .button(key, right, by, bw, BTN_H, tone, None, label, enabled, tip)
                    .clicked;
                right -= dz(12.0);
                hit
            };
        let idle = busy.is_none();
        if button(
            kit,
            &format!("plugins-remove-{}", p.page.id),
            "Remove",
            Tone::Dark,
            idle,
            "Remove it and its settings",
        ) {
            d.plugins.remove_asked = Some(p.page.id.clone());
            d.dialogs.confirm_danger(
                REMOVE_KEY,
                "Remove plugin",
                &format!("Remove {} and its settings?", p.page.name),
                "Remove",
            );
        }
        if let Some(e) = core::update_for(p, &catalog) {
            let label = format!("Update to {}", e.version);
            if button(
                kit,
                &format!("plugins-update-{}", p.page.id),
                &label,
                Tone::Go,
                idle,
                "Get the catalogue's version",
            ) {
                get(d, ctx, e.clone(), "Plugin updated");
            }
        }
        if button(
            kit,
            &format!("plugins-open-{}", p.page.id),
            "Open",
            Tone::Blue,
            true,
            "Open its tab",
        ) {
            d.page = Page::Plugin(i as u8);
        }
        if busy.as_deref() == Some(p.page.id.as_str()) {
            kit.chip(
                right - dz(120.0),
                by + dz(6.0),
                "Working…",
                design::QUEST_WARN,
            );
        }
        y += dz(TILE_H + 12.0);
    }
    panel::at_right(kit, |k| more_card(d, k, ctx, MORE_CARD.taller(dy)));
}

fn more_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context, r: Dr) {
    let (x, mut y, w, _) = hero::card_frame(k, r, "More plugins");
    let Some(catalog) = d.plugins.catalog.clone() else {
        k.caps_text(
            x,
            y,
            w,
            "Reading the plugins list…",
            TEXT_SIZE,
            design::GREY,
            0.0,
        );
        return;
    };
    let offered: Vec<_> = catalog
        .plugins
        .iter()
        .filter(|e| !d.plugins.list.iter().any(|p| p.page.id == e.id))
        .cloned()
        .collect();
    if offered.is_empty() {
        k.caps_text(
            x,
            y,
            w,
            "You have every plugin there is.",
            TEXT_SIZE,
            design::GREY,
            0.0,
        );
    }
    let idle = d.plugins.busy.is_none();
    for e in offered {
        let name = k.label_galley(&e.name, design::din(20.0), design::TEXT, w * 0.6);
        k.put(x, y, name);
        let label = if e.downloadable() {
            "Get"
        } else {
            "Coming soon"
        };
        let bw = k.button_width(label, None, BTN_H).max(dz(110.0));
        let tip = if e.downloadable() {
            "Download and add it"
        } else {
            "Not published yet"
        };
        if k.button(
            &format!("plugins-get-{}", e.id),
            x + w - bw,
            y - dz(4.0),
            bw,
            BTN_H,
            Tone::Go,
            None,
            label,
            idle && e.downloadable(),
            tip,
        )
        .clicked
        {
            get(d, ctx, e.clone(), "Plugin added: it has its own tab now");
        }
        // The summary under the button, not beside it.
        y += BTN_H + dz(8.0);
        if d.plugins.busy.as_deref() == Some(e.id.as_str()) {
            k.chip(x, y, "Downloading…", design::QUEST_WARN);
            y += dz(34.0);
        }
        y += k.caps_text(x, y, w, &e.summary, 15.0, design::GREY, 0.0) + dz(26.0);
    }
    let _ = y;
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
    let mut ui = PluginsUi {
        started: true,
        catalog: Some(PluginCatalog::builtin()),
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
