//! Alerts about the servers: one stopped on an error (a refused sign-in in particular),
//! one that keeps restarting without hosting a match, one that should run but doesn't,
//! one that waits for its sign-in code to be approved, and one that recovered. On the desktop and, when set, to a Discord webhook. After
//! evr-docker's (Echo-VR-Server-on-Docker `pkg/notify`).

use std::collections::{HashMap, VecDeque};

use serde_json::json;

use super::events::Reason;
use super::status::{State, Status};
use super::Alerts;

/// The same alert again only after this (seconds).
const QUIET_S: i64 = 600;
/// Restarts in this window (seconds) without a match make a crash loop.
const LOOP_WINDOW_S: i64 = 900;
const LOOP_RESTARTS: usize = 5;
/// A server that should run and doesn't for this long is down.
const DOWN_S: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Failed,
    CredentialsFailed,
    CrashLoop,
    Down,
    /// It shows a code for its sign-in and waits until it is approved.
    SignInCode,
    Recovered,
}

/// An alert to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub kind: Kind,
    pub server: String,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Default)]
struct Seen {
    state: State,
    restarts: u32,
    matches: u32,
    /// When restarts happened (Unix seconds), within the window.
    restart_times: VecDeque<i64>,
    /// It had a problem (an alert went out): its next registration is a recovery.
    troubled: bool,
    /// Since when it should run and doesn't.
    down_since: Option<i64>,
}

/// Turns what the servers do into alerts.
#[derive(Debug, Default)]
pub struct Watch {
    seen: HashMap<String, Seen>,
    sent: HashMap<(Kind, String), i64>,
}

impl Watch {
    /// Pure: the alerts from `s` (its server should run: `wanted`) at `now`. The first look
    /// at a server only takes note.
    pub fn look(&mut self, s: &Status, wanted: bool, now: i64) -> Vec<Alert> {
        let mut out = Vec::new();
        let first = !self.seen.contains_key(&s.name);
        let seen = self.seen.entry(s.name.clone()).or_default();
        if first {
            seen.state = s.state;
            seen.restarts = s.restarts;
            seen.matches = s.matches;
            return out;
        }
        let alert = |kind: Kind, title: String, text: String| Alert {
            kind,
            server: s.name.clone(),
            title,
            text,
        };
        if s.restarts > seen.restarts {
            for _ in seen.restarts..s.restarts {
                seen.restart_times.push_back(now);
            }
        }
        if s.matches > seen.matches {
            seen.restart_times.clear();
        }
        while seen
            .restart_times
            .front()
            .is_some_and(|&t| now - t > LOOP_WINDOW_S)
        {
            seen.restart_times.pop_front();
        }
        if s.state == State::Failed && seen.state != State::Failed {
            seen.troubled = true;
            if s.reason == Some(Reason::BadCredentials) {
                out.push(alert(
                    Kind::CredentialsFailed,
                    format!("{}: EchoVRCE refused its sign-in", s.name),
                    "Check the servers' account on the Server host page.".into(),
                ));
            } else {
                let why = s.reason.map_or("an error", Reason::words);
                out.push(alert(
                    Kind::Failed,
                    format!("{} stopped", s.name),
                    format!("{why}. {}", short(&s.last_error)),
                ));
            }
        }
        if s.state == State::AwaitingApproval && seen.state != State::AwaitingApproval {
            // Not the code itself: whoever approves it, the server plays as.
            out.push(alert(
                Kind::SignInCode,
                format!("{} waits for its sign-in", s.name),
                "Approve its code: APPROVE on the Server host page, signed in on echovrce.com as the servers' account.".into(),
            ));
        }
        if seen.restart_times.len() >= LOOP_RESTARTS && !s.last_error.is_empty() {
            seen.troubled = true;
            out.push(alert(
                Kind::CrashLoop,
                format!("{} keeps restarting", s.name),
                format!(
                    "{} restarts in {} minutes without a match. Last: {}",
                    seen.restart_times.len(),
                    LOOP_WINDOW_S / 60,
                    short(&s.last_error)
                ),
            ));
        }
        let running = s.fresh(now) && !matches!(s.state, State::Stopped | State::Failed);
        if wanted && !running {
            let since = *seen.down_since.get_or_insert(now);
            if now - since > DOWN_S && s.state != State::Failed {
                seen.troubled = true;
                out.push(alert(
                    Kind::Down,
                    format!("{} is down", s.name),
                    "It should run, but hasn't for over a minute.".into(),
                ));
            }
        } else {
            seen.down_since = None;
        }
        if s.state == State::Registered && seen.state != State::Registered && seen.troubled {
            seen.troubled = false;
            out.push(alert(
                Kind::Recovered,
                format!("{} is back", s.name),
                format!("Registered at {}:{}.", s.public_ip, s.port),
            ));
        }
        seen.state = s.state;
        seen.restarts = s.restarts;
        seen.matches = s.matches;
        out.retain(|a| {
            let key = (a.kind, a.server.clone());
            match self.sent.get(&key) {
                Some(&t) if now - t < QUIET_S => false,
                _ => {
                    self.sent.insert(key, now);
                    true
                }
            }
        });
        out
    }

    /// A server that is gone (deleted).
    pub fn forget(&mut self, name: &str) {
        self.seen.remove(name);
    }
}

fn short(text: &str) -> String {
    let t: String = text.chars().take(300).collect();
    if t.len() < text.len() {
        format!("{t}…")
    } else {
        t
    }
}

