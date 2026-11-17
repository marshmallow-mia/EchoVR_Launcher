//! A server's own `_local/config.yaml` (nEVR's config): EchoVRCE's services, the server
//! account, where it registers, its tick rate and the mods it loads. The file is the
//! host's: at every start the Server host sets only the keys its page has, and keeps
//! every other line ([`crate::core::launcher::yaml_keys`]).

use std::path::Path;

use anyhow::Result;

use crate::core::launcher::host::quote;
use crate::core::launcher::nevr::{self, PluginLine};
use crate::core::launcher::yaml_keys::{self, Edit};

use super::{list, Config, Server, SignIn};

/// The environment variable the password reaches nEVR in: it is never written to a file.
pub const PASSWORD_ENV: &str = "NEVR_PASSWORD";

/// Pure: `uri` with the server's regions and guilds as its query (`regions=a,b`,
/// `guilds=…`), the way EchoVRCE's matchmaking reads them at sign-in. Other query
/// parameters stay.
pub fn with_matchmaking(uri: &str, regions: &[String], guilds: &[String]) -> String {
    let uri = uri.trim();
    // The address itself as it is (its port too): only the query changes.
    let (base, query) = uri.split_once('?').unwrap_or((uri, ""));
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        if k != "regions" && k != "guilds" {
            q.append_pair(&k, &v);
        }
    }
    if !guilds.is_empty() {
        q.append_pair("guilds", &guilds.join(","));
    }
    if !regions.is_empty() {
        q.append_pair("regions", &regions.join(","));
    }
    match q.finish() {
        q if q.is_empty() => base.to_string(),
        q => format!("{base}?{q}"),
    }
}

/// Pure: what is wrong with the services (nEVR stops a server at boot for it): `None`
/// when they are fine.
pub fn check_services(c: &Config) -> Option<String> {
    let s = &c.services;
    if s.socket_uri.trim().is_empty() {
        return Some("A server needs the login address (services.socket_uri).".into());
    }
    for (name, v) in [("Login", &s.socket_uri), ("Registration", &s.serverdb_uri)] {
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        match url::Url::parse(v) {
            Ok(u) if u.scheme() == "ws" || u.scheme() == "wss" => {}
            _ => return Some(format!("{name}: {v:?} is no ws:// or wss:// address.")),
        }
    }
    for (name, ip) in [
        ("Bind address", &c.internal_ip),
        ("Public address", &c.external_ip),
    ] {
        let ip = ip.trim();
        if !ip.is_empty() && ip.parse::<std::net::Ipv4Addr>().is_err() {
            return Some(format!("{name}: {ip:?} is no IPv4 address."));
        }
    }
    if s.http_uri.trim().is_empty() || s.http_key.trim().is_empty() {
        return Some(
            "A server needs the HTTP address and its key (auth.http_uri, http_key).".into(),
        );
    }
    http_problem(&s.http_uri)
}

/// Pure: what is wrong with `uri` as `auth.http_uri`: the server signs in there over HTTP,
/// so a ws:// or wss:// address (the login one, put there by mistake) makes it exit at
/// once. `None` when it is fine.
pub fn http_problem(uri: &str) -> Option<String> {
    let v = uri.trim();
    match url::Url::parse(v) {
        Ok(u) if (u.scheme() == "http" || u.scheme() == "https") && u.host_str().is_some() => None,
        Ok(u) if u.scheme() == "ws" || u.scheme() == "wss" => Some(format!(
            "HTTP: {v:?} is a websocket address (the login's): auth.http_uri needs the HTTP one, http:// or https://, e.g. {}.",
            super::HTTP_URI
        )),
        _ => Some(format!(
            "HTTP: {v:?} is no http:// or https:// address, e.g. {}.",
            super::HTTP_URI
        )),
    }
}

/// The line the Server host keeps at the top of a server's `config.yaml`.
pub const HEADER: &str =
    "# The Echo VR launcher's Server host sets the keys on its page here. Others you add stay.";

/// How the Server host's whole files started (up to 0.13): such a file was all its own.
const OLD_HEADERS: &[&str] = &[
    "# This game server's config, written by the Echo VR launcher",
    "# start: change the server on the launcher's Server host page",
];

