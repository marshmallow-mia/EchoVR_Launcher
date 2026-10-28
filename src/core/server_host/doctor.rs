//! The Doctor: what a server needs, checked one by one, each with what fixes it. After
//! evr-docker's (`internal/core/doctor.go`), for a desktop: the game and its nEVR, Proton
//! on Linux, the account, free disk, the ports, the public address, and on Windows the
//! firewall.

use std::net::{TcpListener, UdpSocket};

use super::{account, Config, SignIn};
use crate::core::launcher::store::LauncherState;
use crate::core::launcher::{mods, nevr};

/// Free disk wanted where the servers' folders are (their copies of `bin/win10`).
const DISK_MIN: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Good,
    Warn,
    Bad,
}

/// What fixes a check.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Fix {
    None,
    /// The Account part of the page.
    SignIn,
    /// Play's PLAY or Settings (install, update, Linux setup).
    Game,
    /// Windows: the firewall rule for these ports (through the admin helper).
    Firewall(Vec<u16>),
    /// Windows: the servers' folder out of Windows Security's scans.
    Defender,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub text: String,
    pub fix: Fix,
}

fn check(name: &'static str, level: Level, text: impl Into<String>, fix: Fix) -> Check {
    Check {
        name,
        level,
        text: text.into(),
        fix,
    }
}

/// Runs every check (it takes a few seconds: EchoVRCE and the public address are asked).
pub fn run(c: &Config, running: &[String]) -> Vec<Check> {
    let mut out = Vec::new();
    let state = LauncherState::load();
    match c.version_in(&state) {
        None => out.push(check(
            "Game",
            Level::Bad,
            "No live build is installed (event builds can't host servers).",
            Fix::Game,
        )),
        Some(v) => {
            let bin = v.bin_dir();
            let dll = std::fs::read(bin.join("BugSplat64.dll")).ok();
            match dll.as_deref().and_then(nevr::version_in) {
                Some(ver) if mods::nevr_in(&bin) => out.push(check(
                    "Game",
                    Level::Good,
                    format!("{} with nEVR {ver}.", v.name),
                    Fix::None,
                )),
                _ => out.push(check(
                    "Game",
                    Level::Bad,
                    format!(
                        "{} has no nEVR in its slot: update the game on Play.",
                        v.name
                    ),
                    Fix::Game,
                )),
            }
        }
    }
    #[cfg(target_os = "linux")]
    out.extend(linux_checks());
    if cfg!(target_os = "linux") {
        if crate::core::linux::echoxr::proton_script().is_file() {
            out.push(check(
                "Proton",
                Level::Good,
                "GE-Proton is set up.",
                Fix::None,
            ));
        } else {
            out.push(check(
                "Proton",
                Level::Bad,
                "Echo VR isn't set up on Linux yet: PLAY it once (it sets up GE-Proton).",
                Fix::Game,
            ));
        }
    }
    out.push(account_check(c));
    out.push(disk_check());
    out.extend(port_checks(c, running));
    out.push(public_ip_check(c));
    #[cfg(windows)]
    {
        out.push(firewall_check(c));
        if let Some(c) = defender_check(c) {
            out.push(c);
        }
    }
    out
}

/// Windows: a server's copy of nEVR that went missing (Windows Security takes it away on
/// some PCs).
#[cfg(windows)]
fn defender_check(c: &Config) -> Option<Check> {
    let gone: Vec<&str> = c
        .servers
        .iter()
        .filter(|s| {
            let bin = super::farm::bin_in(&super::farm_root(&s.dir()));
            bin.is_dir()
                && (!bin.join(mods::SLOT).is_file()
                    || matches!(
                        mods::slot_facts(&bin).0,
                        mods::Loader::NevrIncomplete { .. }
                    ))
        })
        .map(|s| s.name.as_str())
        .collect();
    (!gone.is_empty()).then(|| {
        check(
            "Windows Security",
            Level::Bad,
            format!(
                "nEVR (BugSplat64.dll or nevr-runtime.dll) was taken out of {}'s folder: let Windows Security leave the servers' folder alone.",
                gone.join(", ")
            ),
            Fix::Defender,
        )
    })
}

