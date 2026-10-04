//! The Quest over the network: its Wi-Fi address (read over USB, typed, or found by a scan
//! of the local network), Echo VR's API on it (port 6721, while the game runs with API
//! access on), and ADB over the network (port 5555, turned on once over USB).

use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Result};

use super::game::{self, GameState};
use crate::core::adb;

/// Echo VR's local API (`EnableAPIAccess`).
pub const API_PORT: u16 = 6721;
/// ADB over the network (`adb tcpip`).
pub const ADB_PORT: u16 = 5555;
/// How long a scan waits for one port of one host.
const PROBE_TIMEOUT: Duration = Duration::from_millis(350);
/// Hosts probed at once by a scan.
const SCAN_THREADS: usize = 32;

/// A typed address: an IPv4 address and nothing else (no port, no name).
pub fn parse_ip(text: &str) -> Option<Ipv4Addr> {
    text.trim().parse().ok()
}

/// The Wi-Fi address in the output of `ip -f inet addr show wlan0`.
pub fn parse_wlan_ip(output: &str) -> Option<Ipv4Addr> {
    output.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("inet ")?;
        let addr = rest.split(['/', ' ']).next()?;
        addr.parse::<Ipv4Addr>()
            .ok()
            .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
    })
}

/// Whether an adb serial is a network transport (`192.168.1.20:5555`, or a wireless
/// debugging service name) rather than USB.
pub fn is_network_serial(serial: &str) -> bool {
    serial.parse::<SocketAddr>().is_ok() || serial.contains("._tcp")
}

/// The headset's Wi-Fi address, read over USB. `None` when the device adb talks to isn't
/// on USB, or has no Wi-Fi address.
pub fn ip_over_usb() -> Option<Ipv4Addr> {
    let device = adb::target_device()?;
    if is_network_serial(&device.serial) {
        return None;
    }
    let r = adb::shell("ip -f inet addr show wlan0");
    r.success().then(|| parse_wlan_ip(&r.output)).flatten()
}

/// Echo VR's state on the Quest at `ip`, from its API. A refused or silent port reads as
/// not running: the API can't tell "not running" from "API access off".
pub fn api_state(ip: Ipv4Addr) -> GameState {
    let url = format!("http://{ip}:{API_PORT}/session");
    let answer = crate::core::http::block_on(async {
        let resp = crate::core::http::client()
            .get(&url)
            .timeout(Duration::from_millis(1500))
            .send()
            .await
            .map_err(|_| ())?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().await.unwrap_or_default()))
    });
    game::interpret(answer, false)
}

/// What `adb connect` answered: connected (or already), or why not.
pub fn connect_succeeded(output: &str) -> bool {
    let o = output.to_ascii_lowercase();
    o.contains("connected to") && !o.contains("cannot") && !o.contains("failed")
}

/// Connects ADB to the Quest at `ip` over the network.
pub fn connect(ip: Ipv4Addr) -> Result<()> {
    let r = adb::exec(&["connect", &format!("{ip}:{ADB_PORT}")]);
    if !connect_succeeded(&r.output) {
        bail!(
            "Couldn't reach your Quest at {ip} over the network: {}",
            r.output.trim()
        );
    }
    Ok(())
}

/// Drops ADB's network connection to the Quest at `ip` (USB is unaffected).
pub fn disconnect(ip: Ipv4Addr) {
    adb::exec(&["disconnect", &format!("{ip}:{ADB_PORT}")]);
}

/// Turns on ADB over the network: tells the headset on USB to listen on port 5555, then
/// connects to it at `ip`.
pub fn enable_adb_network(ip: Ipv4Addr) -> Result<()> {
    match adb::target_device() {
        Some(d) if !is_network_serial(&d.serial) => {}
        _ => bail!("Plug in your Quest by USB once to turn on ADB over the network."),
    }
    let r = adb::exec(&["tcpip", &ADB_PORT.to_string()]);
    if !r.success() {
        bail!(
            "Your Quest didn't switch on ADB over the network: {}",
            r.output.trim()
        );
    }
    // The headset's adb daemon restarts in network mode; give it a moment.
    std::thread::sleep(Duration::from_secs(2));
    connect(ip)
}

/// The PC's own address on the network its default route uses. Nothing is sent: a
/// connected UDP socket only picks the route.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    s.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// The other hosts of `ip`'s /24 (no network or broadcast address, not `ip` itself).
pub fn subnet_hosts(ip: Ipv4Addr) -> Vec<Ipv4Addr> {
    let [a, b, c, own] = ip.octets();
    (1..=254u8)
        .filter(|h| *h != own)
        .map(|h| Ipv4Addr::new(a, b, c, h))
        .collect()
}

