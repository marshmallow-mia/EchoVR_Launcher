//! The game service's live side, as echovrce.com's own pages use it: the servers running
//! (and who is on them), joining one, starting a new one, your party and matchmaking
//! queue, your friends' matches, and your match history. Every call is made with the
//! signed-in session's token.

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::{api, call, message, SignedOut, WEB};

// ---- reading answers ----

fn s<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|k| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()))
        .unwrap_or_default()
}

fn n(v: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| v.get(k).and_then(Value::as_f64))
}

/// A time as Unix seconds: RFC 3339, or a number of seconds (or milliseconds).
fn unix(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::String(t) => crate::core::launcher::feed::parse_time(t).map(|t| t.unix_timestamp()),
        Value::Number(x) => x
            .as_f64()
            .map(|f| if f > 1e12 { f / 1000.0 } else { f } as i64),
        Value::Object(o) => o.get("seconds").and_then(Value::as_i64),
        _ => None,
    }
}

fn get(token: &str, url: &str) -> Result<Value> {
    let (status, body) = call(
        reqwest::Method::GET,
        url,
        None,
        Some(&format!("Bearer {token}")),
    )?;
    answer(status, body)
}

fn rpc(token: &str, name: &str, payload: Value) -> Result<Value> {
    let a = api()?;
    let (status, body) = call(
        reqwest::Method::POST,
        &format!("{}/rpc/{name}?unwrap", a.base),
        Some(&payload),
        Some(&format!("Bearer {token}")),
    )?;
    answer(status, body)
}

fn answer(status: u16, body: Value) -> Result<Value> {
    match status {
        200..=299 => Ok(body),
        401 => Err(SignedOut.into()),
        _ => bail!("{} ({status})", message(&body)),
    }
}

// ---- servers ----

/// A game mode, as the service names it (`echo_arena`, `echo_combat_private`, `social_2.0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Arena,
    Combat,
    Social,
    Other,
}

impl Mode {
    pub fn of(id: &str) -> Mode {
        let id = id.to_ascii_lowercase();
        if id.contains("arena") {
            Mode::Arena
        } else if id.contains("combat") {
            Mode::Combat
        } else if id.contains("social") {
            Mode::Social
        } else {
            Mode::Other
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Arena => "Arena",
            Mode::Combat => "Combat",
            Mode::Social => "Social",
            Mode::Other => "Other",
        }
    }
}

/// Who may join a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lobby {
    Public,
    Private,
    /// Waiting to be given a match (what new servers are started on).
    Unassigned,
}

/// Which team a player is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Team {
    Blue,
    Orange,
    Spectator,
    Social,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    pub user_id: String,
    pub discord_id: String,
    pub name: String,
    pub team: Team,
    /// Empty when not in a party.
    pub party_id: String,
    pub ping_ms: Option<u32>,
}

/// A server and the match on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// The service's match id (with its node suffix): joins use it.
    pub id: String,
    pub mode_id: String,
    pub mode: Mode,
    pub lobby: Lobby,
    pub open: bool,
    pub players: Vec<Player>,
    pub limit: u32,
    /// "Dallas, Texas"-like, or a region code.
    pub location: String,
    pub group_id: String,
    pub started: Option<i64>,
    /// Blue and orange.
    pub score: Option<(u32, u32)>,
    /// The region codes it can host (for starting new ones on it).
    pub region_codes: Vec<String>,
    /// The guilds that may start a match on it (when unassigned).
    pub group_ids: Vec<String>,
}

impl Match {
    /// The id the game takes as `-lobbyid` (a spark link's): without the node suffix.
    pub fn lobby_id(&self) -> String {
        lobby_id(&self.id)
    }

    /// Public, running, with room: anyone may join.
    pub fn joinable(&self) -> bool {
        self.lobby == Lobby::Public
            && self.open
            && !self.players.is_empty()
            && (self.limit == 0 || (self.players.len() as u32) < self.limit)
    }
}

/// The id the game takes as `-lobbyid` for the service's match id: without the node
/// suffix, uppercase.
pub fn lobby_id(match_id: &str) -> String {
    match_id
        .split('.')
        .next()
        .unwrap_or(match_id)
        .to_ascii_uppercase()
}

/// The link to send players so they join `match_id` (as echovrce.com shares it).
pub fn share_link(match_id: &str) -> String {
    format!("https://echo.taxi/spark://c/{}", lobby_id(match_id))
}

