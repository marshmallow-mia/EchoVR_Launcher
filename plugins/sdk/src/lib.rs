//! Writing Echo VR launcher plugins in Rust: the bindings of `wit/launcher.wit` and a page
//! builder.
//!
//! A plugin implements [`Plugin`] and exports it:
//!
//! ```ignore
//! struct Example;
//! impl echovr_plugin::Plugin for Example { … }
//! echovr_plugin::export_plugin!(Example);
//! ```
//!
//! Its state lives in a `static` (a plugin is single-threaded): [`State`] wraps one.

pub mod bindings {
    wit_bindgen::generate!({
        world: "plugin",
        path: "../../wit",
        pub_export_macro: true,
        default_bindings_module: "echovr_plugin::bindings",
    });
}

use std::cell::RefCell;

pub use bindings::echovr::launcher::game;
pub use bindings::echovr::launcher::host;
pub use bindings::echovr::launcher::net;
pub use bindings::echovr::launcher::server_host;
pub use bindings::echovr::launcher::types::{
    Button, Card, Cell, Chip, Choice, Event, Field, Level, LogView, Node, Page, Progress, Response,
    Row, Table, Tabs, Text, Toggle, Tone, ValueRow,
};
pub use serde_json::{json, Value};

/// Exports `$t` (a type implementing [`Plugin`]) as the plugin.
#[macro_export]
macro_rules! export_plugin {
    ($t:ty) => {
        struct __EchoVrPlugin;
        impl $crate::bindings::Guest for __EchoVrPlugin {
            fn init(info: String) -> Result<(), String> {
                <$t as $crate::Plugin>::init(&$crate::parse(&info))
            }
            fn render() -> $crate::Page {
                let mut ui = $crate::Ui::new();
                <$t as $crate::Plugin>::render(&mut ui);
                ui.finish()
            }
            fn on_event(e: $crate::Event) {
                <$t as $crate::Plugin>::on_event(e)
            }
            fn on_timer(id: u32) {
                <$t as $crate::Plugin>::on_timer(id)
            }
        }
        $crate::bindings::export!(__EchoVrPlugin with_types_in $crate::bindings);
    };
}

/// A plugin.
pub trait Plugin {
    /// Once, with what the launcher tells plugins (`host::launcher_info`).
    fn init(info: &Value) -> Result<(), String>;
    /// The page as it is now.
    fn render(ui: &mut Ui);
    fn on_event(e: Event);
    fn on_timer(_id: u32) {}
}

/// A plugin's state: one value, borrowed for a moment at a time.
pub struct State<T>(RefCell<Option<T>>);

// A plugin runs on one thread: the launcher never calls it from two at once.
unsafe impl<T> Sync for State<T> {}

impl<T: Default> State<T> {
    pub const fn new() -> Self {
        State(RefCell::new(None))
    }

    /// `f` with the state (its default the first time).
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut s = self.0.borrow_mut();
        f(s.get_or_insert_with(T::default))
    }
}

