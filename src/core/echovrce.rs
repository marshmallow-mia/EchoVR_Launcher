//! EchoVRCE (echovrce.com), the community's game service: signing the launcher in as a
//! device of your account, keeping that session renewed, and reading the account. The
//! calls are the feed bot's, for a person at the launcher.
//!
//! * Sign in: the launcher asks for a device code, you approve it on
//!   `echovrce.com/login/device` (opened with the code filled in), and the launcher picks
//!   up the session: a short-lived token and a refresh token. Tokens can also be pasted.
//! * The token is renewed before it runs out; the refresh token rotates every time.
//! * The keys the calls need are the ones echovrce.com's own web app publishes in its
//!   `config.json`; they are read from there, never stored.
//! * The session is kept in the Windows Credential Manager (on macOS and Linux in a file
//!   only the user can read) and never logged.
//! * Sessions run out by EchoVRCE's clock, not the computer's ([`now`]): a computer clock
//!   that is hours off would make every session look ended.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::http;

pub mod game;

/// The website.
pub const WEB: &str = "https://echovrce.com";
const CONFIG_URL: &str = "https://echovrce.com/config.json";
const DEFAULT_API: &str = "https://g.echovrce.com/v2";
/// Renew the session when it has less than this left.
pub const MARGIN: Duration = Duration::from_secs(300);
/// How long one call may take.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The session ended and couldn't be renewed: sign in again.
#[derive(Debug, thiserror::Error)]
#[error("Your EchoVRCE session has ended. Sign in again.")]
pub struct SignedOut;

/// Whether `e` means signing in again is the only way on.
pub fn is_signed_out(e: &anyhow::Error) -> bool {
    e.downcast_ref::<SignedOut>().is_some()
}

/// EchoVRCE isn't answering: it couldn't be reached, or it answered with a server error
/// (5xx) or "too many requests" (429). Nothing is wrong with the session or the code:
/// trying again later can work.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Unavailable(pub String);

/// Whether `e` means EchoVRCE isn't answering right now ([`Unavailable`]).
pub fn is_unavailable(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Unavailable>().is_some()
}

/// Pure: whether an answer's `status` means EchoVRCE isn't answering right now.
pub fn unavailable_status(status: u16) -> bool {
    status >= 500 || status == 429
}

fn unavailable(status: u16, body: &Value) -> anyhow::Error {
    let detail = match message(body) {
        m if m == "no details" => String::new(),
        m => format!(": {m}"),
    };
    Unavailable(format!(
        "EchoVRCE isn't answering right now ({status}{detail})"
    ))
    .into()
}

/// A session: the token calls use, and the refresh token that renews it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print a token.
        f.debug_struct("Tokens")
            .field("expires", &self.expires())
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "…"))
            .finish()
    }
}

impl Tokens {
    /// When the token runs out (Unix seconds), from its JWT payload.
    pub fn expires(&self) -> Option<i64> {
        jwt_exp(&self.token)
    }

    /// Whether the token is good for at least `MARGIN` more.
    pub fn fresh(&self, now: i64) -> bool {
        self.expires()
            .is_some_and(|exp| exp - now > MARGIN.as_secs() as i64)
    }
}

/// A JWT's expiry (Unix seconds).
pub fn jwt_exp(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("exp")?
        .as_i64()
}

// ---- EchoVRCE's clock ----

/// How far EchoVRCE's clock is ahead of this computer's (seconds; negative: behind), from
/// the `Date` of its last answer; [`NO_OFFSET`] until it has answered.
static CLOCK_OFFSET: AtomicI64 = AtomicI64::new(NO_OFFSET);
const NO_OFFSET: i64 = i64::MIN;
/// A clock off by less than this counts as right.
const CLOCK_SLACK: i64 = 120;

/// Now by EchoVRCE's clock (Unix seconds), which its sessions run out by. This computer's
/// clock until EchoVRCE has answered once.
pub fn now() -> i64 {
    local_now() + clock_offset().unwrap_or(0)
}

/// How far EchoVRCE's clock is ahead of this computer's, once it has answered.
pub fn clock_offset() -> Option<i64> {
    Some(CLOCK_OFFSET.load(Ordering::Relaxed)).filter(|&o| o != NO_OFFSET)
}

