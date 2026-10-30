//! Server host: Echo VR game servers on this PC. The servers run in the launcher's server
//! host process (`EchoVR_Launcher --server-host`); this page reads what they do and asks
//! the launcher's `server-host` service for everything else. It never sees a password, a
//! sign-in or the webhook: those stay in the launcher (its secret fields and services).
use std::collections::BTreeMap;

use echovr_plugin::*;

const POLL: u32 = 1;
const RUNNING: u32 = 2;

/// EchoVRCE's login and registration for nEVR 6 servers (what nEVR has built in). Without
/// the port: launchers up to 0.13.3 turn `wss://g.echovrce.com:443/nevr` back into the old
/// login when they read the settings.
const ECHOVRCE_NEVR: &str = "wss://g.echovrce.com/nevr";
/// What launchers up to 0.13.3 keep as EchoVRCE's: it refuses a nEVR 6 server's login
/// (400), and the server never registers.
const OLD_LOGIN: &str = "ws://g.echovrce.com:80/spr";
const OLD_REGISTRATION: &str = "ws://g.echovrce.com:80/nevr";
const NEVR_443: &str = "wss://g.echovrce.com:443/nevr";

#[derive(Default, PartialEq, Clone, Copy)]
enum Tab {
    #[default]
    Servers,
    Console,
    Doctor,
}

/// A card over the servers: a server's settings, the host's settings, or what EchoVRCE
/// has registered.
#[derive(Clone)]
enum Over {
    Edit(String),
    Settings,
    Registered,
}

/// What a ticket's answer is for.
enum Wait {
    Running,
    Account,
    Code,
    Doctor,
    Fix,
    Registered,
    Webhook,
}

#[derive(Default)]
struct S {
    st: Value,
    error: String,
    running: Option<bool>,
    tab: Tab,
    over: Option<Over>,
    /// The fields of the card over the servers, as typed.
    form: BTreeMap<String, String>,
    form_error: String,
    add_n: String,
    // Console.
    console: String,
    logs: BTreeMap<String, u32>,
    follow: bool,
    noise: bool,
    filter: String,
    // Doctor.
    checks: Option<Value>,
    checking: bool,
    // Account.
    busy: bool,
    code: Option<(String, String)>,
    note: Option<(String, bool)>,
    password: bool,
    discord_id: String,
    registered: Option<Result<Value, String>>,
    // In the background.
    webhook_note: Option<(String, bool)>,
    waits: BTreeMap<u32, Wait>,
    shown: bool,
}

static STATE: State<S> = State::new();

fn refresh(s: &mut S) {
    match call("state", &json!({})) {
        Ok(v) => {
            s.st = v;
            s.error.clear();
        }
        Err(e) => s.error = e,
    }
    let names = servers(s);
    if s.console.is_empty() || !names.contains(&s.console) {
        s.console = names.first().cloned().unwrap_or_default();
    }
}

/// Pure: the settings that move the servers off the old EchoVRCE addresses (`login`,
/// `registration` as saved), if they are on them. Other services stay as they are.
fn nevr_address_change(login: &str, registration: &str) -> Option<Value> {
    if login.trim() != OLD_LOGIN {
        return None;
    }
    let mut args = json!({"socket_uri": ECHOVRCE_NEVR});
    if matches!(registration.trim(), "" | OLD_REGISTRATION | NEVR_443) {
        args["serverdb_uri"] = json!(ECHOVRCE_NEVR);
    }
    Some(args)
}

/// Once the page loads: servers on the old EchoVRCE addresses get the one nEVR 6 signs in
/// and registers at (idle servers start again with it).
fn to_nevr_address(s: &mut S) {
    let change = nevr_address_change(
        &text_at(&s.st, "services.socket_uri"),
        &text_at(&s.st, "services.serverdb_uri"),
    );
    if let Some(args) = change {
        if call("settings", &args).is_ok() {
            notify("The servers sign in and register at EchoVRCE's current address now");
            refresh(s);
        }
    }
}

/// Pure: `v` as the settings keep it: EchoVRCE's nEVR address without its port.
fn kept_address(v: &str) -> String {
    if v.trim() == NEVR_443 {
        ECHOVRCE_NEVR.to_string()
    } else {
        v.to_string()
    }
}

fn ask_running(s: &mut S) {
    if let Ok(t) = start("running", &json!({})) {
        s.waits.insert(t, Wait::Running);
    }
}

fn servers(s: &S) -> Vec<String> {
    s.st["servers"]
        .as_array()
        .map(|a| a.iter().map(|v| text_at(v, "name")).collect())
        .unwrap_or_default()
}

