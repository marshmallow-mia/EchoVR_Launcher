//! What a launcher plugin can do, as the player is asked to allow it: read from its
//! description (the servers its requests go to, the launcher's things its steps use). The
//! player allows it when getting the plugin, again when an update can do more, and on its
//! page when its files changed or it was put there by hand; until then it does nothing.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::plugin_page::{PluginPage, Step};

/// What a plugin can do (or was allowed to).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    /// The servers its description names (`scheme://host:port`).
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub servers: BTreeSet<String>,
    /// Servers the player types in its settings.
    #[serde(skip_serializing_if = "is_false")]
    pub typed_servers: bool,
    /// The classic lobbies server the launcher's settings name.
    #[serde(skip_serializing_if = "is_false")]
    pub relay: bool,
    /// The player's classic lobbies password, sent to that server only.
    #[serde(skip_serializing_if = "is_false")]
    pub relay_password: bool,
    /// Sets the server Echo VR plays on (the launcher asks every time).
    #[serde(skip_serializing_if = "is_false")]
    pub game_server: bool,
    /// Writes a game server's setup into Echo VR's folder (the launcher asks every time).
    #[serde(skip_serializing_if = "is_false")]
    pub host: bool,
    /// Starts Echo VR.
    #[serde(skip_serializing_if = "is_false")]
    pub play: bool,
    /// Keeps EchoXR's settings beside the game in every version.
    #[serde(skip_serializing_if = "is_false")]
    pub echoxr: bool,
    /// A page the launcher draws for it ([`super::plugin_page::BUILTIN`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub builtin: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Permissions {
    /// Pure: what plugin `page` can do.
    pub fn of(page: &PluginPage) -> Permissions {
        let mut p = Permissions {
            echoxr: page
                .settings
                .as_ref()
                .is_some_and(super::echoxr_settings::is_echoxr),
            builtin: page.builtin.clone(),
            ..Default::default()
        };
        // Every template its steps fill in.
        let mut templates: Vec<&str> = Vec::new();
        let requests =
            page.data
                .iter()
                .map(|s| &s.request)
                .chain(page.actions.values().flatten().filter_map(|s| match s {
                    Step::Request { request, .. } => Some(request),
                    _ => None,
                }));
        for r in requests {
            servers_of(&r.url, &mut p);
            templates.push(&r.url);
            if let Some(b) = &r.body {
                strings(b, &mut templates);
            }
        }
        for step in page.actions.values().flatten() {
            match step {
                Step::Play { .. } => p.play = true,
                Step::Server {
                    name,
                    address,
                    key,
                    discord_id,
                    display_name,
                    password,
                    ..
                } => {
                    // `{"server": null}` only goes back to EchoVRCE.
                    if !address.is_empty() {
                        p.game_server = true;
                    }
                    templates.extend(
                        [name, address, key, discord_id, display_name, password]
                            .map(String::as_str),
                    );
                }
                Step::Host { fields, .. } => {
                    p.host = true;
                    templates.extend(fields.values().map(String::as_str));
                }
                _ => {}
            }
        }
        p.relay_password = templates
            .iter()
            .any(|t| t.contains("launcher.relay.password"));
        p
    }

    pub fn is_empty(&self) -> bool {
        *self == Permissions::default()
    }

    /// Pure: what of this `granted` doesn't cover.
    pub fn missing(&self, granted: &Permissions) -> Permissions {
        Permissions {
            servers: self.servers.difference(&granted.servers).cloned().collect(),
            typed_servers: self.typed_servers && !granted.typed_servers,
            relay: self.relay && !granted.relay,
            relay_password: self.relay_password && !granted.relay_password,
            game_server: self.game_server && !granted.game_server,
            host: self.host && !granted.host,
            play: self.play && !granted.play,
            echoxr: self.echoxr && !granted.echoxr,
            builtin: if self.builtin == granted.builtin {
                String::new()
            } else {
                self.builtin.clone()
            },
        }
    }

    /// Pure: this and `other` together.
    pub fn with(&self, other: &Permissions) -> Permissions {
        Permissions {
            servers: self.servers.union(&other.servers).cloned().collect(),
            typed_servers: self.typed_servers || other.typed_servers,
            relay: self.relay || other.relay,
            relay_password: self.relay_password || other.relay_password,
            game_server: self.game_server || other.game_server,
            host: self.host || other.host,
            play: self.play || other.play,
            echoxr: self.echoxr || other.echoxr,
            builtin: if other.builtin.is_empty() {
                self.builtin.clone()
            } else {
                other.builtin.clone()
            },
        }
    }

    /// Pure: what it can do, a line each, for the player.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.builtin == "server-host" {
            out.push(
                "Run game servers on this PC: start and stop them, open their ports in the firewall, and link an EchoVRCE account for them"
                    .to_string(),
            );
        }
        if !self.servers.is_empty() {
            let names: Vec<String> = self.servers.iter().map(|o| shown(o)).collect();
            out.push(format!("Talk to {}", names.join(", ")));
        }
        if self.relay {
            out.push("Talk to the classic lobbies server your settings name".into());
        }
        if self.typed_servers {
            out.push("Talk to servers you type in its settings".into());
        }
        if self.relay_password {
            out.push(
                "Use your classic lobbies password (it goes to the classic lobbies server only)"
                    .into(),
            );
        }
        if self.game_server {
            out.push("Change the server Echo VR plays on (it asks you every time)".into());
        }
        if self.host {
            out.push("Set up a game server in Echo VR's folder (it asks you every time)".into());
        }
        if self.echoxr {
            out.push("Change EchoXR's settings in every installed version".into());
        }
        if self.play {
            out.push("Start Echo VR".into());
        }
        out
    }
}