/// This computer's clock off by `offset` (as [`clock_offset`]) in words, "4 h 0 min
/// ahead"; `None` when it is right.
pub fn clock_off(offset: i64) -> Option<String> {
    if offset.abs() < CLOCK_SLACK {
        return None;
    }
    let way = if offset < 0 { "ahead" } else { "behind" };
    let min = (offset.abs() + 30) / 60;
    Some(match min {
        m if m >= 60 => format!("{} h {} min {way}", m / 60, m % 60),
        m => format!("{m} min {way}"),
    })
}

fn local_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Takes EchoVRCE's clock from an answer's `Date`; says so in the log when this computer's
/// is off (and again when that changes).
fn note_clock(date: Option<&reqwest::header::HeaderValue>) {
    let Some(server) = date.and_then(|d| d.to_str().ok()).and_then(http_date) else {
        return;
    };
    let offset = server - local_now();
    let before = CLOCK_OFFSET.swap(offset, Ordering::Relaxed);
    let was = if before == NO_OFFSET { 0 } else { before };
    if (offset - was).abs() < CLOCK_SLACK {
        return;
    }
    match clock_off(offset) {
        Some(off) => tracing::warn!(
            "this computer's clock is {off} of EchoVRCE's: sessions are timed by EchoVRCE's"
        ),
        None => tracing::info!("this computer's clock agrees with EchoVRCE's again"),
    }
}

/// Pure: an HTTP `Date` ("Sat, 10 Oct 2026 16:41:29 GMT") in Unix seconds.
fn http_date(s: &str) -> Option<i64> {
    let format = time::format_description::parse_borrowed::<1>(
        "[weekday repr:short], [day] [month repr:short] [year] [hour]:[minute]:[second] GMT",
    )
    .ok()?;
    time::PrimitiveDateTime::parse(s.trim(), &format)
        .ok()
        .map(|t| t.assume_utc().unix_timestamp())
}

// ---- tokens entered by hand ----

/// Reads pasted credentials: a JSON object or `key=value` lines with a token and/or a
/// refresh token (common spellings), an `authorization: Bearer …` header, or one bare
/// token (a JWT is the session token, anything else a refresh token). `None` when there
/// is no token in it.
pub fn parse_pasted(text: &str) -> Option<(Option<String>, Option<String>)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut token = None;
    let mut refresh = None;
    let mut take = |key: &str, value: &str| {
        let k: String = key
            .to_ascii_lowercase()
            .chars()
            .filter(char::is_ascii_alphabetic)
            .collect();
        let v = value.trim().trim_matches(['"', '\'']).trim();
        if v.is_empty() {
            return;
        }
        if k == "authorization" {
            if let Some(b) = v
                .strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
            {
                token.get_or_insert(b.trim().to_string());
            }
        } else if matches!(k.as_str(), "refreshtoken" | "refresh" | "rt") {
            refresh.get_or_insert(v.to_string());
        } else if matches!(
            k.as_str(),
            "token" | "jwt" | "accesstoken" | "sessiontoken" | "authtoken"
        ) {
            token.get_or_insert(v.to_string());
        }
    };
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) {
        for (k, v) in &map {
            match v {
                Value::String(s) => take(k, s),
                Value::Object(inner) => {
                    for (k, v) in inner {
                        if let Value::String(s) = v {
                            take(k, s);
                        }
                    }
                }
                _ => {}
            }
        }
    } else {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        for line in &lines {
            if let Some((k, v)) = line.split_once([':', '=']) {
                take(k.trim(), v);
            }
        }
        if token.is_none() && refresh.is_none() && lines.len() == 1 {
            let only = lines[0].trim_matches(['"', '\'']);
            if let Some(b) = only.strip_prefix("Bearer ") {
                token = Some(b.trim().to_string());
            } else if only.starts_with("eyJ") && only.matches('.').count() == 2 {
                token = Some(only.to_string());
            } else if !only.contains(char::is_whitespace) {
                refresh = Some(only.to_string());
            }
        }
    }
    (token.is_some() || refresh.is_some()).then_some((token, refresh))
}

// ---- the API ----

/// Where the API is and the keys its calls need, from the web app's config.
#[derive(Clone)]
struct Api {
    base: String,
    http_key: String,
    server_key: String,
}

