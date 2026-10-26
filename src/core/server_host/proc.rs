//! A server's game process: started so it can be stopped the way nEVR wants (Ctrl+C:
//! it unregisters the server first), and so nothing of it is left behind.
//!
//! - **Linux:** Proton in a process group of its own. Ctrl+C is SIGINT to the game's Wine
//!   process (it becomes CTRL+C inside the game), found by its path as Wine runs it: Wine
//!   gives a Windows program a process group of its own. The end kills Proton's group and
//!   the server's wineserver (everything in its prefix).
//! - **Windows:** echovr.exe in a Job Object that ends it with the supervisor. nEVR opens a
//!   console window of its own for a server, which is hidden; Ctrl+C reaches the game
//!   through it (`GenerateConsoleCtrlEvent` while attached to it).

use std::process::{Child, Command, ExitStatus};

use anyhow::{Context, Result};

/// A started game process.
pub struct Game {
    child: Child,
    #[cfg(windows)]
    job: win::Job,
    #[cfg(windows)]
    hidden: bool,
    /// Linux: the server's prefix, for its wineserver at the end.
    #[cfg(unix)]
    prefix: Option<std::path::PathBuf>,
    #[cfg(unix)]
    wineserver: Option<std::path::PathBuf>,
    /// Linux: the game's path as Wine runs it (`Z:\…\echovr.exe`), unique to the server.
    #[cfg(unix)]
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    wine_exe: String,
}

/// Linux: the server's Wine prefix, its wineserver and the game's path in it.
#[derive(Default)]
#[cfg_attr(windows, allow(dead_code))]
pub struct Wine {
    pub prefix: std::path::PathBuf,
    pub wineserver: std::path::PathBuf,
    pub exe: String,
}

impl Game {
    /// Starts `cmd` (Linux: through Proton, in `wine`).
    #[allow(unused_variables)]
    pub fn start(mut cmd: Command, wine: Option<Wine>) -> Result<Game> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
            let child = cmd.spawn().context("couldn't start the game")?;
            let w = wine.unwrap_or_default();
            Ok(Game {
                child,
                prefix: Some(w.prefix).filter(|p| !p.as_os_str().is_empty()),
                wineserver: Some(w.wineserver).filter(|p| !p.as_os_str().is_empty()),
                wine_exe: w.exe,
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
            let job = win::Job::new()?;
            let child = cmd.spawn().context("couldn't start the game")?;
            job.assign(&child);
            Ok(Game {
                child,
                job,
                hidden: false,
            })
        }
    }

