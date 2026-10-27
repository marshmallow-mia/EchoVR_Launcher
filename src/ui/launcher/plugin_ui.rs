//! Schema 2 plugins' pages: what a plugin's code draws (`core::plugin_host`), with the
//! launcher's own widgets. The plugin runs on a thread of its own; each frame this reads
//! what it sent (its page, notices, questions, what it asks the launcher to do) and sends
//! it what the player did on the page.
//!
//! A secret field's text stays here (`secrets`): the plugin only learns that something was
//! typed, and a service that needs it ([`super::plugin_services`]) reads it from here.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;

use super::plugins::{self as pl, Forms, LEFT, RIGHT, WHOLE};
use super::{hero, panel, Dashboard};
use crate::core::launcher::plugins::{self as core, Installed};
use crate::core::launcher::store::{GameServer, PackServer, ServerKind};
use crate::core::plugin_host::{self as ph, types, Cmd, Node, Out, Running, Shared, UiEvent};
use crate::core::server_host::{events, status};
use crate::ui::design::{self, dz, Dr};
use crate::ui::kit::Kit;
use crate::ui::style::Icon;
use crate::ui::widgets::{MenuItem, Tone, BTN_H};

const CARD_GAP: f32 = 30.0;
const TEXT_SIZE: f32 = 16.0;
const LABEL_H: f32 = 28.0;
const GAP: f32 = 16.0;
const ROW_H: f32 = 60.0;
/// Each line under a row's first.
const ROW_LINE_H: f32 = 24.0;
const ROW_GAP: f32 = 10.0;
const LINE_H: f32 = 22.0;
/// Console lines kept per log.
const LOG_LINES: usize = 5000;
/// How deep containers may nest (a node that names itself stops here).
const DEPTH: usize = 6;
/// A plugin's question (the one whose [`CodeView::asked`] is set asked it).
const ASK_KEY: &str = "plugin-ask";

/// A schema 2 plugin's page while the launcher runs.
#[derive(Default)]
pub(super) struct CodeView {
    running: Option<Running>,
    page: Option<types::Page>,
    /// It stopped: why, and whether Restart may start it again.
    stopped: Option<(String, bool)>,
    /// Fields being typed in: (the value the plugin gave, what is typed).
    drafts: HashMap<String, (String, String)>,
    /// What was typed into its secret fields, by field id: never sent to it.
    pub secrets: HashMap<String, String>,
    logs: HashMap<u32, LogStream>,
    scroll: [f32; 2],
    /// Its question that is up: (its id, how many buttons).
    asked: Option<(String, u32)>,
    shown: bool,
    /// The version that runs (an update starts it again).
    version: String,
    /// The fields drawn this frame (a draft of one no longer shown goes).
    drawn: std::collections::HashSet<String>,
    /// Snapshots: what the player "did" before the page is drawn, sent once it starts.
    queued: Vec<UiEvent>,
}

/// A server's console, tailed here for a `log-view`.
struct LogStream {
    tail: status::Tail,
    lines: VecDeque<String>,
    scroll: f32,
    follow: bool,
    read_at: Option<std::time::Instant>,
}

impl CodeView {
    /// Sends the plugin an event (nothing when it isn't running).
    pub fn event(&self, e: UiEvent) {
        if let Some(r) = &self.running {
            r.event(e);
        }
    }

    /// Snapshots: it drew its first page, or stopped (or doesn't run).
    #[cfg(test)]
    pub fn settled(&self) -> bool {
        self.running.is_none() || self.page.is_some() || self.stopped.is_some()
    }

    /// Something to send it from a worker.
    pub fn sender(&self) -> Option<std::sync::mpsc::Sender<Cmd>> {
        self.running.as_ref().map(Running::sender)
    }
}

/// What the launcher tells a plugin, as of now.
fn shared(d: &Dashboard, p: &Installed) -> Shared {
    let account = d.state.relay_account.clone().unwrap_or_default();
    let relay = if d.state.relay_server.trim().is_empty() {
        crate::core::launcher::relay::DEFAULT_SERVER.to_string()
    } else {
        d.state.relay_server.trim().to_string()
    };
    let mut info = core::launcher_context(&d.state);
    // Whether the launcher is signed in with EchoVRCE (never the sign-in itself).
    if let Some(m) = info.as_object_mut() {
        m.insert(
            "echovrce".into(),
            serde_json::json!({"signed_in": d.vrce.session().is_some()}),
        );
    }
    Shared {
        info,
        relay_server: relay,
        relay_name: account.name,
        relay_password: account.password,
        granted: pl::granted_now(d, p),
        fake_net: d.demo.then_some(demo_net as ph::FakeNet),
    }
}

/// Snapshots: what the classic lobbies server answers Event lobbies (three public matches,
/// two regions, the player in the first match).
fn demo_net(url: &str) -> (u16, String) {
    let path = url.split_once("://").map_or(url, |(_, r)| r);
    let path = path.split_once('/').map_or("", |(_, p)| p);
    let body = if path.starts_with("api/matches/current") {
        serde_json::json!({"ok": true, "match": {"id": "34bc3367-bdbf-cd42-0444-fccbe1ca795f", "gametype": "social_2.0", "mode": "Lobby", "level": "mpl_lobby_b2_spooky", "private": false, "players": 7, "limit": 16}})
    } else if path.starts_with("api/matches") {
        serde_json::json!({"matches": [
            {"id": "34bc3367-bdbf-cd42-0444-fccbe1ca795f", "build": "halloween", "build_name": "Halloween 2018", "gametype": "social_2.0", "mode": "Lobby", "level": "mpl_lobby_b2_spooky", "players": 7, "limit": 16, "joinable": true},
            {"id": "ca51c667-ea0a-ee83-1218-e2d493e30d97", "build": "halloween", "build_name": "Halloween 2018", "gametype": "echo_arena", "mode": "Arena", "level": "mpl_arena_a", "players": 8, "limit": 16, "joinable": true},
            {"id": "f0309645-b344-3d7d-8ff9-25db8a20e9d6", "build": "summer", "build_name": "Summer 2019", "gametype": "social_2.0", "mode": "Lobby", "level": "mpl_lobby_b2_summer", "players": 16, "limit": 16, "joinable": false}
        ]})
    } else if path.starts_with("api/servers/regions") {
        serde_json::json!({"regions": ["EU", "US-East"]})
    } else {
        return (404, String::new());
    };
    (200, body.to_string())
}

