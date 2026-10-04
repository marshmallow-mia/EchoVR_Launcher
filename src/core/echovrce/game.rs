//! The game service's live side, as echovrce.com's own pages use it: the servers running
//! (and who is on them), joining one, starting a new one, your party and matchmaking
//! queue, your friends (and requests) and their matches, finding players, and your match
//! history. Every call is made with the signed-in session's token.

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

/// A call without a body (POST, DELETE) on the service's API.
fn send(token: &str, method: reqwest::Method, url: &str) -> Result<Value> {
    let (status, body) = call(method, url, None, Some(&format!("Bearer {token}")))?;
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
    /// How many players are in it, as the service counts them (its list of them isn't
    /// always given).
    pub size: u32,
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
            && self.size > 0
            && (self.limit == 0 || self.size < self.limit)
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
        size: (players.len() as u32)
            .max(n(&label, &["player_count", "size"]).unwrap_or(0.0) as u32),
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
/// The service's public status (`/status/matches`): every running match with its
/// players, scores and server, and every game server. The match list behind the session
/// (`/v2/match`) names no players any more.
fn status() -> Result<Value> {
    let (code, body) = call(reqwest::Method::GET, STATUS_URL, None, None)?;
    answer(code, body)
}

/// The public status, on the service's web port (its API is on 7350); the feed's status
/// service reads it too.
const STATUS_URL: &str = "https://g.echovrce.com/status/matches";

/// Pure: the matches of the public status, as `parse_match` reads them.
pub fn parse_status_matches(status: &Value) -> Vec<Match> {
    status
        .get("labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|l| {
            let id = l.get("id").and_then(Value::as_str)?;
            parse_match(&json!({ "match_id": id, "label": l }))
        })
        .collect()
}

/// Pure: `with` added to `list` where it has no match of that id yet.
fn merge_matches(mut list: Vec<Match>, with: Vec<Match>) -> Vec<Match> {
    for m in with {
        if !list.iter().any(|x| x.id == m.id) {
            list.push(m);
        }
    }
    list
}