/// The `party_id` the service uses for "no party".
const NO_PARTY: &str = "00000000-0000-0000-0000-000000000000";

fn team(v: &Value) -> Team {
    let t = ["team", "team_id", "teamId", "role"]
        .iter()
        .find_map(|k| v.get(k).filter(|t| !t.is_null()));
    match t {
        Some(Value::String(s)) => match s.to_ascii_lowercase().as_str() {
            "blue" => Team::Blue,
            "orange" => Team::Orange,
            "spectator" => Team::Spectator,
            "social" => Team::Social,
            _ => Team::Other,
        },
        Some(Value::Number(n)) => match n.as_i64() {
            Some(0) => Team::Blue,
            Some(1) => Team::Orange,
            Some(2) => Team::Spectator,
            Some(3) => Team::Social,
            _ => Team::Other,
        },
        _ => Team::Other,
    }
}

/// One entry of `/match`: its id and its label (a JSON string, or `{value}`).
pub fn parse_match(entry: &Value) -> Option<Match> {
    let id = entry.get("match_id").and_then(Value::as_str)?.to_string();
    let raw = entry.get("label").map(|l| l.get("value").unwrap_or(l))?;
    let label: Value = match raw {
        Value::String(t) => serde_json::from_str(t).ok()?,
        v @ Value::Object(_) => v.clone(),
        _ => return None,
    };
    let mode_id = s(&label, &["mode", "game_type"]).to_string();
    let lobby = match ["lobby_type", "lobbyType", "lobbytype"]
        .iter()
        .find_map(|k| label.get(k))
    {
        Some(Value::String(t)) if t == "private" => Lobby::Private,
        Some(Value::String(t)) if t == "unassigned" => Lobby::Unassigned,
        Some(Value::Number(x)) if x.as_i64() == Some(1) => Lobby::Private,
        Some(Value::Number(x)) if x.as_i64() == Some(2) => Lobby::Unassigned,
        Some(_) => Lobby::Public,
        None if mode_id.contains("private") => Lobby::Private,
        None => Lobby::Public,
    };
    let players: Vec<Player> = label
        .get("players")
        .and_then(Value::as_array)
        .map(|ps| {
            ps.iter()
                .map(|p| Player {
                    user_id: s(p, &["user_id", "userId"]).to_string(),
                    discord_id: s(p, &["discord_id", "discordId"]).to_string(),
                    name: s(
                        p,
                        &[
                            "display_name",
                            "displayName",
                            "discord_username",
                            "username",
                        ],
                    )
                    .to_string(),
                    team: team(p),
                    party_id: Some(s(p, &["party_id", "partyId"]))
                        .filter(|id| *id != NO_PARTY)
                        .unwrap_or_default()
                        .to_string(),
                    ping_ms: n(p, &["ping_ms", "pingMillis", "ping", "latency_ms"])
                        .map(|x| x as u32),
                })
                .collect()
        })
        .unwrap_or_default();
    let server = label
        .get("broadcaster")
        .or_else(|| label.get("game_server"))
        .unwrap_or(&Value::Null);
    let city = s(&label, &["server_city"]);
    let city = if city.is_empty() {
        s(server, &["city"])
    } else {
        city
    };
    let region = s(&label, &["server_region"]);
    let region = if region.is_empty() {
        s(server, &["default_region", "region"])
    } else {
        region
    };
    let location = match (city, region) {
        ("", r) => r.to_string(),
        (c, "") => c.to_string(),
        (c, r) => format!("{c}, {r}"),
    };
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut region_codes = strings(server.get("region_codes"));
    if region_codes.is_empty() && !region.is_empty() {
        region_codes.push(region.to_string());
    }
    let state = label.get("game_state").unwrap_or(&Value::Null);
    let score = match (
        n(state, &["blue_score", "blueScore"]),
        n(state, &["orange_score", "orangeScore"]),
    ) {
        (Some(b), Some(o)) => Some((b as u32, o as u32)),
        _ => None,
    };
    Some(Match {
        id,
        mode: Mode::of(&mode_id),
        mode_id,
        lobby,
        open: label.get("open").and_then(Value::as_bool).unwrap_or(true),
        limit: n(&label, &["player_limit", "playerLimit"]).unwrap_or(0.0) as u32,
        players,
        location,
        group_id: s(&label, &["group_id", "groupId", "guild_id", "guildId"]).to_string(),
        started: unix(label.get("start_time").or_else(|| label.get("created_at"))),
        score,
        region_codes,
        group_ids: strings(server.get("group_ids")),
    })
}

