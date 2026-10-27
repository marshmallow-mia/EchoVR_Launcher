//! What a plugin can call (`wit/launcher.wit`'s imports). Each call checks what the
//! plugin may do ([`HostState::may`]) and never panics: a refused call answers an error the
//! plugin can show.

use std::time::{Duration, Instant};

use serde_json::Value;

use super::echovr::launcher::{game, host, net, server_host, types};
use super::{HostState, Out, ServerAsk, Shared, Spec, UiEvent};
use crate::core::launcher::{plugin_settings, plugins};

/// A plugin's storage, in all.
pub const STORAGE_BYTES: usize = 5 << 20;
/// Timers at once.
const TIMERS: usize = 32;
/// Requests out at once.
const IN_FLIGHT: usize = 8;
/// Notices in [`NOTICE_WINDOW`].
const NOTICES: usize = 5;
const NOTICE_WINDOW: Duration = Duration::from_secs(10);
/// How long after a click it may copy.
const CLICK_WINDOW: Duration = Duration::from_secs(1);
/// The storage's file in the plugin's folder (a dot folder: not one of its own files).
pub const STORAGE: &str = ".data/storage.json";

/// Pure: what `host.launcher-info` answers: the launcher's info for plugins, its version and
/// platform. No password, no sign-in.
pub fn info_text(s: &Shared, spec: &Spec) -> String {
    let mut v = s.info.clone();
    if let Some(m) = v.as_object_mut() {
        m.insert("version".into(), env!("CARGO_PKG_VERSION").into());
        m.insert("platform".into(), std::env::consts::OS.into());
        m.insert("plugin".into(), Value::String(spec.id().to_string()));
        if let Some(r) = m.get_mut("relay").and_then(Value::as_object_mut) {
            r.remove("password");
        }
    }
    v.to_string()
}

fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

impl HostState {
    fn storage(&mut self) -> &mut std::collections::BTreeMap<String, String> {
        let path = self.spec.dir.join(STORAGE);
        self.storage.get_or_insert_with(|| {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        })
    }

    fn write_storage(&mut self) -> Result<(), String> {
        let path = self.spec.dir.join(STORAGE);
        let text = serde_json::to_string(self.storage()).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, text).map_err(|e| format!("its storage: {e}"))
    }

    fn clicked_now(&self) -> bool {
        self.last_click.is_some_and(|t| t.elapsed() <= CLICK_WINDOW)
    }

    /// Whether a request to `url` may go out: to a server its `plugin.json` names, the
    /// classic lobbies server, or one the player typed in its settings, as allowed.
    fn may_reach(&self, url: &str) -> Result<String, String> {
        let Some(to) = plugins::origin(url) else {
            return Err(format!("{url:?} isn't an http:// or https:// address."));
        };
        let may = self.may();
        if may.servers.contains(&to) {
            return Ok(to);
        }
        if may.relay && to == self.relay_origin() {
            return Ok(to);
        }
        if may.typed_servers && self.typed_origins().contains(&to) {
            return Ok(to);
        }
        Err(format!(
            "The plugin may not talk to {to}: its plugin.json doesn't name it, or you didn't allow it."
        ))
    }

    fn relay_origin(&self) -> String {
        let server = self
            .shared
            .lock()
            .map(|s| s.relay_server.clone())
            .unwrap_or_default();
        format!("http://{}", server.trim().to_ascii_lowercase())
    }

    /// The servers the player typed in the plugin's settings, as origins.
    fn typed_origins(&self) -> Vec<String> {
        let Some(st) = &self.spec.page.settings else {
            return Vec::new();
        };
        plugin_settings::read_in(&self.spec.dir, st)
            .values()
            .flat_map(|v| {
                let v = v.trim();
                [v.to_string(), format!("http://{v}"), format!("https://{v}")]
            })
            .filter_map(|u| plugins::origin(&u))
            .collect()
    }

    fn services_allowed(&self, name: &str) -> Result<(), String> {
        if !self.may().services.contains(name) {
            return Err(format!("The plugin may not use {name}: its plugin.json doesn't ask for it, or you didn't allow it."));
        }
        if !self.spec.first_party && !cfg!(debug_assertions) {
            return Err(format!(
                "{name} is only for plugins signed by the launcher's makers, and this copy isn't."
            ));
        }
        Ok(())
    }
}

impl types::Host for HostState {}

impl host::Host for HostState {
    fn log(&mut self, level: types::Level, message: String) {
        let id = self.spec.id().to_string();
        let m = cut(&message, 2000);
        match level {
            types::Level::Debug => tracing::debug!("plugin {id}: {m}"),
            types::Level::Info => tracing::info!("plugin {id}: {m}"),
            types::Level::Warn => tracing::warn!("plugin {id}: {m}"),
            types::Level::Error => tracing::error!("plugin {id}: {m}"),
        }
    }