fn server<'a>(s: &'a S, name: &str) -> Option<&'a Value> {
    s.st["servers"]
        .as_array()?
        .iter()
        .find(|v| text_at(v, "name") == name)
}

fn signed_in(s: &S) -> bool {
    text_at(&s.st, "account.sign_in") != "none" && !text_at(&s.st, "account.sign_in").is_empty()
}

/// No account set, and none needed: the servers' nEVR (6.9.0 on) signs each in by a code
/// it shows the first time it starts.
fn by_code(s: &S) -> bool {
    !signed_in(s) && s.st["account"]["by_code"] == true
}

fn tone(t: &str) -> Tone {
    match t {
        "good" => Tone::Good,
        "warn" => Tone::Warn,
        "bad" => Tone::Bad,
        "muted" => Tone::Muted,
        _ => Tone::Normal,
    }
}

/// Runs a call; a failure is said.
fn act(s: &mut S, op: &str, args: Value) -> bool {
    match call(op, &args) {
        Ok(_) => {
            refresh(s);
            true
        }
        Err(e) => {
            host::ask("error", "Server host", &e, &["OK".into()], true);
            false
        }
    }
}

fn run_doctor(s: &mut S) {
    if s.checking {
        return;
    }
    s.checking = true;
    if let Ok(t) = start("doctor", &json!({})) {
        s.waits.insert(t, Wait::Doctor);
    }
}

fn open_console(s: &mut S, name: &str) {
    s.console = name.to_string();
    s.follow = true;
    s.tab = Tab::Console;
}

fn log_of(s: &mut S, name: &str) -> Option<u32> {
    if let Some(l) = s.logs.get(name) {
        return Some(*l);
    }
    let l = server_host::console(name).ok()?;
    s.logs.insert(name.to_string(), l);
    Some(l)
}

fn edit_form(v: &Value) -> BTreeMap<String, String> {
    [
        "port",
        "regions",
        "guilds",
        "tick_rate",
        "empty_ttl",
        "extra_args",
    ]
    .iter()
    .map(|k| (k.to_string(), text_at(v, k)))
    .chain([("mods".to_string(), text_at(v, "mods"))])
    .collect()
}

fn settings_form(st: &Value) -> BTreeMap<String, String> {
    let mut f = BTreeMap::new();
    f.insert("version".into(), text_at(st, "version"));
    for k in ["regions", "guilds", "tick_rate"] {
        f.insert(k.into(), text_at(st, &format!("defaults.{k}")));
    }
    for k in [
        "socket_uri",
        "serverdb_uri",
        "http_uri",
        "http_key",
        "server_key",
    ] {
        f.insert(k.into(), text_at(st, &format!("services.{k}")));
    }
    for k in ["publisher_lock", "internal_ip", "external_ip"] {
        f.insert(k.into(), text_at(st, k));
    }
    f
}

struct ServerHost;

impl Plugin for ServerHost {
    fn init(_info: &Value) -> Result<(), String> {
        STATE.with(|s| {
            s.follow = true;
            s.add_n = "1".into();
            refresh(s);
            to_nevr_address(s);
            ask_running(s);
        });
        Ok(())
    }

    fn render(ui: &mut Ui) {
        STATE.with(|s| {
            match s.over.clone() {
                Some(Over::Edit(name)) => edit_card(ui, s, &name),
                Some(Over::Settings) => settings_card(ui, s),
                Some(Over::Registered) => registered_card(ui, s),
                None => main_card(ui, s),
            }
            account_card(ui, s);
            background_card(ui, s);
        })
    }

    fn on_event(e: Event) {
        STATE.with(|s| event(s, e));
        host::invalidate();
    }

    fn on_timer(id: u32) {
        STATE.with(|s| match id {
            POLL => refresh(s),
            RUNNING => ask_running(s),
            _ => {}
        });
        host::invalidate();
    }
}

