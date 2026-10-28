//! The account the servers sign in as. Either a session linked for them (a device link:
//! from the launcher's own EchoVRCE sign-in, or a code approved on echovrce.com for
//! another account), from which each server gets a session of its own (nEVR renews, and
//! so rotates, its own refresh token: sharing one would sign the others out); or the
//! account's Discord ID and password, which reach nEVR in the environment only.
//!
//! A device link works only for an account without a password (EchoVRCE refuses the
//! server's login otherwise; `/reset-password` in the Echo VR Lounge Discord clears it).

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::echovrce::{self, Tokens};
use crate::core::launcher::nevr::{self, GameLogin};

use super::{Account, Config, Services, SignIn};

/// What the servers' account keeps secret: the linked session or the password.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Secrets {
    pub session: Option<Tokens>,
    pub password: String,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("session", &self.session)
            .field("password", &(!self.password.is_empty()).then_some("…"))
            .finish()
    }
}

pub mod secrets {
    //! Kept like the launcher's own session: the Windows Credential Manager, else a file
    //! in the data folder only the user can read.
    use super::Secrets;
    use anyhow::Result;

    #[cfg(windows)]
    const SERVICE: &str = "EchoVR Launcher";
    #[cfg(windows)]
    const ACCOUNT: &str = "server-host";

    fn file() -> std::path::PathBuf {
        super::super::dir().join("secrets.json")
    }

