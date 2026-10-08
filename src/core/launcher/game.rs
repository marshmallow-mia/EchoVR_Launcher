//! Game state: is Echo running, is its local API up, is it in a match -- and which of the
//! running games is the one the launcher started.
//!
//! A background thread polls once a second: an OS process scan for the game's executables
//! (which also sees games started outside the launcher, and Proton's) plus the game's local
//! HTTP API on port 6721. An HTTP error answer means "running, not in a match"; a refused
//! connection means "not running, or API access disabled".
//!
//! Only clients count. A dedicated server can run on the same PC from the same executable:
//! it is a game process with `pnsradgameserver.dll` loaded, or started with `-server` or
//! `-headless` (as EchoRelay tells them apart). It never makes the game "running", and STOP
//! never ends it. The game the launcher started is known by its process: a descendant of
//! what PLAY spawned (the game itself, Revive's injector, EchoXR.exe) or, on Linux, of the
//! `--play` launcher Steam runs. It is kept in `game.json`, so a restarted launcher still
//! knows it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::core::paths;

pub const API_URL: &str = "http://127.0.0.1:6721/session";

/// Pure: whether a start's exit `code` is Windows' loader turning the game down before it
/// ran (a DLL it loads at its start missing, blocked or flagged), and what it says.
pub fn loader_failure(code: i32) -> Option<&'static str> {
    Some(match code as u32 {
        0xC000_0135 => "a file it loads at its start is missing",
        0xC000_0022 => "Windows denied access to a file it loads at its start",
        0xC000_0043 => "a file it loads at its start was in use (being scanned)",
        0xC000_0906 => "Windows flagged a file it loads at its start as a threat",
        0xC000_0428 => "Windows rejected the signature of a file it loads at its start",
        _ => return None,
    })
}

/// The game server plugin: loaded only by a game running as a dedicated server.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub const SERVER_MODULE: &str = "pnsradgameserver.dll";

/// How long a started game may take to show up before PLAY is back.
pub const LAUNCH_WAIT: Duration = Duration::from_secs(60);

/// How many parents up a game may be from what started it (Steam's `--play` launcher,
/// Proton, Wine's `steam.exe`, EchoXR.exe, the game).
const MAX_HOPS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GameState {
    #[default]
    NotRunning,
    /// Running but the local API is off (`EnableAPIAccess` false) or still starting.
    RunningApiOff,
    /// Running, not in a match (menus / lobby loading).
    Running,
    InMatch {
        session_id: String,
    },
}

impl GameState {
    pub fn is_running(&self) -> bool {
        *self != GameState::NotRunning
    }

    pub fn label(&self) -> String {
        match self {
            GameState::NotRunning => "Echo VR is not running".into(),
            GameState::RunningApiOff => "Echo VR is running".into(),
            GameState::Running => "Echo VR is running (in the menus)".into(),
            GameState::InMatch { session_id } => format!("In a match ({})", short(session_id)),
        }
    }
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Client,
    /// A dedicated server.
    Server,
}

/// A running game process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProcess {
    pub pid: u32,
    /// When it started (Unix seconds): with the pid, what tells it from a later process
    /// that got the same pid.
    pub start: u64,
    pub role: Role,
    /// Its executable, when the OS tells.
    pub exe: Option<PathBuf>,
}

/// Echo on this PC as the monitor last saw it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Local {
    /// The game clients' state (servers left out).
    pub state: GameState,
    /// The installed version the launcher started, while that game runs.
    pub ours: Option<String>,
    /// Clients the launcher didn't start.
    pub others: usize,
    /// Dedicated servers: their executables, when known.
    pub servers: Vec<Option<PathBuf>>,
}

impl Local {
    /// Whether a server runs from the install at `root` (one whose executable isn't known
    /// might: it counts).
    pub fn server_in(&self, root: &str) -> bool {
        self.servers
            .iter()
            .any(|exe| exe.as_deref().is_none_or(|e| in_folder(e, root)))
    }
}

