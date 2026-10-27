//! Event lobbies: play the event builds together on the classic lobbies server (EchoRelay's
//! matches API). The public matches, the player's own match and its id, joining a match by
//! its id, and requesting a game server; Join and Request start the event build after.
//!
//! The player's account: the one typed in its settings, else the launcher's classic lobbies
//! account, which the launcher fills in only for the launcher's own classic lobbies server
//! (`relay_account`): the password never reaches this plugin.
use echovr_plugin::*;
use std::collections::BTreeMap;

/// Matches and the player's match every 10 s, the regions every minute.
const MATCHES: u32 = 1;
const REGIONS: u32 = 2;

/// What a request was for.
#[derive(Clone)]
enum Req {
    Matches,
    Regions,
    Current,
    /// Joining a match: the build to start when its answer doesn't name one.
    Join(String),
    /// Requesting a game server for this build.
    Request(String),
}

/// How a list failed: it couldn't be reached, the server doesn't have the API, it
/// answered with an error, or the answer wasn't what was expected.
#[derive(Clone, PartialEq)]
enum Failed {
    Unreachable,
    Missing,
    Server(String),
    Other(String),
}

#[derive(Default)]
struct S {
    info: Value,
    pending: BTreeMap<u32, Req>,
    matches: Option<Result<Vec<Value>, Failed>>,
    regions: Option<Result<Vec<String>, Failed>>,
    current: Value,
    /// The server's message when it couldn't look up the player's match.
    current_note: String,
    build: String,
    region: String,
    join_id: String,
    /// The last Join or Request: its success, or why it failed.
    answer: String,
    error: String,
    busy: bool,
}

static STATE: State<S> = State::new();

struct EventLobbies;

impl Plugin for EventLobbies {
    fn init(info: &Value) -> Result<(), String> {
        STATE.with(|s| s.info = info.clone());
        Ok(())
    }

    fn render(ui: &mut Ui) {
        STATE.with(|s| render(ui, s));
    }

    fn on_event(e: Event) {
        STATE.with(|s| event(s, e));
        host::invalidate();
    }

    fn on_timer(id: u32) {
        STATE.with(|s| match id {
            MATCHES => {
                fetch(s, Req::Matches);
                fetch(s, Req::Current);
            }
            REGIONS => fetch(s, Req::Regions),
            _ => {}
        });
        host::invalidate();
    }
}

export_plugin!(EventLobbies);

// ---- what it asks ----

/// The classic lobbies server: its settings', else the launcher's.
fn server(s: &S) -> String {
    let own = setting("server");
    if own.trim().is_empty() {
        text_at(&s.info, "relay.server")
    } else {
        own.trim().to_string()
    }
}

/// The event build picked (the first installed one until then).
fn build(s: &S) -> String {
    let builds = builds(s);
    if builds.iter().any(|(id, _)| *id == s.build) {
        s.build.clone()
    } else {
        builds.first().map(|b| b.0.clone()).unwrap_or_default()
    }
}

