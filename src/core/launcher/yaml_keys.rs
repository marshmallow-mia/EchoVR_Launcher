//! The keys the launcher sets in a `config.yaml` that is someone else's: the player's for
//! PLAY, a host's for each game server. The file is theirs ("the player needs to own the
//! config.yaml file", nEVR): the launcher changes only its own keys, line by line, and
//! keeps every other line as it was (comments, order, keys it doesn't know, CRLF, a BOM).
//!
//! What the launcher set is noted beside the file (`config.yaml.launcher.json`): a key it
//! no longer wants goes only while it still holds the launcher's value, and a value the
//! player had there before comes back then. A value the player changed stays theirs.
//!
//! nEVR's `config.yaml` is plain block YAML, two levels deep where the launcher writes
//! (`network:` then `  tick_rate_hz: 120`), so this needs no YAML library. A parent written
//! in flow style (`services: {}`) is rewritten as a block, with its entries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A value the launcher wants at a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Val {
    /// On the key's line: `true`, `120`, `"wss://…"`, `["euw"]`.
    Inline(String),
    /// The lines under a top-level key, indented as they are written (`plugins:`'s list).
    Block(String),
}

/// One change: `path` is a top-level key or `[parent, key]`. None: the launcher wants no
/// value there (it takes back only its own).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub path: Vec<String>,
    pub want: Option<Val>,
}

impl Edit {
    pub fn set(path: &[&str], v: Val) -> Edit {
        Edit {
            path: path.iter().map(|s| s.to_string()).collect(),
            want: Some(v),
        }
    }

    pub fn inline(path: &[&str], v: impl Into<String>) -> Edit {
        Edit::set(path, Val::Inline(v.into()))
    }

    /// `v` when there is one, else none wanted.
    pub fn maybe(path: &[&str], v: Option<String>) -> Edit {
        Edit {
            path: path.iter().map(|s| s.to_string()).collect(),
            want: v.map(Val::Inline),
        }
    }

    pub fn none(path: &[&str]) -> Edit {
        Edit {
            path: path.iter().map(|s| s.to_string()).collect(),
            want: None,
        }
    }
}

/// What the launcher set in one file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes {
    /// By `parent.key`: the value as the file holds it, and the one there before.
    #[serde(default)]
    pub keys: BTreeMap<String, Noted>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Noted {
    pub ours: String,
    #[serde(default)]
    pub before: Option<String>,
}

/// Where the notes on `path` are kept.
pub fn notes_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".launcher.json");
    path.with_file_name(name)
}

pub fn read_notes(path: &Path) -> Notes {
    std::fs::read_to_string(notes_path(path))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The edits that set in another file what the launcher set in the one at `path`: each of
/// `keys` it noted there with its value, the others none (taken back where they are its).
pub fn noted_edits(path: &Path, keys: &[&[&str]]) -> Vec<Edit> {
    let notes = read_notes(path);
    keys.iter()
        .map(|k| Edit {
            path: k.iter().map(|s| s.to_string()).collect(),
            want: notes.keys.get(&k.join(".")).map(|n| from_repr(&n.ours)),
        })
        .collect()
}

/// The comment line the launcher keeps at the top: `header` (once), after the old
/// headers it wrote (lines starting with one of `old`) are taken out of the leading
/// comments.
pub fn with_header(text: &str, header: &str, old: &[&str]) -> String {
    let d = Doc::parse(text);
    let mut lines = d.lines;
    let lead = lines
        .iter()
        .take_while(|l| l.trim().is_empty() || l.trim_start().starts_with('#'))
        .count();
    let mut kept: Vec<String> = Vec::new();
    for (i, l) in lines.drain(..).enumerate() {
        let drop = i < lead && (l == header || old.iter().any(|o| l.starts_with(o)));
        if !drop {
            kept.push(l);
        }
    }
    while kept.first().is_some_and(|l| l.trim().is_empty()) {
        kept.remove(0);
    }
    kept.insert(0, header.to_string());
    Doc {
        lines: kept,
        ..Doc::parse(text)
    }
    .render()
}

/// Pure: `edits` applied to `text`, with what `notes` says the launcher set before. The new
/// text and notes.
pub fn apply_to(text: &str, edits: &[Edit], notes: &Notes) -> (String, Notes) {
    let mut d = Doc::parse(text);
    let mut notes = notes.clone();
    for e in edits {
        let path: Vec<&str> = e.path.iter().map(String::as_str).collect();
        let name = e.path.join(".");
        let cur = d.get(&path);
        let rec = notes.keys.get(&name).cloned();
        match &e.want {
            Some(v) => {
                let before = match &rec {
                    // Set by the launcher before: what was there before that stays.
                    Some(r) => r.before.clone(),
                    None => cur.clone(),
                };
                d.set(&path, Some(v));
                let ours = d.get(&path).unwrap_or_default();
                notes.keys.insert(name, Noted { ours, before });
            }
            None => {
                if let Some(r) = rec {
                    if cur.as_deref() == Some(r.ours.as_str()) {
                        let back = r.before.map(|b| from_repr(&b));
                        d.set(&path, back.as_ref());
                    }
                    notes.keys.remove(&name);
                }
            }
        }
    }
    (d.render(), notes)
}

/// Applies `edits` to the file at `path` (made when there is none), with `header` at its
/// top. Written only when it changed, through a `.part` file. Returns the text.
pub fn apply(path: &Path, edits: &[Edit], header: &str, old: &[&str]) -> Result<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    apply_from(path, &text, edits, header, old)
}