fn event(s: &mut S, e: Event) {
    match e {
        Event::Shown => {
            s.shown = true;
            refresh(s);
            ask_running(s);
            host::set_timer(POLL, 1000, true);
            host::set_timer(RUNNING, 3000, true);
        }
        Event::Hidden => {
            s.shown = false;
            host::clear_timer(POLL);
            host::clear_timer(RUNNING);
        }
        Event::Tab((_, i)) => {
            s.tab = match i {
                1 => Tab::Console,
                2 => Tab::Doctor,
                _ => Tab::Servers,
            };
            if s.tab == Tab::Doctor && s.checks.is_none() {
                run_doctor(s);
            }
        }
        Event::Changed((id, v)) => changed(s, &id, v),
        Event::Toggled((id, on)) => match id.as_str() {
            "follow" => s.follow = on,
            "noise" => s.noise = on,
            "mods" => {
                s.form.insert("mods".into(), on.to_string());
            }
            "autostart" => {
                act(s, "autostart", json!({"on": on}));
            }
            "desktop" => {
                act(s, "alerts", json!({"desktop": on}));
            }
            _ => {}
        },
        Event::Clicked(id) => clicked(s, &id),
        Event::Dialog((id, i)) => dialog(s, &id, i),
        Event::ServiceDone((t, r)) => done(s, t, r),
        _ => {}
    }
}

fn changed(s: &mut S, id: &str, v: String) {
    match id {
        "add_n" => s.add_n = v,
        "filter" => s.filter = v,
        "console" => {
            s.console = v;
            s.follow = true;
        }
        "discord_id" => s.discord_id = v,
        // A secret field: the launcher keeps what was typed.
        "password" | "webhook" => {}
        k => {
            s.form.insert(k.to_string(), v);
        }
    }
}

fn clicked(s: &mut S, id: &str) {
    // A server's row: servers/<name>/<what>.
    if let Some(rest) = id.strip_prefix("servers/") {
        let (name, what) = rest.rsplit_once('/').unwrap_or((rest, ""));
        return row_clicked(s, name, what);
    }
    if let Some(kind) = id.strip_prefix("doctor/").and_then(|r| r.rsplit_once('/')) {
        return fix(s, kind.1);
    }
    match id {
        "add" => {
            let n: u64 = s.add_n.trim().parse().unwrap_or(1);
            match call("add", &json!({"n": n})) {
                Ok(names) => {
                    let names: Vec<String> = names
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    notify(&format!("Added {}", names.join(", ")));
                    refresh(s);
                }
                Err(e) => host::ask("error", "Server host", &e, &["OK".into()], true),
            }
        }
        "start-all" => {
            act(s, "set-run", json!({"name": "*", "run": true}));
        }
        "stop-all" => {
            act(s, "set-run", json!({"name": "*", "run": false}));
        }
        "start-host" => {
            act(s, "start-host", json!({}));
            s.running = Some(true);
        }
        "settings" => {
            s.form = settings_form(&s.st);
            s.form_error.clear();
            s.over = Some(Over::Settings);
        }
        "defaults" => {
            if let Ok(d) = call("defaults", &json!({})) {
                for k in [
                    "socket_uri",
                    "serverdb_uri",
                    "http_uri",
                    "http_key",
                    "server_key",
                ] {
                    s.form.insert(k.into(), text_at(&d, k));
                }
                // The launcher's own defaults are the old addresses up to 0.13.3.
                for k in ["socket_uri", "serverdb_uri"] {
                    s.form.insert(k.into(), ECHOVRCE_NEVR.into());
                }
                s.form.insert("publisher_lock".into(), "echovrce".into());
            }
        }
        "cancel" => s.over = None,
        "save-edit" => {
            if let Some(Over::Edit(name)) = s.over.clone() {
                let mut args = json!({"name": name});
                for (k, v) in &s.form {
                    args[k] = if k == "mods" {
                        json!(v == "true")
                    } else {
                        json!(v)
                    };
                }
                match call("edit", &args) {
                    Ok(_) => {
                        s.over = None;
                        notify("Saved");
                        refresh(s);
                    }
                    Err(e) => s.form_error = e,
                }
            }
        }
        "save-settings" => {
            let args: Value = s
                .form
                .iter()
                .map(|(k, v)| (k.clone(), json!(kept_address(v))))
                .collect();
            match call("settings", &args) {
                Ok(_) => {
                    s.over = None;
                    notify("Saved: the servers use it at their next start");
                    refresh(s);
                }
                Err(e) => s.form_error = e,
            }
        }
        "copy-console" => {
            if let Some(l) = s.logs.get(&s.console) {
                let _ = server_host::console_copy(*l, &s.filter);
            }
        }
        "open-folder" => {
            let name = s.console.clone();
            act(s, "open-folder", json!({"name": name}));
        }
        "check-again" => {
            s.checks = None;
            run_doctor(s);
        }
        "link-mine" => account_op(s, "link-mine", json!({}), Wait::Account),
        "link-code" => account_op(s, "link-code", json!({}), Wait::Code),
        "code-open" => {
            if let Some((code, _)) = s.code.clone() {
                act(s, "open-approve", json!({"code": code}));
            }
        }
        "code-cancel" => {
            s.code = None;
            s.busy = false;
        }
        "password-open" => s.password = true,
        "password-cancel" => s.password = false,
        "sign-in" => {
            let id = s.discord_id.clone();
            account_op(
                s,
                "password",
                json!({"discord_id": id, "secret": "password"}),
                Wait::Account,
            );
        }
        "test" => account_op(s, "test", json!({}), Wait::Account),
        "registered" => {
            s.registered = None;
            s.over = Some(Over::Registered);
            if let Ok(t) = start("registered", &json!({})) {
                s.waits.insert(t, Wait::Registered);
            }
        }
        "sign-out" => host::ask(
            "sign-out",
            "Sign the servers out?",
            if by_code(s) {
                "The servers forget the sign-ins their codes got: each shows a new code at its next start. Running servers keep going until they start again."
            } else {
                "The servers forget their EchoVRCE account: they can't host until one is linked again. Running servers keep going until they start again."
            },
            &["Sign out".into(), "Cancel".into()],
            true,
        ),
        "webhook-save" => {
            s.webhook_note = None;
            if let Ok(t) = start("webhook", &json!({"secret": "webhook"})) {
                s.waits.insert(t, Wait::Webhook);
            }
        }
        _ => {}
    }
}

