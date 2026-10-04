//! argv-based process execution. No shell is ever involved, so arguments are never
//! re-parsed (paths with spaces are safe on every platform). Both output streams are
//! drained on their own threads so a chatty child can never deadlock on a full pipe.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct Output {
    /// Exit code, `None` when the process could not be started or was killed by a signal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Set when the process could not be spawned at all.
    pub spawn_error: Option<String>,
    pub duration: Duration,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout followed by stderr, the way `redirectErrorStream(true)` used to combine them.
    pub fn combined(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout.trim_end_matches('\n'), self.stderr)
        }
    }
}

/// Builds a `Command` that never pops up a console window on Windows.
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs `program args...` to completion and captures both streams.
pub fn run<S: AsRef<OsStr>>(program: impl AsRef<OsStr>, args: &[S], cwd: Option<&Path>) -> Output {
    let mut cmd = command(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    run_command(cmd)
}

pub fn run_command(mut cmd: Command) -> Output {
    let start = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Output {
                spawn_error: Some(e.to_string()),
                duration: start.elapsed(),
                ..Default::default()
            }
        }
    };
    let pump = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut s) = stream {
                let _ = s.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = pump(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let err = pump(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let status = child.wait();
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    Output {
        code: status.ok().and_then(|s| s.code()),
        stdout,
        stderr,
        spawn_error: None,
        duration: start.elapsed(),
    }
}

/// Newline-normalized output with over-long lines truncated. `adb push` progress is one
/// enormous `\r`-laden line.
pub fn normalize(output: &str) -> String {
    const MAX_LINE_CHARS: usize = 500;
    output
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|l| {
            if l.chars().count() > MAX_LINE_CHARS {
                let cut: String = l.chars().take(MAX_LINE_CHARS).collect();
                format!("{cut} ...(line truncated)")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Bounds successful output for the log. Failures are logged in full by the callers.
pub fn excerpt(output: &str) -> String {
    const MAX_LINES: usize = 120;
    const HEAD: usize = 80;
    const TAIL: usize = 20;
    let normalized = normalize(output);
    let lines: Vec<&str> = normalized.split('\n').collect();
    if lines.len() <= MAX_LINES {
        return normalized;
    }
    let mut s = lines[..HEAD].join("\n");
    s.push_str(&format!(
        "\n... {} lines omitted ...\n",
        lines.len() - HEAD - TAIL
    ));
    s.push_str(&lines[lines.len() - TAIL..].join("\n"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_splits_carriage_returns_and_truncates() {
        assert_eq!(normalize("a\r\nb\rc"), "a\nb\nc");
        let long = "x".repeat(600);
        assert!(normalize(&long).ends_with("...(line truncated)"));
    }

    #[test]
    fn excerpt_elides_the_middle() {
        let text: String = (0..200).map(|i| format!("line{i}\n")).collect();
        let e = excerpt(&text);
        assert!(e.contains("line0"));
        assert!(e.contains("lines omitted"));
        assert!(e.contains("line199"));
        assert!(!e.contains("line100\n"));
    }

    #[test]
    fn spawn_failure_is_reported() {
        let out = run("definitely-not-a-real-binary-xyz", &["a"], None);
        assert!(out.spawn_error.is_some());
        assert!(!out.success());
    }
}