/// Pure: whether `path` is inside the folder `root` (case-insensitive, either slash).
pub fn in_folder(path: &Path, root: &str) -> bool {
    let p = paths::normalize(&path.to_string_lossy()).to_ascii_lowercase();
    let r = paths::normalize(root).to_ascii_lowercase();
    !r.is_empty() && p.len() > r.len() && p.starts_with(&r) && p.as_bytes()[r.len()] == b'/'
}

/// The file name in a path with either slash.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn game_name(s: &str) -> bool {
    let f = file_name(s);
    paths::GAME_PROCESSES
        .iter()
        .any(|g| f.eq_ignore_ascii_case(g))
}

/// Pure: whether a process (its name and command line) is the game: by its name, or by
/// the program its command line starts with (Proton's game processes have a cut-off
/// name). Any argument counts only for Wine's own loaders, and only while they aren't
/// running another Windows program, so wrappers that pass the game's path on (Revive's
/// injector, `proton`, EchoXR.exe, GE-Proton's umu.exe) are not the game.
pub fn is_game(name: &str, cmd: &[String]) -> bool {
    if game_name(name) || cmd.first().is_some_and(|c| game_name(c)) {
        return true;
    }
    let n = name.to_ascii_lowercase();
    let loader = n.is_empty() || n.starts_with("wine") || n.ends_with("-preloader");
    // The first Windows program on a loader's command line is what it runs (the scan may
    // keep the command line from before Wine rewrites it: `wine-preloader wine umu.exe
    // .../echovr.exe`); any later one is just an argument.
    let program = cmd
        .iter()
        .find(|a| file_name(a).to_ascii_lowercase().ends_with(".exe"));
    loader && program.is_some_and(|p| game_name(p))
}

/// Pure: a game process's role from its arguments and whether the server plugin is loaded.
/// Clients never use `-server` or `-headless` (Flat and spectating use `-noovr`).
pub fn classify(cmd: &[String], server_module: bool) -> Role {
    let flag = cmd
        .iter()
        .skip(1)
        .any(|a| a.eq_ignore_ascii_case("-server") || a.eq_ignore_ascii_case("-headless"));
    if server_module || flag {
        Role::Server
    } else {
        Role::Client
    }
}

/// The game's executable: what the OS says, else the program on its command line (under
/// Proton a `Z:\` path, the host's file system).
fn game_path(exe: Option<&Path>, cmd: &[String]) -> Option<PathBuf> {
    if let Some(e) = exe.filter(|e| game_name(&e.to_string_lossy())) {
        return Some(e.to_path_buf());
    }
    let arg = cmd.iter().find(|a| game_name(a))?;
    let unix = arg
        .strip_prefix("Z:")
        .or_else(|| arg.strip_prefix("z:"))
        .map(|rest| rest.replace('\\', "/"));
    Some(PathBuf::from(unix.unwrap_or_else(|| arg.clone())))
}

/// Whether the game server plugin is loaded in process `pid`.
#[cfg(windows)]
fn server_module_loaded(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE,
    };
    // SAFETY: a module snapshot we own and close; the entry is sized as the API asks.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        if snap == INVALID_HANDLE_VALUE {
            // Not ours to read (another user's, or elevated): its arguments decide.
            return false;
        }
        let mut entry = MODULEENTRY32W {
            dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        let mut more = Module32FirstW(snap, &mut entry) != 0;
        while more {
            let len = entry
                .szModule
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szModule.len());
            if String::from_utf16_lossy(&entry.szModule[..len]).eq_ignore_ascii_case(SERVER_MODULE)
            {
                found = true;
                break;
            }
            more = Module32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// Whether the game server plugin is mapped into (Wine) process `pid`.
#[cfg(target_os = "linux")]
fn server_module_loaded(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/maps"))
        .is_ok_and(|m| m.to_ascii_lowercase().contains(SERVER_MODULE))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn server_module_loaded(_pid: u32) -> bool {
    false
}

/// True when a process with this executable name (case-insensitive) is running.
pub fn process_running(exe_name: &str) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes()
        .values()
        .any(|p| p.name().to_string_lossy().eq_ignore_ascii_case(exe_name))
}

/// Whether a process runs from a program named `file_name`, by its executable or its
/// command line: for processes that rename themselves (SteamVR's `vrserver` shows up as
/// "2676033: vrwebh").
pub fn program_running(file_name: &str) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet),
    );
    let named = |p: &std::path::Path| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(file_name))
    };
    sys.processes().values().any(|p| {
        p.name().to_string_lossy().eq_ignore_ascii_case(file_name)
            || p.exe().is_some_and(named)
            || p.cmd()
                .first()
                .is_some_and(|c| named(std::path::Path::new(c)))
    })
}