fn account_op(s: &mut S, op: &str, args: Value, w: Wait) {
    s.busy = true;
    s.note = None;
    match start(op, &args) {
        Ok(t) => {
            s.waits.insert(t, w);
        }
        Err(e) => {
            s.busy = false;
            s.note = Some((e, false));
        }
    }
}

fn row_clicked(s: &mut S, name: &str, what: &str) {
    let in_match = server(s, name).is_some_and(|v| v["in_match"] == true);
    match what {
        "start" => {
            act(s, "set-run", json!({"name": name, "run": true}));
        }
        "stop" if in_match => host::ask(
            &format!("stop:{name}"),
            &format!("{name} is hosting a match"),
            "Stop it once the match is over, or now (the players in it are dropped)?",
            &["After the match".into(), "Now".into(), "Cancel".into()],
            false,
        ),
        "stop" => {
            act(s, "set-run", json!({"name": name, "run": false}));
        }
        "restart" if in_match => host::ask(
            &format!("restart:{name}"),
            &format!("{name} is hosting a match"),
            "Start it again once the match is over, or now (the players in it are dropped)?",
            &["After the match".into(), "Now".into(), "Cancel".into()],
            false,
        ),
        "restart" => {
            if act(s, "command", json!({"name": name, "what": "restart"})) {
                notify(&format!("{name} starts again"));
            }
        }
        "approve" => {
            let code = server(s, name).map(|v| text_at(v, "sign_in_code"));
            if let Some(code) = code.filter(|c| !c.is_empty()) {
                act(s, "open-approve", json!({"code": code}));
            }
        }
        "console" => open_console(s, name),
        "edit" => {
            if let Some(v) = server(s, name).cloned() {
                s.form = edit_form(&v);
                s.form_error.clear();
                s.over = Some(Over::Edit(name.to_string()));
            }
        }
        "delete" => host::ask(
            &format!("delete:{name}"),
            &format!("Delete {name}?"),
            "It stops (now, even in a match), and its folder goes: its copy of the game's files, its logs and sign-in.",
            &["Delete".into(), "Cancel".into()],
            true,
        ),
        _ => {}
    }
}

fn dialog(s: &mut S, id: &str, i: u32) {
    let (what, name) = id.split_once(':').unwrap_or((id, ""));
    match (what, i) {
        ("stop", 0) => {
            act(s, "set-run", json!({"name": name, "run": false}));
        }
        ("stop", 1) => {
            act(s, "command", json!({"name": name, "what": "stop-now"}));
            act(s, "set-run", json!({"name": name, "run": false}));
        }
        ("restart", 0) => {
            act(s, "command", json!({"name": name, "what": "apply"}));
        }
        ("restart", 1) => {
            act(s, "command", json!({"name": name, "what": "restart"}));
        }
        ("delete", 0) => {
            if act(s, "delete", json!({"name": name})) {
                notify("Server deleted");
            }
        }
        ("sign-out", 0) => {
            s.note = Some(if act(s, "sign-out", json!({})) {
                (
                    "Signed out. Running servers keep going until they start again.".into(),
                    true,
                )
            } else {
                ("Couldn't sign the servers out.".into(), false)
            });
        }
        _ => {}
    }
}

