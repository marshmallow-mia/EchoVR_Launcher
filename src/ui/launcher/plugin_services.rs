//! The services a privileged plugin's `server-host.start` runs in the launcher
//! (`core::plugin_host::services::STARTS`): what needs the launcher itself (its EchoVRCE
//! sign-in, a password typed into the plugin's secret field, the administrator's consent)
//! or takes a while. Each runs on a thread of its own; the answer goes back to the plugin
//! as `service-done`. No answer holds a password, a sign-in or the webhook.

use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::{json, Value};

use super::{setup, Dashboard, Page};
use crate::core::echovrce::{self, Poll};
use crate::core::launcher::plugins::Installed;
use crate::core::plugin_host::{services, Cmd, UiEvent};
use crate::core::server_host::doctor::{self, Fix, Level};
use crate::core::server_host::status::State;
use crate::core::server_host::{self as host, account, Config, SignIn};

/// Pure: what the servers may host for, in words.
pub(super) fn guild_note(guilds: &[String]) -> String {
    if guilds.is_empty() {
        "Signed in. No guild gave this account its server host role: the servers host public matches.".into()
    } else {
        format!(
            "Signed in. It may host for {} guild(s): {}.",
            guilds.len(),
            guilds.join(", ")
        )
    }
}

/// Runs operation `op` for plugin `p` (its call's `ticket`).
pub(super) fn start(
    d: &mut Dashboard,
    ctx: &egui::Context,
    p: &Installed,
    ticket: u32,
    op: &str,
    args: Value,
) {
    let Some(tx) = d.plugins.code.get(&p.page.id).and_then(|v| v.sender()) else {
        return;
    };
    let wake = ctx.clone();
    let answer = move |r: Result<Value, String>| {
        let _ = tx.send(Cmd::Event(UiEvent::ServiceDone(
            ticket,
            r.map(|v| v.to_string()),
        )));
        wake.request_repaint();
    };
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    // A secret field's text: it stays in the launcher, and goes once used.
    let mut secret = |k: &str| -> String {
        let field = s(k);
        d.plugins
            .code
            .get_mut(&p.page.id)
            .and_then(|v| v.secrets.remove(&field))
            .unwrap_or_default()
    };
    match op {
        "running" => {
            let demo = d.demo;
            std::thread::spawn(move || {
                answer(Ok(json!(demo || crate::core::tray::server_host_running())))
            });
        }
        "link-mine" => {
            if d.vrce.session().is_none() {
                return answer(Err("Sign in on the EchoVRCE page first.".into()));
            }
            std::thread::spawn(move || {
                let r = account::link_launchers()
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|(a, t)| linked(a, t));
                answer(r.map(Value::String));
            });
        }
        "link-code" => {
            std::thread::spawn(move || {
                let r = echovrce::request_code()
                    .map_err(|e| format!("{e:#}"))
                    .map(|c| {
                        let url = echovrce::approve_url(&c.code);
                        let _ = open::that(&url);
                        json!({"code": c.code, "url": url, "expires": c.expires_in.as_secs()})
                    });
                answer(r);
            });
        }
        "link-wait" => {
            let code = s("code");
            let expires = args.get("expires").and_then(Value::as_u64).unwrap_or(600);
            std::thread::spawn(move || answer(wait_for(&code, expires).map(Value::String)));
        }
        "password" => {
            let id = s("discord_id");
            let pw = secret("secret");
            if id.trim().is_empty() || pw.is_empty() {
                return answer(Err("The Discord ID and the password, please.".into()));
            }
            let services = Config::load().services;
            std::thread::spawn(move || {
                let r = account::check_password(&services, &id, &pw)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|t| {
                        let mut c = Config::load();
                        let guilds = account::keep_password(&mut c, &id, &pw, &t)
                            .map_err(|e| format!("{e:#}"))?;
                        c.save().map_err(|e| format!("{e:#}"))?;
                        services::restart_idle(&c);
                        Ok(guild_note(&guilds))
                    });
                answer(r.map(Value::String));
            });
        }
        "test" => {
            std::thread::spawn(move || {
                let c = Config::load();
                let r = account::session(&c).map(|t| {
                    if c.account.sign_in == SignIn::Password {
                        guild_note(&account::host_guilds(&t.token))
                    } else {
                        "Signed in (a linked session: the guilds it may host for show on EchoVRCE)."
                            .into()
                    }
                });
                answer(r.map(Value::String).map_err(|e| format!("{e:#}")));
            });
        }
        "registered" => {
            std::thread::spawn(move || {
                let r = account::registered(&Config::load()).map(|list| {
                    Value::Array(
                        list.iter()
                            .map(|r| {
                                json!({
                                    "mode": r.mode, "players": r.players, "open": r.open,
                                    "region": r.region, "country": r.country,
                                    "guilds": r.guilds.len(),
                                })
                            })
                            .collect(),
                    )
                });
                answer(r.map_err(|e| format!("{e:#}")));
            });
        }
        "doctor" if d.demo => answer(Ok(super::plugin_ui::snap_checks())),
        "doctor" => {
            std::thread::spawn(move || answer(Ok(doctor_json(&Config::load()))));
        }
        "fix" => match s("kind").as_str() {
            "game" => {
                d.page = Page::Play;
                answer(Ok(Value::Null));
            }
            "firewall" => {
                let ports = firewall_ports(&Config::load());
                let mut consent = setup::consent_asker(d.worker.tx(ctx));
                std::thread::spawn(move || {
                    let r = crate::core::elevation::allow_server_ports(&ports, &mut consent)
                        .map(|()| json!("Windows Firewall lets the servers' ports in"))
                        .map_err(|e| format!("{e:#}"));
                    answer(r);
                });
            }
            "defender" => {
                let dir = doctor::servers_folder();
                let mut consent = setup::consent_asker(d.worker.tx(ctx));
                std::thread::spawn(move || {
                    let _ = std::fs::create_dir_all(&dir);
                    let r = crate::core::elevation::exclude_from_defender(&dir, &mut consent)
                        .map(|()| json!("Windows Security leaves the servers' folder alone"))
                        .map_err(|e| format!("{e:#}"));
                    answer(r);
                });
            }
            other => answer(Err(format!("no fix {other:?}"))),
        },
        "webhook" => {
            let url = secret("secret").trim().to_string();
            if !url.is_empty() && !host::notify::is_webhook(&url) {
                return answer(Err(
                    "That isn't a Discord webhook address (https://discord.com/api/webhooks/…)."
                        .into(),
                ));
            }
            let mut c = Config::load();
            c.alerts.webhook = url.clone();
            if let Err(e) = c.save() {
                return answer(Err(format!("{e:#}")));
            }
            if url.is_empty() {
                return answer(Ok(json!("No webhook: alerts stay on this PC.")));
            }
            std::thread::spawn(move || {
                let a = host::notify::test_alert();
                let r = host::notify::post(
                    &url,
                    &host::notify::webhook_body(&a, &host::notify::host_name()),
                );
                answer(
                    r.map(|()| json!("Saved: a test message went to the webhook."))
                        .map_err(|e| format!("{e:#}")),
                );
            });
        }
        other => answer(Err(format!("no operation {other:?}"))),
    }
}