    fn notify(&mut self, text: String, tone: types::Tone) {
        let now = Instant::now();
        self.notices
            .retain(|t| now.duration_since(*t) < NOTICE_WINDOW);
        if self.notices.len() >= NOTICES {
            return;
        }
        self.notices.push(now);
        self.send(Out::Notify(cut(&text, 300), tone));
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn set_timer(&mut self, id: u32, ms: u32, repeat: bool) {
        if !self.timers.contains_key(&id) && self.timers.len() >= TIMERS {
            return;
        }
        let every = Duration::from_millis(u64::from(ms.max(100)));
        self.timers
            .insert(id, (Instant::now() + every, repeat.then_some(every)));
    }

    fn clear_timer(&mut self, id: u32) {
        self.timers.remove(&id);
    }

    fn storage_get(&mut self, key: String) -> Option<String> {
        self.storage().get(&key).cloned()
    }

    fn storage_set(&mut self, key: String, value: String) -> Result<(), String> {
        if key.is_empty() || key.len() > 200 {
            return Err("a key is 1 to 200 bytes".into());
        }
        let used: usize = self
            .storage()
            .iter()
            .filter(|(k, _)| **k != key)
            .map(|(k, v)| k.len() + v.len())
            .sum();
        if used + key.len() + value.len() > STORAGE_BYTES {
            return Err(format!(
                "its storage is full ({} MB at most)",
                STORAGE_BYTES >> 20
            ));
        }
        self.storage().insert(key, value);
        self.write_storage()
    }

    fn storage_remove(&mut self, key: String) {
        if self.storage().remove(&key).is_some() {
            let _ = self.write_storage();
        }
    }

    fn settings(&mut self) -> Vec<(String, String)> {
        match &self.spec.page.settings {
            Some(st) => plugin_settings::read_in(&self.spec.dir, st)
                .into_iter()
                .collect(),
            None => Vec::new(),
        }
    }

    fn set_settings(&mut self, values: Vec<(String, String)>) -> Result<(), String> {
        let Some(st) = self.spec.page.settings.clone() else {
            return Err("it has no settings".into());
        };
        let mut checked = Vec::with_capacity(values.len());
        for (k, v) in &values {
            let Some(f) = st.field(k) else {
                return Err(format!("it has no setting {k:?}"));
            };
            checked.push((k.clone(), f.check(v).map_err(|e| format!("{k}: {e}"))?));
        }
        let values = checked;
        if crate::core::launcher::echoxr_settings::is_echoxr(&st) && !self.may().echoxr {
            return Err("it may not change EchoXR's settings".into());
        }
        plugin_settings::write_in(&self.spec.dir, &st, &values).map_err(|e| format!("{e:#}"))?;
        self.send(Out::SettingsChanged);
        Ok(())
    }

    fn launcher_info(&mut self) -> String {
        self.shared
            .lock()
            .map(|s| info_text(&s, &self.spec))
            .unwrap_or_default()
    }

    fn copy(&mut self, text: String) -> Result<(), String> {
        if !self.clicked_now() {
            return Err("only right after a click on its page".into());
        }
        self.send(Out::Copy(cut(&text, 1 << 20)));
        Ok(())
    }

    fn ask(&mut self, id: String, title: String, text: String, buttons: Vec<String>, warn: bool) {
        let mut buttons: Vec<String> = buttons.iter().take(4).map(|b| cut(b, 40)).collect();
        if buttons.is_empty() {
            buttons.push("OK".into());
        }
        self.send(Out::Ask {
            id: cut(&id, 100),
            title: cut(&title, 120),
            text: cut(&text, 2000),
            buttons,
            warn,
        });
    }
}

impl net::Host for HostState {
    fn fetch(&mut self, r: net::Request) -> Result<u32, String> {
        let url = r.url.trim().to_string();
        let to = self.may_reach(&url)?;
        if self.in_flight >= IN_FLIGHT {
            return Err(format!("{IN_FLIGHT} requests are out already"));
        }
        let mut body = r.body.clone();
        if r.relay_account {
            if !self.may().relay_password || to != self.relay_origin() {
                return Err("your classic lobbies account goes to the classic lobbies server only, and only for a plugin you allowed it".into());
            }
            let (name, password) = self
                .shared
                .lock()
                .map(|s| (s.relay_name.clone(), s.relay_password.clone()))
                .unwrap_or_default();
            body = body.map(|b| with_account(&b, &name, &password));
        }
        let ticket = self.ticket();
        self.in_flight += 1;
        if self.clicked_now() {
            self.clicked_tickets.insert(ticket);
        }
        let own = self.own.clone();
        let method = match r.method {
            net::Method::Get => "GET",
            net::Method::Post => "POST",
        };
        let fake = self.shared.lock().ok().and_then(|s| s.fake_net);
        std::thread::spawn(move || {
            let result = match fake {
                Some(f) => Ok(f(&url)),
                None => crate::core::http::plugin_request(method, &url, body.as_deref())
                    .map_err(|e| format!("{e:#}")),
            };
            let _ = own.send(super::Cmd::Event(UiEvent::NetDone(ticket, result)));
        });
        Ok(ticket)
    }

