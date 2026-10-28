//! The servers' process (`EchoVR_Launcher --server-host`): runs every server that should
//! run, each on a thread of its own, and starts it again after every match (nEVR ends a
//! server when its match is over). It follows `servers.json` (Start, Stop, changes) and
//! each server's `control` file, writes `status.json` and `console.log`, and ends once no
//! server is left to run.
//!
//! A run: the server's game folder is brought up to date with the installed game, its
//! config and sign-in written, then the game starts and its log is read:
//! - registered: ready (a 180 s limit, else it is stopped: ServerDB unreachable, or a
//!   sign-in that never came);
//! - a fatal error or a refused sign-in: stopped, and started again after a wait (5 s
//!   doubling up to 5 minutes), or, for one a restart can't fix, after 10 minutes;
//! - Ctrl+C done ("teardown complete"): nEVR then hangs (#723), so it is ended 3 s later.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use super::events::{self, Kind, Reason};
use super::status::{self, Command, Console, State, Status};
use super::{account, config, farm, notify, Config, Server, SignIn};
use crate::core::launcher::nevr_root;
use crate::core::launcher::store::LauncherState;
use crate::core::launcher::{mods, nevr};
use crate::core::paths;

const REGISTER_TIMEOUT: Duration = Duration::from_secs(180);
const TEARDOWN_GRACE: Duration = Duration::from_secs(3);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(25);
const BACKOFF_FIRST: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(300);
const FAILED_WAIT: Duration = Duration::from_secs(600);
/// The supervisor ends this long after the last server stopped.
const IDLE_EXIT: Duration = Duration::from_secs(10);

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What a server's thread is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    /// Stop it now and end the thread.
    StopNow,
    /// Stop it once it is idle (not in a match) and end the thread.
    Stop,
    /// Start it again: now if idle (or `now`), else after its match.
    Restart { now: bool },
}

/// A server's thread.
struct Worker {
    tx: Sender<Order>,
    handle: JoinHandle<()>,
}

