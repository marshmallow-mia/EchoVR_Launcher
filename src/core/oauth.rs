//! Discord OAuth2 in the system browser, exchanging the code for a personalized patch URL.
//!
//! Fixes over the Java flow: a random `state` is sent and checked (any local page could
//! otherwise inject a code), the callback server keeps listening until the real
//! `/callback` arrives (a favicon or preconnect request used to consume the single accept
//! and read as a timeout), a denied consent is reported as such, the exchange JSON is
//! serialized properly, and the returned patch URL is validated before it is downloaded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::core::launcher::versions::Step;

const CLIENT_ID: &str = "1326594571584409650";
const SERVER_URL: &str = "https://files.echovr.de";
/// Must match the redirect registered in the Discord developer portal.
const CALLBACK_PORT: u16 = 53124;
/// Discord's in-browser "Service got rate limited" page never redirects back, so the only
/// signal is the callback not arriving; keep the wait short so trying again is quick.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(60);
/// The Echo VR Patcher server: only its members get a patch.
pub const PATCHER_INVITE: &str = "https://discord.gg/bMpsva6fmA";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Dll,
    Apk,
}

impl FileType {
    fn as_str(self) -> &'static str {
        match self {
            FileType::Dll => "dll",
            FileType::Apk => "apk",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthError {
    NotInGuild(String),
    PhoneVerification,
    Busy(String),
    Cancelled,
    Timeout,
    Denied,
    PortInUse,
    NoBrowser(String),
    Server(String),
}

impl OAuthError {
    /// (dialog title, message) as the Java `OAuth2ErrorHandler` showed them.
    /// `None` for a user-initiated cancel, which shows nothing.
    pub fn dialog(&self) -> Option<(&'static str, String)> {
        Some(match self {
            OAuthError::NotInGuild(m) => ("Join Server First", m.clone()),
            OAuthError::PhoneVerification => (
                "Phone Verification Required",
                "Discord requires a verified phone number to interact in this server.\n\nPlease verify your phone number in Discord Settings → Account → Phone Number, then try again.".into(),
            ),
            OAuthError::Busy(m) => ("Bot Busy", m.clone()),
            OAuthError::Cancelled => return None,
            OAuthError::Timeout => (
                "Try again in a minute",
                "Discord didn't complete the authorization in time.\n\nIf you saw a \"Service got rate limited\" message, that's Discord throttling —\nwait about a minute, then try again.".into(),
            ),
            OAuthError::Denied => (
                "Authorization Failed",
                "The Discord authorization was cancelled in the browser.\nTry again and click \"Authorize\" to continue.".into(),
            ),
            OAuthError::PortInUse => (
                "Authorization busy",
                format!("Couldn't open the Discord callback port ({CALLBACK_PORT}).\nAnother authorization may still be finishing — wait a moment and try again."),
            ),
            OAuthError::NoBrowser(url) => (
                "Authorization Failed",
                format!("Could not open your browser automatically.\nPlease open this URL manually:\n{url}"),
            ),
            OAuthError::Server(m) => ("Authorization Failed", m.clone()),
        })
    }
}

/// What the callback server saw on one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callback {
    /// Not our redirect (favicon, preconnect, stale state) -- keep waiting.
    Ignore,
    Code(String),
    Denied,
}

/// Pure: interprets a request target like `/callback?code=..&state=..`.
pub fn parse_callback(target: &str, expected_state: &str) -> Callback {
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Ignore;
    };
    if url.path() != "/callback" {
        return Callback::Ignore;
    }
    let get = |k: &str| {
        url.query_pairs()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.into_owned())
    };
    if get("state").as_deref() != Some(expected_state) {
        return Callback::Ignore;
    }
    if get("error").is_some() {
        return Callback::Denied;
    }
    match get("code") {
        Some(c) if !c.is_empty() => Callback::Code(c),
        _ => Callback::Ignore,
    }
}

fn authorize_url(state: &str) -> String {
    let redirect = format!("http://127.0.0.1:{CALLBACK_PORT}/callback");
    let mut u = url::Url::parse("https://discord.com/api/oauth2/authorize").expect("static url");
    u.query_pairs_mut()
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", "identify guilds")
        .append_pair("state", state);
    u.into()
}