/// Every running schema 2 plugin, each frame: what they sent, the answers to their
/// questions, what they know of the launcher.
pub(super) fn tick(d: &mut Dashboard, ctx: &egui::Context) {
    // The answer to a plugin's question.
    if let Some(a) = d.dialogs.take(ASK_KEY) {
        if let Some(v) = d.plugins.code.values_mut().find(|v| v.asked.is_some()) {
            if let Some((ask, n)) = v.asked.take() {
                let i = match a {
                    crate::ui::dialogs::Answer::Button(i) => i as u32,
                    _ => n,
                };
                v.event(UiEvent::Dialog(ask, i));
            }
        }
    }
    let ids: Vec<String> = d.plugins.code.keys().cloned().collect();
    for id in ids {
        let Some(p) = d.plugins.list.iter().find(|p| p.page.id == id).cloned() else {
            d.plugins.code.remove(&id);
            continue;
        };
        let updated = d
            .plugins
            .code
            .get(&id)
            .is_some_and(|v| v.running.is_some() && v.version != p.page.version);
        if !pl::runs_now(d, &p) || updated {
            stop(d, &id);
            continue;
        }
        let now = shared(d, &p);
        let outs: Vec<Out> = match d.plugins.code.get(&id).and_then(|v| v.running.as_ref()) {
            Some(r) => {
                if let Ok(mut s) = r.shared.lock() {
                    if s.info != now.info {
                        r.event(UiEvent::LauncherChanged);
                    }
                    s.info = now.info;
                    s.relay_server = now.relay_server;
                    s.relay_name = now.relay_name;
                    s.relay_password = now.relay_password;
                    s.granted = now.granted;
                }
                r.rx.try_iter().collect()
            }
            None => Vec::new(),
        };
        for o in outs {
            handle(d, ctx, &p, o);
        }
        // Its consoles.
        if let Some(v) = d.plugins.code.get_mut(&id) {
            for l in v.logs.values_mut() {
                if l.read_at
                    .is_none_or(|t| t.elapsed() >= Duration::from_millis(500))
                {
                    l.read_at = Some(std::time::Instant::now());
                    l.lines.extend(l.tail.read());
                    while l.lines.len() > LOG_LINES {
                        l.lines.pop_front();
                    }
                }
            }
            if !v.logs.is_empty() && v.shown {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    }
}

/// What plugin `p` asked of the page.
fn handle(d: &mut Dashboard, ctx: &egui::Context, p: &Installed, o: Out) {
    let id = p.page.id.clone();
    match o {
        Out::Page(page) => {
            if let Some(v) = d.plugins.code.get_mut(&id) {
                v.page = Some(page);
                v.stopped = None;
            }
        }
        Out::Notify(text, tone) => match tone {
            types::Tone::Bad => d.notify_error(&format!("{}: {text}", p.page.name)),
            _ => d.notify(&text),
        },
        Out::Ask {
            id: ask,
            title,
            text,
            buttons,
            warn,
        } => {
            let refs: Vec<&str> = buttons.iter().map(String::as_str).collect();
            // One question at a time: an earlier one that is still up is closed.
            for v in d.plugins.code.values_mut() {
                if let Some((ask, n)) = v.asked.take() {
                    v.event(UiEvent::Dialog(ask, n));
                }
            }
            d.dialogs.options(
                ASK_KEY,
                &title,
                &text,
                if warn {
                    crate::ui::dialogs::Icon::Warning
                } else {
                    crate::ui::dialogs::Icon::Question
                },
                &refs,
            );
            if let Some(v) = d.plugins.code.get_mut(&id) {
                v.asked = Some((ask, buttons.len() as u32));
            }
        }
        Out::Copy(text) => {
            ctx.copy_text(text);
            d.notify("Copied");
        }
        Out::OpenUrl(url) => {
            if let Err(e) = open::that(&url) {
                d.notify_error(&format!("Couldn't open {url}: {e}"));
            }
        }
        Out::Play { version, args } => {
            if let Err(why) = pl::play_version(d, ctx, &version, &args) {
                d.notify_error(&why);
            }
        }
        Out::SetServer { ticket, server } => {
            let gs = game_server(d, p, server);
            let answer = match pl::propose_server(d, p, gs) {
                Ok(true) => {
                    d.plugins.server_ticket = Some((id.clone(), ticket));
                    None
                }
                Ok(false) => Some(Ok("\"unchanged\"".to_string())),
                Err(why) => Some(Err(why)),
            };
            if let (Some(a), Some(v)) = (answer, d.plugins.code.get(&id)) {
                v.event(UiEvent::ServiceDone(ticket, a));
            }
        }
        Out::ResetServer => pl::reset_server_of(d, &id),
        Out::SettingsChanged => {
            pl::settings_changed(d, p);
        }
        Out::Service { ticket, op, args } => {
            super::plugin_services::start(d, ctx, p, ticket, &op, args);
        }
        Out::OpenLog { log, server } => {
            let dir = crate::core::server_host::server_dir(&server);
            if let Some(v) = d.plugins.code.get_mut(&id) {
                v.logs.insert(
                    log,
                    LogStream {
                        tail: status::Tail::new(&dir, 2 * 1024 * 1024),
                        lines: VecDeque::new(),
                        scroll: 0.0,
                        follow: true,
                        read_at: None,
                    },
                );
            }
        }
        Out::CopyLog { log, filter } => {
            let lines = d
                .plugins
                .code
                .get(&id)
                .and_then(|v| v.logs.get(&log))
                .map(|l| shown_lines(&l.lines, &filter, true))
                .unwrap_or_default();
            ctx.copy_text(lines.join("\n"));
            d.notify("Copied the console");
        }
        Out::Stopped { why, restart } => {
            if let Some(v) = d.plugins.code.get_mut(&id) {
                v.stopped = Some((why, restart));
            }
        }
    }
}

/// The server a plugin asked for, as the launcher keeps it: an EchoRelay server that is the
/// classic lobbies server gets the player's account there when the plugin gave none (the
/// plugin never sees it), if it may use it.
fn game_server(d: &Dashboard, p: &Installed, s: ph::ServerAsk) -> GameServer {
    let kind = if s.kind.trim() == "echorelay" {
        ServerKind::EchoRelay
    } else {
        ServerKind::Nakama
    };
    let mut password = s.password;
    let mut display_name = s.display_name.trim().to_string();
    let to_relay = kind == ServerKind::EchoRelay
        && core::to_relay(&d.state, &format!("http://{}", s.address.trim()));
    if to_relay && pl::granted_now(d, p).relay_password && p.needs.relay_password {
        if let Some(a) = &d.state.relay_account {
            if password.is_empty() {
                password = a.password.clone();
            }
            if display_name.is_empty() {
                display_name = a.name.clone();
            }
        }
    }
    GameServer {
        name: s.name.trim().to_string(),
        kind,
        server: PackServer {
            address: s.address.trim().to_string(),
            server_key: s.key.trim().to_string(),
            discord_id: s.discord_id.trim().to_string(),
            password,
        },
        display_name,
        plugin: p.page.id.clone(),
    }
}

/// Pure: a console's lines as shown: cleaned (a jsonl record's time, level and message),
/// nEVR's noise left out unless `noise`, only those containing `filter` (any case).
pub(super) fn shown_lines(lines: &VecDeque<String>, filter: &str, noise: bool) -> Vec<String> {
    let filter = filter.trim().to_lowercase();
    lines
        .iter()
        .filter_map(|raw| {
            let clean = events::clean(raw);
            if !noise && events::is_noise(&clean) {
                return None;
            }
            let line = match events::level(raw) {
                Some(level) => {
                    let ts = serde_json::from_str::<serde_json::Value>(raw)
                        .ok()
                        .and_then(|v| {
                            v.get("ts")
                                .and_then(|t| t.as_str())
                                .map(|t| t.get(11..19).unwrap_or("").to_string())
                        })
                        .unwrap_or_default();
                    format!("{ts} {level:<5} {clean}")
                }
                None => clean,
            };
            (filter.is_empty() || line.to_lowercase().contains(&filter)).then_some(line)
        })
        .collect()
}

fn line_color(l: &str) -> egui::Color32 {
    if l.starts_with("[host]") {
        design::LINK
    } else if l.contains("FATAL") || l.contains(" error ") || l.contains("\"level\":\"error\"") {
        design::DANGER
    } else if l.contains(" warn ") || l.contains("\"level\":\"warn\"") {
        design::QUEST_WARN
    } else {
        design::BODY
    }
}

fn tone_color(t: types::Tone) -> egui::Color32 {
    match t {
        types::Tone::Normal => design::BODY,
        types::Tone::Muted => design::GREY,
        types::Tone::Good => design::QUEST_ON,
        types::Tone::Warn => design::QUEST_WARN,
        types::Tone::Bad => design::DANGER,
    }
}

/// Starts plugin `p` unless it runs.
fn start(d: &mut Dashboard, ctx: &egui::Context, p: &Installed) {
    let fresh = shared(d, p);
    let v = d.plugins.code.entry(p.page.id.clone()).or_default();
    if v.running.is_some() {
        return;
    }
    v.version = p.page.version.clone();
    let c = ctx.clone();
    v.running = Some(ph::spawn(
        ph::Spec {
            page: p.page.clone(),
            dir: p.dir.clone(),
            first_party: p.first_party,
        },
        fresh,
        Arc::new(move || c.request_repaint()),
    ));
    for e in std::mem::take(&mut v.queued) {
        v.event(e);
    }
}

/// Stops plugin `id` (turned off, removed or updated): it starts again when shown.
pub(super) fn stop(d: &mut Dashboard, id: &str) {
    d.plugins.code.remove(id);
}

/// The page is shown or hidden now: tells the plugins that care.
pub(super) fn shown(d: &mut Dashboard, id: Option<&str>) {
    for (pid, v) in d.plugins.code.iter_mut() {
        let on = Some(pid.as_str()) == id;
        if on != v.shown {
            v.shown = on;
            v.event(if on { UiEvent::Shown } else { UiEvent::Hidden });
        }
    }
}

// ---- drawing ----

/// Plugin `p`'s page (it runs and the player allowed it: see `plugins::show_page`).
pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context, p: &Installed) {
    start(d, ctx, p);
    shown(d, Some(&p.page.id));
    let id = p.page.id.clone();
    let (stopped, page) = match d.plugins.code.get(&id) {
        Some(v) => (v.stopped.clone(), v.page.clone()),
        None => (None, None),
    };
    if let Some((why, restart)) = stopped {
        let text = if restart {
            format!("{} stopped: {why}. The launcher kept running.", p.page.name)
        } else {
            format!(
                "{} stopped: {why}. It stopped too often to start it again: restart the launcher, or update or remove the plugin.",
                p.page.name
            )
        };
        if pl::panel_button(kit, p, &text, "Start again", restart) {
            if let Some(v) = d.plugins.code.get_mut(&id) {
                v.stopped = None;
                v.page = None;
                if let Some(r) = &v.running {
                    r.send(Cmd::Restart);
                }
            }
        }
        return;
    }
    let Some(page) = page else {
        // Its first page is on its way.
        super::empty_panel(kit, pl::rail_icon(&p.page), &p.page.name, "Starting…");
        return;
    };
    let f = pl::forms_of(d, p);
    let (dx, dy) = (kit.dx(), kit.dy());
    let mut events = Vec::new();
    if let Some(v) = d.plugins.code.get_mut(&id) {
        v.drawn.clear();
    }
    match page.columns.len() {
        0 => {}
        1 => column(
            d,
            kit,
            ctx,
            p,
            &page,
            &f,
            0,
            WHOLE.wider(dx).taller(dy),
            &mut events,
        ),
        _ => {
            column(
                d,
                kit,
                ctx,
                p,
                &page,
                &f,
                0,
                LEFT.wider(dx).taller(dy),
                &mut events,
            );
            panel::at_right(kit, |k| {
                column(d, k, ctx, p, &page, &f, 1, RIGHT.taller(dy), &mut events)
            });
        }
    }
    if let Some(v) = d.plugins.code.get_mut(&id) {
        let drawn = std::mem::take(&mut v.drawn);
        v.drafts.retain(|field, _| drawn.contains(field));
        // What is typed in a field counts before a click (Save) on the same page.
        if events
            .iter()
            .any(|e| matches!(e, UiEvent::Clicked(_) | UiEvent::Toggled(..)))
        {
            let mut typed: Vec<UiEvent> = Vec::new();
            for (field, draft) in v.drafts.iter_mut() {
                if draft.1.trim() != draft.0 {
                    let t = draft.1.trim().to_string();
                    typed.push(UiEvent::Changed(field.clone(), t.clone()));
                    *draft = (t.clone(), t);
                }
            }
            typed.extend(events);
            events = typed;
        }
        for e in events {
            v.event(e);
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
    page: &types::Page,
    f: &Forms,
    n: usize,
    r: Dr,
    events: &mut Vec<UiEvent>,
) {
    let w = dz(r.w - 44.0);
    let cards = &page.columns[n];
    let heights: Vec<f32> = cards
        .iter()
        .map(|&i| card_height(kit, page, i, f, w))
        .collect();
    let total = heights.iter().sum::<f32>() + dz(CARD_GAP) * cards.len().saturating_sub(1) as f32;
    let (x, y, cw, ch) = (dz(r.x), dz(r.y), dz(r.w), dz(r.h));
    let id = p.page.id.clone();
    let mut scroll = d.plugins.code.get(&id).map_or(0.0, |v| v.scroll[n.min(1)]);
    kit.scroll_area(
        &format!("plugin2-{id}-col{n}"),
        x,
        y,
        cw + dz(14.0),
        ch,
        total,
        &mut scroll,
    );
    if let Some(v) = d.plugins.code.get_mut(&id) {
        v.scroll[n.min(1)] = scroll;
    }
    kit.clipped(x - dz(4.0), y, cw + dz(8.0), ch, |k| {
        let mut top = y - scroll;
        for (ci, (&i, h)) in cards.iter().zip(&heights).enumerate() {
            let cr = Dr::new(r.x, top / dz(1.0), r.w, h / dz(1.0));
            draw_card(d, k, ctx, p, page, f, i, cr, &format!("{n}-{ci}"), events);
            top += h + dz(CARD_GAP);
        }
    });
}

fn text_height(kit: &Kit, text: &str, w: f32) -> f32 {
    kit.caps_block(text, TEXT_SIZE, design::BODY, w)
        .iter()
        .map(|g| g.size().y)
        .sum::<f32>()
}

/// Whether a card's first child is tabs: they are its title.
fn tabs_first(page: &types::Page, c: &types::Card) -> bool {
    c.title.is_empty()
        && c.children
            .first()
            .is_some_and(|&i| matches!(page.nodes.get(i as usize), Some(Node::Tabs(_))))
}

fn card_height(kit: &Kit, page: &types::Page, i: u32, f: &Forms, w: f32) -> f32 {
    let Some(Node::Card(c)) = page.nodes.get(i as usize) else {
        return 0.0;
    };
    let mut h = if c.title.is_empty() && !tabs_first(page, c) {
        dz(22.0)
    } else {
        dz(72.0)
    };
    if !c.help.is_empty() {
        h += text_height(kit, &c.help, w) + dz(GAP);
    }
    for (n, &k) in c.children.iter().enumerate() {
        h += if n == 0 && tabs_first(page, c) {
            tabs_body_height(kit, page, k, f, w, 1)
        } else {
            node_height(kit, page, k, f, w, 1)
        };
    }
    h + dz(10.0)
}

fn tabs_body_height(kit: &Kit, page: &types::Page, i: u32, f: &Forms, w: f32, depth: usize) -> f32 {
    match page.nodes.get(i as usize) {
        Some(Node::Tabs(t)) => t
            .children
            .iter()
            .map(|&k| node_height(kit, page, k, f, w, depth + 1))
            .sum(),
        _ => 0.0,
    }
}

/// How many rows a row of buttons `w` wide takes, and each button's width.
fn button_rows(kit: &Kit, bs: &[types::Button], w: f32) -> (usize, Vec<f32>) {
    let widths: Vec<f32> = bs
        .iter()
        .map(|b| match button_icon(&b.icon) {
            Some(_) => BTN_H,
            None => kit.button_width(&b.label, None, BTN_H).max(dz(120.0)),
        })
        .collect();
    let (mut rows, mut x) = (usize::from(!bs.is_empty()), 0.0);
    for bw in &widths {
        if x > 0.0 && x + bw > w {
            rows += 1;
            x = 0.0;
        }
        x += bw + dz(12.0);
    }
    (rows, widths)
}

fn node_height(kit: &Kit, page: &types::Page, i: u32, f: &Forms, w: f32, depth: usize) -> f32 {
    if depth > DEPTH {
        return 0.0;
    }
    let gap = dz(GAP);
    let Some(n) = page.nodes.get(i as usize) else {
        return 0.0;
    };
    match n {
        Node::Card(c) => c
            .children
            .iter()
            .map(|&k| node_height(kit, page, k, f, w, depth + 1))
            .sum(),
        Node::Text(t) => {
            if t.text.is_empty() {
                0.0
            } else {
                text_height(kit, &t.text, w) + gap
            }
        }
        Node::Value(_) | Node::Field(_) | Node::Choice(_) => dz(LABEL_H) + BTN_H + gap,
        Node::Toggle(_) => dz(24.0) + gap,
        Node::Button(_) => BTN_H + gap,
        Node::Buttons(bs) => {
            let (rows, _) = button_rows(kit, bs, w);
            rows as f32 * (BTN_H + dz(10.0)) + gap - dz(10.0).min(rows as f32 * dz(10.0))
        }
        Node::Table(t) => {
            if t.rows.is_empty() {
                if t.empty.is_empty() {
                    0.0
                } else {
                    text_height(kit, &t.empty, w) + gap
                }
            } else {
                t.rows
                    .iter()
                    .map(|r| row_height(kit, r, w) + dz(ROW_GAP))
                    .sum::<f32>()
                    + gap
            }
        }
        Node::Chip(_) => dz(30.0) + gap,
        Node::Progress(p) => (if p.label.is_empty() { 0.0 } else { dz(LABEL_H) }) + dz(12.0) + gap,
        Node::LogView(l) => dz(LINE_H) * l.lines.clamp(4, 60) as f32 + dz(8.0) + gap,
        Node::Tabs(t) => {
            dz(48.0)
                + t.children
                    .iter()
                    .map(|&k| node_height(kit, page, k, f, w, depth + 1))
                    .sum::<f32>()
        }
        Node::Form(section) => match &f.settings {
            Some(s) => {
                let part = pl::form_part(s, section);
                let h = super::plugin_form::height(kit, &part, &f.values, &f.state, w);
                if h > 0.0 {
                    h + gap
                } else {
                    0.0
                }
            }
            None => 0.0,
        },
        Node::Presets => {
            if f.settings.is_some() {
                pl::presets_height(kit, f, w) + gap
            } else {
                0.0
            }
        }
    }
}

/// A row: its first line (name, chips, buttons), then its other cells and its detail,
/// a line each under it, as wide as the row.
fn row_height(k: &Kit, r: &types::Row, w: f32) -> f32 {
    let cells = usize::from(r.cells.iter().filter(|c| !c.chip).count() > 1);
    dz(ROW_H + ROW_LINE_H * cells as f32)
        + detail_galley(k, r, w).map_or(0.0, |g| g.size().y + dz(6.0))
}

/// A row's detail, at most two lines in a row `w` wide.
fn detail_galley(k: &Kit, r: &types::Row, w: f32) -> Option<Arc<egui::Galley>> {
    (!r.detail.is_empty()).then(|| k.caps_rows(&r.detail, 15.0, design::GREY, w - dz(32.0), 2))
}

/// A button's icon (`button.icon`), when it names one of these.
fn button_icon(name: &str) -> Option<Icon> {
    Some(match name {
        "info" => Icon::Info,
        "gear" => Icon::Gear,
        "trash" => Icon::Trash,
        "folder" => Icon::Folder,
        "copy" => Icon::Copy,
        "refresh" => Icon::Refresh,
        "plus" => Icon::Plus,
        _ => return None,
    })
}

/// Button `b`'s width (an icon: a square).
fn button_w(k: &Kit, b: &types::Button) -> f32 {
    match button_icon(&b.icon) {
        Some(_) => BTN_H,
        None => k.button_width(&b.label, None, BTN_H).max(dz(100.0)),
    }
}

/// Draws button `b` at (x, y), `bw` wide: whether it was clicked.
fn draw_button(k: &mut Kit, id: &str, x: f32, y: f32, bw: f32, b: &types::Button) -> bool {
    let (icon, label, hint) = match button_icon(&b.icon) {
        Some(i) => (
            Some(i),
            "",
            if b.hint.is_empty() { &b.label } else { &b.hint },
        ),
        None => (None, b.label.as_str(), &b.hint),
    };
    k.button(
        id,
        x,
        y,
        bw,
        BTN_H,
        button_tone(b),
        icon,
        label,
        b.enabled,
        hint,
    )
    .clicked
}

#[allow(clippy::too_many_arguments)]
fn draw_card(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    p: &Installed,
    page: &types::Page,
    f: &Forms,
    i: u32,
    r: Dr,
    key: &str,
    events: &mut Vec<UiEvent>,
) {
    let Some(Node::Card(c)) = page.nodes.get(i as usize) else {
        return;
    };
    let tabs = tabs_first(page, c);
    let (x, mut y, w) = if tabs {
        let (x, y, w, _) = hero::card_frame_bare(k, r);
        (x, y, w)
    } else if c.title.is_empty() {
        hero::card_frame(k, r, "");
        (dz(r.x + 22.0), dz(r.y + 22.0), dz(r.w - 44.0))
    } else {
        let (x, y, w, _) = hero::card_frame(k, r, &c.title);
        (x, y, w)
    };
    if tabs {
        if let Some(Node::Tabs(t)) = page.nodes.get(c.children[0] as usize) {
            let titles: Vec<(&str, bool)> = t
                .labels
                .iter()
                .enumerate()
                .map(|(n, l)| (l.as_str(), n as u32 == t.selected))
                .collect();
            let tx = dz(r.x + 22.0);
            if let Some(n) = hero::card_tabs(
                k,
                &format!("p2-{}-{key}-tabs", p.page.id),
                tx,
                dz(r.y + 20.0),
                &titles,
            ) {
                events.push(UiEvent::Tab(t.id.clone(), n as u32));
            }
        }
    }
    if !c.help.is_empty() {
        y += k.caps_text(x, y, w, &c.help, TEXT_SIZE, design::GREY, 0.0) + dz(GAP);
    }
    for (n, &ki) in c.children.iter().enumerate() {
        let nkey = format!("p2-{}-{key}-{n}", p.page.id);
        if n == 0 && tabs {
            if let Some(Node::Tabs(t)) = page.nodes.get(ki as usize) {
                for (m, &ti) in t.children.iter().enumerate() {
                    y += draw_node(
                        d,
                        k,
                        ctx,
                        p,
                        page,
                        f,
                        ti,
                        x,
                        y,
                        w,
                        &format!("{nkey}-{m}"),
                        2,
                        events,
                    );
                }
            }
            continue;
        }
        y += draw_node(d, k, ctx, p, page, f, ki, x, y, w, &nkey, 1, events);
    }
}

fn button_tone(b: &types::Button) -> Tone {
    if b.danger {
        Tone::Danger
    } else if b.primary {
        Tone::Go
    } else {
        Tone::Dark
    }
}

/// Draws node `i` at `y`; its height.
#[allow(clippy::too_many_arguments)]
fn draw_node(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    p: &Installed,
    page: &types::Page,
    f: &Forms,
    i: u32,
    x: f32,
    y: f32,
    w: f32,
    key: &str,
    depth: usize,
    events: &mut Vec<UiEvent>,
) -> f32 {
    let h = node_height(k, page, i, f, w, depth);
    if h == 0.0 || depth > DEPTH {
        return 0.0;
    }
    let Some(n) = page.nodes.get(i as usize) else {
        return 0.0;
    };
    let id = p.page.id.clone();
    match n {
        Node::Card(c) => {
            let mut cy = y;
            for (m, &ki) in c.children.iter().enumerate() {
                cy += draw_node(
                    d,
                    k,
                    ctx,
                    p,
                    page,
                    f,
                    ki,
                    x,
                    cy,
                    w,
                    &format!("{key}-{m}"),
                    depth + 1,
                    events,
                );
            }
        }
        Node::Text(t) => {
            k.caps_text(x, y, w, &t.text, TEXT_SIZE, tone_color(t.tone), 0.0);
        }
        Node::Value(v) => {
            k.caption(x, y, &v.label);
            let fy = y + dz(LABEL_H);
            let bw = if v.copy {
                k.button_width("Copy", Some(Icon::Copy), BTN_H)
                    .max(dz(120.0))
            } else {
                0.0
            };
            let box_w = if v.copy { w - bw - dz(12.0) } else { w };
            let r = k.rect(x, fy, box_w, BTN_H);
            k.ui.painter()
                .rect_filled(r, dz(4.0), egui::Color32::from_black_alpha(90));
            let g = k.label_galley(
                &v.value,
                egui::FontId::monospace(dz(17.0)),
                design::TEXT,
                box_w - dz(20.0),
            );
            let gy = fy + (BTN_H - g.size().y) / 2.0;
            k.put(x + dz(10.0), gy, g);
            if v.copy
                && k.button(
                    &format!("{key}-copy"),
                    x + w - bw,
                    fy,
                    bw,
                    BTN_H,
                    Tone::Dark,
                    Some(Icon::Copy),
                    "Copy",
                    !v.value.is_empty(),
                    "",
                )
                .clicked
            {
                ctx.copy_text(v.value.clone());
                d.notify("Copied");
            }
        }
        Node::Field(fl) => {
            k.caption(x, y, &fl.label);
            let fy = y + dz(LABEL_H);
            let Some(view) = d.plugins.code.get_mut(&id) else {
                return h;
            };
            view.drawn.insert(fl.id.clone());
            if fl.secret {
                let text = view.secrets.entry(fl.id.clone()).or_default();
                if k.secret_field(key, text, x, fy, w, BTN_H, &fl.hint, false, "") && fl.enabled {
                    events.push(UiEvent::Changed(fl.id.clone(), String::new()));
                }
            } else {
                let draft = view
                    .drafts
                    .entry(fl.id.clone())
                    .or_insert_with(|| (fl.value.clone(), fl.value.clone()));
                // The plugin changed it while nothing new was typed: its value.
                if draft.0 != fl.value && draft.1 == draft.0 {
                    *draft = (fl.value.clone(), fl.value.clone());
                }
                let mut text = draft.1.clone();
                let done = k.field(key, &mut text, x, fy, w, BTN_H, &fl.hint, false, "");
                if fl.enabled {
                    draft.1 = text.clone();
                }
                if done && fl.enabled && text.trim() != fl.value {
                    events.push(UiEvent::Changed(fl.id.clone(), text.trim().to_string()));
                    *draft = (text.trim().to_string(), text.trim().to_string());
                }
            }
        }
        Node::Choice(c) => {
            k.caption(x, y, &c.label);
            let shown = c
                .options
                .iter()
                .find(|(v, _)| *v == c.selected)
                .or(c.options.first())
                .map_or_else(|| "None".to_string(), |(_, t)| t.clone());
            let items: Vec<MenuItem> = c
                .options
                .iter()
                .map(|(_, t)| MenuItem::row(t, ""))
                .collect();
            if let Some(n) = k.menu_button(key, &shown, &items, x, y + dz(LABEL_H), w, BTN_H, "") {
                if c.enabled {
                    events.push(UiEvent::Changed(c.id.clone(), c.options[n].0.clone()));
                }
            }
        }
        Node::Toggle(t) => {
            let mut on = t.on;
            if k.check(key, &mut on, &t.label, x, y, t.enabled, "") && t.enabled {
                events.push(UiEvent::Toggled(t.id.clone(), on));
            }
        }
        Node::Button(b) => {
            let bw = match button_icon(&b.icon) {
                Some(_) => BTN_H,
                None => k.button_width(&b.label, None, BTN_H).max(dz(160.0)),
            };
            if draw_button(k, key, x, y, bw, b) {
                events.push(UiEvent::Clicked(b.id.clone()));
            }
        }
        Node::Buttons(bs) => {
            let (_, widths) = button_rows(k, bs, w);
            let (mut bx, mut by) = (x, y);
            for (m, (b, bw)) in bs.iter().zip(widths).enumerate() {
                if bx > x && bx + bw > x + w {
                    bx = x;
                    by += BTN_H + dz(10.0);
                }
                if draw_button(k, &format!("{key}-{m}"), bx, by, bw, b) {
                    events.push(UiEvent::Clicked(b.id.clone()));
                }
                bx += bw + dz(12.0);
            }
        }
        Node::Table(t) => {
            if t.rows.is_empty() {
                k.caps_text(x, y, w, &t.empty, TEXT_SIZE, design::GREY, 0.0);
                return h;
            }
            let mut ry = y;
            for (m, r) in t.rows.iter().enumerate() {
                let rh = row_height(k, r, w);
                hero::tile(k, x, ry, w, rh, false);
                let pad = dz(16.0);
                let widths: Vec<f32> = r.buttons.iter().map(|b| button_w(k, b)).collect();
                let bw_all: f32 =
                    widths.iter().sum::<f32>() + dz(10.0) * widths.len().saturating_sub(1) as f32;
                // The first line beside the buttons, the others as wide as the row.
                let tw = w - bw_all - pad * 3.0;
                let lw = w - pad * 2.0;
                // The first cell big, chips after it; the other cells and the detail below.
                let mut cx = x + pad;
                let mut first = true;
                let mut rest = Vec::new();
                for c in &r.cells {
                    if c.chip {
                        cx += chip(k, cx, ry + dz(12.0), &c.text, c.tone) + dz(10.0);
                    } else if first {
                        first = false;
                        let g = k.label_galley(&c.text, design::din(20.0), design::TEXT, tw);
                        let gw = g.size().x;
                        k.put(cx, ry + dz(10.0), g);
                        cx += gw + dz(14.0);
                    } else {
                        rest.push(c);
                    }
                }
                let mut ly = ry + dz(ROW_H - 8.0);
                if !rest.is_empty() {
                    let line: Vec<&str> = rest.iter().map(|c| c.text.as_str()).collect();
                    let tone = rest.first().map_or(types::Tone::Muted, |c| c.tone);
                    let color = if tone == types::Tone::Normal {
                        design::GREY
                    } else {
                        tone_color(tone)
                    };
                    let g = k.label_galley(&line.join(" · "), design::din(15.0), color, lw);
                    k.put(x + pad, ly, g);
                    ly += dz(ROW_LINE_H);
                }
                if let Some(g) = detail_galley(k, r, w) {
                    k.put(x + pad, ly, g);
                }
                let mut bx = x + w - pad - bw_all;
                let by = ry + (dz(ROW_H) - BTN_H) / 2.0;
                for (b, bw) in r.buttons.iter().zip(widths) {
                    if draw_button(k, &format!("{key}-r{m}-{}", b.id), bx, by, bw, b) {
                        events.push(UiEvent::Clicked(format!("{}/{}/{}", t.id, r.key, b.id)));
                    }
                    bx += bw + dz(10.0);
                }
                ry += rh + dz(ROW_GAP);
            }
        }
        Node::Chip(c) => {
            chip(k, x, y, &c.text, c.tone);
        }
        Node::Progress(pr) => {
            let mut py = y;
            if !pr.label.is_empty() {
                k.caption(x, y, &pr.label);
                py += dz(LABEL_H);
            }
            let r = k.rect(x, py, w, dz(12.0));
            k.ui.painter()
                .rect_filled(r, dz(6.0), egui::Color32::from_black_alpha(110));
            let fw = (w * pr.fraction.clamp(0.0, 1.0)).max(0.0);
            if fw > 0.0 {
                k.ui.painter()
                    .rect_filled(k.rect(x, py, fw, dz(12.0)), dz(6.0), design::BLUE);
            }
        }
        Node::LogView(l) => draw_log(d, k, &id, l, x, y, w, key),
        Node::Tabs(t) => {
            let titles: Vec<(&str, bool)> = t
                .labels
                .iter()
                .enumerate()
                .map(|(n, l)| (l.as_str(), n as u32 == t.selected))
                .collect();
            if let Some(n) = hero::card_tabs(k, &format!("{key}-tabs"), x, y, &titles) {
                events.push(UiEvent::Tab(t.id.clone(), n as u32));
            }
            let mut cy = y + dz(48.0);
            for (m, &ki) in t.children.iter().enumerate() {
                cy += draw_node(
                    d,
                    k,
                    ctx,
                    p,
                    page,
                    f,
                    ki,
                    x,
                    cy,
                    w,
                    &format!("{key}-{m}"),
                    depth + 1,
                    events,
                );
            }
        }
        Node::Form(section) => {
            if pl::draw_form(d, k, p, f, section, x, y, w, key) {
                events.push(UiEvent::SettingsChanged);
            }
        }
        Node::Presets => {
            if pl::draw_presets_of(d, k, p, f, x, y, w, key) {
                events.push(UiEvent::SettingsChanged);
            }
        }
    }
    h
}

/// A state's chip at (x, y): its width.
fn chip(k: &Kit, x: f32, y: f32, text: &str, tone: types::Tone) -> f32 {
    let color = match tone {
        types::Tone::Normal => design::BLUE,
        types::Tone::Muted => design::QUEST_OFF,
        other => tone_color(other),
    };
    let g = k.label_galley(
        &text.to_uppercase(),
        design::din(14.0),
        design::TEXT,
        f32::INFINITY,
    );
    let (gw, gh) = (g.size().x, g.size().y);
    let r = k.rect(x, y, gw + dz(20.0), dz(26.0));
    k.ui.painter()
        .rect_filled(r, dz(13.0), color.gamma_multiply(0.55));
    k.put(x + dz(10.0), y + (dz(26.0) - gh) / 2.0, g);
    gw + dz(20.0)
}

#[allow(clippy::too_many_arguments)]
fn draw_log(
    d: &mut Dashboard,
    k: &mut Kit,
    id: &str,
    l: &types::LogView,
    x: f32,
    y: f32,
    w: f32,
    key: &str,
) {
    let lh = dz(LINE_H);
    let area_h = lh * l.lines.clamp(4, 60) as f32;
    let Some(stream) = d
        .plugins
        .code
        .get_mut(id)
        .and_then(|v| v.logs.get_mut(&l.log))
    else {
        k.caps_text(x, y, w, "No console.", 15.0, design::GREY, 0.0);
        return;
    };
    let shown = shown_lines(&stream.lines, &l.filter, l.noise);
    let content = shown.len() as f32 * lh;
    if l.follow && stream.follow {
        stream.scroll = (content - area_h).max(0.0);
    }
    let before = stream.scroll;
    k.scroll_area(
        &format!("{key}-log"),
        x,
        y,
        w + dz(16.0),
        area_h,
        content,
        &mut stream.scroll,
    );
    if stream.scroll < before - 0.5 {
        stream.follow = false;
    } else if content - area_h - stream.scroll < 1.0 {
        stream.follow = true;
    }
    let scroll = stream.scroll;
    k.ui.painter().rect_filled(
        k.rect(x - dz(6.0), y - dz(4.0), w + dz(12.0), area_h + dz(8.0)),
        dz(4.0),
        egui::Color32::from_rgba_unmultiplied(6, 2, 16, 200),
    );
    let font = egui::FontId::monospace(dz(14.5));
    k.clipped(x - dz(6.0), y, w + dz(12.0), area_h, |k| {
        let first = (scroll / lh).floor().max(0.0) as usize;
        let n = (area_h / lh).ceil() as usize + 1;
        for (i, line) in shown.iter().enumerate().skip(first).take(n) {
            let ly = y - scroll + i as f32 * lh;
            let text: String = line.chars().take(400).collect();
            k.ui.painter().text(
                k.rect(x, ly, 1.0, lh).left_center(),
                egui::Align2::LEFT_CENTER,
                text,
                font.clone(),
                line_color(line),
            );
        }
    });
    if shown.is_empty() {
        let note = if stream.lines.is_empty() {
            "Nothing yet: the console fills while the server runs."
        } else {
            "No line matches."
        };
        k.caps_text(x, y + dz(10.0), w, note, 15.0, design::GREY, 0.0);
    }
}

/// Snapshots: Server host with four made-up servers (three running, one waiting to start
/// again), on its tab or card of variant `v`; not there for any other variant.
pub(super) fn snap_server_host(d: &mut Dashboard, v: Option<super::SnapVariant>) {
    use super::SnapVariant as V;
    use crate::core::server_host::{self as host, Account, Config, SignIn};
    use status::{State, Status};
    let id = host::PLUGIN_ID;
    d.plugins.list.retain(|p| p.page.id != id);
    stop(d, id);
    let queued = match v {
        Some(V::ServerHost) => vec![],
        Some(V::ServerHostConsole) => vec![UiEvent::Tab("tab".into(), 1)],
        Some(V::ServerHostDoctor) => vec![UiEvent::Tab("tab".into(), 2)],
        Some(V::ServerHostEdit) => vec![UiEvent::Clicked("servers/echo-2/edit".into())],
        Some(V::ServerHostSettings) => vec![UiEvent::Clicked("settings".into())],
        _ => return,
    };
    let dir = std::env::temp_dir().join("echovr-snap-server-host");
    host::snap_dir(dir.clone());
    let _ = std::fs::remove_dir_all(&dir);
    let t = crate::core::echovrce::now();
    let mut cfg = Config {
        account: Account {
            sign_in: SignIn::Device,
            discord_id: "123456789012345678".into(),
            user_id: "u".into(),
            username: "mias-servers".into(),
        },
        ..Config::default()
    };
    cfg.publisher_lock = "echovrce".into();
    let _ = cfg.add(4);
    for (i, s) in cfg.servers.iter_mut().enumerate() {
        s.run = i < 3;
        s.regions = "uwu".into();
    }
    cfg.servers[1].guilds = "1234567890".into();
    let _ = cfg.save();
    for (i, (name, state, matches)) in [
        ("echo-1", State::InSession, 6),
        ("echo-2", State::Registered, 3),
        ("echo-3", State::Backoff, 0),
    ]
    .into_iter()
    .enumerate()
    {
        let mut s = Status {
            name: name.into(),
            state,
            since: t - 47 * 60,
            updated: t + 3600,
            port: host::FIRST_PORT + i as u16,
            public_ip: "203.0.113.7".into(),
            runtime: "5.4.3+0.3caed2ef".into(),
            matches,
            restarts: matches.max(2),
            ..Status::default()
        };
        if state == State::Backoff {
            s.reason = Some(events::Reason::UdpUnreachable);
            s.last_error =
                "[NEVR.FATAL] registration rejected by ServerDB code=10 (broadcaster unreachable)"
                    .into();
            s.next_start = t + 37;
        }
        let sd = host::server_dir(name);
        let _ = std::fs::create_dir_all(&sd);
        let _ = status::write(&sd, &s);
    }
    let console = [
        "[host] starting echo-1 on port 6792 (HTTP API 7792)",
        r#"{"ts":"2026-10-23T15:45:44.072Z","level":"info","msg":"[NEVR.PATCH] NEVR-BUILD 5.4.3 3caed2ef1064f4f55b734bfec1a0047d865464d9"}"#,
        r#"{"ts":"2026-10-23T15:45:51.648Z","level":"info","msg":"[NEVR.WS] login injected xpid=OVR-ORG-123456789012345678 platform=4 conn=1 (login)"}"#,
        r#"{"ts":"2026-10-23T15:45:52.321Z","level":"info","msg":"[NEVR.WS] LOGIN SUCCESS"}"#,
        r#"{"ts":"2026-10-23T15:45:52.519Z","level":"info","msg":"[EVR] Beginning multiplayer"}"#,
        r#"{"ts":"2026-10-23T15:45:52.655Z","level":"info","msg":"[EVR] Dedicated: broadcaster initialized at [192.168.1.20:6792]"}"#,
        r#"{"ts":"2026-10-23T15:45:55.622Z","level":"info","msg":"[NEVR.SERVERDB] Connecting to ServerDB at ws://g.echovrce.com:80/nevr"}"#,
        r#"{"ts":"2026-10-23T15:45:56.104Z","level":"info","msg":"[EVR] Dedicated: registered as 0xE4A74026793A4079 at [192.168.1.20/203.0.113.7:6792]"}"#,
        "[host] READY: registered as 203.0.113.7:6792",
        r#"{"ts":"2026-10-23T15:52:10.880Z","level":"info","msg":"[NEVR.GAMESERVER] session starting session_id=7a1c-44 mode=echo_arena"}"#,
        r#"{"ts":"2026-10-23T15:52:11.002Z","level":"warn","msg":"[NEVR.PATCH] slow frame 41 ms (tick 120)"}"#,
    ];
    let _ = std::fs::write(
        host::server_dir("echo-1").join(status::CONSOLE),
        console.join("\n") + "\n",
    );
    let (page, _) = core::read_description(include_str!(
        "../../../docs/plugins/server-host/plugin.json"
    ));
    let Some(page) = page else {
        return;
    };
    d.plugins.list.push(Installed {
        from_catalog: Some(page.version.clone()),
        page: Arc::new(page),
        dir: std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/plugins/server-host"),
        first_party: true,
        ..Default::default()
    });
    d.plugins.code.entry(id.into()).or_default().queued = queued;
}

/// Snapshots: the made-up Doctor's checks (`plugin_services`' doctor in demo mode).
pub(super) fn snap_checks() -> serde_json::Value {
    let c = |name: &str, level: &str, text: &str, fix: &str, label: &str| serde_json::json!({"name": name, "level": level, "text": text, "fix": fix, "label": label});
    serde_json::json!([
        c("Game", "good", "Live build with nEVR 5.4.3+0.3caed2ef.", "", ""),
        c("Account", "good", "Signs in as mias-servers (linked).", "", ""),
        c("Disk", "good", "212 GB free.", "", ""),
        c("Ports", "bad", "In use by another program: 6795 (echo-4). Give the server another port.", "", ""),
        c("Address", "good", "This PC is 203.0.113.7 on the internet. Forward each server's port (UDP and TCP) to this PC with the same number outside (6792, 6793, 6794, 6795): EchoVRCE checks it when a server registers.", "", ""),
        c("Firewall", "warn", "Windows Firewall may block port(s) 6795.", "firewall", "Let them in"),
    ])
}