/// Pure: `scheme://host:port` as the player reads it: the host, and the port when it
/// isn't the scheme's own; `http://` when it isn't encrypted.
fn shown(origin: &str) -> String {
    let (scheme, rest) = origin.split_once("://").unwrap_or(("", origin));
    let rest = match scheme {
        "https" => rest.strip_suffix(":443").unwrap_or(rest),
        "http" => rest.strip_suffix(":80").unwrap_or(rest),
        _ => rest,
    };
    if scheme == "http" {
        format!("http://{rest}")
    } else {
        rest.to_string()
    }
}

/// Every string in a JSON body (its templates).
fn strings<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|v| strings(v, out)),
        Value::Object(m) => m.values().for_each(|v| strings(v, out)),
        _ => {}
    }
}

/// Where a request's template `url` can go, into `p`: the server it names, or where its
/// `{…}` server comes from. A server from anywhere else (an answer, the page) is never sent
/// to ([`super::plugins::request_url`]), so it needs nothing.
fn servers_of(url: &str, p: &mut Permissions) {
    let url = url.trim();
    let (scheme, after) = match url.split_once("://") {
        Some((s, a)) if !s.contains('{') => (s, a),
        // All of it from one value: `{settings.url}/x`.
        _ => {
            if let Some(expr) = url.strip_prefix('{').and_then(|r| r.split_once('}')) {
                from_expr(expr.0, None, p);
            }
            return;
        }
    };
    // Its server: up to the first `/`, `?` or `#` outside `{…}`.
    let mut depth = 0;
    let end = after
        .char_indices()
        .find(|&(_, c)| {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                '/' | '?' | '#' if depth == 0 => return true,
                _ => {}
            }
            false
        })
        .map_or(after.len(), |(i, _)| i);
    let authority = &after[..end];
    if !authority.contains('{') {
        if let Some(o) = super::plugins::origin(&format!("{scheme}://{authority}")) {
            p.servers.insert(o);
        }
        return;
    }
    let mut rest = authority;
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}') else { break };
        from_expr(&rest[i + 1..i + j], Some((scheme, authority)), p);
        rest = &rest[i + j + 1..];
    }
}