fn api() -> Result<Api> {
    static API: Mutex<Option<Api>> = Mutex::new(None);
    if let Some(a) = API.lock().unwrap_or_else(|p| p.into_inner()).clone() {
        return Ok(a);
    }
    let cfg: Value = serde_json::from_str(&http::get_text(CONFIG_URL)?)
        .context("echovrce.com's configuration couldn't be read")?;
    let get = |k: &str| cfg.get(k).and_then(Value::as_str).unwrap_or_default();
    let base = match get("VITE_NAKAMA_API_BASE") {
        b if b.starts_with("https://") => b.trim_end_matches('/').to_string(),
        _ => DEFAULT_API.to_string(),
    };
    let a = Api {
        base,
        http_key: get("VITE_NAKAMA_HTTP_KEY").to_string(),
        server_key: get("VITE_NAKAMA_SERVER_KEY").to_string(),
    };
    if a.http_key.is_empty() {
        bail!("echovrce.com's configuration has no API key");
    }
    *API.lock().unwrap_or_else(|p| p.into_inner()) = Some(a.clone());
    Ok(a)
}

impl Api {
    fn rpc_url(&self, name: &str) -> String {
        format!(
            "{}/rpc/{name}?unwrap&http_key={}",
            self.base,
            url::form_urlencoded::byte_serialize(self.http_key.as_bytes()).collect::<String>()
        )
    }

    fn basic_server(&self) -> String {
        let raw = format!("{}:", self.server_key);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }
}

/// One call: (status, JSON body or `null`). `auth` is an `Authorization` header value.
fn call(
    method: reqwest::Method,
    url: &str,
    body: Option<&Value>,
    auth: Option<&str>,
) -> Result<(u16, Value)> {
    http::block_on(async {
        let mut req = http::client().request(method, url).timeout(TIMEOUT);
        if let Some(b) = body {
            req = req.json(b);
        }
        if let Some(a) = auth {
            req = req.header(reqwest::header::AUTHORIZATION, a);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| anyhow!(Unavailable(format!("EchoVRCE couldn't be reached: {e}"))))?;
        note_clock(resp.headers().get(reqwest::header::DATE));
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    })
}

fn message(body: &Value) -> String {
    ["message", "error"]
        .iter()
        .find_map(|k| body.get(k).and_then(Value::as_str))
        .unwrap_or("no details")
        .chars()
        .take(160)
        .collect()
}

/// The tokens in an answer, if there are any.
fn tokens_in(body: &Value) -> Option<Tokens> {
    let s = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_string);
    let token = s("token").or_else(|| s("access_token"))?;
    Some(Tokens {
        token,
        refresh_token: s("refresh_token").or_else(|| s("refreshToken")),
    })
}

// ---- signing in with a device code ----

/// A code to approve on echovrce.com.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub code: String,
    /// How long it can be approved for.
    pub expires_in: Duration,
}

/// Asks for a device code.
pub fn request_code() -> Result<DeviceCode> {
    let a = api()?;
    let (status, body) = call(
        reqwest::Method::POST,
        &a.rpc_url("device/auth/request"),
        Some(&json!({})),
        None,
    )?;
    if unavailable_status(status) {
        return Err(unavailable(status, &body));
    }
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .filter(|_| status == 200);
    let Some(code) = code else {
        bail!(
            "EchoVRCE didn't give a sign-in code ({status}: {}).",
            message(&body)
        );
    };
    Ok(DeviceCode {
        code: code.to_string(),
        expires_in: Duration::from_secs(
            body.get("expires_in")
                .and_then(Value::as_u64)
                .unwrap_or(300),
        ),
    })
}

/// The page that approves `code`, with it filled in.
pub fn approve_url(code: &str) -> String {
    let plain: String = code.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("{WEB}/login/device?code={plain}")
}

/// What polling a device code found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// Not approved yet.
    Pending,
    Approved(Tokens),
}

/// Asks whether `code` was approved yet.
pub fn poll(code: &str) -> Result<Poll> {
    let a = api()?;
    let (status, body) = call(
        reqwest::Method::POST,
        &a.rpc_url("device/auth/poll"),
        Some(&json!({ "code": code })),
        None,
    )?;
    poll_answer(status, &body)
}

/// Pure: what an answer to a poll means. EchoVRCE not answering ([`Unavailable`]) isn't
/// the code's fault: the caller asks again.
fn poll_answer(status: u16, body: &Value) -> Result<Poll> {
    match (status, tokens_in(body)) {
        (200, Some(t)) => Ok(Poll::Approved(t)),
        (200, None) if body.get("status").and_then(Value::as_str) == Some("pending") => {
            Ok(Poll::Pending)
        }
        (s, _) if unavailable_status(s) => Err(unavailable(s, body)),
        _ => bail!(
            "The sign-in code didn't work ({status}: {}). Try again.",
            message(body)
        ),
    }
}

