//! Important notices for players (`launcher/feed/alerts.json`, written with `echo-alert` on
//! release.echo): a mod found to be harmful, a server problem, anything everyone (or some)
//! must know. Each says how important it is:
//!
//! * `info`: in the top bar while nothing else is said there, until dismissed or expired.
//! * `warning`: the same, plus one desktop notification.
//! * `critical`: a card at start (and when it arrives) that has to be acknowledged, plus one
//!   desktop notification; back at every start until it expires, unless dismissible.
//!
//! An alert can be for some launchers only ([`Target`]): a platform, launcher versions,
//! channels, players who have a mod (in some versions) installed, dev channels. Links go
//! only to a few sites ([`link_ok`]).

use std::collections::BTreeSet;

use serde::Deserialize;
use time::OffsetDateTime;

use super::store::LauncherState;
use crate::core::semver;

pub const URL: &str = "https://release.echovr.de/launcher/feed/alerts.json";

/// The sites an alert may link to (and their subdomains).
const LINK_HOSTS: [&str; 5] = ["echovr.de", "echovrce.com", "github.com", "discord.gg", "discord.com"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Alert {
    pub id: String,
    /// `info`, `warning` or `critical`.
    pub severity: String,
    pub title: String,
    /// Discord markdown.
    pub body: String,
    pub link_url: String,
    pub link_label: String,
    /// From when, until when (RFC 3339, UTC); none: always.
    pub starts: Option<String>,
    pub expires: Option<String>,
    /// The player may dismiss it (a critical one: only if it says so).
    pub dismissible: Option<bool>,
    pub target: Target,
}

/// Who an alert is for: every condition it sets must hold (none: everyone).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Target {
    /// `windows`, `linux`.
    pub platforms: Vec<String>,
    /// Launcher versions, a range (`<0.13.2`).
    pub launcher: String,
    /// `main`, `beta`, `alpha`.
    pub channels: Vec<String>,
    /// Players who have one of these installed.
    pub mods: Vec<ModTarget>,
    /// Players with one of these dev channels (their names).
    pub dev_channels: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ModTarget {
    pub id: String,
    /// A range; empty or `*`: any version.
    pub versions: String,
}

/// What an alert's target is matched against: this launcher and what it has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Here {
    pub platform: String,
    pub launcher: String,
    pub channel: String,
    /// Mods installed in any version: catalogue id and version.
    pub mods: Vec<(String, String)>,
    pub dev_channels: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Alert {
    pub fn severity(&self) -> Severity {
        match self.severity.as_str() {
            "critical" => Severity::Critical,
            "warning" => Severity::Warning,
            _ => Severity::Info,
        }
    }

    /// The player may dismiss it.
    pub fn can_dismiss(&self) -> bool {
        self.dismissible.unwrap_or(self.severity() != Severity::Critical)
    }

    /// The link to show, if it goes to an allowed site.
    pub fn link(&self) -> Option<(&str, &str)> {
        link_ok(&self.link_url).then(|| {
            let label = if self.link_label.trim().is_empty() { "More" } else { self.link_label.as_str() };
            (label, self.link_url.as_str())
        })
    }

    /// Pure: whether it is for `here` at `now`.
    pub fn applies(&self, here: &Here, now: OffsetDateTime) -> bool {
        let t = &self.target;
        let at = |s: &Option<String>| {
            s.as_deref()
                .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        };
        if self.id.is_empty() || self.title.trim().is_empty() {
            return false;
        }
        if at(&self.starts).is_some_and(|s| now < s) || at(&self.expires).is_some_and(|e| now >= e) {
            return false;
        }
        (t.platforms.is_empty() || t.platforms.iter().any(|p| *p == here.platform))
            && (t.launcher.trim().is_empty() || semver::in_range(&here.launcher, &t.launcher, true))
            && (t.channels.is_empty() || t.channels.iter().any(|c| *c == here.channel))
            && (t.mods.is_empty()
                || t.mods.iter().any(|m| {
                    here.mods.iter().any(|(id, version)| {
                        *id == m.id
                            && (matches!(m.versions.trim(), "" | "*")
                                || semver::in_range(version, &m.versions, true))
                    })
                }))
            && (t.dev_channels.is_empty() || t.dev_channels.iter().any(|c| here.dev_channels.contains(c)))
    }
}

/// Pure: whether `url` is https on one of [`LINK_HOSTS`] (or a subdomain).
pub fn link_ok(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
    u.scheme() == "https" && LINK_HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// Pure: the alerts in `text`; one that doesn't read right is left out.
pub fn parse(text: &str) -> Vec<Alert> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        alerts: Vec<serde_json::Value>,
    }
    serde_json::from_str::<Raw>(text)
        .map(|r| r.alerts.into_iter().filter_map(|a| serde_json::from_value(a).ok()).collect())
        .unwrap_or_default()
}

/// This launcher and what it has (reads every installed version's mods).
pub fn here(state: &LauncherState) -> Here {
    let mut mods = BTreeSet::new();
    for v in state.versions.iter().filter(|v| v.present()) {
        for p in super::mods::read(v).plugins {
            if let super::mods::Source::Catalog { id, version } = p.source {
                mods.insert((id, version));
            }
        }
    }
    Here {
        platform: if cfg!(windows) { "windows" } else if cfg!(target_os = "linux") { "linux" } else { "macos" }.into(),
        launcher: env!("CARGO_PKG_VERSION").into(),
        channel: state.launcher_channel.name().into(),
        mods: mods.into_iter().collect(),
        dev_channels: state.dev_codes.iter().filter_map(|c| super::dev_code::channel_name(c)).collect(),
    }
}

