//! The launcher's services for privileged plugins (`permissions.services`, signed by the
//! launcher's makers only): `server-host`, game servers on this PC (`core::server_host`).
//!
//! [`call`] answers at once, on the plugin's thread: it reads and writes the server host's
//! files. [`STARTS`] are the operations that take a while or need the launcher itself (an
//! EchoVRCE sign-in, a password typed into the plugin's secret field, the administrator's
//! consent): the plugin's page runs them (`ui::launcher::plugin_services`) and the answer
//! goes back as `service-done`. No answer holds a password, a sign-in or the webhook.
//!
//! The operations and their JSON: `docs/plugins/v2.md`.

use serde_json::{json, Value};

use crate::core::echovrce;
use crate::core::server_host::events::Reason;
use crate::core::server_host::status::{self, Command, State, Status};
use crate::core::server_host::{self as host, autostart, Config, Server, SignIn};

/// The operations `server-host.start` takes.
pub const STARTS: &[&str] = &[
    "running",
    "link-mine",
    "link-code",
    "link-wait",
    "password",
    "test",
    "registered",
    "doctor",
    "fix",
    "webhook",
];

/// A `server-host.call`: its answer.
pub fn call(op: &str, args: &Value) -> Result<Value, String> {
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let name = s("name");
    match op {
        "state" => Ok(state(&Config::load(), echovrce::now())),
        "defaults" => Ok(services_json(&host::Services::default())),
        "add" => {
            let n = args.get("n").and_then(Value::as_u64).unwrap_or(1) as usize;
            let mut c = Config::load();
            let names = c
                .add(n.clamp(1, host::MAX_SERVERS))
                .map_err(|e| format!("{e:#}"))?;
            save(&c)?;
            Ok(json!(names))
        }
        "delete" => {
            let mut c = Config::load();
            if c.server(&name).is_none() {
                return Err(format!("no server {name:?}"));
            }
            delete(&mut c, &name)?;
            Ok(Value::Null)
        }
        "set-run" => {
            let run = args.get("run").and_then(Value::as_bool).unwrap_or(false);
            let mut c = Config::load();
            let names: Vec<String> = if name == "*" {
                c.servers.iter().map(|s| s.name.clone()).collect()
            } else {
                vec![name.clone()]
            };
            if run && c.account.sign_in == SignIn::None {
                return Err("Sign the servers in first: they need an account of their own on EchoVRCE (Account).".into());
            }
            for n in &names {
                match c.server_mut(n) {
                    Some(sv) => sv.run = run,
                    None => return Err(format!("no server {n:?}")),
                }
            }
            save(&c)?;
            if run {
                host::supervisor::ensure_running();
            }
            Ok(Value::Null)
        }
        "start-host" => {
            host::supervisor::ensure_running();
            Ok(Value::Null)
        }
        "command" => {
            let cmd = match s("what").as_str() {
                "apply" => Command::Apply,
                "restart" => Command::Restart,
                "stop-now" => Command::StopNow,
                other => return Err(format!("no command {other:?}")),
            };
            if Config::load().server(&name).is_none() {
                return Err(format!("no server {name:?}"));
            }
            status::ask(&host::server_dir(&name), cmd).map_err(|e| format!("{e:#}"))?;
            Ok(Value::Null)
        }
        "edit" => {
            let mut c = Config::load();
            edit(&mut c, &name, args)?;
            save(&c)?;
            let _ = status::ask(&host::server_dir(&name), Command::Apply);
            Ok(Value::Null)
        }
        "settings" => {
            let mut c = Config::load();
            settings(&mut c, args)?;
            save(&c)?;
            restart_idle(&c);
            Ok(Value::Null)
        }
        "autostart" => {
            let on = args.get("on").and_then(Value::as_bool).unwrap_or(false);
            autostart::set(on).map_err(|e| format!("{e:#}"))?;
            Ok(json!(autostart::on()))
        }
        "alerts" => {
            let mut c = Config::load();
            c.alerts.desktop = args.get("desktop").and_then(Value::as_bool).unwrap_or(true);
            save(&c)?;
            Ok(Value::Null)
        }
        "sign-out" => {
            let mut c = Config::load();
            host::account::forget(&mut c).map_err(|e| format!("{e:#}"))?;
            save(&c)?;
            Ok(Value::Null)
        }
        "open-approve" => {
            let code = s("code");
            if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err("no code".into());
            }
            open::that(echovrce::approve_url(&code)).map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "open-folder" => {
            let dir = if name.is_empty() {
                host::dir()
            } else if Config::load().server(&name).is_some() {
                host::server_dir(&name)
            } else {
                return Err(format!("no server {name:?}"));
            };
            let _ = std::fs::create_dir_all(&dir);
            open::that(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            Ok(Value::Null)
        }
        other => Err(format!("no operation {other:?}")),
    }
}

fn save(c: &Config) -> Result<(), String> {
    c.save().map_err(|e| format!("{e:#}"))
}

/// After the account or the services changed: the idle servers start again with them.
pub fn restart_idle(c: &Config) {
    for s in c.servers.iter().filter(|s| s.run) {
        let _ = status::ask(&host::server_dir(&s.name), Command::Apply);
    }
}

/// Deletes a server: it stops, and its folder (game folder, logs, prefix) goes once it has.
fn delete(c: &mut Config, name: &str) -> Result<(), String> {
    let dir = host::server_dir(name);
    let _ = status::ask(&dir, Command::StopNow);
    c.servers.retain(|s| s.name != name);
    save(c)?;
    let name = name.to_string();
    std::thread::spawn(move || {
        // Until the server host let go of it (at most a minute).
        for _ in 0..60 {
            let now = echovrce::now();
            let stopped =
                status::read(&dir).is_none_or(|s| s.state == State::Stopped || !s.fresh(now));
            if stopped {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!("server host: removing {}: {e}", dir.display());
        }
        let compat = crate::core::linux::echoxr::server_compat_data(&format!("host-{name}"));
        if cfg!(target_os = "linux") && compat.is_dir() {
            let _ = std::fs::remove_dir_all(compat);
        }
    });
    Ok(())
}

fn text(args: &Value, k: &str) -> Option<String> {
    args.get(k).map(|v| match v {
        Value::String(s) => s.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

fn number(args: &Value, k: &str, what: &str) -> Result<Option<u32>, String> {
    match text(args, k) {
        None => Ok(None),
        Some(t) => t
            .parse::<u32>()
            .map(Some)
            .map_err(|_| format!("{what}: a number, please.")),
    }
}

/// Pure: server `name` of `c` with what `args` changes (`port`, `regions`, `guilds`,
/// `tick_rate`, `empty_ttl`, `mods`, `extra_args`), checked against the others.
pub fn edit(c: &mut Config, name: &str, args: &Value) -> Result<(), String> {
    let Some(old) = c.server(name).cloned() else {
        return Err("This server is gone.".into());
    };
    let port = match number(args, "port", "Port")? {
        Some(p) => u16::try_from(p).map_err(|_| "Port: up to 65535.".to_string())?,
        None => old.port,
    };
    let s = Server {
        port,
        regions: text(args, "regions").unwrap_or(old.regions.clone()),
        guilds: text(args, "guilds").unwrap_or(old.guilds.clone()),
        tick_rate: number(args, "tick_rate", "Tick rate")?.unwrap_or(old.tick_rate),
        empty_ttl: number(args, "empty_ttl", "Empty server")?.unwrap_or(old.empty_ttl),
        mods: args
            .get("mods")
            .and_then(Value::as_bool)
            .unwrap_or(old.mods),
        extra_args: text(args, "extra_args").unwrap_or(old.extra_args.clone()),
        ..old
    };
    if let Some(why) = host::check_server(&s, &c.servers) {
        return Err(why);
    }
    if let Some(slot) = c.server_mut(name) {
        *slot = s;
    }
    Ok(())
}

/// Pure: `c` with the settings in `args` (the version, the defaults for new servers, the
/// services, the publisher lock, the addresses), checked.
pub fn settings(c: &mut Config, args: &Value) -> Result<(), String> {
    let mut n = c.clone();
    let set = |v: &mut String, k: &str| {
        if let Some(t) = text(args, k) {
            *v = t;
        }
    };
    set(&mut n.version, "version");
    set(&mut n.defaults.regions, "regions");
    set(&mut n.defaults.guilds, "guilds");
    if let Some(t) = number(args, "tick_rate", "Tick rate")? {
        n.defaults.tick_rate = t;
    }
    let sv = &mut n.services;
    set(&mut sv.socket_uri, "socket_uri");
    set(&mut sv.serverdb_uri, "serverdb_uri");
    set(&mut sv.http_uri, "http_uri");
    set(&mut sv.http_key, "http_key");
    set(&mut sv.server_key, "server_key");
    set(&mut n.publisher_lock, "publisher_lock");
    set(&mut n.internal_ip, "internal_ip");
    set(&mut n.external_ip, "external_ip");
    if let Some(why) = host::config::check_services(&n) {
        return Err(why);
    }
    if !(10..=1000).contains(&n.defaults.tick_rate) {
        return Err("Tick rate: 10 to 1000.".into());
    }
    *c = n;
    Ok(())
}

fn services_json(s: &host::Services) -> Value {
    json!({
        "socket_uri": s.socket_uri,
        "serverdb_uri": s.serverdb_uri,
        "http_uri": s.http_uri,
        "http_key": s.http_key,
        "server_key": s.server_key,
    })
}

/// Pure: how long, in words ("3 min", "2 h 5 min").
pub fn span(secs: i64) -> String {
    let m = secs.max(0) / 60;
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h {} min", m / 60, m % 60)
    }
}

/// Pure: a server's row as the page shows it at `now`: its chip and tone, its line (port,
/// regions, nEVR, guilds) and what it last did.
pub fn row(s: &Server, st: Option<&Status>, now: i64) -> Value {
    let summary = st.map_or_else(|| "Stopped".to_string(), |st| st.summary(now));
    let chip = match st {
        Some(st) if !s.run && st.state != State::Stopped && st.fresh(now) => "Stopping".to_string(),
        _ => summary.split(':').next().unwrap_or("").to_string(),
    };
    let tone = match st {
        Some(st) if !st.fresh(now) && st.state != State::Stopped => "muted",
        Some(st) => match st.state {
            State::Registered => "good",
            State::InSession => "normal",
            State::Starting | State::Stopping | State::Backoff => "warn",
            State::Failed => "bad",
            State::Stopped => "muted",
        },
        None => "muted",
    };
    let mut line = vec![format!("Port {}", s.port)];
    let regions = host::list(&s.regions);
    line.push(if regions.is_empty() {
        "default region".into()
    } else {
        regions.join(", ")
    });
    if let Some(v) = st.map(|st| st.runtime.as_str()).filter(|v| !v.is_empty()) {
        line.push(format!("nEVR {v}"));
    }
    if !host::list(&s.guilds).is_empty() {
        line.push(format!("{} guild(s)", host::list(&s.guilds).len()));
    }
    let mut detail = match st {
        Some(st) if st.state == State::Registered || st.state == State::InSession => {
            format!("Registered at {}:{}", st.public_ip, st.port)
        }
        Some(st) if matches!(st.state, State::Failed | State::Backoff) => {
            let why = st.reason.map_or("It stopped on an error", Reason::words);
            match st.next_start - now {
                n if n > 0 => format!("{why} (again in {n} s)"),
                _ => why.to_string(),
            }
        }
        _ => String::new(),
    };
    let alive = st.is_some_and(|st| st.fresh(now) && st.state != State::Stopped);
    if let Some(st) = st.filter(|_| alive) {
        let up = format!(
            "{} matches · {} restarts · up {}",
            st.matches,
            st.restarts,
            span(now - st.since)
        );
        let failing = matches!(st.state, State::Failed | State::Backoff);
        detail = match (detail.is_empty(), failing) {
            (true, _) => up,
            (false, true) => detail,
            (false, false) => format!("{detail} · {up}"),
        };
    }
    json!({
        "name": s.name,
        "run": s.run,
        "port": s.port,
        "regions": s.regions,
        "guilds": s.guilds,
        "tick_rate": s.tick_rate,
        "empty_ttl": s.empty_ttl,
        "mods": s.mods,
        "extra_args": s.extra_args,
        "chip": chip,
        "tone": tone,
        "line": line.join(" · "),
        "detail": detail,
        "alive": alive,
        "in_match": st.is_some_and(|st| st.state == State::InSession && st.fresh(now)),
    })
}

/// Pure: what the page shows of `c` at `now` (the servers as their status files say).
pub fn state(c: &Config, now: i64) -> Value {
    let servers: Vec<Value> = c
        .servers
        .iter()
        .map(|s| row(s, status::read(&host::server_dir(&s.name)).as_ref(), now))
        .collect();
    let a = &c.account;
    let who = if a.username.is_empty() {
        a.discord_id.clone()
    } else {
        a.username.clone()
    };
    json!({
        "servers": servers,
        "max_servers": host::MAX_SERVERS,
        "first_port": host::FIRST_PORT,
        "account": {
            "sign_in": match a.sign_in {
                SignIn::None => "none",
                SignIn::Device => "device",
                SignIn::Password => "password",
            },
            "who": who,
        },
        "version": c.version,
        "defaults": {
            "regions": c.defaults.regions,
            "guilds": c.defaults.guilds,
            "tick_rate": c.defaults.tick_rate,
        },
        "services": services_json(&c.services),
        "publisher_lock": c.publisher_lock,
        "internal_ip": c.internal_ip,
        "external_ip": c.external_ip,
        "alerts": {
            "desktop": c.alerts.desktop,
            "webhook": !c.alerts.webhook.trim().is_empty(),
        },
        "autostart": autostart::on(),
        "platform": std::env::consts::OS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        let mut c = Config::default();
        c.add(2).unwrap();
        c
    }

    #[test]
    fn edits_are_checked() {
        let mut c = cfg();
        let (a, b) = (c.servers[0].name.clone(), c.servers[1].port);
        assert!(edit(&mut c, &a, &json!({"port": b.to_string()})).is_err());
        assert_eq!(
            edit(&mut c, &a, &json!({"port": "x"})).unwrap_err(),
            "Port: a number, please."
        );
        edit(
            &mut c,
            &a,
            &json!({"port": "7000", "regions": " uwu ", "mods": false}),
        )
        .unwrap();
        let s = c.server(&a).unwrap();
        assert_eq!((s.port, s.regions.as_str(), s.mods), (7000, "uwu", false));
        assert!(edit(&mut c, "nope", &json!({})).is_err());
    }

    #[test]
    fn settings_need_an_http_address_for_the_http_uri() {
        let mut c = cfg();
        // The mistake a player made: the login address in auth.http_uri.
        let e = settings(
            &mut c,
            &json!({"http_uri": "wss://g.echovrce.com:443/nevr"}),
        )
        .unwrap_err();
        assert!(e.contains("http://") && e.contains("https://"), "{e}");
        assert_eq!(c.services.http_uri, host::HTTP_URI);
        settings(
            &mut c,
            &json!({"http_uri": "https://g.echovrce.com:7350", "tick_rate": "60"}),
        )
        .unwrap();
        assert_eq!(c.defaults.tick_rate, 60);
        assert!(settings(&mut c, &json!({"tick_rate": "5"})).is_err());
    }

    #[test]
    fn a_row_says_what_a_server_does() {
        let mut c = cfg();
        c.servers[0].run = true;
        let s = &c.servers[0];
        let r = row(s, None, 1000);
        assert_eq!(r["chip"], "Stopped");
        assert_eq!(r["tone"], "muted");
        assert_eq!(r["alive"], false);
        let st = Status {
            name: s.name.clone(),
            state: State::Registered,
            since: 1000 - 3 * 60,
            updated: 1000,
            port: s.port,
            public_ip: "1.2.3.4".into(),
            runtime: "5.4.3".into(),
            matches: 2,
            ..Default::default()
        };
        let r = row(s, Some(&st), 1000);
        assert_eq!(r["chip"], "Ready");
        assert_eq!(r["tone"], "good");
        assert!(r["line"].as_str().unwrap().contains("nEVR 5.4.3"));
        let detail = r["detail"].as_str().unwrap();
        assert!(
            detail.starts_with(&format!("Registered at 1.2.3.4:{}", s.port)),
            "{detail}"
        );
        assert!(
            detail.contains("2 matches") && detail.contains("up 3 min"),
            "{detail}"
        );
    }

    #[test]
    fn unknown_operations_are_refused() {
        assert!(call("rm -rf", &Value::Null).is_err());
    }
}