    fn open_url(&mut self, url: String) -> Result<(), String> {
        self.may_reach(&url)?;
        if !self.clicked_now() {
            return Err("only right after a click on its page".into());
        }
        self.send(Out::OpenUrl(url));
        Ok(())
    }
}

/// Pure: JSON `body` with the string values `{relay.name}` and `{relay.password}` filled in
/// (anywhere in it); text that isn't JSON as it is.
pub fn with_account(body: &str, name: &str, password: &str) -> String {
    fn fill(v: &mut Value, name: &str, password: &str) {
        match v {
            Value::String(s) if s == "{relay.name}" => *s = name.to_string(),
            Value::String(s) if s == "{relay.password}" => *s = password.to_string(),
            Value::Array(a) => a.iter_mut().for_each(|v| fill(v, name, password)),
            Value::Object(m) => m.values_mut().for_each(|v| fill(v, name, password)),
            _ => {}
        }
    }
    match serde_json::from_str::<Value>(body) {
        Ok(mut v) => {
            fill(&mut v, name, password);
            v.to_string()
        }
        Err(_) => body.to_string(),
    }
}

impl game::Host for HostState {
    fn play(&mut self, version: String, args: Vec<String>) -> Result<(), String> {
        if !self.may().play {
            return Err("it may not start Echo VR".into());
        }
        if !self.clicked_now() {
            return Err("only right after a click on its page".into());
        }
        self.send(Out::Play {
            version: cut(&version, 100),
            args: args.iter().take(16).map(|a| cut(a, 200)).collect(),
        });
        Ok(())
    }

    fn set_server(&mut self, s: game::Server) -> Result<u32, String> {
        if !self.may().game_server {
            return Err("it may not change the server Echo VR plays on".into());
        }
        let ticket = self.ticket();
        self.send(Out::SetServer {
            ticket,
            server: ServerAsk {
                kind: cut(&s.kind, 20),
                name: cut(&s.name, 100),
                address: cut(&s.address, 300),
                key: cut(&s.key, 200),
                discord_id: cut(&s.discord_id, 40),
                display_name: cut(&s.display_name, 100),
                password: cut(&s.password, 200),
            },
        });
        Ok(ticket)
    }

    fn reset_server(&mut self) -> Result<(), String> {
        if !self.may().game_server {
            return Err("it may not change the server Echo VR plays on".into());
        }
        self.send(Out::ResetServer);
        Ok(())
    }
}

impl server_host::Host for HostState {
    fn call(&mut self, op: String, args: String) -> Result<String, String> {
        self.services_allowed("server-host")?;
        let args: Value = serde_json::from_str(&args).unwrap_or(Value::Null);
        super::services::call(&op, &args).map(|v| v.to_string())
    }

    fn start(&mut self, op: String, args: String) -> Result<u32, String> {
        self.services_allowed("server-host")?;
        if !super::services::STARTS.contains(&op.as_str()) {
            return Err(format!("no operation {op:?}"));
        }
        let args: Value = serde_json::from_str(&args).unwrap_or(Value::Null);
        let ticket = self.ticket();
        self.send(Out::Service { ticket, op, args });
        Ok(ticket)
    }

    fn console(&mut self, server: String) -> Result<u32, String> {
        self.services_allowed("server-host")?;
        if !crate::core::server_host::Config::load()
            .servers
            .iter()
            .any(|s| s.name == server)
        {
            return Err(format!("no server {server:?}"));
        }
        if let Some((log, _)) = self.logs.iter().find(|(_, s)| **s == server) {
            return Ok(*log);
        }
        let log = self.ticket();
        self.logs.insert(log, server.clone());
        self.send(Out::OpenLog { log, server });
        Ok(log)
    }

    fn console_copy(&mut self, log: u32, filter: String) -> Result<(), String> {
        self.services_allowed("server-host")?;
        if !self.logs.contains_key(&log) {
            return Err("no such console".into());
        }
        if !self.clicked_now() {
            return Err("only right after a click on its page".into());
        }
        self.send(Out::CopyLog {
            log,
            filter: cut(&filter, 200),
        });
        Ok(())
    }
}
