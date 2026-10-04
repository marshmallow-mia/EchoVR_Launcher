//! Single home for invoking the bundled `adb`.
//!
//! Every invocation goes through [`exec`], which
//! * traces the call (command, exit code, duration, bounded output) under `adb[n]`
//!   markers, because the install, the updater and the connection check run on
//!   different threads and their lines interleave;
//! * pins the device: a serial is resolved once from `adb devices -l` and passed as
//!   `-s <serial>` to every subcommand that accepts one;
//! * names failures: output is scanned for adb's device-identity errors.
//!
//! A serial the user picked in the device picker is remembered separately from the
//! heuristically resolved one, so re-probing (after `kill-server`, a reconnect, or the
//! next connection check) keeps talking to the headset the user chose. In the Java
//! version every probe overwrote the choice, so picking a device never stuck.

pub mod bundle;
pub mod devices;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use devices::{Device, Selection, Status};

/// Android package name of Echo VR on Quest.
pub const PACKAGE: &str = "com.readyatdawn.r15";

/// Subcommands that address the server rather than a device; `-s` is invalid there.
const NO_SERIAL: [&str; 7] = [
    "devices",
    "start-server",
    "kill-server",
    "version",
    "help",
    "connect",
    "disconnect",
];

struct State {
    /// Chosen by the user in the picker; survives re-probes while the device is attached.
    preferred: Option<String>,
    /// What device commands are currently pinned to.
    target: Option<String>,
    resolved: bool,
    last: Selection,
}

static STATE: Mutex<State> = Mutex::new(State {
    preferred: None,
    target: None,
    resolved: false,
    last: Selection {
        serial: None,
        all: Vec::new(),
        reason: String::new(),
        status: Status::None,
    },
});
static SEQ: AtomicU64 = AtomicU64::new(0);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

/// True when `-s <serial>` belongs before this subcommand.
pub fn takes_serial(subcommand: &str) -> bool {
    !subcommand.is_empty() && !subcommand.starts_with('-') && !NO_SERIAL.contains(&subcommand)
}

/// Serials are USB serials, `emulator-NNNN` or `host:port` -- never whitespace-bearing.
fn usable_serial(serial: &str) -> bool {
    !serial.is_empty() && !serial.chars().any(char::is_whitespace)
}

/// argv for `adb <args...>` (without the binary), with `-s <serial>` inserted before the
/// subcommand when one applies. `-s` is a global option, so position matters.
pub fn build_argv(serial: Option<&str>, args: &[&str]) -> Vec<String> {
    let mut argv = Vec::with_capacity(args.len() + 2);
    if let (Some(first), Some(s)) = (args.first(), serial) {
        if takes_serial(first) && usable_serial(s) {
            argv.push("-s".to_string());
            argv.push(s.to_string());
        }
    }
    argv.extend(args.iter().map(|a| a.to_string()));
    argv
}

/// Device-identity failures: the pinned target is wrong or gone.
const IDENTITY_MARKERS: [&str; 4] = [
    "more than one device",
    "no devices/emulators found",
    "device offline",
    "device unauthorized",
];
const OTHER_MARKERS: [&str; 5] = [
    "adb: error:",
    "no permissions",
    "protocol fault",
    "error: closed",
    "cannot connect to daemon",
];

/// The matched problem marker, or `None` when the output looks clean.
pub fn diagnose(output: &str) -> Option<&'static str> {
    let lower = output.to_lowercase();
    if let Some(m) = IDENTITY_MARKERS.iter().find(|m| lower.contains(*m)) {
        return Some(m);
    }
    if lower.contains("device '") && lower.contains("' not found") {
        return Some("device '...' not found");
    }
    OTHER_MARKERS.iter().find(|m| lower.contains(*m)).copied()
}

pub fn is_identity_problem(marker: &str) -> bool {
    marker == "device '...' not found" || IDENTITY_MARKERS.contains(&marker)
}

#[derive(Debug, Clone, Default)]
pub struct AdbResult {
    pub code: Option<i32>,
    /// stdout + stderr.
    pub output: String,
}

impl AdbResult {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// The serial device commands are pinned to, resolving it on first use.
fn target_serial() -> Option<String> {
    {
        let s = state();
        if s.resolved {
            return s.target.clone();
        }
    }
    probe().serial
}

/// Runs `adb <args...>` through the trace.
pub fn exec(args: &[&str]) -> AdbResult {
    let bin = match bundle::binary() {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("adb: unavailable: {e:#}");
            return AdbResult {
                code: None,
                output: format!("adb unavailable: {e}"),
            };
        }
    };
    let sub = args.first().copied().unwrap_or("");
    let serial = if takes_serial(sub) {
        target_serial()
    } else {
        None
    };
    let argv = build_argv(serial.as_deref(), args);
    let id = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    tracing::info!("adb[{id}] > {}", argv.join(" "));

    let out = crate::core::process::run(&bin, &argv, None);
    let output = match &out.spawn_error {
        Some(e) => format!("could not start adb: {e}"),
        None => out.combined(),
    };
    let ms = out.duration.as_millis();
    tracing::info!("adb[{id}] < exit={:?} in {ms}ms", out.code);
    let body = if out.success() {
        crate::core::process::excerpt(&output)
    } else {
        crate::core::process::normalize(&output)
    };
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        tracing::info!("adb[{id}] | {line}");
    }

    after_run(id, sub, &output);
    AdbResult {
        code: out.code,
        output,
    }
}

