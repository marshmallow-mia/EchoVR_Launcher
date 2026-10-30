//! The Play page's feed, from static files on release.echovr.de: the overview atop RIGHT NOW
//! (`servers.json`, aggregated from the EchoVRCE status API by the feed bot)
//! and Community News (`news.json`, mirrored from Discord by the feed bot).
//! News images are referenced by content-hashed file names, so a name that didn't change
//! never needs downloading again. A dev code's folder under `launcher/dev/` may have a
//! `news.json` (and its images) of its own: its slots go over the feed's, so news can be
//! tried with a code before it goes to everyone.

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
    /// File name of the message's first image under [`BASE`] (or, for a dev code's news,
    /// in its folder).
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub link_label: String,
    #[serde(default)]
    pub link_url: String,
    /// Where its image is: the feed's, or a dev code's folder.
    #[serde(skip)]
    pub from: Source,
}

/// Where a news item (its image) comes from.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum Source {
    #[default]
    Feed,
    /// A dev code's folder (`news.json` there): the code, never shown or logged.
    Dev(String),
}

/// Never the code.
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Feed => f.write_str("Feed"),
            Source::Dev(_) => f.write_str("Dev"),
        }
    }
}

/// A news image to download: `key` names it among the feed's images (a dev folder's
/// apart from the feed's, without the code), `name` is its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub key: String,
    pub name: String,
    pub from: Source,
}

impl NewsItem {
    /// Its image, if it has one (a dev folder's only by a plain file name).
    pub fn image_ref(&self) -> Option<ImageRef> {
        let name = self.image.clone()?;
        let key = match &self.from {
            Source::Feed => name.clone(),
            Source::Dev(_) if plain_name(&name) => format!("dev/{name}"),
            Source::Dev(_) => return None,
        };
        Some(ImageRef {
            key,
            name,
            from: self.from.clone(),
        })
    }
}

/// Pure: `name` is a file name in a folder, nothing that leaves it.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

pub fn fetch_servers() -> Result<Servers> {
    Ok(serde_json::from_str(&http::get_text(&format!(
        "{BASE}servers.json"
    ))?)?)
}

/// The news: the feed's, with a dev code's (the first code's folder with a `news.json`)
/// over it slot by slot. An error only when neither has any.
pub fn fetch_news() -> Result<News> {
    let feed = http::get_text(&format!("{BASE}news.json"))
        .and_then(|t| Ok(serde_json::from_str::<News>(&t)?));
    let dev = dev_news();
    match feed {
        Ok(feed) => Ok(over(Some(feed), dev).unwrap_or_default()),
        Err(e) => over(None, dev).ok_or(e),
    }
}

/// The first dev code's news (its folder's `news.json`), its items marked as its own.
fn dev_news() -> Option<News> {
    for code in crate::core::launcher::dev_code::codes() {
        match crate::core::launcher::dev_code::fetch_ours(&code, "news.json") {
            Ok(Some(text)) => match serde_json::from_str::<News>(&text) {
                Ok(n) => return Some(from_dev(n, &code)),
                Err(e) => tracing::warn!("a dev code's news.json doesn't read: {e}"),
            },
            Ok(None) => {}
            // Already without the code.
            Err(e) => tracing::info!("a dev code's news unavailable: {e:#}"),
        }
    }
    None
}

/// Pure: `n`'s items as `code`'s.
fn from_dev(mut n: News, code: &str) -> News {
    for item in [&mut n.slots.main, &mut n.slots.community]
        .into_iter()
        .flatten()
    {
        item.from = Source::Dev(code.to_string());
    }
    n
}

/// Pure: `dev`'s slots over `feed`'s, each where it has one; `None` when neither is there.
fn over(feed: Option<News>, dev: Option<News>) -> Option<News> {
    let Some(dev) = dev else { return feed };
    let mut n = feed.unwrap_or_default();
    if dev.slots.main.is_some() {
        n.slots.main = dev.slots.main;
    }
    if dev.slots.community.is_some() {
        n.slots.community = dev.slots.community;
    }
    Some(n)
}

