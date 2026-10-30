//! What a game server's log says, line by line: the lines the supervisor acts on (signed
//! in, registered, a match starting or ending, a fatal error), from nEVR's jsonl or its
//! plain console text alike. After evr-supervisor (Echo-VR-Server-on-Docker), measured on
//! nEVR 5.4.2; the sign-in by code on 6.9.0.

/// A line the supervisor acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    None,
    /// `NEVR-BUILD <version>`.
    RuntimeVersion,
    LoginSuccess,
    /// EchoVRCE turned the server's sign-in down.
    LoginFailed,
    Authenticated,
    /// The UDP port the broadcaster bound.
    Bound,
    /// ServerDB can't be reached: nEVR retries for ever, the register timeout ends it.
    ServerDbError,
    Registered,
    SessionStart,
    SessionEnd,
    /// The server unregistered after Ctrl+C; nEVR then hangs (#723) until it is killed.
    Teardown,
    Shutdown,
    Fatal,
    /// The game loop crashed and won't run again (`outcome=server_hold`).
    Held,
    /// nEVR has neither a token nor a password to sign in with.
    NoCredentials,
    /// A server without an account (nEVR 6.9.0 on) shows a code to approve: `url` (the
    /// code in it), for `expires` seconds. A new one follows when it runs out.
    SignInCode,
    /// Its sign-in by code ended: `ok` when approved.
    SignInEnded,
}

/// Why a server stopped. Retrying helps, except for a refused sign-in and a broken config:
/// those wait for a change of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    BadCredentials,
    UdpUnreachable,
    InvalidConfig,
    RegistrationRejected,
    NotRegistered,
    Crashed,
    /// The sign-in by code ended without one (EchoVRCE couldn't be asked).
    SignIn,
    Fatal,
}

impl Reason {
    pub fn retryable(self) -> bool {
        !matches!(self, Reason::BadCredentials | Reason::InvalidConfig)
    }

    /// What it means, for the overview.
    pub fn words(self) -> &'static str {
        match self {
            Reason::BadCredentials => "EchoVRCE refused the server's sign-in",
            Reason::UdpUnreachable => {
                "EchoVRCE couldn't reach its port: forward it (UDP) to this PC"
            }
            Reason::InvalidConfig => "The server's settings are wrong",
            Reason::RegistrationRejected => "EchoVRCE refused the server's registration",
            Reason::NotRegistered => "The server didn't register in time",
            Reason::Crashed => "The game crashed",
            Reason::SignIn => "The server's sign-in by code didn't go through",
            Reason::Fatal => "The game stopped on an error",
        }
    }
}

/// One line, classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub kind: Kind,
    /// The line without colours and without its jsonl wrapper.
    pub line: String,
    /// `Bound`, `Registered`.
    pub port: u16,
    /// `Registered`: the address EchoVRCE saw.
    pub ip: String,
    /// `SessionStart`, `SessionEnd`.
    pub session: String,
    /// `RuntimeVersion`.
    pub version: String,
    /// `LoginFailed`, `Fatal`, `Held`.
    pub reason: Option<Reason>,
    /// `SignInCode`: the page that approves it, the code in it.
    pub url: String,
    /// `SignInCode`: how many seconds the code is good for.
    pub expires: u32,
    /// `SignInEnded`: approved.
    pub ok: bool,
}

/// Pure: the code a server without an account shows (nEVR 6.9.0 on), a line of its own
/// on its error output: `{"tag":"NEVR.AUTH","event":"server_sign_in_code","code":…,
/// "url":"…?code=…","expires_in_s":300,"msg":…}`. Its log has the code masked. (url,
/// seconds).
fn sign_in_code(raw: &str) -> Option<(String, u32)> {
    let s = raw.trim();
    if !s.starts_with('{') || !s.contains(crate::core::launcher::nevr::SERVER_SIGN_IN_CODE) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let text = |k: &str| v.get(k).and_then(serde_json::Value::as_str).unwrap_or("");
    let url = text("url");
    (text("tag") == "NEVR.AUTH"
        && text("event") == crate::core::launcher::nevr::SERVER_SIGN_IN_CODE
        && (url.starts_with("https://") || url.starts_with("http://")))
    .then(|| {
        let secs = v.get("expires_in_s").and_then(serde_json::Value::as_u64);
        (url.to_string(), secs.unwrap_or(300).min(3600) as u32)
    })
}

/// Pure: whether `raw` is the line with a server's sign-in code (it goes to the page, not
/// into its console file).
pub fn is_sign_in_code(raw: &str) -> bool {
    sign_in_code(&clean_colours(raw)).is_some()
}