fn after_run(id: u64, sub: &str, output: &str) {
    // Restarting the server reshuffles transports; the resolved serial is stale (the
    // user's preference is not -- it is re-applied by the next probe).
    if sub == "kill-server" || sub == "start-server" {
        clear_target(&format!("adb {sub}"));
    }
    let Some(problem) = diagnose(output) else {
        return;
    };
    tracing::warn!("ADB-PROBLEM: {problem} (raised by adb[{id}])");
    let had_target = {
        let s = state();
        tracing::warn!("ADB-PROBLEM: target serial was {:?}", s.target);
        for d in &s.last.all {
            tracing::warn!("ADB-PROBLEM:   {}", d.describe());
        }
        s.target.is_some()
    };
    if is_identity_problem(problem) {
        clear_target(&format!("device-identity error: {problem}"));
        if had_target && sub != "devices" {
            tracing::warn!("ADB-PROBLEM: re-reading the device table now");
            probe();
        }
    }
}

/// Runs a shell script on the device. The script is a single argv element, so only the
/// device's `sh` splits it.
pub fn shell(script: &str) -> AdbResult {
    exec(&["shell", script])
}

/// Pushes a local file to an absolute remote *file* path.
pub fn push_file(local: &Path, remote: &str) -> bool {
    let local = local.to_string_lossy();
    let r = exec(&["push", &local, remote]);
    r.success() && pushed_data(&r.output)
}

/// True when adb's push summary reports a transfer.
pub fn pushed_data(output: &str) -> bool {
    output.contains("bytes") && !output.contains("0 files pushed")
}

pub fn pull(remote: &str, local: &Path) -> bool {
    exec(&["pull", remote, &local.to_string_lossy()]).success()
}

/// Runs `start-server` then `devices -l`, decides which device to target and caches it.
pub fn probe() -> Selection {
    exec(&["start-server"]);
    let out = exec(&["devices", "-l"]);
    let preferred = state().preferred.clone();
    let sel = devices::select(devices::parse(&out.output), preferred.as_deref());
    for d in &sel.all {
        tracing::info!("adb-devices =   {}", d.describe());
    }
    tracing::info!(
        "adb-devices: status={:?} serial={:?}: {}",
        sel.status,
        sel.serial,
        sel.reason
    );
    let mut s = state();
    s.target = sel.serial.clone();
    s.resolved = true;
    s.last = sel.clone();
    sel
}

/// Fresh connection check: re-reads the device table (never answers from a cached serial,
/// since a different headset may have been plugged in).
pub fn connection_status() -> Status {
    clear_target("connection check");
    probe().status
}

/// Pins a serial chosen by the user.
pub fn set_preferred(serial: &str) {
    let mut s = state();
    s.preferred = Some(serial.to_string());
    s.target = Some(serial.to_string());
    s.resolved = true;
    if let Some(sel) = Some(&mut s.last) {
        if sel.all.iter().any(|d| d.serial == serial) {
            sel.serial = Some(serial.to_string());
            sel.status = Status::Ready;
        }
    }
    tracing::info!("adb: target serial set to {serial} (chosen by the user)");
}

pub fn clear_target(why: &str) {
    let mut s = state();
    if s.resolved {
        tracing::info!("adb: target serial cleared -- {why}");
    }
    s.target = None;
    s.resolved = false;
}

pub fn last_selection() -> Selection {
    state().last.clone()
}

/// The device commands are pinned to, from the last probe. Display only.
pub fn target_device() -> Option<Device> {
    let s = state();
    let serial = s.target.clone().or_else(|| s.last.serial.clone())?;
    s.last.all.iter().find(|d| d.serial == serial).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_goes_before_device_subcommands_only() {
        assert_eq!(
            build_argv(Some("ABC"), &["shell", "ls"]),
            ["-s", "ABC", "shell", "ls"]
        );
        assert_eq!(
            build_argv(Some("ABC"), &["devices", "-l"]),
            ["devices", "-l"]
        );
        assert_eq!(build_argv(Some("ABC"), &["kill-server"]), ["kill-server"]);
        assert_eq!(
            build_argv(None, &["install", "-g", "x.apk"]),
            ["install", "-g", "x.apk"]
        );
        assert_eq!(build_argv(Some("A B"), &["shell", "ls"]), ["shell", "ls"]);
        assert_eq!(
            build_argv(Some("ABC"), &["-s", "X", "shell"]),
            ["-s", "X", "shell"]
        );
        assert_eq!(
            build_argv(Some("h:5555"), &["push", "a", "b"]),
            ["-s", "h:5555", "push", "a", "b"]
        );
    }

    #[test]
    fn diagnoses_identity_problems() {
        assert_eq!(
            diagnose("adb: error: more than one device/emulator"),
            Some("more than one device")
        );
        assert!(is_identity_problem(
            diagnose("error: device 'XYZ' not found").unwrap()
        ));
        assert!(is_identity_problem(
            diagnose("error: device offline").unwrap()
        ));
        let other = diagnose("adb: error: failed to stat").unwrap();
        assert!(!is_identity_problem(other));
        assert_eq!(diagnose("Success"), None);
    }

    #[test]
    fn push_summary() {
        assert!(pushed_data(
            "_data.zip: 1 file pushed, 0 skipped. 38.2 MB/s (123 bytes in 0.1s)"
        ));
        assert!(!pushed_data("adb: error: failed to copy"));
        assert!(!pushed_data("0 files pushed. 123 bytes"));
    }
}