/// A process, as pid and start (Unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Owned {
    pub pid: u32,
    pub start: u64,
}

/// The game the launcher started: what PLAY spawned, and the game processes found since.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
struct Ours {
    /// The installed version it is.
    version: String,
    /// What PLAY spawned (the game, Revive's injector, EchoXR.exe), while it starts.
    starter: Option<u32>,
    /// When PLAY was clicked (Unix seconds).
    since: u64,
    /// The folder of the version's executable: a game starting from there meanwhile is
    /// the one PLAY started, when its parents can't tell.
    bin: Option<PathBuf>,
    /// Its game processes.
    games: Vec<Owned>,
}

impl Ours {
    fn pending(&self, now: u64) -> bool {
        now < self.since + LAUNCH_WAIT.as_secs()
    }
}

/// Pure: adds to `ours` the clients among `games` that are its: descended from one of
/// `roots` (`parents`: each process's parent), or -- while a launch is pending and no game
/// is known yet -- started since from its executable's folder. Drops the games that ended.
/// Whether anything changed.
fn adopt(
    ours: &mut Ours,
    games: &[GameProcess],
    parents: &HashMap<u32, u32>,
    roots: &[u32],
    now: u64,
) -> bool {
    let before = ours.games.clone();
    ours.games
        .retain(|o| games.iter().any(|g| g.pid == o.pid && g.start == o.start));
    let descends = |pid: u32| {
        let mut cur = pid;
        (0..MAX_HOPS).any(|_| match parents.get(&cur) {
            Some(&p) if p != cur => {
                cur = p;
                roots.contains(&p)
            }
            _ => false,
        })
    };
    let fallback = ours.pending(now) && ours.games.is_empty();
    for g in games {
        let o = Owned {
            pid: g.pid,
            start: g.start,
        };
        // What PLAY started is ours even as a server (`-server` in the launch options).
        let started = roots.contains(&g.pid) || descends(g.pid);
        if ours.games.contains(&o) || !started && g.role == Role::Server {
            continue;
        }
        // Process start times are whole seconds: allow for the one PLAY was clicked in.
        let from_bin = fallback
            && g.start + 1 >= ours.since
            && ours
                .bin
                .as_deref()
                .zip(g.exe.as_deref())
                .is_some_and(|(bin, exe)| in_folder(exe, &bin.to_string_lossy()));
        if started || from_bin {
            ours.games.push(o);
        }
    }
    ours.games != before
}

/// Pure: the clients' count and the servers, given which games are ours.
fn summarize(games: &[GameProcess], ours: &[Owned]) -> (usize, usize, Vec<Option<PathBuf>>) {
    let mine = |g: &GameProcess| {
        ours.contains(&Owned {
            pid: g.pid,
            start: g.start,
        })
    };
    let ours_n = games.iter().filter(|g| mine(g)).count();
    let others = games
        .iter()
        .filter(|g| !mine(g) && g.role == Role::Client)
        .count();
    let servers = games
        .iter()
        .filter(|g| !mine(g) && g.role == Role::Server)
        .map(|g| g.exe.clone())
        .collect();
    (ours_n, others, servers)
}

/// Where the game the launcher started is remembered.
fn ours_file() -> PathBuf {
    paths::data_dir().join("game.json")
}

