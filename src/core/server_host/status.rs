//! What the supervisor and the window share, per server, as files in its folder:
//! `status.json` (the supervisor writes it at every change and every 15 seconds),
//! `control` (the window asks for a start, stop or restart) and `console.log` (what the
//! server said, for the console view).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::events::Reason;

pub const STATUS: &str = "status.json";
pub const CONTROL: &str = "control";
pub const CONSOLE: &str = "console.log";
/// The console log moves to `console.log.1` at this size.
const CONSOLE_MAX: u64 = 8 * 1024 * 1024;
/// How often the supervisor rewrites a status that didn't change.
pub const HEARTBEAT_S: i64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Started, not registered yet.
    Starting,
    /// It shows a code for its sign-in (no account, nEVR 6.9.0 on) and waits until it is
    /// approved on echovrce.com: [`Status::sign_in_url`].
    AwaitingApproval,
    /// Waiting for a match.
    Registered,
    /// Hosting a match.
    InSession,
    Stopping,
    /// Waiting before it starts again after an error.
    Backoff,
    /// Stopped on an error a restart can't fix (sign-in, settings): waits for a change.
    Failed,
    #[default]
    Stopped,
}

impl State {
    /// Whether it can be restarted without dropping players.
    pub fn idle(self) -> bool {
        !matches!(self, State::InSession | State::Stopping)
    }

    pub fn words(self) -> &'static str {
        match self {
            State::Starting => "Starting",
            State::AwaitingApproval => "Approve its sign-in",
            State::Registered => "Ready",
            State::InSession => "In a match",
            State::Stopping => "Stopping",
            State::Backoff => "Waiting to restart",
            State::Failed => "Stopped on an error",
            State::Stopped => "Stopped",
        }
    }
}

/// A server as the supervisor last saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Status {
    pub name: String,
    pub state: State,
    /// Since when it is in that state (Unix seconds).
    pub since: i64,
    /// When this was written (Unix seconds).
    pub updated: i64,
    pub pid: u32,
    pub port: u16,
    /// The address EchoVRCE registered it at.
    pub public_ip: String,
    /// nEVR's version, from its log.
    pub runtime: String,
    /// The match it hosts.
    pub session: String,
    pub matches: u32,
    pub restarts: u32,
    pub registrations: u32,
    pub last_exit: Option<i32>,
    pub reason: Option<Reason>,
    /// What went wrong, as the log said it.
    pub last_error: String,
    /// When it starts again (Backoff, Failed), Unix seconds.
    pub next_start: i64,
    /// AwaitingApproval: the page that approves its code (the code in it), and until when
    /// (Unix seconds) before it shows a new one.
    pub sign_in_url: String,
    pub sign_in_expires: i64,
}

impl Status {
    /// Whether the supervisor wrote it lately (else it is gone, and this is old).
    pub fn fresh(&self, now: i64) -> bool {
        now - self.updated <= 3 * HEARTBEAT_S
    }

    /// The overview's words for what it is doing.
    pub fn summary(&self, now: i64) -> String {
        if !self.fresh(now) && self.state != State::Stopped {
            return "Not running (the server host isn't running)".into();
        }
        match (self.state, self.reason) {
            (State::Failed | State::Backoff, Some(r)) => {
                format!("{}: {}", self.state.words(), r.words())
            }
            (s, _) => s.words().to_string(),
        }
    }
}

pub fn status_path(dir: &Path) -> PathBuf {
    dir.join(STATUS)
}

/// The status in `dir`, if there is one.
pub fn read(dir: &Path) -> Option<Status> {
    let text = std::fs::read_to_string(status_path(dir)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes `s` into `dir` (through a `.part` and a rename, so a reader never sees half).
pub fn write(dir: &Path, s: &Status) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = status_path(dir);
    let part = path.with_extension("json.part");
    std::fs::write(&part, serde_json::to_string_pretty(s)?)?;
    std::fs::rename(&part, &path).with_context(|| format!("replace {}", path.display()))
}

/// What the window asks a server's supervisor for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Start it again now (also out of a wait after an error).
    Restart,
    /// Stop it now, even in a match.
    StopNow,
    /// Its settings changed: use them at its next start (after a match), or now when idle.
    Apply,
}

impl Command {
    fn word(self) -> &'static str {
        match self {
            Command::Restart => "restart",
            Command::StopNow => "stop-now",
            Command::Apply => "apply",
        }
    }

    fn from_word(w: &str) -> Option<Command> {
        match w.trim() {
            "restart" => Some(Command::Restart),
            "stop-now" => Some(Command::StopNow),
            "apply" => Some(Command::Apply),
            _ => None,
        }
    }
}