/// One `{a|b|'c'}` of a request's server (`scheme` and the whole server part for a
/// literal), into `p`.
fn from_expr(expr: &str, server: Option<(&str, &str)>, p: &mut Permissions) {
    for alt in expr.split('|').map(str::trim) {
        if alt.starts_with("settings.") {
            p.typed_servers = true;
        } else if alt == "launcher.relay.server" {
            p.relay = true;
        } else if let (Some(lit), Some((scheme, authority))) = (
            alt.strip_prefix('\'').and_then(|a| a.strip_suffix('\'')),
            server,
        ) {
            let filled = authority.replace(&format!("{{{expr}}}"), lit);
            if let Some(o) = super::plugins::origin(&format!("{scheme}://{filled}")) {
                p.servers.insert(o);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::launcher::plugins::read_description;

    fn of(text: &str) -> Permissions {
        Permissions::of(&read_description(text).0.expect("a plugin"))
    }

    #[test]
    fn the_shipped_plugins_permissions() {
        let lobbies = of(include_str!(
            "../../../docs/plugins/event-lobbies/plugin.json"
        ));
        assert_eq!(
            lobbies,
            Permissions {
                typed_servers: true,
                relay: true,
                relay_password: true,
                play: true,
                ..Default::default()
            }
        );
        let controllers = of(include_str!(
            "../../../docs/plugins/controllers/plugin.json"
        ));
        assert_eq!(
            controllers,
            Permissions {
                echoxr: true,
                ..Default::default()
            }
        );
        let game_server = of(include_str!(
            "../../../docs/plugins/game-server/plugin.json"
        ));
        assert!(game_server.game_server && game_server.relay_password);
        assert!(game_server.servers.is_empty() && !game_server.typed_servers);
        let host = of(include_str!(
            "../../../docs/plugins/server-host/plugin.json"
        ));
        assert_eq!(host.builtin, "server-host");
        assert_eq!(host.lines().len(), 1);
    }

    #[test]
    fn servers_from_the_description_settings_and_the_launcher() {
        let mut p = Permissions::default();
        servers_of("https://API.example.com/v1/{page.q}", &mut p);
        servers_of("http://{settings.host|'lobby.example.com:8080'}/x", &mut p);
        servers_of(
            "https://{settings.host}.example.com:{settings.port}/",
            &mut p,
        );
        servers_of("http://{launcher.relay.server}/api", &mut p);
        servers_of("{settings.url}/api", &mut p);
        // From an answer: never sent to, so nothing.
        servers_of("https://{data.next.host}/x", &mut p);
        assert_eq!(
            p.servers.iter().collect::<Vec<_>>(),
            [
                "http://lobby.example.com:8080",
                "https://api.example.com:443"
            ]
        );
        assert!(p.typed_servers && p.relay);
        assert_eq!(
            p.lines(),
            [
                "Talk to http://lobby.example.com:8080, api.example.com",
                "Talk to the classic lobbies server your settings name",
                "Talk to servers you type in its settings",
            ]
        );
    }

    #[test]
    fn what_an_update_adds() {
        let before = Permissions {
            servers: ["https://a.example:443".to_string()].into(),
            relay: true,
            ..Default::default()
        };
        let after = Permissions {
            servers: [
                "https://a.example:443".to_string(),
                "https://b.example:443".to_string(),
            ]
            .into(),
            relay: true,
            play: true,
            ..Default::default()
        };
        let added = after.missing(&before);
        assert_eq!(added.servers.len(), 1);
        assert!(added.play && !added.relay);
        assert!(before.missing(&after).is_empty());
        assert!(after.missing(&before.with(&added)).is_empty());
        // Kept as the player allowed it, read back the same.
        let text = serde_json::to_string(&after).unwrap();
        assert_eq!(serde_json::from_str::<Permissions>(&text).unwrap(), after);
        assert_eq!(
            serde_json::to_string(&Permissions::default()).unwrap(),
            "{}"
        );
    }
}