/// The alerts for this launcher now (network): none when the file isn't there or can't be read.
pub fn fetch(state: &LauncherState) -> Vec<Alert> {
    let alerts = match crate::core::http::get_text_opt(URL) {
        Ok(Some(t)) => parse(&t),
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::info!("alerts.json unavailable: {e:#}");
            return Vec::new();
        }
    };
    if alerts.is_empty() {
        return alerts;
    }
    let here = here(state);
    let now = OffsetDateTime::now_utc();
    alerts.into_iter().filter(|a| a.applies(&here, now)).collect()
}

/// Of `alerts`, the warnings and critical ones not announced on the desktop yet, now
/// recorded as announced (`alerts-notified.json`).
pub fn not_yet_notified(alerts: &[Alert]) -> Vec<Alert> {
    let path = crate::core::paths::data_dir().join("alerts-notified.json");
    let mut seen: BTreeSet<String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let fresh: Vec<Alert> = alerts
        .iter()
        .filter(|a| a.severity() >= Severity::Warning && !seen.contains(&a.id))
        .cloned()
        .collect();
    if !fresh.is_empty() {
        seen.extend(fresh.iter().map(|a| a.id.clone()));
        let _ = std::fs::create_dir_all(crate::core::paths::data_dir());
        let _ = std::fs::write(&path, serde_json::to_string(&seen).unwrap_or_default());
    }
    fresh
}

/// Announces `fresh` alerts on the desktop.
pub fn notify_desktop(fresh: &[Alert]) {
    for a in fresh {
        let title = match a.severity() {
            Severity::Critical => format!("Important: {}", a.title),
            _ => a.title.clone(),
        };
        crate::core::desktop_notify::show(&title, &plain(&a.body));
    }
}

/// Pure: Discord markdown as plain text, for a desktop notification: `[label](url)` as its
/// label, emphasis, code and heading marks dropped.
pub fn plain(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find("](").and_then(|j| after[j..].find(')').map(|k| (j, j + k))) {
            Some((j, end)) => {
                out.push_str(&after[..j]);
                rest = &after[end + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.lines()
        .map(|l| l.trim_start_matches(['#', '>', ' ']).replace(['*', '`', '_'], ""))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here() -> Here {
        Here {
            platform: "linux".into(),
            launcher: "0.13.0-alpha.1".into(),
            channel: "alpha".into(),
            mods: vec![("cool".into(), "1.1.0".into())],
            dev_channels: vec!["mia".into()],
        }
    }

    fn alert(target: &str, extra: &str) -> Alert {
        parse(&format!(
            r#"{{"alerts": [{{"id": "a1", "severity": "warning", "title": "T", "body": "B",
                "target": {target}{extra}}}]}}"#
        ))
        .remove(0)
    }

    #[test]
    fn targets() {
        let now = OffsetDateTime::now_utc();
        assert!(alert("{}", "").applies(&here(), now));
        assert!(alert(r#"{"platforms": ["linux"], "channels": ["alpha", "beta"]}"#, "").applies(&here(), now));
        assert!(!alert(r#"{"platforms": ["windows"]}"#, "").applies(&here(), now));
        assert!(alert(r#"{"launcher": ">=0.13.0-alpha.1"}"#, "").applies(&here(), now));
        assert!(!alert(r#"{"launcher": "<0.12.0"}"#, "").applies(&here(), now));
        assert!(alert(r#"{"mods": [{"id": "cool", "versions": "<1.2.0"}]}"#, "").applies(&here(), now));
        assert!(!alert(r#"{"mods": [{"id": "cool", "versions": ">=1.2.0"}]}"#, "").applies(&here(), now));
        assert!(!alert(r#"{"mods": [{"id": "other"}]}"#, "").applies(&here(), now));
        assert!(alert(r#"{"dev_channels": ["mia"]}"#, "").applies(&here(), now));
        assert!(!alert(r#"{"dev_channels": ["cat"]}"#, "").applies(&here(), now));
    }

    #[test]
    fn times() {
        let now = OffsetDateTime::now_utc();
        assert!(!alert("{}", r#", "expires": "2000-01-01T00:00:00Z""#).applies(&here(), now));
        assert!(!alert("{}", r#", "starts": "2999-01-01T00:00:00Z""#).applies(&here(), now));
        assert!(alert("{}", r#", "starts": "2000-01-01T00:00:00Z", "expires": "2999-01-01T00:00:00Z""#)
            .applies(&here(), now));
    }

    #[test]
    fn links_and_dismissing() {
        assert!(link_ok("https://github.com/EchoTools/echo-mods/issues/3"));
        assert!(link_ok("https://release.echovr.de/x"));
        assert!(!link_ok("http://github.com/x"));
        assert!(!link_ok("https://github.com.evil.example/x"));
        assert!(!link_ok("https://evil.example/?github.com"));
        let critical = parse(r#"{"alerts": [{"id": "c", "severity": "critical", "title": "T"}]}"#).remove(0);
        assert!(!critical.can_dismiss());
        assert!(alert("{}", "").can_dismiss());
        assert_eq!(plain("**Remove** [cool](https://x.y/z) now\n# Why"), "Remove cool now\nWhy");
        let bad_link = alert("{}", r#", "link_url": "https://evil.example""#);
        assert_eq!(bad_link.link(), None);
        assert!(parse("not json").is_empty());
    }
}