/// The status a thread keeps and writes.
struct Shared {
    dir: PathBuf,
    status: Mutex<Status>,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut Status)) {
        let mut s = self.status.lock().unwrap_or_else(|p| p.into_inner());
        let before = s.state;
        f(&mut s);
        if s.state != before {
            s.since = now();
        }
        s.updated = now();
        if let Err(e) = status::write(&self.dir, &s) {
            tracing::warn!("server host: status of {}: {e:#}", s.name);
        }
    }

    fn get(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// `--server-host`: runs until no server is left to run. Its exit code.
pub fn main() -> i32 {
    // `--server-host link`: a device link for the servers' account, without a window (a
    // server PC without a desktop).
    if std::env::args().nth(2).as_deref() == Some("link") {
        return match link_from_terminal() {
            Ok(who) => {
                println!("Linked: the servers sign in as {who}.");
                0
            }
            Err(e) => {
                eprintln!("{e:#}");
                1
            }
        };
    }
    if running_elsewhere() {
        tracing::info!("server host: one runs already");
        return 0;
    }
    tracing::info!("server host: started");
    let mut workers: std::collections::BTreeMap<String, Worker> = Default::default();
    let mut watch = notify::Watch::default();
    let mut idle_since: Option<Instant> = None;
    let mut last_look = Instant::now() - Duration::from_secs(60);
    loop {
        let cfg = Config::load();
        // Threads whose server ended (stopped, or its thread gave up).
        workers.retain(|_, w| !w.handle.is_finished());
        for s in &cfg.servers {
            let dir = s.dir();
            let asked = status::take(&dir);
            if let Some(w) = workers.get(&s.name) {
                if !s.run {
                    let _ = w.tx.send(if asked.contains(&Command::StopNow) {
                        Order::StopNow
                    } else {
                        Order::Stop
                    });
                    continue;
                }
                for c in asked {
                    let o = match c {
                        Command::Restart => Order::Restart { now: true },
                        Command::Apply => Order::Restart { now: false },
                        Command::StopNow => Order::StopNow,
                    };
                    let _ = w.tx.send(o);
                }
            } else if s.run {
                workers.insert(s.name.clone(), spawn(s.name.clone()));
            }
        }
        // Servers deleted while running.
        for (name, w) in &workers {
            if cfg.server(name).is_none() {
                let _ = w.tx.send(Order::StopNow);
                watch.forget(name);
            }
        }
        if last_look.elapsed() >= Duration::from_secs(5) {
            last_look = Instant::now();
            let t = now();
            for s in &cfg.servers {
                if let Some(st) = status::read(&s.dir()) {
                    for a in watch.look(&st, s.run, t) {
                        notify::send(&cfg.alerts, &a);
                    }
                }
            }
        }
        if workers.is_empty() && !cfg.servers.iter().any(|s| s.run) {
            let since = *idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= IDLE_EXIT {
                break;
            }
        } else {
            idle_since = None;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    tracing::info!("server host: no server left to run, ended");
    0
}

/// Asks EchoVRCE for a code, says where to approve it, waits for that and keeps the link
/// for the servers: the account's name.
fn link_from_terminal() -> Result<String> {
    use crate::core::echovrce::{self, Poll};
    let code = echovrce::request_code()?;
    println!(
        "Approve the code {} at {} while signed in as the servers' account (it runs out in {} minutes).",
        code.code,
        echovrce::approve_url(&code.code),
        code.expires_in.as_secs() / 60
    );
    let until = Instant::now() + code.expires_in;
    while Instant::now() < until {
        std::thread::sleep(Duration::from_secs(3));
        match echovrce::poll(&code.code) {
            Ok(Poll::Approved(t)) => {
                let a = account::from_code(&t)?;
                let who = a.username.clone();
                let mut cfg = Config::load();
                account::use_device(&mut cfg, a, t)?;
                cfg.save()?;
                return Ok(who);
            }
            Ok(Poll::Pending) => {}
            Err(e) if echovrce::is_unavailable(&e) => {}
            Err(e) => return Err(e),
        }
    }
    bail!("The code ran out: run it again.")
}

/// Whether another server host of this user runs.
fn running_elsewhere() -> bool {
    crate::core::tray::server_host_running()
}

/// Starts the server host unless one runs (the window does, at Start).
pub fn ensure_running() {
    if crate::core::tray::server_host_running() {
        return;
    }
    crate::core::tray::start_with(super::FLAG);
}

fn spawn(name: String) -> Worker {
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::Builder::new()
        .name(format!("server {name}"))
        .spawn(move || run_server(&name, rx))
        .expect("a server's thread");
    Worker { tx, handle }
}

/// What one run of the game ended with.
#[derive(Debug, Default)]
struct Outcome {
    code: Option<i32>,
    /// Ended the normal way: its match over, or stopped as asked.
    clean: bool,
    registered: bool,
    reason: Option<Reason>,
    detail: String,
    /// Asked to stop: the thread ends.
    stop: bool,
    /// nEVR said its Ctrl+C teardown is done (it unregistered the server).
    torn_down: bool,
}

/// A server's thread: its runs, until it is told to stop.
fn run_server(name: &str, rx: Receiver<Order>) {
    let dir = super::server_dir(name);
    let previous = status::read(&dir).unwrap_or_default();
    let shared = Arc::new(Shared {
        dir: dir.clone(),
        status: Mutex::new(Status {
            name: name.to_string(),
            state: State::Starting,
            matches: previous.matches,
            restarts: previous.restarts,
            registrations: previous.registrations,
            ..Status::default()
        }),
    });
    let mut console = Console::open(&dir);
    let mut backoff = BACKOFF_FIRST;
    let heartbeat = Arc::new(AtomicBool::new(true));
    {
        let (shared, beat) = (shared.clone(), heartbeat.clone());
        std::thread::spawn(move || {
            while beat.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(status::HEARTBEAT_S as u64));
                if beat.load(Ordering::Relaxed) {
                    shared.update(|_| {});
                }
            }
        });
    }
    loop {
        let out = match prepare(name, &dir) {
            Ok(p) => run_once(&p, &shared, &mut console, &rx),
            Err(e) => {
                let msg = format!("{e:#}");
                console.host(&format!("can't start: {msg}"));
                Outcome {
                    reason: Some(Reason::InvalidConfig),
                    detail: msg,
                    ..Outcome::default()
                }
            }
        };
        shared.update(|s| {
            s.last_exit = out.code;
            s.pid = 0;
            s.session.clear();
        });
        if out.stop {
            break;
        }
        let wait = if out.clean {
            backoff = BACKOFF_FIRST;
            console.host("the game ended: starting it again");
            Duration::from_secs(5)
        } else if let Some(r) = out.reason.filter(|r| !r.retryable()) {
            console.host(&format!(
                "stopped: {}. Fix it on the Server host page; it tries again in 10 minutes",
                r.words()
            ));
            shared.update(|s| {
                s.state = State::Failed;
                s.reason = Some(r);
                s.last_error = out.detail.clone();
                s.next_start = now() + FAILED_WAIT.as_secs() as i64;
            });
            FAILED_WAIT
        } else {
            let w = backoff;
            backoff = (backoff * 2).min(BACKOFF_MAX);
            console.host(&format!(
                "the game ended ({}): starting it again in {} s",
                out.reason.map_or("unexpectedly", Reason::words),
                w.as_secs()
            ));
            shared.update(|s| {
                s.state = State::Backoff;
                s.reason = out.reason;
                s.last_error = out.detail.clone();
                s.next_start = now() + w.as_secs() as i64;
            });
            w
        };
        if out.registered {
            backoff = BACKOFF_FIRST;
        }
        match rx.recv_timeout(wait) {
            Ok(Order::Stop | Order::StopNow) => break,
            Ok(Order::Restart { .. }) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        shared.update(|s| s.restarts += 1);
    }
    heartbeat.store(false, Ordering::Relaxed);
    shared.update(|s| {
        s.state = State::Stopped;
        s.pid = 0;
        s.next_start = 0;
    });
    console.host("stopped");
}

/// What a run needs, worked out before it.
struct Prepared {
    server: Server,
    exe: PathBuf,
    bin: PathBuf,
    config_json: PathBuf,
    sign_in: SignIn,
    password: String,
    /// Windows: the server's own `LOCALAPPDATA` (nEVR's log goes there).
    appdata: PathBuf,
    /// Linux: its Proton data.
    compat: PathBuf,
}

/// The server's game folder, config and sign-in, made ready for a run.
fn prepare(name: &str, dir: &Path) -> Result<Prepared> {
    let cfg = Config::load();
    let Some(server) = cfg.server(name).cloned() else {
        bail!("{name} is no longer set up");
    };
    if let Some(why) = super::check_server(&server, &cfg.servers) {
        bail!("{why}");
    }
    if let Some(why) = config::check_services(&cfg) {
        bail!("{why}");
    }
    if cfg.account.sign_in == SignIn::None || cfg.account.discord_id.trim().is_empty() {
        bail!("the servers have no account yet: sign them in under Account");
    }
    let state = LauncherState::load();
    let Some(v) = cfg.version_in(&state) else {
        bail!("no installed live build to run (event builds can't host servers)");
    };
    if !mods::nevr_in(&v.bin_dir()) {
        bail!(
            "{} has no nEVR in its slot: a server needs it (update the game)",
            v.name
        );
    }
    if cfg!(target_os = "linux") && !crate::core::linux::echoxr::proton_script().is_file() {
        bail!("set Echo VR up on Linux first (PLAY does it): servers run through its Proton");
    }
    let (plugins, local) = if server.mods {
        mods::server_plugins(v)
    } else {
        (Vec::new(), false)
    };
    let yaml = config::render(&cfg, &server, &plugins, local);
    let json = config::game_config(&cfg.publisher_lock);
    let game = Path::new(&v.root).join(paths::ARENA_DIR);
    let root = super::farm_root(dir);
    let f = farm::Farm {
        game: &game,
        root: &root,
        port: server.port,
        config_yaml: &yaml,
        config_json: &json,
    };
    f.build().context("the server's game folder")?;
    let appdata = dir.join("appdata");
    let compat = crate::core::linux::echoxr::server_compat_data(&format!("host-{name}"));
    // The server's own nEVR root (in its own LOCALAPPDATA): a root host's runtime, and the
    // sign-in nEVR 5.13 on keeps there. A prefix Proton hasn't made yet gets them next run.
    let local = local_app_data(&appdata, &compat);
    let root = local
        .filter(|l| cfg!(windows) || l.is_dir())
        .map(|l| nevr_root::root_in(&l));
    if let Some(root) = &root {
        nevr_root::put_in_place(&v.bin_dir(), root, &dir.join("nevr-root.json"), false)
            .context("nEVR's runtime for the server")?;
    }
    account::ensure_login(&cfg, &f.local(), root.as_deref()).context("the server's sign-in")?;
    let password = match cfg.account.sign_in {
        SignIn::Password => account::secrets::load().password,
        _ => String::new(),
    };
    if cfg.account.sign_in == SignIn::Password && password.is_empty() {
        bail!("the servers' password is gone: sign in again under Account");
    }
    Ok(Prepared {
        exe: f.bin().join(v.exe_name()),
        bin: f.bin(),
        config_json: f.local().join(nevr::GAME_CONFIG),
        sign_in: cfg.account.sign_in,
        password,
        appdata,
        compat,
        server,
    })
}

/// Pure: the game's arguments for `s` (`config` as the game sees the path).
pub fn game_args(s: &Server, config: &str) -> Vec<String> {
    let mut a: Vec<String> = [
        "-server",
        "-headless",
        "-noovr",
        "-nosymbollookup",
        "-port",
        &s.port.to_string(),
        "-httpport",
        &s.http_port().to_string(),
        "-config-path",
        config,
    ]
    .iter()
    .map(|x| x.to_string())
    .collect();
    a.extend(s.extra_args.split_whitespace().map(str::to_string));
    a
}

/// The command for a run.
fn command(p: &Prepared) -> (std::process::Command, Option<super::proc::Wine>) {
    if cfg!(target_os = "linux") {
        use crate::core::linux::echoxr;
        let config = echoxr::wine_path(&[], &p.config_json);
        let mut c = std::process::Command::new(echoxr::proton_script());
        c.arg("run")
            .arg(&p.exe)
            .args(game_args(&p.server, &config))
            .current_dir(&p.bin)
            .env("STEAM_COMPAT_DATA_PATH", &p.compat)
            .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", "")
            .env("UMU_ID", "umu-default")
            .env("PROTON_LOG", "0")
            .env("WINEDEBUG", "-all")
            .env("WINEDLLOVERRIDES", "mscoree,mshtml=");
        let _ = std::fs::create_dir_all(&p.compat);
        if p.sign_in == SignIn::Password {
            c.env(config::PASSWORD_ENV, &p.password);
        }
        let wine = super::proc::Wine {
            prefix: p.compat.join("pfx"),
            wineserver: echoxr::wineserver(),
            exe: echoxr::wine_path(&[], &p.exe),
        };
        return (c, Some(wine));
    }
    let mut c = std::process::Command::new(&p.exe);
    c.args(game_args(&p.server, &p.config_json.to_string_lossy()))
        .current_dir(&p.bin)
        .env("LOCALAPPDATA", &p.appdata);
    let _ = std::fs::create_dir_all(&p.appdata);
    if p.sign_in == SignIn::Password {
        c.env(config::PASSWORD_ENV, &p.password);
    }
    (c, None)
}

/// Lines from the game: nEVR's log file (its jsonl, followed from the start), and what
/// Proton says on its output (Linux). The game's own output reaches neither a pipe under
/// Proton nor one on Windows (nEVR's console window has it).
fn lines(
    child_out: Option<(std::process::ChildStdout, std::process::ChildStderr)>,
    log_dirs: Vec<PathBuf>,
    since: std::time::SystemTime,
    stop: Arc<AtomicBool>,
) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    if let Some((out, err)) = child_out {
        for r in [
            Box::new(out) as Box<dyn std::io::Read + Send>,
            Box::new(err),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                use std::io::BufRead;
                for l in std::io::BufReader::new(r).lines() {
                    let Ok(l) = l else { break };
                    if tx.send(l).is_err() {
                        break;
                    }
                }
            });
        }
    }
    std::thread::spawn(move || {
        let mut file: Option<PathBuf> = None;
        let mut tail: Option<status::Tail> = None;
        while !stop.load(Ordering::Relaxed) {
            // The newest of nEVR's logs since the start (it starts a new one at 50 MB).
            if let Some((newest, _)) = log_dirs
                .iter()
                .filter_map(|d| {
                    crate::core::logs::newest_in(d, nevr::is_run_log)
                        .into_iter()
                        .next()
                })
                .filter(|(_, t)| *t >= since)
                .max_by_key(|(_, t)| *t)
            {
                if file.as_ref() != Some(&newest) {
                    tail = Some(status::Tail::file(&newest));
                    file = Some(newest);
                }
            }
            if let Some(t) = &mut tail {
                for l in t.read() {
                    if tx.send(l).is_err() {
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    rx
}

/// The server's `%LOCALAPPDATA%`: on Windows its own (`appdata`), on Linux its prefix's
/// (in `compat`).
fn local_app_data(appdata: &Path, compat: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        Some(compat.join("pfx/drive_c/users/steamuser/AppData/Local"))
    } else if cfg!(windows) {
        Some(appdata.to_path_buf())
    } else {
        None
    }
}

/// Where nEVR writes its log for the server: its root's `logs` (5.13 on), else
/// `%LOCALAPPDATA%\EchoVR\logs`, in the server's own `LOCALAPPDATA`.
fn log_dirs(p: &Prepared) -> Vec<PathBuf> {
    local_app_data(&p.appdata, &p.compat)
        .map(|l| {
            vec![
                nevr_root::root_in(&l).join("logs"),
                l.join("EchoVR").join("logs"),
            ]
        })
        .unwrap_or_default()
}

/// One run of the game: until it ends.
fn run_once(p: &Prepared, shared: &Shared, console: &mut Console, rx: &Receiver<Order>) -> Outcome {
    let (mut cmd, wine) = command(p);
    let linux = cfg!(target_os = "linux");
    cmd.stdin(std::process::Stdio::null());
    if linux {
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    } else {
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    let started_at = std::time::SystemTime::now() - Duration::from_secs(2);
    let mut game = match super::proc::Game::start(cmd, wine) {
        Ok(g) => g,
        Err(e) => {
            return Outcome {
                reason: Some(Reason::Fatal),
                detail: format!("{e:#}"),
                ..Outcome::default()
            }
        }
    };
    let port = p.server.port;
    console.host(&format!(
        "starting {} on port {port} (HTTP API {})",
        p.server.name,
        p.server.http_port()
    ));
    shared.update(|s| {
        s.state = State::Starting;
        s.pid = game.pid();
        s.port = port;
        s.reason = None;
        s.last_error.clear();
        s.next_start = 0;
        s.public_ip.clear();
    });
    let stop_tail = Arc::new(AtomicBool::new(false));
    let out = linux.then(|| game.take_output()).flatten();
    let lines = lines(out, log_dirs(p), started_at, stop_tail.clone());

    let mut o = Outcome::default();
    let started = Instant::now();
    let mut kill_at: Option<Instant> = None;
    let mut stopping = false;
    // Stop once idle (Stop, or a restart after the match).
    let mut after_match: Option<bool> = None;
    let mut ended: Option<std::process::ExitStatus> = None;
    let mut no_credentials = false;
    let schedule = |kill_at: &mut Option<Instant>, d: Duration| {
        let at = Instant::now() + d;
        if kill_at.is_none_or(|k| at < k) {
            *kill_at = Some(at);
        }
    };
    loop {
        if started.elapsed() < Duration::from_secs(60) {
            game.hide_console();
        }
        let mut got = 0;
        while let Ok(raw) = lines.try_recv() {
            got += 1;
            console.line(&raw);
            let ev = events::parse(&raw);
            handle(
                &ev,
                p,
                shared,
                console,
                &mut o,
                &mut kill_at,
                &mut no_credentials,
                &schedule,
                &mut game,
            );
            if got > 500 {
                break;
            }
        }
        if let Some(st) = game.try_wait() {
            ended = Some(st);
            break;
        }
        // Orders.
        while let Ok(order) = rx.try_recv() {
            match order {
                Order::StopNow => {
                    o.stop = true;
                    after_match = None;
                    if !stopping {
                        stopping = true;
                        console.host("stopping: Ctrl+C to the game");
                        shared.update(|s| s.state = State::Stopping);
                        game.interrupt();
                        schedule(&mut kill_at, STOP_TIMEOUT);
                    }
                }
                Order::Stop => {
                    o.stop = true;
                    after_match = Some(true);
                }
                Order::Restart { now: true } => {
                    if !stopping {
                        stopping = true;
                        console.host("restarting now: Ctrl+C to the game");
                        shared.update(|s| s.state = State::Stopping);
                        game.interrupt();
                        schedule(&mut kill_at, STOP_TIMEOUT);
                    }
                }
                Order::Restart { now: false } => {
                    if shared.get().state.idle() {
                        after_match = Some(false);
                    } else {
                        // nEVR ends the server after the match: the next run has them.
                        console.host("its settings changed: they apply after this match");
                    }
                }
            }
        }
        if let Some(stop_it) = after_match {
            if shared.get().state.idle() && !stopping {
                stopping = true;
                o.stop = stop_it;
                console.host(if stop_it {
                    "stopping: Ctrl+C to the game"
                } else {
                    "restarting: Ctrl+C to the game"
                });
                shared.update(|s| s.state = State::Stopping);
                game.interrupt();
                schedule(&mut kill_at, STOP_TIMEOUT);
            }
        }
        if !o.registered && !stopping && started.elapsed() >= REGISTER_TIMEOUT {
            o.reason = Some(Reason::NotRegistered);
            o.detail = if no_credentials {
                "not registered within 180 s: it had no sign-in (link the servers again under Account)".into()
            } else {
                "not registered within 180 s (ServerDB unreachable, or the sign-in never came)"
                    .into()
            };
            console.host(&format!("{}; ending the game", o.detail));
            game.kill();
            break;
        }
        if kill_at.is_some_and(|k| Instant::now() >= k) {
            console.host(if o.torn_down {
                "unregistered; nEVR hangs after that (nevr-runtime #723): ending the game"
            } else if stopping {
                "the game didn't end after Ctrl+C: ending it"
            } else {
                "the game didn't end after shutting down (nevr-runtime #723): ending it"
            });
            game.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(if got > 0 { 20 } else { 200 }));
    }
    // What is left of its log.
    let drain = Instant::now() + Duration::from_secs(1);
    while Instant::now() < drain {
        match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(raw) => {
                console.line(&raw);
                let ev = events::parse(&raw);
                if ev.kind != Kind::None {
                    let mut ignore = None;
                    handle(
                        &ev,
                        p,
                        shared,
                        console,
                        &mut o,
                        &mut ignore,
                        &mut no_credentials,
                        &schedule,
                        &mut game,
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    stop_tail.store(true, Ordering::Relaxed);
    // Wine's helpers, the server's wineserver, a job's leftovers.
    game.kill();
    o.code = ended.and_then(|s| s.code()).or(o.code);
    if stopping || (o.reason.is_none() && (o.clean || o.code == Some(0))) {
        o.clean = true;
    }
    o
}

/// What one line does to the run.
#[allow(clippy::too_many_arguments)]
fn handle(
    ev: &events::Event,
    p: &Prepared,
    shared: &Shared,
    console: &mut Console,
    o: &mut Outcome,
    kill_at: &mut Option<Instant>,
    no_credentials: &mut bool,
    schedule: &dyn Fn(&mut Option<Instant>, Duration),
    game: &mut super::proc::Game,
) {
    match ev.kind {
        Kind::RuntimeVersion => shared.update(|s| s.runtime = ev.version.clone()),
        Kind::NoCredentials => *no_credentials = true,
        Kind::Bound => {
            if ev.port != p.server.port {
                o.reason = Some(Reason::InvalidConfig);
                o.detail = format!(
                    "the game took port {} instead of {}",
                    ev.port, p.server.port
                );
                console.host(&format!("{}; ending it", o.detail));
                game.kill();
            }
        }
        Kind::Registered => {
            o.registered = true;
            shared.update(|s| {
                s.state = State::Registered;
                s.public_ip = ev.ip.clone();
                s.registrations += 1;
                s.reason = None;
                s.last_error.clear();
            });
            console.host(&format!("READY: registered as {}:{}", ev.ip, ev.port));
        }
        Kind::SessionStart => shared.update(|s| {
            s.state = State::InSession;
            s.session = ev.session.clone();
            s.matches += 1;
        }),
        Kind::SessionEnd => {
            o.clean = true;
            shared.update(|s| s.state = State::Stopping);
        }
        Kind::Teardown => {
            o.torn_down = true;
            o.clean = o.reason.is_none();
            schedule(kill_at, TEARDOWN_GRACE);
        }
        Kind::Shutdown => {
            if o.reason.is_none() && !o.registered {
                // nEVR gives up by itself when ServerDB drops a server it never
                // registered (its port unreachable, a refused registration).
                o.reason = Some(Reason::NotRegistered);
                o.detail = "ServerDB dropped the server before it registered (is its port reachable from the internet?)".into();
            }
            if o.reason.is_none() {
                o.clean = true;
            }
            schedule(kill_at, SHUTDOWN_GRACE);
        }
        Kind::LoginFailed | Kind::Fatal | Kind::Held => {
            if o.reason.is_none() || ev.kind != Kind::LoginFailed {
                o.reason = ev.reason;
                o.detail = ev.line.clone();
            }
            o.clean = false;
            if matches!(ev.kind, Kind::LoginFailed | Kind::Held) {
                // It stays up but never registers (or never plays): no point waiting.
                let why = if ev.kind == Kind::Held {
                    "the game loop crashed"
                } else if p.sign_in == SignIn::Device {
                    "EchoVRCE refused the sign-in: a linked account must have no password (/reset-password in the Echo VR Lounge Discord)"
                } else {
                    "EchoVRCE refused the sign-in: check the Discord ID and password"
                };
                console.host(&format!("{why}; ending the game"));
                game.kill();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_the_game_as_a_server_on_its_port() {
        let s = Server {
            name: "echo-1".into(),
            port: 6800,
            extra_args: " -foo  bar ".into(),
            ..Server::default()
        };
        let a = game_args(&s, r"Z:\x\config.json");
        assert_eq!(
            a,
            [
                "-server",
                "-headless",
                "-noovr",
                "-nosymbollookup",
                "-port",
                "6800",
                "-httpport",
                "7800",
                "-config-path",
                r"Z:\x\config.json",
                "-foo",
                "bar"
            ]
        );
    }
}