/// The servers running a match, busiest first (`with_empty`: also the waiting ones,
/// which new matches are started on).
pub fn matches(token: &str, with_empty: bool) -> Result<Vec<Match>> {
    let a = api()?;
    let min = if with_empty { "&min_size=0" } else { "" };
    let body = get(
        token,
        &format!("{}/match?limit=100&authoritative=true{min}", a.base),
    )?;
    let mut list: Vec<Match> = body
        .get("matches")
        .and_then(Value::as_array)
        .map(|m| m.iter().filter_map(parse_match).collect())
        .unwrap_or_default();
    if !with_empty {
        list.retain(|m| !m.players.is_empty());
    }
    list.sort_by_key(|m| std::cmp::Reverse(m.players.len()));
    Ok(list)
}

/// Queues `match_id` as your next match: the game's terminal then takes you there (how
/// the site joins a server while the game runs).
pub fn set_next_match(token: &str, discord_id: &str, match_id: &str) -> Result<()> {
    rpc(
        token,
        "player/setnextmatch",
        json!({ "discord_id": discord_id, "match_id": match_id, "role": "", "join_immediately": false }),
    )
    .map(|_| ())
}

// ---- starting a server ----

/// What your account may do with servers: the guilds you may start matches for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Role {
    pub allocator_guilds: Vec<String>,
    /// "Global Operators" and the like.
    pub system_groups: Vec<String>,
}

impl Role {
    pub fn can_start(&self) -> bool {
        !self.allocator_guilds.is_empty() || self.operator()
    }

    pub fn operator(&self) -> bool {
        self.system_groups
            .iter()
            .any(|g| g == "Global Operators" || g == "Global Developers")
    }
}