fn fix(s: &mut S, kind: &str) {
    match kind {
        "sign-in" => s.tab = Tab::Servers,
        "" => {}
        k => {
            if let Ok(t) = start("fix", &json!({"kind": k})) {
                s.waits.insert(t, Wait::Fix);
            }
        }
    }
}

fn done(s: &mut S, t: u32, r: Result<String, String>) {
    let Some(w) = s.waits.remove(&t) else {
        return;
    };
    let v = r.map(|t| parse(&t));
    match w {
        Wait::Running => s.running = v.ok().and_then(|v| v.as_bool()),
        Wait::Account => {
            s.busy = false;
            s.code = None;
            match v {
                Ok(note) => {
                    s.note = Some((note.as_str().unwrap_or("Done.").to_string(), true));
                    s.password = false;
                }
                Err(e) => s.note = Some((e, false)),
            }
            refresh(s);
        }
        Wait::Code => match v {
            Ok(c) => {
                let (code, url) = (text_at(&c, "code"), text_at(&c, "url"));
                let expires = c["expires"].as_u64().unwrap_or(600);
                s.code = Some((code.clone(), url));
                match start("link-wait", &json!({"code": code, "expires": expires})) {
                    Ok(t) => {
                        s.waits.insert(t, Wait::Account);
                    }
                    Err(e) => {
                        s.busy = false;
                        s.note = Some((e, false));
                    }
                }
            }
            Err(e) => {
                s.busy = false;
                s.note = Some((e, false));
            }
        },
        Wait::Doctor => {
            s.checking = false;
            s.checks =
                Some(v.unwrap_or_else(|e| json!([{"name": "Doctor", "level": "bad", "text": e}])));
        }
        Wait::Fix => {
            match v {
                Ok(t) => {
                    if let Some(t) = t.as_str() {
                        notify(t);
                    }
                }
                Err(e) => host::ask("error", "Server host", &e, &["OK".into()], true),
            }
            s.checks = None;
            run_doctor(s);
        }
        Wait::Registered => s.registered = Some(v),
        Wait::Webhook => {
            s.webhook_note = Some(match v {
                Ok(t) => (t.as_str().unwrap_or("Saved.").to_string(), true),
                Err(e) => (e, false),
            });
            refresh(s);
        }
    }
}

// ---- the page ----

fn main_card(ui: &mut Ui, s: &mut S) {
    let tab = s.tab as u32;
    let st = s.st.clone();
    let log = if s.tab == Tab::Console && !s.console.is_empty() {
        let name = s.console.clone();
        log_of(s, &name)
    } else {
        None
    };
    ui.card(0, "", "", |c| {
        c.tabs("tab", &["Servers", "Console", "Doctor"], tab, |t| {
            match s.tab {
                Tab::Servers => servers_tab(t, s, &st),
                Tab::Console => console_tab(t, s, log),
                Tab::Doctor => doctor_tab(t, s),
            }
        });
    });
}

fn servers_tab(t: &mut Kids, s: &S, st: &Value) {
    if !s.error.is_empty() {
        t.warn(format!("The server host's files: {}", s.error));
    }
    let list = st["servers"].as_array().cloned().unwrap_or_default();
    let wanted = list.iter().any(|v| v["run"] == true);
    if wanted && s.running == Some(false) {
        t.warn("The server host isn't running, so the servers set to run aren't.");
        t.button(button("start-host", "START IT").primary());
    }
    if !signed_in(s) && !by_code(s) {
        t.warn("Sign the servers in first (Account, on the right): they need an account of their own on EchoVRCE.");
    }
    let full = list.len() as u64 >= st["max_servers"].as_u64().unwrap_or(32);
    t.field("add_n", "How many to add", &s.add_n, "1");
    t.buttons(vec![
        button("add", "ADD").primary().enabled(!full),
        button("start-all", "START ALL").enabled(!list.is_empty()),
        button("stop-all", "STOP ALL").enabled(!list.is_empty()),
        button("settings", "SETTINGS"),
    ]);
    let rows: Vec<Row> = list
        .iter()
        .map(|v| {
            let run = v["run"] == true;
            let alive = v["alive"] == true;
            let approve = !text_at(v, "sign_in_code").is_empty();
            let mut buttons = Vec::new();
            if approve {
                buttons.push(button("approve", "APPROVE").primary());
            }
            buttons.extend([
                if run {
                    button("stop", "STOP")
                } else {
                    button("start", "START").primary().enabled(!approve)
                },
                button("restart", "RESTART").enabled(run && alive),
                button("console", "Its live console").icon("info"),
                button("edit", "Its port, regions, guilds, tick rate and more").icon("gear"),
                button("delete", "Delete this server").icon("trash"),
            ]);
            Row {
                key: text_at(v, "name"),
                cells: vec![
                    cell(text_at(v, "name")),
                    chip_cell(text_at(v, "chip"), tone(&text_at(v, "tone"))),
                    cell(text_at(v, "line")),
                ],
                detail: text_at(v, "detail"),
                buttons,
            }
        })
        .collect();
    let first = st["first_port"].as_u64().unwrap_or(6792);
    t.table(
        "servers",
        rows,
        &format!("No servers yet. Add one: it gets the next free port from {first}. Each server's port has to be forwarded (UDP and TCP, the same number outside) to this PC."),
    );
}