/// The window: asks the server in `dir` for `c`.
pub fn ask(dir: &Path, c: Command) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(CONTROL))?;
    writeln!(f, "{}", c.word())?;
    Ok(())
}

/// The supervisor: what was asked for the server in `dir` (once).
pub fn take(dir: &Path) -> Vec<Command> {
    let path = dir.join(CONTROL);
    let taken = path.with_extension("taken");
    if std::fs::rename(&path, &taken).is_err() {
        return Vec::new();
    }
    let text = std::fs::read_to_string(&taken).unwrap_or_default();
    let _ = std::fs::remove_file(&taken);
    text.lines().filter_map(Command::from_word).collect()
}

/// Where a server's console goes: appended to `console.log`, which moves to `.1` when it
/// gets big.
pub struct Console {
    path: PathBuf,
    file: Option<std::fs::File>,
    size: u64,
}

impl Console {
    pub fn open(dir: &Path) -> Console {
        let path = dir.join(CONSOLE);
        let _ = std::fs::create_dir_all(dir);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Console { path, file, size }
    }

    /// One line, as it came.
    pub fn line(&mut self, text: &str) {
        if self.size > CONSOLE_MAX {
            self.file = None;
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
            self.file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok();
            self.size = 0;
        }
        if let Some(f) = &mut self.file {
            let line = format!("{}\n", text.trim_end_matches(['\r', '\n']));
            if f.write_all(line.as_bytes()).is_ok() {
                self.size += line.len() as u64;
            }
        }
    }

    /// One of the supervisor's own lines (`[host] …`).
    pub fn host(&mut self, text: &str) {
        self.line(&format!("[host] {text}"));
    }
}

/// The window's view of a console log: reads what was added since the last call.
#[derive(Debug, Default)]
pub struct Tail {
    path: PathBuf,
    pos: u64,
    /// The file's start, to tell a new one (moved to `.1`) from the old.
    head: Vec<u8>,
    partial: String,
}

impl Tail {
    /// Follows `dir`'s console log, from its last `back` bytes.
    pub fn new(dir: &Path, back: u64) -> Tail {
        let path = dir.join(CONSOLE);
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Tail {
            path,
            pos: len.saturating_sub(back),
            head: Vec::new(),
            partial: String::new(),
        }
    }

    /// Follows the file `path` from its start.
    pub fn file(path: &Path) -> Tail {
        Tail {
            path: path.to_path_buf(),
            ..Tail::default()
        }
    }

    /// The whole lines added since the last call.
    pub fn read(&mut self) -> Vec<String> {
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let mut head = vec![0u8; 64.min(len as usize)];
        let _ = f.read_exact(&mut head);
        if len < self.pos
            || (!self.head.is_empty()
                && !head.starts_with(&self.head[..self.head.len().min(head.len())]))
        {
            // A new file: from its start.
            self.pos = 0;
            self.partial.clear();
        }
        self.head = head;
        if f.seek(SeekFrom::Start(self.pos)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        if f.take(4 * 1024 * 1024).read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        self.pos += buf.len() as u64;
        let text = format!("{}{}", self.partial, String::from_utf8_lossy(&buf));
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        self.partial = lines.pop().unwrap_or_default();
        lines
            .into_iter()
            .map(|l| l.trim_end_matches('\r').to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trip_and_freshness() {
        let dir = tempfile::tempdir().unwrap();
        let s = Status {
            name: "echo-1".into(),
            state: State::Registered,
            updated: 100,
            ..Status::default()
        };
        write(dir.path(), &s).unwrap();
        assert_eq!(read(dir.path()).unwrap(), s);
        assert!(s.fresh(140));
        assert!(!s.fresh(200));
        assert!(s.summary(200).starts_with("Not running"));
        assert_eq!(s.summary(110), "Ready");
        assert!(!State::InSession.idle());
    }

    #[test]
    fn commands_are_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        ask(dir.path(), Command::Apply).unwrap();
        ask(dir.path(), Command::Restart).unwrap();
        assert_eq!(take(dir.path()), vec![Command::Apply, Command::Restart]);
        assert!(take(dir.path()).is_empty());
    }

    #[test]
    fn tails_the_console() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Console::open(dir.path());
        c.line("one");
        let mut t = Tail::new(dir.path(), 1 << 20);
        assert_eq!(t.read(), vec!["one"]);
        c.host("two\r\n");
        assert_eq!(t.read(), vec!["[host] two"]);
        assert!(t.read().is_empty());
    }
}
