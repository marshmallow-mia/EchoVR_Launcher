//! Plugin settings from a description: a plugin says what can be changed (each setting's
//! type, label, help, range or choices, and how its control looks) and where the values
//! go, and the Mods page draws the controls. See docs/plugins/settings.md.
//!
//! A description comes, in this order, from the plugin's catalogue entry (`settings`),
//! from `<Stem>.plugin.json` beside its DLL, or from the launcher itself (for a plugin
//! whose release doesn't ship one yet). It is only a description: it runs nothing, and the
//! only file it may name is one in `plugins/`.
//!
//! Values are text throughout, as nEVR hands plugins their arguments. They go into nEVR's
//! arguments for the plugin (applied at the next start), or into a settings file of the
//! plugin's own in `plugins/` (`Key = value` lines, or a flat JSON object), which a plugin
//! may re-read while the game runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};

use super::mods::{self, ModEntry, Plugin};
use super::store::InstalledVersion;

/// The description files' format version this launcher reads.
pub const SCHEMA: u64 = 1;
/// Beside a plugin's DLL: `<Stem>.plugin.json`.
pub const SIDECAR_SUFFIX: &str = ".plugin.json";
/// How many settings a plugin may show on its row in the list.
pub const ROW_MAX: usize = 2;

// ---- the description ----

/// A plugin's settings: where they are kept, and what they are.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub store: Store,
    /// Settings shown on the plugin's row (bools and choices, at most [`ROW_MAX`]).
    pub row: Vec<String>,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Section {
    pub title: String,
    pub help: String,
    /// Shown while the pointer is on its title.
    pub tooltip: String,
    pub fields: Vec<Field>,
}

/// Where values are kept.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Store {
    /// nEVR's arguments for the plugin (the launcher's `launcher-mods.json`, then
    /// `config.yaml`): read by the plugin when it starts.
    #[default]
    Args,
    /// A file of the plugin's own in `plugins/`.
    File {
        file: String,
        format: FileFormat,
        /// The plugin re-reads it while the game runs.
        live: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// `Key = value` lines (comments with `#` or `;`), as an INI without sections.
    KeyValue,
    /// One flat JSON object.
    Json,
}

impl Store {
    /// Changes apply while the game runs.
    pub fn live(&self) -> bool {
        matches!(self, Store::File { live: true, .. })
    }
}

/// One setting (or a button, a note, a link).
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// Its key in the store (empty for an action, a note or a link).
    pub key: String,
    pub kind: Kind,
    pub label: String,
    /// Under its control.
    pub help: String,
    /// Shown while the pointer is on its label or control (empty: its help).
    pub tooltip: String,
    /// Its default as text (empty: none).
    pub default: String,
    /// Folded away under "Show advanced".
    pub advanced: bool,
    /// Shown only while these settings have these values (as text).
    pub visible_if: Vec<(String, String)>,
    /// Usable only while these settings have these values.
    pub enabled_if: Vec<(String, String)>,
    /// Applies at the next start, though the store is live (or the other way round).
    pub restart: Option<bool>,
    pub style: Style,
    /// Its own store instead of the description's.
    pub store: Option<Store>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// On or off, kept as `on` / `off`.
    Bool { on: String, off: String },
    Text {
        placeholder: String,
        min_len: usize,
        max_len: usize,
        format: Option<TextFormat>,
    },
    /// Text shown as dots.
    Secret { placeholder: String },
    /// One of a list.
    Choice { choices: Vec<Choice> },
    /// Some of a list, kept comma-separated in the list's order.
    Multi { choices: Vec<Choice> },
    Number {
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
        integer: bool,
        unit: String,
    },
    /// `#rrggbb` (`#rrggbbaa` with alpha).
    Color { alpha: bool },
    /// A key with modifiers, as `Ctrl+Alt+C`.
    Key { modifiers: bool },
    /// A button that writes these values once.
    Action {
        sets: Vec<(String, String)>,
        danger: bool,
        /// Asked before (empty: not asked).
        confirm: String,
    },
    /// A grey paragraph.
    Note { text: String },
    /// An underlined link (https).
    Link { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    /// Beside it in a dropdown.
    pub help: String,
    /// Shown while the pointer is on it (empty: its help, else the setting's).
    pub tooltip: String,
}