impl<T: Default> Default for State<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Pure: JSON text as a value (`null` when it isn't JSON).
pub fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// Pure: the text at `path` (`a.b.0.c`) in `v`, empty when there is none. Numbers and
/// booleans as written.
pub fn text_at(v: &Value, path: &str) -> String {
    match at(v, path) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Pure: the value at `path` (`a.b.0.c`) in `v`.
pub fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .filter(|p| !p.is_empty())
        .try_fold(v, |v, p| match v {
            Value::Object(m) => m.get(p),
            Value::Array(a) => a.get(p.parse::<usize>().ok()?),
            _ => None,
        })
}

/// Pure: whether `v` counts as set: not null, false, 0, "" or empty.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.trim().is_empty() && s != "false" && s != "0",
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// The plugin's settings as a map.
pub fn settings() -> std::collections::BTreeMap<String, String> {
    host::settings().into_iter().collect()
}

/// One setting (empty when there is none).
pub fn setting(key: &str) -> String {
    host::settings()
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .unwrap_or_default()
}

pub fn log(message: &str) {
    host::log(Level::Info, message);
}

pub fn warn(message: &str) {
    host::log(Level::Warn, message);
}

pub fn notify(text: &str) {
    host::notify(text, Tone::Normal);
}

/// A GET request; its answer comes as `Event::NetDone(ticket, …)`.
pub fn get(url: &str) -> Result<u32, String> {
    net::fetch(&net::Request {
        method: net::Method::Get,
        url: url.into(),
        body: None,
        relay_account: false,
    })
}

/// A POST of `body` (JSON); `relay_account`: the launcher fills `{relay.name}` and
/// `{relay.password}` in for the classic lobbies server.
pub fn post(url: &str, body: &Value, relay_account: bool) -> Result<u32, String> {
    net::fetch(&net::Request {
        method: net::Method::Post,
        url: url.into(),
        body: Some(body.to_string()),
        relay_account,
    })
}

/// A server-host operation that answers at once, its answer parsed.
pub fn call(op: &str, args: &Value) -> Result<Value, String> {
    server_host::call(op, &args.to_string()).map(|t| parse(&t))
}

/// One that takes a while: its ticket.
pub fn start(op: &str, args: &Value) -> Result<u32, String> {
    server_host::start(op, &args.to_string())
}

// ---- building a page ----

pub fn button(id: &str, label: &str) -> Button {
    Button {
        id: id.into(),
        label: label.into(),
        primary: false,
        danger: false,
        enabled: true,
        icon: String::new(),
        hint: String::new(),
    }
}

pub trait ButtonExt {
    fn primary(self) -> Self;
    fn danger(self) -> Self;
    fn enabled(self, on: bool) -> Self;
    /// A small square with icon `icon` (its label shows on hover).
    fn icon(self, icon: &str) -> Self;
    fn hint(self, hint: &str) -> Self;
}

impl ButtonExt for Button {
    fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    fn icon(mut self, icon: &str) -> Self {
        self.icon = icon.into();
        self
    }
    fn hint(mut self, hint: &str) -> Self {
        self.hint = hint.into();
        self
    }
}

pub fn cell(text: impl Into<String>) -> Cell {
    Cell {
        text: text.into(),
        tone: Tone::Normal,
        chip: false,
    }
}

pub fn chip_cell(text: impl Into<String>, tone: Tone) -> Cell {
    Cell {
        text: text.into(),
        tone,
        chip: true,
    }
}

/// A page being built: cards in up to two columns.
pub struct Ui {
    nodes: Vec<Node>,
    columns: Vec<Vec<u32>>,
}

/// A container being filled (a card's or a tab's children).
pub struct Kids<'a> {
    nodes: &'a mut Vec<Node>,
    kids: Vec<u32>,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    pub fn new() -> Self {
        Ui {
            nodes: Vec::new(),
            columns: Vec::new(),
        }
    }

    /// A card in column `col` (0: the wide one, 1: the right-hand panel).
    pub fn card(&mut self, col: usize, title: &str, help: &str, f: impl FnOnce(&mut Kids)) {
        let mut k = Kids {
            nodes: &mut self.nodes,
            kids: Vec::new(),
        };
        f(&mut k);
        let children = k.kids;
        let i = push(
            &mut self.nodes,
            Node::Card(Card {
                title: title.into(),
                help: help.into(),
                children,
            }),
        );
        while self.columns.len() <= col.min(1) {
            self.columns.push(Vec::new());
        }
        self.columns[col.min(1)].push(i);
    }

    pub fn finish(self) -> Page {
        Page {
            nodes: self.nodes,
            columns: self.columns,
        }
    }
}

fn push(nodes: &mut Vec<Node>, n: Node) -> u32 {
    nodes.push(n);
    (nodes.len() - 1) as u32
}