/// Pure: whether `raw`, a line of the game's own output, is one its log file has too:
/// nEVR's and the game's (`[NEVR.…]`, `[EVR]`).
pub fn is_logged(raw: &str) -> bool {
    let l = clean(raw);
    l.contains("[NEVR") || l.contains("[EVR]")
}

/// Pure: the code in an approval page's address (`…?code=ABCD`).
pub fn code_in(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let code = u.query_pairs().find(|(k, _)| k == "code")?.1.to_string();
    (!code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then_some(code)
}

/// Pure: `raw` without ANSI colours and, for a jsonl record, its `msg`.
pub fn clean(raw: &str) -> String {
    let s = clean_colours(raw);
    if s.starts_with('{') && s.contains("\"msg\"") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(m) = v
                .get("msg")
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
            {
                return m.trim_end_matches(['\r', '\n']).to_string();
            }
        }
    }
    s
}

/// Pure: `raw` without ANSI colours and its line end.
fn clean_colours(raw: &str) -> String {
    let mut s = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(&n) = chars.peek() {
                chars.next();
                if !(n.is_ascii_digit() || n == ';') {
                    break;
                }
            }
            continue;
        }
        s.push(c);
    }
    s.trim_end_matches(['\r', '\n']).to_string()
}

/// Pure: the level of a jsonl record (`info`, `warn`, `error`), if it is one.
pub fn level(raw: &str) -> Option<String> {
    if !raw.starts_with('{') {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()?
        .get("level")?
        .as_str()
        .map(str::to_string)
}

/// The word after `key` up to whitespace.
fn word_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = &line[line.find(key)? + key.len()..];
    let w = rest.split_whitespace().next()?;
    (!w.is_empty()).then_some(w)
}

/// `…[<ip>:<port>]` after `key`: (ip, port). For `registered as X at [int/pub:port]` the ip
/// is the public one (after the `/`).
fn bracket_after(line: &str, key: &str) -> Option<(String, u16)> {
    let rest = &line[line.find(key)? + key.len()..];
    let open = rest.find('[')?;
    let close = open + rest[open..].find(']')?;
    let inside = &rest[open + 1..close];
    let (addr, port) = inside.rsplit_once(':')?;
    let ip = addr.rsplit('/').next().unwrap_or(addr);
    Some((ip.to_string(), port.parse().ok()?))
}

fn fatal_reason(line: &str) -> Reason {
    if line.contains("device sign-in ended without a token") {
        Reason::SignIn
    } else if line.contains("HTTP 401")
        || line.contains("no valid token")
        || line.contains("the service refused the credential")
    {
        Reason::BadCredentials
    } else if line.contains("registration rejected") && line.contains("code=10") {
        Reason::UdpUnreachable
    } else if line.contains("registration rejected") {
        Reason::RegistrationRejected
    } else if [
        "config.yaml",
        "socket_uri",
        "missing config keys",
        "session_router",
    ]
    .iter()
    .any(|k| line.contains(k))
    {
        Reason::InvalidConfig
    } else {
        Reason::Fatal
    }
}