fn load_ours() -> Option<Ours> {
    let text = std::fs::read_to_string(ours_file()).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_ours(ours: Option<&Ours>) {
    let f = ours_file();
    match ours.filter(|o| !o.games.is_empty()) {
        Some(o) => {
            if let Ok(json) = serde_json::to_vec(o) {
                let _ = std::fs::create_dir_all(paths::data_dir());
                let _ = std::fs::write(f, json);
            }
        }
        None => {
            let _ = std::fs::remove_file(f);
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The scan the monitor thread keeps (one `System`, refreshed each time).
struct Scanner {
    sys: sysinfo::System,
    /// Games once seen as servers: they stay servers (a lobby build blanks its `-server`).
    servers: HashSet<Owned>,
}

impl Scanner {
    fn new() -> Scanner {
        Scanner {
            sys: sysinfo::System::new(),
            servers: HashSet::new(),
        }
    }

    /// Every game process now, and each process's parent.
    fn scan(&mut self) -> (Vec<GameProcess>, HashMap<u32, u32>) {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet),
        );
        let mut parents = HashMap::new();
        let mut games = Vec::new();
        for p in self.sys.processes().values() {
            let pid = p.pid().as_u32();
            if let Some(parent) = p.parent() {
                parents.insert(pid, parent.as_u32());
            }
            let cmd: Vec<String> = p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            if !is_game(&p.name().to_string_lossy(), &cmd) {
                continue;
            }
            let key = Owned {
                pid,
                start: p.start_time(),
            };
            let role = if self.servers.contains(&key) {
                Role::Server
            } else {
                classify(&cmd, server_module_loaded(pid))
            };
            if role == Role::Server {
                self.servers.insert(key);
            }
            games.push(GameProcess {
                pid,
                start: key.start,
                role,
                exe: game_path(p.exe(), &cmd),
            });
        }
        self.servers
            .retain(|s| games.iter().any(|g| g.pid == s.pid && g.start == s.start));
        (games, parents)
    }
}

/// Ends the game processes in `games` that still run (the same pid and start): the game
/// the launcher started, never another. Returns how many were ended.
fn stop(games: &[Owned]) -> usize {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pids: Vec<Pid> = games.iter().map(|g| Pid::from_u32(g.pid)).collect();
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        true,
        ProcessRefreshKind::nothing(),
    );
    let mut ended = 0;
    for g in games {
        if let Some(p) = sys.process(Pid::from_u32(g.pid)) {
            if p.start_time() == g.start && p.kill() {
                tracing::info!("stopped {} (pid {})", p.name().to_string_lossy(), g.pid);
                ended += 1;
            }
        }
    }
    ended
}

#[derive(Deserialize)]
struct Session {
    #[serde(default)]
    sessionid: String,
    #[serde(default)]
    err_code: Option<i64>,
}

/// Pure: interprets one API poll. `Ok((status, body))` is an HTTP answer.
pub fn interpret(api: Result<(u16, String), ()>, process: bool) -> GameState {
    match api {
        Ok((200, body)) => match serde_json::from_str::<Session>(&body) {
            Ok(s) if s.err_code.unwrap_or(0) == 0 && !s.sessionid.is_empty() => {
                GameState::InMatch {
                    session_id: s.sessionid,
                }
            }
            _ => GameState::Running,
        },
        Ok(_) => GameState::Running,
        Err(()) if process => GameState::RunningApiOff,
        Err(()) => GameState::NotRunning,
    }
}

fn poll_api() -> Result<(u16, String), ()> {
    crate::core::http::block_on(async {
        let resp = crate::core::http::client()
            .get(API_URL)
            .timeout(Duration::from_millis(800))
            .send()
            .await
            .map_err(|_| ())?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().await.unwrap_or_default()))
    })
}

