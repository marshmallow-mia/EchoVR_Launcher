//! The Server host plugin's page (built in: `"builtin": "server-host"`). Left, the servers
//! (each with what it is doing, Start / Stop, Restart, its console, Edit, Delete), their
//! live console, and the Doctor; right, the servers' account and what runs in the
//! background (the server host process, starting with the PC, alerts). The servers run in
//! `EchoVR_Launcher --server-host` (`core::server_host::supervisor`), which this page
//! starts and reads: it never runs a game itself.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use super::{hero, panel, setup, Dashboard};
use crate::core::echovrce::{self, DeviceCode, Poll, Tokens};
use crate::core::server_host::account::{self, Registered};
use crate::core::server_host::doctor::{self, Check, Fix, Level};
use crate::core::server_host::events;
use crate::core::server_host::status::{self, Command, State, Status};
use crate::core::server_host::{self as host, autostart, Account, Config, Server, SignIn};
use crate::ui::design::{self, dz, Dr};
use crate::ui::dialogs::Icon as DlgIcon;
use crate::ui::kit::Kit;
use crate::ui::parts::Worker;
use crate::ui::style::Icon;
use crate::ui::widgets::{Tone, BTN_H};

const LEFT: Dr = Dr::new(137.0, 156.0, 1146.0, 890.0);
const ACCOUNT: Dr = Dr::new(1318.0, 156.0, 555.0, 470.0);
const BACKGROUND: Dr = Dr::new(1318.0, 656.0, 555.0, 390.0);
const ROW_H: f32 = 118.0;
/// Console lines kept.
const CONSOLE_LINES: usize = 5000;
const LINE_H: f32 = 22.0;
const STOP_KEY: &str = "server-host-stop";
const RESTART_KEY: &str = "server-host-restart";
const DELETE_KEY: &str = "server-host-delete";
const SIGN_OUT_KEY: &str = "server-host-sign-out";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Tab {
    #[default]
    Servers,
    Console,
    Doctor,
}