impl Kids<'_> {
    fn add(&mut self, n: Node) -> &mut Self {
        let i = push(self.nodes, n);
        self.kids.push(i);
        self
    }

    pub fn text(&mut self, text: impl Into<String>, tone: Tone) -> &mut Self {
        self.add(Node::Text(Text {
            text: text.into(),
            tone,
        }))
    }

    pub fn muted(&mut self, text: impl Into<String>) -> &mut Self {
        self.text(text, Tone::Muted)
    }

    pub fn warn(&mut self, text: impl Into<String>) -> &mut Self {
        self.text(text, Tone::Warn)
    }

    pub fn value(&mut self, label: &str, value: impl Into<String>, copy: bool) -> &mut Self {
        self.add(Node::Value(ValueRow {
            label: label.into(),
            value: value.into(),
            copy,
        }))
    }

    pub fn field(&mut self, id: &str, label: &str, value: &str, hint: &str) -> &mut Self {
        self.add(Node::Field(Field {
            id: id.into(),
            label: label.into(),
            hint: hint.into(),
            value: value.into(),
            secret: false,
            enabled: true,
        }))
    }

    /// A field whose value stays with the launcher (a password).
    pub fn secret(&mut self, id: &str, label: &str, hint: &str) -> &mut Self {
        self.add(Node::Field(Field {
            id: id.into(),
            label: label.into(),
            hint: hint.into(),
            value: String::new(),
            secret: true,
            enabled: true,
        }))
    }

    pub fn choice(
        &mut self,
        id: &str,
        label: &str,
        options: Vec<(String, String)>,
        selected: &str,
    ) -> &mut Self {
        self.add(Node::Choice(Choice {
            id: id.into(),
            label: label.into(),
            options,
            selected: selected.into(),
            enabled: true,
        }))
    }

    pub fn toggle(&mut self, id: &str, label: &str, on: bool) -> &mut Self {
        self.add(Node::Toggle(Toggle {
            id: id.into(),
            label: label.into(),
            on,
            enabled: true,
        }))
    }

    pub fn button(&mut self, b: Button) -> &mut Self {
        self.add(Node::Button(b))
    }

    pub fn buttons(&mut self, b: Vec<Button>) -> &mut Self {
        self.add(Node::Buttons(b))
    }

    pub fn table(&mut self, id: &str, rows: Vec<Row>, empty: &str) -> &mut Self {
        self.add(Node::Table(Table {
            id: id.into(),
            rows,
            empty: empty.into(),
        }))
    }

    pub fn chip(&mut self, text: impl Into<String>, tone: Tone) -> &mut Self {
        self.add(Node::Chip(Chip {
            text: text.into(),
            tone,
        }))
    }

    pub fn progress(&mut self, label: &str, fraction: f32) -> &mut Self {
        self.add(Node::Progress(Progress {
            label: label.into(),
            fraction,
        }))
    }

    pub fn log_view(
        &mut self,
        log: u32,
        filter: &str,
        follow: bool,
        noise: bool,
        lines: u32,
    ) -> &mut Self {
        self.add(Node::LogView(LogView {
            log,
            filter: filter.into(),
            follow,
            noise,
            lines,
        }))
    }

    /// Tabs; `f` fills what the selected one shows.
    pub fn tabs(
        &mut self,
        id: &str,
        labels: &[&str],
        selected: u32,
        f: impl FnOnce(&mut Kids),
    ) -> &mut Self {
        let mut k = Kids {
            nodes: self.nodes,
            kids: Vec::new(),
        };
        f(&mut k);
        let children = k.kids;
        self.add(Node::Tabs(Tabs {
            id: id.into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            selected,
            children,
        }))
    }

    /// A section of the plugin's settings as a form (`*`: all).
    pub fn form(&mut self, section: &str) -> &mut Self {
        self.add(Node::Form(section.into()))
    }

    pub fn presets(&mut self) -> &mut Self {
        self.add(Node::Presets)
    }
}