/// Pure: what `raw` says. Lines that don't matter are `Kind::None`.
pub fn parse(raw: &str) -> Event {
    let line = clean(raw);
    let mut ev = Event {
        kind: Kind::None,
        line,
        port: 0,
        ip: String::new(),
        session: String::new(),
        version: String::new(),
        reason: None,
        url: String::new(),
        expires: 0,
        ok: false,
    };
    if let Some((url, secs)) = sign_in_code(&clean_colours(raw)) {
        ev.kind = Kind::SignInCode;
        ev.line = format!("approve this server: {url}");
        ev.url = url;
        ev.expires = secs;
        return ev;
    }
    let l = ev.line.as_str();
    if l.contains("[NEVR.AUTH] server sign-in ended: result=") {
        ev.kind = Kind::SignInEnded;
        ev.ok = l.contains("result=ok");
    } else if l.contains("NEVR-BUILD ") {
        if let Some(v) = word_after(l, "NEVR-BUILD ") {
            ev.kind = Kind::RuntimeVersion;
            ev.version = v.to_string();
        }
    } else if l.contains("[NEVR.WS] LOGIN SUCCESS") {
        ev.kind = Kind::LoginSuccess;
    } else if l.contains("[NEVR.WS] login failed status=") {
        // A server's login always "fails" this way and nEVR answers it itself (5.4.2):
        // only a refusal it doesn't settle counts.
        if !l.contains("synthesized LoginSuccess") {
            ev.kind = Kind::LoginFailed;
            ev.reason = Some(Reason::BadCredentials);
        }
    } else if l.contains("remote auth: no token and no URL credentials") {
        ev.kind = Kind::NoCredentials;
    } else if l.contains("[NEVR.GAMESERVER] Server authenticated")
        || l.contains("ServerDB auth via refreshed OAuth2 token")
    {
        ev.kind = Kind::Authenticated;
    } else if l.contains("Dedicated: broadcaster initialized at") {
        if let Some((_, port)) = bracket_after(l, "broadcaster initialized at") {
            ev.kind = Kind::Bound;
            ev.port = port;
        }
    } else if l.contains("[NEVR.SERVERDB] Connection error") {
        ev.kind = Kind::ServerDbError;
    } else if l.contains("Dedicated: registered as") {
        if let Some((ip, port)) = bracket_after(l, "Dedicated: registered as") {
            ev.kind = Kind::Registered;
            ev.ip = ip;
            ev.port = port;
        }
    } else if l.contains("[NEVR.GAMESERVER] session starting") {
        ev.kind = Kind::SessionStart;
        ev.session = word_after(l, "session_id=").unwrap_or_default().to_string();
    } else if l.contains("session ended by ServerDB") {
        ev.kind = Kind::SessionEnd;
        ev.session = word_after(l, "session_id=").unwrap_or_default().to_string();
    } else if l.contains("CTRL+C teardown complete") {
        ev.kind = Kind::Teardown;
    } else if l.contains("Graceful shutdown complete") || l.contains("Graceful shutdown initiated")
    {
        ev.kind = Kind::Shutdown;
    } else if l.contains("game loop crash recovered") && l.contains("outcome=server_hold") {
        ev.kind = Kind::Held;
        ev.reason = Some(Reason::Crashed);
    } else if l.contains("[NEVR.FATAL]") {
        ev.kind = Kind::Fatal;
        ev.reason = Some(fatal_reason(l));
    }
    ev
}

/// nEVR's high-volume diagnostics, hidden in the console unless asked for.
const NOISE: &[&str] = &[
    "[NEVR.SOCIAL]",
    "server->game",
    "game->server",
    "hook_liveness",
    "[NEVR.LOGFILTER] health",
    "NSUSER] saved",
    "[NEVR.SELFCHECK]",
    // The sign-in by code asks every 3 s.
    "[NEVR.AUTH] poll #",
];