    #[cfg(windows)]
    fn entry() -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, ACCOUNT).ok()
    }

    pub fn load() -> Secrets {
        #[cfg(windows)]
        if let Some(json) = entry().and_then(|e| e.get_password().ok()) {
            return serde_json::from_str(&json).unwrap_or_default();
        }
        std::fs::read_to_string(file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(s: &Secrets) -> Result<()> {
        let json = serde_json::to_string(s)?;
        #[cfg(windows)]
        if let Some(e) = entry() {
            if e.set_password(&json).is_ok() {
                let _ = std::fs::remove_file(file());
                return Ok(());
            }
        }
        let f = file();
        if let Some(d) = f.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::core::launcher::nevr::write_private(&f, json.as_bytes())
    }
}

/// Pure: the account a session belongs to, from its claims (`uid`, `usn`, `vrs.did`).
pub fn account_of(token: &str) -> Account {
    let c = echovrce::jwt_claims(token);
    let s = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or_default().to_string();
    Account {
        sign_in: SignIn::Device,
        discord_id: s(c.pointer("/vrs/did")),
        user_id: s(c.get("uid")),
        username: s(c.get("usn")),
    }
}

/// Pure: the guilds a session may host for (ids): `vrs.memberships` (a JSON object, or
/// one in a string) maps a guild to its role bits; 8 is the server host role, 32 a
/// suspension. A device link's session carries none.
pub fn host_guilds(token: &str) -> Vec<String> {
    let c = echovrce::jwt_claims(token);
    let m = match c.pointer("/vrs/memberships") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    };
    let mut out: Vec<String> = m
        .as_object()
        .map(|o| {
            o.iter()
                .filter(|(_, bits)| {
                    let b = bits.as_u64().unwrap_or(0);
                    b & 8 != 0 && b & 32 == 0
                })
                .map(|(g, _)| g.clone())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Links the servers to the account the launcher is signed in with (EchoVRCE page).
pub fn link_launchers() -> Result<(Account, Tokens)> {
    let Some(mine) = echovrce::store::load() else {
        bail!("Sign in on the EchoVRCE page first.");
    };
    let mine = echovrce::ensure(&mine)?.unwrap_or(mine);
    let linked = echovrce::link_device(&mine.token)?;
    Ok((finish(&linked)?, linked))
}

/// The account of a session linked by a code (or from the launcher's): its Discord ID
/// from the session, else from the account.
fn finish(t: &Tokens) -> Result<Account> {
    let mut a = account_of(&t.token);
    if a.discord_id.is_empty() || a.user_id.is_empty() {
        let acc = echovrce::account(&t.token)?;
        a.discord_id = acc.discord_id;
        a.user_id = acc.id;
        a.username = acc.username;
    }
    if a.discord_id.is_empty() {
        bail!("EchoVRCE didn't say which Discord account this is.");
    }
    Ok(a)
}

/// A code approved for the servers (another account): its account.
pub fn from_code(t: &Tokens) -> Result<Account> {
    finish(t)
}

/// Keeps a device link for the servers in `c` (and its session as a secret).
pub fn use_device(c: &mut Config, a: Account, t: Tokens) -> Result<()> {
    secrets::save(&Secrets {
        session: Some(t),
        password: String::new(),
    })?;
    c.account = Account {
        sign_in: SignIn::Device,
        ..a
    };
    // The servers' own sessions came from another link: they are made anew.
    for s in &c.servers {
        let _ = std::fs::remove_file(super::farm_local(&s.dir()).join(nevr::CREDENTIALS));
    }
    Ok(())
}

/// One RPC on the servers' Nakama (`services.http_uri`): with its HTTP key, or as the
/// session `bearer`. (status, answer).
fn rpc(s: &Services, name: &str, body: &Value, bearer: Option<&str>) -> Result<(u16, Value)> {
    use crate::core::http;
    let base = s.http_uri.trim().trim_end_matches('/');
    let mut url = format!("{base}/v2/rpc/{name}?unwrap");
    if bearer.is_none() {
        let key: String =
            url::form_urlencoded::byte_serialize(s.http_key.trim().as_bytes()).collect();
        url.push_str(&format!("&http_key={key}"));
    }
    http::block_on(async {
        let mut req = http::client()
            .post(&url)
            .json(body)
            .timeout(std::time::Duration::from_secs(20));
        if let Some(t) = bearer {
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("{base} couldn't be reached"))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    })
}

/// Pure: an error answer's message.
fn message(body: &Value) -> String {
    ["message", "error"]
        .iter()
        .find_map(|k| body.get(k).and_then(Value::as_str))
        .unwrap_or("no details")
        .chars()
        .take(160)
        .collect()
}

/// Signs in with the account's Discord ID and password on the servers' Nakama.
fn password_session(s: &Services, discord_id: &str, password: &str) -> Result<Tokens> {
    let (status, body) = rpc(
        s,
        "account/authenticate/password",
        &json!({ "discord_id": discord_id.trim(), "password": password }),
        None,
    )?;
    if status == 200 {
        let get = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_string);
        if let Some(token) = get("token").or_else(|| get("access_token")) {
            return Ok(Tokens {
                token,
                refresh_token: get("refresh_token"),
            });
        }
    }
    let m = message(&body);
    if status == 401 || m.to_ascii_lowercase().contains("unauthenticated") {
        bail!("EchoVRCE didn't take that Discord ID and password.");
    }
    bail!("The sign-in was refused ({status}: {m}).")
}

/// Checks the Discord ID and password with the servers' Nakama (a network call): the
/// session.
pub fn check_password(s: &Services, discord_id: &str, password: &str) -> Result<Tokens> {
    let id = discord_id.trim();
    if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_digit()) {
        bail!("A Discord ID is a number (Discord: Settings, Advanced, Developer Mode on; then right-click your name, Copy User ID).");
    }
    if password.is_empty() {
        bail!("Fill in the password.");
    }
    password_session(s, id, password)
}

/// Keeps a checked Discord ID and password (`t`, their session) for the servers in `c`.
/// Returns the guilds the account may host for.
pub fn keep_password(
    c: &mut Config,
    discord_id: &str,
    password: &str,
    t: &Tokens,
) -> Result<Vec<String>> {
    secrets::save(&Secrets {
        session: None,
        password: password.to_string(),
    })?;
    let a = account_of(&t.token);
    c.account = Account {
        sign_in: SignIn::Password,
        discord_id: discord_id.trim().to_string(),
        user_id: a.user_id,
        username: a.username,
    };
    for s in &c.servers {
        let _ = std::fs::remove_file(super::farm_local(&s.dir()).join(nevr::CREDENTIALS));
    }
    Ok(host_guilds(&t.token))
}

/// Signs the servers out: the account and its secrets go.
pub fn forget(c: &mut Config) -> Result<()> {
    secrets::save(&Secrets::default())?;
    c.account = Account::default();
    for s in &c.servers {
        let _ = std::fs::remove_file(super::farm_local(&s.dir()).join(nevr::CREDENTIALS));
    }
    Ok(())
}

/// A session of the servers' account, now (renewed when due; a renewed linked session is
/// kept).
pub fn session(c: &Config) -> Result<Tokens> {
    let mut s = secrets::load();
    match c.account.sign_in {
        SignIn::None => bail!("The servers have no account yet: Account, then sign in."),
        SignIn::Password => {
            if s.password.is_empty() {
                bail!("The servers' password is gone: sign in again under Account.");
            }
            password_session(&c.services, &c.account.discord_id, &s.password)
        }
        SignIn::Device => {
            let Some(t) = s.session.clone() else {
                bail!("The servers' link is gone: link them again under Account.");
            };
            match echovrce::ensure(&t)? {
                Some(renewed) => {
                    s.session = Some(renewed.clone());
                    secrets::save(&s)?;
                    Ok(renewed)
                }
                None => Ok(t),
            }
        }
    }
}

/// Before a start: the server in `local` (its `_local`) has its own sign-in (a device
/// link), made from the servers' session unless it has a good one. A password sign-in
/// needs none. `root`: the server's nEVR root, where nEVR 5.13 on keeps the sign-in (and
/// takes `_local`'s once): it gets the same one.
pub fn ensure_login(c: &Config, local: &Path, root: Option<&Path>) -> Result<()> {
    if c.account.sign_in != SignIn::Device {
        return Ok(());
    }
    let now = echovrce::now();
    let read = |dir: &Path| -> Option<GameLogin> {
        std::fs::read_to_string(dir.join(nevr::CREDENTIALS))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
    };
    let good = root
        .into_iter()
        .chain(std::iter::once(local))
        .filter_map(read)
        .find(|l| l.good_for(&c.account.user_id, now));
    if let Some(login) = good {
        // In both places, as nEVR may have moved it.
        for dir in root.into_iter().chain(std::iter::once(local)) {
            if read(dir).as_ref() != Some(&login) {
                nevr::write_login_at(dir, &login)?;
            }
        }
        return Ok(());
    }
    let master = session(c)?;
    let own = echovrce::link_device(&master.token).context("a session for the server")?;
    let Some(refresh) = own.refresh_token.as_deref() else {
        bail!("EchoVRCE gave the server a session it can't renew.");
    };
    let login = GameLogin::new(refresh, &c.account.user_id, &c.account.username, now);
    for dir in root.into_iter().chain(std::iter::once(local)) {
        nevr::write_login_at(dir, &login)?;
    }
    Ok(())
}

/// A server of this account as EchoVRCE has it registered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Registered {
    pub id: String,
    pub mode: String,
    pub players: u64,
    pub open: bool,
    /// Where EchoVRCE thinks it is (its geolocation, not the regions asked for).
    pub region: String,
    pub country: String,
    pub guilds: Vec<String>,
}