fn ids(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| match e {
                    Value::String(s) => Some(s.clone()),
                    o => Some(s(o, &["id", "group_id"]).to_string()).filter(|s| !s.is_empty()),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_role(body: &Value) -> Role {
    Role {
        allocator_guilds: ids(body.get("allocator_guilds")),
        system_groups: ids(body.get("system_groups")),
    }
}

pub fn role(token: &str) -> Result<Role> {
    rpc(token, "get_client_role", json!({})).map(|b| parse_role(&b))
}

/// The names of guilds by id.
pub fn guild_names(token: &str, ids: &[String]) -> Result<Vec<(String, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let a = api()?;
    let list: Vec<String> = ids
        .iter()
        .map(|i| url::form_urlencoded::byte_serialize(i.as_bytes()).collect())
        .collect();
    let body = get(
        token,
        &format!("{}/rpc/guildgroup?ids={}&unwrap", a.base, list.join(",")),
    )?;
    Ok(body
        .get("guild_groups")
        .and_then(Value::as_array)
        .map(|g| {
            g.iter()
                .filter_map(|e| {
                    let g = e.get("group")?;
                    Some((s(g, &["id"]).to_string(), s(g, &["name"]).to_string()))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// A place a new match can be started: a region code, its location, and the guilds
/// that may start one there.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub code: String,
    pub location: String,
    pub groups: Vec<String>,
}

/// Where new matches can be started, from the waiting servers (skipping the service's
/// own test regions).
pub fn regions(servers: &[Match]) -> Vec<Region> {
    let mut out: Vec<Region> = Vec::new();
    for m in servers.iter().filter(|m| m.lobby == Lobby::Unassigned) {
        for code in &m.region_codes {
            let lower = code.to_ascii_lowercase();
            if ["echovrce", "cevr", "pgv"]
                .iter()
                .any(|x| lower.contains(x))
            {
                continue;
            }
            match out.iter_mut().find(|r| r.code == *code) {
                Some(r) => {
                    for g in &m.group_ids {
                        if !r.groups.contains(g) {
                            r.groups.push(g.clone());
                        }
                    }
                }
                None => out.push(Region {
                    code: code.clone(),
                    location: m.location.clone(),
                    groups: m.group_ids.clone(),
                }),
            }
        }
    }
    out.sort_by(|a, b| a.location.cmp(&b.location));
    out
}

/// Modes a new match can be started in: (service id, label).
pub const START_MODES: [(&str, &str); 4] = [
    ("echo_arena", "Arena (public)"),
    ("echo_arena_private", "Arena (private)"),
    ("echo_combat", "Combat (public)"),
    ("echo_combat_private", "Combat (private)"),
];

/// Combat maps: (service id, name).
pub const COMBAT_LEVELS: [(&str, &str); 4] = [
    ("mpl_combat_fission", "Fission"),
    ("mpl_combat_combustion", "Combustion"),
    ("mpl_combat_dyson", "Dyson"),
    ("mpl_combat_gauss", "Gauss"),
];

/// Starts a match for `guild` in `region`; returns its id.
pub fn start(
    token: &str,
    guild: &str,
    mode: &str,
    region: &str,
    level: Option<&str>,
) -> Result<String> {
    let mut body = json!({ "group_id": guild, "mode": mode, "region": region });
    if let Some(l) = level.filter(|_| mode.contains("combat")) {
        body["level"] = json!(l);
    }
    let answer = rpc(token, "match/allocate", body).map_err(|e| {
        let m = format!("{e:#}");
        if m.contains("does not host") || m.contains("allocator") || m.contains("required features")
        {
            anyhow::anyhow!("No server there can start this match right now ({m}).")
        } else {
            e
        }
    })?;
    let id = s(&answer, &["id", "match_id"]);
    if id.is_empty() {
        bail!("The service started no match.");
    }
    Ok(id.to_string())
}

// ---- you, your party and your friends ----

/// Your place in matchmaking.
#[derive(Debug, Clone, PartialEq)]
pub struct Ticket {
    pub mode: Mode,
    /// When it was made (Unix seconds).
    pub since: Option<i64>,
    /// Everyone on it, you included.
    pub players: Vec<String>,
}

/// Your tickets in `matchmaker/state`'s index (`nakama_id` is your account id).
pub fn parse_tickets(state: &Value, nakama_id: &str) -> Vec<Ticket> {
    state
        .get("index")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| {
                    let presences = e
                        .get("Presences")
                        .or_else(|| e.get("presences"))?
                        .as_array()?;
                    let mine = presences
                        .iter()
                        .any(|p| s(p, &["UserId", "user_id"]) == nakama_id);
                    if !mine {
                        return None;
                    }
                    let props = e.get("StringProperties").unwrap_or(&Value::Null);
                    let created = e
                        .get("CreatedAt")
                        .and_then(Value::as_f64)
                        .map(|ns| (ns / 1e9) as i64);
                    Some(Ticket {
                        mode: Mode::of(s(props, &["game_mode"])),
                        since: created.or_else(|| unix(e.get("CreateTime"))),
                        players: presences
                            .iter()
                            .map(|p| s(p, &["Username", "username"]).to_string())
                            .collect(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn tickets(token: &str, nakama_id: &str) -> Result<Vec<Ticket>> {
    rpc(token, "matchmaker/state", json!({})).map(|b| parse_tickets(&b, nakama_id))
}

/// The match you are in and your party there (you included), from the server list.
pub fn find_me<'a>(servers: &'a [Match], nakama_id: &str) -> Option<(&'a Match, Vec<&'a Player>)> {
    servers.iter().find_map(|m| {
        let me = m.players.iter().find(|p| p.user_id == nakama_id)?;
        let party = if me.party_id.is_empty() {
            vec![me]
        } else {
            m.players
                .iter()
                .filter(|p| p.party_id == me.party_id)
                .collect()
        };
        Some((m, party))
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Friend {
    pub user_id: String,
    pub discord_id: String,
    pub name: String,
}

pub fn parse_friends(body: &Value) -> Vec<Friend> {
    body.get("friends")
        .and_then(Value::as_array)
        .map(|f| {
            f.iter()
                .filter_map(|e| {
                    let u = e.get("user")?;
                    let meta: Value = match u.get("metadata") {
                        Some(Value::String(t)) => serde_json::from_str(t).unwrap_or(Value::Null),
                        Some(v) => v.clone(),
                        None => Value::Null,
                    };
                    Some(Friend {
                        user_id: s(u, &["id"]).to_string(),
                        discord_id: s(&meta, &["discord_id"]).to_string(),
                        name: s(u, &["display_name", "username"]).to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Your friends (accepted ones).
pub fn friends(token: &str) -> Result<Vec<Friend>> {
    let a = api()?;
    get(token, &format!("{}/friend?limit=100&state=0", a.base)).map(|b| parse_friends(&b))
}

/// The match a friend is in, from the server list.
pub fn friend_match<'a>(servers: &'a [Match], f: &Friend) -> Option<&'a Match> {
    servers.iter().find(|m| {
        m.players.iter().any(|p| {
            (!f.user_id.is_empty() && p.user_id == f.user_id)
                || (!f.discord_id.is_empty() && p.discord_id == f.discord_id)
        })
    })
}

// ---- invites ----

/// An invite to a match, from a friend.
#[derive(Debug, Clone, PartialEq)]
pub struct Invite {
    pub id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub match_id: String,
    pub created: Option<i64>,
}

pub fn parse_invites(body: &Value) -> Vec<Invite> {
    body.get("invites")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .filter_map(|i| {
                    let id = s(i, &["id", "invite_id"]);
                    let match_id = s(i, &["match_id"]);
                    (!id.is_empty() && !match_id.is_empty()).then(|| Invite {
                        id: id.to_string(),
                        sender_id: s(i, &["sender_id"]).to_string(),
                        sender_name: s(i, &["sender_username", "sender_name"]).to_string(),
                        match_id: match_id.to_string(),
                        created: unix(i.get("created_at")),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Display names by user id, from Nakama's `/user`.
pub fn parse_users(body: &Value) -> Vec<(String, String)> {
    body.get("users")
        .and_then(Value::as_array)
        .map(|u| {
            u.iter()
                .map(|u| {
                    (
                        s(u, &["id"]).to_string(),
                        s(u, &["display_name", "username"]).to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The invites waiting for you, with who sent them.
pub fn pending_invites(token: &str) -> Result<Vec<Invite>> {
    let mut invites =
        get(token, &format!("{WEB}/api/game-invites/pending")).map(|b| parse_invites(&b))?;
    let unnamed: Vec<String> = invites
        .iter()
        .filter(|i| i.sender_name.is_empty() && !i.sender_id.is_empty())
        .map(|i| i.sender_id.clone())
        .collect();
    if !unnamed.is_empty() {
        let a = api()?;
        let q: Vec<String> = unnamed
            .iter()
            .map(|id| {
                format!(
                    "ids={}",
                    url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
                )
            })
            .collect();
        if let Ok(body) = get(token, &format!("{}/user?{}", a.base, q.join("&"))) {
            let names = parse_users(&body);
            for i in &mut invites {
                if let Some((_, n)) = names.iter().find(|(id, _)| *id == i.sender_id) {
                    i.sender_name = n.clone();
                }
            }
        }
    }
    Ok(invites)
}

fn post_web(token: &str, url: &str, body: Value) -> Result<Value> {
    let (status, answer_body) = call(
        reqwest::Method::POST,
        url,
        Some(&body),
        Some(&format!("Bearer {token}")),
    )?;
    answer(status, answer_body)
}

/// Invites the user `user_id` to `match_id`.
pub fn invite(token: &str, user_id: &str, match_id: &str) -> Result<()> {
    post_web(
        token,
        &format!("{WEB}/api/game-invites"),
        json!({ "target_user_id": user_id, "match_id": match_id }),
    )
    .map(|_| ())
}

/// Accepts invite `id` (before joining its match).
pub fn accept_invite(token: &str, id: &str) -> Result<()> {
    let id: String = url::form_urlencoded::byte_serialize(id.as_bytes()).collect();
    post_web(
        token,
        &format!("{WEB}/api/game-invites/{id}/accept"),
        json!({}),
    )
    .map(|_| ())
}

// ---- match history ----

/// One of your past matches.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub id: String,
    pub mode: Mode,
    pub played: Option<i64>,
    pub duration_s: Option<u32>,
    /// Blue and orange.
    pub score: Option<(u32, u32)>,
    /// Your team, when you are in its player list.
    pub team: Option<Team>,
}

impl Summary {
    /// Won, lost, or neither (a draw, a social lobby, no score).
    pub fn won(&self) -> Option<bool> {
        let (b, o) = self.score?;
        match self.team? {
            _ if b == o => None,
            Team::Blue => Some(b > o),
            Team::Orange => Some(o > b),
            _ => None,
        }
    }
}

pub fn parse_history(body: &Value, nakama_id: &str) -> Vec<Summary> {
    let list = body
        .as_array()
        .or_else(|| body.get("items").and_then(Value::as_array))
        .or_else(|| body.get("matches").and_then(Value::as_array));
    list.map(|l| {
        l.iter()
            .map(|m| {
                let teams = m.get("teams").and_then(Value::as_array);
                let score_of = |i: usize| {
                    teams
                        .and_then(|t| t.get(i))
                        .and_then(|t| n(t, &["score"]))
                        .or_else(|| {
                            m.get("final_scores")
                                .and_then(|f| f.get(i))
                                .and_then(Value::as_f64)
                        })
                };
                let score = match (score_of(0), score_of(1)) {
                    (Some(b), Some(o)) => Some((b as u32, o as u32)),
                    _ => None,
                };
                let in_team = |i: usize| {
                    teams
                        .and_then(|t| t.get(i))
                        .and_then(|t| t.get("players"))
                        .and_then(Value::as_array)
                        .is_some_and(|ps| ps.iter().any(|p| s(p, &["user_id"]) == nakama_id))
                };
                let team = if in_team(0) {
                    Some(Team::Blue)
                } else if in_team(1) {
                    Some(Team::Orange)
                } else {
                    m.get("participants")
                        .and_then(Value::as_array)
                        .and_then(|ps| ps.iter().find(|p| s(p, &["user_id"]) == nakama_id))
                        .map(team)
                };
                let played = unix(m.get("created_at").or_else(|| m.get("start_time")));
                let ended = unix(m.get("end_time"));
                let duration = n(m, &["duration"])
                    .map(|d| d as u32)
                    .or_else(|| Some((ended? - played?).max(0) as u32));
                Summary {
                    id: s(m, &["match_id", "_id"]).to_string(),
                    mode: Mode::of(s(m, &["mode"])),
                    played,
                    duration_s: duration,
                    score,
                    team,
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Your last `limit` matches (from echovrce.com's own match history).
pub fn history(token: &str, nakama_id: &str, limit: u32) -> Result<Vec<Summary>> {
    let id: String = url::form_urlencoded::byte_serialize(nakama_id.as_bytes()).collect();
    get(
        token,
        &format!("{WEB}/api/v3/match-summaries?user_id={id}&limit={limit}"),
    )
    .map(|b| parse_history(&b, nakama_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: Value) -> Value {
        json!({ "match_id": "0f5c1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b.nakama1_us-east", "label": label.to_string() })
    }

    #[test]
    fn reads_a_match_label() {
        let m = parse_match(&entry(json!({
            "mode": "echo_arena", "lobby_type": "public", "open": true, "player_limit": 8,
            "group_id": "g1", "start_time": "2026-10-01T10:00:00Z",
            "server_city": "Dallas", "server_region": "US-TX",
            "game_state": {"blue_score": 3, "orange_score": 2},
            "players": [
                {"user_id": "u1", "display_name": "Ace", "team": 0, "party_id": "p1", "ping_ms": 31},
                {"user_id": "u2", "discord_username": "bee", "team": "orange",
                 "party_id": "00000000-0000-0000-0000-000000000000"}
            ]
        })))
        .unwrap();
        assert_eq!(m.mode, Mode::Arena);
        assert_eq!(m.lobby, Lobby::Public);
        assert_eq!(m.location, "Dallas, US-TX");
        assert_eq!(m.score, Some((3, 2)));
        assert_eq!(m.players[0].team, Team::Blue);
        assert_eq!(m.players[1].name, "bee");
        assert_eq!(m.players[1].party_id, "");
        assert_eq!(m.started, Some(1_790_848_800));
        assert!(m.joinable());
        assert_eq!(m.lobby_id(), "0F5C1A2B-3C4D-5E6F-7A8B-9C0D1E2F3A4B");
    }

    #[test]
    fn private_full_and_unassigned() {
        let private = parse_match(&entry(
            json!({"mode": "echo_arena_private", "players": [{"user_id": "a"}]}),
        ))
        .unwrap();
        assert_eq!(private.lobby, Lobby::Private);
        assert!(!private.joinable());
        let full = parse_match(&entry(
            json!({"lobby_type": 0, "player_limit": 1, "players": [{"user_id": "a"}]}),
        ))
        .unwrap();
        assert!(!full.joinable());
        let free = parse_match(&entry(json!({
            "lobby_type": "unassigned",
            "broadcaster": {"city": "Frankfurt", "default_region": "EU-DE",
                            "region_codes": ["eu-central", "echovrce-test"], "group_ids": ["g1"]}
        })))
        .unwrap();
        assert_eq!(free.lobby, Lobby::Unassigned);
        let r = regions(&[free]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].code, "eu-central");
        assert_eq!(r[0].location, "Frankfurt, EU-DE");
        assert_eq!(r[0].groups, ["g1"]);
        assert!(parse_match(&json!({"match_id": "x", "label": "not json"})).is_none());
    }

    #[test]
    fn finds_you_and_your_party() {
        let m = parse_match(&entry(json!({"players": [
            {"user_id": "me", "party_id": "p"}, {"user_id": "pal", "party_id": "p"}, {"user_id": "x"}
        ]})))
        .unwrap();
        let servers = [m];
        let (_, party) = find_me(&servers, "me").unwrap();
        assert_eq!(party.len(), 2);
        assert!(find_me(&servers, "nobody").is_none());
        let f = Friend {
            user_id: "x".into(),
            discord_id: String::new(),
            name: "X".into(),
        };
        assert!(friend_match(&servers, &f).is_some());
    }

    #[test]
    fn reads_tickets_role_and_friends() {
        let state = json!({"index": [
            {"Presences": [{"UserId": "me", "Username": "Me"}, {"UserId": "pal", "Username": "Pal"}],
             "StringProperties": {"game_mode": "echo_combat"}, "CreatedAt": 1_790_848_800_000_000_000u64},
            {"Presences": [{"UserId": "other"}], "StringProperties": {"game_mode": "echo_arena"}}
        ]});
        let t = parse_tickets(&state, "me");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].mode, Mode::Combat);
        assert_eq!(t[0].since, Some(1_790_848_800));
        assert_eq!(t[0].players, ["Me", "Pal"]);
        let role = parse_role(
            &json!({"allocator_guilds": [{"id": "g1"}, "g2"], "system_groups": ["Global Developers"]}),
        );
        assert_eq!(role.allocator_guilds, ["g1", "g2"]);
        assert!(role.operator() && role.can_start());
        assert!(!parse_role(&json!({})).can_start());
        let f = parse_friends(&json!({"friends": [
            {"user": {"id": "u1", "display_name": "One", "metadata": "{\"discord_id\":\"42\"}"}, "state": 0}
        ]}));
        assert_eq!(f[0].discord_id, "42");
        assert_eq!(f[0].name, "One");
    }

    #[test]
    fn share_links() {
        assert_eq!(
            share_link("0f5c1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b.nakama1_us-east"),
            "https://echo.taxi/spark://c/0F5C1A2B-3C4D-5E6F-7A8B-9C0D1E2F3A4B"
        );
        // And a launcher reading it gets the same lobby back.
        let link = crate::core::links::parse(&share_link("0f5c1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b.n"))
            .unwrap();
        assert_eq!(link.lobby, "0F5C1A2B-3C4D-5E6F-7A8B-9C0D1E2F3A4B");
    }

    #[test]
    fn reads_invites() {
        let i = parse_invites(&json!({"invites": [
            {"id": "i1", "sender_id": "u1", "match_id": "m.n", "created_at": "2026-10-01T10:00:00Z"},
            {"id": "i2", "sender_id": "u2", "sender_username": "Ace", "match_id": "m2.n"},
            {"id": "", "match_id": "x"}
        ]}));
        assert_eq!(i.len(), 2);
        assert_eq!(i[0].created, Some(1_790_848_800));
        assert_eq!(i[1].sender_name, "Ace");
        let u = parse_users(
            &json!({"users": [{"id": "u1", "username": "one"}, {"id": "u2", "display_name": "Two", "username": "two"}]}),
        );
        assert_eq!(
            u,
            [
                ("u1".to_string(), "one".to_string()),
                ("u2".to_string(), "Two".to_string())
            ]
        );
    }

    #[test]
    fn reads_history() {
        let h = parse_history(
            &json!({"items": [
                {"match_id": "m1", "mode": "echo_arena", "created_at": "2026-10-01T10:00:00Z", "duration": 600,
                 "teams": [{"name": "Blue", "score": 5, "players": [{"user_id": "me"}]},
                           {"name": "Orange", "score": 3, "players": []}]},
                {"_id": "m2", "mode": "echo_combat", "final_scores": [1, 2],
                 "participants": [{"user_id": "me", "team": 0}]}
            ]}),
            "me",
        );
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].won(), Some(true));
        assert_eq!(h[0].duration_s, Some(600));
        assert_eq!(h[1].id, "m2");
        assert_eq!(h[1].won(), Some(false));
    }
}
