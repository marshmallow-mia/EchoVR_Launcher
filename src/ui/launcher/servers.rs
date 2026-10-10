//! Servers page: the game service's live servers, signed in with EchoVRCE. Left, the
//! servers (filtered by mode) with JOIN, and your match history; right, where you are
//! (in a match, in the queue) with your party, and your friends with the match they are
//! in. JOIN starts Echo VR straight into the match (its lobby id), or -- with the game
//! already running, or on the Quest -- queues it as your next match. START A SERVER
//! (for guilds that may) starts a new match and opens its card: its link to copy, JOIN
//! NOW, and INVITE for your friends. Invites to you are read every 30 seconds and shown
//! on the right, with JOIN.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::{hero, panel, setup, Dashboard, Page};
use crate::core::echovrce::game::{
    self, Found, Friend, FriendState, Invite, Lobby, Match, Mode, Region, Role, Summary, Ticket,
};
use crate::core::echovrce::{Account, Tokens};
use crate::core::launcher::catalog::Platform;
use crate::core::launcher::feed;
use crate::ui::design::{self, dz, Dr};
use crate::ui::kit::Kit;
use crate::ui::parts::Worker;
use crate::ui::style::Icon;
use crate::ui::widgets::{Tone, BTN_H};

const LIST_EVERY: Duration = Duration::from_secs(5);
/// While Play shows RIGHT NOW: for the matches your friends are in.
const LIST_ON_PLAY: Duration = Duration::from_secs(15);
const TICKETS_EVERY: Duration = Duration::from_secs(5);
/// While the page isn't shown: for the queue line in the status bar.
const TICKETS_AWAY: Duration = Duration::from_secs(30);
const FRIENDS_EVERY: Duration = Duration::from_secs(30);
/// While the Friends page is shown: requests come and go.
const FRIENDS_ON_PAGE: Duration = Duration::from_secs(5);
/// Whether or not the page is shown.
const INVITES_EVERY: Duration = Duration::from_secs(15);
/// Your match history (its tab, and Friends' PLAYED WITH).
const HISTORY_EVERY: Duration = Duration::from_secs(5 * 60);
/// A match you started that never showed up on the list is forgotten after this.
const CREATED_UNLISTED: Duration = Duration::from_secs(10 * 60);

// Geometry in design pixels, under the header strip.
// The list where Install has its hero and VERSIONS; YOUR MATCH and FRIENDS where it has
// YOUR LIBRARY.
const LIST: Dr = Dr::new(137.0, 80.0, 1146.0, 966.0);
const YOU: Dr = Dr::new(1318.0, 80.0, 555.0, 456.0);
const FRIENDS: Dr = Dr::new(1318.0, 566.0, 555.0, 480.0);
/// A tile's padding, a friend's (and an invite's) tile, the gap between tiles, and the
/// buttons in a tile.
const TILE_PAD: f32 = 14.0;
const SMALL_TILE_H: f32 = 64.0;
const TILE_GAP: f32 = 10.0;
const ACTION_H: f32 = 40.0;
const ROW_H: f32 = 76.0;
const ROW_PAD: f32 = 12.0;

/// Which page shows the game service's data: the Servers page (all of it, often), Play
/// (RIGHT NOW: your friends and the matches they are in), Friends (them, requests and the
/// players of your last matches), or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shown {
    Servers,
    Play,
    Friends,
    Neither,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Tab {
    #[default]
    Live,
    History,
}