/// A news image: from the feed, or from the dev code's folder its news came from.
pub fn fetch_image(image: &ImageRef) -> Result<image::RgbaImage> {
    let bytes = match &image.from {
        Source::Feed => http::get_bytes(&format!("{BASE}{}", image.name))?,
        Source::Dev(code) => crate::core::launcher::dev_code::fetch_ours_bytes(code, &image.name)?
            .ok_or_else(|| anyhow::anyhow!("not in the dev code's folder"))?,
    };
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
                title: "Welcome".into(),
                body: "Your launcher for Echo VR on PC (Quest is coming soon).\n\n\
                       - Install, update and play from one place\n\
                       - Friends, servers and matches to join, on the left\n\
                       - Server status and this week's best on the right"
                    .into(),
                image: None,
                link_label: "How to play".into(),
                link_url: "https://echovr.de".into(),
                from: Source::Feed,
            }),
            community: Some(NewsItem {
                title: "Community".into(),
                body: "Matches, events, help and the latest builds: the Echo VR community \
                       meets on Discord."
                    .into(),
                image: None,
                link_label: "Join the Discord".into(),
                link_url: "https://echovr.de".into(),
                from: Source::Feed,
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

    fn news(main: Option<&str>, community: Option<&str>) -> News {
        let item = |t: &str| NewsItem {
            title: t.into(),
            image: Some(format!("{t}.jpg")),
            ..Default::default()
        };
        News {
            slots: Slots {
                main: main.map(item),
                community: community.map(item),
            },
        }
    }

    fn titles(n: &News) -> (Option<&str>, Option<&str>) {
        (
            n.slots.main.as_ref().map(|i| i.title.as_str()),
            n.slots.community.as_ref().map(|i| i.title.as_str()),
        )
    }

    #[test]
    fn dev_news_goes_over_the_feed_slot_by_slot() {
        let dev = || Some(from_dev(news(Some("event"), None), "code"));
        // The dev code's main over the feed's; the feed's community stays.
        let n = over(Some(news(Some("feed"), Some("lounge"))), dev()).unwrap();
        assert_eq!(titles(&n), (Some("event"), Some("lounge")));
        assert_eq!(n.slots.main.unwrap().from, Source::Dev("code".into()));
        assert_eq!(n.slots.community.unwrap().from, Source::Feed);
        // No feed news (its news.json missing): the dev code's alone.
        let n = over(None, dev()).unwrap();
        assert_eq!(titles(&n), (Some("event"), None));
        // No dev news (a 404 in its folder): the feed's as it is.
        let n = over(Some(news(Some("feed"), None)), None).unwrap();
        assert_eq!(titles(&n), (Some("feed"), None));
        // Neither: nothing, so the Play page keeps its own banner.
        assert!(over(None, None).is_none());
    }

    #[test]
    fn dev_images_come_from_their_folder_under_their_own_key() {
        let feed = news(Some("a"), None).slots.main.unwrap();
        let r = feed.image_ref().unwrap();
        assert_eq!((r.key.as_str(), r.from), ("a.jpg", Source::Feed));
        let dev = from_dev(news(Some("a"), None), "secretcode")
            .slots
            .main
            .unwrap();
        let r = dev.image_ref().unwrap();
        // Apart from the feed's a.jpg, and never naming the code.
        assert_eq!(r.key, "dev/a.jpg");
        assert_eq!(r.name, "a.jpg");
        assert_eq!(r.from, Source::Dev("secretcode".into()));
        assert!(!format!("{r:?}").contains("secretcode"));
        // Only a plain file name in the dev folder.
        for bad in ["../x.jpg", "a/b.jpg", ".hidden", "", "a b.jpg"] {
            let mut i = dev.clone();
            i.image = Some(bad.into());
            assert!(i.image_ref().is_none(), "{bad}");
        }
        assert!(NewsItem::default().image_ref().is_none());
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
