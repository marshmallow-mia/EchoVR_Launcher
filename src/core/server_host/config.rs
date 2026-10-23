//! A server's own `_local/config.yaml` (nEVR's config): EchoVRCE's services, the server
//! account, where it registers, its tick rate and the mods it loads. Written at every
//! start: change a server on the page, not in the file.

use crate::core::launcher::host::quote;
use crate::core::launcher::nevr::{self, PluginLine};

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
    None
}

/// Pure: `s`'s `config.yaml` under `c`, loading `plugins`. A password sign-in reads the
/// password from [`PASSWORD_ENV`]; a device link signs in with the server's
/// `.credentials.json`.
pub fn render(c: &Config, s: &Server, plugins: &[PluginLine], local: bool) -> String {
    let mut out = String::from(
        "# This game server's config, written by the Echo VR launcher (Server host) at every\n\
         # start: change the server on the launcher's Server host page, not here.\n",
    );
    let (regions, guilds) = (list(&s.regions), list(&s.guilds));
    let sv = &c.services;
    out.push_str("services:\n");
    out.push_str(&format!(
        "  socket_uri: {}\n",
        quote(&with_matchmaking(&sv.socket_uri, &regions, &guilds))
    ));
    if !sv.serverdb_uri.trim().is_empty() {
        out.push_str(&format!(
            "  serverdb_uri: {}\n",
            quote(sv.serverdb_uri.trim())
        ));
    }
    if !c.account.discord_id.trim().is_empty() {
        out.push_str(&format!(
            "identity:\n  discord_id: {}\n",
            quote(c.account.discord_id.trim())
        ));
    }
    out.push_str("auth:\n");
    for (k, v) in [
        ("http_uri", sv.http_uri.trim()),
        ("http_key", sv.http_key.trim()),
        ("server_key", sv.server_key.trim()),
    ] {
        if !v.is_empty() {
            out.push_str(&format!("  {k}: {}\n", quote(v)));
        }
    }
    if c.account.sign_in == SignIn::Password {
        // Read by nEVR from the environment (the supervisor sets it): never on disk.
        out.push_str(&format!("  password: \"${{{PASSWORD_ENV}}}\"\n"));
    }
    out.push_str("network:\n");
    if !regions.is_empty() {
        let r: Vec<String> = regions.iter().map(|r| quote(r)).collect();
        out.push_str(&format!("  regions: [{}]\n", r.join(", ")));
    }
    for (k, v) in [
        ("internal_ip", &c.internal_ip),
        ("external_ip", &c.external_ip),
    ] {
        if !v.trim().is_empty() {
            out.push_str(&format!("  {k}: {}\n", quote(v.trim())));
        }
    }
    out.push_str(&format!("  tick_rate_hz: {}\n", s.tick_rate));
    if s.empty_ttl > 0 {
        out.push_str(&format!("  empty_server_ttl_seconds: {}\n", s.empty_ttl));
    }
    if !guilds.is_empty() {
        let g: Vec<String> = guilds.iter().map(|g| quote(g)).collect();
        out.push_str(&format!("guilds: [{}]\n", g.join(", ")));
    }
    out.push_str(&nevr::render_plugin_list(plugins, local));
    out
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
        assert!(text.contains("services:\n  socket_uri: \"wss://g.echovrce.com:443/nevr?regions=uwu\"\n  serverdb_uri: \"ws://g.echovrce.com:80/nevr\"\n"));
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