fn console_tab(t: &mut Kids, s: &S, log: Option<u32>) {
    let names = servers(s);
    if names.is_empty() {
        t.muted("No servers yet.");
        return;
    }
    t.choice(
        "console",
        "Server",
        names.iter().map(|n| (n.clone(), n.clone())).collect(),
        &s.console,
    );
    t.toggle("follow", "Follow (the newest lines in view)", s.follow);
    t.toggle(
        "noise",
        "Noise (nEVR's high-volume diagnostics too)",
        s.noise,
    );
    t.field(
        "filter",
        "Filter",
        &s.filter,
        "Only lines with this (any case)",
    );
    t.buttons(vec![
        button("copy-console", "COPY"),
        button("open-folder", "OPEN ITS FOLDER"),
    ]);
    match log {
        Some(l) => {
            t.log_view(l, &s.filter, s.follow, s.noise, 24);
        }
        None => {
            t.warn("Its console can't be shown.");
        }
    }
}

fn doctor_tab(t: &mut Kids, s: &S) {
    t.button(button("check-again", "CHECK AGAIN").enabled(!s.checking));
    let Some(checks) = s.checks.as_ref().and_then(Value::as_array) else {
        t.muted("Checking…");
        return;
    };
    let rows: Vec<Row> = checks
        .iter()
        .map(|c| {
            let level = text_at(c, "level");
            let fix = text_at(c, "fix");
            Row {
                key: text_at(c, "name"),
                cells: vec![
                    cell(text_at(c, "name")),
                    chip_cell(
                        match level.as_str() {
                            "good" => "OK",
                            "warn" => "Check",
                            _ => "Problem",
                        },
                        tone(&level),
                    ),
                ],
                detail: text_at(c, "text"),
                buttons: if fix.is_empty() {
                    Vec::new()
                } else {
                    vec![button(&fix, &text_at(c, "label").to_uppercase())]
                },
            }
        })
        .collect();
    t.table("doctor", rows, "");
}

fn edit_card(ui: &mut Ui, s: &S, name: &str) {
    let in_match = server(s, name).is_some_and(|v| v["in_match"] == true);
    let f = &s.form;
    let g = |k: &str| f.get(k).cloned().unwrap_or_default();
    ui.card(0, &format!("Server {name}"), "", |c| {
        c.field(
            "port",
            "Port (UDP and TCP)",
            &g("port"),
            "Forward it to this PC with the same number outside",
        );
        c.field(
            "regions",
            "Regions (comma-separated)",
            &g("regions"),
            "Where it registers, e.g. uwu, euw. Empty: EchoVRCE's default",
        );
        c.field(
            "guilds",
            "Guilds (ids, comma-separated)",
            &g("guilds"),
            "Each has to give the account its server host role. Empty: any",
        );
        c.field(
            "tick_rate",
            "Tick rate (frames per second)",
            &g("tick_rate"),
            "120 is EchoVRCE's",
        );
        c.field(
            "empty_ttl",
            "Empty server stays (seconds)",
            &g("empty_ttl"),
            "0: nEVR's own",
        );
        c.field(
            "extra_args",
            "More arguments for the game",
            &g("extra_args"),
            "Space-separated",
        );
        c.toggle(
            "mods",
            "Load the mods the Mods page has on (a content pack such as EchoCombat too)",
            g("mods") == "true",
        );
        if !s.form_error.is_empty() {
            c.warn(s.form_error.clone());
        }
        c.muted(if in_match {
            "It is hosting a match: the changes apply once that is over."
        } else {
            "A running server starts again with the changes."
        });
        c.buttons(vec![
            button("save-edit", "SAVE").primary(),
            button("cancel", "CANCEL"),
        ]);
    });
}