/// [`apply`] on `text` as the file's current text (an older copy brought back).
pub fn apply_from(
    path: &Path,
    text: &str,
    edits: &[Edit],
    header: &str,
    old: &[&str],
) -> Result<String> {
    let notes = read_notes(path);
    let (out, new_notes) = apply_to(text, edits, &notes);
    let out = with_header(&out, header, old);
    if std::fs::read_to_string(path).ok().as_deref() != Some(out.as_str()) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        write_atomic(path, &out)?;
    }
    if new_notes != notes {
        save_notes(path, &new_notes)?;
    }
    Ok(out)
}

pub fn save_notes(path: &Path, notes: &Notes) -> Result<()> {
    let text = serde_json::to_string_pretty(notes)?;
    write_atomic(&notes_path(path), &text)
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let mut part = path.as_os_str().to_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, text).with_context(|| format!("Couldn't write {}", part.display()))?;
    std::fs::rename(&part, path).with_context(|| {
        let _ = std::fs::remove_file(&part);
        format!("Couldn't replace {}", path.display())
    })
}

/// A value as the notes keep it, back to one to write.
fn from_repr(r: &str) -> Val {
    match r.strip_prefix('\n') {
        Some(body) => Val::Block(body.to_string()),
        None => Val::Inline(r.to_string()),
    }
}

// ---- the lines ----

/// The file as lines (without their ends), and how it writes them back.
struct Doc {
    lines: Vec<String>,
    crlf: bool,
    bom: bool,
}

/// An entry's lines: `start` is its key's, `end` is past its last.
#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

impl Doc {
    fn parse(text: &str) -> Doc {
        let bom = text.starts_with('\u{feff}');
        let text = text.trim_start_matches('\u{feff}');
        Doc {
            lines: text
                .lines()
                .map(|l| l.trim_end_matches('\r').to_string())
                .collect(),
            crlf: text.contains("\r\n"),
            bom,
        }
    }

    fn render(&self) -> String {
        let nl = if self.crlf { "\r\n" } else { "\n" };
        let mut out = String::new();
        if self.bom {
            out.push('\u{feff}');
        }
        for l in &self.lines {
            out.push_str(l);
            out.push_str(nl);
        }
        out
    }

    /// The top-level entry `key`.
    fn top(&self, key: &str) -> Option<Span> {
        let mut i = 0;
        while i < self.lines.len() {
            if top_key(&self.lines[i]).is_some_and(|k| k == key) {
                return Some(Span {
                    start: i,
                    end: self.entry_end(i, 0),
                });
            }
            i += 1;
        }
        None
    }

    /// Past the last line of the entry whose key is on line `start`, at `indent`: the lines
    /// after it that are deeper (or a list at the top level), without the blank lines and
    /// comments that end it.
    fn entry_end(&self, start: usize, indent: usize) -> usize {
        let mut last = start;
        for (i, l) in self.lines.iter().enumerate().skip(start + 1) {
            let t = l.trim_start();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let deeper = indent_of(l) > indent || (indent == 0 && t.starts_with('-'));
            if !deeper {
                break;
            }
            last = i;
        }
        last + 1
    }