/// Pure: the keys `s`'s `config.yaml` gets under `c`, loading `plugins`: every field the
/// page has, none where it is empty (the host's own value stays or comes back). A
/// password sign-in reads the password from [`PASSWORD_ENV`]; a device link signs in with
/// the server's `.credentials.json`.
pub fn edits(c: &Config, s: &Server, plugins: &[PluginLine], local: bool) -> Vec<Edit> {
    let (regions, guilds) = (list(&s.regions), list(&s.guilds));
    let sv = &c.services;
    let some = |v: &str| (!v.trim().is_empty()).then(|| quote(v.trim()));
    let flow = |l: &[String]| {
        let q: Vec<String> = l.iter().map(|r| quote(r)).collect();
        (!q.is_empty()).then(|| format!("[{}]", q.join(", ")))
    };
    vec![
        Edit::inline(
            &["services", "socket_uri"],
            quote(&with_matchmaking(&sv.socket_uri, &regions, &guilds)),
        ),
        Edit::maybe(&["services", "serverdb_uri"], some(&sv.serverdb_uri)),
        Edit::maybe(&["identity", "discord_id"], some(&c.account.discord_id)),
        Edit::maybe(&["auth", "http_uri"], some(&sv.http_uri)),
        Edit::maybe(&["auth", "http_key"], some(&sv.http_key)),
        Edit::maybe(&["auth", "server_key"], some(&sv.server_key)),
        // Read by nEVR from the environment (the supervisor sets it): never on disk.
        Edit::maybe(
            &["auth", "password"],
            (c.account.sign_in == SignIn::Password).then(|| format!("\"${{{PASSWORD_ENV}}}\"")),
        ),
        Edit::maybe(&["network", "regions"], flow(&regions)),
        Edit::maybe(&["network", "internal_ip"], some(&c.internal_ip)),
        Edit::maybe(&["network", "external_ip"], some(&c.external_ip)),
        Edit::inline(&["network", "tick_rate_hz"], s.tick_rate.to_string()),
        Edit::maybe(
            &["network", "empty_server_ttl_seconds"],
            (s.empty_ttl > 0).then(|| s.empty_ttl.to_string()),
        ),
        Edit::maybe(&["guilds"], flow(&guilds)),
        Edit::maybe(
            &[crate::core::launcher::mods::LOCAL_PLUGINS_KEY],
            local.then(|| "true".to_string()),
        ),
        Edit::set(&["plugins"], nevr::plugin_list(plugins)),
    ]
}

/// Sets `edits` in the server's `config.yaml` at `path` and keeps the rest. One the Server
/// host wrote whole (its old header) is started again.
pub fn write(path: &Path, edits: &[Edit]) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let old = text.lines().any(|l| {
        OLD_HEADERS
            .iter()
            .any(|h| l.trim_start_matches('\u{feff}').starts_with(h))
    });
    if old {
        let _ = std::fs::remove_file(yaml_keys::notes_path(path));
        yaml_keys::apply_from(path, "", edits, HEADER, OLD_HEADERS)?;
    } else {
        yaml_keys::apply(path, edits, HEADER, OLD_HEADERS)?;
    }
    Ok(())
}

