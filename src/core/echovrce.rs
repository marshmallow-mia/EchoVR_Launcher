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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
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
            .map_err(|e| anyhow!("EchoVRCE couldn't be reached: {e}"))?;
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
    match (status, tokens_in(&body)) {
        (200, Some(t)) => Ok(Poll::Approved(t)),
        (200, None) if body.get("status").and_then(Value::as_str) == Some("pending") => {
            Ok(Poll::Pending)
        }
        _ => bail!(
            "The sign-in code didn't work ({status}: {}). Try again.",
            message(&body)
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
/// its own scripts run, once per `account`: later the site renews that session itself.
/// With no tokens it only signs out a session left from another account.
pub fn site_sign_in_script(account: &str, tokens: Option<&Tokens>) -> String {
    let js = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let set = match tokens {
        Some(t) => format!(
            "localStorage.setItem('jwt',{});{}",
            js(&t.token),
            t.refresh_token
                .as_deref()
                .map(|r| format!("localStorage.setItem('refreshToken',{});", js(r)))
                .unwrap_or_default()
        ),
        None => String::new(),
    };
    format!(
        "(function(){{try{{var want={account},have=localStorage.getItem('echovrLauncherAccount')||'';\
         if(have!==want){{localStorage.removeItem('jwt');localStorage.removeItem('refreshToken');\
         localStorage.removeItem('authTokenExpiry');{set}localStorage.setItem('echovrLauncherAccount',want);}}}}catch(e){{}}}})();",
        account = js(account)
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
    }
    if refused {
        Err(SignedOut.into())
    } else {
        bail!("EchoVRCE didn't renew the session right now. It tries again later.")
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

/// Ends the session on the server (best effort; the local copy is forgotten anyway).
pub fn logout(tokens: &Tokens) {
    let Ok(a) = api() else {
        return;
    };
    let body = json!({ "token": tokens.token, "refresh_token": tokens.refresh_token });
    let _ = call(
        reqwest::Method::POST,
        &format!("{}/session/logout", a.base),
        Some(&body),
        Some(&format!("Bearer {}", tokens.token)),
    );
}

// ---- where the session is kept ----

pub mod store {
    //! The session between runs: the Windows Credential Manager, else (macOS, Linux) a
    //! file in the data folder only the user can read. Not the macOS Keychain: it asks for
    //! access again whenever an unsigned build changes, so at every start of a new build.

    use super::Tokens;
    use anyhow::{Context, Result};

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

    /// Keeps `tokens` for the next run.
    pub fn save(tokens: &Tokens) -> Result<()> {
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

    /// Forgets the saved session.
    pub fn forget() {
        #[cfg(windows)]
        if let Some(e) = entry() {
            let _ = e.delete_credential();
        }
        let _ = std::fs::remove_file(file());
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