impl Choice {
    /// What shows while the pointer is on it, as part of `f`.
    pub fn hover<'a>(&'a self, f: &'a Field) -> &'a str {
        [&self.tooltip, &self.help]
            .into_iter()
            .find(|t| !t.is_empty())
            .map_or_else(|| f.hover(), String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFormat {
    /// `scheme://…` without spaces.
    Url,
    Hex,
    Digits,
    /// Letters, digits, space, `.`, `-`, `_`.
    Name,
}

/// How a control looks: plugins pick among these, the launcher draws them in its own
/// design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    /// The type's own.
    #[default]
    Default,
    // bool
    Check,
    Switch,
    // text
    Field,
    Area,
    // choice
    Dropdown,
    Segmented,
    Radio,
    // multi
    Checks,
    Chips,
    // number
    Slider,
    Stepper,
}

impl Field {
    /// What shows while the pointer is on it: its tooltip, else its help.
    pub fn hover(&self) -> &str {
        if self.tooltip.is_empty() {
            &self.help
        } else {
            &self.tooltip
        }
    }

    /// It holds a value (an action, a note or a link doesn't).
    pub fn has_value(&self) -> bool {
        !matches!(
            self.kind,
            Kind::Action { .. } | Kind::Note { .. } | Kind::Link { .. }
        )
    }

    /// Its store: its own, else the description's.
    pub fn store<'a>(&'a self, s: &'a Settings) -> &'a Store {
        self.store.as_ref().unwrap_or(&s.store)
    }

    /// Whether a change applies while the game runs.
    pub fn live(&self, s: &Settings) -> bool {
        match self.restart {
            Some(restart) => !restart,
            None => self.store(s).live(),
        }
    }

    /// `input` as this setting keeps it, or why it can't be.
    pub fn check(&self, input: &str) -> std::result::Result<String, String> {
        let t = input.trim();
        match &self.kind {
            Kind::Bool { on, off } => match t.to_ascii_lowercase().as_str() {
                _ if t == on => Ok(on.clone()),
                _ if t == off => Ok(off.clone()),
                "true" | "1" | "on" | "yes" => Ok(on.clone()),
                "false" | "0" | "off" | "no" | "" => Ok(off.clone()),
                _ => Err(format!("{t} is neither on nor off")),
            },
            Kind::Text {
                min_len,
                max_len,
                format,
                ..
            } => {
                let n = t.chars().count();
                if n < *min_len {
                    return Err(format!("At least {min_len} characters"));
                }
                if *max_len > 0 && n > *max_len {
                    return Err(format!("At most {max_len} characters"));
                }
                if !t.is_empty() {
                    if let Some(f) = format {
                        check_format(*f, t)?;
                    }
                }
                Ok(t.to_string())
            }
            Kind::Secret { .. } => Ok(input.to_string()),
            Kind::Choice { choices } => choices
                .iter()
                .find(|c| c.value == t)
                .map(|c| c.value.clone())
                .ok_or_else(|| format!("{t} isn't one of the choices")),
            Kind::Multi { choices } => {
                let picked: Vec<&str> = t
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if let Some(bad) = picked
                    .iter()
                    .find(|p| !choices.iter().any(|c| c.value == **p))
                {
                    return Err(format!("{bad} isn't one of the choices"));
                }
                Ok(choices
                    .iter()
                    .filter(|c| picked.contains(&c.value.as_str()))
                    .map(|c| c.value.as_str())
                    .collect::<Vec<_>>()
                    .join(","))
            }
            Kind::Number {
                min,
                max,
                step,
                integer,
                ..
            } => {
                let n: f64 = t.parse().map_err(|_| format!("{t} isn't a number"))?;
                if !n.is_finite() {
                    return Err(format!("{t} isn't a number"));
                }
                if min.is_some_and(|m| n < m - 1e-9) || max.is_some_and(|m| n > m + 1e-9) {
                    return Err(match (min, max) {
                        (Some(a), Some(b)) => format!("From {} to {}", num(*a), num(*b)),
                        (Some(a), None) => format!("At least {}", num(*a)),
                        (None, Some(b)) => format!("At most {}", num(*b)),
                        (None, None) => unreachable!(),
                    });
                }
                Ok(snap(n, *min, *max, *step, *integer))
            }
            Kind::Color { alpha } => {
                let hex = t.strip_prefix('#').unwrap_or(t);
                let ok_len = hex.len() == 6 || (*alpha && hex.len() == 8);
                if !ok_len || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(if *alpha {
                        "A colour as #rrggbb or #rrggbbaa".into()
                    } else {
                        "A colour as #rrggbb".into()
                    });
                }
                Ok(format!("#{}", hex.to_ascii_lowercase()))
            }
            Kind::Key { modifiers } => check_key(t, *modifiers),
            Kind::Action { .. } | Kind::Note { .. } | Kind::Link { .. } => {
                Err("This holds no value".into())
            }
        }
    }
}

fn check_format(f: TextFormat, t: &str) -> std::result::Result<(), String> {
    let ok = match f {
        TextFormat::Url => {
            t.split_once("://").is_some_and(|(scheme, rest)| {
                !scheme.is_empty()
                    && scheme
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '+')
                    && !rest.is_empty()
            }) && !t.chars().any(char::is_whitespace)
        }
        TextFormat::Hex => t.chars().all(|c| c.is_ascii_hexdigit()),
        TextFormat::Digits => t.chars().all(|c| c.is_ascii_digit()),
        TextFormat::Name => t
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_')),
    };
    if ok {
        Ok(())
    } else {
        Err(match f {
            TextFormat::Url => "An address like https://example.com".into(),
            TextFormat::Hex => "Only 0-9 and a-f".into(),
            TextFormat::Digits => "Only digits".into(),
            TextFormat::Name => "Letters, digits, spaces, . - _".into(),
        })
    }
}

/// The modifiers a key setting takes, as written.
pub const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];

fn check_key(t: &str, modifiers: bool) -> std::result::Result<String, String> {
    if t.is_empty() {
        return Ok(String::new());
    }
    let parts: Vec<&str> = t.split('+').map(str::trim).collect();
    let (key, mods) = parts.split_last().ok_or("Press a key")?;
    if !modifiers && !mods.is_empty() {
        return Err("A single key, without Ctrl, Alt or Shift".into());
    }
    let mut out = Vec::new();
    for m in MODIFIERS {
        if mods.iter().any(|x| x.eq_ignore_ascii_case(m)) {
            out.push(m.to_string());
        }
    }
    if out.len() != mods.len() {
        return Err("Modifiers are Ctrl, Alt, Shift and Win".into());
    }
    if key.is_empty() || key.len() > 16 || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("{key} isn't a key name"));
    }
    let key = if key.len() == 1 {
        key.to_ascii_uppercase()
    } else {
        let mut c = key.chars();
        c.next()
            .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
            .unwrap_or_default()
    };
    out.push(key);
    Ok(out.join("+"))
}

/// A number as text: no trailing zeros, no float noise.
pub fn num(n: f64) -> String {
    let s = format!("{n:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// `n` on the step's grid (from `min`, else 0), inside the range, as text.
pub fn snap(
    n: f64,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    integer: bool,
) -> String {
    let mut n = n;
    if let Some(step) = step.filter(|s| *s > 0.0) {
        let base = min.unwrap_or(0.0);
        n = base + ((n - base) / step).round() * step;
    }
    if integer {
        n = n.round();
    }
    if let Some(m) = min {
        n = n.max(m);
    }
    if let Some(m) = max {
        n = n.min(m);
    }
    if integer {
        format!("{}", n as i64)
    } else {
        num(n)
    }
}

// ---- reading a description ----

/// What was wrong with a description (the rest of it still applies).
pub type Warning = String;

/// The `settings` of a description file's text.
pub fn parse_file(text: &str) -> (Option<Settings>, Vec<Warning>) {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => parse_manifest(&v),
        Err(e) => (None, vec![format!("not JSON: {e}")]),
    }
}

/// The `settings` of a description file (`{ "schema": 1, "settings": {..} }`).
pub fn parse_manifest(v: &Value) -> (Option<Settings>, Vec<Warning>) {
    let schema = v.get("schema").and_then(Value::as_u64).unwrap_or(SCHEMA);
    let mut warnings = Vec::new();
    if schema > SCHEMA {
        warnings.push(format!(
            "schema {schema} is newer than this launcher's ({SCHEMA}): what it knows is shown"
        ));
    }
    match v.get("settings") {
        Some(s) => {
            let (settings, w) = parse_settings(s);
            warnings.extend(w);
            (settings, warnings)
        }
        None => (None, warnings),
    }
}

/// A `settings` object: bad parts are left out with a warning.
pub fn parse_settings(v: &Value) -> (Option<Settings>, Vec<Warning>) {
    let mut w = Vec::new();
    let Some(o) = v.as_object() else {
        return (None, vec!["settings isn't an object".into()]);
    };
    let store = match o.get("store") {
        None => Store::Args,
        Some(s) => match parse_store(s) {
            Ok(s) => s,
            Err(e) => return (None, vec![e]),
        },
    };
    let mut sections = Vec::new();
    let mut keys = std::collections::HashSet::new();
    let raw_sections = match o.get("sections") {
        Some(Value::Array(a)) => a.clone(),
        // A plain list of fields: one section without a title.
        _ => match o.get("fields") {
            Some(f) => vec![serde_json::json!({ "fields": f })],
            None => Vec::new(),
        },
    };
    for (si, sec) in raw_sections.iter().enumerate() {
        let Some(so) = sec.as_object() else {
            w.push(format!("section {si} isn't an object"));
            continue;
        };
        let mut fields = Vec::new();
        for (fi, f) in so
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            match parse_field(f) {
                Ok(field) if field.has_value() && !keys.insert(field.key.to_ascii_lowercase()) => {
                    w.push(format!("{} appears twice: the second left out", field.key));
                }
                Ok(field) => fields.push(field),
                Err(e) => w.push(format!("section {si}, field {fi}: {e}")),
            }
        }
        sections.push(Section {
            title: str_of(so, "title"),
            help: str_of(so, "help"),
            tooltip: str_of(so, "tooltip"),
            fields,
        });
    }
    // Conditions name settings of this description, compared as their kept text.
    let by_key: BTreeMap<String, Field> = sections
        .iter()
        .flat_map(|s| &s.fields)
        .filter(|f| f.has_value())
        .map(|f| (f.key.clone(), f.clone()))
        .collect();
    for s in &mut sections {
        for f in &mut s.fields {
            for list in [&mut f.visible_if, &mut f.enabled_if] {
                list.retain_mut(|(k, val)| match by_key.get(k.as_str()) {
                    Some(target) => match target.check(val) {
                        Ok(norm) => {
                            *val = norm;
                            true
                        }
                        Err(_) => true,
                    },
                    None => {
                        w.push(format!("a condition names {k}, which isn't a setting here"));
                        false
                    }
                });
            }
        }
    }
    let mut row = Vec::new();
    for k in o
        .get("row")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        match by_key.get(k) {
            Some(f) if matches!(f.kind, Kind::Bool { .. } | Kind::Choice { .. }) => {
                if row.len() < ROW_MAX {
                    row.push(k.to_string());
                } else {
                    w.push(format!("row: only {ROW_MAX} settings fit, {k} left out"));
                }
            }
            Some(_) => w.push(format!("row: {k} isn't on/off or a choice")),
            None => w.push(format!("row: {k} isn't a setting here")),
        }
    }
    let settings = Settings {
        store,
        row,
        sections,
    };
    (Some(settings), w)
}