/// Pure: whether glibc `version` ("2.36") is at least `major.minor`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn glibc_at_least(version: &str, major: u32, minor: u32) -> bool {
    let mut parts = version
        .trim()
        .split('.')
        .map(|p| p.parse::<u32>().unwrap_or(0));
    let (a, b) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    (a, b) >= (major, minor)
}

/// Linux: what the launcher's GE-Proton needs from the system: glibc 2.38 or newer (its
/// Wine won't load on older ones, such as Debian 12), and the Vulkan loader (its start
/// script loads it even for a server).
#[cfg(target_os = "linux")]
fn linux_checks() -> Vec<Check> {
    let mut out = Vec::new();
    let glibc = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) }
        .to_string_lossy()
        .into_owned();
    if !glibc_at_least(&glibc, 2, 38) {
        out.push(check(
            "System",
            Level::Bad,
            format!("This system's glibc is {glibc}: GE-Proton needs 2.38 or newer (Debian 13, Ubuntu 24.04, Fedora 39 and newer)."),
            Fix::None,
        ));
    }
    let name = std::ffi::CString::new("libvulkan.so.1").unwrap_or_default();
    let h = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY) };
    if h.is_null() {
        out.push(check(
            "Vulkan",
            Level::Bad,
            "The Vulkan loader (libvulkan.so.1) is missing: Proton won't start without it. Install it (libvulkan1 or vulkan-loader).",
            Fix::None,
        ));
    } else {
        unsafe { libc::dlclose(h) };
    }
    out
}

fn account_check(c: &Config) -> Check {
    match c.account.sign_in {
        SignIn::None => check(
            "Account",
            Level::Bad,
            "The servers have no account yet.",
            Fix::SignIn,
        ),
        sign_in => match account::session(c) {
            Ok(t) => {
                let guilds = account::host_guilds(&t.token);
                let who = match c.account.username.as_str() {
                    "" => c.account.discord_id.clone(),
                    u => u.to_string(),
                };
                let text = match (sign_in, guilds.is_empty()) {
                    (SignIn::Password, false) => {
                        format!("Signs in as {who}; may host for {} guild(s).", guilds.len())
                    }
                    (SignIn::Password, true) => format!(
                        "Signs in as {who}, but no guild gave it the server host role: it hosts public matches only."
                    ),
                    _ => format!("Signs in as {who} (linked)."),
                };
                check("Account", Level::Good, text, Fix::None)
            }
            Err(e) => check("Account", Level::Bad, format!("{e:#}"), Fix::SignIn),
        },
    }
}

fn disk_check() -> Check {
    let dir = super::dir();
    let _ = std::fs::create_dir_all(&dir);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let free = disks
        .list()
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space());
    match free {
        Some(f) if f < DISK_MIN => check(
            "Disk",
            Level::Warn,
            format!(
                "{} GB free where the servers' folders are: each server keeps a copy of the game's bin folder.",
                f / (1024 * 1024 * 1024)
            ),
            Fix::None,
        ),
        Some(f) => check(
            "Disk",
            Level::Good,
            format!("{} GB free.", f / (1024 * 1024 * 1024)),
            Fix::None,
        ),
        None => check("Disk", Level::Warn, "Couldn't tell the free disk space.", Fix::None),
    }
}

/// Pure-ish: whether `port` can be bound (UDP and TCP) on this PC.
pub fn port_free(port: u16) -> bool {
    UdpSocket::bind(("0.0.0.0", port)).is_ok() && TcpListener::bind(("0.0.0.0", port)).is_ok()
}