    /// Its output and error output, when they were piped (once).
    pub fn take_output(
        &mut self,
    ) -> Option<(std::process::ChildStdout, std::process::ChildStderr)> {
        Some((self.child.stdout.take()?, self.child.stderr.take()?))
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Its exit, if it has ended.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Windows: hides the console window nEVR opened (once it is there); true when done.
    pub fn hide_console(&mut self) -> bool {
        #[cfg(windows)]
        {
            if !self.hidden {
                self.hidden = win::hide_console(self.child.id());
            }
            self.hidden
        }
        #[cfg(not(windows))]
        true
    }

    /// Ctrl+C: nEVR unregisters the server and shuts the game down.
    pub fn interrupt(&self) {
        #[cfg(target_os = "linux")]
        {
            let games = linux::games(&self.wine_exe);
            if games.is_empty() {
                // Not running as the game yet: Proton's group.
                unsafe { libc::kill(-(self.child.id() as i32), libc::SIGINT) };
            }
            for pid in games {
                unsafe { libc::kill(pid, libc::SIGINT) };
            }
        }
        // (macOS builds run no servers.)
        #[cfg(windows)]
        win::ctrl_c(self.child.id());
    }

    /// Ends it and everything it started.
    pub fn kill(&mut self) {
        #[cfg(unix)]
        {
            #[cfg(target_os = "linux")]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.kill();
            if let (Some(ws), Some(prefix)) = (&self.wineserver, &self.prefix) {
                if ws.is_file() && prefix.is_dir() {
                    let _ = Command::new(ws)
                        .arg("-k")
                        .env("WINEPREFIX", prefix)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
            }
        }
        #[cfg(windows)]
        {
            self.job.terminate();
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "linux")]
mod linux {
    /// The processes whose command line starts with `exe` (the game's path as Wine runs
    /// it): the game itself, not Proton or umu.exe, which name it as a Unix path.
    pub fn games(exe: &str) -> Vec<i32> {
        if exe.is_empty() {
            return Vec::new();
        }
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        dir.flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
            .filter(|&pid| {
                std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| {
                    let first = c.split(|b| *b == 0).next().unwrap_or_default();
                    super::is_game(&String::from_utf8_lossy(first), exe)
                })
            })
            .collect()
    }
}

/// Pure: whether a process whose command line starts with `first` is the game at `exe` (as
/// Wine runs it; any case, either slash).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_game(first: &str, exe: &str) -> bool {
    let n = |s: &str| s.replace('/', "\\").to_ascii_lowercase();
    !exe.is_empty() && n(first.trim()) == n(exe)
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_the_game_by_its_wine_path() {
        let exe = r"Z:\data\server-host\servers\echo-1\root\bin\win10\echovr.exe";
        assert!(super::is_game(exe, exe));
        assert!(super::is_game(&exe.to_lowercase(), exe));
        assert!(!super::is_game(
            r"Z:\data\server-host\servers\echo-2\root\bin\win10\echovr.exe",
            exe
        ));
        assert!(!super::is_game("c:\\windows\\system32\\umu.exe", exe));
        assert!(!super::is_game("", ""));
    }
}

#[cfg(windows)]
mod win {
    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, GetConsoleWindow,
        SetConsoleCtrlHandler, CTRL_C_EVENT,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

    /// A process can be attached to one console at a time: one server's at a time.
    static CONSOLE: Mutex<()> = Mutex::new(());

    /// A Job Object that ends its processes when it is closed (the supervisor gone).
    pub struct Job(HANDLE);

    // The handle is only used through the Win32 calls, which are thread-safe.
    unsafe impl Send for Job {}

    impl Job {
        pub fn new() -> anyhow::Result<Job> {
            let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if h.is_null() {
                anyhow::bail!("couldn't create a job object");
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            unsafe {
                SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
            }
            Ok(Job(h))
        }

        pub fn assign(&self, child: &std::process::Child) {
            use std::os::windows::io::AsRawHandle;
            let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
            if ok == 0 {
                tracing::warn!("server host: the game isn't in a job object");
            }
        }

        pub fn terminate(&self) {
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Runs `f` attached to `pid`'s console; false when it has none (yet).
    fn attached(pid: u32, f: impl FnOnce()) -> bool {
        let _held = CONSOLE.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            FreeConsole();
            if AttachConsole(pid) == 0 {
                return false;
            }
        }
        f();
        unsafe { FreeConsole() };
        true
    }

    /// Hides `pid`'s console window; false when it has none yet.
    pub fn hide_console(pid: u32) -> bool {
        let mut done = false;
        attached(pid, || {
            let w = unsafe { GetConsoleWindow() };
            if !w.is_null() {
                unsafe { ShowWindow(w, SW_HIDE) };
                done = true;
            }
        });
        done
    }

    /// Ctrl+C into `pid`'s console (this process ignores it meanwhile).
    pub fn ctrl_c(pid: u32) {
        let sent = attached(pid, || unsafe {
            SetConsoleCtrlHandler(None, 1);
            GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0);
            // The event is delivered on a thread of the game's: give it a moment before
            // this process leaves the console.
            std::thread::sleep(std::time::Duration::from_millis(200));
            SetConsoleCtrlHandler(None, 0);
        });
        if !sent {
            tracing::warn!("server host: no console to send Ctrl+C to (pid {pid})");
        }
    }
}