/// A host a scan found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub ip: Ipv4Addr,
    /// Echo VR's API answers (the game runs with API access on).
    pub api: bool,
    /// ADB over the network answers.
    pub adb: bool,
}

fn open(ip: Ipv4Addr, port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((ip, port)), PROBE_TIMEOUT).is_ok()
}

/// Looks for Echo VR's API and ADB on `hosts`, a few dozen at a time; `on` gets the
/// progress (0 to 1) on the calling thread.
pub fn scan(hosts: &[Ipv4Addr], cancel: &AtomicBool, on: &mut dyn FnMut(f32)) -> Vec<Found> {
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let found = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..SCAN_THREADS.min(hosts.len()) {
            s.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(&ip) = hosts.get(i) else {
                    return;
                };
                let (api, adb) = (open(ip, API_PORT), open(ip, ADB_PORT));
                if api || adb {
                    found
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push(Found { ip, api, adb });
                }
                done.fetch_add(1, Ordering::Relaxed);
            });
        }
        while done.load(Ordering::Relaxed) < hosts.len() && !cancel.load(Ordering::Relaxed) {
            on(done.load(Ordering::Relaxed) as f32 / hosts.len().max(1) as f32);
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    on(1.0);
    let mut found = found.into_inner().unwrap_or_else(|p| p.into_inner());
    // The game's API first: that host is the Quest for sure.
    found.sort_by_key(|f| (!f.api, f.ip));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_wifi_address() {
        let out = "35: wlan0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc mq state UP group default qlen 3000\n    inet 192.168.178.45/24 brd 192.168.178.255 scope global wlan0\n       valid_lft forever preferred_lft forever\n";
        assert_eq!(parse_wlan_ip(out), Some(Ipv4Addr::new(192, 168, 178, 45)));
        assert_eq!(parse_wlan_ip("Device \"wlan0\" does not exist."), None);
        assert_eq!(parse_wlan_ip("    inet 127.0.0.1/8 scope host lo"), None);
    }

    #[test]
    fn typed_addresses() {
        assert_eq!(parse_ip(" 10.0.0.7 "), Some(Ipv4Addr::new(10, 0, 0, 7)));
        for bad in ["", "10.0.0", "10.0.0.7:5555", "quest.local", "300.1.1.1"] {
            assert_eq!(parse_ip(bad), None, "{bad}");
        }
    }

    #[test]
    fn tells_network_serials_from_usb() {
        assert!(is_network_serial("192.168.1.20:5555"));
        assert!(is_network_serial(
            "adb-2G0YC5ZF8R0123-abc._adb-tls-connect._tcp"
        ));
        assert!(!is_network_serial("2G0YC5ZF8R0123"));
        assert!(!is_network_serial("emulator-5554"));
    }

    #[test]
    fn connect_answers() {
        assert!(connect_succeeded("connected to 192.168.1.20:5555"));
        assert!(connect_succeeded("already connected to 192.168.1.20:5555"));
        assert!(!connect_succeeded(
            "failed to connect to '192.168.1.20:5555': Connection refused"
        ));
        assert!(!connect_succeeded(
            "cannot connect to 192.168.1.20:5555: No route to host"
        ));
    }

    #[test]
    fn subnet_without_self_and_ends() {
        let hosts = subnet_hosts(Ipv4Addr::new(192, 168, 1, 10));
        assert_eq!(hosts.len(), 253);
        assert!(!hosts.contains(&Ipv4Addr::new(192, 168, 1, 10)));
        assert!(!hosts.contains(&Ipv4Addr::new(192, 168, 1, 0)));
        assert!(!hosts.contains(&Ipv4Addr::new(192, 168, 1, 255)));
        assert_eq!(hosts[0], Ipv4Addr::new(192, 168, 1, 1));
    }

    #[test]
    fn scan_finds_an_open_port() {
        // A listener on this machine stands in for the game's API.
        let listener = match std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, API_PORT)) {
            Ok(l) => l,
            Err(_) => return, // the port is taken here: nothing to learn
        };
        let cancel = AtomicBool::new(false);
        let mut last = 0.0;
        let found = scan(&[Ipv4Addr::LOCALHOST], &cancel, &mut |p| last = p);
        drop(listener);
        assert_eq!(last, 1.0);
        assert_eq!(found.len(), 1);
        assert!(found[0].api);
    }
}