/// Pure: the Discord webhook's message for `a`.
pub fn webhook_body(a: &Alert, host: &str) -> serde_json::Value {
    let color = match a.kind {
        Kind::Recovered => 0x1bbd1b,
        Kind::Down | Kind::CrashLoop | Kind::SignInCode => 0xd69614,
        Kind::Failed | Kind::CredentialsFailed => 0xd62c40,
    };
    json!({
        "username": "Echo VR servers",
        "embeds": [{
            "title": a.title,
            "description": a.text,
            "color": color,
            "footer": { "text": host },
            "timestamp": time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        }]
    })
}

/// Pure: whether `url` is a Discord webhook.
pub fn is_webhook(url: &str) -> bool {
    let u = url.trim();
    [
        "https://discord.com/api/webhooks/",
        "https://discordapp.com/api/webhooks/",
        "https://ptb.discord.com/api/webhooks/",
        "https://canary.discord.com/api/webhooks/",
    ]
    .iter()
    .any(|p| u.starts_with(p))
}

/// This PC's name, for the webhook's footer.
pub fn host_name() -> String {
    sysinfo::System::host_name().unwrap_or_else(|| "this PC".into())
}

/// Sends `a` where `alerts` say.
pub fn send(alerts: &Alerts, a: &Alert) {
    tracing::info!("server host alert: {}: {}", a.title, a.text);
    if alerts.desktop {
        crate::core::desktop_notify::show(&a.title, &a.text);
    }
    if is_webhook(&alerts.webhook) {
        if let Err(e) = post(&alerts.webhook, &webhook_body(a, &host_name())) {
            tracing::warn!("server host: the Discord webhook: {e:#}");
        }
    }
}

/// Posts to a webhook; an error unless Discord took it.
pub fn post(url: &str, body: &serde_json::Value) -> anyhow::Result<()> {
    let (status, text) = crate::core::http::post_json(url.trim(), body)?;
    if !(200..300).contains(&status) {
        anyhow::bail!("Discord answered {status}: {}", short(&text));
    }
    Ok(())
}

/// The test message Settings sends when a webhook is saved.
pub fn test_alert() -> Alert {
    Alert {
        kind: Kind::Recovered,
        server: String::new(),
        title: "Echo VR servers: alerts work".into(),
        text: format!("The Server host on {} sends its alerts here.", host_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(state: State, restarts: u32, matches: u32, now: i64) -> Status {
        Status {
            name: "echo-1".into(),
            state,
            restarts,
            matches,
            updated: now,
            last_error: "boom".into(),
            ..Status::default()
        }
    }

    #[test]
    fn says_when_a_server_fails_and_recovers() {
        let mut w = Watch::default();
        assert!(w.look(&st(State::Registered, 0, 0, 0), true, 0).is_empty());
        let mut f = st(State::Failed, 1, 0, 10);
        f.reason = Some(Reason::BadCredentials);
        let a = w.look(&f, true, 10);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, Kind::CredentialsFailed);
        // Not again while it stays failed.
        assert!(w.look(&f, true, 20).is_empty());
        let a = w.look(&st(State::Registered, 2, 0, 30), true, 30);
        assert_eq!(a[0].kind, Kind::Recovered);
    }

    #[test]
    fn asks_once_to_approve_a_code() {
        let mut w = Watch::default();
        w.look(&st(State::Starting, 0, 0, 0), true, 0);
        let a = w.look(&st(State::AwaitingApproval, 0, 0, 5), true, 5);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, Kind::SignInCode);
        assert!(!a[0].text.contains("code="), "no code in an alert");
        // A new code (the old one ran out): no new alert.
        assert!(w
            .look(&st(State::AwaitingApproval, 0, 0, 310), true, 310)
            .is_empty());
        // Approved and registered: nothing went wrong, no "is back".
        assert!(w
            .look(&st(State::Registered, 0, 0, 320), true, 320)
            .is_empty());
    }

    #[test]
    fn sees_a_crash_loop_once() {
        let mut w = Watch::default();
        w.look(&st(State::Starting, 0, 0, 0), true, 0);
        let mut kinds = Vec::new();
        for i in 1..=7 {
            kinds.extend(
                w.look(
                    &st(State::Backoff, i, 0, i as i64 * 10),
                    true,
                    i as i64 * 10,
                )
                .into_iter()
                .map(|a| a.kind),
            );
        }
        assert_eq!(kinds, vec![Kind::CrashLoop]);
    }

    #[test]
    fn sees_a_server_down() {
        let mut w = Watch::default();
        w.look(&st(State::Stopped, 0, 0, 0), true, 0);
        assert!(w.look(&st(State::Stopped, 0, 0, 0), true, 30).is_empty());
        let a = w.look(&st(State::Stopped, 0, 0, 0), true, 100);
        assert_eq!(a[0].kind, Kind::Down);
        // Not wanted: no alert.
        let mut w = Watch::default();
        w.look(&st(State::Stopped, 0, 0, 0), false, 0);
        assert!(w.look(&st(State::Stopped, 0, 0, 0), false, 500).is_empty());
    }

    #[test]
    fn webhooks() {
        assert!(is_webhook("https://discord.com/api/webhooks/1/x"));
        assert!(!is_webhook("http://discord.com/api/webhooks/1/x"));
        assert!(!is_webhook("https://example.com/"));
        let b = webhook_body(&test_alert(), "pc");
        assert_eq!(b["username"], "Echo VR servers");
        assert_eq!(b["embeds"][0]["footer"]["text"], "pc");
    }
}