fn bind() -> Result<tiny_http::Server, OAuthError> {
    for _ in 0..5 {
        match tiny_http::Server::http(("127.0.0.1", CALLBACK_PORT)) {
            Ok(s) => return Ok(s),
            Err(e) => {
                tracing::warn!("OAuth: bind {CALLBACK_PORT} failed: {e}");
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    Err(OAuthError::PortInUse)
}

fn respond(req: tiny_http::Request, status: u16, body: &str) {
    let html =
        format!("<html><body style=\"font-family:sans-serif\"><h1>{body}</h1></body></html>");
    let header =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=UTF-8"[..])
            .expect("static header");
    let _ = req.respond(
        tiny_http::Response::from_string(html)
            .with_status_code(status)
            .with_header(header),
    );
}

/// Runs the whole flow on the calling worker thread. Returns the patch download URL.
pub fn run(
    file_type: FileType,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<String, OAuthError> {
    on(Step::Status("Opening Discord in your browser...".into()));
    let server = bind()?;
    let state = hex::encode(rand::random::<[u8; 16]>());
    let auth_url = authorize_url(&state);
    tracing::info!(
        "OAuth: opening Discord's authorization page ({}); waiting on 127.0.0.1:{CALLBACK_PORT} for up to {}s",
        file_type.as_str(),
        CALLBACK_TIMEOUT.as_secs()
    );
    if crate::core::platform::try_open_url(&auth_url).is_err() {
        return Err(OAuthError::NoBrowser(auth_url));
    }
    on(Step::Browser(auth_url.clone()));

    let deadline = Instant::now() + CALLBACK_TIMEOUT;
    let code = loop {
        if cancel.load(Ordering::Relaxed) {
            tracing::info!("OAuth: cancelled while waiting for Discord");
            return Err(OAuthError::Cancelled);
        }
        if Instant::now() >= deadline {
            tracing::warn!(
                "OAuth: no answer from Discord within {}s (the browser didn't open, or the authorization wasn't finished there)",
                CALLBACK_TIMEOUT.as_secs()
            );
            return Err(OAuthError::Timeout);
        }
        let req = match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!("OAuth: callback server failed: {e}");
                return Err(OAuthError::Server(format!("Callback server failed: {e}")));
            }
        };
        match parse_callback(req.url(), &state) {
            Callback::Ignore => {
                let path = req.url().split('?').next().unwrap_or("").to_string();
                tracing::info!("OAuth: ignored a request for {path}");
                respond(req, 404, "Not found")
            }
            Callback::Denied => {
                tracing::info!("OAuth: authorization cancelled on Discord's page");
                respond(
                    req,
                    200,
                    "Authorization cancelled. You can close this window.",
                );
                return Err(OAuthError::Denied);
            }
            Callback::Code(c) => {
                tracing::info!("OAuth: Discord answered with a code");
                respond(
                    req,
                    200,
                    "Authorization complete! You can close this window.",
                );
                break c;
            }
        }
    };
    drop(server); // free the port right away for another try

    on(Step::Status("Generating your patch file...".into()));
    exchange(&code, file_type)
}

#[derive(Deserialize)]
struct ExchangeResponse {
    #[serde(rename = "patchUrl")]
    patch_url: Option<String>,
}

fn exchange(code: &str, file_type: FileType) -> Result<String, OAuthError> {
    let body = serde_json::json!({ "code": code, "type": file_type.as_str() });
    let (status, text) = crate::core::http::post_json(&format!("{SERVER_URL}/api/exchange"), &body)
        .map_err(|e| OAuthError::Server(format!("Couldn't reach the patch server:\n{e:#}")))?;
    tracing::info!("OAuth: exchange returned status {status}");
    interpret_exchange(status, &text, file_type)
}

/// Pure: maps the exchange response for a `file_type` request to a validated patch URL or
/// an error.
pub fn interpret_exchange(
    status: u16,
    body: &str,
    file_type: FileType,
) -> Result<String, OAuthError> {
    if status == 403 && body.contains("not_in_guild") {
        return Err(OAuthError::NotInGuild(format!(
            "You must join the Echo VR Patcher server first.\n{PATCHER_INVITE}"
        )));
    }
    if status == 403 && body.contains("phone_verification") {
        return Err(OAuthError::PhoneVerification);
    }
    if status == 409 && body.contains("busy") {
        return Err(OAuthError::Busy(
            "Bot is busy generating another file. Please try again in 30 seconds.".into(),
        ));
    }
    if status != 200 {
        let snippet: String = body.chars().take(300).collect();
        return Err(OAuthError::Server(format!(
            "Server returned error ({status}): {snippet}"
        )));
    }
    let url = serde_json::from_str::<ExchangeResponse>(body)
        .ok()
        .and_then(|r| r.patch_url)
        .ok_or_else(|| OAuthError::Server("Failed to get patch URL from server".into()))?;
    // The patch server posts a PC patch as an attachment in your Discord thread and
    // answers with its link (a Quest build goes on files.echovr.de): the same links a
    // pasted patch link may be.
    let trusted = match file_type {
        FileType::Dll => is_trusted_patch_host(&url) || validate_dll_url(&url).is_some(),
        FileType::Apk => is_trusted_patch_host(&url),
    };
    if !trusted {
        tracing::warn!(
            "OAuth: the patch server sent an unexpected download location: {}",
            crate::core::download::redact(&url)
        );
        return Err(OAuthError::Server(
            "The server returned an unexpected download location.".into(),
        ));
    }
    Ok(url)
}

fn https_host(url: &str) -> Option<(String, String)> {
    let u = url::Url::parse(url.trim()).ok()?;
    if u.scheme() != "https" {
        return None;
    }
    Some((u.host_str()?.to_ascii_lowercase(), u.path().to_string()))
}

fn is_trusted_patch_host(url: &str) -> bool {
    matches!(https_host(url), Some((h, _)) if h == "files.echovr.de" || h == "evr.echo.taxi")
}

/// A pasted licence-patch URL: a `files.echovr.de` link, or the legacy Discord CDN
/// `pnsovr.dll` attachment. Parsed, so `files.echovr.de.evil.com` no longer passes.
pub fn validate_dll_url(url: &str) -> Option<String> {
    let (host, path) = https_host(url)?;
    let ok = host == "files.echovr.de"
        || (host == "cdn.discordapp.com"
            && path.starts_with("/attachments/")
            && path.ends_with("/pnsovr.dll"));
    ok.then(|| url.trim().to_string())
}

/// A pasted patched-APK URL: `files.echovr.de` only.
pub fn validate_apk_url(url: &str) -> Option<String> {
    let (host, _) = https_host(url)?;
    (host == "files.echovr.de").then(|| url.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_parsing() {
        let s = "abc";
        assert_eq!(
            parse_callback("/callback?code=XYZ&state=abc", s),
            Callback::Code("XYZ".into())
        );
        assert_eq!(
            parse_callback("/callback?code=a%2Bb&state=abc", s),
            Callback::Code("a+b".into())
        );
        assert_eq!(parse_callback("/favicon.ico", s), Callback::Ignore);
        assert_eq!(
            parse_callback("/callback?code=XYZ&state=evil", s),
            Callback::Ignore
        );
        assert_eq!(parse_callback("/callback?code=XYZ", s), Callback::Ignore);
        assert_eq!(
            parse_callback("/callback?error=access_denied&state=abc", s),
            Callback::Denied
        );
    }

    #[test]
    fn authorize_url_carries_state() {
        let u = authorize_url("s1");
        assert!(u.contains("client_id=1326594571584409650"));
        assert!(u.contains("state=s1"));
        assert!(u.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A53124%2Fcallback"));
    }

    #[test]
    fn exchange_mapping() {
        assert!(matches!(
            interpret_exchange(403, "{\"error\":\"not_in_guild\"}", FileType::Dll),
            Err(OAuthError::NotInGuild(_))
        ));
        assert!(matches!(
            interpret_exchange(409, "busy", FileType::Dll),
            Err(OAuthError::Busy(_))
        ));
        assert!(matches!(
            interpret_exchange(500, "x", FileType::Dll),
            Err(OAuthError::Server(_))
        ));
        assert_eq!(
            interpret_exchange(
                200,
                "{\"patchUrl\": \"https://files.echovr.de/p/abc/pnsovr.dll\"}",
                FileType::Dll
            )
            .unwrap(),
            "https://files.echovr.de/p/abc/pnsovr.dll"
        );
        // A PC patch comes as a Discord attachment, as the patch server answers today.
        let discord = "https://cdn.discordapp.com/attachments/1555314284198625333/1555314300749357208/pnsovr.dll?backend=b2&ex=1&is=2&hm=3&";
        let body = format!("{{\"patchUrl\": \"{discord}\"}}");
        assert_eq!(
            interpret_exchange(200, &body, FileType::Dll).unwrap(),
            discord
        );
        // ...but not as a Quest build, nor any other file there.
        assert!(interpret_exchange(200, &body, FileType::Apk).is_err());
        let other = "{\"patchUrl\":\"https://cdn.discordapp.com/attachments/1/2/evil.exe\"}";
        assert!(interpret_exchange(200, other, FileType::Dll).is_err());
        let apk = "{\"patchUrl\":\"https://files.echovr.de/apks/1/personilizedechoapk.apk\"}";
        assert!(interpret_exchange(200, apk, FileType::Apk).is_ok());
        let evil = "{\"patchUrl\":\"https://evil.example/x\"}";
        assert!(interpret_exchange(200, evil, FileType::Dll).is_err());
        assert!(interpret_exchange(200, "{}", FileType::Dll).is_err());
    }

    #[test]
    fn pasted_url_validation() {
        assert!(validate_dll_url("https://files.echovr.de/x/pnsovr.dll").is_some());
        assert!(
            validate_dll_url("https://cdn.discordapp.com/attachments/1/2/pnsovr.dll").is_some()
        );
        assert!(validate_dll_url("https://cdn.discordapp.com/attachments/1/2/other.exe").is_none());
        assert!(validate_dll_url("https://files.echovr.de.evil.com/x").is_none());
        assert!(validate_dll_url("http://files.echovr.de/x").is_none());
        assert!(validate_apk_url("https://files.echovr.de/q.apk").is_some());
        assert!(
            validate_apk_url("https://cdn.discordapp.com/attachments/1/2/pnsovr.dll").is_none()
        );
        assert!(validate_apk_url("").is_none());
    }
}