/// The running matches, the fullest first: the public status's (with their players),
/// and the session's list for any it hasn't caught yet. `with_empty`: the empty ones too.
pub fn matches(token: &str, with_empty: bool) -> Result<Vec<Match>> {
    let a = api()?;
    let min = if with_empty { "&min_size=0" } else { "" };
    let listed = get(
        token,
        &format!("{}/match?limit=100&authoritative=true{min}", a.base),
    )
    .map(|body| {
        body.get("matches")
            .and_then(Value::as_array)
            .map(|m| m.iter().filter_map(parse_match).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    let public = status()
        .map(|s| parse_status_matches(&s))
        .inspect_err(|e| tracing::warn!("server status: {e:#}"));
    let mut list = match (public, listed) {
        (Ok(p), Ok(l)) => merge_matches(p, l),
        (Ok(p), Err(_)) => p,
        (Err(_), l) => l?,
    };
    if !with_empty {
        list.retain(|m| m.size > 0);
    }
    list.sort_by_key(|m| std::cmp::Reverse(m.size));
    Ok(list)
}

/// Pure: where new matches can be started, from the public status's game servers: each
/// default region once, with the guilds whose servers are there.
pub fn parse_status_regions(status: &Value) -> Vec<Region> {
    let mut out: Vec<Region> = Vec::new();
    for g in status
        .get("gameservers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let code = s(g, &["default_region"]);
        let lower = code.to_ascii_lowercase();
        if code.is_empty()
            || ["echovrce", "cevr", "pgv"]
                .iter()
                .any(|x| lower.contains(x))
        {
            continue;
        }
        let groups: Vec<String> = g
            .get("group_ids")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        match out.iter_mut().find(|r| r.code == code) {
            Some(r) => {
                for id in groups {
                    if !r.groups.contains(&id) {
                        r.groups.push(id);
                    }
                }
            }
            None => {
                let location = match (s(g, &["region"]), s(g, &["country_code"])) {
                    ("", "") => code.to_string(),
                    (r, "") | ("", r) => r.to_string(),
                    (r, c) => format!("{r}, {c}"),
                };
                out.push(Region {
                    code: code.to_string(),
                    location,
                    groups,
                });
            }
        }
    }
    out.sort_by(|a, b| a.location.cmp(&b.location));
    out
}

/// Where new matches can be started: the public status's game servers, or (without it)
/// the waiting servers of the session's list.
pub fn start_regions(token: &str) -> Result<Vec<Region>> {
    if let Ok(r) = status().map(|s| parse_status_regions(&s)) {
        if !r.is_empty() {
            return Ok(r);
        }
    }
    matches(token, true).map(|all| regions(&all))
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

/// Where a friendship stands (the service's friend states 0-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FriendState {
    #[default]
    Friend,
    /// You asked them; they haven't answered.
    Sent,
    /// They asked you.
    Received,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Friend {
    pub user_id: String,
    pub discord_id: String,
    pub name: String,
    pub state: FriendState,
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
                    let state = match e.get("state").and_then(Value::as_i64).unwrap_or(0) {
                        0 => FriendState::Friend,
                        1 => FriendState::Sent,
                        2 => FriendState::Received,
                        _ => FriendState::Blocked,
                    };
                    Some(Friend {
                        user_id: s(u, &["id"]).to_string(),
                        discord_id: s(&meta, &["discord_id"]).to_string(),
                        name: s(u, &["display_name", "username"]).to_string(),
                        state,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Your friends, and the requests you sent and got (not the players you blocked).
pub fn friends(token: &str) -> Result<Vec<Friend>> {
    let a = api()?;
    get(token, &format!("{}/friend?limit=500", a.base)).map(|b| {
        let mut f = parse_friends(&b);
        f.retain(|f| f.state != FriendState::Blocked && !f.user_id.is_empty());
        f
    })
}

/// Asks player `user_id` to be your friend, or accepts their request.
pub fn add_friend(token: &str, user_id: &str) -> Result<()> {
    let a = api()?;
    let id: String = url::form_urlencoded::byte_serialize(user_id.as_bytes()).collect();
    send(
        token,
        reqwest::Method::POST,
        &format!("{}/friend?ids={id}", a.base),
    )
    .map(|_| ())
}

/// Removes friend `user_id`, or turns down or takes back a request.
pub fn remove_friend(token: &str, user_id: &str) -> Result<()> {
    let a = api()?;
    let id: String = url::form_urlencoded::byte_serialize(user_id.as_bytes()).collect();
    send(
        token,
        reqwest::Method::DELETE,
        &format!("{}/friend?ids={id}", a.base),
    )
    .map(|_| ())
}

// ---- finding players ----

/// A player found by name.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub user_id: String,
    /// The name it matched (a display name, which can differ per guild).
    pub name: String,
    pub username: String,
}

/// Pure: what the search sends for `text`, or `None` when it's too short. The service
/// matches it as a pattern against lowercased display names, so it is lowercased and
/// anything but letters, digits, spaces, `-` and `_` is escaped.
pub fn search_pattern(text: &str) -> Option<String> {
    let t = text.trim().to_lowercase();
    if t.chars().count() < 2 {
        return None;
    }
    let mut out = String::new();
    for c in t.chars().take(32) {
        if !(c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')) {
            out.push('\\');
        }
        out.push(c);
    }
    Some(out)
}

/// Pure: the search's answer, one entry per player (it lists one per guild name).
pub fn parse_found(body: &Value) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for m in body
        .get("display_name_matches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = s(m, &["user_id"]);
        if id.is_empty() || out.iter().any(|f| f.user_id == id) {
            continue;
        }
        out.push(Found {
            user_id: id.to_string(),
            name: s(m, &["display_name", "username"]).to_string(),
            username: s(m, &["username"]).to_string(),
        });
    }
    out
}

/// Players whose display name matches `text` (see `search_pattern`).
pub fn search(token: &str, text: &str) -> Result<Vec<Found>> {
    let Some(pattern) = search_pattern(text) else {
        return Ok(Vec::new());
    };
    rpc(token, "account/search", json!({ "display_name": pattern })).map(|b| parse_found(&b))
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
    /// Who played on blue or orange (user id, name), you too: not the spectators, and no
    /// one from a social lobby (its list is everyone who passed through).
    pub players: Vec<(String, String)>,
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
                let mut players: Vec<(String, String)> = Vec::new();
                // teams[0] and [1] are blue's and orange's players; participants are
                // everyone, with their team.
                let on_teams = teams
                    .into_iter()
                    .flatten()
                    .take(2)
                    .filter_map(|t| t.get("players").and_then(Value::as_array))
                    .flatten();
                let participants = m
                    .get("participants")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|p| matches!(self::team(p), Team::Blue | Team::Orange));
                for p in on_teams.chain(participants) {
                    let id = s(p, &["user_id"]);
                    if !id.is_empty() && !players.iter().any(|(i, _)| i == id) {
                        let name = s(p, &["display_name", "username"]).to_string();
                        players.push((id.to_string(), name));
                    }
                }
                Summary {
                    id: s(m, &["match_id", "_id"]).to_string(),
                    mode: Mode::of(s(m, &["mode"])),
                    played,
                    duration_s: duration,
                    score,
                    team,
                    players,
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Someone you played with lately.
#[derive(Debug, Clone, PartialEq)]
pub struct Recent {
    pub user_id: String,
    pub name: String,
    /// Your last match together.
    pub mode: Mode,
    pub played: Option<i64>,
}

/// Pure: the players of `history`'s Arena and Combat matches but you (`me`), the latest
/// match together first, each once.
pub fn recent_players(history: &[Summary], me: &str) -> Vec<Recent> {
    let mut matches: Vec<&Summary> = history
        .iter()
        .filter(|m| matches!(m.mode, Mode::Arena | Mode::Combat))
        .collect();
    matches.sort_by_key(|m| std::cmp::Reverse(m.played));
    let mut out: Vec<Recent> = Vec::new();
    for m in matches {
        for (id, name) in &m.players {
            if id == me || name.is_empty() || out.iter().any(|r| &r.user_id == id) {
                continue;
            }
            out.push(Recent {
                user_id: id.clone(),
                name: name.clone(),
                mode: m.mode,
                played: m.played,
            });
        }
    }
    out
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
    fn reads_the_public_status() {
        let status = json!({
            "labels": [
                {"id": "m1.n", "lobby_type": "public", "mode": "echo_arena", "player_count": 2, "player_limit": 8,
                 "players": [{"user_id": "a", "team": 0}, {"user_id": "b", "team": 1}],
                 "game_state": {"blue_score": 2, "orange_score": 1}},
                {"lobby_type": "public"}
            ],
            "gameservers": [
                {"default_region": "gb-england", "region": "England", "country_code": "GB", "group_ids": ["g1"]},
                {"default_region": "gb-england", "region": "England", "country_code": "GB", "group_ids": ["g2", "g1"]},
                {"default_region": "echovrce-test", "group_ids": ["g3"]},
                {"default_region": "us-nebraska", "region": "", "country_code": "US", "group_ids": []}
            ]
        });
        let m = parse_status_matches(&status);
        assert_eq!(m.len(), 1);
        assert_eq!(
            (m[0].id.as_str(), m[0].size, m[0].score),
            ("m1.n", 2, Some((2, 1)))
        );
        // The session's list: a count, no players.
        let listed = parse_match(&entry(
            json!({"player_count": 3, "lobby_type": "public", "open": true}),
        ))
        .unwrap();
        assert_eq!((listed.size, listed.players.len()), (3, 0));
        assert!(listed.joinable());
        let merged = merge_matches(m.clone(), vec![m[0].clone(), listed]);
        assert_eq!(merged.len(), 2);
        let r = parse_status_regions(&status);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].location, "England, GB");
        assert_eq!(r[0].groups, ["g1", "g2"]);
        assert_eq!(
            (r[1].code.as_str(), r[1].location.as_str()),
            ("us-nebraska", "US")
        );
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
            name: "X".into(),
            ..Default::default()
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
    fn friend_states_and_search() {
        let f = parse_friends(&json!({"friends": [
            {"user": {"id": "a", "display_name": "A"}, "state": 0},
            {"user": {"id": "b", "display_name": "B"}, "state": 1},
            {"user": {"id": "c", "display_name": "C"}, "state": 2},
            {"user": {"id": "d", "display_name": "D"}, "state": 3}
        ]}));
        let states: Vec<FriendState> = f.iter().map(|f| f.state).collect();
        assert_eq!(
            states,
            [
                FriendState::Friend,
                FriendState::Sent,
                FriendState::Received,
                FriendState::Blocked
            ]
        );
        assert_eq!(search_pattern(" Mia "), Some("mia".into()));
        assert_eq!(search_pattern("m"), None);
        assert_eq!(search_pattern(".*"), Some(r"\.\*".into()));
        assert_eq!(search_pattern("mia - (500)"), Some(r"mia - \(500\)".into()));
        let found = parse_found(&json!({"display_name_matches": [
            {"display_name": "Mars", "username": "mars1", "user_id": "u1", "group_id": "g1"},
            {"display_name": "mars", "username": "mars1", "user_id": "u1", "group_id": "g2"},
            {"display_name": "Marsh", "username": "m2", "user_id": "u2"},
            {"display_name": "nobody"}
        ]}));
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].name.as_str(), found[1].user_id.as_str()),
            ("Mars", "u2")
        );
        assert!(parse_found(&json!({})).is_empty());
    }

    #[test]
    fn players_you_played_with() {
        let h = parse_history(
            &json!([
                {"_id": "old", "mode": "echo_arena", "start_time": "2026-10-01T10:00:00Z",
                 "participants": [{"user_id": "me", "display_name": "Me", "team": 0}, {"user_id": "p1", "display_name": "One", "team": 1},
                                  {"user_id": "p2", "display_name": "Two", "team": 0},
                                  {"user_id": "watcher", "display_name": "Watcher", "team": 2}]},
                {"_id": "lobby", "mode": "social_2.0", "start_time": "2026-10-04T10:00:00Z",
                 "participants": [{"user_id": "me", "team": 3}, {"user_id": "stranger", "display_name": "Stranger", "team": 3}]},
                {"_id": "new", "mode": "echo_combat", "start_time": "2026-10-03T10:00:00Z",
                 "teams": [{"players": [{"user_id": "me"}, {"user_id": "p2", "display_name": "Two"}]},
                           {"players": [{"user_id": "p3", "username": "three"}]}]}
            ]),
            "me",
        );
        assert_eq!(h[0].players.len(), 3, "no spectators");
        assert!(h[1].players.is_empty(), "no one from a social lobby");
        assert_eq!(h[2].players.len(), 3);
        let r = recent_players(&h, "me");
        let who: Vec<(&str, Mode)> = r.iter().map(|r| (r.name.as_str(), r.mode)).collect();
        assert_eq!(
            who,
            [
                ("Two", Mode::Combat),
                ("three", Mode::Combat),
                ("One", Mode::Arena)
            ]
        );
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