fn port_checks(c: &Config, running: &[String]) -> Vec<Check> {
    let mut out = Vec::new();
    let taken: Vec<String> = c
        .servers
        .iter()
        .filter(|s| !running.contains(&s.name))
        .filter(|s| !port_free(s.port))
        .map(|s| format!("{} ({})", s.port, s.name))
        .collect();
    if c.servers.is_empty() {
        return out;
    }
    if taken.is_empty() {
        out.push(check(
            "Ports",
            Level::Good,
            "The stopped servers' ports are free.",
            Fix::None,
        ));
    } else {
        out.push(check(
            "Ports",
            Level::Bad,
            format!(
                "In use by another program: {}. Give the server another port.",
                taken.join(", ")
            ),
            Fix::None,
        ));
    }
    out
}

fn public_ip_check(c: &Config) -> Check {
    let ip = ["https://api.ipify.org", "https://ifconfig.me/ip"]
        .iter()
        .find_map(|u| crate::core::http::get_text(u).ok())
        .map(|t| t.trim().to_string())
        .filter(|t| t.parse::<std::net::IpAddr>().is_ok());
    let ports: Vec<String> = c.servers.iter().map(|s| s.port.to_string()).collect();
    match ip {
        Some(ip) => check(
            "Address",
            Level::Good,
            format!(
                "This PC is {ip} on the internet. Forward each server's port (UDP and TCP) to this PC with the same number outside{}: EchoVRCE checks it when a server registers.",
                if ports.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", ports.join(", "))
                }
            ),
            Fix::None,
        ),
        None => check(
            "Address",
            Level::Warn,
            "Couldn't tell this PC's address on the internet.",
            Fix::None,
        ),
    }
}

/// The firewall rule's name (Windows).
pub const FIREWALL_RULE: &str = "Echo VR servers (Echo VR Launcher)";

/// Pure: the ports in `netsh … show rule` output's `LocalPort:` lines.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn rule_ports(text: &str) -> Vec<u16> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("LocalPort:"))
        .flat_map(|p| p.split(','))
        .filter_map(|p| p.trim().parse().ok())
        .collect()
}

#[cfg(windows)]
fn firewall_check(c: &Config) -> Check {
    let ports: Vec<u16> = c.servers.iter().map(|s| s.port).collect();
    if ports.is_empty() {
        return check("Firewall", Level::Good, "No servers yet.", Fix::None);
    }
    let out = crate::core::process::command("netsh")
        .args(["advfirewall", "firewall", "show", "rule"])
        .arg(format!("name={FIREWALL_RULE}"))
        .output();
    let have = out
        .map(|o| rule_ports(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    let missing: Vec<u16> = ports
        .iter()
        .copied()
        .filter(|p| !have.contains(p))
        .collect();
    if missing.is_empty() {
        check(
            "Firewall",
            Level::Good,
            "Windows Firewall lets the servers' ports in.",
            Fix::None,
        )
    } else {
        check(
            "Firewall",
            Level::Warn,
            format!(
                "Windows Firewall may block port(s) {}.",
                missing
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Fix::Firewall(ports),
        )
    }
}

/// The servers' folders, for Windows Security's exclusions.
pub fn servers_folder() -> std::path::PathBuf {
    super::dir().join("servers")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_an_old_glibc() {
        assert!(glibc_at_least("2.38", 2, 38));
        assert!(glibc_at_least("2.41", 2, 38));
        assert!(!glibc_at_least("2.36", 2, 38));
        assert!(glibc_at_least("3.0", 2, 38));
    }

    #[test]
    fn reads_the_firewall_rules_ports() {
        let text = "Rule Name:   Echo VR servers (Echo VR Launcher)\n----\nProtocol:  UDP\nLocalPort:   6792,6793\n\nProtocol: TCP\nLocalPort: 6792\n";
        assert_eq!(rule_ports(text), vec![6792, 6793, 6792]);
        assert!(rule_ports("No rules match the specified criteria.").is_empty());
    }
}