/// A card over the page.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Card {
    /// A server's settings, as typed (`new`: added just now).
    Edit(Box<EditForm>),
    /// The defaults for new servers, the services, the version.
    Settings(Box<SettingsForm>),
    /// The account's Discord ID and password.
    Password {
        discord_id: String,
        password: String,
    },
    /// What EchoVRCE has registered for the account.
    Registered,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EditForm {
    pub name: String,
    pub port: String,
    pub regions: String,
    pub guilds: String,
    pub tick_rate: String,
    pub empty_ttl: String,
    pub mods: bool,
    pub extra_args: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SettingsForm {
    pub version: String,
    pub regions: String,
    pub guilds: String,
    pub tick_rate: String,
    pub socket_uri: String,
    pub serverdb_uri: String,
    pub http_uri: String,
    pub http_key: String,
    pub server_key: String,
    pub publisher_lock: String,
    pub internal_ip: String,
    pub external_ip: String,
    pub error: Option<String>,
}

enum Msg {
    Linked(Result<(Account, Tokens), String>),
    Code(Result<DeviceCode, String>),
    Polled(String, Result<Poll, String>),
    CodeAccount(Result<(Account, Tokens), String>),
    Password(String, String, Result<Tokens, String>),
    Tested(Result<String, String>),
    Registered(Result<Vec<Registered>, String>),
    Doctor(Vec<Check>),
    Fixed(Result<String, String>),
    Webhook(Result<(), String>),
    Running(bool),
}

/// The console of one server, as shown.
#[derive(Default)]
struct ConsoleView {
    server: String,
    tail: Option<status::Tail>,
    lines: VecDeque<String>,
}

/// The page's state.
#[derive(Default)]
pub(super) struct HostUi {
    worker: Worker<Msg>,
    cfg: Option<Config>,
    statuses: HashMap<String, Status>,
    read_at: Option<Instant>,
    /// The server host process runs (looked at every few seconds).
    running: Option<bool>,
    running_at: Option<Instant>,
    checking_running: bool,
    pub tab: Tab,
    pub card: Option<Card>,
    scroll: f32,
    add_n: String,
    // Console.
    console: ConsoleView,
    follow: bool,
    noise: bool,
    filter: String,
    console_scroll: f32,
    // Account.
    busy: bool,
    code: Option<(DeviceCode, Instant)>,
    account_note: Option<(String, bool)>,
    registered: Option<Result<Vec<Registered>, String>>,
    // Doctor.
    checks: Option<Vec<Check>>,
    checking: bool,
    // Background.
    webhook: String,
    webhook_note: Option<(String, bool)>,
    autostart: Option<bool>,
    /// What a confirmation is about (a server's name).
    asked: Option<String>,
}

impl HostUi {
    fn cfg(&mut self) -> &mut Config {
        self.cfg.get_or_insert_with(Config::load)
    }
}

fn now() -> i64 {
    echovrce::now()
}

/// Saves the servers' settings (a failure is said).
fn save(d: &mut Dashboard) {
    let c = d.server_host.cfg().clone();
    if let Err(e) = c.save() {
        d.dialogs
            .error("Server host", &format!("{e:#}"), Default::default());
    }
}

/// What the files say now: every second while the page is open.
fn refresh(d: &mut Dashboard, ctx: &egui::Context) {
    if d.demo {
        return;
    }
    let h = &mut d.server_host;
    if h.read_at
        .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
    {
        return;
    }
    h.read_at = Some(Instant::now());
    let names: Vec<String> = h.cfg().servers.iter().map(|s| s.name.clone()).collect();
    h.statuses = names
        .iter()
        .filter_map(|n| status::read(&host::server_dir(n)).map(|s| (n.clone(), s)))
        .collect();
    // The console of the server shown.
    if h.console.server.is_empty() || !names.contains(&h.console.server) {
        h.console = ConsoleView {
            server: names.first().cloned().unwrap_or_default(),
            ..ConsoleView::default()
        };
    }
    if !h.console.server.is_empty() {
        let dir = host::server_dir(&h.console.server);
        let tail = h
            .console
            .tail
            .get_or_insert_with(|| status::Tail::new(&dir, 2 * 1024 * 1024));
        for l in tail.read() {
            h.console.lines.push_back(l);
        }
        while h.console.lines.len() > CONSOLE_LINES {
            h.console.lines.pop_front();
        }
    }
    if !h.checking_running
        && h.running_at
            .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
    {
        h.checking_running = true;
        h.running_at = Some(Instant::now());
        h.worker.spawn(ctx, |tx| {
            tx.send(Msg::Running(crate::core::tray::server_host_running()))
        });
    }
    if h.autostart.is_none() {
        h.autostart = Some(autostart::on());
        h.webhook = h.cfg().alerts.webhook.clone();
        h.follow = true;
        h.add_n = "1".into();
    }
    ctx.request_repaint_after(Duration::from_millis(500));
}

fn answers(d: &mut Dashboard, ctx: &egui::Context) {
    for m in d.server_host.worker.drain() {
        let h = &mut d.server_host;
        match m {
            Msg::Running(r) => {
                h.running = Some(r);
                h.checking_running = false;
            }
            Msg::Linked(r) | Msg::CodeAccount(r) => {
                h.busy = false;
                h.code = None;
                match r {
                    Ok((a, t)) => {
                        let who = a.username.clone();
                        let r = account::use_device(h.cfg(), a, t);
                        match r {
                            Ok(()) => {
                                h.account_note =
                                    Some((format!("The servers sign in as {who} now."), true));
                                save(d);
                                restart_all_idle(d);
                            }
                            Err(e) => h.account_note = Some((format!("{e:#}"), false)),
                        }
                    }
                    Err(e) => h.account_note = Some((e, false)),
                }
            }
            Msg::Code(r) => match r {
                Ok(code) => {
                    let _ = open::that(echovrce::approve_url(&code.code));
                    h.code = Some((code, Instant::now()));
                    poll(d, ctx);
                }
                Err(e) => {
                    h.busy = false;
                    h.account_note = Some((e, false));
                }
            },
            Msg::Polled(code, r) => {
                let current = h.code.as_ref().is_some_and(|(c, _)| c.code == code);
                if !current {
                    continue;
                }
                match r {
                    Ok(Poll::Approved(t)) => {
                        h.worker.spawn(ctx, move |tx| {
                            let r = account::from_code(&t)
                                .map(|a| (a, t))
                                .map_err(|e| format!("{e:#}"));
                            tx.send(Msg::CodeAccount(r));
                        });
                    }
                    Ok(Poll::Pending) => poll(d, ctx),
                    Err(e)
                        if crate::core::echovrce::is_unavailable(&anyhow::anyhow!(e.clone())) =>
                    {
                        poll(d, ctx)
                    }
                    Err(e) => {
                        h.busy = false;
                        h.code = None;
                        h.account_note = Some((e, false));
                    }
                }
            }
            Msg::Password(id, pw, r) => {
                h.busy = false;
                match r {
                    Ok(t) => match account::keep_password(h.cfg(), &id, &pw, &t) {
                        Ok(guilds) => {
                            h.card = None;
                            h.account_note = Some((guild_note(&guilds), true));
                            save(d);
                            restart_all_idle(d);
                        }
                        Err(e) => h.account_note = Some((format!("{e:#}"), false)),
                    },
                    Err(e) => h.account_note = Some((e, false)),
                }
            }
            Msg::Tested(r) => {
                h.busy = false;
                h.account_note = Some(match r {
                    Ok(t) => (t, true),
                    Err(e) => (e, false),
                });
            }
            Msg::Registered(r) => {
                h.busy = false;
                h.registered = Some(r);
            }
            Msg::Doctor(c) => {
                h.checking = false;
                h.checks = Some(c);
            }
            Msg::Fixed(r) => {
                match r {
                    Ok(t) => d.notify(&t),
                    Err(e) => d.dialogs.error("Server host", &e, Default::default()),
                }
                run_doctor(d, ctx);
            }
            Msg::Webhook(r) => {
                h.webhook_note = Some(match r {
                    Ok(()) => ("Saved: a test message went to the webhook.".into(), true),
                    Err(e) => (e, false),
                });
            }
        }
    }
    // The confirmations.
    if let Some(a) = d.dialogs.take(STOP_KEY) {
        if let Some(name) = d.server_host.asked.take() {
            match a {
                crate::ui::dialogs::Answer::Button(0) => set_run(d, &name, false),
                crate::ui::dialogs::Answer::Button(1) => {
                    let _ = status::ask(&host::server_dir(&name), Command::StopNow);
                    set_run(d, &name, false);
                }
                _ => {}
            }
        }
    }
    if let Some(a) = d.dialogs.take(RESTART_KEY) {
        if let Some(name) = d.server_host.asked.take() {
            let dir = host::server_dir(&name);
            match a {
                crate::ui::dialogs::Answer::Button(0) => {
                    let _ = status::ask(&dir, Command::Apply);
                }
                crate::ui::dialogs::Answer::Button(1) => {
                    let _ = status::ask(&dir, Command::Restart);
                }
                _ => {}
            }
        }
    }
    if let Some(a) = d.dialogs.take(DELETE_KEY) {
        if let (true, Some(name)) = (a.is_yes(), d.server_host.asked.take()) {
            delete(d, &name);
        }
    }
    if d.dialogs.take(SIGN_OUT_KEY).is_some_and(|a| a.is_yes()) {
        let r = account::forget(d.server_host.cfg());
        save(d);
        d.server_host.account_note = Some(match r {
            Ok(()) => (
                "Signed out. Running servers keep going until they start again.".into(),
                true,
            ),
            Err(e) => (format!("{e:#}"), false),
        });
    }
}

fn guild_note(guilds: &[String]) -> String {
    if guilds.is_empty() {
        "Signed in. No guild gave this account its server host role: the servers host public matches.".into()
    } else {
        format!(
            "Signed in. It may host for {} guild(s): {}.",
            guilds.len(),
            guilds.join(", ")
        )
    }
}

fn poll(d: &mut Dashboard, ctx: &egui::Context) {
    let Some((code, at)) = d.server_host.code.clone() else {
        return;
    };
    if at.elapsed() > code.expires_in {
        d.server_host.code = None;
        d.server_host.busy = false;
        d.server_host.account_note = Some(("The code ran out: link again.".into(), false));
        return;
    }
    d.server_host.worker.spawn(ctx, move |tx| {
        std::thread::sleep(Duration::from_secs(3));
        let r = echovrce::poll(&code.code).map_err(|e| format!("{e:#}"));
        tx.send(Msg::Polled(code.code, r));
    });
}

// ---- what the buttons do ----

fn set_run(d: &mut Dashboard, name: &str, run: bool) {
    if let Some(s) = d.server_host.cfg().server_mut(name) {
        s.run = run;
    }
    save(d);
    if run {
        start_host(d);
    }
}

/// Starts the server host process unless it runs.
fn start_host(d: &mut Dashboard) {
    if d.server_host.running != Some(true) {
        host::supervisor::ensure_running();
        d.server_host.running = Some(true);
        d.server_host.running_at = Some(Instant::now());
    }
}

/// After the account changed: the idle servers start again with it.
fn restart_all_idle(d: &mut Dashboard) {
    let names: Vec<String> = d
        .server_host
        .cfg()
        .servers
        .iter()
        .filter(|s| s.run)
        .map(|s| s.name.clone())
        .collect();
    for n in names {
        let _ = status::ask(&host::server_dir(&n), Command::Apply);
    }
}

fn stop(d: &mut Dashboard, name: &str) {
    let in_match = d
        .server_host
        .statuses
        .get(name)
        .is_some_and(|s| s.state == State::InSession && s.fresh(now()));
    if in_match {
        d.server_host.asked = Some(name.to_string());
        d.dialogs.options(
            STOP_KEY,
            &format!("{name} is hosting a match"),
            "Stop it once the match is over, or now (the players in it are dropped)?",
            DlgIcon::Question,
            &["After the match", "Now", "Cancel"],
        );
    } else {
        set_run(d, name, false);
    }
}

fn restart(d: &mut Dashboard, name: &str) {
    let in_match = d
        .server_host
        .statuses
        .get(name)
        .is_some_and(|s| s.state == State::InSession && s.fresh(now()));
    if in_match {
        d.server_host.asked = Some(name.to_string());
        d.dialogs.options(
            RESTART_KEY,
            &format!("{name} is hosting a match"),
            "Start it again once the match is over, or now (the players in it are dropped)?",
            DlgIcon::Question,
            &["After the match", "Now", "Cancel"],
        );
    } else {
        let _ = status::ask(&host::server_dir(name), Command::Restart);
        d.notify(&format!("{name} starts again"));
    }
}

/// Deletes a server: it stops, and its folder (game folder, logs, prefix) goes once it
/// has.
fn delete(d: &mut Dashboard, name: &str) {
    let dir = host::server_dir(name);
    let _ = status::ask(&dir, Command::StopNow);
    d.server_host.cfg().servers.retain(|s| s.name != name);
    save(d);
    let name = name.to_string();
    std::thread::spawn(move || {
        // Until the server host let go of it (at most a minute).
        for _ in 0..60 {
            let stopped =
                status::read(&dir).is_none_or(|s| s.state == State::Stopped || !s.fresh(now()));
            if stopped {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!("server host: removing {}: {e}", dir.display());
        }
        let compat = crate::core::linux::echoxr::server_compat_data(&format!("host-{name}"));
        if cfg!(target_os = "linux") && compat.is_dir() {
            let _ = std::fs::remove_dir_all(compat);
        }
    });
    d.notify("Server deleted");
}

fn add(d: &mut Dashboard) {
    let n: usize = d
        .server_host
        .add_n
        .trim()
        .parse()
        .unwrap_or(1)
        .clamp(1, host::MAX_SERVERS);
    match d.server_host.cfg().add(n) {
        Ok(names) => {
            save(d);
            d.notify(&format!("Added {}", names.join(", ")));
        }
        Err(e) => d
            .dialogs
            .error("Server host", &format!("{e:#}"), Default::default()),
    }
}

fn edit_form(s: &Server) -> EditForm {
    EditForm {
        name: s.name.clone(),
        port: s.port.to_string(),
        regions: s.regions.clone(),
        guilds: s.guilds.clone(),
        tick_rate: s.tick_rate.to_string(),
        empty_ttl: s.empty_ttl.to_string(),
        mods: s.mods,
        extra_args: s.extra_args.clone(),
        error: None,
    }
}

fn settings_form(c: &Config) -> SettingsForm {
    SettingsForm {
        version: c.version.clone(),
        regions: c.defaults.regions.clone(),
        guilds: c.defaults.guilds.clone(),
        tick_rate: c.defaults.tick_rate.to_string(),
        socket_uri: c.services.socket_uri.clone(),
        serverdb_uri: c.services.serverdb_uri.clone(),
        http_uri: c.services.http_uri.clone(),
        http_key: c.services.http_key.clone(),
        server_key: c.services.server_key.clone(),
        publisher_lock: c.publisher_lock.clone(),
        internal_ip: c.internal_ip.clone(),
        external_ip: c.external_ip.clone(),
        error: None,
    }
}

fn run_doctor(d: &mut Dashboard, ctx: &egui::Context) {
    if d.server_host.checking {
        return;
    }
    d.server_host.checking = true;
    let c = d.server_host.cfg().clone();
    let running: Vec<String> = d
        .server_host
        .statuses
        .values()
        .filter(|s| s.fresh(now()) && s.state != State::Stopped)
        .map(|s| s.name.clone())
        .collect();
    d.server_host.worker.spawn(ctx, move |tx| {
        tx.send(Msg::Doctor(doctor::run(&c, &running)));
    });
}

fn fix(d: &mut Dashboard, ctx: &egui::Context, f: &Fix) {
    match f {
        Fix::None => {}
        Fix::SignIn => d.server_host.tab = Tab::Servers,
        Fix::Game => d.page = super::Page::Play,
        Fix::Firewall(ports) => {
            let ports = ports.clone();
            let mut consent = setup::consent_asker(d.worker.tx(ctx));
            d.server_host.worker.spawn(ctx, move |tx| {
                let r = crate::core::elevation::allow_server_ports(&ports, &mut consent)
                    .map(|()| "Windows Firewall lets the servers' ports in".to_string())
                    .map_err(|e| format!("{e:#}"));
                tx.send(Msg::Fixed(r));
            });
        }
        Fix::Defender => {
            let dir = doctor::servers_folder();
            let mut consent = setup::consent_asker(d.worker.tx(ctx));
            d.server_host.worker.spawn(ctx, move |tx| {
                let _ = std::fs::create_dir_all(&dir);
                let r = crate::core::elevation::exclude_from_defender(&dir, &mut consent)
                    .map(|()| "Windows Security leaves the servers' folder alone".to_string())
                    .map_err(|e| format!("{e:#}"));
                tx.send(Msg::Fixed(r));
            });
        }
    }
}

// ---- drawing ----

/// The page.
pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    refresh(d, ctx);
    answers(d, ctx);
    let (dx, dy) = (kit.dx(), kit.dy());
    left_card(d, kit, ctx, LEFT.wider(dx).taller(dy));
    panel::at_right(kit, |k| {
        account_card(d, k, ctx, ACCOUNT);
        background_card(d, k, ctx, BACKGROUND.taller(dy));
    });
    if d.server_host.card.is_some() {
        let blocked = kit.blocked;
        kit.modal("server-host-card", blocked, |k| card(d, k, ctx));
    }
}

fn left_card(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context, r: Dr) {
    let (x, _, w, _) = hero::card_frame_bare(kit, r);
    let top = dz(r.y + 20.0);
    let tabs = [
        (Tab::Servers, "Servers"),
        (Tab::Console, "Console"),
        (Tab::Doctor, "Doctor"),
    ];
    let titles = tabs.map(|(t, title)| (title, d.server_host.tab == t));
    if let Some(n) = hero::card_tabs(kit, "server-host-tab", x, top, &titles) {
        d.server_host.tab = tabs[n].0;
        if d.server_host.tab == Tab::Doctor && d.server_host.checks.is_none() {
            run_doctor(d, ctx);
        }
    }
    let body = top + dz(62.0);
    let bottom = dz(r.bottom() - 22.0);
    match d.server_host.tab {
        Tab::Servers => servers_tab(d, kit, ctx, x, top, body, w, bottom),
        Tab::Console => console_tab(d, kit, x, top, body, w, bottom),
        Tab::Doctor => doctor_tab(d, kit, ctx, x, top, body, w, bottom),
    }
}

/// A state's chip colour.
fn state_color(s: Option<&Status>) -> egui::Color32 {
    let t = now();
    match s {
        Some(s) if !s.fresh(t) && s.state != State::Stopped => design::QUEST_OFF,
        Some(s) => match s.state {
            State::Registered => design::QUEST_ON,
            State::InSession => design::BLUE,
            State::Starting | State::Stopping | State::Backoff => design::QUEST_WARN,
            State::Failed => design::RED,
            State::Stopped => design::QUEST_OFF,
        },
        None => design::QUEST_OFF,
    }
}

/// Pure: how long, in words ("3 min", "2 h 5 min").
fn span(secs: i64) -> String {
    let m = secs.max(0) / 60;
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h {} min", m / 60, m % 60)
    }
}