/// A second session for the same account, as another device of it -- for echovrce.com
/// inside the window, and for the game (nEVR), which renew (and so rotate) their own
/// refresh tokens: sharing the launcher's would sign one of them out. `token` is the
/// launcher's session.
pub fn link_device(token: &str) -> Result<Tokens> {
    let a = api()?;
    let auth = format!("Bearer {token}");
    let rpc = |name: &str| format!("{}/rpc/{name}?unwrap", a.base);
    let (status, body) = call(
        reqwest::Method::POST,
        &rpc("device/auth/request"),
        Some(&json!({})),
        Some(&auth),
    )?;
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .filter(|_| status == 200)
        .map(str::to_string);
    let Some(code) = code else {
        if status == 401 {
            return Err(SignedOut.into());
        }
        bail!(
            "EchoVRCE didn't link a session ({status}: {}).",
            message(&body)
        );
    };
    let (status, body) = call(
        reqwest::Method::POST,
        &rpc("device/auth/verify"),
        Some(&json!({ "code": code })),
        Some(&auth),
    )?;
    if status != 200 {
        bail!(
            "EchoVRCE didn't approve the linked session ({status}: {}).",
            message(&body)
        );
    }
    for _ in 0..10 {
        let (status, body) = call(
            reqwest::Method::POST,
            &rpc("device/auth/poll"),
            Some(&json!({ "code": code })),
            Some(&auth),
        )?;
        if status == 200 {
            if let Some(t) = tokens_in(&body) {
                return Ok(t);
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    bail!("EchoVRCE didn't hand over the linked session. Try again.")
}

/// The script that signs echovrce.com in with `tokens` (a session linked for it) before
/// its own scripts run, once per linked session: later the site renews that session
/// itself. A newly linked session always replaces what the site had (on macOS its storage
/// outlives the launcher's data, so it may still hold an ended session of the same
/// account). With no tokens it only signs out a session left from another account.
pub fn site_sign_in_script(account: &str, tokens: Option<&Tokens>) -> String {
    use sha2::{Digest, Sha256};
    let js = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let (session, set) = match tokens {
        Some(t) => (
            hex::encode(&Sha256::digest(t.token.as_bytes())[..8]),
            format!(
                "localStorage.setItem('jwt',{});{}",
                js(&t.token),
                t.refresh_token
                    .as_deref()
                    .map(|r| format!("localStorage.setItem('refreshToken',{});", js(r)))
                    .unwrap_or_default()
            ),
        ),
        None => (String::new(), String::new()),
    };
    format!(
        "(function(){{try{{var want={account},have=localStorage.getItem('echovrLauncherAccount')||'',\
         session={session},had=localStorage.getItem('echovrLauncherSession')||'';\
         if(have!==want||(session&&had!==session)){{localStorage.removeItem('jwt');\
         localStorage.removeItem('refreshToken');localStorage.removeItem('authTokenExpiry');{set}\
         localStorage.setItem('echovrLauncherAccount',want);\
         localStorage.setItem('echovrLauncherSession',session||had);}}}}catch(e){{}}}})();",
        account = js(account),
        session = js(&session),
    )
}

// ---- keeping the session ----

/// Renews the session with `refresh_token`: as a device, then as a plain session. A
/// refused refresh token means signing in again (`SignedOut`).
pub fn refresh(refresh_token: &str) -> Result<Tokens> {
    let a = api()?;
    let attempts = [
        (
            a.rpc_url("device/auth/refresh"),
            json!({ "refresh_token": refresh_token, "token": refresh_token }),
            None,
        ),
        (
            format!("{}/account/session/refresh", a.base),
            json!({ "token": refresh_token }),
            Some(a.basic_server()),
        ),
    ];
    let mut refused = true;
    let mut down = None;
    for (url, body, auth) in attempts {
        let (status, answer) = call(reqwest::Method::POST, &url, Some(&body), auth.as_deref())?;
        if status == 200 {
            if let Some(mut t) = tokens_in(&answer) {
                // A refresh that doesn't rotate keeps the old refresh token.
                t.refresh_token
                    .get_or_insert_with(|| refresh_token.to_string());
                return Ok(t);
            }
        }
        tracing::info!("echovrce: refresh attempt answered {status}");
        refused &= matches!(status, 400 | 401 | 403 | 404);
        if unavailable_status(status) {
            down = Some(unavailable(status, &answer));
        }
    }
    match down {
        _ if refused => Err(SignedOut.into()),
        Some(e) => Err(e),
        None => bail!("EchoVRCE didn't renew the session right now. It tries again later."),
    }
}

/// The session good for at least `MARGIN` more: `tokens` itself, or renewed (`Some`).
pub fn ensure(tokens: &Tokens) -> Result<Option<Tokens>> {
    if tokens.fresh(now()) {
        return Ok(None);
    }
    match &tokens.refresh_token {
        Some(rt) => refresh(rt).map(Some),
        // A pasted token without a refresh token lasts until it runs out.
        None if tokens.expires().is_some_and(|e| e > now()) => Ok(None),
        None => Err(SignedOut.into()),
    }
}

/// Your account, as the profile shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Account {
    /// The account's id on the game service.
    pub id: String,
    pub username: String,
    pub display_name: String,
    /// The Discord user it belongs to (`custom_id`): joins are made in its name.
    pub discord_id: String,
}

impl Account {
    /// The name to show: the display name, else the username.
    pub fn name(&self) -> &str {
        if self.display_name.is_empty() {
            &self.username
        } else {
            &self.display_name
        }
    }
}

/// Reads the account `token` belongs to; `SignedOut` when the token is refused.
pub fn account(token: &str) -> Result<Account> {
    let a = api()?;
    let (status, body) = call(
        reqwest::Method::GET,
        &format!("{}/account", a.base),
        None,
        Some(&format!("Bearer {token}")),
    )?;
    match status {
        200 => {
            let user = body.get("user").unwrap_or(&Value::Null);
            let s = |k: &str| {
                user.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Ok(Account {
                id: s("id"),
                username: s("username"),
                display_name: s("display_name"),
                discord_id: body
                    .get("custom_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        }
        401 => Err(SignedOut.into()),
        s if unavailable_status(s) => Err(unavailable(s, &body)),
        _ => bail!(
            "EchoVRCE didn't answer right now ({status}: {}).",
            message(&body)
        ),
    }
}

/// Renews the session if it is due, then reads the account: (renewed session, account).
/// `SignedOut` when it can't go on.
pub fn check(tokens: &Tokens) -> Result<(Option<Tokens>, Account)> {
    let renewed = ensure(tokens)?;
    let current = renewed.as_ref().unwrap_or(tokens);
    match account(&current.token) {
        // Refused although it looked valid (revoked, or the clock is off): renew once.
        Err(e) if is_signed_out(&e) && renewed.is_none() => {
            let rt = current.refresh_token.clone().ok_or(SignedOut)?;
            let again = refresh(&rt)?;
            let acc = account(&again.token)?;
            Ok((Some(again), acc))
        }
        r => r.map(|acc| (renewed, acc)),
    }
}

/// Ends the session on the server (the local copy is forgotten anyway). A token that ran
/// out is renewed first. EchoVRCE refuses its own refresh tokens there (400): then the
/// session token alone is ended, and the refresh token runs out by itself.
pub fn logout(tokens: &Tokens) -> Result<()> {
    let (mut status, mut body) = logout_with(tokens, true)?;
    if status == 401 {
        match tokens.refresh_token.as_deref().map(refresh) {
            Some(Ok(renewed)) => (status, body) = logout_with(&renewed, true)?,
            // Nothing left to end.
            Some(Err(e)) if is_signed_out(&e) => return Ok(()),
            Some(Err(e)) => return Err(e),
            None => return Ok(()),
        }
    }
    if status == 400 && tokens.refresh_token.is_some() {
        (status, body) = logout_with(tokens, false)?;
    }
    if !(200..300).contains(&status) {
        bail!(
            "EchoVRCE didn't end the session ({status}: {})",
            message(&body)
        );
    }
    Ok(())
}

/// `/session/logout` for `tokens` (`with_refresh`: its refresh token too).
fn logout_with(tokens: &Tokens, with_refresh: bool) -> Result<(u16, Value)> {
    let a = api()?;
    let mut body = json!({ "token": tokens.token });
    if with_refresh {
        body["refresh_token"] = json!(tokens.refresh_token);
    }
    call(
        reqwest::Method::POST,
        &format!("{}/session/logout", a.base),
        Some(&body),
        Some(&format!("Bearer {}", tokens.token)),
    )
}

// ---- where the session is kept ----

pub mod store {
    //! The session between runs: the Windows Credential Manager, else (macOS, Linux) a
    //! file in the data folder only the user can read. Not the macOS Keychain: it asks for
    //! access again whenever an unsigned build changes, so at every start of a new build.

    use std::sync::{Mutex, MutexGuard};

    use super::Tokens;
    use anyhow::{Context, Result};

    /// Counts the times the session was forgotten: a save for an earlier one (a renewal
    /// still under way when you signed out) is dropped. Held while the store changes.
    static ERA: Mutex<u64> = Mutex::new(0);

    fn era_lock() -> MutexGuard<'static, u64> {
        ERA.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The session's era now, for a later [`save`].
    pub fn era() -> u64 {
        *era_lock()
    }

    #[cfg(windows)]
    const SERVICE: &str = "EchoVR Launcher";
    #[cfg(windows)]
    const ACCOUNT: &str = "echovrce-session";

    fn file() -> std::path::PathBuf {
        crate::core::paths::data_dir().join("echovrce.session.json")
    }

    #[cfg(windows)]
    fn entry() -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, ACCOUNT)
            .map_err(|e| tracing::warn!("echovrce: credential store unavailable: {e}"))
            .ok()
    }

    /// The saved session, if there is one.
    pub fn load() -> Option<Tokens> {
        #[cfg(windows)]
        if let Some(json) = entry().and_then(|e| e.get_password().ok()) {
            return serde_json::from_str(&json).ok();
        }
        let text = std::fs::read_to_string(file()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Keeps `tokens` for the next run, unless the session was forgotten since `era`.
    pub fn save(tokens: &Tokens, era: u64) -> Result<()> {
        let held = era_lock();
        if *held != era {
            tracing::info!("echovrce: a session that came after signing out wasn't kept");
            return Ok(());
        }
        let json = serde_json::to_string(tokens)?;
        #[cfg(windows)]
        if let Some(e) = entry() {
            match e.set_password(&json) {
                Ok(()) => {
                    let _ = std::fs::remove_file(file());
                    return Ok(());
                }
                Err(err) => tracing::warn!("echovrce: credential store refused the session: {err}"),
            }
        }
        write_private(&file(), &json)
    }

    /// Forgets the saved session (and drops saves still to come for it).
    pub fn forget() {
        let mut held = era_lock();
        *held += 1;
        #[cfg(windows)]
        if let Some(e) = entry() {
            match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(err) => {
                    tracing::warn!("echovrce: the credential store kept the session: {err}")
                }
            }
        }
        match std::fs::remove_file(file()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("echovrce: {} stayed: {e}", file().display()),
        }
    }

    /// Writes `text` to `path` readable by its owner only, replacing it in one step.
    fn write_private(path: &std::path::Path, text: &str) -> Result<()> {
        let dir = path.parent().context("no data folder")?;
        std::fs::create_dir_all(dir)?;
        let tmp = path.with_extension("tmp");
        {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            std::io::Write::write_all(&mut f, text.as_bytes())?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        #[test]
        #[cfg(unix)]
        fn the_file_is_private() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("s.json");
            super::write_private(&p, "{}").unwrap();
            let mode = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A poll that EchoVRCE doesn't answer is asked again; a refused code isn't.
    #[test]
    fn poll_answers() {
        let pending = json!({ "status": "pending" });
        assert_eq!(poll_answer(200, &pending).unwrap(), Poll::Pending);
        let approved = json!({ "token": "t", "refresh_token": "r" });
        assert!(matches!(
            poll_answer(200, &approved).unwrap(),
            Poll::Approved(_)
        ));
        for status in [502, 503, 500, 429] {
            let e = poll_answer(status, &Value::Null).unwrap_err();
            assert!(is_unavailable(&e), "{status}");
            assert_eq!(
                e.to_string(),
                format!("EchoVRCE isn't answering right now ({status})")
            );
        }
        let e = poll_answer(503, &json!({ "message": "maintenance" })).unwrap_err();
        assert_eq!(
            e.to_string(),
            "EchoVRCE isn't answering right now (503: maintenance)"
        );
        for status in [400, 404, 410] {
            let e = poll_answer(status, &json!({ "message": "expired" })).unwrap_err();
            assert!(!is_unavailable(&e), "{status}");
        }
    }

    /// A JWT with this payload (unsigned: only the payload is read).
    fn jwt(payload: &str) -> String {
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        format!("eyJhbGciOiJIUzI1NiJ9.{p}.c2ln")
    }

    #[test]
    fn reads_the_expiry() {
        assert_eq!(
            jwt_exp(&jwt(r#"{"exp":1790000000,"uid":"x"}"#)),
            Some(1_790_000_000)
        );
        assert_eq!(jwt_exp("not-a-jwt"), None);
        let t = Tokens {
            token: jwt(r#"{"exp":1000}"#),
            refresh_token: None,
        };
        assert!(t.fresh(1000 - 301));
        assert!(!t.fresh(1000 - 299));
    }

    /// EchoVRCE's clock comes from its answers' `Date`, and a clock that is off reads
    /// as the player would say it.
    #[test]
    fn reads_echovrces_clock() {
        assert_eq!(
            http_date("Sat, 10 Oct 2026 16:41:29 GMT"),
            Some(1_791_650_489)
        );
        assert_eq!(http_date("Sat, 10 Oct 2026 16:41:29"), None);
        assert_eq!(http_date("yesterday"), None);
        // The player's PC: 4 h ahead (EchoVRCE's clock behind it).
        assert_eq!(clock_off(-14_399).as_deref(), Some("4 h 0 min ahead"));
        assert_eq!(clock_off(600).as_deref(), Some("10 min behind"));
        assert_eq!(clock_off(-90), None);
    }

    #[test]
    fn never_prints_a_token() {
        let t = Tokens {
            token: jwt(r#"{"exp":1000}"#),
            refresh_token: Some("secret-refresh".into()),
        };
        let shown = format!("{t:?}");
        assert!(!shown.contains("secret-refresh"));
        assert!(!shown.contains("eyJ"));
    }

    #[test]
    fn reads_pasted_credentials() {
        let tok = jwt(r#"{"exp":1}"#);
        let both = |t: &str, r: &str| Some((Some(t.to_string()), Some(r.to_string())));
        assert_eq!(
            parse_pasted(&format!(r#"{{"token":"{tok}","refresh_token":"rt1"}}"#)),
            both(&tok, "rt1")
        );
        assert_eq!(
            parse_pasted(&format!("refreshToken = rt2\njwt={tok}")),
            both(&tok, "rt2")
        );
        assert_eq!(
            parse_pasted(&format!("authorization: Bearer {tok}")),
            Some((Some(tok.clone()), None))
        );
        assert_eq!(parse_pasted(&tok), Some((Some(tok.clone()), None)));
        assert_eq!(
            parse_pasted("rt-only"),
            Some((None, Some("rt-only".into())))
        );
        assert_eq!(parse_pasted(""), None);
        assert_eq!(parse_pasted("hello there friend"), None);
    }

    #[test]
    fn site_script_signs_in_once_per_account() {
        let t = Tokens {
            token: "a.b'c".into(),
            refresh_token: Some("r\"1".into()),
        };
        let s = site_sign_in_script("acc-1", Some(&t));
        assert!(s.contains(r#"var want="acc-1""#));
        // Values are JSON strings: quotes in them can't break out.
        assert!(s.contains(r#"localStorage.setItem('jwt',"a.b'c")"#));
        assert!(s.contains(r#"localStorage.setItem('refreshToken',"r\"1")"#));
        let none = site_sign_in_script("", None);
        assert!(!none.contains("setItem('jwt'"));
        assert!(none.contains("removeItem('jwt')"));
    }

    #[test]
    fn approval_link_has_the_code() {
        assert_eq!(
            approve_url("C6N4-QX7Z"),
            "https://echovrce.com/login/device?code=C6N4QX7Z"
        );
    }

    #[test]
    fn tokens_from_answers() {
        assert_eq!(
            tokens_in(&json!({"token": "a", "refresh_token": "b"})),
            Some(Tokens {
                token: "a".into(),
                refresh_token: Some("b".into())
            })
        );
        assert_eq!(tokens_in(&json!({"status": "pending"})), None);
    }

    /// The real sign-in: a code comes back and is pending (needs network).
    #[test]
    #[ignore]
    fn live_device_code_is_pending() {
        let c = request_code().unwrap();
        assert_eq!(c.code.len(), 9);
        assert_eq!(poll(&c.code).unwrap(), Poll::Pending);
    }
}
