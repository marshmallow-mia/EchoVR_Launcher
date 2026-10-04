//! The Play page's feed, from static files on release.echovr.de: the overview atop RIGHT NOW
//! (`servers.json`, aggregated from the EchoVRCE status API by the feed bot)
//! and Community News (`news.json`, mirrored from Discord by the feed bot).
//! News images are referenced by content-hashed file names, so a name that didn't change
//! never needs downloading again.

use std::sync::OnceLock;

use anyhow::Result;
use serde::Deserialize;
use time::{OffsetDateTime, UtcOffset};

use crate::core::http;

pub const BASE: &str = "https://release.echovr.de/launcher/feed/";

/// What is going on: players online, the queue, the matches per mode, the week's top 3.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Servers {
    /// "ok", "stale" (the API's data is old) or "down" (the API doesn't answer).
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub players: Players,
    #[serde(default)]
    pub modes: Modes,
    /// The matchmaking queue (needs the status service's EchoVRCE login).
    #[serde(default)]
    pub queue: Option<Queue>,
    /// The week's Arena top 3 (same).
    #[serde(default)]
    pub top: Option<Top>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Queue {
    /// Players searching, all modes.
    #[serde(default)]
    pub total: u32,
    /// Typical wait of the latest matches made, in seconds.
    #[serde(default)]
    pub wait_s: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Top {
    #[serde(default)]
    pub entries: Vec<TopEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TopEntry {
    #[serde(default)]
    pub rank: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub wins: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Players {
    #[serde(default)]
    pub online: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Modes {
    #[serde(default)]
    pub lobby: Mode,
    #[serde(default)]
    pub arena: Mode,
    #[serde(default)]
    pub combat: Mode,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Mode {
    #[serde(default)]
    pub public: Slot,
}

/// The public matches of one mode, added up.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Slot {
    #[serde(default)]
    pub matches: u32,
}

/// The Community News blocks: `main` feeds the banner and the first card, `community`
/// the second card. Either may be unset.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct News {
    #[serde(default)]
    pub slots: Slots,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Slots {
    #[serde(default)]
    pub main: Option<NewsItem>,
    #[serde(default)]
    pub community: Option<NewsItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewsItem {
    #[serde(default)]
    pub title: String,
    /// Discord markdown.
    #[serde(default)]
    pub body: String,
    /// File name of the message's first image under [`BASE`].
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub link_label: String,
    #[serde(default)]
    pub link_url: String,
}

pub fn fetch_servers() -> Result<Servers> {
    Ok(serde_json::from_str(&http::get_text(&format!(
        "{BASE}servers.json"
    ))?)?)
}

pub fn fetch_news() -> Result<News> {
    Ok(serde_json::from_str(&http::get_text(&format!(
        "{BASE}news.json"
    ))?)?)
}

/// A feed image by file name.
pub fn fetch_image(name: &str) -> Result<image::RgbaImage> {
    let bytes = http::get_bytes(&format!("{BASE}{name}"))?;
    Ok(image::load_from_memory(&bytes)?.to_rgba8())
}

// ---- time ----

static LOCAL_OFFSET: OnceLock<UtcOffset> = OnceLock::new();

/// Reads the local UTC offset. Call first thing in `main`: on Unix the `time` crate only
/// answers while the process is still single-threaded.
pub fn init_local_offset() {
    if let Ok(o) = UtcOffset::current_local_offset() {
        let _ = LOCAL_OFFSET.set(o);
    }
}

/// `t` in local time (the offset read at start-up, see `init_local_offset`).
pub fn local(t: OffsetDateTime) -> OffsetDateTime {
    t.to_offset(LOCAL_OFFSET.get().copied().unwrap_or(UtcOffset::UTC))
}

fn now_local() -> OffsetDateTime {
    local(OffsetDateTime::now_utc())
}

pub fn parse_time(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

fn clock(t: OffsetDateTime) -> String {
    let (h, m) = (t.hour(), t.minute());
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{m:02} {}", if h < 12 { "AM" } else { "PM" })
}

fn month(t: OffsetDateTime) -> String {
    t.month().to_string()
}

/// Discord's footer time: "Today at 12:32 PM", "Yesterday at …", else "9/26/2026 8:16 AM".
pub fn footer_time(t: OffsetDateTime) -> String {
    footer_time_at(local(t), now_local())
}

fn footer_time_at(t: OffsetDateTime, now: OffsetDateTime) -> String {
    let days = (now.date() - t.date()).whole_days();
    match days {
        0 => format!("Today at {}", clock(t)),
        1 => format!("Yesterday at {}", clock(t)),
        _ => format!(
            "{}/{}/{} {}",
            u8::from(t.month()),
            t.day(),
            t.year(),
            clock(t)
        ),
    }
}

/// A Discord timestamp mention (`<t:unix:style>`) in local time.
pub fn discord_time(unix: i64, style: char) -> String {
    let Ok(t) = OffsetDateTime::from_unix_timestamp(unix) else {
        return String::new();
    };
    discord_time_at(local(t), style, now_local())
}

fn discord_time_at(t: OffsetDateTime, style: char, now: OffsetDateTime) -> String {
    let date_long = format!("{} {}, {}", month(t), t.day(), t.year());
    match style {
        't' => clock(t),
        'T' => {
            let s = clock(t);
            let (hm, ampm) = s.split_once(' ').unwrap_or((&s, ""));
            format!("{hm}:{:02} {ampm}", t.second())
        }
        'd' => format!("{}/{}/{}", u8::from(t.month()), t.day(), t.year()),
        'D' => date_long,
        'F' => format!("{}, {date_long} {}", t.weekday(), clock(t)),
        'R' => {
            let secs = (now - t).whole_seconds();
            let (n, unit) = match secs.unsigned_abs() {
                s if s < 60 => (s, "second"),
                s if s < 3600 => (s / 60, "minute"),
                s if s < 86_400 => (s / 3600, "hour"),
                s if s < 30 * 86_400 => (s / 86_400, "day"),
                s if s < 365 * 86_400 => (s / (30 * 86_400), "month"),
                s => (s / (365 * 86_400), "year"),
            };
            let plural = if n == 1 { "" } else { "s" };
            if secs >= 0 {
                format!("{n} {unit}{plural} ago")
            } else {
                format!("in {n} {unit}{plural}")
            }
        }
        // 'f', the default.
        _ => format!("{date_long} {}", clock(t)),
    }
}

// ---- made-up data (snapshots, and the look of the design concept) ----

/// The design concept's numbers.
pub fn mock_servers() -> Servers {
    let public = |matches| Mode {
        public: Slot { matches },
    };
    Servers {
        status: "ok".into(),
        players: Players { online: 142 },
        modes: Modes {
            lobby: public(11),
            arena: public(6),
            combat: public(2),
        },
        queue: Some(Queue {
            total: 9,
            wait_s: Some(80),
        }),
        top: Some(Top {
            entries: [("Rainy32", 77), ("pepe_1", 43), ("Ace", 35)]
                .into_iter()
                .enumerate()
                .map(|(i, (name, wins))| TopEntry {
                    rank: i as u32 + 1,
                    name: name.into(),
                    wins,
                })
                .collect(),
        }),
    }
}

pub fn mock_news() -> News {
    News {
        slots: Slots {
            main: Some(NewsItem {
                title: "Title".into(),
                body: "Iusto odio ducimus qui blanditiis praesentium voluptatum deleniti atque\n\n\
                       - Quos dolores et quas\n\
                       - Molestias excepturi sint occaecati\n\
                       - Cupiditate non provident.\n\n\
                       Similique sunt in culpa qui officia deserunt mollitia animi, id"
                    .into(),
                image: None,
                link_label: "How to play".into(),
                link_url: "https://echovr.de".into(),
            }),
            community: Some(NewsItem {
                title: "Community".into(),
                body: "Iusto odio dignissimos ducimus qui  voluptatum deleniti atque corrupti\n\n\
                       Similique sunt in culpa qui officia deserunt mollitia est laborum et \
                       dolorum fuga. Et harum quidem"
                    .into(),
                image: None,
                link_label: "Link".into(),
                link_url: "https://echovr.de".into(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> OffsetDateTime {
        parse_time(s).unwrap()
    }

    #[test]
    fn parses_feed_files() {
        // The status service's file, older fields and all.
        let s: Servers = serde_json::from_str(
            r#"{"status":"ok","source_time":"2026-09-29T11:36:32Z",
            "servers":{"total":62,"public":52,"private":10},
            "players":{"online":4,"last_hour":4},
            "modes":{"lobby":{"public":{"matches":1,"players":4,"limit":12,"spectators":0},
            "private":{"matches":0,"players":0}}},
            "queue":{"total":3,"wait_s":null},
            "top":{"board":"arena_wins_week","entries":[{"rank":1,"name":"A","wins":9}]},
            "updated_at":"2026-09-29T11:36:33+00:00"}"#,
        )
        .unwrap();
        assert_eq!(s.players.online, 4);
        assert_eq!(s.modes.lobby.public.matches, 1);
        assert_eq!(s.modes.combat.public.matches, 0);
        assert_eq!(s.queue.unwrap().total, 3);
        assert_eq!(s.top.unwrap().entries[0].name, "A");
        let n: News = serde_json::from_str(
            r#"{"slots":{"main":{"id":"1","title":"Halloween","body":"- a","image":null,
            "link_label":"READ MORE","link_url":"https://x","jump_url":"https://y",
            "posted_at":null,"edited_at":null},"community":null},"updated_at":"x"}"#,
        )
        .unwrap();
        assert_eq!(n.slots.main.unwrap().title, "Halloween");
        assert!(n.slots.community.is_none());
    }

    #[test]
    fn footer_times() {
        let now = at("2026-09-29T15:00:00Z");
        assert_eq!(
            footer_time_at(at("2026-09-29T12:32:00Z"), now),
            "Today at 12:32 PM"
        );
        assert_eq!(
            footer_time_at(at("2026-09-28T00:05:00Z"), now),
            "Yesterday at 12:05 AM"
        );
        assert_eq!(
            footer_time_at(at("2026-09-26T08:16:00Z"), now),
            "9/26/2026 8:16 AM"
        );
    }

    #[test]
    fn discord_timestamps() {
        let t = at("2026-09-26T08:16:00Z");
        let now = at("2026-09-29T08:16:00Z");
        assert_eq!(discord_time_at(t, 'f', now), "September 26, 2026 8:16 AM");
        assert_eq!(discord_time_at(t, 't', now), "8:16 AM");
        assert_eq!(discord_time_at(t, 'R', now), "3 days ago");
        assert_eq!(discord_time_at(now, 'R', t), "in 3 days");
    }
}
