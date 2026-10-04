//! What EchoVRCE says when it turns a login down, read from the game's log.
//!
//! The game shows the service's message in a box of four lines, and EchoVRCE's often has
//! more: "Please authorize this new location. Check your Discord DMs from @EchoVRCE." and,
//! on the fifth line, the code to pick there. The game's log (`_local/r14logs`, a new file
//! at every start) has all of it:
//!
//! ```text
//! [10-04-2026] [16:03:41]: [LOGIN] [XPID:OVR-ORG-1513203222339391610 /
//! Discord:1513203222339391610]
//!  Please authorize this new location.
//! Check your Discord DMs from @EchoVRCE.
//! Select code >>> 58 <<<
//! ```
//!
//! [`LoginWatch`] follows the newest log of a start and hands out each `[LOGIN]` block once
//! it is complete (the next line has a time again, or nothing more came).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::core::paths;

/// A login the service turned down, and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginNotice {
    /// EchoVRCE wants this PC confirmed in its Discord DM, by picking this code.
    pub code: Option<String>,
    /// The message without the account line, one line per line.
    pub message: String,
}

impl LoginNotice {
    /// Whether it asks you to confirm this location (a new IP) in Discord.
    pub fn confirm_location(&self) -> bool {
        self.code.is_some()
            || self
                .message
                .to_ascii_lowercase()
                .contains("authorize this new location")
    }
}

/// Whether `line` starts a log entry: `[MM-DD-YYYY] [HH:MM:SS]: `.
fn stamped(line: &str) -> bool {
    let b = line.as_bytes();
    b.len() >= 25
        && b[0] == b'['
        && b[3] == b'-'
        && b[6] == b'-'
        && b[11] == b']'
        && b[13] == b'['
        && b[22] == b']'
        && b[23] == b':'
}

/// Pure: a `[LOGIN]` block (its text after `[LOGIN]`, then its continuation lines) as a
/// notice. `None` when there is no message in it.
pub fn parse_block(text: &str) -> Option<LoginNotice> {
    // Drop the account line: "[XPID:… / Discord:…]", which may wrap.
    let rest = match text.trim_start().strip_prefix("[XPID:") {
        Some(r) => r.split_once(']').map_or("", |(_, after)| after),
        None => text,
    };
    let lines: Vec<&str> = rest
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let code = lines.iter().find_map(|l| {
        let (_, after) = l.split_once(">>>")?;
        let (code, _) = after.split_once("<<<")?;
        Some(code.trim().to_string()).filter(|c| !c.is_empty())
    });
    Some(LoginNotice {
        code,
        message: lines.join("\n"),
    })
}

/// Pure: the complete `[LOGIN]` blocks in `text` (log lines), and whether one is still
/// open at its end (more lines may follow).
fn blocks(text: &str) -> (Vec<String>, Option<String>) {
    let mut done = Vec::new();
    let mut open: Option<String> = None;
    for line in text.lines() {
        if stamped(line) {
            if let Some(b) = open.take() {
                done.push(b);
            }
            if let Some((_, after)) = line.split_once("]: [LOGIN]") {
                open = Some(after.trim().to_string());
            }
        } else if let Some(b) = open.as_mut() {
            b.push('\n');
            b.push_str(line);
        }
    }
    (done, open)
}

/// Follows the game's log of one start for login notices.
#[derive(Debug)]
pub struct LoginWatch {
    dir: PathBuf,
    /// Only logs written since the start count.
    since: SystemTime,
    file: Option<PathBuf>,
    pos: u64,
    /// Lines read but not yet ending in a newline, and the block still open.
    partial: String,
    open: Option<String>,
}

impl LoginWatch {
    /// For the game at install `root`, started at `since`.
    pub fn new(root: &str, since: SystemTime) -> LoginWatch {
        LoginWatch {
            dir: Path::new(root)
                .join(paths::ARENA_DIR)
                .join("_local/r14logs"),
            since,
            file: None,
            pos: 0,
            partial: String::new(),
            open: None,
        }
    }