/// Pure: whether a (clean) line is one of nEVR's high-volume diagnostics.
pub fn is_noise(line: &str) -> bool {
    NOISE.iter().any(|n| line.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<Event> {
        text.lines()
            .map(parse)
            .filter(|e| e.kind != Kind::None)
            .collect()
    }

    #[test]
    fn unwraps_jsonl_and_colours() {
        assert_eq!(
            clean(r#"{"ts":"x","level":"info","msg":"[NEVR.WS] LOGIN SUCCESS\n"}"#),
            "[NEVR.WS] LOGIN SUCCESS"
        );
        assert_eq!(clean("\u{1b}[31mred\u{1b}[0m text\r\n"), "red text");
        assert_eq!(
            level(r#"{"level":"warn","msg":"x"}"#).as_deref(),
            Some("warn")
        );
    }

    #[test]
    fn follows_a_registered_server_until_ctrl_c() {
        let evs = kinds(include_str!("testdata/registered-sigint.jsonl"));
        let k: Vec<Kind> = evs.iter().map(|e| e.kind).collect();
        assert!(k.starts_with(&[Kind::RuntimeVersion, Kind::LoginSuccess, Kind::Bound]));
        assert_eq!(evs[0].version, "5.4.2");
        let bound = evs.iter().find(|e| e.kind == Kind::Bound).unwrap();
        assert_eq!(bound.port, 6795);
        let reg = evs.iter().find(|e| e.kind == Kind::Registered).unwrap();
        assert_eq!((reg.ip.as_str(), reg.port), ("203.0.113.7", 6795));
        assert!(k.contains(&Kind::Teardown));
        assert!(!k.contains(&Kind::Fatal));
    }

    #[test]
    fn a_servers_settled_login_failure_is_no_failure() {
        let evs = kinds(include_str!("testdata/no-password.jsonl"));
        assert!(evs.iter().all(|e| e.kind != Kind::LoginFailed));
        assert!(evs.iter().any(|e| e.kind == Kind::NoCredentials));
        let refused = parse(r#"{"msg":"[NEVR.WS] login failed status=401 message_bytes=40"}"#);
        assert_eq!(refused.kind, Kind::LoginFailed);
        assert_eq!(refused.reason, Some(Reason::BadCredentials));
    }

    #[test]
    fn sees_serverdb_trouble_and_a_held_game_loop() {
        let evs = kinds(include_str!("testdata/serverdb-unreachable.jsonl"));
        assert!(evs.iter().any(|e| e.kind == Kind::ServerDbError));
        let evs = kinds(include_str!("testdata/wine11-crash.jsonl"));
        let held = evs.iter().find(|e| e.kind == Kind::Held).unwrap();
        assert_eq!(held.reason, Some(Reason::Crashed));
    }

    #[test]
    fn tells_fatal_errors_apart() {
        let f = |m: &str| parse(&format!("[NEVR.FATAL] {m}")).reason.unwrap();
        assert_eq!(f("ServerDB auth failed: HTTP 401"), Reason::BadCredentials);
        assert_eq!(
            f("registration rejected by ServerDB code=10"),
            Reason::UdpUnreachable
        );
        assert_eq!(
            f("registration rejected code=3"),
            Reason::RegistrationRejected
        );
        assert_eq!(
            f("no services.socket_uri in config.yaml — a dedicated server cannot log in"),
            Reason::InvalidConfig
        );
        assert_eq!(f("something else"), Reason::Fatal);
        assert!(!Reason::InvalidConfig.retryable());
        assert!(Reason::UdpUnreachable.retryable());
    }

    #[test]
    fn a_server_without_an_account_shows_a_code() {
        // nEVR 6.9.0 on echo (evr-docker's fixture; code and ids replaced).
        let evs = kinds(include_str!("testdata/v6.9.0-device-code.log"));
        let k: Vec<Kind> = evs.iter().map(|e| e.kind).collect();
        let code = evs.iter().find(|e| e.kind == Kind::SignInCode).unwrap();
        assert_eq!(code.url, "https://echovrce.com/device?code=TEST");
        assert_eq!(code.expires, 300);
        assert_eq!(
            code.line,
            "approve this server: https://echovrce.com/device?code=TEST"
        );
        assert_eq!(code_in(&code.url).as_deref(), Some("TEST"));
        let ended = evs.iter().find(|e| e.kind == Kind::SignInEnded).unwrap();
        assert!(ended.ok);
        let at = |kind| k.iter().position(|x| *x == kind).unwrap();
        assert!(at(Kind::SignInCode) < at(Kind::SignInEnded));
        assert!(at(Kind::SignInEnded) < at(Kind::Registered));
        assert!(is_noise(&clean(
            r#"2026-10-30T13:59:16.007Z info [NEVR.AUTH] poll #1 http=200 status=pending"#
        )));
        // The masked line in its log is no code.
        let masked = parse("[NEVR.AUTH] code shown url=https://echovrce.com/device?code=<4 chars masked> open_result=33 (shown)");
        assert_eq!(masked.kind, Kind::None);
        assert!(is_sign_in_code(
            r#"{"code":"AB12","event":"server_sign_in_code","expires_in_s":300,"msg":"x","tag":"NEVR.AUTH","url":"https://echovrce.com/device?code=AB12"}"#
        ));
        assert!(!is_sign_in_code(
            r#"{"event":"server_sign_in_code","tag":"NEVR.AUTH","url":"javascript:x"}"#
        ));
        assert!(is_logged("\u{1b}[90m2026-10-30T17:24:31.381Z\u{1b}[0m \u{1b}[0minfo\u{1b}[0m [NEVR.PATCH] shutdown signal received"));
        assert!(is_logged(
            r#"{"ts":"x","level":"info","msg":"[EVR] [NETGAME] NetGame switching state"}"#
        ));
        assert!(!is_logged(
            "Proton: Executable is inside wine prefix, launching normally."
        ));
        let failed = parse("[NEVR.AUTH] server sign-in ended: result=failed seconds=12");
        assert_eq!((failed.kind, failed.ok), (Kind::SignInEnded, false));
        let f = parse("[NEVR.FATAL] server login refused: device sign-in ended without a token");
        assert_eq!(f.reason, Some(Reason::SignIn));
        assert!(Reason::SignIn.retryable());
    }

    #[test]
    fn sessions_and_noise() {
        let s = parse("[NEVR.GAMESERVER] session starting session_id=abc-1 mode=arena");
        assert_eq!((s.kind, s.session.as_str()), (Kind::SessionStart, "abc-1"));
        let e = parse("[NEVR.GAMESERVER] session ended by ServerDB session_id=abc-1");
        assert_eq!(e.kind, Kind::SessionEnd);
        assert!(is_noise("[NEVR.SOCIAL] game->server conn=1"));
        assert!(!is_noise("[NEVR.WS] LOGIN SUCCESS"));
    }
}