#[allow(clippy::too_many_arguments)]
fn servers_tab(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    x: f32,
    top: f32,
    body: f32,
    w: f32,
    bottom: f32,
) {
    // The title row's right: Settings. Under the tabs: Add (how many), Start all, Stop all.
    let s = dz(36.0);
    if kit
        .button(
            "server-host-settings",
            x + w - s,
            top,
            s,
            s,
            Tone::Dark,
            Some(Icon::Gear),
            "",
            true,
            "The defaults for new servers, the services, the game version",
        )
        .clicked
    {
        let f = settings_form(d.server_host.cfg());
        d.server_host.card = Some(Card::Settings(Box::new(f)));
    }
    let nw = dz(56.0);
    let bad_n = d.server_host.add_n.trim().parse::<usize>().is_err();
    kit.field(
        "server-host-add-n",
        &mut d.server_host.add_n,
        x,
        body,
        nw,
        s,
        "1",
        bad_n,
        "How many servers Add adds",
    );
    let full = d.server_host.cfg().servers.len() >= host::MAX_SERVERS;
    if kit
        .button(
            "server-host-add",
            x + nw + dz(8.0),
            body,
            dz(100.0),
            s,
            Tone::Blue,
            Some(Icon::Plus),
            "Add",
            !full,
            if full {
                "This PC has as many servers as the launcher runs: delete one first"
            } else {
                "Add servers with the defaults, on the next free ports"
            },
        )
        .clicked
    {
        add(d);
    }
    let any = !d.server_host.cfg().servers.is_empty();
    let lw = dz(120.0);
    let names: Vec<String> = d
        .server_host
        .cfg()
        .servers
        .iter()
        .map(|s| s.name.clone())
        .collect();
    if kit
        .button(
            "server-host-stop-all",
            x + w - lw,
            body,
            lw,
            s,
            Tone::Dark,
            None,
            "Stop all",
            any,
            "Stop every server (one in a match after its match)",
        )
        .clicked
    {
        for n in &names {
            set_run(d, n, false);
        }
    }
    if kit
        .button(
            "server-host-start-all",
            x + w - 2.0 * lw - dz(10.0),
            body,
            lw,
            s,
            Tone::Go,
            None,
            "Start all",
            any,
            "Start every server",
        )
        .clicked
    {
        if d.server_host.cfg().account.sign_in == SignIn::None {
            d.dialogs.info(
                "Sign the servers in first",
                "The servers need an account of their own on EchoVRCE: Account, on the right.",
            );
        } else {
            for n in &names {
                set_run(d, n, true);
            }
        }
    }

    let mut y = body + s + dz(20.0);
    // Wanted, but nothing runs them.
    let wanted = d.server_host.cfg().servers.iter().any(|s| s.run);
    if wanted && d.server_host.running == Some(false) {
        let tw = w - dz(220.0);
        let th = kit.caps_text(
            x,
            y,
            tw,
            "The server host isn't running, so the servers set to run aren't.",
            16.0,
            design::QUEST_WARN,
            0.0,
        );
        if kit
            .button(
                "server-host-run",
                x + w - dz(200.0),
                y - dz(6.0),
                dz(200.0),
                BTN_H,
                Tone::Go,
                None,
                "Start it",
                true,
                "Start the process that runs the servers",
            )
            .clicked
        {
            start_host(d);
        }
        y += th.max(BTN_H) + dz(14.0);
    }
    if d.server_host.cfg().account.sign_in == SignIn::None {
        let th = kit.caps_text(x, y, w, "Sign the servers in first (Account, on the right): they need an account of their own on EchoVRCE.", 16.0, design::QUEST_WARN, 0.0);
        y += th + dz(14.0);
    }
    let servers = d.server_host.cfg().servers.clone();
    if servers.is_empty() {
        kit.caps_text(x, y, w, &format!("No servers yet. Add one: it gets the next free port from {}. Each server's port has to be forwarded (UDP and TCP, the same number outside) to this PC.", host::FIRST_PORT), 16.0, design::BODY, 0.0);
        return;
    }
    let row_h = dz(ROW_H);
    let list_h = bottom - y;
    let pad = dz(12.0);
    kit.scroll_area(
        "server-host-list",
        x,
        y,
        w + pad + dz(4.0),
        list_h,
        servers.len() as f32 * row_h,
        &mut d.server_host.scroll,
    );
    let scroll = d.server_host.scroll;
    let mut action: Option<(&'static str, String)> = None;
    let t = now();
    let statuses = d.server_host.statuses.clone();
    kit.clipped(x - pad, y, w + 2.0 * pad, list_h, |k| {
        for (i, s) in servers.iter().enumerate() {
            let ry = y - scroll + i as f32 * row_h;
            if ry + row_h < y || ry > bottom {
                continue;
            }
            let rr = k.rect(x - pad, ry, w + 2.0 * pad - dz(8.0), row_h - dz(8.0));
            if i % 2 == 0 {
                k.ui.painter()
                    .rect_filled(rr, dz(6.0), egui::Color32::from_white_alpha(6));
            }
            let st = statuses.get(&s.name);
            // Name and state.
            let name = k.label_galley(&s.name, design::din(20.0), design::TEXT, dz(260.0));
            let nw = name.size().x;
            k.put(x + dz(6.0), ry + dz(8.0), name);
            let summary = st.map_or_else(|| "Stopped".to_string(), |st| st.summary(t));
            let chip = match st {
                Some(st) if !s.run && st.state != State::Stopped && st.fresh(t) => {
                    "Stopping".to_string()
                }
                _ => summary.split(':').next().unwrap_or("").to_string(),
            };
            k.chip(
                x + dz(6.0) + nw + dz(14.0),
                ry + dz(8.0),
                &chip,
                state_color(st),
            );
            // Where and what.
            let mut line = vec![format!("Port {}", s.port)];
            let regions = host::list(&s.regions);
            line.push(if regions.is_empty() {
                "default region".into()
            } else {
                regions.join(", ")
            });
            if let Some(v) = st.map(|st| st.runtime.as_str()).filter(|v| !v.is_empty()) {
                line.push(format!("nEVR {v}"));
            }
            if !host::list(&s.guilds).is_empty() {
                line.push(format!("{} guild(s)", host::list(&s.guilds).len()));
            }
            let g = k.label_galley(
                &line.join("  ·  "),
                design::din(14.0),
                design::SUBTLE,
                dz(560.0),
            );
            k.put(x + dz(6.0), ry + dz(42.0), g);
            // What it last did, and how it has gone since it started.
            let mut detail = match st {
                Some(st) if st.state == State::Registered || st.state == State::InSession => {
                    format!("Registered at {}:{}", st.public_ip, st.port)
                }
                Some(st) if matches!(st.state, State::Failed | State::Backoff) => {
                    // In words; the log's own line is in its console.
                    let why = st
                        .reason
                        .map_or("It stopped on an error", events::Reason::words);
                    match st.next_start - t {
                        n if n > 0 => format!("{why} (again in {n} s)"),
                        _ => why.to_string(),
                    }
                }
                _ => String::new(),
            };
            if let Some(st) = st.filter(|st| st.fresh(t) && st.state != State::Stopped) {
                let up = format!(
                    "{} matches  ·  {} restarts  ·  up {}",
                    st.matches,
                    st.restarts,
                    span(t - st.since)
                );
                // An error alone: it says what to do.
                let failing = matches!(st.state, State::Failed | State::Backoff);
                detail = match (detail.is_empty(), failing) {
                    (true, _) => up,
                    (false, true) => detail,
                    (false, false) => format!("{detail}  ·  {up}"),
                };
            }
            if !detail.is_empty() {
                let g = k.label_galley(&detail, design::din(14.0), design::GREY, dz(770.0));
                k.put(x + dz(6.0), ry + dz(74.0), g);
            }
            // Buttons on the right: Start/Stop, Restart, Console, Edit, Delete.
            let bh = dz(40.0);
            let cy = ry + (row_h - dz(8.0)) / 2.0 - bh / 2.0;
            let sq = bh;
            let mut bx = x + w - sq - dz(8.0);
            if k.button(
                &format!("sh-del-{}", s.name),
                bx,
                cy,
                sq,
                sq,
                Tone::Dark,
                Some(Icon::Trash),
                "",
                true,
                "Delete this server",
            )
            .clicked
            {
                action = Some(("delete", s.name.clone()));
            }
            bx -= sq + dz(8.0);
            if k.button(
                &format!("sh-edit-{}", s.name),
                bx,
                cy,
                sq,
                sq,
                Tone::Dark,
                Some(Icon::Gear),
                "",
                true,
                "Its port, regions, guilds, tick rate and more",
            )
            .clicked
            {
                action = Some(("edit", s.name.clone()));
            }
            bx -= sq + dz(8.0);
            if k.button(
                &format!("sh-log-{}", s.name),
                bx,
                cy,
                sq,
                sq,
                Tone::Dark,
                Some(Icon::Info),
                "",
                true,
                "Its live console",
            )
            .clicked
            {
                action = Some(("console", s.name.clone()));
            }
            bx -= sq + dz(8.0);
            let alive = st.is_some_and(|st| st.fresh(t) && st.state != State::Stopped);
            if k.button(
                &format!("sh-restart-{}", s.name),
                bx,
                cy,
                sq,
                sq,
                Tone::Dark,
                Some(Icon::Refresh),
                "",
                s.run && alive,
                "Start it again (now, or after its match)",
            )
            .clicked
            {
                action = Some(("restart", s.name.clone()));
            }
            let bw = dz(110.0);
            bx -= bw + dz(8.0);
            let (label, tone, what) = if s.run {
                ("Stop", Tone::Dark, "stop")
            } else {
                ("Start", Tone::Go, "start")
            };
            if k.button(
                &format!("sh-run-{}", s.name),
                bx,
                cy,
                bw,
                bh,
                tone,
                None,
                label,
                true,
                "",
            )
            .clicked
            {
                action = Some((what, s.name.clone()));
            }
        }
    });
    if let Some((what, name)) = action {
        match what {
            "start" => {
                if d.server_host.cfg().account.sign_in == SignIn::None {
                    d.dialogs.info("Sign the servers in first", "The servers need an account of their own on EchoVRCE: Account, on the right.");
                } else {
                    set_run(d, &name, true);
                }
            }
            "stop" => stop(d, &name),
            "restart" => restart(d, &name),
            "console" => {
                d.server_host.console = ConsoleView {
                    server: name,
                    ..ConsoleView::default()
                };
                d.server_host.tab = Tab::Console;
                d.server_host.follow = true;
                d.server_host.read_at = None;
            }
            "edit" => {
                if let Some(s) = d.server_host.cfg().server(&name).cloned() {
                    d.server_host.card = Some(Card::Edit(Box::new(edit_form(&s))));
                }
            }
            "delete" => {
                d.server_host.asked = Some(name.clone());
                d.dialogs.confirm_danger(DELETE_KEY, &format!("Delete {name}?"), "It stops (now, even in a match), and its folder goes: its copy of the game's files, its logs and sign-in.", "Delete");
            }
            _ => {}
        }
    }
    let _ = ctx;
}

/// A console line's colour.
fn line_color(l: &str) -> egui::Color32 {
    if l.starts_with("[host]") {
        design::LINK
    } else if l.contains("FATAL") || l.contains(" error ") || l.contains("\"level\":\"error\"") {
        design::DANGER
    } else if l.contains("\"level\":\"warn\"") || l.contains(" warn ") {
        design::QUEST_WARN
    } else {
        design::BODY
    }
}

#[allow(clippy::too_many_arguments)]
fn console_tab(d: &mut Dashboard, kit: &mut Kit, x: f32, top: f32, body: f32, w: f32, bottom: f32) {
    let names: Vec<String> = d
        .server_host
        .cfg()
        .servers
        .iter()
        .map(|s| s.name.clone())
        .collect();
    if names.is_empty() {
        kit.caps_text(x, body, w, "No servers yet.", 16.0, design::BODY, 0.0);
        return;
    }
    // The title row's right: open its folder, copy what is shown.
    let s = dz(36.0);
    let dir = host::server_dir(&d.server_host.console.server);
    if kit
        .button(
            "sh-console-folder",
            x + w - s,
            top,
            s,
            s,
            Tone::Dark,
            Some(Icon::Folder),
            "",
            true,
            "Open the server's folder (its console log, game folder, nEVR's logs)",
        )
        .clicked
    {
        hero::open_folder(d, &dir.to_string_lossy());
    }
    let shown = shown_lines(d);
    if kit
        .button(
            "sh-console-copy",
            x + w - 2.0 * s - dz(10.0),
            top,
            s,
            s,
            Tone::Dark,
            Some(Icon::Copy),
            "",
            !shown.is_empty(),
            "Copy the lines shown",
        )
        .clicked
    {
        kit.ctx().copy_text(shown.join("\n"));
        d.notify("Copied the console");
    }
    // Which server: one button each, as many as fit.
    let mut bx = x;
    let mut pick = None;
    for n in &names {
        let bw = kit.button_width(n, None, dz(34.0)).max(dz(90.0));
        if bx + bw > x + w * 0.55 {
            break;
        }
        let on = *n == d.server_host.console.server;
        if kit
            .button(
                &format!("sh-console-{n}"),
                bx,
                body,
                bw,
                dz(34.0),
                if on { Tone::Blue } else { Tone::Dark },
                None,
                n,
                true,
                "",
            )
            .clicked
        {
            pick = Some(n.clone());
        }
        bx += bw + dz(8.0);
    }
    if let Some(n) = pick {
        d.server_host.console = ConsoleView {
            server: n,
            ..ConsoleView::default()
        };
        d.server_host.read_at = None;
        d.server_host.follow = true;
    }
    // Follow, nEVR's noise, the filter.
    let fw = dz(240.0);
    let fx = x + w - fw;
    kit.field(
        "sh-console-filter",
        &mut d.server_host.filter,
        fx,
        body,
        fw,
        dz(34.0),
        "Filter",
        false,
        "Only lines with this (any case)",
    );
    let mut noise = d.server_host.noise;
    let nx = fx - dz(20.0) - kit.check_width("Noise");
    if kit.check(
        "sh-console-noise",
        &mut noise,
        "Noise",
        nx,
        body + dz(5.0),
        true,
        "nEVR's high-volume diagnostics too",
    ) {
        d.server_host.noise = noise;
    }
    let mut follow = d.server_host.follow;
    let flx = nx - dz(20.0) - kit.check_width("Follow");
    if kit.check(
        "sh-console-follow",
        &mut follow,
        "Follow",
        flx,
        body + dz(5.0),
        true,
        "Keep the newest lines in view",
    ) {
        d.server_host.follow = follow;
    }
    // The lines.
    let ly = body + dz(50.0);
    let lh = dz(LINE_H);
    let area_h = bottom - ly;
    let font = egui::FontId::monospace(dz(14.5));
    let content = shown.len() as f32 * lh;
    if d.server_host.follow {
        d.server_host.console_scroll = (content - area_h).max(0.0);
    }
    let before = d.server_host.console_scroll;
    kit.scroll_area(
        "sh-console",
        x,
        ly,
        w + dz(16.0),
        area_h,
        content,
        &mut d.server_host.console_scroll,
    );
    if d.server_host.console_scroll < before - 0.5 {
        // Scrolled up: it stops following until Follow is ticked again or it's at the end.
        d.server_host.follow = false;
    } else if content - area_h - d.server_host.console_scroll < 1.0
        && !d.server_host.follow
        && d.server_host.console_scroll > before
    {
        d.server_host.follow = true;
    }
    let scroll = d.server_host.console_scroll;
    kit.ui.painter().rect_filled(
        kit.rect(x - dz(6.0), ly - dz(4.0), w + dz(12.0), area_h + dz(8.0)),
        dz(4.0),
        egui::Color32::from_rgba_unmultiplied(6, 2, 16, 200),
    );
    kit.clipped(x - dz(6.0), ly, w + dz(12.0), area_h, |k| {
        let first = (scroll / lh).floor().max(0.0) as usize;
        let n = (area_h / lh).ceil() as usize + 1;
        for (i, l) in shown.iter().enumerate().skip(first).take(n) {
            let y = ly - scroll + i as f32 * lh;
            let text: String = l.chars().take(400).collect();
            k.ui.painter().text(
                k.rect(x, y, 1.0, lh).left_center(),
                egui::Align2::LEFT_CENTER,
                text,
                font.clone(),
                line_color(l),
            );
        }
    });
    if shown.is_empty() {
        let note = if d.server_host.console.lines.is_empty() {
            "Nothing yet: the console fills while the server runs."
        } else {
            "No line matches."
        };
        kit.caps_text(x, ly + dz(10.0), w, note, 15.0, design::GREY, 0.0);
    }
}

/// The console's lines as shown: cleaned (a jsonl record's level and message), without
/// nEVR's noise unless asked, filtered.
fn shown_lines(d: &Dashboard) -> Vec<String> {
    let h = &d.server_host;
    let filter = h.filter.trim().to_lowercase();
    h.console
        .lines
        .iter()
        .filter_map(|raw| {
            let clean = events::clean(raw);
            if !h.noise && events::is_noise(&clean) {
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

#[allow(clippy::too_many_arguments)]
fn doctor_tab(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    x: f32,
    top: f32,
    body: f32,
    w: f32,
    bottom: f32,
) {
    let s = dz(36.0);
    if kit
        .button(
            "sh-doctor-run",
            x + w - s,
            top,
            s,
            s,
            Tone::Dark,
            Some(Icon::Refresh),
            "",
            !d.server_host.checking,
            "Check again",
        )
        .clicked
    {
        run_doctor(d, ctx);
    }
    let Some(checks) = d.server_host.checks.clone() else {
        kit.caps_text(x, body, w, "Checking…", 16.0, design::BODY, 0.0);
        return;
    };
    let mut y = body;
    let mut fix_it = None;
    for (i, c) in checks.iter().enumerate() {
        if y > bottom - dz(40.0) {
            break;
        }
        let color = match c.level {
            Level::Good => design::QUEST_ON,
            Level::Warn => design::QUEST_WARN,
            Level::Bad => design::DANGER,
        };
        kit.mark(c.level == Level::Good, color, dz(22.0), x, y + dz(2.0));
        let name = kit.label_galley(c.name, design::din(18.0), design::TEXT, dz(160.0));
        kit.put(x + dz(36.0), y, name);
        let bw = dz(170.0);
        let tx = x + dz(200.0);
        let tw = w
            - dz(200.0)
            - if c.fix == Fix::None {
                0.0
            } else {
                bw + dz(16.0)
            };
        let th = kit.caps_text(tx, y + dz(2.0), tw, &c.text, 15.0, design::BODY, 0.0);
        let label = match &c.fix {
            Fix::None => None,
            Fix::SignIn => Some("Account"),
            Fix::Game => Some("Go to Play"),
            Fix::Firewall(_) => Some("Let them in"),
            Fix::Defender => Some("Allow"),
        };
        if let Some(l) = label {
            let tip = match &c.fix {
                Fix::Firewall(_) | Fix::Defender => "Asks for administrator rights",
                _ => "",
            };
            if kit
                .button(
                    &format!("sh-fix-{i}"),
                    x + w - bw,
                    y - dz(4.0),
                    bw,
                    BTN_H,
                    Tone::Blue,
                    None,
                    l,
                    true,
                    tip,
                )
                .clicked
            {
                fix_it = Some(c.fix.clone());
            }
        }
        y += th.max(dz(30.0)) + dz(18.0);
    }
    if let Some(f) = fix_it {
        fix(d, ctx, &f);
    }
}

fn account_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context, r: Dr) {
    let (x, mut y, w, bottom) = hero::card_frame(k, r, "Account");
    let a = d.server_host.cfg().account.clone();
    let who = if a.username.is_empty() {
        a.discord_id.clone()
    } else {
        a.username.clone()
    };
    let state = match a.sign_in {
        SignIn::None => "The servers aren't signed in yet: link an EchoVRCE account for them below. Use an account of their own, or the one you play with if it has no password.".to_string(),
        SignIn::Device => format!("The servers sign in as {who} (linked, no password)."),
        SignIn::Password => format!("The servers sign in as {who} with its Discord ID and password."),
    };
    y += k.caps_text(x, y, w, &state, 16.0, design::BODY, 0.0) + dz(14.0);
    if let Some((code, _)) = d.server_host.code.clone() {
        y += k.caps_text(x, y, w, &format!("Approve the code {} on echovrce.com (it opened in your browser), signed in as the servers' account.", code.code), 16.0, design::HEADING, 0.0) + dz(12.0);
        let bw = (w - dz(10.0)) / 2.0;
        if k.button(
            "sh-code-open",
            x,
            y,
            bw,
            BTN_H,
            Tone::Blue,
            None,
            "Open the page",
            true,
            "",
        )
        .clicked
        {
            let _ = open::that(echovrce::approve_url(&code.code));
        }
        if k.button(
            "sh-code-cancel",
            x + bw + dz(10.0),
            y,
            bw,
            BTN_H,
            Tone::Dark,
            None,
            "Cancel",
            true,
            "",
        )
        .clicked
        {
            d.server_host.code = None;
            d.server_host.busy = false;
        }
        y += BTN_H + dz(14.0);
    } else if a.sign_in == SignIn::None {
        let mine = d.vrce.session().is_some();
        let busy = d.server_host.busy;
        if k.button(
            "sh-link-mine",
            x,
            y,
            w,
            BTN_H,
            Tone::Go,
            None,
            "Use my EchoVRCE account",
            mine && !busy,
            if mine {
                "Links the servers to the account you're signed in with on the EchoVRCE page"
            } else {
                "Sign in on the EchoVRCE page first"
            },
        )
        .clicked
        {
            d.server_host.busy = true;
            d.server_host.account_note = None;
            d.server_host.worker.spawn(ctx, |tx| {
                tx.send(Msg::Linked(
                    account::link_launchers().map_err(|e| format!("{e:#}")),
                ));
            });
        }
        y += BTN_H + dz(10.0);
        let bw = (w - dz(10.0)) / 2.0;
        if k.button(
            "sh-link-code",
            x,
            y,
            bw,
            BTN_H,
            Tone::Dark,
            None,
            "Link another",
            !busy,
            "Approve a code on echovrce.com, signed in as another account",
        )
        .clicked
        {
            d.server_host.busy = true;
            d.server_host.account_note = None;
            d.server_host.worker.spawn(ctx, |tx| {
                tx.send(Msg::Code(
                    echovrce::request_code().map_err(|e| format!("{e:#}")),
                ));
            });
        }
        if k.button(
            "sh-password",
            x + bw + dz(10.0),
            y,
            bw,
            BTN_H,
            Tone::Dark,
            None,
            "Password",
            !busy,
            "Sign in with the account's Discord ID and password",
        )
        .clicked
        {
            d.server_host.card = Some(Card::Password {
                discord_id: String::new(),
                password: String::new(),
            });
        }
        y += BTN_H + dz(14.0);
    } else {
        let bw = (w - dz(20.0)) / 3.0;
        let busy = d.server_host.busy;
        if k.button(
            "sh-test",
            x,
            y,
            bw,
            BTN_H,
            Tone::Dark,
            None,
            "Test",
            !busy,
            "Sign in with EchoVRCE now, and say which guilds it may host for",
        )
        .clicked
        {
            d.server_host.busy = true;
            let c = d.server_host.cfg().clone();
            d.server_host.worker.spawn(ctx, move |tx| {
                let r = account::session(&c).map(|t| {
                    if c.account.sign_in == SignIn::Password {
                        guild_note(&account::host_guilds(&t.token))
                    } else {
                        "Signed in (a linked session: the guilds it may host for show on EchoVRCE)."
                            .into()
                    }
                });
                tx.send(Msg::Tested(r.map_err(|e| format!("{e:#}"))));
            });
        }
        if k.button(
            "sh-registered",
            x + bw + dz(10.0),
            y,
            bw,
            BTN_H,
            Tone::Dark,
            None,
            "On EchoVRCE",
            !busy,
            "The servers EchoVRCE has registered for this account",
        )
        .clicked
        {
            d.server_host.busy = true;
            d.server_host.registered = None;
            d.server_host.card = Some(Card::Registered);
            let c = d.server_host.cfg().clone();
            d.server_host.worker.spawn(ctx, move |tx| {
                tx.send(Msg::Registered(
                    account::registered(&c).map_err(|e| format!("{e:#}")),
                ));
            });
        }
        if k.button(
            "sh-signout",
            x + 2.0 * (bw + dz(10.0)),
            y,
            bw,
            BTN_H,
            Tone::Danger,
            None,
            "Sign out",
            !busy,
            "The servers forget the account",
        )
        .clicked
        {
            d.dialogs.confirm_danger(
                SIGN_OUT_KEY,
                "Sign the servers out?",
                "The servers forget their EchoVRCE account: they can't host until one is linked again. Running servers keep going until they start again.",
                "Sign out",
            );
        }
        y += BTN_H + dz(14.0);
    }
    if let Some((note, good)) = d.server_host.account_note.clone() {
        y += k.caps_text(
            x,
            y,
            w,
            &note,
            15.0,
            if good {
                design::QUEST_ON
            } else {
                design::QUEST_WARN
            },
            0.0,
        ) + dz(12.0);
    }
    if y < bottom - dz(60.0) {
        k.caps_text(x, y, w, "A linked account must have no password on EchoVRCE, or EchoVRCE refuses the servers' sign-in. To remove it: /reset-password in the Echo VR Lounge Discord.", 14.0, design::GREY, 0.0);
    }
}

fn background_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context, r: Dr) {
    let (x, mut y, w, _) = hero::card_frame(k, r, "In the background");
    let text = match d.server_host.running {
        Some(true) => {
            "The server host is running: the servers keep going with the launcher closed."
        }
        Some(false) => "The server host isn't running: it starts with the first server you start.",
        None => "Looking…",
    };
    y += k.caps_text(x, y, w, text, 15.0, design::BODY, 0.0) + dz(14.0);
    let mut auto = d.server_host.autostart.unwrap_or(false);
    if k.check(
        "sh-autostart",
        &mut auto,
        "Start the servers with this PC",
        x,
        y,
        true,
        "At sign-in, the server host starts the servers set to run",
    ) {
        match autostart::set(auto) {
            Ok(()) => d.server_host.autostart = Some(auto),
            Err(e) => d
                .dialogs
                .error("Server host", &format!("{e:#}"), Default::default()),
        }
    }
    y += dz(40.0);
    let mut desktop = d.server_host.cfg().alerts.desktop;
    if k.check(
        "sh-desktop-alerts",
        &mut desktop,
        "Desktop alerts",
        x,
        y,
        true,
        "When a server stops on an error, keeps restarting, is down or is back",
    ) {
        d.server_host.cfg().alerts.desktop = desktop;
        save(d);
    }
    y += dz(44.0);
    k.caption(x, y, "Discord webhook (optional)");
    y += dz(26.0);
    let bw = dz(150.0);
    let bad = !d.server_host.webhook.trim().is_empty()
        && !host::notify::is_webhook(&d.server_host.webhook);
    k.secret_field(
        "sh-webhook",
        &mut d.server_host.webhook,
        x,
        y,
        w - bw - dz(10.0),
        BTN_H,
        "https://discord.com/api/webhooks/…",
        bad,
        "The alerts go to this Discord channel too",
    );
    let saved = d.server_host.cfg().alerts.webhook.clone();
    let changed = d.server_host.webhook.trim() != saved;
    if k.button(
        "sh-webhook-save",
        x + w - bw,
        y,
        bw,
        BTN_H,
        Tone::Blue,
        None,
        "Save",
        changed && !bad,
        "Saves it and sends a test message",
    )
    .clicked
    {
        let url = d.server_host.webhook.trim().to_string();
        d.server_host.cfg().alerts.webhook = url.clone();
        save(d);
        if url.is_empty() {
            d.server_host.webhook_note = Some(("No webhook: alerts stay on this PC.".into(), true));
        } else {
            d.server_host.worker.spawn(ctx, move |tx| {
                let a = host::notify::test_alert();
                let r = host::notify::post(
                    &url,
                    &host::notify::webhook_body(&a, &host::notify::host_name()),
                );
                tx.send(Msg::Webhook(r.map_err(|e| format!("{e:#}"))));
            });
        }
    }
    y += BTN_H + dz(12.0);
    if let Some((note, good)) = d.server_host.webhook_note.clone() {
        k.caps_text(
            x,
            y,
            w,
            &note,
            14.0,
            if good {
                design::QUEST_ON
            } else {
                design::QUEST_WARN
            },
            0.0,
        );
    }
}

// ---- the cards over the page ----

/// A card over the page (setup's overlay).
fn card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    match d.server_host.card.clone() {
        Some(Card::Edit(f)) => edit_card(d, k, *f),
        Some(Card::Settings(f)) => settings_card(d, k, *f),
        Some(Card::Password {
            discord_id,
            password,
        }) => password_card(d, k, ctx, discord_id, password),
        Some(Card::Registered) => registered_card(d, k),
        None => {}
    }
    if k.key(egui::Key::Escape) {
        d.server_host.card = None;
    }
}

/// A labelled field in a card; true when it lost focus.
#[allow(clippy::too_many_arguments)]
fn labelled(
    k: &mut Kit,
    key: &str,
    label: &str,
    text: &mut String,
    x: f32,
    y: f32,
    w: f32,
    tip: &str,
) -> bool {
    k.caption(x, y, label);
    k.field(key, text, x, y + dz(24.0), w, BTN_H, "", false, tip)
}

fn edit_card(d: &mut Dashboard, k: &mut Kit, mut f: EditForm) {
    let (x, y, w, bottom) = setup::card(k, dz(1000.0), dz(560.0), &format!("Server {}", f.name));
    let col = (w - dz(30.0)) / 2.0;
    let x2 = x + col + dz(30.0);
    let step = dz(86.0);
    labelled(
        k,
        "sh-e-port",
        "Port (UDP and TCP)",
        &mut f.port,
        x,
        y,
        col,
        "Forward it to this PC with the same number outside",
    );
    labelled(
        k,
        "sh-e-regions",
        "Regions (comma-separated)",
        &mut f.regions,
        x2,
        y,
        col,
        "Where it registers, e.g. uwu, euw. Empty: EchoVRCE's default",
    );
    labelled(
        k,
        "sh-e-guilds",
        "Guilds (ids, comma-separated)",
        &mut f.guilds,
        x,
        y + step,
        col,
        "The guilds it hosts for. Each has to give the account its server host role. Empty: any",
    );
    labelled(
        k,
        "sh-e-tick",
        "Tick rate (frames per second)",
        &mut f.tick_rate,
        x2,
        y + step,
        col,
        "120 is EchoVRCE's",
    );
    labelled(
        k,
        "sh-e-ttl",
        "Empty server stays (seconds)",
        &mut f.empty_ttl,
        x,
        y + 2.0 * step,
        col,
        "How long an empty server keeps its match before going back to the lobby. 0: nEVR's own",
    );
    labelled(
        k,
        "sh-e-args",
        "More arguments for the game",
        &mut f.extra_args,
        x2,
        y + 2.0 * step,
        col,
        "Space-separated, added to the server's command line",
    );
    let mut mods = f.mods;
    if k.check(
        "sh-e-mods",
        &mut mods,
        "Load the mods the Mods page has on",
        x,
        y + 3.0 * step + dz(10.0),
        true,
        "A content pack such as EchoCombat too",
    ) {
        f.mods = mods;
    }
    let mut ty = y + 3.0 * step + dz(60.0);
    if let Some(e) = &f.error {
        ty += k.caps_text(x, ty, w, e, 15.0, design::QUEST_WARN, 0.0) + dz(10.0);
    }
    let _ = ty;
    let in_match = d
        .server_host
        .statuses
        .get(&f.name)
        .is_some_and(|s| s.state == State::InSession && s.fresh(now()));
    k.caps_text(
        x,
        bottom - BTN_H - dz(44.0),
        w,
        if in_match {
            "It is hosting a match: the changes apply once that is over."
        } else {
            "A running server starts again with the changes."
        },
        14.0,
        design::GREY,
        0.0,
    );
    let bw = dz(160.0);
    if k.button(
        "sh-e-cancel",
        x + w - 2.0 * bw - dz(10.0),
        bottom - BTN_H,
        bw,
        BTN_H,
        Tone::Dark,
        None,
        "Cancel",
        true,
        "",
    )
    .clicked
    {
        d.server_host.card = None;
        return;
    }
    if k.button(
        "sh-e-save",
        x + w - bw,
        bottom - BTN_H,
        bw,
        BTN_H,
        Tone::Go,
        None,
        "Save",
        true,
        "",
    )
    .clicked
    {
        match apply_edit(d, &f) {
            Ok(()) => {
                d.server_host.card = None;
                let _ = status::ask(&host::server_dir(&f.name), Command::Apply);
                d.notify("Saved");
                return;
            }
            Err(e) => f.error = Some(e),
        }
    }
    d.server_host.card = Some(Card::Edit(Box::new(f)));
}

fn apply_edit(d: &mut Dashboard, f: &EditForm) -> Result<(), String> {
    let num = |t: &str, what: &str| {
        t.trim()
            .parse::<u32>()
            .map_err(|_| format!("{what}: a number, please."))
    };
    let port = num(&f.port, "Port")?;
    let Ok(port) = u16::try_from(port) else {
        return Err("Port: up to 65535.".into());
    };
    let c = d.server_host.cfg();
    let Some(old) = c.server(&f.name).cloned() else {
        return Err("This server is gone.".into());
    };
    let s = Server {
        port,
        regions: f.regions.trim().to_string(),
        guilds: f.guilds.trim().to_string(),
        tick_rate: num(&f.tick_rate, "Tick rate")?,
        empty_ttl: num(&f.empty_ttl, "Empty server")?,
        mods: f.mods,
        extra_args: f.extra_args.trim().to_string(),
        ..old
    };
    if let Some(why) = host::check_server(&s, &c.servers) {
        return Err(why);
    }
    if let Some(slot) = c.server_mut(&f.name) {
        *slot = s;
    }
    c.save().map_err(|e| format!("{e:#}"))
}

fn settings_card(d: &mut Dashboard, k: &mut Kit, mut f: SettingsForm) {
    let (x, y, w, bottom) = setup::card(k, dz(1100.0), dz(820.0), "Server host settings");
    let col = (w - dz(30.0)) / 2.0;
    let x2 = x + col + dz(30.0);
    let step = dz(86.0);
    // The game version: the live builds installed.
    k.caption(x, y, "Game version the servers run");
    let lives: Vec<(String, String)> = d
        .state
        .versions
        .iter()
        .filter(|v| v.publisher_lock.is_none() && v.present())
        .map(|v| (v.id.clone(), v.name.clone()))
        .collect();
    let mut bx = x;
    let options: Vec<(String, String)> =
        std::iter::once((String::new(), "The selected one".to_string()))
            .chain(lives)
            .collect();
    for (id, name) in &options {
        let bw = k.button_width(name, None, dz(34.0)).max(dz(120.0));
        if bx + bw > x + w {
            break;
        }
        let on = f.version == *id;
        if k.button(
            &format!("sh-s-v-{id}"),
            bx,
            y + dz(24.0),
            bw,
            dz(34.0),
            if on { Tone::Blue } else { Tone::Dark },
            None,
            name,
            true,
            "",
        )
        .clicked
        {
            f.version = id.clone();
        }
        bx += bw + dz(8.0);
    }
    let y = y + step;
    k.caption(x, y, "New servers start with");
    let y = y + dz(30.0);
    labelled(
        k,
        "sh-s-regions",
        "Regions",
        &mut f.regions,
        x,
        y,
        col / 2.0 - dz(10.0),
        "",
    );
    labelled(
        k,
        "sh-s-tick",
        "Tick rate",
        &mut f.tick_rate,
        x + col / 2.0,
        y,
        col / 2.0,
        "",
    );
    labelled(k, "sh-s-guilds", "Guilds", &mut f.guilds, x2, y, col, "");
    let y = y + step + dz(10.0);
    k.caption(
        x,
        y,
        "Services: EchoVRCE's, unless the servers play on another Nakama",
    );
    let y = y + dz(30.0);
    labelled(
        k,
        "sh-s-socket",
        "Login (services.socket_uri)",
        &mut f.socket_uri,
        x,
        y,
        col,
        "",
    );
    labelled(
        k,
        "sh-s-serverdb",
        "Registration (services.serverdb_uri)",
        &mut f.serverdb_uri,
        x2,
        y,
        col,
        "Empty: through the login",
    );
    labelled(
        k,
        "sh-s-http",
        "HTTP (auth.http_uri)",
        &mut f.http_uri,
        x,
        y + step,
        col,
        "",
    );
    labelled(
        k,
        "sh-s-httpkey",
        "HTTP key (auth.http_key)",
        &mut f.http_key,
        x2,
        y + step,
        col,
        "EchoVRCE's is public",
    );
    labelled(
        k,
        "sh-s-serverkey",
        "Server key (auth.server_key)",
        &mut f.server_key,
        x,
        y + 2.0 * step,
        col,
        "EchoVRCE's is public",
    );
    labelled(
        k,
        "sh-s-lock",
        "Publisher lock",
        &mut f.publisher_lock,
        x2,
        y + 2.0 * step,
        col,
        "echovrce for EchoVRCE",
    );
    labelled(
        k,
        "sh-s-internal",
        "Bind address (optional)",
        &mut f.internal_ip,
        x,
        y + 3.0 * step,
        col,
        "network.internal_ip: the address on this PC the servers use. Empty: nEVR's choice",
    );
    labelled(
        k,
        "sh-s-external",
        "Public address (optional)",
        &mut f.external_ip,
        x2,
        y + 3.0 * step,
        col,
        "network.external_ip: the address EchoVRCE gives players. Empty: the one it sees the servers come from",
    );
    if let Some(e) = &f.error {
        k.caps_text(x, y + 4.0 * step, w, e, 15.0, design::QUEST_WARN, 0.0);
    }
    let bw = dz(180.0);
    if k.button(
        "sh-s-reset",
        x,
        bottom - BTN_H,
        dz(260.0),
        BTN_H,
        Tone::Dark,
        None,
        "EchoVRCE's services",
        true,
        "Puts EchoVRCE's addresses and keys back",
    )
    .clicked
    {
        let s = host::Services::default();
        f.socket_uri = s.socket_uri;
        f.serverdb_uri = s.serverdb_uri;
        f.http_uri = s.http_uri;
        f.http_key = s.http_key;
        f.server_key = s.server_key;
        f.publisher_lock = "echovrce".into();
    }
    if k.button(
        "sh-s-cancel",
        x + w - 2.0 * bw - dz(10.0),
        bottom - BTN_H,
        bw,
        BTN_H,
        Tone::Dark,
        None,
        "Cancel",
        true,
        "",
    )
    .clicked
    {
        d.server_host.card = None;
        return;
    }
    if k.button(
        "sh-s-save",
        x + w - bw,
        bottom - BTN_H,
        bw,
        BTN_H,
        Tone::Go,
        None,
        "Save",
        true,
        "",
    )
    .clicked
    {
        let mut c = d.server_host.cfg().clone();
        c.version = f.version.clone();
        c.defaults.regions = f.regions.trim().into();
        c.defaults.guilds = f.guilds.trim().into();
        c.defaults.tick_rate = f.tick_rate.trim().parse().unwrap_or(host::TICK_RATE);
        c.services = host::Services {
            socket_uri: f.socket_uri.trim().into(),
            serverdb_uri: f.serverdb_uri.trim().into(),
            http_uri: f.http_uri.trim().into(),
            http_key: f.http_key.trim().into(),
            server_key: f.server_key.trim().into(),
        };
        c.publisher_lock = f.publisher_lock.trim().into();
        c.internal_ip = f.internal_ip.trim().into();
        c.external_ip = f.external_ip.trim().into();
        let problem = host::config::check_services(&c).or_else(|| {
            (!(10..=1000).contains(&c.defaults.tick_rate))
                .then(|| "Tick rate: 10 to 1000.".to_string())
        });
        match problem {
            Some(why) => f.error = Some(why),
            None => {
                *d.server_host.cfg() = c;
                save(d);
                restart_all_idle(d);
                d.server_host.card = None;
                d.notify("Saved: the servers use it at their next start");
                return;
            }
        }
    }
    d.server_host.card = Some(Card::Settings(Box::new(f)));
}

fn password_card(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    mut id: String,
    mut pw: String,
) {
    let (x, y, w, bottom) = setup::card(k, dz(800.0), dz(520.0), "The servers' account");
    let mut ty = y + k.caps_text(x, y, w, "The Discord ID and password the servers sign in with. The password reaches the game only while it starts, never a file.", 16.0, design::BODY, 0.0) + dz(20.0);
    labelled(k, "sh-p-id", "Discord ID", &mut id, x, ty, w, "Discord: Settings, Advanced, Developer Mode on; then right-click the account's name, Copy User ID");
    ty += dz(86.0);
    k.caption(x, ty, "Password");
    k.secret_field(
        "sh-p-pw",
        &mut pw,
        x,
        ty + dz(24.0),
        w,
        BTN_H,
        "",
        false,
        "The account's EchoVRCE password",
    );
    if let Some((note, false)) = d.server_host.account_note.clone() {
        k.caps_text(x, ty + dz(80.0), w, &note, 15.0, design::QUEST_WARN, 0.0);
    }
    let bw = dz(160.0);
    if k.button(
        "sh-p-cancel",
        x + w - 2.0 * bw - dz(10.0),
        bottom - BTN_H,
        bw,
        BTN_H,
        Tone::Dark,
        None,
        "Cancel",
        true,
        "",
    )
    .clicked
    {
        d.server_host.card = None;
        return;
    }
    let busy = d.server_host.busy;
    if k.button(
        "sh-p-ok",
        x + w - bw,
        bottom - BTN_H,
        bw,
        BTN_H,
        Tone::Go,
        None,
        if busy { "Checking…" } else { "Sign in" },
        !busy && !id.trim().is_empty() && !pw.is_empty(),
        "Checks them with EchoVRCE",
    )
    .clicked
    {
        d.server_host.busy = true;
        d.server_host.account_note = None;
        let (i, p) = (id.clone(), pw.clone());
        let services = d.server_host.cfg().services.clone();
        d.server_host.worker.spawn(ctx, move |tx| {
            let r = account::check_password(&services, &i, &p).map_err(|e| format!("{e:#}"));
            tx.send(Msg::Password(i, p, r));
        });
    }
    if d.server_host.card.is_some() {
        d.server_host.card = Some(Card::Password {
            discord_id: id,
            password: pw,
        });
    }
}

fn registered_card(d: &mut Dashboard, k: &mut Kit) {
    let (x, y, w, bottom) = setup::card(k, dz(900.0), dz(640.0), "On EchoVRCE");
    let mut ty = y;
    match &d.server_host.registered {
        None => {
            k.caps_text(x, ty, w, "Asking EchoVRCE…", 16.0, design::BODY, 0.0);
        }
        Some(Err(e)) => {
            k.caps_text(x, ty, w, e, 16.0, design::QUEST_WARN, 0.0);
        }
        Some(Ok(list)) if list.is_empty() => {
            k.caps_text(
                x,
                ty,
                w,
                "EchoVRCE has no server of this account registered right now.",
                16.0,
                design::BODY,
                0.0,
            );
        }
        Some(Ok(list)) => {
            ty += k.caps_text(x, ty, w, &format!("{} server(s) of this account are registered (from any PC). Where they are is EchoVRCE's guess, not the regions asked for.", list.len()), 15.0, design::GREY, 0.0) + dz(14.0);
            for r in list.iter().take(12) {
                let mode = if r.mode.is_empty() {
                    "Waiting for a match".to_string()
                } else {
                    format!("{}, {} player(s)", r.mode, r.players)
                };
                let place = [r.region.as_str(), r.country.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ");
                let line = format!(
                    "{mode}  ·  {place}{}",
                    if r.guilds.is_empty() {
                        String::new()
                    } else {
                        format!("  ·  {} guild(s)", r.guilds.len())
                    }
                );
                let g = k.label_galley(&line, design::din(15.0), design::TEXT, w);
                k.put(x, ty, g);
                ty += dz(36.0);
            }
        }
    }
    let bw = dz(160.0);
    if k.button(
        "sh-r-close",
        x + w - bw,
        bottom - BTN_H,
        bw,
        BTN_H,
        Tone::Dark,
        None,
        "Close",
        true,
        "",
    )
    .clicked
    {
        d.server_host.card = None;
    }
}

// ---- snapshots ----

/// Snapshots: the plugin installed (its tab second) with made-up servers, or not.
pub(super) fn snap(d: &mut Dashboard, v: Option<super::SnapVariant>) {
    use super::SnapVariant as V;
    d.plugins.list.retain(|p| p.page.id != host::PLUGIN_ID);
    let tab = match v {
        Some(V::ServerHost | V::ServerHostEdit | V::ServerHostSettings) => Tab::Servers,
        Some(V::ServerHostConsole) => Tab::Console,
        Some(V::ServerHostDoctor) => Tab::Doctor,
        _ => return,
    };
    super::plugins::snap_install(
        d,
        include_str!("../../../docs/plugins/server-host/plugin.json"),
    );
    let t = now();
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
    let st = |name: &str, state: State, matches: u32, restarts: u32| Status {
        name: name.into(),
        state,
        since: t - 47 * 60,
        updated: t,
        port: 0,
        public_ip: "203.0.113.7".into(),
        runtime: "5.2.1+0.3caed2ef".into(),
        matches,
        restarts,
        ..Status::default()
    };
    let mut statuses = HashMap::new();
    for (i, (name, state, m, r)) in [
        ("echo-1", State::InSession, 6, 6),
        ("echo-2", State::Registered, 3, 3),
        ("echo-3", State::Backoff, 0, 2),
    ]
    .into_iter()
    .enumerate()
    {
        let mut s = st(name, state, m, r);
        s.port = host::FIRST_PORT + i as u16;
        if state == State::Backoff {
            s.reason = Some(events::Reason::UdpUnreachable);
            s.last_error =
                "[NEVR.FATAL] registration rejected by ServerDB code=10 (broadcaster unreachable)"
                    .into();
            s.next_start = t + 37;
        }
        statuses.insert(name.to_string(), s);
    }
    let console: VecDeque<String> = [
        "[host] starting echo-1 on port 6792 (HTTP API 7792)",
        r#"{"ts":"2026-10-23T15:45:44.072Z","level":"info","msg":"[NEVR.PATCH] NEVR-BUILD 5.2.1 3caed2ef1064f4f55b734bfec1a0047d865464d9"}"#,
        r#"{"ts":"2026-10-23T15:45:51.648Z","level":"info","msg":"[NEVR.WS] login injected xpid=OVR-ORG-123456789012345678 platform=4 conn=1 (login)"}"#,
        r#"{"ts":"2026-10-23T15:45:52.321Z","level":"info","msg":"[NEVR.WS] LOGIN SUCCESS"}"#,
        r#"{"ts":"2026-10-23T15:45:52.519Z","level":"info","msg":"[EVR] Beginning multiplayer"}"#,
        r#"{"ts":"2026-10-23T15:45:52.655Z","level":"info","msg":"[EVR] Dedicated: broadcaster initialized at [192.168.1.20:6792]"}"#,
        r#"{"ts":"2026-10-23T15:45:55.622Z","level":"info","msg":"[NEVR.SERVERDB] Connecting to ServerDB at ws://g.echovrce.com:80/nevr"}"#,
        r#"{"ts":"2026-10-23T15:45:56.104Z","level":"info","msg":"[EVR] Dedicated: registered as 0xE4A74026793A4079 at [192.168.1.20/203.0.113.7:6792]"}"#,
        "[host] READY: registered as 203.0.113.7:6792",
        r#"{"ts":"2026-10-23T15:52:10.880Z","level":"info","msg":"[NEVR.GAMESERVER] session starting session_id=7a1c-44 mode=echo_arena"}"#,
        r#"{"ts":"2026-10-23T15:52:11.002Z","level":"warn","msg":"[NEVR.PATCH] slow frame 41 ms (tick 120)"}"#,
    ]
    .iter()
    .map(|l| l.to_string())
    .collect();
    let checks = vec![
        Check { name: "Game", level: Level::Good, text: "Live build with nEVR 5.2.1+0.3caed2ef.".into(), fix: Fix::None },
        Check { name: "Account", level: Level::Good, text: "Signs in as mias-servers (linked).".into(), fix: Fix::None },
        Check { name: "Disk", level: Level::Good, text: "212 GB free.".into(), fix: Fix::None },
        Check { name: "Ports", level: Level::Bad, text: "In use by another program: 6795 (echo-4). Give the server another port.".into(), fix: Fix::None },
        Check { name: "Address", level: Level::Good, text: "This PC is 203.0.113.7 on the internet. Forward each server's port (UDP and TCP) to this PC with the same number outside (6792, 6793, 6794, 6795): EchoVRCE checks it when a server registers.".into(), fix: Fix::None },
        Check { name: "Firewall", level: Level::Warn, text: "Windows Firewall may block port(s) 6795.".into(), fix: Fix::Firewall(vec![6792, 6793, 6794, 6795]) },
    ];
    let card = match v {
        Some(V::ServerHostEdit) => Some(Card::Edit(Box::new(edit_form(&cfg.servers[1])))),
        Some(V::ServerHostSettings) => Some(Card::Settings(Box::new(settings_form(&cfg)))),
        _ => None,
    };
    d.server_host = HostUi {
        cfg: Some(cfg),
        statuses,
        running: Some(true),
        tab,
        card,
        add_n: "1".into(),
        console: ConsoleView {
            server: "echo-1".into(),
            tail: None,
            lines: console,
        },
        follow: true,
        checks: Some(checks),
        autostart: Some(true),
        account_note: Some((
            "Signed in (a linked session: the guilds it may host for show on EchoVRCE).".into(),
            true,
        )),
        ..HostUi::default()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_long() {
        assert_eq!(span(59), "0 min");
        assert_eq!(span(3 * 60), "3 min");
        assert_eq!(span(125 * 60), "2 h 5 min");
    }
}