/// Pure: the servers in an answer to the `match` RPC.
pub fn parse_registered(body: &Value) -> Vec<Registered> {
    let labels = body
        .get("labels")
        .or_else(|| body.get("matches"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    labels
        .iter()
        .map(|l| {
            // A label may come as a JSON string.
            let l = match l.get("label") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                Some(v @ Value::Object(_)) => v.clone(),
                _ => l.clone(),
            };
            let s = |p: &str| {
                l.pointer(p)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Registered {
                id: s("/id"),
                mode: s("/mode"),
                players: l.get("player_count").and_then(Value::as_u64).unwrap_or(0),
                open: l.get("open").and_then(Value::as_bool).unwrap_or(false),
                region: s("/broadcaster/default_region"),
                country: s("/broadcaster/country_code"),
                guilds: l
                    .pointer("/broadcaster/group_ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|g| g.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// The servers EchoVRCE has registered for this account (any PC's).
pub fn registered(c: &Config) -> Result<Vec<Registered>> {
    let t = session(c)?;
    let uid = match account_of(&t.token).user_id {
        u if !u.is_empty() => u,
        _ => c.account.user_id.clone(),
    };
    let (status, body) = rpc(
        &c.services,
        "match",
        &json!({ "query": format!("+label.broadcaster.oper:{uid}") }),
        Some(&t.token),
    )?;
    if status != 200 {
        bail!("EchoVRCE didn't list the servers ({status}).");
    }
    Ok(parse_registered(&body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn jwt(claims: Value) -> String {
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("e30.{p}.sig")
    }

    #[test]
    fn reads_the_account_and_its_host_guilds() {
        let t = jwt(json!({
            "uid": "u-1", "usn": "srv",
            "vrs": {"did": "123", "memberships": "{\"g1\": 8, \"g2\": 40, \"g3\": 9, \"g4\": 1}"}
        }));
        let a = account_of(&t);
        assert_eq!(
            (
                a.user_id.as_str(),
                a.username.as_str(),
                a.discord_id.as_str()
            ),
            ("u-1", "srv", "123")
        );
        assert_eq!(host_guilds(&t), vec!["g1", "g3"]);
        assert!(host_guilds(&jwt(json!({"uid": "x"}))).is_empty());
    }

    #[test]
    fn lists_registered_servers() {
        let body = json!({"labels": [
            {"id": "m1", "open": true, "mode": "echo_arena", "player_count": 3,
             "broadcaster": {"default_region": "de-saxony", "country_code": "DE", "group_ids": ["g1"]}},
            {"label": "{\"id\": \"m2\", \"mode\": \"social\"}"}
        ]});
        let r = parse_registered(&body);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].players, 3);
        assert_eq!(r[0].guilds, vec!["g1"]);
        assert_eq!(r[1].id, "m2");
    }
}
