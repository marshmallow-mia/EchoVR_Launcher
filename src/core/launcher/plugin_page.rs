//! Launcher plugins' pages: a plugin describes its page in its `plugin.json` (cards of
//! blocks, the data it fetches, the actions its buttons run) and the launcher draws it with
//! its own widgets. No plugin code runs in the launcher. The format:
//! `docs/plugins/pages.md`.
//!
//! Everything a page shows comes from one JSON context: `settings` (the plugin's settings),
//! `page` (values the page sets while it is open), `data` (what its data sources fetched),
//! `launcher` (what the launcher tells plugins) and, inside a list, `item`. Text in a
//! description fills `{path}` from it: `{settings.name}`, `{data.matches.matches}`,
//! `{item.players}`, with `|` for a fallback (`{settings.server|launcher.relay.server}`).
use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::plugin_settings::{self, Settings, Warning};

/// The description format this launcher reads.
pub const SCHEMA: u64 = 1;
/// How often a data source may be fetched at most (seconds).
pub const MIN_EVERY: u64 = 3;

/// A launcher plugin, as its `plugin.json` describes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PluginPage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub summary: String,
    /// Its rail tab's icon, one of [`ICONS`].
    pub icon: String,
    pub settings: Option<Settings>,
    pub data: Vec<Source>,
    pub actions: BTreeMap<String, Vec<Step>>,
    /// The page: columns of cards, left to right.
    pub columns: Vec<Vec<Card>>,
}

/// The rail icons a plugin can pick.
pub const ICONS: &[&str] = &["plugin", "calendar", "globe"];

/// Something the page fetches, by name (`data.<name>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub name: String,
    pub request: Request,
    /// Fetched again every this many seconds while the page is open (0: once, and on
    /// `refresh`).
    pub every: u64,
    /// Only fetched while this holds (a template; see [`truthy`]).
    pub when: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: Method,
    /// A template.
    pub url: String,
    /// A JSON body whose strings are templates (POST only).
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// A card on the page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Card {
    pub title: String,
    pub help: String,
    /// Shown only while this holds.
    pub when: String,
    pub blocks: Vec<Block>,
}

/// What a card holds, top to bottom. Text is a template throughout.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text {
        text: String,
        tone: Tone,
        when: String,
    },
    /// A value with a label, e.g. an id to give to friends.
    Value {
        label: String,
        value: String,
        copy: bool,
        when: String,
    },
    /// An input bound to `settings.<key>` (kept) or `page.<key>` (while the page is open).
    Field {
        bind: String,
        label: String,
        hint: String,
        secret: bool,
        when: String,
    },
    /// A choice bound like a field, its options from a list in the context.
    Choice {
        bind: String,
        label: String,
        /// The path of a list (`launcher.event_builds`).
        options: String,
        /// Each option's value and text, templates over `item`.
        value: String,
        text: String,
        when: String,
    },
    /// A row per item of a list.
    List {
        /// The path of a list (`data.matches.matches`).
        items: String,
        /// Only items where this holds (a template over `item`).
        filter: String,
        title: String,
        detail: String,
        button: Option<Button>,
        /// Shown when no item is left.
        empty: String,
        when: String,
    },
    Button(Button),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub label: String,
    pub action: String,
    /// Clickable only while this holds (empty: always).
    pub enabled: String,
    pub primary: bool,
    pub when: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Normal,
    Muted,
    Good,
    Warn,
}

/// One step of an action. Steps run in order; a failed step ends the action.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// An HTTP request; its JSON answer goes to `save` (a `page.` path).
    Request {
        request: Request,
        save: String,
    },
    /// Ends the action unless this holds.
    Require {
        when: String,
        otherwise: String,
    },
    /// Sets a `page.` (or `settings.`) value.
    Set {
        path: String,
        to: Value,
    },
    /// Starts the installed version this names (an event build's id, e.g. `halloween`, or
    /// a version id).
    Play {
        version: String,
        args: Vec<String>,
    },
    Copy {
        text: String,
    },
    /// Fetches these data sources again now.
    Refresh {
        sources: Vec<String>,
    },
    /// Sets the server the live build plays on instead of EchoVRCE, and the account there
    /// (the launcher asks the player first). An empty address: EchoVRCE again.
    Server {
        /// `echorelay`, or empty / `nakama` for EchoVRCE's kind.
        kind: String,
        name: String,
        address: String,
        key: String,
        discord_id: String,
        /// EchoRelay: the display name there.
        display_name: String,
        password: String,
    },
}