fn str_of(o: &Map<String, Value>, k: &str) -> String {
    o.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn parse_store(v: &Value) -> std::result::Result<Store, Warning> {
    let o = v.as_object().ok_or("store isn't an object")?;
    match o.get("kind").and_then(Value::as_str).unwrap_or("args") {
        "args" => Ok(Store::Args),
        "file" => {
            let file = str_of(o, "file");
            if !is_store_file(&file) {
                return Err(format!(
                    "store file {file:?} isn't a plain .txt, .ini, .cfg or .json name in plugins/"
                ));
            }
            let format = match o.get("format").and_then(Value::as_str) {
                Some("json") => FileFormat::Json,
                Some("keyvalue") | Some("ini") | None => {
                    if file.to_ascii_lowercase().ends_with(".json") {
                        FileFormat::Json
                    } else {
                        FileFormat::KeyValue
                    }
                }
                Some(other) => {
                    return Err(format!("store format {other:?} isn't keyvalue or json"))
                }
            };
            let live = o.get("live").and_then(Value::as_bool).unwrap_or(false);
            Ok(Store::File { file, format, live })
        }
        other => Err(format!("store kind {other:?} isn't args or file")),
    }
}

/// Pure: a settings file a description may name: a plain name in `plugins/`, a text or
/// JSON file (never a DLL, never a description).
pub fn is_store_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && [".txt", ".ini", ".cfg", ".json"]
            .iter()
            .any(|e| lower.ends_with(e) && lower.len() > e.len())
        && !lower.ends_with(SIDECAR_SUFFIX)
}

/// Pure: a key a setting may have: what nEVR arguments and `Key = value` lines take.
pub fn is_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn parse_field(v: &Value) -> std::result::Result<Field, String> {
    let o = v.as_object().ok_or("not an object")?;
    let ty = o.get("type").and_then(Value::as_str).unwrap_or("text");
    let key = str_of(o, "key");
    let label = str_of(o, "label");
    let style_name = o.get("style").and_then(Value::as_str);
    let choices = || -> std::result::Result<Vec<Choice>, String> {
        let list: Vec<Choice> = o
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| match c {
                Value::Object(c) => {
                    let value = text_of(c.get("value")?)?;
                    Some(Choice {
                        label: c
                            .get("label")
                            .and_then(Value::as_str)
                            .map_or_else(|| value.clone(), str::to_string),
                        help: str_of(c, "help"),
                        tooltip: str_of(c, "tooltip"),
                        value,
                    })
                }
                other => text_of(other).map(|v| Choice {
                    label: v.clone(),
                    value: v,
                    help: String::new(),
                    tooltip: String::new(),
                }),
            })
            .collect();
        if list.is_empty() {
            Err(format!("{key}: no choices"))
        } else {
            Ok(list)
        }
    };
    let f64_of = |k: &str| o.get(k).and_then(Value::as_f64);
    let kind = match ty {
        "bool" => {
            let values: Vec<String> = o
                .get("values")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(text_of).collect())
                .unwrap_or_default();
            let (on, off) = match values.as_slice() {
                [on, off] if on != off => (on.clone(), off.clone()),
                [] => ("true".into(), "false".into()),
                _ => return Err(format!("{key}: values must be [on, off]")),
            };
            Kind::Bool { on, off }
        }
        "text" => Kind::Text {
            placeholder: str_of(o, "placeholder"),
            min_len: o.get("min_len").and_then(Value::as_u64).unwrap_or(0) as usize,
            max_len: o.get("max_len").and_then(Value::as_u64).unwrap_or(0) as usize,
            format: match o.get("format").and_then(Value::as_str) {
                None => None,
                Some("url") => Some(TextFormat::Url),
                Some("hex") => Some(TextFormat::Hex),
                Some("digits") => Some(TextFormat::Digits),
                Some("name") => Some(TextFormat::Name),
                Some(other) => {
                    tracing::warn!(
                        "plugin settings: {key}: format {other:?} unknown, any text taken"
                    );
                    None
                }
            },
        },
        "secret" => Kind::Secret {
            placeholder: str_of(o, "placeholder"),
        },
        "choice" => Kind::Choice {
            choices: choices()?,
        },
        "multi" => Kind::Multi {
            choices: choices()?,
        },
        "number" => {
            let (min, max) = (f64_of("min"), f64_of("max"));
            if let (Some(a), Some(b)) = (min, max) {
                if a > b {
                    return Err(format!("{key}: min is above max"));
                }
            }
            let step = f64_of("step").filter(|s| *s > 0.0);
            Kind::Number {
                min,
                max,
                step,
                integer: o.get("integer").and_then(Value::as_bool).unwrap_or(false),
                unit: str_of(o, "unit"),
            }
        }
        "color" => Kind::Color {
            alpha: o.get("alpha").and_then(Value::as_bool).unwrap_or(false),
        },
        "key" => Kind::Key {
            modifiers: o
                .get("allow_modifiers")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        },
        "action" => {
            let sets: Vec<(String, String)> = o
                .get("sets")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter_map(|(k, v)| Some((k.clone(), text_of(v)?)))
                .collect();
            if sets.is_empty() || sets.iter().any(|(k, _)| !is_key(k)) {
                return Err(format!("{label}: an action needs sets: {{key: value}}"));
            }
            Kind::Action {
                sets,
                danger: o.get("tone").and_then(Value::as_str) == Some("danger"),
                confirm: str_of(o, "confirm"),
            }
        }
        "note" => Kind::Note {
            text: o
                .get("text")
                .and_then(Value::as_str)
                .map_or_else(|| label.clone(), str::to_string),
        },
        "link" => {
            let url = str_of(o, "url");
            if !url.starts_with("https://") {
                return Err(format!("{label}: a link must be https"));
            }
            Kind::Link { url }
        }
        other => {
            // A newer type: its value can still be edited as text.
            tracing::warn!("plugin settings: {key}: type {other:?} unknown, shown as text");
            Kind::Text {
                placeholder: String::new(),
                min_len: 0,
                max_len: 0,
                format: None,
            }
        }
    };
    let mut field = Field {
        key: key.clone(),
        kind,
        label,
        help: str_of(o, "help"),
        tooltip: str_of(o, "tooltip"),
        default: String::new(),
        advanced: o.get("advanced").and_then(Value::as_bool).unwrap_or(false),
        visible_if: conditions(o.get("visible_if")),
        enabled_if: conditions(o.get("enabled_if")),
        restart: o.get("restart").and_then(Value::as_bool),
        style: Style::Default,
        store: None,
    };
    if field.has_value() && !is_key(&field.key) {
        return Err(format!("key {:?} isn't letters, digits, . - _", field.key));
    }
    if field.label.is_empty() {
        field.label = field.key.clone();
    }
    if let Some(s) = o.get("store") {
        field.store = Some(parse_store(s)?);
    }
    field.style = style_name.map_or(Style::Default, |n| style_for(&field.kind, n, &key));
    if let Some(d) = o.get("default") {
        let text = match d {
            Value::Array(a) => a.iter().filter_map(text_of).collect::<Vec<_>>().join(","),
            other => text_of(other).unwrap_or_default(),
        };
        match field.check(&text) {
            Ok(norm) => field.default = norm,
            Err(e) => tracing::warn!("plugin settings: {key}: default {text:?} left out ({e})"),
        }
    } else if let Kind::Bool { off, .. } = &field.kind {
        field.default = off.clone();
    }
    Ok(field)
}