    /// Reads what was added since the last call (file I/O: cheap, the log is small).
    /// Returns the notices that are complete now.
    pub fn poll(&mut self) -> Vec<LoginNotice> {
        if self.file.is_none() {
            self.file = newest_since(&self.dir, self.since);
        }
        let Some(file) = &self.file else {
            return Vec::new();
        };
        let mut added = String::new();
        if let Ok(mut f) = std::fs::File::open(file) {
            if f.seek(SeekFrom::Start(self.pos)).is_ok() {
                let mut buf = Vec::new();
                if f.read_to_end(&mut buf).is_ok() {
                    self.pos += buf.len() as u64;
                    added = String::from_utf8_lossy(&buf).into_owned();
                }
            }
        }
        let quiet = added.is_empty();
        self.partial.push_str(&added);
        // Only whole lines; the rest waits for the next read.
        let cut = self.partial.rfind('\n').map_or(0, |i| i + 1);
        let lines: String = self.partial.drain(..cut).collect();
        let mut text = String::new();
        if let Some(open) = self.open.take() {
            // Its first line was a stamped one: put it back in front.
            text.push_str("[00-00-0000] [00:00:00]: [LOGIN] ");
            text.push_str(&open);
            text.push('\n');
        }
        text.push_str(&lines);
        let (done, open) = blocks(&text);
        let mut out: Vec<LoginNotice> = done.iter().filter_map(|b| parse_block(b)).collect();
        match open {
            // Nothing more came: the game shows the message and waits. It's complete.
            Some(b) if quiet => out.extend(parse_block(&b)),
            other => self.open = other,
        }
        out
    }
}

/// The newest log in `dir` written since `since`.
fn newest_since(dir: &Path, since: SystemTime) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let t = e.metadata().ok()?.modified().ok()?;
            (t >= since && e.file_name().to_string_lossy().ends_with(".log")).then(|| (e.path(), t))
        })
        .max_by_key(|(_, t)| *t)
        .map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As the game wrote it on the Linux test box (2026-10-04).
    const LOG: &str = "[10-04-2026] [16:03:40]: ======= Build: goldmaster 631547 from //rad/rad15_live('r') =======\n\
[10-04-2026] [16:03:40]: [SYSNET] Found Internet connection (IPv4)\n\
[10-04-2026] [16:03:41]: [LOGIN] [XPID:OVR-ORG-1513203222339391610 /\n\
Discord:1513203222339391610]\n \
Please authorize this new location.\n\
Check your Discord DMs from @EchoVRCE.\n\
Select code >>> 58 <<<\n";

    #[test]
    fn reads_the_location_code() {
        let (done, open) = blocks(LOG);
        assert!(done.is_empty());
        let n = parse_block(&open.unwrap()).unwrap();
        assert_eq!(n.code.as_deref(), Some("58"));
        assert!(n.confirm_location());
        assert_eq!(
            n.message,
            "Please authorize this new location.\nCheck your Discord DMs from @EchoVRCE.\nSelect code >>> 58 <<<"
        );
    }

    #[test]
    fn other_messages_and_stamps() {
        let n = parse_block("[XPID:OVR-ORG-1 / Discord:1]\nThis account is suspended.").unwrap();
        assert_eq!(n.code, None);
        assert!(!n.confirm_location());
        assert_eq!(n.message, "This account is suspended.");
        assert!(parse_block("[XPID:OVR-ORG-1 /\nDiscord:1]\n").is_none());
        assert!(stamped("[10-04-2026] [16:03:41]: [LOGIN] x"));
        assert!(!stamped("Discord:1513203222339391610]"));
        // A block ends at the next stamped line.
        let (done, open) = blocks(&format!("{LOG}[10-04-2026] [16:03:42]: [NETGAME] x\n"));
        assert_eq!(done.len(), 1);
        assert!(open.is_none());
    }

    #[test]
    fn follows_the_log_as_it_grows() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join(paths::ARENA_DIR).join("_local/r14logs");
        std::fs::create_dir_all(&logs).unwrap();
        let since = SystemTime::now() - std::time::Duration::from_secs(5);
        let mut w = LoginWatch::new(&dir.path().to_string_lossy(), since);
        assert!(w.poll().is_empty());
        let file = logs.join("[r14(client)]-[10-04-2026]_[16-03-37]_312.log");
        let (head, tail) = LOG.split_at(LOG.find("Check your").unwrap());
        std::fs::write(&file, head).unwrap();
        // Lines are still coming in: the block stays open.
        assert!(w.poll().is_empty());
        let mut all = head.to_string();
        all.push_str(tail);
        std::fs::write(&file, &all).unwrap();
        assert!(w.poll().is_empty());
        // Nothing new: the game waits with the message. Once, then never again.
        let n = w.poll();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].code.as_deref(), Some("58"));
        assert!(w.poll().is_empty());
    }
}