// ---- parsing ----

fn warn(out: &mut Vec<Warning>, path: &str, message: impl Into<String>) {
    let message = message.into();
    out.push(if path.is_empty() {
        message
    } else {
        format!("{path}: {message}")
    });
}

fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_bind(p: &str) -> bool {
    let mut parts = p.splitn(2, '.');
    matches!(parts.next(), Some("page" | "settings"))
        && parts.next().is_some_and(|k| {
            plugin_settings::is_key(k) || k.split('.').all(plugin_settings::is_key)
        })
}

/// Parses `plugin.json`. A plugin without an id, name or page is refused; a block or step
/// that breaks a rule is left out with a warning.
pub fn parse(text: &str) -> (Option<PluginPage>, Vec<Warning>) {
    let mut w = Vec::new();
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warn(&mut w, "", format!("not JSON: {e}"));
            return (None, w);
        }
    };
    if let Some(schema) = v.get("schema").and_then(Value::as_u64) {
        if schema > SCHEMA {
            warn(
                &mut w,
                "schema",
                format!("schema {schema} is newer than this launcher's {SCHEMA}"),
            );
            return (None, w);
        }
    }
    let id = s(&v, "id");
    if !super::catalog::is_safe_id(&id) {
        warn(
            &mut w,
            "id",
            "a plugin needs an id: lowercase letters, digits, '.', '-', '_'",
        );
        return (None, w);
    }
    let name = s(&v, "name");
    if name.trim().is_empty() {
        warn(&mut w, "name", "a plugin needs a name");
        return (None, w);
    }
    let mut icon = s(&v, "icon");
    if !ICONS.contains(&icon.as_str()) {
        if !icon.is_empty() {
            warn(&mut w, "icon", format!("unknown icon {icon:?}"));
        }
        icon = "plugin".into();
    }
    let settings = v.get("settings").and_then(|st| {
        let (parsed, sw) = plugin_settings::parse_settings(st);
        w.extend(sw.into_iter().map(|x| format!("settings: {x}")));
        parsed
    });
    let mut data = Vec::new();
    if let Some(obj) = v.get("data").and_then(Value::as_object) {
        for (name, d) in obj {
            let path = format!("data.{name}");
            if !plugin_settings::is_key(name) {
                warn(&mut w, &path, "a data source's name is a key");
                continue;
            }
            let Some(request) = parse_request(d, &path, &mut w) else {
                continue;
            };
            let every = d.get("every").and_then(Value::as_u64).unwrap_or(0);
            data.push(Source {
                name: name.clone(),
                request,
                every: if every == 0 { 0 } else { every.max(MIN_EVERY) },
                when: s(d, "when"),
            });
        }
    }
    let mut actions = BTreeMap::new();
    if let Some(obj) = v.get("actions").and_then(Value::as_object) {
        for (name, steps) in obj {
            let path = format!("actions.{name}");
            let steps = match steps {
                Value::Array(a) => a.clone(),
                one => vec![one.clone()],
            };
            let parsed: Vec<Step> = steps
                .iter()
                .enumerate()
                .filter_map(|(i, st)| parse_step(st, &format!("{path}[{i}]"), &mut w))
                .collect();
            actions.insert(name.clone(), parsed);
        }
    }
    let columns: Vec<Vec<Card>> = match v
        .get("page")
        .and_then(|p| p.get("columns"))
        .and_then(Value::as_array)
    {
        Some(cols) => cols
            .iter()
            .enumerate()
            .map(|(ci, col)| {
                col.as_array()
                    .map(|cards| {
                        cards
                            .iter()
                            .enumerate()
                            .map(|(i, c)| {
                                parse_card(c, &format!("page.columns[{ci}][{i}]"), &mut w)
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect(),
        None => {
            warn(
                &mut w,
                "page",
                "a plugin needs a page (\"page\": {\"columns\": [...]})",
            );
            return (None, w);
        }
    };
    for (ci, col) in columns.iter().enumerate() {
        for card in col {
            for b in &card.blocks {
                let action = match b {
                    Block::Button(b) => Some(&b.action),
                    Block::List {
                        button: Some(b), ..
                    } => Some(&b.action),
                    _ => None,
                };
                if let Some(a) = action {
                    if !actions.contains_key(a) {
                        warn(
                            &mut w,
                            &format!("page.columns[{ci}]"),
                            format!("no action {a:?}"),
                        );
                    }
                }
            }
        }
    }
    let page = PluginPage {
        id,
        name,
        version: s(&v, "version"),
        author: s(&v, "author"),
        summary: s(&v, "summary"),
        icon,
        settings,
        data,
        actions,
        columns,
    };
    (Some(page), w)
}

fn parse_request(v: &Value, path: &str, w: &mut Vec<Warning>) -> Option<Request> {
    let (method, url) = match (
        v.get("get").and_then(Value::as_str),
        v.get("post").and_then(Value::as_str),
    ) {
        (Some(u), None) => (Method::Get, u),
        (None, Some(u)) => (Method::Post, u),
        _ => {
            warn(w, path, "a request has \"get\" or \"post\" with its URL");
            return None;
        }
    };
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        warn(w, path, "a request's URL starts with http:// or https://");
        return None;
    }
    Some(Request {
        method,
        url: url.to_string(),
        body: (method == Method::Post)
            .then(|| v.get("body").cloned().unwrap_or(Value::Object(Map::new()))),
    })
}

fn parse_button(v: &Value) -> Option<Button> {
    let action = s(v, "action");
    if action.is_empty() {
        return None;
    }
    Some(Button {
        label: s(v, "label"),
        action,
        enabled: s(v, "enabled"),
        primary: v.get("primary").and_then(Value::as_bool).unwrap_or(false),
        when: s(v, "when"),
    })
}

fn parse_card(v: &Value, path: &str, w: &mut Vec<Warning>) -> Card {
    let blocks = v
        .get("blocks")
        .and_then(Value::as_array)
        .map(|bs| {
            bs.iter()
                .enumerate()
                .filter_map(|(i, b)| parse_block(b, &format!("{path}.blocks[{i}]"), w))
                .collect()
        })
        .unwrap_or_default();
    Card {
        title: s(v, "title"),
        help: s(v, "help"),
        when: s(v, "when"),
        blocks,
    }
}

fn parse_block(v: &Value, path: &str, w: &mut Vec<Warning>) -> Option<Block> {
    let when = s(v, "when");
    if let Some(text) = v.get("text").and_then(Value::as_str) {
        let tone = match v.get("tone").and_then(Value::as_str) {
            Some("muted") => Tone::Muted,
            Some("good") => Tone::Good,
            Some("warn") => Tone::Warn,
            _ => Tone::Normal,
        };
        return Some(Block::Text {
            text: text.into(),
            tone,
            when,
        });
    }
    if let Some(value) = v.get("value").and_then(Value::as_str) {
        return Some(Block::Value {
            label: s(v, "label"),
            value: value.into(),
            copy: v.get("copy").and_then(Value::as_bool).unwrap_or(false),
            when,
        });
    }
    for kind in ["field", "choice"] {
        if let Some(bind) = v.get(kind).and_then(Value::as_str) {
            if !is_bind(bind) {
                warn(
                    w,
                    path,
                    format!("{bind:?}: a {kind} binds to page.<key> or settings.<key>"),
                );
                return None;
            }
            return Some(if kind == "field" {
                Block::Field {
                    bind: bind.into(),
                    label: s(v, "label"),
                    hint: s(v, "hint"),
                    secret: v.get("secret").and_then(Value::as_bool).unwrap_or(false),
                    when,
                }
            } else {
                Block::Choice {
                    bind: bind.into(),
                    label: s(v, "label"),
                    options: s(v, "options"),
                    value: s(v, "option_value"),
                    text: s(v, "option_text"),
                    when,
                }
            });
        }
    }
    if let Some(items) = v.get("list").and_then(Value::as_str) {
        return Some(Block::List {
            items: items.into(),
            filter: s(v, "filter"),
            title: s(v, "title"),
            detail: s(v, "detail"),
            button: v.get("button").and_then(parse_button),
            empty: s(v, "empty"),
            when,
        });
    }
    if let Some(b) = v.get("button") {
        return match parse_button(b) {
            Some(mut b) => {
                if b.when.is_empty() {
                    b.when = when;
                }
                Some(Block::Button(b))
            }
            None => {
                warn(w, path, "a button needs an action");
                None
            }
        };
    }
    warn(w, path, "unknown block");
    None
}

fn parse_step(v: &Value, path: &str, w: &mut Vec<Warning>) -> Option<Step> {
    if v.get("get").is_some() || v.get("post").is_some() {
        let request = parse_request(v, path, w)?;
        let save = s(v, "save");
        if !save.is_empty() && !save.starts_with("page.") {
            warn(w, path, "a request saves its answer to page.<key>");
            return None;
        }
        return Some(Step::Request { request, save });
    }
    if let Some(when) = v.get("require").and_then(Value::as_str) {
        return Some(Step::Require {
            when: when.into(),
            otherwise: s(v, "otherwise"),
        });
    }
    if let Some(p) = v.get("set").and_then(Value::as_str) {
        if !is_bind(p) {
            warn(
                w,
                path,
                format!("{p:?}: set takes page.<key> or settings.<key>"),
            );
            return None;
        }
        return Some(Step::Set {
            path: p.into(),
            to: v.get("to").cloned().unwrap_or(Value::String(String::new())),
        });
    }
    if let Some(version) = v.get("play").and_then(Value::as_str) {
        let args = v
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        return Some(Step::Play {
            version: version.into(),
            args,
        });
    }
    if let Some(text) = v.get("copy").and_then(Value::as_str) {
        return Some(Step::Copy { text: text.into() });
    }
    if let Some(r) = v.get("refresh") {
        let sources = match r {
            Value::String(one) => vec![one.clone()],
            Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect(),
            _ => Vec::new(),
        };
        return Some(Step::Refresh { sources });
    }
    if let Some(server) = v.get("server") {
        if !(server.is_object() || server.is_null()) {
            warn(
                w,
                path,
                "server takes {kind, address, key, discord_id, display_name, password, name}, or null for EchoVRCE",
            );
            return None;
        }
        let kind = s(server, "kind");
        if !matches!(kind.as_str(), "" | "nakama" | "echorelay") {
            warn(
                w,
                path,
                format!("{kind:?}: a server's kind is nakama or echorelay"),
            );
            return None;
        }
        return Some(Step::Server {
            kind,
            name: s(server, "name"),
            address: s(server, "address"),
            key: s(server, "key"),
            discord_id: s(server, "discord_id"),
            display_name: s(server, "display_name"),
            password: s(server, "password"),
        });
    }
    warn(w, path, "unknown step");
    None
}

// ---- templates ----

/// The value at `path` (`a.b.0.c`) in `ctx`.
pub fn lookup<'a>(ctx: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = ctx;
    for part in path.trim().split('.') {
        cur = match cur {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// The first of `a|b|c` that is set and not empty; `'text'` is a literal.
fn resolve<'a>(ctx: &'a Value, expr: &str) -> Option<std::borrow::Cow<'a, Value>> {
    for alt in expr.split('|') {
        let alt = alt.trim();
        if let Some(lit) = alt.strip_prefix('\'').and_then(|a| a.strip_suffix('\'')) {
            return Some(std::borrow::Cow::Owned(Value::String(lit.into())));
        }
        match lookup(ctx, alt) {
            None | Some(Value::Null) => continue,
            Some(Value::String(s)) if s.is_empty() => continue,
            Some(v) => return Some(std::borrow::Cow::Borrowed(v)),
        }
    }
    None
}

/// A value as text: strings as they are, numbers and booleans written out, nothing for
/// null, JSON for the rest.
pub fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Fills a template: `{path}` (with `|` fallbacks and `'literal'`s) from `ctx`; `{{` and
/// `}}` are braces.
pub fn render(template: &str, ctx: &Value) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let Some(after) = tail.strip_prefix('}') {
            out.push('}');
            rest = after;
            continue;
        }
        match tail.find('}') {
            Some(end) => {
                if let Some(v) = resolve(ctx, &tail[1..end]) {
                    out.push_str(&text_of(&v));
                }
                rest = &tail[end + 1..];
            }
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Fills a template, keeping the value's type when the template is one `{path}` and
/// nothing else (so a body can send numbers, booleans, objects).
pub fn render_value(template: &str, ctx: &Value) -> Value {
    let t = template.trim();
    if t.starts_with('{')
        && t.ends_with('}')
        && !t.starts_with("{{")
        && t[1..t.len() - 1].find(['{', '}']).is_none()
    {
        return resolve(ctx, &t[1..t.len() - 1])
            .map(|v| v.into_owned())
            .unwrap_or(Value::Null);
    }
    Value::String(render(template, ctx))
}

/// A JSON body with every string filled in.
pub fn render_body(body: &Value, ctx: &Value) -> Value {
    match body {
        Value::String(t) => render_value(t, ctx),
        Value::Array(a) => Value::Array(a.iter().map(|x| render_body(x, ctx)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), render_body(x, ctx)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Whether a condition holds: empty holds; otherwise the filled-in template is not empty,
/// `false`, `0` or `null`. `a == b` and `a != b` compare the two filled-in sides; a leading
/// `!` negates.
pub fn truthy(cond: &str, ctx: &Value) -> bool {
    let c = cond.trim();
    if c.is_empty() {
        return true;
    }
    if let Some(rest) = c.strip_prefix('!') {
        return !truthy(rest, ctx);
    }
    for (op, eq) in [("!=", false), ("==", true)] {
        if let Some(i) = c.find(op) {
            let (a, b) = (render(c[..i].trim(), ctx), render(c[i + 2..].trim(), ctx));
            return (a == b) == eq;
        }
    }
    let r = render(c, ctx);
    !matches!(r.trim(), "" | "false" | "0" | "null")
}

/// The items of the list at `path` in `ctx` (none if it isn't one).
pub fn items(ctx: &Value, path: &str) -> Vec<Value> {
    match lookup(ctx, path) {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

/// `ctx` with `item` set (for a list's rows).
pub fn with_item(ctx: &Value, item: &Value) -> Value {
    let mut c = ctx.clone();
    if let Value::Object(m) = &mut c {
        m.insert("item".into(), item.clone());
    }
    c
}

/// Sets `path` (`page.a.b`) in `ctx` to `value`, making objects on the way.
pub fn set_path(ctx: &mut Value, path: &str, value: Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = ctx;
    for (i, part) in parts.iter().enumerate() {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let m = cur.as_object_mut().expect("an object");
        if i + 1 == parts.len() {
            m.insert((*part).into(), value);
            return;
        }
        cur = m.entry(*part).or_insert_with(|| Value::Object(Map::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Value {
        json!({
            "settings": {"name": "Alice", "server": ""},
            "launcher": {"relay": {"server": "168.119.2.92:6800"}},
            "page": {"build": "halloween"},
            "data": {"matches": {"matches": [
                {"id": "a", "build": "halloween", "players": 2, "limit": 16, "joinable": true},
                {"id": "b", "build": "summer", "players": 16, "limit": 16, "joinable": false}
            ]}}
        })
    }

    #[test]
    fn templates_fill_paths_fallbacks_and_literals() {
        let c = ctx();
        assert_eq!(render("Hi {settings.name}!", &c), "Hi Alice!");
        assert_eq!(
            render("http://{settings.server|launcher.relay.server}/api", &c),
            "http://168.119.2.92:6800/api"
        );
        assert_eq!(render("{settings.nope|'none'}", &c), "none");
        assert_eq!(
            render(
                "{data.matches.matches.0.players}/{data.matches.matches.0.limit}",
                &c
            ),
            "2/16"
        );
        assert_eq!(render("{{literal}} {missing}", &c), "{literal} ");
        assert_eq!(
            render_value("{data.matches.matches.0.players}", &c),
            json!(2)
        );
        assert_eq!(render_value("{page.build}-x", &c), json!("halloween-x"));
        assert_eq!(
            render_body(
                &json!({"id": "{page.build}", "n": 3, "list": ["{settings.name}"]}),
                &c
            ),
            json!({"id": "halloween", "n": 3, "list": ["Alice"]})
        );
    }

    #[test]
    fn conditions() {
        let c = ctx();
        assert!(truthy("", &c));
        assert!(truthy("{settings.name}", &c));
        assert!(!truthy("{settings.server}", &c));
        assert!(truthy("!{settings.server}", &c));
        assert!(truthy("{page.build} == halloween", &c));
        assert!(!truthy("{page.build} != halloween", &c));
        let row = with_item(&c, &items(&c, "data.matches.matches")[1]);
        assert!(!truthy("{item.joinable}", &row));
        assert!(truthy("{item.build} == summer", &row));
    }

    #[test]
    fn set_path_makes_objects() {
        let mut c = json!({});
        set_path(&mut c, "page.result.ok", json!(true));
        assert_eq!(lookup(&c, "page.result.ok"), Some(&json!(true)));
    }

    #[test]
    fn parses_a_page_and_warns_about_mistakes() {
        let text = r#"{
          "schema": 1, "id": "event-lobbies", "name": "Event lobbies", "icon": "calendar",
          "data": {"matches": {"get": "http://{settings.server}/api/matches", "every": 1},
                   "bad": {"get": "ftp://x"}},
          "actions": {"join": [
              {"post": "http://x/api/matches/join", "body": {"id": "{item.id}"}, "save": "page.join"},
              {"require": "{page.join.ok}", "otherwise": "{page.join.message}"},
              {"play": "{item.build}"},
              {"nope": 1}]},
          "page": {"columns": [[{"title": "Matches", "blocks": [
              {"list": "data.matches.matches", "title": "{item.id}", "button": {"label": "JOIN", "action": "join"}},
              {"field": "settings.name", "label": "Name"},
              {"field": "data.x"},
              {"button": {"label": "X", "action": "missing"}}]}]]}
        }"#;
        let (p, w) = parse(text);
        let p = p.unwrap();
        assert_eq!(p.icon, "calendar");
        assert_eq!(p.data.len(), 1);
        assert_eq!(p.data[0].every, MIN_EVERY);
        assert_eq!(p.actions["join"].len(), 3);
        assert_eq!(p.columns[0][0].blocks.len(), 3);
        let says = |s: &str| w.iter().any(|x| x.contains(s));
        assert!(says("http:// or https://"), "{w:?}");
        assert!(says("unknown step"), "{w:?}");
        assert!(says("binds to page"), "{w:?}");
        assert!(says("no action \"missing\""), "{w:?}");
    }

    #[test]
    fn parses_the_server_step() {
        let (p, w) = parse(
            r#"{"id": "s", "name": "S", "page": {"columns": []}, "actions": {
              "use": [{"server": {"name": "{settings.name}", "address": "{settings.address}",
                                  "key": "{settings.key}", "discord_id": "{settings.id}",
                                  "password": "{settings.password}"}}],
              "back": [{"server": null}],
              "relay": [{"server": {"kind": "echorelay", "address": "h:1", "display_name": "n"}}],
              "odd": [{"server": {"kind": "steam"}}],
              "bad": [{"server": "echovrce"}]}}"#,
        );
        let p = p.unwrap();
        assert_eq!(
            p.actions["use"],
            [Step::Server {
                kind: String::new(),
                name: "{settings.name}".into(),
                address: "{settings.address}".into(),
                key: "{settings.key}".into(),
                discord_id: "{settings.id}".into(),
                display_name: String::new(),
                password: "{settings.password}".into(),
            }]
        );
        assert!(
            matches!(&p.actions["back"][..], [Step::Server { address, .. }] if address.is_empty())
        );
        assert!(
            matches!(&p.actions["relay"][..], [Step::Server { kind, display_name, .. }] if kind == "echorelay" && display_name == "n")
        );
        assert!(p.actions["odd"].is_empty() && p.actions["bad"].is_empty());
        assert!(w.iter().any(|x| x.contains("nakama or echorelay")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("null for EchoVRCE")), "{w:?}");
    }

    /// The Game server plugin reads without a warning, and says it needs a newer launcher
    /// only where the launcher tells plugins nothing about the game's server.
    #[test]
    fn game_server_plugin() {
        let (p, w) = parse(include_str!(
            "../../../docs/plugins/game-server/plugin.json"
        ));
        assert!(w.is_empty(), "{w:?}");
        let p = p.unwrap();
        assert!(matches!(
            &p.actions["relay"][..],
            [Step::Server { kind, .. }] if kind == "echorelay"
        ));
        assert!(matches!(
            &p.actions["nakama"][..],
            [Step::Require { .. }, Step::Server { .. }]
        ));
        let new = serde_json::json!({"launcher": {"features": {"echorelay": true},
            "server": {"address": "", "kind": ""}, "relay": {"server": "h:1", "name": "N"}}});
        let old = serde_json::json!({"launcher": {"server": {"address": ""}}});
        assert!(!truthy("!{launcher.features.echorelay}", &new));
        assert!(truthy("!{launcher.features.echorelay}", &old));
        assert!(truthy("!{launcher.server.address}", &new));
        // An empty field falls back to the classic lobbies account.
        assert_eq!(
            render("{settings.relay_name|launcher.relay.name}", &new),
            "N"
        );
    }

    #[test]
    fn refuses_a_plugin_without_id_name_or_page() {
        assert!(parse(r#"{"name": "x", "page": {"columns": []}}"#)
            .0
            .is_none());
        assert!(parse(r#"{"id": "x", "page": {"columns": []}}"#).0.is_none());
        assert!(parse(r#"{"id": "x", "name": "X"}"#).0.is_none());
        assert!(
            parse(r#"{"schema": 9, "id": "x", "name": "X", "page": {"columns": []}}"#)
                .0
                .is_none()
        );
    }

    /// The Event lobbies page's conditions: a message per kind of failure, the plain error
    /// for a launcher that doesn't say the kind, nothing when the list came, and the relay's
    /// "log in once" answer turned into the page's own words.
    #[test]
    fn event_lobbies_conditions() {
        let page: Value = serde_json::from_str(include_str!(
            "../../../docs/plugins/event-lobbies/plugin.json"
        ))
        .unwrap();
        let blocks = page["page"]["columns"][0][0]["blocks"].as_array().unwrap();
        let shown = |data: Value| -> Vec<String> {
            let c = json!({ "data": { "matches": data }, "launcher": {}, "settings": {} });
            blocks
                .iter()
                .filter(|b| b.get("text").is_some() && b.get("when").is_some())
                .filter(|b| truthy(b["when"].as_str().unwrap(), &c))
                .map(|b| render(b["text"].as_str().unwrap(), &c))
                .filter(|t| !t.starts_with("Install an event build"))
                .collect()
        };
        assert!(shown(json!({"matches": []})).is_empty());
        let down = shown(json!({"error": "GET …: no route", "failed": "unreachable"}));
        assert_eq!(down.len(), 1);
        assert!(down[0].starts_with("Can't reach"));
        let missing = shown(json!({"error": "GET …: 404", "failed": "missing"}));
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("matches API"));
        assert_eq!(
            shown(json!({"error": "GET …: 500", "failed": "server"})).len(),
            1
        );
        // A launcher before 0.11.5 says only the error.
        let old = shown(json!({"error": "GET …: no route"}));
        assert_eq!(
            old,
            ["Couldn't load the matches list (GET …: no route). Trying again every 10 seconds."]
        );

        for (action, words) in [
            ("request", "request a game server"),
            ("join", "join a match"),
        ] {
            let steps = page["actions"][action].as_array().unwrap();
            let check = steps
                .iter()
                .find(|s| s["require"].as_str().is_some_and(|r| r.contains("!=")))
                .unwrap();
            let relay = format!("Log in to the game on this server once first, then {words}.");
            let c = json!({ "page": { "answer": { "ok": false, "message": relay } } });
            assert!(!truthy(check["require"].as_str().unwrap(), &c), "{action}");
            assert!(check["otherwise"]
                .as_str()
                .unwrap()
                .starts_with("Start an event build once"));
            let c = json!({ "page": { "answer": { "ok": true, "message": "Requested." } } });
            assert!(truthy(check["require"].as_str().unwrap(), &c), "{action}");
        }
    }
}