/// A JSON scalar as text (`true`, `0.5`, `"x"`).
fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.as_f64().map_or_else(|| n.to_string(), num)),
        _ => None,
    }
}

fn conditions(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), text_of(v)?)))
        .collect()
}

/// The style named `name`, if it fits the type (else the type's own).
fn style_for(kind: &Kind, name: &str, key: &str) -> Style {
    let style = match (kind, name) {
        (Kind::Bool { .. }, "check") => Style::Check,
        (Kind::Bool { .. }, "switch") => Style::Switch,
        (Kind::Text { .. }, "field") => Style::Field,
        (Kind::Text { .. }, "area") => Style::Area,
        (Kind::Choice { .. }, "dropdown") => Style::Dropdown,
        (Kind::Choice { .. }, "segmented") => Style::Segmented,
        (Kind::Choice { .. }, "radio") => Style::Radio,
        (Kind::Multi { .. }, "checks") => Style::Checks,
        (Kind::Multi { .. }, "chips") => Style::Chips,
        (Kind::Number { .. }, "field") => Style::Field,
        (Kind::Number { .. }, "slider") => Style::Slider,
        (Kind::Number { .. }, "stepper") => Style::Stepper,
        _ => {
            tracing::warn!("plugin settings: {key}: style {name:?} doesn't fit, its own used");
            Style::Default
        }
    };
    // A slider needs both ends.
    let ends = matches!(
        kind,
        Kind::Number {
            min: Some(_),
            max: Some(_),
            ..
        }
    );
    if style == Style::Slider && !ends {
        tracing::warn!("plugin settings: {key}: a slider needs min and max, a field used");
        return Style::Field;
    }
    style
}

impl Settings {
    pub fn fields(&self) -> impl Iterator<Item = &Field> {
        self.sections.iter().flat_map(|s| &s.fields)
    }

    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields().find(|f| f.has_value() && f.key == key)
    }

    /// Some setting is folded under "Show advanced".
    pub fn has_advanced(&self) -> bool {
        self.fields().any(|f| f.advanced)
    }

    /// Every setting's default (those that have one).
    pub fn defaults(&self) -> BTreeMap<String, String> {
        self.fields()
            .filter(|f| f.has_value() && !f.default.is_empty())
            .map(|f| (f.key.clone(), f.default.clone()))
            .collect()
    }

    /// Every change applies while the game runs.
    pub fn all_live(&self) -> bool {
        self.fields()
            .filter(|f| f.has_value())
            .all(|f| f.live(self))
    }
}

/// Pure: whether `f`'s conditions hold for `values`.
pub fn holds(conds: &[(String, String)], values: &BTreeMap<String, String>) -> bool {
    conds
        .iter()
        .all(|(k, v)| values.get(k).is_some_and(|have| have == v))
}

// ---- where a plugin's description comes from ----

/// Where a description was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Catalogue,
    Sidecar,
    Builtin,
}

/// The launcher's own descriptions, for plugins whose release doesn't ship one yet:
/// (plugin file, description).
const BUILTIN: [(&str, &str); 1] = [(
    crate::core::echoxr_hands::PLUGIN,
    include_str!("../../../docs/plugins/echoxr-hands.plugin.json"),
)];

/// `<Stem>.plugin.json` for plugin file `file`.
pub fn sidecar_name(file: &str) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    format!("{stem}{SIDECAR_SUFFIX}")
}

/// Pure: plugin `file`'s description: its catalogue entry's, else `sidecar` (the text of
/// the file beside it), else the launcher's own.
pub fn describe(
    file: &str,
    entry: Option<&ModEntry>,
    sidecar: Option<&str>,
) -> Option<(Arc<Settings>, Origin)> {
    let warn = |from: &str, w: Vec<Warning>| {
        for w in w {
            tracing::warn!("plugin settings of {file} ({from}): {w}");
        }
    };
    if let Some(v) = entry.and_then(|e| e.settings.as_ref()) {
        let (s, w) = parse_settings(v);
        warn("catalogue", w);
        if let Some(s) = s {
            return Some((Arc::new(s), Origin::Catalogue));
        }
    }
    if let Some(text) = sidecar {
        let (s, w) = parse_file(text);
        warn("plugin.json", w);
        if let Some(s) = s {
            return Some((Arc::new(s), Origin::Sidecar));
        }
    }
    let (_, text) = BUILTIN.iter().find(|(f, _)| f.eq_ignore_ascii_case(file))?;
    let (s, w) = parse_file(text);
    warn("built in", w);
    s.map(|s| (Arc::new(s), Origin::Builtin))
}