/// The installed event builds: (id, name).
fn builds(s: &S) -> Vec<(String, String)> {
    at(&s.info, "event_builds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|b| (text_at(b, "id"), text_at(b, "name")))
        .filter(|(id, _)| !id.is_empty())
        .collect()
}

/// The account a request sends: the one typed in its settings, else the launcher's (its
/// name and password filled in by the launcher, for the launcher's own server only).
/// `None`: there is none to send.
fn account(s: &S) -> Option<(Value, bool)> {
    let name = setting("name");
    if !name.trim().is_empty() {
        return Some((
            json!({"displayname": name.trim(), "password": setting("password")}),
            false,
        ));
    }
    if text_at(&s.info, "relay.name").is_empty() {
        return None;
    }
    Some((
        json!({"displayname": "{relay.name}", "password": "{relay.password}"}),
        true,
    ))
}

fn fetch(s: &mut S, r: Req) {
    if s.pending.values().any(|p| same(p, &r)) {
        return;
    }
    let base = format!("http://{}", server(s));
    let sent = match &r {
        Req::Matches => get(&format!("{base}/api/matches")),
        Req::Regions => {
            let b = build(s);
            if b.is_empty() {
                return;
            }
            get(&format!("{base}/api/servers/regions?build={b}"))
        }
        Req::Current => {
            let Some((body, relay)) = account(s) else {
                return;
            };
            post(&format!("{base}/api/matches/current"), &body, relay)
        }
        Req::Join(_) | Req::Request(_) => unreachable!("sent by join() and request()"),
    };
    match sent {
        Ok(t) => {
            s.pending.insert(t, r);
        }
        Err(e) => {
            let f = Failed::Other(e);
            match r {
                Req::Matches => s.matches = Some(Err(f)),
                Req::Regions => s.regions = Some(Err(f)),
                _ => {}
            }
        }
    }
}

fn same(a: &Req, b: &Req) -> bool {
    matches!(
        (a, b),
        (Req::Matches, Req::Matches) | (Req::Regions, Req::Regions) | (Req::Current, Req::Current)
    )
}

/// Join match `id`; `build`: the build to start when the answer names none.
fn join(s: &mut S, id: &str, build: &str) {
    let Some((mut body, relay)) = account(s) else {
        s.error =
            "Set up your classic lobbies account first: on the Play page, or in Account here."
                .into();
        return;
    };
    body["id"] = json!(id.trim());
    let url = format!("http://{}/api/matches/join", server(s));
    send(s, &url, &body, relay, Req::Join(build.to_string()));
}

fn request(s: &mut S) {
    let Some((mut body, relay)) = account(s) else {
        s.error =
            "Set up your classic lobbies account first: on the Play page, or in Account here."
                .into();
        return;
    };
    let b = build(s);
    body["build"] = json!(b);
    body["region"] = json!(region(s));
    let url = format!("http://{}/api/servers/request", server(s));
    send(s, &url, &body, relay, Req::Request(b));
}

fn send(s: &mut S, url: &str, body: &Value, relay: bool, r: Req) {
    s.answer.clear();
    s.error.clear();
    match post(url, body, relay) {
        Ok(t) => {
            s.pending.insert(t, r);
            s.busy = true;
        }
        Err(e) => s.error = e,
    }
}

/// The region picked (the first one until then).
fn region(s: &S) -> String {
    match &s.regions {
        Some(Ok(r)) if r.contains(&s.region) => s.region.clone(),
        Some(Ok(r)) => r.first().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

// ---- what happened ----

fn event(s: &mut S, e: Event) {
    match e {
        Event::Shown => {
            s.info = parse(&host::launcher_info());
            fetch(s, Req::Matches);
            fetch(s, Req::Current);
            fetch(s, Req::Regions);
            host::set_timer(MATCHES, 10_000, true);
            host::set_timer(REGIONS, 60_000, true);
        }
        Event::Hidden => {
            host::clear_timer(MATCHES);
            host::clear_timer(REGIONS);
        }
        Event::LauncherChanged => {
            let was = server(s);
            s.info = parse(&host::launcher_info());
            if server(s) != was {
                s.matches = None;
                s.regions = None;
                s.current = Value::Null;
                fetch(s, Req::Matches);
                fetch(s, Req::Current);
            }
            if s.regions.is_none() {
                fetch(s, Req::Regions);
            }
        }
        Event::SettingsChanged => {
            s.info = parse(&host::launcher_info());
            s.matches = None;
            s.regions = None;
            s.current = Value::Null;
            fetch(s, Req::Matches);
            fetch(s, Req::Current);
            fetch(s, Req::Regions);
        }
        Event::Changed((id, v)) => match id.as_str() {
            "join_id" => s.join_id = v,
            "build" => {
                s.build = v;
                s.regions = None;
                fetch(s, Req::Regions);
            }
            "region" => s.region = v,
            _ => {}
        },
        Event::Clicked(id) => match id.as_str() {
            "join" => {
                let (id, b) = (s.join_id.clone(), build(s));
                join(s, &id, &b);
            }
            "request" => request(s),
            "refresh" => {
                fetch(s, Req::Matches);
                fetch(s, Req::Current);
            }
            other => {
                // A public match's Join: matches/<id>/join.
                if let Some(mid) = other
                    .strip_prefix("matches/")
                    .and_then(|r| r.strip_suffix("/join"))
                {
                    let b = list(s)
                        .iter()
                        .find(|m| text_at(m, "id") == mid)
                        .map(|m| text_at(m, "build"))
                        .unwrap_or_default();
                    join(s, mid, &b);
                }
            }
        },
        Event::NetDone((t, r)) => {
            if let Some(req) = s.pending.remove(&t) {
                done(s, req, r);
            }
        }
        _ => {}
    }
}

/// Pure: why answer `r` isn't a good one, or its body as JSON.
fn answer(r: &Result<Response, String>) -> Result<Value, Failed> {
    match r {
        Err(_) => Err(Failed::Unreachable),
        Ok(resp) if resp.status == 404 => Err(Failed::Missing),
        Ok(resp) if resp.status >= 500 => Err(Failed::Server(format!("HTTP {}", resp.status))),
        Ok(resp) => {
            let v = parse(&resp.body);
            if v.is_null() {
                Err(Failed::Other(format!("HTTP {}: not JSON", resp.status)))
            } else {
                Ok(v)
            }
        }
    }
}

/// What the server says when the player never signed in there with the game.
const NOT_SIGNED_IN: &str = "Log in to the game on this server once first";

fn done(s: &mut S, req: Req, r: Result<Response, String>) {
    match req {
        Req::Matches => {
            s.matches = Some(answer(&r).map(|v| {
                at(&v, "matches")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            }));
        }
        Req::Regions => {
            s.regions = Some(answer(&r).map(|v| {
                at(&v, "regions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.as_str().map(String::from))
                    .collect()
            }));
        }
        Req::Current => {
            let v = answer(&r).unwrap_or(Value::Null);
            s.current_note = text_at(&v, "message");
            s.current = at(&v, "match").cloned().unwrap_or(Value::Null);
        }
        Req::Join(fallback) | Req::Request(fallback) => {
            s.busy = false;
            let v = match answer(&r) {
                Ok(v) => v,
                Err(Failed::Unreachable) => {
                    s.error = format!("Can't reach {}.", server(s));
                    return;
                }
                Err(_) => {
                    // Its answer may still say why.
                    match &r {
                        Ok(resp) => parse(&resp.body),
                        Err(_) => Value::Null,
                    }
                }
            };
            let message = text_at(&v, "message");
            if message.starts_with(NOT_SIGNED_IN) {
                s.error = "Start an event build once and press PLAY in it first: that signs you in on this server. Then you can join a match or request a game server.".into();
                return;
            }
            if !truthy(at(&v, "ok")) {
                s.error = if message.is_empty() {
                    "The server didn’t answer.".into()
                } else {
                    message
                };
                return;
            }
            s.answer = message;
            let b = match text_at(&v, "build") {
                b if b.is_empty() => fallback,
                b => b,
            };
            if let Err(e) = game::play(&b, &[]) {
                s.error = format!("Couldn't start the game: {e}");
            }
            fetch(s, Req::Current);
        }
    }
}

// ---- the page ----

fn list(s: &S) -> Vec<Value> {
    match &s.matches {
        Some(Ok(m)) => m.clone(),
        _ => Vec::new(),
    }
}

fn render(ui: &mut Ui, s: &S) {
    let server = server(s);
    let builds = builds(s);
    ui.card(
        0,
        "Public matches",
        &format!("Running on {server}. Who plays in them isn't shown."),
        |c| {
            if builds.is_empty() {
                c.warn("Install an event build first: the Install page lists them.");
            }
            match &s.matches {
                Some(Err(Failed::Unreachable)) => {
                    c.warn(format!("Can't reach {server}: the server may be down, or this PC is offline. Trying again every 10 seconds."));
                }
                Some(Err(Failed::Missing)) => {
                    c.warn("This server doesn't offer the matches list yet: its EchoRelay needs the matches API. Joining by id and requesting a game server need it too.");
                }
                Some(Err(Failed::Server(e))) => {
                    c.warn(format!("The server answered with an error ({e}). Trying again every 10 seconds."));
                }
                Some(Err(Failed::Other(e))) => {
                    c.warn(format!("Couldn't load the matches list ({e}). Trying again every 10 seconds."));
                }
                _ => {}
            }
            let rows: Vec<Row> = list(s)
                .iter()
                .map(|m| {
                    let or = |a: &str, b: &str| {
                        let v = text_at(m, a);
                        if v.is_empty() {
                            text_at(m, b)
                        } else {
                            v
                        }
                    };
                    Row {
                        key: text_at(m, "id"),
                        cells: vec![cell(format!(
                            "{} · {}",
                            or("build_name", "build"),
                            or("mode", "gametype")
                        ))],
                        detail: format!(
                            "{} · {}/{} players",
                            text_at(m, "level"),
                            text_at(m, "players"),
                            text_at(m, "limit")
                        ),
                        buttons: vec![button("join", "JOIN")
                            .enabled(truthy(at(m, "joinable")) && !s.busy)],
                    }
                })
                .collect();
            c.table(
                "matches",
                rows,
                if s.matches.is_none() {
                    "Loading…"
                } else {
                    "No public matches right now. Request a game server to start one."
                },
            );
            c.button(button("refresh", "REFRESH"));
        },
    );
    ui.card(
        0,
        "Your match",
        "Give your friends this id: they join with it, also in a private match.",
        |c| {
            let id = text_at(&s.current, "id");
            if id.is_empty() {
                c.muted("You aren't in a match.");
            } else {
                c.value("Match id", id, true);
                let mode = match text_at(&s.current, "mode") {
                    m if m.is_empty() => text_at(&s.current, "gametype"),
                    m => m,
                };
                c.muted(format!(
                    "{mode} on {} · {}/{} players",
                    text_at(&s.current, "level"),
                    text_at(&s.current, "players"),
                    text_at(&s.current, "limit")
                ));
                if truthy(at(&s.current, "private")) {
                    c.muted("Private match: only players with its id can join.");
                }
            }
            if s.current_note.starts_with(NOT_SIGNED_IN) {
                c.warn("Start an event build once and press PLAY in it first: that signs you in on this server. Then your match shows here, and you can join by id and request game servers.");
            }
        },
    );
    let relay_name = match text_at(&s.info, "relay.name") {
        n if n.is_empty() => "none yet".to_string(),
        n => n,
    };
    ui.card(
        0,
        "Account",
        &format!(
            "Who you play as there. Empty: the launcher's classic lobbies account ({relay_name} on {}).",
            text_at(&s.info, "relay.server")
        ),
        |c| {
            c.form("Account");
        },
    );
    ui.card(
        1,
        "Join by id",
        "A friend's match id, public or private. Then Play starts the game and puts you there.",
        |c| {
            c.field(
                "join_id",
                "Match id",
                &s.join_id,
                "e.g. 34bc3367-bdbf-cd42-0444-fccbe1ca795f",
            );
            c.button(
                button("join", "JOIN")
                    .primary()
                    .enabled(!s.join_id.trim().is_empty() && !s.busy),
            );
        },
    );
    let regions = match &s.regions {
        Some(Ok(r)) => r.clone(),
        _ => Vec::new(),
    };
    ui.card(
        1,
        "Request a game server",
        "Starts a game server for you; your next Play goes there. Limited to a few every ten minutes.",
        |c| {
            c.choice("build", "Version", builds.clone(), &build(s));
            c.choice(
                "region",
                "Region",
                regions.iter().map(|r| (r.clone(), r.clone())).collect(),
                &region(s),
            );
            if matches!(&s.regions, Some(Ok(r)) if r.is_empty()) {
                c.warn("No PC hosts this version right now.");
            }
            c.button(
                button("request", "REQUEST")
                    .primary()
                    .enabled(!regions.is_empty() && !s.busy),
            );
        },
    );
    if !s.answer.is_empty() || !s.error.is_empty() {
        ui.card(1, "", "", |c| {
            if !s.answer.is_empty() {
                c.text(s.answer.clone(), Tone::Good);
            }
            if !s.error.is_empty() {
                c.warn(s.error.clone());
            }
        });
    }
}