enum Msg {
    List(Result<Vec<Match>, String>),
    Friends(Vec<Friend>),
    Tickets(Vec<Ticket>),
    Role(Role, Vec<(String, String)>),
    History(Result<Vec<Summary>, String>),
    /// Where new matches can be started.
    Regions(Result<Vec<Region>, String>),
    Invites(Vec<Invite>),
    /// An invite was sent (or not): to whom (user id, name), for which match.
    Invited(String, String, String, Result<(), String>),
    /// A join or start finished.
    Done(Result<Done, String>),
    /// What a search for players found (the text searched for).
    Found(String, Result<Vec<Found>, String>),
    /// A friend added, accepted, removed or turned down (or not): the player (user id,
    /// name), what was done ("add", "remove"), and how it went.
    FriendChanged(String, String, &'static str, Result<(), String>),
}

/// What a finished join or start leads to.
enum Done {
    /// Queued as your next match: what to say.
    Queued(String),
    /// Start the game into this lobby.
    Launch(String),
    /// A new match is up (its id): its card opens, to share it.
    Started(String),
}

/// A match you started: its card can be opened again until it ends.
pub(super) struct Created {
    pub id: String,
    at: Instant,
    /// Seen on the server list (gone from it later: ended).
    listed: bool,
    /// You were seen in it (out of it later: left, and it's no longer yours to share).
    joined: bool,
}

/// The page's data and what is being fetched.
#[derive(Default)]
pub(super) struct Servers {
    worker: Worker<Msg>,
    pub list: Vec<Match>,
    pub updated: Option<Instant>,
    pub loading: bool,
    pub error: Option<String>,
    pub filter: Option<Mode>,
    pub tab: Tab,
    pub role: Option<Role>,
    pub guilds: Vec<(String, String)>,
    /// Your friends (accepted).
    pub friends: Vec<Friend>,
    /// Friend requests you sent and got.
    pub requests: Vec<Friend>,
    /// Friends: the search field, and what the last search found (for which text).
    pub search: String,
    pub found: Option<(String, Result<Vec<Found>, String>)>,
    pub searching: bool,
    /// Players whose friendship is being changed (user ids).
    pub changing: HashSet<String>,
    /// Friends' lists, scrolled: found players, requests, friends; and PLAYED WITH.
    pub friends_page_scroll: [f32; 3],
    pub played_scroll: f32,
    /// A friend's remove button was clicked (user id, name), and the question asked.
    pub remove_clicked: Option<(String, String)>,
    pub remove_asked: Option<(String, String)>,
    pub tickets: Vec<Ticket>,
    pub history: Option<Result<Vec<Summary>, String>>,
    pub regions: Option<Result<Vec<Region>, String>>,
    /// A join or start under way.
    pub busy: bool,
    list_at: Option<Instant>,
    tickets_at: Option<Instant>,
    friends_at: Option<Instant>,
    role_asked: bool,
    /// When your match history was last asked for (`None`: never, or Refresh).
    history_at: Option<Instant>,
    pub scroll: f32,
    /// A join waiting for the game to start into this match (lobby id).
    pub launch: Option<String>,
    /// Invites waiting for you.
    pub invites: Vec<Invite>,
    invites_at: Option<Instant>,
    /// Invites announced already, and those dismissed here.
    seen: HashSet<String>,
    dismissed: HashSet<String>,
    /// A new invite arrived: the window asks for attention.
    attention: bool,
    /// Friends invited (user id, match id), and those being invited (user id).
    invited: HashSet<(String, String)>,
    inviting: HashSet<String>,
    /// The match you started last, until it ends.
    pub created: Option<Created>,
    /// A started match whose card opens next.
    ready: Option<String>,
    /// The share card's Copy was clicked (it reads "Copied" for a moment).
    copied: Option<Instant>,
    /// The share card's friend list.
    pub share_scroll: f32,
    pub friends_scroll: f32,
    /// RIGHT NOW's friend list, on Play.
    pub now_scroll: f32,
    /// Which account the data is for.
    account: String,
    /// Snapshots: made-up data, nothing fetched.
    demo: bool,
}

fn due(at: Option<Instant>, every: Duration) -> bool {
    at.is_none_or(|t| t.elapsed() >= every)
}

impl Servers {
    /// Every frame: takes in what arrived, and refreshes what is due (more while a page
    /// shows it). Returns a notice for the status bar.
    pub(super) fn tick(
        &mut self,
        ctx: &egui::Context,
        session: Option<(Tokens, Account)>,
        shown: Shown,
    ) -> Option<String> {
        let visible = shown == Shown::Servers;
        let social = shown != Shown::Neither;
        if self.demo {
            return None;
        }
        let Some((tokens, account)) = session else {
            if !self.account.is_empty() {
                *self = Servers::default();
            }
            return None;
        };
        if self.account != account.id {
            *self = Servers {
                account: account.id.clone(),
                ..Servers::default()
            };
        }
        let mut notice = None;
        for m in self.worker.drain() {
            match m {
                Msg::List(r) => {
                    self.loading = false;
                    match r {
                        Ok(list) => {
                            if let Some(c) = &mut self.created {
                                let lobby = game::lobby_id(&c.id);
                                let in_it = game::find_me(&list, &account.id)
                                    .is_some_and(|(m, _)| m.lobby_id() == lobby);
                                c.joined |= in_it;
                                if list.iter().any(|m| m.lobby_id() == lobby) {
                                    c.listed = true;
                                }
                                let ended = !list.iter().any(|m| m.lobby_id() == lobby)
                                    && (c.listed || c.at.elapsed() > CREATED_UNLISTED);
                                if ended || (c.joined && !in_it) {
                                    self.created = None;
                                }
                            }
                            self.list = list;
                            self.error = None;
                            self.updated = Some(Instant::now());
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Friends(f) => {
                    (self.friends, self.requests) = f
                        .into_iter()
                        .partition(|f| f.state == game::FriendState::Friend);
                }
                Msg::Tickets(t) => self.tickets = t,
                Msg::Role(r, g) => {
                    self.role = Some(r);
                    self.guilds = g;
                }
                Msg::History(h) => self.history = Some(h),
                Msg::Regions(r) => self.regions = Some(r),
                Msg::Invites(list) => {
                    let new: Vec<&Invite> =
                        list.iter().filter(|i| !self.seen.contains(&i.id)).collect();
                    if let Some(first) = new.first() {
                        let who = sender(first);
                        notice = Some(match new.len() {
                            1 => format!("{who} invited you to a match"),
                            n => format!("{who} and {} more invited you to matches", n - 1),
                        });
                        self.attention = true;
                    }
                    self.seen.extend(list.iter().map(|i| i.id.clone()));
                    self.invites = list;
                }
                Msg::Invited(user, name, match_id, r) => {
                    self.inviting.remove(&user);
                    notice = Some(match r {
                        Ok(()) => {
                            self.invited.insert((user, match_id));
                            format!("Invited {name}")
                        }
                        Err(e) => format!("Couldn't invite {name}: {e}"),
                    });
                }
                Msg::Found(text, r) => {
                    self.searching = false;
                    self.found = Some((text, r));
                }
                Msg::FriendChanged(user, name, what, r) => {
                    self.changing.remove(&user);
                    self.friends_at = None;
                    notice = Some(match r {
                        Ok(()) => {
                            let asked = self.requests.iter().position(|f| f.user_id == user);
                            match (what, asked) {
                                ("add", Some(i))
                                    if self.requests[i].state == FriendState::Received =>
                                {
                                    let mut f = self.requests.remove(i);
                                    f.state = FriendState::Friend;
                                    self.friends.push(f);
                                    format!("You and {name} are friends now")
                                }
                                ("add", _) => {
                                    self.requests.push(Friend {
                                        user_id: user,
                                        name: name.clone(),
                                        state: FriendState::Sent,
                                        ..Default::default()
                                    });
                                    format!("Sent {name} a friend request")
                                }
                                _ => {
                                    self.friends.retain(|f| f.user_id != user);
                                    self.requests.retain(|f| f.user_id != user);
                                    format!("{name} is off your list")
                                }
                            }
                        }
                        Err(e) => format!("Couldn't change your friendship with {name}: {e}"),
                    });
                }
                Msg::Done(r) => {
                    self.busy = false;
                    self.list_at = None;
                    match r {
                        Ok(Done::Queued(text)) => notice = Some(text),
                        Ok(Done::Launch(lobby)) => self.launch = Some(lobby),
                        Ok(Done::Started(id)) => {
                            self.created = Some(Created {
                                id: id.clone(),
                                at: Instant::now(),
                                listed: false,
                                joined: false,
                            });
                            self.ready = Some(id);
                        }
                        Err(e) => notice = Some(e),
                    }
                }
            }
        }
        let token = tokens.token;
        let list_every = if visible { LIST_EVERY } else { LIST_ON_PLAY };
        if social && !self.loading && due(self.list_at, list_every) {
            self.refresh(ctx, &token);
        }
        let friends_every = if shown == Shown::Friends {
            FRIENDS_ON_PAGE
        } else {
            FRIENDS_EVERY
        };
        if social && due(self.friends_at, friends_every) {
            self.friends_at = Some(Instant::now());
            let t = token.clone();
            self.worker.spawn(ctx, move |tx| {
                if let Ok(f) = game::friends(&t) {
                    tx.send(Msg::Friends(f));
                }
            });
        }
        if due(self.invites_at, INVITES_EVERY) {
            self.invites_at = Some(Instant::now());
            let t = token.clone();
            self.worker.spawn(ctx, move |tx| {
                if let Ok(i) = game::pending_invites(&t) {
                    tx.send(Msg::Invites(i));
                }
            });
        }
        // Invites arrive while the launcher sits in the background too.
        ctx.request_repaint_after(INVITES_EVERY);
        let every = if visible { TICKETS_EVERY } else { TICKETS_AWAY };
        if due(self.tickets_at, every) {
            self.tickets_at = Some(Instant::now());
            let (t, me) = (token.clone(), account.id.clone());
            self.worker.spawn(ctx, move |tx| {
                if let Ok(q) = game::tickets(&t, &me) {
                    tx.send(Msg::Tickets(q));
                }
            });
        }
        if visible && !self.role_asked {
            self.role_asked = true;
            let t = token.clone();
            self.worker.spawn(ctx, move |tx| {
                if let Ok(role) = game::role(&t) {
                    let names = game::guild_names(&t, &role.allocator_guilds).unwrap_or_default();
                    tx.send(Msg::Role(role, names));
                }
            });
        }
        if ((visible && self.tab == Tab::History) || shown == Shown::Friends)
            && due(self.history_at, HISTORY_EVERY)
        {
            self.history_at = Some(Instant::now());
            let (t, me) = (token, account.id);
            self.worker.spawn(ctx, move |tx| {
                tx.send(Msg::History(
                    game::history(&t, &me, 15).map_err(|e| format!("{e:#}")),
                ))
            });
        }
        if visible || shown == Shown::Friends {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else if social {
            ctx.request_repaint_after(LIST_ON_PLAY);
        }
        notice
    }

    /// Friends: looks for players named like `text`.
    pub(super) fn search_players(&mut self, ctx: &egui::Context, token: &str, text: &str) {
        if self.demo || game::search_pattern(text).is_none() {
            return;
        }
        self.searching = true;
        let (t, text) = (token.to_string(), text.trim().to_string());
        self.worker.spawn(ctx, move |tx| {
            let r = game::search(&t, &text).map_err(|e| format!("{e:#}"));
            tx.send(Msg::Found(text, r));
        });
    }

    /// Friends: asks player `user` to be your friend, or accepts their request (`add`),
    /// or removes them, turns their request down or takes yours back.
    pub(super) fn change_friend(
        &mut self,
        ctx: &egui::Context,
        token: &str,
        user: &str,
        name: &str,
        add: bool,
    ) {
        if self.demo || !self.changing.insert(user.to_string()) {
            return;
        }
        let (t, user, name) = (token.to_string(), user.to_string(), name.to_string());
        self.worker.spawn(ctx, move |tx| {
            let r = if add {
                game::add_friend(&t, &user)
            } else {
                game::remove_friend(&t, &user)
            };
            let what = if add { "add" } else { "remove" };
            tx.send(Msg::FriendChanged(
                user,
                name,
                what,
                r.map_err(|e| format!("{e:#}")),
            ));
        });
    }

    /// Where you stand with player `user`, if anywhere.
    pub(super) fn friendship(&self, user: &str) -> Option<FriendState> {
        self.friends
            .iter()
            .chain(&self.requests)
            .find(|f| f.user_id == user)
            .map(|f| f.state)
    }

    /// Snapshots: servers, a party in the queue, friends and a history, all made up.
    pub(super) fn demo(&mut self, tab: Tab) {
        use serde_json::json;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let m = |id: &str, label: serde_json::Value| {
            game::parse_match(&json!({ "match_id": id, "label": label.to_string() }))
        };
        let players = |n: usize, prefix: &str| {
            (0..n)
                .map(|i| json!({ "user_id": format!("{prefix}{i}"), "display_name": format!("Player {}", i + 1), "team": i % 2 }))
                .collect::<Vec<_>>()
        };
        let mut pal = players(5, "a");
        pal.push(json!({ "user_id": "pal", "display_name": "Ace", "team": 1 }));
        self.list = [
            m("0f5c1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b.nakama1", json!({"mode": "echo_arena", "lobby_type": "public", "player_limit": 8, "server_city": "Dallas", "server_region": "US-TX", "start_time": now - 720, "game_state": {"blue_score": 3, "orange_score": 2}, "players": pal})),
            m("1f5c1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b.nakama1", json!({"mode": "echo_combat", "lobby_type": "public", "player_limit": 10, "server_city": "Frankfurt", "server_region": "EU-DE", "start_time": now - 300, "players": players(4, "b")})),
            m("2f5c1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b.nakama1", json!({"mode": "social_2.0", "lobby_type": "public", "player_limit": 12, "server_city": "Sydney", "server_region": "AU-NSW", "start_time": now - 3600, "players": players(9, "c")})),
            m("3f5c1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b.nakama1", json!({"mode": "echo_arena_private", "player_limit": 8, "server_city": "Chicago", "server_region": "US-IL", "players": players(6, "d")})),
            m("4f5c1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b.nakama1", json!({"mode": "echo_arena", "lobby_type": "public", "player_limit": 8, "server_city": "Amsterdam", "server_region": "EU-NL", "start_time": now - 1500, "game_state": {"blue_score": 7, "orange_score": 7}, "players": players(8, "e")})),
        ]
        .into_iter()
        .flatten()
        .collect();
        self.updated = Some(Instant::now());
        self.friends = vec![
            Friend {
                user_id: "pal".into(),
                discord_id: String::new(),
                name: "Ace".into(),
                ..Default::default()
            },
            Friend {
                user_id: "x1".into(),
                discord_id: String::new(),
                name: "Pebbles".into(),
                ..Default::default()
            },
            Friend {
                user_id: "x2".into(),
                discord_id: String::new(),
                name: "Wrench".into(),
                ..Default::default()
            },
        ];
        self.requests = vec![
            Friend {
                user_id: "x3".into(),
                name: "Nova".into(),
                state: game::FriendState::Received,
                ..Default::default()
            },
            Friend {
                user_id: "x4".into(),
                name: "Glitch".into(),
                state: game::FriendState::Sent,
                ..Default::default()
            },
        ];
        self.tickets = vec![Ticket {
            mode: Mode::Combat,
            since: Some(now - 83),
            players: vec!["Marshmallow".into(), "Pebbles".into()],
        }];
        self.role = Some(Role {
            allocator_guilds: vec!["g1".into()],
            system_groups: Vec::new(),
        });
        self.guilds = vec![("g1".into(), "Echo VR Lounge".into())];
        self.regions = Some(Ok(vec![
            Region {
                code: "us-central".into(),
                location: "Dallas, US-TX".into(),
                groups: vec!["g1".into()],
            },
            Region {
                code: "eu-west".into(),
                location: "Amsterdam, EU-NL".into(),
                groups: vec!["g1".into()],
            },
            Region {
                code: "oce".into(),
                location: "Sydney, AU-NSW".into(),
                groups: vec!["g1".into()],
            },
        ]));
        let summary = |mode: Mode, ago: i64, score: (u32, u32), team| Summary {
            id: format!("m{ago}"),
            mode,
            played: Some(now - ago),
            duration_s: Some(540),
            score: Some(score),
            team: Some(team),
            players: [
                ("demo", "Marshmallow"),
                ("pal", "Ace"),
                ("x5", "Rookie"),
                ("x6", "Sprocket"),
                ("x3", "Nova"),
            ]
            .iter()
            .map(|(i, n)| (i.to_string(), n.to_string()))
            .collect(),
        };
        self.history = Some(Ok(vec![
            summary(Mode::Arena, 3600, (5, 3), game::Team::Blue),
            summary(Mode::Arena, 7200, (2, 4), game::Team::Blue),
            summary(Mode::Combat, 90000, (3, 1), game::Team::Orange),
        ]));
        self.tab = tab;
        self.demo = true;
    }

    /// Snapshots, after [`Servers::demo`]: a search for players and what it found.
    pub(super) fn demo_search(&mut self) {
        self.search = "mars".into();
        let found = |id: &str, name: &str, username: &str| Found {
            user_id: id.into(),
            name: name.into(),
            username: username.into(),
        };
        self.found = Some((
            "mars".into(),
            Ok(vec![
                found("x7", "Mars", "mars_orbit"),
                found("x8", "marsisthegoat", "marsisthegoat"),
                found("x4", "Glitch", "mars2"),
                found("pal", "Ace", "marsh"),
            ]),
        ));
    }

    /// Snapshots, after [`Servers::demo`]: an invite to you, a friend invited, and you in
    /// the Combat match (`in_match`) or having started the private one. Returns the
    /// match to share.
    pub(super) fn demo_social(&mut self, in_match: bool) -> String {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let (arena, combat, private) = (
            self.list[0].id.clone(),
            self.list[1].id.clone(),
            self.list[3].id.clone(),
        );
        let shared = if in_match {
            self.list[1].players.push(game::Player {
                user_id: "demo".into(),
                discord_id: String::new(),
                name: "Marshmallow".into(),
                team: game::Team::Blue,
                party_id: String::new(),
                ping_ms: Some(31),
            });
            self.tickets.clear();
            combat
        } else {
            self.created = Some(Created {
                id: private.clone(),
                at: Instant::now(),
                listed: true,
                joined: false,
            });
            private
        };
        self.invites = vec![Invite {
            id: "i1".into(),
            sender_id: "x1".into(),
            sender_name: "Pebbles".into(),
            match_id: arena,
            created: Some(now - 40),
        }];
        self.invited.insert(("x2".into(), shared.clone()));
        shared
    }

    /// Fetches the server list now.
    pub(super) fn refresh(&mut self, ctx: &egui::Context, token: &str) {
        self.loading = true;
        self.list_at = Some(Instant::now());
        let t = token.to_string();
        self.worker.spawn(ctx, move |tx| {
            tx.send(Msg::List(
                game::matches(&t, false).map_err(|e| format!("{e:#}")),
            ))
        });
    }

    /// Fetches where new matches can be started (the waiting servers).
    fn fetch_regions(&mut self, ctx: &egui::Context, token: &str) {
        self.regions = None;
        let t = token.to_string();
        self.worker.spawn(ctx, move |tx| {
            let r = game::start_regions(&t).map_err(|e| format!("{e:#}"));
            tx.send(Msg::Regions(r));
        });
    }

    /// The queue line for the status bar: "Searching for a Combat match · 1:23".
    pub(super) fn queue_line(&self) -> Option<String> {
        let t = self.tickets.first()?;
        let waited = t
            .since
            .map(|s| (time::OffsetDateTime::now_utc().unix_timestamp() - s).max(0))
            .map(|s| format!("   ·   {}:{:02}", s / 60, s % 60))
            .unwrap_or_default();
        let party = match t.players.len() {
            0 | 1 => String::new(),
            n => format!(" with a party of {n}"),
        };
        Some(format!(
            "Searching for a {} match{party}{waited}",
            t.mode.label()
        ))
    }

    /// The invites not dismissed here.
    pub(super) fn open_invites(&self) -> Vec<&Invite> {
        self.invites
            .iter()
            .filter(|i| !self.dismissed.contains(&i.id))
            .collect()
    }

    /// A listed match by its id (or lobby id).
    fn find(&self, match_id: &str) -> Option<&Match> {
        let lobby = game::lobby_id(match_id);
        self.list.iter().find(|m| m.lobby_id() == lobby)
    }
}

/// Who sent an invite.
fn sender(i: &Invite) -> &str {
    if i.sender_name.is_empty() {
        "A friend"
    } else {
        &i.sender_name
    }
}

/// "Arena · Dallas, US-TX", or what is known.
fn about(m: Option<&Match>) -> String {
    m.map_or("A match".to_string(), |m| {
        if m.location.is_empty() {
            m.mode.label().to_string()
        } else {
            format!("{} · {}", m.mode.label(), m.location)
        }
    })
}

// ---- joining and starting ----

/// Echo VR can be started straight into a match: on PC, set up, with the game closed.
pub(super) fn starts_pc(d: &Dashboard) -> bool {
    d.platform == Platform::Pc && !d.game().is_running() && super::setup::pc_play_supported(d)
}

fn queued(quest: bool) -> Done {
    Done::Queued(if quest {
        "Queued as your next match: start Echo VR on your Quest and use the terminal to go there"
            .into()
    } else {
        "Queued as your next match: use the terminal in Echo VR to go there".into()
    })
}

/// JOIN: on PC with the game closed, Echo VR starts straight into the match; otherwise
/// the match is queued as your next one (the game's terminal takes you there).
fn join(d: &mut Dashboard, ctx: &egui::Context, match_id: &str) {
    let Some((tokens, account)) = d.vrce.session() else {
        return;
    };
    if starts_pc(d) {
        let lobby = super::play::Join::lobby(game::lobby_id(match_id));
        super::play::join(d, ctx, lobby);
        return;
    }
    let quest = d.platform == Platform::Quest;
    let match_id = match_id.to_string();
    d.servers.busy = true;
    d.servers.worker.spawn(ctx, move |tx| {
        let r = game::set_next_match(&tokens.token, &account.discord_id, &match_id)
            .map(|()| queued(quest))
            .map_err(|e| format!("Couldn't join: {e:#}"));
        tx.send(Msg::Done(r));
    });
}

/// The lobby card's JOIN when the game can't be started into it: queues the match of
/// `lobby` (from a link) as your next one, found on the server list.
pub(super) fn queue_lobby(d: &mut Dashboard, ctx: &egui::Context, lobby: &str) {
    let Some((tokens, account)) = d.vrce.session() else {
        d.notify("Sign in with EchoVRCE to queue a match");
        return;
    };
    let known = d.servers.find(lobby).map(|m| m.id.clone());
    let quest = d.platform == Platform::Quest;
    let lobby = lobby.to_string();
    d.servers.busy = true;
    d.servers.worker.spawn(ctx, move |tx| {
        let id = match known {
            Some(id) => Ok(id),
            None => game::matches(&tokens.token, true).and_then(|all| {
                all.into_iter()
                    .find(|m| m.lobby_id() == lobby)
                    .map(|m| m.id)
                    .ok_or_else(|| anyhow::anyhow!("that match isn't running"))
            }),
        };
        let r = id
            .and_then(|id| game::set_next_match(&tokens.token, &account.discord_id, &id))
            .map(|()| queued(quest))
            .map_err(|e| format!("Couldn't join: {e:#}"));
        tx.send(Msg::Done(r));
    });
}

/// An invite's JOIN: accepts it, then joins its match.
fn accept(d: &mut Dashboard, ctx: &egui::Context, invite: &Invite) {
    let Some((tokens, account)) = d.vrce.session() else {
        return;
    };
    let direct = starts_pc(d);
    let quest = d.platform == Platform::Quest;
    let (id, match_id) = (invite.id.clone(), invite.match_id.clone());
    d.servers.invites.retain(|i| i.id != id);
    d.servers.busy = true;
    d.servers.worker.spawn(ctx, move |tx| {
        let r = game::accept_invite(&tokens.token, &id)
            .and_then(|()| {
                if direct {
                    Ok(Done::Launch(game::lobby_id(&match_id)))
                } else {
                    game::set_next_match(&tokens.token, &account.discord_id, &match_id)
                        .map(|()| queued(quest))
                }
            })
            .map_err(|e| format!("Couldn't join: {e:#}"));
        tx.send(Msg::Done(r));
    });
}

/// INVITE: asks the site to invite friend `f` to `match_id`.
fn invite(d: &mut Dashboard, ctx: &egui::Context, f: &Friend, match_id: &str) {
    let Some((tokens, _)) = d.vrce.session() else {
        return;
    };
    d.servers.inviting.insert(f.user_id.clone());
    let (user, name, m) = (f.user_id.clone(), f.name.clone(), match_id.to_string());
    d.servers.worker.spawn(ctx, move |tx| {
        let r = game::invite(&tokens.token, &user, &m).map_err(|e| format!("{e:#}"));
        tx.send(Msg::Invited(user, name, m, r));
    });
}

/// Puts a match's link on the clipboard.
fn copy_link(d: &mut Dashboard, ctx: &egui::Context, match_id: &str) {
    ctx.copy_text(game::share_link(match_id));
    d.servers.copied = Some(Instant::now());
    d.notify("Copied the match's link: send it to the players");
}

/// Opens a match's card: its link, and invites.
fn share(d: &mut Dashboard, match_id: &str, started: bool) {
    d.servers.share_scroll = 0.0;
    d.servers.copied = None;
    d.overlay = Some(setup::Overlay::ShareMatch {
        match_id: match_id.to_string(),
        started,
    });
}

/// After the tick: starts the game into the match a join waits for, opens the card of a
/// match just started, and has the window ask for attention when an invite arrived.
pub(super) fn follow_up(d: &mut Dashboard, ctx: &egui::Context) {
    if let Some(lobby) = d.servers.launch.take() {
        if starts_pc(d) {
            super::play::join(d, ctx, super::play::Join::lobby(lobby));
        }
    }
    if let Some(id) = d.servers.ready.take() {
        share(d, &id, true);
    }
    if std::mem::take(&mut d.servers.attention) {
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
            egui::UserAttentionType::Informational,
        ));
    }
}

/// "Start a server": mode, place, guild (and map, for Combat).
pub(super) fn start_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let Some(setup::Overlay::StartServer {
        mode,
        region,
        guild,
        level,
    }) = d.overlay.clone()
    else {
        return;
    };
    let (w, h) = (dz(1000.0), dz(640.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, "Start a server");
    let set = |d: &mut Dashboard, m: usize, r: Option<String>, g: Option<String>, l: usize| {
        d.overlay = Some(setup::Overlay::StartServer {
            mode: m,
            region: r,
            guild: g,
            level: l,
        });
    };

    // Mode.
    k.caption(x, y, "Mode");
    let bw = (cw - 3.0 * dz(10.0)) / 4.0;
    for (i, (_, label)) in game::START_MODES.iter().enumerate() {
        let tone = if i == mode { Tone::Blue } else { Tone::Dark };
        let bx = x + i as f32 * (bw + dz(10.0));
        if k.button(
            &format!("start-mode-{i}"),
            bx,
            y + dz(26.0),
            bw,
            dz(40.0),
            tone,
            None,
            label,
            true,
            "",
        )
        .clicked
        {
            set(d, i, region.clone(), guild.clone(), level);
        }
    }
    let mut ty = y + dz(26.0 + 40.0 + 24.0);
    let combat = game::START_MODES[mode].0.contains("combat");
    if combat {
        k.caption(x, ty, "Map");
        for (i, (_, label)) in game::COMBAT_LEVELS.iter().enumerate() {
            let tone = if i == level { Tone::Blue } else { Tone::Dark };
            let bx = x + i as f32 * (bw + dz(10.0));
            if k.button(
                &format!("start-level-{i}"),
                bx,
                ty + dz(26.0),
                bw,
                dz(40.0),
                tone,
                None,
                label,
                true,
                "",
            )
            .clicked
            {
                set(d, mode, region.clone(), guild.clone(), i);
            }
        }
        ty += dz(26.0 + 40.0 + 24.0);
    }

    // Guild: the one you may start matches for (picked when there are several).
    let guilds = d.servers.guilds.clone();
    let chosen_guild = guild
        .clone()
        .or_else(|| guilds.first().map(|(id, _)| id.clone()));
    if guilds.len() > 1 {
        k.caption(x, ty, "Guild");
        for (i, (id, name)) in guilds.iter().take(4).enumerate() {
            let on = chosen_guild.as_deref() == Some(id);
            let tone = if on { Tone::Blue } else { Tone::Dark };
            let bx = x + i as f32 * (bw + dz(10.0));
            let label = if name.is_empty() { id } else { name };
            if k.button(
                &format!("start-guild-{i}"),
                bx,
                ty + dz(26.0),
                bw,
                dz(40.0),
                tone,
                None,
                label,
                true,
                "",
            )
            .clicked
            {
                set(d, mode, region.clone(), Some(id.clone()), level);
            }
        }
        ty += dz(26.0 + 40.0 + 24.0);
    }

    // Where: the places a server waits for this guild.
    k.caption(x, ty, "Where");
    let list_top = ty + dz(26.0);
    let by = bottom - BTN_H;
    match d.servers.regions.clone() {
        None => {
            k.caps_text(
                x,
                list_top,
                cw,
                "Looking for free servers…",
                16.0,
                design::BODY,
                0.0,
            );
        }
        Some(Err(e)) => {
            k.caps_text(
                x,
                list_top,
                cw,
                &format!("Couldn't find free servers: {e}"),
                16.0,
                design::DANGER,
                0.0,
            );
        }
        Some(Ok(regions)) => {
            let operator = d.servers.role.as_ref().is_some_and(Role::operator);
            let usable: Vec<Region> = regions
                .into_iter()
                .filter(|r| operator || chosen_guild.as_ref().is_some_and(|g| r.groups.contains(g)))
                .collect();
            if usable.is_empty() {
                k.caps_text(
                    x,
                    list_top,
                    cw,
                    "No free server is waiting for your guild right now.",
                    16.0,
                    design::BODY,
                    0.0,
                );
            }
            let cols = 3usize;
            let rw = (cw - 2.0 * dz(10.0)) / cols as f32;
            let max_rows = ((by - dz(20.0) - list_top) / dz(50.0)).floor().max(1.0) as usize;
            for (i, r) in usable.iter().take(cols * max_rows).enumerate() {
                let on = region.as_deref() == Some(r.code.as_str());
                let tone = if on { Tone::Blue } else { Tone::Dark };
                let bx = x + (i % cols) as f32 * (rw + dz(10.0));
                let byy = list_top + (i / cols) as f32 * dz(50.0);
                let label = if r.location.is_empty() {
                    &r.code
                } else {
                    &r.location
                };
                if k.button(
                    &format!("start-region-{i}"),
                    bx,
                    byy,
                    rw,
                    dz(40.0),
                    tone,
                    Some(Icon::Globe),
                    label,
                    true,
                    &r.code,
                )
                .clicked
                {
                    set(d, mode, Some(r.code.clone()), chosen_guild.clone(), level);
                }
            }
        }
    }

    // Start / Cancel.
    let sw = k.button_width("Start", None, BTN_H).max(dz(220.0));
    let cw2 = k.button_width("Cancel", None, BTN_H).max(dz(160.0));
    let right = x + cw;
    if k.button(
        "start-cancel",
        right - cw2,
        by,
        cw2,
        BTN_H,
        Tone::Dark,
        None,
        "Cancel",
        true,
        "",
    )
    .clicked
        || k.key(egui::Key::Escape)
    {
        d.overlay = None;
        return;
    }
    let ready = region.is_some() && chosen_guild.is_some() && !d.servers.busy;
    if k.button(
        "start-go",
        right - cw2 - dz(12.0) - sw,
        by,
        sw,
        BTN_H,
        Tone::Go,
        None,
        "Start",
        ready,
        "Start the match: then copy its link, invite friends and join it",
    )
    .clicked
    {
        let (Some(region), Some(guild), Some((tokens, _))) =
            (region, chosen_guild, d.vrce.session())
        else {
            return;
        };
        d.overlay = None;
        d.servers.busy = true;
        let mode_id = game::START_MODES[mode].0;
        let level_id = combat.then(|| game::COMBAT_LEVELS[level].0);
        d.servers.worker.spawn(ctx, move |tx| {
            let r = game::start(&tokens.token, &guild, mode_id, &region, level_id)
                .map(Done::Started)
                .map_err(|e| format!("Couldn't start the server: {e:#}"));
            tx.send(Msg::Done(r));
        });
    }
}

/// A match's card: its link with COPY, INVITE for each friend not in it, and JOIN NOW
/// (unless you are in it).
pub(super) fn share_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let Some(setup::Overlay::ShareMatch { match_id, started }) = d.overlay.clone() else {
        return;
    };
    let me = d.vrce.session().map(|(_, a)| a.id).unwrap_or_default();
    let (w, h) = (dz(1000.0), dz(600.0));
    let title = if started {
        "Your match is ready"
    } else {
        "Share your match"
    };
    let (x, y, cw, bottom) = setup::card(k, w, h, title);
    let m = d.servers.find(&match_id).cloned();
    let inside = |id: &str| {
        m.as_ref()
            .is_some_and(|m| m.players.iter().any(|p| p.user_id == id))
    };
    let joined = inside(&me);
    let line = format!(
        "{}. Send its link to the players: it opens Echo VR (or Spark) right in this match.",
        about(m.as_ref())
    );
    let th = k.caps_text(x, y, cw, &line, 17.0, design::BODY, 0.0);

    // The link, and COPY.
    let mut ty = y + th + dz(22.0);
    k.caption(x, ty, "Link");
    ty += dz(26.0);
    let copied = d
        .servers
        .copied
        .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
    let label = if copied { "Copied" } else { "Copy" };
    let bw = k
        .button_width("Copied", Some(Icon::Copy), BTN_H)
        .max(dz(170.0));
    let mut link = game::share_link(&match_id);
    k.field(
        "share-link",
        &mut link,
        x,
        ty,
        cw - bw - dz(12.0),
        BTN_H,
        "",
        false,
        "",
    );
    if k.button(
        "share-copy",
        x + cw - bw,
        ty,
        bw,
        BTN_H,
        Tone::Blue,
        Some(Icon::Copy),
        label,
        true,
        "Put the link on the clipboard",
    )
    .clicked
    {
        copy_link(d, ctx, &match_id);
    }
    if copied {
        ctx.request_repaint_after(Duration::from_millis(500));
    }
    ty += BTN_H + dz(30.0);

    // Friends not in the match, with INVITE.
    k.caption(x, ty, "Invite friends");
    ty += dz(30.0);
    let by = bottom - BTN_H;
    let mut friends: Vec<Friend> = d
        .servers
        .friends
        .iter()
        .filter(|f| !inside(&f.user_id))
        .cloned()
        .collect();
    friends.sort_by_key(|f| f.name.to_ascii_lowercase());
    let list_h = by - dz(24.0) - ty;
    if friends.is_empty() {
        let text = if d.servers.friends.is_empty() {
            "No friends yet: add them on the EchoVRCE page."
        } else {
            "All your friends are in this match."
        };
        k.caps_text(x, ty, cw, text, 15.0, design::GREY, 0.0);
    } else {
        let row_h = dz(56.0);
        k.scroll_area(
            "share-friends",
            x,
            ty,
            cw + dz(14.0),
            list_h,
            friends.len() as f32 * row_h,
            &mut d.servers.share_scroll,
        );
        let scroll = d.servers.share_scroll;
        let servers = d.servers.list.clone();
        let (invited, inviting) = (&d.servers.invited, &d.servers.inviting);
        let mut invite_it = None;
        k.clipped(x, ty, cw, list_h, |k| {
            for (i, f) in friends.iter().enumerate() {
                let ry = ty - scroll + i as f32 * row_h;
                if ry + row_h < ty || ry > ty + list_h {
                    continue;
                }
                let name = k.label_galley(&f.name, design::din(16.0), design::TEXT, cw - dz(200.0));
                k.put(x, ry, name);
                let what = game::friend_match(&servers, f)
                    .map_or("Not in a match".to_string(), |m| about(Some(m)));
                let wg = k.label_galley(&what, design::din(13.0), design::GREY, cw - dz(200.0));
                k.put(x, ry + dz(22.0), wg);
                if invite_button(k, f, &match_id, invited, inviting, x + cw, ry + dz(4.0)) {
                    invite_it = Some(f.clone());
                }
            }
        });
        if let Some(f) = invite_it {
            invite(d, ctx, &f, &match_id);
        }
    }

    // JOIN NOW / CLOSE.
    let cw2 = k.button_width("Close", None, BTN_H).max(dz(160.0));
    let right = x + cw;
    if k.button(
        "share-close",
        right - cw2,
        by,
        cw2,
        BTN_H,
        Tone::Dark,
        None,
        "Close",
        true,
        "",
    )
    .clicked
        || k.key(egui::Key::Escape)
    {
        d.overlay = None;
        return;
    }
    if !joined {
        let jw = k.button_width("Join now", None, BTN_H).max(dz(200.0));
        let tip = if starts_pc(d) {
            "Start Echo VR straight into this match"
        } else {
            "Queue this match as your next one"
        };
        if k.button(
            "share-join",
            right - cw2 - dz(12.0) - jw,
            by,
            jw,
            BTN_H,
            Tone::Go,
            None,
            "Join now",
            !d.servers.busy,
            tip,
        )
        .clicked
        {
            d.overlay = None;
            join(d, ctx, &match_id);
        }
    }
}

/// INVITE for friend `f` (right-aligned at `right`), or how far that got; returns the
/// click.
fn invite_button(
    k: &mut Kit,
    f: &Friend,
    match_id: &str,
    invited: &HashSet<(String, String)>,
    inviting: &HashSet<String>,
    right: f32,
    y: f32,
) -> bool {
    if invited.contains(&(f.user_id.clone(), match_id.to_string())) {
        let cw = k.chip_width("Invited");
        k.chip(right - cw, y + dz(4.5), "Invited", design::QUEST_ON);
        return false;
    }
    let busy = inviting.contains(&f.user_id);
    let bw = dz(120.0);
    let label = if busy { "Inviting…" } else { "Invite" };
    k.button(
        &format!("invite-{}", f.user_id),
        right - bw,
        y,
        bw,
        dz(36.0),
        Tone::Blue,
        None,
        label,
        !busy,
        "Invite them to this match",
    )
    .clicked
}

// ---- the page ----

pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let session = d.vrce.session();
    let (dx, dy) = (kit.dx(), kit.dy());
    match &session {
        Some((tokens, _)) => list_card(d, kit, ctx, tokens, LIST.wider(dx).taller(dy)),
        None => signed_out(d, kit, ctx),
    }
    let account = session.map(|(_, a)| a);
    panel::at_right(kit, |k| {
        you_card(d, k, ctx, account.as_ref(), YOU);
        friends_card(d, k, ctx, account.as_ref(), FRIENDS.taller(dy));
    });
}

/// Not signed in: what the page needs, and the way there.
fn signed_out(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let r = Dr::new(LIST.x, LIST.y, LIST.w + kit.dx(), 300.0);
    let title = if d.vrce.waiting_for_server() {
        "EchoVRCE isn't answering"
    } else {
        "Sign in to see the servers"
    };
    let (x, y, w, bottom) = hero::card_frame(kit, r, title);
    super::echovrce::prompt_text(d, kit, (x, y, w), "The servers, joining them, starting one, your party and friends come from EchoVRCE: sign in with your EchoVRCE account on its page.", 17.0, true);
    super::echovrce::sign_in_button(d, kit, ctx, "servers-sign-in", x, bottom - BTN_H);
}

/// The servers (or your match history), as VERSIONS has its rows.
fn list_card(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context, tokens: &Tokens, r: Dr) {
    kit.image_d("card_bg.png", r);
    kit.gradient_frame(
        kit.drect(r),
        dz(6.0),
        dz(2.0),
        design::RIM_TOP,
        design::RIM_BOTTOM,
    );
    let (x, w) = (dz(r.x + 22.0), dz(r.w - 44.0));
    let top = dz(r.y + 20.0);

    // Tabs: the design's title, the other one greyed.
    let mut tx = x;
    for (tab, title) in [(Tab::Live, "Servers"), (Tab::History, "My matches")] {
        let on = d.servers.tab == tab;
        let color = if on { design::TEXT } else { design::GREY };
        let g = kit.spaced_galley(
            &title.to_uppercase(),
            design::conthrax(24.0),
            color,
            dz(1.8),
            false,
        );
        let gr = kit.rect(tx, top, g.size().x, g.size().y);
        let (resp, _, _) = kit.hot(&format!("servers-tab-{title}"), gr, !on, title);
        kit.put(tx, top, g);
        if resp.clicked {
            d.servers.tab = tab;
            d.servers.scroll = 0.0;
        }
        tx += gr.width() + dz(40.0);
    }

    // On the title row's right: Refresh, and left of it the mode filters (servers only).
    let live = d.servers.tab == Tab::Live;
    let s = dz(36.0);
    let tip = if live {
        "Read the server list again"
    } else {
        "Read your matches again"
    };
    if kit
        .button(
            "servers-refresh",
            x + w - s,
            top,
            s,
            s,
            Tone::Dark,
            Some(Icon::Refresh),
            "",
            !d.servers.loading,
            tip,
        )
        .clicked
    {
        if live {
            d.servers.refresh(ctx, &tokens.token);
        } else {
            d.servers.history = None;
            d.servers.history_at = None;
        }
    }
    if live {
        let modes = [
            (None, "All"),
            (Some(Mode::Arena), "Arena"),
            (Some(Mode::Combat), "Combat"),
            (Some(Mode::Social), "Social"),
        ];
        let fw = dz(110.0);
        let mut fx = x + w - s - dz(16.0) - modes.len() as f32 * (fw + dz(8.0)) + dz(8.0);
        for (mode, label) in modes {
            let on = d.servers.filter == mode;
            let tone = if on { Tone::Blue } else { Tone::Dark };
            if kit
                .button(
                    &format!("servers-filter-{label}"),
                    fx,
                    top,
                    fw,
                    dz(36.0),
                    tone,
                    None,
                    label,
                    true,
                    "",
                )
                .clicked
            {
                d.servers.filter = mode;
                d.servers.scroll = 0.0;
            }
            fx += fw + dz(8.0);
        }
    }

    // Column captions over the rows, and when the list was read.
    let head = top + dz(62.0);
    let columns = if live {
        [("Match", 6.0), ("Players", 470.0), ("Score · time", 600.0)]
    } else {
        [("Match", 6.0), ("Score", 470.0), ("Length", 600.0)]
    };
    for (label, cx) in columns {
        kit.caption(x + dz(cx), head, label);
    }
    let right = match d.servers.updated {
        _ if !live => "Result".to_string(),
        _ if d.servers.loading => "Reading…".to_string(),
        Some(t) => format!("Updated {}s ago", t.elapsed().as_secs()),
        None => String::new(),
    };
    let g = kit.label_galley(&right, design::din(14.0), design::GREY, f32::INFINITY);
    kit.put(x + w - dz(8.0) - g.size().x, head, g);
    let list_top = head + dz(34.0);
    let bottom = dz(r.bottom() - 22.0);
    match d.servers.tab {
        Tab::Live => live_rows(d, kit, ctx, x, list_top, w, bottom),
        Tab::History => history_rows(d, kit, x, list_top, w, bottom),
    }
}

#[allow(clippy::too_many_arguments)]
fn live_rows(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    x: f32,
    top: f32,
    w: f32,
    bottom: f32,
) {
    let filter = d.servers.filter;
    let list: Vec<Match> = d
        .servers
        .list
        .iter()
        .filter(|m| m.lobby != Lobby::Unassigned && filter.is_none_or(|f| m.mode == f))
        .cloned()
        .collect();
    if list.is_empty() {
        let text = match (&d.servers.error, d.servers.updated) {
            (Some(e), _) => format!("The servers couldn't be read: {e}"),
            (None, None) => "Reading the servers…".to_string(),
            (None, Some(_)) => "No match is running right now.".to_string(),
        };
        kit.caps_text(x, top, w, &text, 16.0, design::BODY, 0.0);
        return;
    }
    let (row_h, pad) = (dz(ROW_H), dz(ROW_PAD));
    let list_h = bottom - top;
    kit.scroll_area(
        "servers",
        x,
        top,
        w + pad + dz(4.0),
        list_h,
        list.len() as f32 * row_h,
        &mut d.servers.scroll,
    );
    let scroll = d.servers.scroll;
    let busy = d.servers.busy;
    // Links are shared for public matches, and the one you are in or started.
    let me = d.vrce.session().map(|(_, a)| a.id).unwrap_or_default();
    let own: Vec<String> = game::find_me(&list, &me)
        .map(|(m, _)| m.id.clone())
        .into_iter()
        .chain(d.servers.created.as_ref().map(|c| c.id.clone()))
        .collect();
    let mut join_it = None;
    let mut copy_it = None;
    kit.clipped(x - pad, top, w + 2.0 * pad, list_h, |k| {
        for (i, m) in list.iter().enumerate() {
            let ry = top - scroll + i as f32 * row_h;
            if ry + row_h < top || ry > bottom {
                continue;
            }
            let rr = k.rect(x - pad, ry, w + 2.0 * pad - dz(8.0), row_h - dz(8.0));
            if i % 2 == 0 {
                k.ui.painter().rect_filled(rr, dz(6.0), egui::Color32::from_white_alpha(6));
            }
            // Left: mode and who may join; under it, where.
            let lobby = match m.lobby {
                Lobby::Public => "Public",
                Lobby::Private => "Private",
                Lobby::Unassigned => "Waiting",
            };
            let name = k.label_galley(&format!("{}  ·  {lobby}", m.mode.label()), design::din(18.0), design::TEXT, dz(420.0));
            k.put(x + dz(6.0), ry + dz(8.0), name);
            let place = k.label_galley(&m.location, design::din(14.0), design::GREY, dz(420.0));
            k.put(x + dz(6.0), ry + dz(36.0), place);

            // Middle: players, score, running time.
            let players = if m.limit > 0 {
                format!("{} / {}", m.size, m.limit)
            } else {
                m.size.to_string()
            };
            let pg = k.label_galley(&players, design::din(20.0), design::TEXT, dz(140.0));
            let cy = ry + (row_h - dz(8.0)) / 2.0;
            k.put(x + dz(470.0), cy - pg.size().y / 2.0, pg);
            let mut detail = Vec::new();
            if let Some((b, o)) = m.score {
                detail.push(format!("{b} – {o}"));
            }
            if let Some(s) = m.started {
                let mins = (time::OffsetDateTime::now_utc().unix_timestamp() - s).max(0) / 60;
                detail.push(format!("{mins} min"));
            }
            let dg = k.label_galley(&detail.join("   ·   "), design::din(15.0), design::SUBTLE, dz(260.0));
            k.put(x + dz(600.0), cy - dg.size().y / 2.0, dg);

            // Right: JOIN, or why not; left of it, the link to copy.
            let bw = dz(130.0);
            let bx = x + w - bw - dz(8.0);
            let left = if m.joinable() {
                let tip = "Start Echo VR straight into this match (with the game running: queue it as your next)";
                if k.button(&format!("join-{}", m.id), bx, cy - dz(20.0), bw, dz(40.0), Tone::Go, None, "Join", !busy, tip).clicked {
                    join_it = Some(m.id.clone());
                }
                bx
            } else {
                let why = if m.lobby == Lobby::Private { "Private" } else { "Full" };
                let cw = k.chip_width(why);
                k.chip(x + w - cw - dz(8.0), cy - dz(13.5), why, design::QUEST_OFF);
                x + w - cw - dz(8.0)
            };
            if m.lobby == Lobby::Public || own.contains(&m.id) {
                let s = dz(40.0);
                let tip = "Copy this match's link, to send to players";
                if k.button(&format!("copy-{}", m.id), left - dz(10.0) - s, cy - dz(20.0), s, s, Tone::Dark, Some(Icon::Copy), "", true, tip).clicked {
                    copy_it = Some(m.id.clone());
                }
            }
        }
    });
    if let Some(id) = join_it {
        join(d, ctx, &id);
    }
    if let Some(id) = copy_it {
        copy_link(d, ctx, &id);
    }
}

fn history_rows(d: &mut Dashboard, kit: &mut Kit, x: f32, top: f32, w: f32, bottom: f32) {
    let rows = match d.servers.history.clone() {
        None => {
            kit.caps_text(x, top, w, "Reading your matches…", 16.0, design::BODY, 0.0);
            return;
        }
        Some(Err(e)) => {
            kit.caps_text(
                x,
                top,
                w,
                &format!("Your matches couldn't be read: {e}"),
                16.0,
                design::DANGER,
                0.0,
            );
            return;
        }
        Some(Ok(rows)) if rows.is_empty() => {
            kit.caps_text(x, top, w, "No matches yet.", 16.0, design::BODY, 0.0);
            return;
        }
        Some(Ok(rows)) => rows,
    };
    let (row_h, pad) = (dz(ROW_H), dz(ROW_PAD));
    let list_h = bottom - top;
    kit.scroll_area(
        "history",
        x,
        top,
        w + pad + dz(4.0),
        list_h,
        rows.len() as f32 * row_h,
        &mut d.servers.scroll,
    );
    let scroll = d.servers.scroll;
    kit.clipped(x - pad, top, w + 2.0 * pad, list_h, |k| {
        for (i, s) in rows.iter().enumerate() {
            let ry = top - scroll + i as f32 * row_h;
            if ry + row_h < top || ry > bottom {
                continue;
            }
            let rr = k.rect(x - pad, ry, w + 2.0 * pad - dz(8.0), row_h - dz(8.0));
            if i % 2 == 0 {
                k.ui.painter()
                    .rect_filled(rr, dz(6.0), egui::Color32::from_white_alpha(6));
            }
            let name = k.label_galley(s.mode.label(), design::din(18.0), design::TEXT, dz(300.0));
            k.put(x + dz(6.0), ry + dz(8.0), name);
            let when = s
                .played
                .and_then(|t| time::OffsetDateTime::from_unix_timestamp(t).ok())
                .map(feed::footer_time)
                .unwrap_or_default();
            let wg = k.label_galley(&when, design::din(14.0), design::GREY, dz(400.0));
            k.put(x + dz(6.0), ry + dz(36.0), wg);
            let cy = ry + (row_h - dz(8.0)) / 2.0;
            if let Some((b, o)) = s.score {
                let sg = k.label_galley(
                    &format!("{b} – {o}"),
                    design::din(20.0),
                    design::TEXT,
                    dz(160.0),
                );
                k.put(x + dz(470.0), cy - sg.size().y / 2.0, sg);
            }
            if let Some(secs) = s.duration_s {
                let g = k.label_galley(
                    &format!("{} min", secs / 60),
                    design::din(15.0),
                    design::SUBTLE,
                    dz(200.0),
                );
                k.put(x + dz(600.0), cy - g.size().y / 2.0, g);
            }
            let (chip, color) = match s.won() {
                Some(true) => ("Won", design::QUEST_ON),
                Some(false) => ("Lost", design::RED),
                None => ("", design::QUEST_OFF),
            };
            if !chip.is_empty() {
                let cw = k.chip_width(chip);
                k.chip(x + w - cw - dz(8.0), cy - dz(13.5), chip, color);
            }
        }
    });
}

/// Where you are, as YOUR MATCH's tile says it.
struct Status {
    title: String,
    line: String,
    chip: Option<(&'static str, egui::Color32)>,
    /// In a match or the queue: the tile is blue.
    active: bool,
    party: Vec<String>,
}

fn status(d: &Dashboard, account: Option<&Account>, created: Option<&str>) -> Status {
    let plain = |title: &str, line: &str| Status {
        title: title.to_string(),
        line: line.to_string(),
        chip: None,
        active: false,
        party: Vec::new(),
    };
    let Some(account) = account else {
        return plain("Signed out", "Sign in on the EchoVRCE page.");
    };
    if let Some((m, party)) = game::find_me(&d.servers.list, &account.id) {
        return Status {
            title: "In a match".into(),
            line: format!("{} · {} players", about(Some(m)), m.size),
            chip: Some(("Live", design::QUEST_ON)),
            active: true,
            party: party.iter().map(|p| p.name.clone()).collect(),
        };
    }
    if let Some(t) = d.servers.tickets.first() {
        let waited = t
            .since
            .map(|s| (time::OffsetDateTime::now_utc().unix_timestamp() - s).max(0))
            .map(|s| format!(" · {}:{:02}", s / 60, s % 60))
            .unwrap_or_default();
        let party = match t.players.len() {
            0 | 1 => String::new(),
            n => format!(", party of {n}"),
        };
        return Status {
            title: format!("Searching{waited}"),
            line: format!("For a {} match{party}", t.mode.label()),
            chip: Some(("Queue", design::BLUE)),
            active: true,
            party: t.players.clone(),
        };
    }
    match created {
        Some(id) => plain("Your match is ready", &about(d.servers.find(id))),
        None => plain(
            "Not in a match",
            "Join one on the left, start one, or join one from a link.",
        ),
    }
}

/// YOUR MATCH: where you are (with your match's link and invites), your party, invites
/// to you, and START A SERVER and JOIN FROM LINK at the bottom.
fn you_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context, account: Option<&Account>, r: Dr) {
    let (x, y, w, bottom) = hero::card_frame(k, r, "Your match");
    let pad = dz(TILE_PAD);
    let mine = account
        .and_then(|a| game::find_me(&d.servers.list, &a.id))
        .map(|(m, _)| m.id.clone());
    let created = d.servers.created.as_ref().map(|c| c.id.clone());
    let st = status(d, account, created.as_deref());
    // Your match (or the one you started): its link, and invites.
    let shared = mine.or(created);

    // Where you are: a tile, with COPY LINK and INVITE FRIENDS for your match.
    let mut ty = y;
    let th = dz(if shared.is_some() { 138.0 } else { 82.0 });
    hero::tile(k, x, ty, w, th, st.active);
    let tx = x + pad + dz(6.0);
    let mut chip_w = 0.0;
    if let Some((text, color)) = st.chip {
        chip_w = k.chip_width(text);
        k.chip(x + w - pad - chip_w, ty + pad, text, color);
    }
    let title = k.label_galley(
        &st.title,
        design::din(18.0),
        design::TEXT,
        w - 3.0 * pad - chip_w,
    );
    k.put(tx, ty + pad, title);
    let line = k.label_galley(
        &st.line,
        design::din(14.0),
        design::GREY,
        w - 2.0 * pad - dz(6.0),
    );
    k.put(tx, ty + pad + dz(28.0), line);
    if let Some(id) = &shared {
        let by = ty + th - pad - dz(ACTION_H);
        let bw = (w - 2.0 * pad - dz(8.0)) / 2.0;
        let copied = d
            .servers
            .copied
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
        let label = if copied { "Copied" } else { "Copy link" };
        if k.button(
            "now-copy",
            x + pad,
            by,
            bw,
            dz(ACTION_H),
            Tone::Dark,
            Some(Icon::Copy),
            label,
            true,
            "Copy the match's link, to send to players",
        )
        .clicked
        {
            copy_link(d, ctx, id);
        }
        if k.button(
            "now-invite",
            x + pad + bw + dz(8.0),
            by,
            bw,
            dz(ACTION_H),
            Tone::Blue,
            None,
            "Invite friends",
            true,
            "The match's link, and invites for your friends",
        )
        .clicked
        {
            share(d, id, false);
        }
        if copied {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
    ty += th + dz(22.0);

    // Your party, as chips.
    if st.party.len() > 1 {
        k.caption(x, ty, "Party");
        ty += dz(28.0);
        let mut cx = x;
        for name in &st.party {
            let cw = k.chip_width(name);
            if cx > x && cx + cw > x + w {
                cx = x;
                ty += dz(34.0);
            }
            k.chip(cx, ty, name, design::QUEST_OFF);
            cx += cw + dz(8.0);
        }
        ty += dz(27.0 + 22.0);
    }

    // Invites to you, above the buttons: as many as fit.
    let buttons_y = bottom - BTN_H;
    invite_tiles(d, k, ctx, x, ty, w, buttons_y - dz(16.0), usize::MAX);

    // START A SERVER (for guilds that may) and JOIN FROM LINK.
    let can_start = account.is_some() && d.servers.role.as_ref().is_some_and(Role::can_start);
    let gap = dz(12.0);
    let lw = if can_start { (w - gap) / 2.0 } else { w };
    if can_start
        && k.button(
            "servers-start",
            x,
            buttons_y,
            lw,
            BTN_H,
            Tone::Blue,
            Some(Icon::Globe),
            "Start a server",
            !d.servers.busy,
            "Start a new match on a free server",
        )
        .clicked
    {
        if let Some((tokens, _)) = d.vrce.session() {
            d.overlay = Some(setup::Overlay::StartServer {
                mode: 0,
                region: None,
                guild: None,
                level: 0,
            });
            d.servers.fetch_regions(ctx, &tokens.token);
        }
    }
    let lx = if can_start { x + lw + gap } else { x };
    if k.button(
        "servers-link",
        lx,
        buttons_y,
        lw,
        BTN_H,
        Tone::Dark,
        None,
        "Join from link",
        true,
        "Join a match from a spark:// or echo.taxi link",
    )
    .clicked
    {
        super::play::open_lobby_card(d);
    }
}

/// Invites to you under an INVITES caption from `y`, a tile each with JOIN and a close
/// button: as many as fit above `bottom`, at most `max`. Returns where the next thing goes
/// (`y` when there are none).
#[allow(clippy::too_many_arguments)]
pub(super) fn invite_tiles(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    x: f32,
    y: f32,
    w: f32,
    bottom: f32,
    max: usize,
) -> f32 {
    let pad = dz(TILE_PAD);
    let invites: Vec<Invite> = d.servers.open_invites().into_iter().cloned().collect();
    let pitch = dz(SMALL_TILE_H + TILE_GAP);
    let room = ((bottom - y - dz(28.0) + dz(TILE_GAP)) / pitch)
        .floor()
        .max(0.0) as usize;
    let room = room.min(max);
    if invites.is_empty() || room == 0 {
        return y;
    }
    let mut ty = y;
    k.caption(x, ty, "Invites");
    if invites.len() > room {
        let more = format!("+{} more", invites.len() - room);
        let g = k.label_galley(&more, design::din(14.0), design::GREY, f32::INFINITY);
        k.put(x + w - g.size().x, ty, g);
    }
    ty += dz(28.0);
    let busy = d.servers.busy;
    let mut accept_it = None;
    let mut dismiss_it = None;
    for inv in invites.iter().take(room) {
        let h = dz(SMALL_TILE_H);
        hero::tile(k, x, ty, w, h, false);
        let s = dz(36.0);
        let bw = dz(100.0);
        let text_w = w - 2.0 * pad - s - bw - dz(16.0);
        let name = k.label_galley(
            &format!("{} invited you", sender(inv)),
            design::din(16.0),
            design::TEXT,
            text_w,
        );
        k.put(x + pad, ty + dz(11.0), name);
        let what = about(d.servers.find(&inv.match_id));
        let wg = k.label_galley(&what, design::din(13.0), design::GREY, text_w);
        k.put(x + pad, ty + dz(35.0), wg);
        let cy = ty + (h - s) / 2.0;
        if k.button(
            &format!("dismiss-{}", inv.id),
            x + w - pad - s,
            cy,
            s,
            s,
            Tone::Dark,
            Some(Icon::Close),
            "",
            true,
            "Hide this invite",
        )
        .clicked
        {
            dismiss_it = Some(inv.id.clone());
        }
        if k.button(
            &format!("accept-{}", inv.id),
            x + w - pad - s - dz(8.0) - bw,
            cy,
            bw,
            s,
            Tone::Go,
            None,
            "Join",
            !busy,
            "Join their match",
        )
        .clicked
        {
            accept_it = Some(inv.clone());
        }
        ty += pitch;
    }
    if let Some(id) = dismiss_it {
        d.servers.dismissed.insert(id);
    }
    if let Some(inv) = accept_it {
        accept(d, ctx, &inv);
    }
    ty - dz(TILE_GAP) + dz(22.0)
}

/// FRIENDS: a tile each, those in a match first, with JOIN when it can be joined or
/// INVITE into your match.
fn friends_card(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    account: Option<&Account>,
    r: Dr,
) {
    let (x, y, w, bottom) = hero::card_frame(k, r, "Friends");
    let Some(account) = account else {
        k.caps_text(
            x,
            y,
            w,
            "Sign in to see your friends and the matches they are in.",
            15.0,
            design::GREY,
            0.0,
        );
        return;
    };
    let mut scroll = d.servers.friends_scroll;
    let playing = friend_tiles(
        d,
        k,
        ctx,
        account,
        "friends",
        (x, y, w, bottom),
        &mut scroll,
        false,
    );
    d.servers.friends_scroll = scroll;
    // How many are playing, on the title row's right.
    if let Some(playing) = playing {
        let g = k.label_galley(
            &format!("{playing} in a match"),
            design::din(14.0),
            design::GREY,
            f32::INFINITY,
        );
        k.put(x + w - g.size().x, dz(r.y + 28.0), g);
    }
}

/// The friends in `area` (left, top, width, bottom), scrolled by `scroll` (`key` names the
/// scroll area): a tile each, those in a match first, with JOIN when it can be joined or
/// INVITE into your match, and (`removable`, the Friends page) a button to remove them
/// (`Servers::remove_clicked`). Returns how many are in a match, or `None` without friends
/// (it says how to add some instead).
#[allow(clippy::too_many_arguments)]
pub(super) fn friend_tiles(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    account: &Account,
    key: &str,
    area: (f32, f32, f32, f32),
    scroll: &mut f32,
    removable: bool,
) -> Option<usize> {
    let (x, y, w, bottom) = area;
    let servers = d.servers.list.clone();
    let mine = game::find_me(&servers, &account.id).map(|(m, _)| m.id.clone());
    let shared = mine
        .clone()
        .or(d.servers.created.as_ref().map(|c| c.id.clone()));
    let mut friends: Vec<(Friend, Option<Match>)> = d
        .servers
        .friends
        .iter()
        .map(|f| (f.clone(), game::friend_match(&servers, f).cloned()))
        .collect();
    friends.sort_by_key(|(f, m)| (m.is_none(), f.name.to_ascii_lowercase()));
    if friends.is_empty() {
        let text = if removable {
            "No friends yet: find players by name, or add the players of your last matches."
        } else {
            "No friends yet: find players and add them on the Friends page."
        };
        let th = k.caps_text(x, y, w, text, 15.0, design::GREY, 0.0);
        if !removable
            && k.link(
                &format!("{key}-open-friends"),
                x,
                y + th + dz(16.0),
                "Find friends",
                14.0,
                "",
            )
            .clicked
        {
            d.page = Page::Friends;
        }
        return None;
    }
    let playing = friends.iter().filter(|(_, m)| m.is_some()).count();

    let (h, pitch) = (dz(SMALL_TILE_H), dz(SMALL_TILE_H + TILE_GAP));
    let list_h = bottom - y;
    k.scroll_area(
        key,
        x,
        y,
        w + dz(16.0),
        list_h,
        friends.len() as f32 * pitch - dz(TILE_GAP),
        scroll,
    );
    let scroll = *scroll;
    let busy = d.servers.busy;
    let (invited, inviting) = (&d.servers.invited, &d.servers.inviting);
    let pad = dz(TILE_PAD);
    let mut join_it = None;
    let mut invite_it = None;
    let mut remove_it = None;
    let changing = &d.servers.changing;
    k.clipped(x, y, w, list_h, |k| {
        for (i, (f, m)) in friends.iter().enumerate() {
            let ty = y - scroll + i as f32 * pitch;
            if ty + pitch < y || ty > bottom {
                continue;
            }
            hero::tile(k, x, ty, w, h, false);
            let with_you = m.is_some() && m.as_ref().map(|m| &m.id) == mine.as_ref();
            let dot = if m.is_some() {
                design::QUEST_ON
            } else {
                design::QUEST_OFF
            };
            let c = k.rect(x + pad + dz(5.0), ty + h / 2.0, 0.0, 0.0).min;
            k.ui.painter().circle_filled(c, dz(5.0), dot);
            let tx = x + pad + dz(22.0);
            let text_w = w - (tx - x) - dz(150.0);
            let name = k.label_galley(&f.name, design::din(16.0), design::TEXT, text_w);
            k.put(tx, ty + dz(11.0), name);
            let what = match m {
                _ if with_you => "In your match".to_string(),
                Some(m) => about(Some(m)),
                None => "Not in a match".to_string(),
            };
            let wg = k.label_galley(&what, design::din(13.0), design::GREY, text_w);
            k.put(tx, ty + dz(35.0), wg);
            let (right, by) = (x + w - pad, ty + (h - dz(36.0)) / 2.0);
            if removable
                && k.button(
                    &format!("remove-friend-{}", f.user_id),
                    right - dz(120.0 + 8.0 + 36.0),
                    by,
                    dz(36.0),
                    dz(36.0),
                    Tone::Dark,
                    Some(Icon::Close),
                    "",
                    !changing.contains(&f.user_id),
                    "Remove from your friends",
                )
                .clicked
            {
                remove_it = Some((f.user_id.clone(), f.name.clone()));
            }
            match (m.as_ref().filter(|m| m.joinable() && !with_you), &shared) {
                (Some(m), _) => {
                    let bw = dz(120.0);
                    if k.button(
                        &format!("join-friend-{}", f.user_id),
                        right - bw,
                        by,
                        bw,
                        dz(36.0),
                        Tone::Go,
                        None,
                        "Join",
                        !busy,
                        "Join their match",
                    )
                    .clicked
                    {
                        join_it = Some(m.id.clone());
                    }
                }
                _ if with_you => {
                    let cw = k.chip_width("With you");
                    k.chip(right - cw, by + dz(4.5), "With you", design::BLUE);
                }
                (None, Some(id)) => {
                    if invite_button(k, f, id, invited, inviting, right, by) {
                        invite_it = Some((f.clone(), id.clone()));
                    }
                }
                (None, None) => {}
            }
        }
    });
    if let Some(id) = join_it {
        join(d, ctx, &id);
    }
    if let Some((f, id)) = invite_it {
        invite(d, ctx, &f, &id);
    }
    if remove_it.is_some() {
        d.servers.remove_clicked = remove_it;
    }
    Some(playing)
}