/// Shared, continuously updated game state: on this PC, and on the Quest at its
/// network address (through its API) once that is known.
#[derive(Clone, Default)]
pub struct Monitor {
    local: Arc<Mutex<Local>>,
    ours: Arc<Mutex<Option<Ours>>>,
    quest: Arc<Mutex<GameState>>,
    quest_ip: Arc<Mutex<Option<std::net::Ipv4Addr>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Stores `next` in `shared`; whether it changed.
fn store<T: PartialEq>(shared: &Mutex<T>, next: T) -> bool {
    let mut s = lock(shared);
    let changed = *s != next;
    *s = next;
    changed
}

impl Monitor {
    /// Starts the polling threads; `on_change` runs whenever a state changes.
    pub fn start(on_change: impl Fn() + Send + Sync + 'static) -> Monitor {
        let m = Monitor {
            ours: Arc::new(Mutex::new(load_ours())),
            ..Monitor::default()
        };
        let on_change = Arc::new(on_change);
        let (quest, quest_ip, notify) = (m.quest.clone(), m.quest_ip.clone(), on_change.clone());
        std::thread::Builder::new()
            .name("quest-monitor".into())
            .spawn(move || loop {
                let ip = *lock(&quest_ip);
                let next = ip.map_or(GameState::NotRunning, super::quest_net::api_state);
                if store(&quest, next) {
                    notify();
                }
                std::thread::sleep(Duration::from_secs(2));
            })
            .expect("spawn quest monitor");
        let me = m.clone();
        std::thread::Builder::new()
            .name("game-monitor".into())
            .spawn(move || {
                let mut scanner = Scanner::new();
                loop {
                    if store(&me.local, me.poll(&mut scanner)) {
                        on_change();
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .expect("spawn game monitor");
        m
    }

    /// One scan: which games run, which is ours, and the clients' state.
    fn poll(&self, scanner: &mut Scanner) -> Local {
        let (games, parents) = scanner.scan();
        let now = unix_now();
        // Linux: the game Steam's shortcut started through `--play` is ours too.
        let play = crate::core::linux::playing().filter(|p| parents.contains_key(&p.pid));
        let (ours_games, version) = {
            let mut guard = lock(&self.ours);
            if let Some(p) = &play {
                if guard.as_ref().is_none_or(|o| o.version != p.version) {
                    *guard = Some(Ours {
                        version: p.version.clone(),
                        ..Ours::default()
                    });
                }
            }
            if let Some(o) = guard.as_mut() {
                let mut roots: Vec<u32> = play.iter().map(|p| p.pid).collect();
                roots.extend(play.as_ref().and_then(|p| p.reaper));
                roots.extend(o.starter.filter(|_| o.pending(now)));
                if adopt(o, &games, &parents, &roots, now) {
                    save_ours(Some(o));
                }
            }
            if guard
                .as_ref()
                .is_some_and(|o| o.games.is_empty() && !o.pending(now) && play.is_none())
            {
                *guard = None;
                save_ours(None);
            }
            match guard.as_ref() {
                Some(o) => (o.games.clone(), Some(o.version.clone())),
                None => (Vec::new(), None),
            }
        };
        let (ours_n, others, servers) = summarize(&games, &ours_games);
        let client = ours_n + others > 0;
        let api = if client { poll_api() } else { Err(()) };
        Local {
            state: interpret(api, client),
            ours: version.filter(|_| ours_n > 0),
            others,
            servers,
        }
    }

    /// PLAY started version `version` from `bin` (its executable's folder): `starter` is
    /// what was spawned (`None` on Linux, where Steam starts it).
    pub fn launched(&self, starter: Option<u32>, version: &str, bin: &Path) {
        *lock(&self.ours) = Some(Ours {
            version: version.to_string(),
            starter,
            since: unix_now(),
            bin: Some(bin.to_path_buf()),
            games: Vec::new(),
        });
    }

    /// Ends the game the launcher started (and only that). Returns how many processes
    /// were ended.
    pub fn stop_ours(&self) -> usize {
        let games = lock(&self.ours)
            .as_ref()
            .map(|o| o.games.clone())
            .unwrap_or_default();
        stop(&games)
    }

    /// Where the Quest is on the network (`None`: not known, nothing polled).
    pub fn set_quest_ip(&self, ip: Option<std::net::Ipv4Addr>) {
        *lock(&self.quest_ip) = ip;
        if ip.is_none() {
            store(&self.quest, GameState::NotRunning);
        }
    }

    /// Echo VR on the Quest, from its API over the network.
    pub fn quest(&self) -> GameState {
        lock(&self.quest).clone()
    }

    /// Echo on this PC: its clients' state, the launcher's own game, servers.
    pub fn local(&self) -> Local {
        lock(&self.local).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_the_loader_turning_it_down() {
        // A player's starts with antivirus taking BugSplat64.dll (beta.4).
        for code in [
            0xC000_0135_u32,
            0xC000_0022,
            0xC000_0043,
            0xC000_0906,
            0xC000_0428,
        ] {
            assert!(loader_failure(code as i32).is_some(), "{code:#x}");
        }
        for code in [0, 1, 0xC000_0005_u32 as i32, 0xC000_0409_u32 as i32] {
            assert_eq!(loader_failure(code), None);
        }
    }

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    fn game(pid: u32, start: u64, role: Role, exe: &str) -> GameProcess {
        GameProcess {
            pid,
            start,
            role,
            exe: Some(PathBuf::from(exe)),
        }
    }

    #[test]
    fn state_ladder() {
        assert_eq!(interpret(Err(()), false), GameState::NotRunning);
        assert_eq!(interpret(Err(()), true), GameState::RunningApiOff);
        assert_eq!(
            interpret(Ok((404, String::new())), true),
            GameState::Running
        );
        assert_eq!(
            interpret(Ok((200, r#"{"err_code":-6}"#.into())), true),
            GameState::Running
        );
        assert_eq!(
            interpret(
                Ok((
                    200,
                    r#"{"sessionid":"ABCDEF12-0000","game_status":"playing"}"#.into()
                )),
                true
            ),
            GameState::InMatch {
                session_id: "ABCDEF12-0000".into()
            }
        );
        assert_eq!(
            GameState::InMatch {
                session_id: "ABCDEF1234".into()
            }
            .label(),
            "In a match (ABCDEF12)"
        );
    }

    #[test]
    fn knows_the_game_from_its_wrappers() {
        assert!(is_game("echovr.exe", &[]));
        assert!(is_game("EchoArena.exe", &[]));
        assert!(is_game("echovr_openxr.exe", &[]));
        // Proton: the name is cut off, the command line is the Windows one.
        assert!(is_game(
            "echovr_openxr.e",
            &args(r"Z:\home\u\Echo\ready-at-dawn-echo-arena\bin\win10\echovr_openxr.exe -noovr")
        ));
        assert!(is_game(
            "wine64-preloader",
            &args(r"/usr/bin/wine64-preloader C:\Echo\bin\win10\echovr.exe")
        ));
        // Wrappers that carry the game's path along aren't the game.
        assert!(!is_game(
            "ReviveInjector.exe",
            &args(r"C:\Revive\ReviveInjector.exe C:\Echo\bin\win10\echovr.exe /app x")
        ));
        assert!(!is_game(
            "python3",
            &args("python3 /p/proton waitforexitandrun /g/bin/win10/EchoXR.exe -noovr")
        ));
        assert!(!is_game(
            "EchoXR.exe",
            &args(r"Z:\g\bin\win10\EchoXR.exe -noovr")
        ));
        assert!(!is_game("echovr.exe.bak", &[]));
        // GE-Proton starts the game through umu.exe, run by Wine's preloader.
        let umu = args(r"c:\windows\system32\umu.exe /home/u/g/bin/win10/echovr.exe -noovr");
        assert!(!is_game("wine-preloader", &umu));
        assert!(!is_game("umu.exe", &umu));
        // ...as first seen, before Wine rewrites its command line.
        let early = args("/p/wine-preloader /p/wine c:\\windows\\system32\\umu.exe /g/bin/win10/echovr.exe -noovr");
        assert!(!is_game("wine-preloader", &early));
        // The game itself, as first seen.
        let game = args("/p/wine64-preloader /p/wine X:\\g\\bin\\win10\\echovr.exe -noovr");
        assert!(is_game("wine64-preloader", &game));
    }

    #[test]
    fn tells_servers_from_clients() {
        let exe = r"C:\Echo\bin\win10\echovr.exe";
        assert_eq!(classify(&args(exe), false), Role::Client);
        assert_eq!(
            classify(
                &args(&format!("{exe} -noovr -spectatorstream -windowed")),
                false
            ),
            Role::Client
        );
        assert_eq!(
            classify(&args(&format!("{exe} -server -headless -noovr")), false),
            Role::Server
        );
        assert_eq!(
            classify(&args(&format!("{exe} -HEADLESS")), false),
            Role::Server
        );
        // A lobby build blanks its -server: the plugin tells.
        assert_eq!(classify(&args(exe), true), Role::Server);
        // The program itself is never a flag.
        assert_eq!(classify(&args("-server"), false), Role::Client);
    }

    #[test]
    fn folders() {
        let exe = Path::new("C:/Echo/ready-at-dawn-echo-arena/bin/win10/echovr.exe");
        assert!(in_folder(exe, "C:/Echo"));
        assert!(in_folder(exe, r"c:\echo\"));
        assert!(!in_folder(exe, "C:/Ech"));
        assert!(!in_folder(exe, ""));
        assert_eq!(
            game_path(None, &args(r"Z:\home\u\bin\win10\echovr_openxr.exe -noovr")),
            Some(PathBuf::from("/home/u/bin/win10/echovr_openxr.exe"))
        );
        let local = Local {
            servers: vec![Some(exe.to_path_buf())],
            ..Local::default()
        };
        assert!(local.server_in("C:/Echo"));
        assert!(!local.server_in("D:/Other"));
        let unknown = Local {
            servers: vec![None],
            ..Local::default()
        };
        assert!(unknown.server_in("D:/Other"));
    }

    #[test]
    fn adopts_what_play_started() {
        let bin = "C:/E/ready-at-dawn-echo-arena/bin/win10";
        let mut ours = Ours {
            version: "pc-latest".into(),
            starter: Some(10),
            since: 1000,
            bin: Some(PathBuf::from(bin)),
            games: Vec::new(),
        };
        // 10 = EchoXR.exe (or Revive's injector), 11 its game; 20 a client started
        // elsewhere; 30 a server.
        let parents = HashMap::from([(10, 1), (11, 10), (20, 2), (30, 3)]);
        let games = vec![
            game(11, 1001, Role::Client, &format!("{bin}/echovr_openxr.exe")),
            game(20, 900, Role::Client, &format!("{bin}/echovr.exe")),
            game(30, 1002, Role::Server, &format!("{bin}/echovr.exe")),
        ];
        assert!(adopt(&mut ours, &games, &parents, &[10], 1005));
        assert_eq!(
            ours.games,
            [Owned {
                pid: 11,
                start: 1001
            }]
        );
        assert_eq!(summarize(&games, &ours.games).0, 1);
        assert_eq!(summarize(&games, &ours.games).1, 1);
        assert_eq!(summarize(&games, &ours.games).2.len(), 1);
        // Nothing new, and the game stays ours once its starter is gone.
        assert!(!adopt(&mut ours, &games, &HashMap::new(), &[], 2000));
        assert_eq!(ours.games.len(), 1);
        // It ended (or its pid now belongs to another process): forgotten.
        let later = vec![game(11, 3000, Role::Client, "C:/x/echovr.exe")];
        assert!(adopt(&mut ours, &later, &HashMap::new(), &[], 3001));
        assert!(ours.games.is_empty());
    }

    #[test]
    fn falls_back_to_the_folder_while_starting() {
        let bin = "C:/E/ready-at-dawn-echo-arena/bin/win10";
        let mut ours = Ours {
            version: "pc-latest".into(),
            since: 1000,
            bin: Some(PathBuf::from(bin)),
            ..Ours::default()
        };
        let games = vec![
            // Started before PLAY: not ours.
            game(5, 990, Role::Client, &format!("{bin}/echovr.exe")),
            // Another install's: not ours.
            game(6, 1003, Role::Client, "C:/Other/bin/win10/echovr.exe"),
            // A server from the same folder: not ours.
            game(7, 1003, Role::Server, &format!("{bin}/echovr.exe")),
            game(8, 1004, Role::Client, &format!("{bin}/echovr.exe")),
        ];
        assert!(adopt(&mut ours, &games, &HashMap::new(), &[], 1010));
        assert_eq!(
            ours.games,
            [Owned {
                pid: 8,
                start: 1004
            }]
        );
        // After the wait, nothing is taken by its folder any more.
        let mut late = Ours {
            games: Vec::new(),
            ..ours
        };
        assert!(!adopt(&mut late, &games, &HashMap::new(), &[], 1100));
    }

    #[test]
    fn remembers_ours() {
        let o = Ours {
            version: "pc-latest".into(),
            starter: Some(4),
            since: 5,
            bin: Some(PathBuf::from("C:/E/bin/win10")),
            games: vec![Owned { pid: 9, start: 7 }],
        };
        let back: Ours = serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(back, o);
    }
}