/// The servers sign in as account `a` now (a device link): what to tell the player.
fn linked(a: host::Account, t: echovrce::Tokens) -> Result<String, String> {
    let who = a.username.clone();
    let mut c = Config::load();
    account::use_device(&mut c, a, t).map_err(|e| format!("{e:#}"))?;
    c.save().map_err(|e| format!("{e:#}"))?;
    services::restart_idle(&c);
    Ok(format!("The servers sign in as {who} now."))
}

/// Polls device code `code` until it is approved (the servers are linked to its account),
/// refused, or `expires` seconds are over.
fn wait_for(code: &str, expires: u64) -> Result<String, String> {
    let until = Instant::now() + Duration::from_secs(expires.min(1800));
    loop {
        if Instant::now() > until {
            return Err("The code ran out: link again.".into());
        }
        std::thread::sleep(Duration::from_secs(3));
        match echovrce::poll(code) {
            Ok(Poll::Approved(t)) => {
                let a = account::from_code(&t).map_err(|e| format!("{e:#}"))?;
                return linked(a, t);
            }
            Ok(Poll::Pending) => {}
            Err(e) if echovrce::is_unavailable(&e) => {}
            Err(e) => return Err(format!("{e:#}")),
        }
    }
}

/// The ports the firewall rule lets in: every server's.
fn firewall_ports(c: &Config) -> Vec<u16> {
    let mut ports: Vec<u16> = c.servers.iter().map(|s| s.port).collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// The Doctor's checks as a plugin shows them: each one's name, level, text and the fix
/// it offers (`kind`: `sign-in`, `game`, `firewall` or `defender`; empty: none).
fn doctor_json(c: &Config) -> Value {
    let now = echovrce::now();
    let running: Vec<String> = c
        .servers
        .iter()
        .filter(|s| {
            crate::core::server_host::status::read(&s.dir())
                .is_some_and(|st| st.fresh(now) && st.state != State::Stopped)
        })
        .map(|s| s.name.clone())
        .collect();
    Value::Array(
        doctor::run(c, &running)
            .iter()
            .map(|ch| {
                let (kind, label) = match &ch.fix {
                    Fix::None => ("", ""),
                    Fix::SignIn => ("sign-in", "Account"),
                    Fix::Game => ("game", "Go to Play"),
                    Fix::Firewall(_) => ("firewall", "Let them in"),
                    Fix::Defender => ("defender", "Allow"),
                };
                json!({
                    "name": ch.name,
                    "level": match ch.level {
                        Level::Good => "good",
                        Level::Warn => "warn",
                        Level::Bad => "bad",
                    },
                    "text": ch.text,
                    "fix": kind,
                    "label": label,
                })
            })
            .collect(),
    )
}