fn settings_card(ui: &mut Ui, s: &S) {
    let f = &s.form;
    let g = |k: &str| f.get(k).cloned().unwrap_or_default();
    let info = parse(&host::launcher_info());
    let mut versions: Vec<(String, String)> =
        vec![(String::new(), "The live build PLAY starts".into())];
    for v in info["versions"].as_array().into_iter().flatten() {
        if v["live"] == true {
            versions.push((text_at(v, "id"), text_at(v, "name")));
        }
    }
    ui.card(0, "Server host settings", "", |c| {
        c.choice(
            "version",
            "Game version the servers run",
            versions,
            &g("version"),
        );
        c.field(
            "regions",
            "New servers: regions",
            &g("regions"),
            "Comma-separated. Empty: EchoVRCE's default",
        );
        c.field(
            "guilds",
            "New servers: guilds",
            &g("guilds"),
            "Guild ids, comma-separated. Empty: any",
        );
        c.field(
            "tick_rate",
            "New servers: tick rate",
            &g("tick_rate"),
            "10 to 1000",
        );
        c.field(
            "socket_uri",
            "Login (services.socket_uri)",
            &g("socket_uri"),
            "ws:// or wss://",
        );
        c.field(
            "serverdb_uri",
            "Registration (services.serverdb_uri)",
            &g("serverdb_uri"),
            "ws:// or wss://",
        );
        c.field(
            "http_uri",
            "HTTP (auth.http_uri)",
            &g("http_uri"),
            "http:// or https://, e.g. https://g.echovrce.com:7350",
        );
        c.field("http_key", "HTTP key", &g("http_key"), "");
        c.field("server_key", "Server key", &g("server_key"), "");
        c.field(
            "publisher_lock",
            "Publisher lock",
            &g("publisher_lock"),
            "echovrce",
        );
        c.field(
            "internal_ip",
            "Bind address (network.internal_ip)",
            &g("internal_ip"),
            "Empty: nEVR's choice",
        );
        c.field(
            "external_ip",
            "Public address (network.external_ip)",
            &g("external_ip"),
            "Empty: the one EchoVRCE sees",
        );
        if !s.form_error.is_empty() {
            c.warn(s.form_error.clone());
        }
        c.buttons(vec![
            button("save-settings", "SAVE").primary(),
            button("defaults", "ECHOVRCE'S"),
            button("cancel", "CANCEL"),
        ]);
    });
}

fn registered_card(ui: &mut Ui, s: &S) {
    ui.card(0, "On EchoVRCE", "", |c| {
        match &s.registered {
            None => {
                c.muted("Asking EchoVRCE…");
            }
            Some(Err(e)) => {
                c.warn(e.clone());
            }
            Some(Ok(list)) => {
                let list = list.as_array().cloned().unwrap_or_default();
                if list.is_empty() {
                    c.text("EchoVRCE has no server of this account registered right now.", Tone::Normal);
                } else {
                    c.muted(format!("{} server(s) of this account are registered (from any PC). Where they are is EchoVRCE's guess, not the regions asked for.", list.len()));
                    let rows = list
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            let mode = text_at(r, "mode");
                            let mode = if mode.is_empty() {
                                "Waiting for a match".to_string()
                            } else {
                                format!("{mode}, {} player(s)", text_at(r, "players"))
                            };
                            let place: Vec<String> = [text_at(r, "region"), text_at(r, "country")]
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect();
                            Row {
                                key: i.to_string(),
                                cells: vec![cell(mode), cell(place.join(", "))],
                                detail: match r["guilds"].as_u64().unwrap_or(0) {
                                    0 => String::new(),
                                    n => format!("{n} guild(s)"),
                                },
                                buttons: Vec::new(),
                            }
                        })
                        .collect();
                    c.table("registered", rows, "");
                }
            }
        }
        c.button(button("cancel", "CLOSE"));
    });
}