/// Pure: the game's `_local/config.json` for the publisher lock (without one nEVR uses
/// `echotools`, a different version lock than EchoVRCE's).
pub fn game_config(publisher_lock: &str) -> String {
    serde_json::json!({ "publisher_lock": publisher_lock.trim() }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::server_host::{Account, Services};

    /// The keys set in an empty file.
    fn render(c: &Config, s: &Server, plugins: &[PluginLine], local: bool) -> String {
        yaml_keys::apply_to("", &edits(c, s, plugins, local), &Default::default()).0
    }

    fn config(sign_in: SignIn) -> Config {
        Config {
            account: Account {
                sign_in,
                discord_id: "123456789012345678".into(),
                ..Account::default()
            },
            services: Services::default(),
            ..Config::default()
        }
    }

    #[test]
    fn puts_matchmaking_into_the_login_address() {
        let r = |s: &str| vec![s.to_string()];
        assert_eq!(
            with_matchmaking("wss://g.echovrce.com:443/nevr", &r("euw"), &[]),
            "wss://g.echovrce.com:443/nevr?regions=euw"
        );
        assert_eq!(
            with_matchmaking(
                "ws://h:80/spr?format=evr&regions=old",
                &["euw".into(), "uwu".into()],
                &r("42")
            ),
            "ws://h:80/spr?format=evr&guilds=42&regions=euw%2Cuwu"
        );
        assert_eq!(
            with_matchmaking(" wss://g.echovrce.com:443/nevr?regions=x ", &[], &[]),
            "wss://g.echovrce.com:443/nevr"
        );
    }

    #[test]
    fn always_writes_the_login_address() {
        let mut s = Server {
            name: "echo-1".into(),
            regions: "uwu".into(),
            ..Server::default()
        };
        let text = render(&config(SignIn::Password), &s, &[], false);
        assert!(text.contains("services:\n  socket_uri: \"ws://g.echovrce.com:80/spr?regions=uwu\"\n  serverdb_uri: \"ws://g.echovrce.com:80/nevr\"\n"));
        assert!(text.contains("identity:\n  discord_id: \"123456789012345678\"\n"));
        assert!(text.contains("  password: \"${NEVR_PASSWORD}\"\n"));
        assert!(text.contains("network:\n  regions: [\"uwu\"]\n  tick_rate_hz: 120\n"));
        assert!(!text.contains("empty_server_ttl"));
        assert!(text.ends_with("plugins: []\n"));
        s.guilds = "1, 2".into();
        s.empty_ttl = 30;
        let text = render(&config(SignIn::Device), &s, &[], false);
        assert!(!text.contains("password"));
        assert!(text.contains("guilds: [\"1\", \"2\"]\n"));
        assert!(text.contains("  empty_server_ttl_seconds: 30\n"));
    }

    #[test]
    fn keeps_the_hosts_own_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let mut c = config(SignIn::Device);
        c.external_ip = "203.0.113.7".into();
        let s = Server::default();
        let read = || std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            "# mine\nnetwork:\n  foo: 1\n  internal_ip: \"10.0.0.2\"\n",
        )
        .unwrap();
        write(&path, &edits(&c, &s, &[], false)).unwrap();
        let t = read();
        assert!(
            t.starts_with(&format!("{HEADER}\n# mine\nnetwork:\n  foo: 1\n")),
            "{t}"
        );
        // An empty field on the page: the host's own value stays.
        assert!(t.contains("  internal_ip: \"10.0.0.2\"\n"));
        assert!(t.contains("  external_ip: \"203.0.113.7\"\n"));
        // Cleared on the page: the page's goes.
        c.external_ip.clear();
        write(&path, &edits(&c, &s, &[], false)).unwrap();
        let t = read();
        assert!(!t.contains("external_ip") && t.contains("internal_ip"));
        // A file the Server host wrote whole (up to 0.13): started again.
        std::fs::write(
            &path,
            "# This game server's config, written by the Echo VR launcher (Server host) at every\n\
             # start: change the server on the launcher's Server host page, not here.\n\
             services:\n  socket_uri: \"x\"\nnetwork:\n  external_ip: \"198.51.100.1\"\n",
        )
        .unwrap();
        write(&path, &edits(&c, &s, &[], false)).unwrap();
        let t = read();
        assert!(t.starts_with(&format!("{HEADER}\nservices:\n")), "{t}");
        assert!(!t.contains("198.51.100.1") && !t.contains("written by"));
    }

    #[test]
    fn checks_the_services() {
        let mut c = config(SignIn::Password);
        assert_eq!(check_services(&c), None);
        c.services.socket_uri = " ".into();
        assert!(check_services(&c).is_some());
        c.services.socket_uri = "https://g.echovrce.com".into();
        assert!(check_services(&c).unwrap().contains("ws://"));
        c.services.socket_uri = SOCKET.into();
        c.services.serverdb_uri = String::new();
        assert_eq!(check_services(&c), None);
        c.external_ip = "example.com".into();
        assert!(check_services(&c).unwrap().contains("IPv4"));
        c.external_ip = "203.0.113.7".into();
        assert_eq!(check_services(&c), None);
        let text = render(&c, &Server::default(), &[], false);
        assert!(text.contains("network:\n  external_ip: \"203.0.113.7\"\n  tick_rate_hz: 120\n"));
    }

    const SOCKET: &str = crate::core::server_host::SOCKET_URI;

    #[test]
    fn locks_the_publisher() {
        assert_eq!(game_config("echovrce"), r#"{"publisher_lock":"echovrce"}"#);
    }
}