/// Reads plugin `file`'s description beside it in `plugins` (file I/O).
pub fn read_sidecar(plugins: &Path, file: &str) -> Option<String> {
    let path = plugins.join(sidecar_name(file));
    let meta = std::fs::metadata(&path).ok()?;
    // A description is small: anything big isn't one.
    if meta.len() > 256 * 1024 {
        tracing::warn!("{} is too big to be a description", path.display());
        return None;
    }
    std::fs::read_to_string(path).ok()
}

// ---- values ----

/// A plugin's settings and their values, as the Mods page shows them.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginSettings {
    pub settings: Arc<Settings>,
    pub origin: Origin,
    /// Every setting's value (its default where none is kept).
    pub values: BTreeMap<String, String>,
}

fn store_path(plugins: &Path, file: &str) -> PathBuf {
    plugins.join(file)
}

/// Pure: the values of `s` from the plugin's arguments (`args`) and its settings files
/// (`files`: file name to text), with defaults where none is kept.
pub fn values_from(
    s: &Settings,
    args: &BTreeMap<String, String>,
    files: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for f in s.fields().filter(|f| f.has_value()) {
        let kept = match f.store(s) {
            Store::Args => args.get(&f.key).cloned(),
            Store::File { file, format, .. } => files.get(file).and_then(|t| match format {
                FileFormat::KeyValue => keyvalue::get(t, &f.key),
                FileFormat::Json => json_get(t, &f.key),
            }),
        };
        let value = kept
            .map(|k| f.check(&k).unwrap_or(k))
            .unwrap_or_else(|| f.default.clone());
        out.insert(f.key.clone(), value);
    }
    out
}

/// The settings files `s` names (file names).
pub fn store_files(s: &Settings) -> Vec<String> {
    let mut out: Vec<String> = s
        .fields()
        .filter_map(|f| match f.store(s) {
            Store::File { file, .. } => Some(file.clone()),
            Store::Args => None,
        })
        .chain(match &s.store {
            Store::File { file, .. } => Some(file.clone()),
            Store::Args => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Plugin `p`'s settings in `plugins` with their values (file I/O: on a worker).
pub fn read(plugins: &Path, p: &Plugin, entry: Option<&ModEntry>) -> Option<PluginSettings> {
    let sidecar = read_sidecar(plugins, &p.file);
    let (settings, origin) = describe(&p.file, entry, sidecar.as_deref())?;
    let files = store_files(&settings)
        .into_iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(store_path(plugins, &f)).ok()?;
            Some((f, text))
        })
        .collect();
    let values = values_from(&settings, &p.arg_strings(), &files);
    Some(PluginSettings {
        settings,
        origin,
        values,
    })
}

/// Writes `changes` (key to value, already checked) of plugin `file`'s settings `s` in
/// `v`: arguments through the launcher's choices, files in `plugins/` in place (their other
/// lines kept).
pub fn write(
    v: &InstalledVersion,
    file: &str,
    s: &Settings,
    changes: &[(String, String)],
) -> Result<()> {
    let plugins = mods::plugins_dir(v);
    let mut args = Vec::new();
    let mut by_file: BTreeMap<String, (FileFormat, Vec<(String, String)>)> = BTreeMap::new();
    for (key, value) in changes {
        if !is_key(key) {
            bail!("{key:?} isn't a setting's key");
        }
        // An action may set keys it doesn't describe: they go to the description's store.
        let store = s.field(key).map_or(&s.store, |f| f.store(s));
        match store {
            Store::Args => args.push((key.clone(), value.clone())),
            Store::File { file, format, .. } => by_file
                .entry(file.clone())
                .or_insert_with(|| (*format, Vec::new()))
                .1
                .push((key.clone(), value.clone())),
        }
    }
    if !args.is_empty() {
        mods::set_some_args(v, file, &args)?;
    }
    write_files(&plugins, s, by_file)
}

/// Writes `changes` of settings `s` kept in files in `dir` (a launcher plugin's folder);
/// settings kept as nEVR arguments can't be written there.
pub fn write_in(dir: &Path, s: &Settings, changes: &[(String, String)]) -> Result<()> {
    let mut by_file: BTreeMap<String, (FileFormat, Vec<(String, String)>)> = BTreeMap::new();
    for (key, value) in changes {
        if !is_key(key) {
            bail!("{key:?} isn't a setting's key");
        }
        match s.field(key).map_or(&s.store, |f| f.store(s)) {
            Store::Args => bail!("{key}: kept as an argument, which only a mod has"),
            Store::File { file, format, .. } => by_file
                .entry(file.clone())
                .or_insert_with(|| (*format, Vec::new()))
                .1
                .push((key.clone(), value.clone())),
        }
    }
    write_files(dir, s, by_file)
}

/// The values of settings `s` kept in files in `dir`, with defaults where none is kept.
pub fn read_in(dir: &Path, s: &Settings) -> BTreeMap<String, String> {
    let files = store_files(s)
        .into_iter()
        .filter_map(|f| {
            Some((
                f.clone(),
                std::fs::read_to_string(store_path(dir, &f)).ok()?,
            ))
        })
        .collect();
    values_from(s, &BTreeMap::new(), &files)
}

fn write_files(
    plugins: &Path,
    s: &Settings,
    by_file: BTreeMap<String, (FileFormat, Vec<(String, String)>)>,
) -> Result<()> {
    for (name, (format, kv)) in by_file {
        if !is_store_file(&name) {
            bail!("{name} isn't a settings file the launcher writes");
        }
        let path = store_path(plugins, &name);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let new = match format {
            FileFormat::KeyValue => {
                let mut t = text.clone();
                for (k, val) in &kv {
                    if val.contains(['\n', '\r']) {
                        bail!("{k}: a value in {name} is one line");
                    }
                    t = keyvalue::set(&t, k, val);
                }
                t
            }
            FileFormat::Json => {
                let mut t = text.clone();
                for (k, val) in &kv {
                    t = json_set(&t, k, val, s.field(k))?;
                }
                t
            }
        };
        if new != text {
            write_atomic(&path, &new)?;
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("launcher.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("Couldn't write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't replace {}", path.display())
    })
}

fn json_get(text: &str, key: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    let x = v.get(key)?;
    match x {
        Value::Array(a) => Some(a.iter().filter_map(text_of).collect::<Vec<_>>().join(",")),
        other => text_of(other),
    }
}

/// Pure: `text` (a flat JSON object, or nothing) with `key` set to `value`, typed as the
/// setting is: a bool as true/false (when kept as such), a number as a number.
fn json_set(text: &str, key: &str, value: &str, field: Option<&Field>) -> Result<String> {
    let mut o = if text.trim().is_empty() {
        Map::new()
    } else {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(o)) => o,
            _ => bail!("the settings file isn't a JSON object: the launcher leaves it alone"),
        }
    };
    let typed = match field.map(|f| &f.kind) {
        Some(Kind::Bool { on, off }) if on == "true" && off == "false" => {
            Value::Bool(value == "true")
        }
        Some(Kind::Number { .. }) => value
            .parse::<f64>()
            .ok()
            .and_then(|n| {
                if n.fract() == 0.0 && n.abs() < 9e15 {
                    Some(Value::from(n as i64))
                } else {
                    serde_json::Number::from_f64(n).map(Value::Number)
                }
            })
            .unwrap_or_else(|| Value::String(value.into())),
        _ => Value::String(value.into()),
    };
    o.insert(key.to_string(), typed);
    let mut out = serde_json::to_string_pretty(&Value::Object(o))?;
    out.push('\n');
    Ok(out)
}