fn account_card(ui: &mut Ui, s: &S) {
    let info = parse(&host::launcher_info());
    let mine = info["echovrce"]["signed_in"] == true;
    let who = text_at(&s.st, "account.who");
    let how = text_at(&s.st, "account.sign_in");
    ui.card(1, "Account", "", |c| {
        c.text(
            match how.as_str() {
                "device" => format!("The servers sign in as {who} (linked, no password)."),
                "password" => format!("The servers sign in as {who} with its Discord ID and password."),
                _ if by_code(s) => "No account needed: each server shows a code the first time it starts (APPROVE on its row). Approve it signed in on echovrce.com as the account the servers should play as, one of their own or yours if it has no password; the server keeps that sign-in. Or sign them all in as one account here:".to_string(),
                _ => "The servers aren't signed in yet: link an EchoVRCE account for them below. Use an account of their own, or the one you play with if it has no password.".to_string(),
            },
            Tone::Normal,
        );
        if let Some((code, _)) = &s.code {
            c.text(
                format!("Approve the code {code} on echovrce.com (it opened in your browser), signed in as the servers' account."),
                Tone::Normal,
            );
            c.buttons(vec![button("code-open", "OPEN THE PAGE"), button("code-cancel", "CANCEL")]);
        } else if how == "none" || how.is_empty() {
            if s.password {
                c.field("discord_id", "Discord ID", &s.discord_id, "Discord: Developer Mode, right-click the account, Copy User ID");
                c.secret("password", "Password", "The account's EchoVRCE password");
                c.muted("The password reaches the game only while it starts, never a file.");
                c.buttons(vec![
                    button("sign-in", if s.busy { "CHECKING…" } else { "SIGN IN" })
                        .primary()
                        .enabled(!s.busy && !s.discord_id.trim().is_empty()),
                    button("password-cancel", "CANCEL"),
                ]);
            } else {
                c.button(
                    button("link-mine", "USE MY ECHOVRCE ACCOUNT")
                        .primary()
                        .enabled(mine && !s.busy),
                );
                if !mine {
                    c.muted("To use your own account: sign in on the EchoVRCE page first.");
                }
                c.buttons(vec![
                    button("link-code", "LINK ANOTHER").enabled(!s.busy),
                    button("password-open", "PASSWORD").enabled(!s.busy),
                ]);
                let n = s.st["account"]["code_signed_in"].as_u64().unwrap_or(0);
                if by_code(s) && n > 0 {
                    c.muted(format!("{n} server(s) signed in by their code. SIGN OUT forgets those sign-ins: each shows a new code at its next start."));
                    c.button(button("sign-out", "SIGN OUT").danger().enabled(!s.busy));
                }
            }
        } else {
            c.buttons(vec![
                button("test", "TEST").enabled(!s.busy),
                button("registered", "ON ECHOVRCE").enabled(!s.busy),
                button("sign-out", "SIGN OUT").danger().enabled(!s.busy),
            ]);
        }
        if let Some((note, good)) = &s.note {
            c.text(note.clone(), if *good { Tone::Good } else { Tone::Warn });
        }
        c.muted("A linked account must have no password on EchoVRCE, or EchoVRCE refuses the servers' sign-in. To remove it: /reset-password in the Echo VR Lounge Discord.");
    });
}

fn background_card(ui: &mut Ui, s: &S) {
    let webhook = s.st["alerts"]["webhook"] == true;
    ui.card(1, "In the background", "", |c| {
        c.text(
            match s.running {
                Some(true) => {
                    "The server host is running: the servers keep going with the launcher closed."
                }
                Some(false) => {
                    "The server host isn't running: it starts with the first server you start."
                }
                None => "Looking…",
            },
            Tone::Normal,
        );
        c.toggle(
            "autostart",
            "Start the servers with this PC",
            s.st["autostart"] == true,
        );
        c.toggle(
            "desktop",
            "Desktop alerts",
            s.st["alerts"]["desktop"] != false,
        );
        c.secret(
            "webhook",
            "Discord webhook (optional)",
            if webhook {
                "A webhook is set: type a new one, or save it empty to remove it"
            } else {
                "https://discord.com/api/webhooks/…"
            },
        );
        c.button(button("webhook-save", "SAVE THE WEBHOOK"));
        if let Some((note, good)) = &s.webhook_note {
            c.text(note.clone(), if *good { Tone::Good } else { Tone::Warn });
        }
    });
}

export_plugin!(ServerHost);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_only_the_old_echovrce_addresses() {
        let c = nevr_address_change(OLD_LOGIN, OLD_REGISTRATION).unwrap();
        assert_eq!(c["socket_uri"], ECHOVRCE_NEVR);
        assert_eq!(c["serverdb_uri"], ECHOVRCE_NEVR);
        // Another registration set by the host stays.
        let c = nevr_address_change(OLD_LOGIN, "wss://other:443/nevr").unwrap();
        assert!(c.get("serverdb_uri").is_none());
        assert!(nevr_address_change(ECHOVRCE_NEVR, OLD_REGISTRATION).is_none());
        assert!(nevr_address_change("wss://other/nevr", "").is_none());
    }

    #[test]
    fn keeps_echovrces_address_without_its_port() {
        assert_eq!(
            kept_address(" wss://g.echovrce.com:443/nevr "),
            ECHOVRCE_NEVR
        );
        assert_eq!(kept_address("wss://other:443/nevr"), "wss://other:443/nevr");
    }
}