    /// The child entries of the block at `p`: their key, span and indent.
    fn children(&self, p: Span) -> Vec<(String, Span)> {
        let Some(indent) = self.child_indent(p) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut i = p.start + 1;
        while i < p.end {
            let l = &self.lines[i];
            if indent_of(l) == indent {
                if let Some(k) = key_of(l.trim_start()) {
                    let end = self.entry_end(i, indent).min(p.end);
                    out.push((k, Span { start: i, end }));
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    fn child_indent(&self, p: Span) -> Option<usize> {
        self.lines[p.start + 1..p.end]
            .iter()
            .find(|l| {
                let t = l.trim_start();
                !t.is_empty() && !t.starts_with('#')
            })
            .map(|l| indent_of(l))
            .filter(|i| *i > 0)
    }

    /// The value at `path`, as the notes keep it: what is on the key's line (without a
    /// comment), and the lines under it after a newline.
    fn get(&self, path: &[&str]) -> Option<String> {
        let top = self.top(path[0])?;
        let span = match path {
            [_] => top,
            [_, k] => {
                if let Some(flow) = self.flow(top) {
                    return flow.into_iter().find(|(fk, _)| fk == k).map(|(_, v)| v);
                }
                self.children(top).into_iter().find(|(c, _)| c == k)?.1
            }
            _ => return None,
        };
        Some(self.repr(span))
    }

    fn repr(&self, s: Span) -> String {
        let first = &self.lines[s.start];
        let inline = first
            .split_once(':')
            .map(|(_, v)| strip_comment(v).trim().to_string())
            .unwrap_or_default();
        let body: Vec<&str> = self.lines[s.start + 1..s.end]
            .iter()
            .map(|l| l.trim_end())
            .collect();
        if body.is_empty() {
            inline
        } else {
            format!("{inline}\n{}", body.join("\n"))
        }
    }

    /// The entries of a top-level value written in flow style (`{a: 1, b: "x"}`).
    fn flow(&self, top: Span) -> Option<Vec<(String, String)>> {
        let first = &self.lines[top.start];
        let v = first.split_once(':')?.1;
        let v = strip_comment(v).trim();
        let inner = v.strip_prefix('{')?.strip_suffix('}')?;
        Some(
            split_flow(inner)
                .into_iter()
                .filter_map(|e| {
                    let (k, v) = e.split_once(':')?;
                    Some((unquote(k.trim()), v.trim().to_string()))
                })
                .collect(),
        )
    }

    /// A flow-style top-level value becomes a block with the same entries.
    fn unflow(&mut self, top: Span) -> Span {
        let Some(entries) = self.flow(top) else {
            return top;
        };
        let key = top_key(&self.lines[top.start]).unwrap_or_default();
        let mut new = vec![format!("{}:", quote_key(&key))];
        new.extend(
            entries
                .iter()
                .map(|(k, v)| format!("  {}: {v}", quote_key(k))),
        );
        let n = new.len();
        self.lines.splice(top.start..top.end, new);
        Span {
            start: top.start,
            end: top.start + n,
        }
    }

    /// `path` set to `v`, or taken out (None).
    fn set(&mut self, path: &[&str], v: Option<&Val>) {
        match path {
            [k] => self.set_top(k, v),
            [p, k] => self.set_child(p, k, v),
            _ => {}
        }
    }

    fn set_top(&mut self, key: &str, v: Option<&Val>) {
        let at = self.top(key);
        let Some(v) = v else {
            if let Some(s) = at {
                self.lines.drain(s.start..s.end);
            }
            return;
        };
        let new = entry_lines(&quote_key(key), v, 0);
        match at {
            Some(s) => {
                self.lines.splice(s.start..s.end, new);
            }
            None => {
                self.trim_end();
                self.lines.extend(new);
            }
        }
    }

    fn set_child(&mut self, parent: &str, key: &str, v: Option<&Val>) {
        let Some(top) = self.top(parent) else {
            if let Some(v) = v {
                self.trim_end();
                self.lines.push(format!("{}:", quote_key(parent)));
                self.lines.extend(entry_lines(&quote_key(key), v, 2));
            }
            return;
        };
        let top = self.unflow(top);
        let indent = self.child_indent(top).unwrap_or(2);
        let at = self.children(top).into_iter().find(|(c, _)| c == key);
        match (at, v) {
            (Some((_, s)), Some(v)) => {
                self.lines
                    .splice(s.start..s.end, entry_lines(&quote_key(key), v, indent));
            }
            (None, Some(v)) => {
                // A parent with a scalar of its own (`services: x`) becomes a block.
                let first = &self.lines[top.start];
                if first
                    .split_once(':')
                    .is_some_and(|(_, r)| !strip_comment(r).trim().is_empty())
                {
                    self.lines[top.start] = format!("{}:", quote_key(parent));
                }
                let end = self.entry_end(top.start, 0);
                self.lines
                    .splice(end..end, entry_lines(&quote_key(key), v, indent));
            }
            (Some((_, s)), None) => {
                self.lines.drain(s.start..s.end);
                // A parent left with nothing goes too (a value or comment keeps it).
                let top = self.top(parent).expect("still there");
                let empty_line = self.lines[top.start]
                    .split_once(':')
                    .is_some_and(|(_, r)| r.trim().is_empty());
                if empty_line && top.end == top.start + 1 {
                    self.lines.remove(top.start);
                }
            }
            (None, None) => {}
        }
    }

    fn trim_end(&mut self) {
        while self.lines.last().is_some_and(|l| l.trim().is_empty()) {
            self.lines.pop();
        }
    }
}

fn entry_lines(key: &str, v: &Val, indent: usize) -> Vec<String> {
    let pad = " ".repeat(indent);
    match v {
        Val::Inline(s) => vec![format!("{pad}{key}: {s}")],
        Val::Block(body) => {
            let mut out = vec![format!("{pad}{key}:")];
            out.extend(body.lines().map(str::to_string));
            out
        }
    }
}

fn indent_of(l: &str) -> usize {
    l.len() - l.trim_start_matches([' ', '\t']).len()
}

/// The key on a top-level line (not a comment, a list item or a document mark).
fn top_key(l: &str) -> Option<String> {
    if l.starts_with([' ', '\t', '#', '-']) || l.starts_with("...") {
        return None;
    }
    key_of(l)
}

/// The key of `key: …` (quoted or not), on a line without its indent.
fn key_of(t: &str) -> Option<String> {
    if t.starts_with(['#', '-']) {
        return None;
    }
    let (k, rest) = if let Some(q) = t.strip_prefix('"') {
        let end = q.find('"')?;
        (q[..end].to_string(), &q[end + 1..])
    } else if let Some(q) = t.strip_prefix('\'') {
        let end = q.find('\'')?;
        (q[..end].to_string(), &q[end + 1..])
    } else {
        let i = t.find(':')?;
        (t[..i].trim_end().to_string(), &t[i..])
    };
    let rest = rest.trim_start();
    let after = rest.strip_prefix(':')?;
    (after.is_empty() || after.starts_with([' ', '\t'])).then_some(k)
}

fn quote_key(k: &str) -> String {
    let plain = !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if plain {
        k.to_string()
    } else {
        serde_json::to_string(k).unwrap_or_default()
    }
}

fn unquote(k: &str) -> String {
    k.trim_matches(['"', '\'']).to_string()
}

/// `v` up to a ` #` comment outside quotes.
fn strip_comment(v: &str) -> &str {
    let (mut dq, mut sq, mut prev) = (false, false, ' ');
    for (i, c) in v.char_indices() {
        match c {
            '"' if !sq && prev != '\\' => dq = !dq,
            '\'' if !dq => sq = !sq,
            '#' if !dq && !sq && prev.is_whitespace() => return &v[..i],
            _ => {}
        }
        prev = c;
    }
    v
}

/// The entries of a flow mapping's inside, split at the commas outside quotes and brackets.
fn split_flow(s: &str) -> Vec<String> {
    let (mut out, mut cur) = (Vec::new(), String::new());
    let (mut depth, mut dq, mut sq) = (0i32, false, false);
    for c in s.chars() {
        match c {
            '"' if !sq => dq = !dq,
            '\'' if !dq => sq = !sq,
            '[' | '{' if !dq && !sq => depth += 1,
            ']' | '}' if !dq && !sq => depth -= 1,
            ',' if depth == 0 && !dq && !sq => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out.into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "# Set by the launcher.";

    fn run(text: &str, edits: &[Edit], notes: &Notes) -> (String, Notes) {
        apply_to(text, edits, notes)
    }

    #[test]
    fn keeps_what_it_doesnt_set() {
        let text = "# mine\nfoo: 1 # a note\nnetwork:\n  # the port\n  foo: bar\n  tick_rate_hz: 60\nplugins: []\n";
        let (out, notes) = run(
            text,
            &[
                Edit::inline(&["network", "tick_rate_hz"], "120"),
                Edit::inline(&["network", "external_ip"], "\"1.2.3.4\""),
                Edit::inline(&["services", "socket_uri"], "\"ws://h\""),
            ],
            &Notes::default(),
        );
        assert_eq!(
            out,
            "# mine\nfoo: 1 # a note\nnetwork:\n  # the port\n  foo: bar\n  tick_rate_hz: 120\n  external_ip: \"1.2.3.4\"\nplugins: []\nservices:\n  socket_uri: \"ws://h\"\n"
        );
        let n = &notes.keys["network.tick_rate_hz"];
        assert_eq!((n.ours.as_str(), n.before.as_deref()), ("120", Some("60")));
        assert_eq!(notes.keys["network.external_ip"].before, None);
    }

    #[test]
    fn takes_back_only_its_own_and_brings_the_players_back() {
        let text = "network:\n  tick_rate_hz: 60\n";
        let set = [
            Edit::inline(&["network", "tick_rate_hz"], "120"),
            Edit::inline(&["network", "external_ip"], "\"1.2.3.4\""),
        ];
        let (out, notes) = run(text, &set, &Notes::default());
        let clear = [
            Edit::none(&["network", "tick_rate_hz"]),
            Edit::none(&["network", "external_ip"]),
        ];
        let (back, n2) = run(&out, &clear, &notes);
        assert_eq!(back, text);
        assert!(n2.keys.is_empty());

        // Changed by the player since: it stays theirs.
        let changed = out.replace("\"1.2.3.4\"", "\"5.6.7.8\"");
        let (kept, n3) = run(&changed, &clear, &notes);
        assert!(kept.contains("external_ip: \"5.6.7.8\""));
        assert!(kept.contains("tick_rate_hz: 60"));
        assert!(n3.keys.is_empty());

        // A parent the launcher made goes with its last key.
        let (out, notes) = run("", &set[1..], &Notes::default());
        assert_eq!(out, "network:\n  external_ip: \"1.2.3.4\"\n");
        assert_eq!(run(&out, &clear[1..], &notes).0, "");
    }

    #[test]
    fn sets_again_without_losing_what_was_before() {
        let text = "network:\n  tick_rate_hz: 60\n";
        let e = |v: &str| [Edit::inline(&["network", "tick_rate_hz"], v)];
        let (out, n) = run(text, &e("120"), &Notes::default());
        let (out, n) = run(&out, &e("90"), &n);
        assert_eq!(n.keys["network.tick_rate_hz"].before.as_deref(), Some("60"));
        let (back, _) = run(&out, &[Edit::none(&["network", "tick_rate_hz"])], &n);
        assert_eq!(back, text);
    }

    #[test]
    fn a_block_at_the_top_level() {
        let list = "  - name: \"a\"\n    file: \"a.dll\"\n    enabled: true\n";
        let text = "x: 1\nplugins:\n- name: old\n  file: old.dll\n# after\ny: 2\n";
        let (out, notes) = run(
            text,
            &[Edit::set(&["plugins"], Val::Block(list.into()))],
            &Notes::default(),
        );
        assert_eq!(out, format!("x: 1\nplugins:\n{list}# after\ny: 2\n"));
        let (back, _) = run(&out, &[Edit::none(&["plugins"])], &notes);
        assert_eq!(back, text);
        // Inline again (none on).
        let (out, _) = run(&out, &[Edit::inline(&["plugins"], "[]")], &Notes::default());
        assert_eq!(out, "x: 1\nplugins: []\n# after\ny: 2\n");
    }

    #[test]
    fn a_flow_parent_becomes_a_block() {
        let (out, _) = run(
            "services: {}\nauth: {http_uri: \"x\", 'k': [1, 2]} # c\n",
            &[
                Edit::inline(&["services", "socket_uri"], "\"ws://h\""),
                Edit::inline(&["auth", "server_key"], "\"s\""),
            ],
            &Notes::default(),
        );
        assert_eq!(
            out,
            "services:\n  socket_uri: \"ws://h\"\nauth:\n  http_uri: \"x\"\n  k: [1, 2]\n  server_key: \"s\"\n"
        );
    }

    #[test]
    fn crlf_bom_and_header() {
        let text = "\u{feff}# Written by the Echo VR launcher before every start.\r\n# Changes here are replaced.\r\n# mine\r\nfoo: 1\r\n";
        let (out, _) = run(text, &[Edit::inline(&["bar"], "2")], &Notes::default());
        let out = with_header(
            &out,
            H,
            &["# Written by the Echo VR launcher", "# Changes here"],
        );
        assert_eq!(
            out,
            format!("\u{feff}{H}\r\n# mine\r\nfoo: 1\r\nbar: 2\r\n")
        );
        assert_eq!(with_header(&out, H, &[]), out);
        assert_eq!(with_header("", H, &[]), format!("{H}\n"));
    }

    #[test]
    fn writes_only_what_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "mine: 1\n").unwrap();
        let e = [Edit::inline(&["x-local-plugins"], "true")];
        let out = apply(&path, &e, H, &[]).unwrap();
        assert_eq!(out, format!("{H}\nmine: 1\nx-local-plugins: true\n"));
        assert_eq!(read_notes(&path).keys["x-local-plugins"].ours, "true");
        let at = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        apply(&path, &e, H, &[]).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), at);
        apply(&path, &[Edit::none(&["x-local-plugins"])], H, &[]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{H}\nmine: 1\n")
        );
    }
}