/// `Key = value` lines: comments start with `#` or `;` (at a line's start, or after a
/// value and a space, so `Color = #ff0000` is a value).
pub mod keyvalue {
    /// Pure: where a line's value ends (its comment starts), from the `=`.
    fn value_end(rest: &str) -> usize {
        let mut seen_value = false;
        let mut prev_space = true;
        for (i, c) in rest.char_indices() {
            if (c == '#' || c == ';') && prev_space && seen_value {
                return i;
            }
            if !c.is_whitespace() {
                seen_value = true;
            }
            prev_space = c.is_whitespace();
        }
        rest.len()
    }

    fn split(line: &str) -> Option<(&str, &str)> {
        let t = line.trim_start();
        if t.starts_with('#') || t.starts_with(';') || t.starts_with('[') {
            return None;
        }
        line.split_once('=')
    }

    /// Pure: `key`'s value in `text`.
    pub fn get(text: &str, key: &str) -> Option<String> {
        text.lines().find_map(|l| {
            let (k, rest) = split(l)?;
            (k.trim() == key).then(|| rest[..value_end(rest)].trim().to_string())
        })
    }

    /// Pure: `text` with `key` set to `value`: on its line (its comment kept), else added
    /// at the end. Every other line stays as it is.
    pub fn set(text: &str, key: &str, value: &str) -> String {
        let mut found = false;
        let mut out: Vec<String> = text
            .lines()
            .map(|l| match split(l) {
                Some((k, rest)) if !found && k.trim() == key => {
                    found = true;
                    let end = value_end(rest);
                    let comment = &rest[end..];
                    if comment.is_empty() {
                        format!("{} = {value}", k.trim_end())
                    } else {
                        // Keep the gap before the comment.
                        let gap = rest[..end].len() - rest[..end].trim_end().len();
                        format!(
                            "{} = {value}{}{comment}",
                            k.trim_end(),
                            " ".repeat(gap.max(1))
                        )
                    }
                }
                _ => l.to_string(),
            })
            .collect();
        if !found {
            out.push(format!("{key} = {value}"));
        }
        let mut s = out.join("\n");
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(v: Value) -> (Settings, Vec<Warning>) {
        let (s, w) = parse_settings(&v);
        (s.expect("settings"), w)
    }

    fn field(v: Value) -> Field {
        parse_field(&v).expect("field")
    }

    #[test]
    fn reads_every_type() {
        let (s, w) = settings(json!({
            "sections": [{ "title": "All", "fields": [
                { "key": "on", "type": "bool", "values": ["1", "0"], "default": true, "style": "switch" },
                { "key": "name", "type": "text", "max_len": 8, "format": "name" },
                { "key": "token", "type": "secret" },
                { "key": "mode", "type": "choice", "choices": ["a", { "value": "b", "label": "Bee" }], "default": "b", "style": "segmented" },
                { "key": "parts", "type": "multi", "choices": ["x", "y", "z"], "default": ["z", "x"] },
                { "key": "speed", "type": "number", "min": 0, "max": 2, "step": 0.25, "default": 0.5, "style": "slider" },
                { "key": "tint", "type": "color", "default": "#FF8800" },
                { "key": "hotkey", "type": "key", "default": "ctrl+alt+c" },
                { "type": "action", "label": "Go", "sets": { "go": 1 }, "tone": "danger" },
                { "type": "note", "text": "Hello" },
                { "type": "link", "label": "Docs", "url": "https://example.com" }
            ]}]
        }));
        assert!(w.is_empty(), "{w:?}");
        let d = s.defaults();
        assert_eq!(d["on"], "1");
        assert_eq!(d["mode"], "b");
        assert_eq!(d["parts"], "x,z");
        assert_eq!(d["speed"], "0.5");
        assert_eq!(d["tint"], "#ff8800");
        assert_eq!(d["hotkey"], "Ctrl+Alt+C");
        assert_eq!(s.field("on").unwrap().style, Style::Switch);
        assert_eq!(s.field("speed").unwrap().style, Style::Slider);
        assert_eq!(s.sections[0].fields.len(), 11);
        match &s.field("mode").unwrap().kind {
            Kind::Choice { choices } => assert_eq!(choices[1].label, "Bee"),
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn leaves_out_what_is_wrong() {
        let (s, w) = settings(json!({
            "row": ["on", "name", "missing"],
            "fields": [
                { "key": "on", "type": "bool" },
                { "key": "on", "type": "bool" },
                { "key": "bad key!", "type": "text" },
                { "key": "name", "type": "text" },
                { "key": "c", "type": "choice", "choices": [] },
                { "key": "n", "type": "number", "min": 3, "max": 1 },
                { "type": "link", "url": "http://insecure" },
                { "key": "s", "type": "number", "style": "slider" },
                { "key": "v", "type": "text", "visible_if": { "nope": true } }
            ]
        }));
        let keys: Vec<&str> = s.fields().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["on", "name", "s", "v"]);
        assert_eq!(s.row, ["on"]);
        // A slider without both ends is a field.
        assert_eq!(s.field("s").unwrap().style, Style::Field);
        assert!(s.field("v").unwrap().visible_if.is_empty());
        assert!(w.len() >= 8, "{w:?}");
    }

    #[test]
    fn refuses_files_outside_plugins() {
        for bad in [
            "../config.yaml",
            "a/b.txt",
            "x.dll",
            ".hidden.txt",
            "Other.plugin.json",
            "C:\\x.txt",
            ".txt",
        ] {
            assert!(!is_store_file(bad), "{bad}");
            let (s, w) = parse_settings(&json!({ "store": { "kind": "file", "file": bad } }));
            assert!(s.is_none() && !w.is_empty(), "{bad}");
        }
        for good in ["EchoXRHands.txt", "my-plugin.ini", "x.cfg", "s.json"] {
            assert!(is_store_file(good), "{good}");
        }
        let (s, _) = settings(json!({ "store": { "kind": "file", "file": "s.json" } }));
        assert!(matches!(
            s.store,
            Store::File {
                format: FileFormat::Json,
                live: false,
                ..
            }
        ));
    }

    #[test]
    fn checks_values() {
        let b = field(json!({ "key": "b", "type": "bool", "values": ["1", "0"] }));
        assert_eq!(b.check("true").unwrap(), "1");
        assert_eq!(b.check("0").unwrap(), "0");
        assert!(b.check("maybe").is_err());

        let n = field(json!({ "key": "n", "type": "number", "min": 0.05, "max": 3, "step": 0.05 }));
        assert_eq!(n.check("0.5").unwrap(), "0.5");
        assert_eq!(n.check("0.52").unwrap(), "0.5");
        assert_eq!(n.check(" 0.3 ").unwrap(), "0.3");
        assert!(n.check("4").is_err());
        assert!(n.check("abc").is_err());
        assert!(n.check("NaN").is_err());

        let i = field(json!({ "key": "i", "type": "number", "integer": true }));
        assert_eq!(i.check("2.6").unwrap(), "3");

        let c = field(json!({ "key": "c", "type": "choice", "choices": ["a", "b"] }));
        assert!(c.check("c").is_err());
        let m = field(json!({ "key": "m", "type": "multi", "choices": ["a", "b", "c"] }));
        assert_eq!(m.check("c, a").unwrap(), "a,c");
        assert_eq!(m.check("").unwrap(), "");
        assert!(m.check("a,d").is_err());

        let t = field(json!({ "key": "t", "type": "text", "format": "url" }));
        assert!(t.check("wss://relay.example/ws").is_ok());
        assert!(t.check("not a url").is_err());
        assert!(t.check("").is_ok());
        let d = field(json!({ "key": "d", "type": "text", "format": "digits", "min_len": 3 }));
        assert!(d.check("12").is_err());
        assert!(d.check("12a").is_err());
        assert!(d.check("123").is_ok());

        let col = field(json!({ "key": "c", "type": "color" }));
        assert_eq!(col.check("AABBCC").unwrap(), "#aabbcc");
        assert!(col.check("#aabbccdd").is_err());

        let k = field(json!({ "key": "k", "type": "key" }));
        assert_eq!(k.check("shift + f5").unwrap(), "Shift+F5");
        assert!(k.check("Hyper+X").is_err());
        let single = field(json!({ "key": "k", "type": "key", "allow_modifiers": false }));
        assert!(single.check("Ctrl+X").is_err());
    }

    #[test]
    fn hover_texts_fall_back_to_help() {
        let (s, w) = settings(
            json!({ "sections": [{ "title": "S", "tooltip": "About S", "fields": [
                { "key": "a", "type": "bool", "help": "Under it", "tooltip": "On hover" },
                { "key": "b", "type": "bool", "help": "Under it" },
                { "key": "c", "type": "choice", "help": "Pick one", "choices": [
                    { "value": "x", "tooltip": "X on hover", "help": "beside x" },
                    { "value": "y", "help": "beside y" },
                    "z"
                ]}
            ]}]}),
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.sections[0].tooltip, "About S");
        assert_eq!(s.field("a").unwrap().hover(), "On hover");
        assert_eq!(s.field("b").unwrap().hover(), "Under it");
        let c = s.field("c").unwrap();
        let Kind::Choice { choices } = &c.kind else {
            panic!()
        };
        let hovers: Vec<&str> = choices.iter().map(|ch| ch.hover(c)).collect();
        assert_eq!(hovers, ["X on hover", "beside y", "Pick one"]);
    }

    #[test]
    fn conditions_compare_kept_values() {
        let (s, w) = settings(json!({ "fields": [
            { "key": "net", "type": "bool", "values": ["1", "0"] },
            { "key": "relay", "type": "text", "visible_if": { "net": true } }
        ]}));
        assert!(w.is_empty(), "{w:?}");
        let relay = s.field("relay").unwrap();
        assert_eq!(relay.visible_if, [("net".to_string(), "1".to_string())]);
        let mut values = s.defaults();
        assert!(!holds(&relay.visible_if, &values));
        values.insert("net".into(), "1".into());
        assert!(holds(&relay.visible_if, &values));
    }

    #[test]
    fn keyvalue_keeps_comments_order_and_unknown_lines() {
        let text = "# EchoXR Hands settings.\n\nEnabled = 1\n\n# Which slot\nGameHandLeft = 0\nNetwork = 1   # share\nColor = #ff0000\nUnknown=stays\n";
        assert_eq!(keyvalue::get(text, "Network").as_deref(), Some("1"));
        assert_eq!(keyvalue::get(text, "Color").as_deref(), Some("#ff0000"));
        assert_eq!(keyvalue::get(text, "Missing"), None);
        let t = keyvalue::set(text, "Network", "0");
        assert!(t.contains("Network = 0   # share"), "{t}");
        let t = keyvalue::set(&t, "GameHandLeft", "1");
        let t = keyvalue::set(&t, "NewKey", "x");
        assert_eq!(
            t,
            "# EchoXR Hands settings.\n\nEnabled = 1\n\n# Which slot\nGameHandLeft = 1\nNetwork = 0   # share\nColor = #ff0000\nUnknown=stays\nNewKey = x\n"
        );
        // A commented-out key isn't the key.
        assert_eq!(keyvalue::get("# Network = 1\n", "Network"), None);
    }

    #[test]
    fn json_store_keeps_types_and_other_keys() {
        let f = field(json!({ "key": "speed", "type": "number" }));
        let b = field(json!({ "key": "on", "type": "bool" }));
        let t = json_set("{\"other\": [1, 2]}", "speed", "0.5", Some(&f)).unwrap();
        let t = json_set(&t, "on", "true", Some(&b)).unwrap();
        let t = json_set(
            &t,
            "count",
            "3",
            Some(&field(json!({ "key": "count", "type": "number" }))),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(
            v,
            json!({ "other": [1, 2], "speed": 0.5, "on": true, "count": 3 })
        );
        assert_eq!(json_get(&t, "speed").as_deref(), Some("0.5"));
        assert_eq!(json_get(&t, "on").as_deref(), Some("true"));
        assert!(json_set("[1]", "a", "b", None).is_err());
    }

    #[test]
    fn values_come_from_their_stores_with_defaults() {
        let (s, _) = settings(json!({
            "store": { "kind": "file", "file": "P.txt", "live": true },
            "fields": [
                { "key": "a", "type": "number", "default": 1 },
                { "key": "b", "type": "bool", "values": ["1", "0"] },
                { "key": "logging", "type": "choice", "choices": ["quiet", "normal"], "default": "normal", "store": { "kind": "args" } }
            ]
        }));
        let args = BTreeMap::from([("logging".to_string(), "quiet".to_string())]);
        let files = BTreeMap::from([("P.txt".to_string(), "b = 1\n".to_string())]);
        let v = values_from(&s, &args, &files);
        assert_eq!(v["a"], "1");
        assert_eq!(v["b"], "1");
        assert_eq!(v["logging"], "quiet");
        assert!(s.field("a").unwrap().live(&s));
        assert!(!s.field("logging").unwrap().live(&s));
        assert_eq!(store_files(&s), ["P.txt"]);
    }

    #[test]
    fn lookup_order() {
        let entry = ModEntry {
            file: "X.dll".into(),
            settings: Some(json!({ "fields": [{ "key": "from_catalogue", "type": "bool" }] })),
            ..Default::default()
        };
        let sidecar = r#"{ "schema": 1, "settings": { "fields": [{ "key": "from_file", "type": "bool" }] } }"#;
        let (s, o) = describe("X.dll", Some(&entry), Some(sidecar)).unwrap();
        assert_eq!(
            (o, s.sections[0].fields[0].key.as_str()),
            (Origin::Catalogue, "from_catalogue")
        );
        let (s, o) = describe("X.dll", None, Some(sidecar)).unwrap();
        assert_eq!(
            (o, s.sections[0].fields[0].key.as_str()),
            (Origin::Sidecar, "from_file")
        );
        assert!(describe("X.dll", None, None).is_none());
        assert_eq!(sidecar_name("EchoXRHands.dll"), "EchoXRHands.plugin.json");
    }

    /// A version on disk with nEVR, NvrAssetPatches and EchoXR Hands (its real settings
    /// file).
    fn version(dir: &Path) -> InstalledVersion {
        use crate::core::paths;
        let root = dir.join("pc");
        let bin = root.join(paths::ARENA_DIR).join("bin/win10");
        std::fs::create_dir_all(bin.join("plugins")).unwrap();
        std::fs::write(bin.join("echovr.exe"), "MZ").unwrap();
        std::fs::write(bin.join(mods::SLOT), "MZ [NEVR.BOOT] \x004.0.0+1.abcdef0\0").unwrap();
        std::fs::write(bin.join("plugins/NvrAssetPatches.dll"), "MZ").unwrap();
        std::fs::write(bin.join("plugins/EchoXRHands.dll"), "MZ").unwrap();
        std::fs::write(
            bin.join("plugins/EchoXRHands.txt"),
            include_str!("testdata/EchoXRHands.txt"),
        )
        .unwrap();
        InstalledVersion {
            id: "pc-latest".into(),
            name: "Echo VR".into(),
            root: paths::normalize(&root.to_string_lossy()),
            ..Default::default()
        }
    }

    fn shown(v: &InstalledVersion, file: &str) -> Plugin {
        mods::read(v)
            .plugins
            .into_iter()
            .find(|p| p.file == file)
            .unwrap()
    }

    /// From the Mods page's read to the plugin's own file: values read, two changed, every
    /// other line as it was.
    #[test]
    fn writes_a_plugins_own_file_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let v = version(dir.path());
        let p = shown(&v, "EchoXRHands.dll");
        let sp = p.settings.expect("EchoXR Hands has its description");
        assert_eq!(sp.origin, Origin::Builtin);
        assert_eq!(sp.values["Network"], "0");
        assert_eq!(sp.values["FilterMinCutoff"], "0.5");
        let changes = [
            ("Network".to_string(), "1".to_string()),
            ("FilterMinCutoff".to_string(), "0.3".to_string()),
        ];
        write(&v, &p.file, &sp.settings, &changes).unwrap();
        let path = mods::plugins_dir(&v).join("EchoXRHands.txt");
        let after = std::fs::read_to_string(&path).unwrap();
        let before = include_str!("testdata/EchoXRHands.txt");
        let changed: Vec<(&str, &str)> = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(
            changed,
            [
                ("FilterMinCutoff = 0.5", "FilterMinCutoff = 0.3"),
                ("Network = 0", "Network = 1"),
            ]
        );
        assert_eq!(before.lines().count(), after.lines().count());
        let again = shown(&v, "EchoXRHands.dll").settings.unwrap();
        assert_eq!(again.values["Network"], "1");
    }

    /// A description beside the DLL whose settings are nEVR arguments: written into the
    /// launcher's choices, read back as the plugin's arguments.
    #[test]
    fn writes_arguments_through_the_launchers_choices() {
        let dir = tempfile::tempdir().unwrap();
        let v = version(dir.path());
        std::fs::write(
            mods::plugins_dir(&v).join("NvrAssetPatches.plugin.json"),
            r#"{ "schema": 1, "settings": { "fields": [
                { "key": "logging", "type": "choice", "choices": ["quiet", "normal", "verbose"], "default": "normal" }
            ] } }"#,
        )
        .unwrap();
        let p = shown(&v, "NvrAssetPatches.dll");
        let sp = p.settings.expect("the description beside it");
        assert_eq!(sp.origin, Origin::Sidecar);
        assert_eq!(sp.values["logging"], "normal");
        write(
            &v,
            &p.file,
            &sp.settings,
            &[("logging".into(), "verbose".into())],
        )
        .unwrap();
        let p = shown(&v, "NvrAssetPatches.dll");
        assert_eq!(
            p.args.get("logging"),
            Some(&Value::String("verbose".into()))
        );
        assert_eq!(p.settings.unwrap().values["logging"], "verbose");
        // Back to its default: the choice goes away again.
        write(
            &v,
            &p.file,
            &sp.settings,
            &[("logging".into(), "normal".into())],
        )
        .unwrap();
        assert_eq!(
            shown(&v, "NvrAssetPatches.dll").args.get("logging"),
            Some(&Value::String("normal".into()))
        );
    }

    /// The launcher's own description of EchoXR Hands reads without a warning, and its
    /// defaults are what the plugin's own settings file says.
    #[test]
    fn builtin_echoxr_hands() {
        let (s, w) = parse_file(BUILTIN[0].1);
        assert!(w.is_empty(), "{w:?}");
        let s = s.unwrap();
        assert!(
            matches!(&s.store, Store::File { file, live: true, .. } if file == "EchoXRHands.txt")
        );
        assert_eq!(s.row, ["Network"]);
        let fixture = include_str!("testdata/EchoXRHands.txt");
        for f in s
            .fields()
            .filter(|f| f.has_value() && !f.default.is_empty())
        {
            let kept = keyvalue::get(fixture, &f.key)
                .unwrap_or_else(|| panic!("{} isn't in EchoXRHands.txt", f.key));
            assert_eq!(f.check(&kept).unwrap(), f.default, "{}", f.key);
        }
    }
}
