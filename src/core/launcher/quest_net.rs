//! The Quest over the network: its Wi-Fi address (read over USB, typed, or found by a scan
//! of the local network), Echo VR's API on it (port 6721, while the game runs with API
//! access on), and ADB over the network (port 5555, turned on once over USB, or the
//! headset's wireless debugging, which adb finds by mDNS); and the list of every headset
//! found these ways.

use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Result};

use super::game::{self, GameState};
use crate::core::adb::{self, devices::Device};

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

/// A headset's wireless debugging, as adb found it by mDNS (`adb mdns services`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mdns {
    /// The service's instance name (`adb-<serial>-<id>`).
    pub name: String,
    pub addr: std::net::SocketAddrV4,
}

/// Pure: the adb services in `adb mdns services` output (`name  _adb-tls-connect._tcp
/// ip:port` rows; `_adb-tls-pairing` is for pairing, not connecting).
pub fn parse_mdns(output: &str) -> Vec<Mdns> {
    output
        .lines()
        .filter_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            let [name, kind, addr] = t[..] else {
                return None;
            };
            if !(kind.starts_with("_adb-tls-connect.") || kind.starts_with("_adb._tcp")) {
                return None;
            }
            Some(Mdns {
                name: name.to_string(),
                addr: addr.parse().ok()?,
            })
        })
        .collect()
}

/// The headsets adb sees by mDNS (none when this adb can't look).
pub fn mdns() -> Vec<Mdns> {
    let r = adb::exec(&["mdns", "services"]);
    if r.success() {
        parse_mdns(&r.output)
    } else {
        Vec::new()
    }
}

/// How a listed headset stands with adb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    /// adb can use it.
    Connected,
    /// It hasn't allowed this PC yet (the prompt in the headset).
    Unauthorized,
    /// adb sees it in another state (`offline`, `no permissions`...).
    Other(String),
    /// Found on the network, not connected: [`FoundQuest::connect`] says where.
    NotConnected,
}

/// One headset in the Quest side's list: what adb has, what answered on the network, the
/// same headset once however it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundQuest {
    /// Its model ("Quest 3"), or its address when only the network knows it.
    pub label: String,
    /// adb's serial for it, when adb has it: the USB one when it has both.
    pub serial: Option<String>,
    /// Where `adb connect` reaches it, for one adb doesn't have yet.
    pub connect: Option<String>,
    pub ip: Option<Ipv4Addr>,
    pub usb: bool,
    /// adb reaches it over the network (`adb connect`, wireless debugging).
    pub network: bool,
    /// Echo VR's API answers on it: Echo VR is running there.
    pub api: bool,
    /// Its ADB port answers, or adb found its wireless debugging.
    pub adb_open: bool,
    pub conn: Conn,
    /// By its model or codename, or because Echo VR answers there.
    pub quest_like: bool,
}

impl FoundQuest {
    /// How it was found, for its line in the list (its address unless that is its label).
    pub fn via(&self) -> String {
        let mut v = Vec::new();
        if self.usb {
            v.push("USB");
        }
        if self.network || (self.adb_open && self.serial.is_none()) {
            v.push("Wi-Fi");
        }
        if self.api {
            v.push("Echo VR running");
        }
        match self.ip.filter(|ip| self.label != ip.to_string()) {
            Some(ip) if !v.is_empty() => format!("{} · {ip}", v.join(" + ")),
            Some(ip) => ip.to_string(),
            None => v.join(" + "),
        }
    }
}

/// The address in a network serial (`192.168.1.20:5555`).
fn serial_ip(serial: &str) -> Option<Ipv4Addr> {
    match serial.parse::<SocketAddr>().ok()?.ip() {
        std::net::IpAddr::V4(ip) => Some(ip),
        _ => None,
    }
}

/// Pure: every headset found, once each. `devices` is adb's table, `found` what a scan of
/// the network found, `mdns` what adb found by mDNS, `usb_ip` a USB serial's Wi-Fi address
/// (read over USB). The same headset on USB and over the network is one entry (by its
/// address), and so is one adb connected to that a scan or mDNS found again. Quest-like
/// ones first, then connected ones.
pub fn merge(
    devices: &[Device],
    found: &[Found],
    mdns: &[Mdns],
    usb_ip: Option<(&str, Ipv4Addr)>,
) -> Vec<FoundQuest> {
    let mut list: Vec<FoundQuest> = Vec::new();
    for d in devices.iter().filter(|d| !d.emulator()) {
        let network = d.network();
        let ip = if network {
            serial_ip(&d.serial).or_else(|| {
                mdns.iter()
                    .find(|m| d.serial.starts_with(&m.name))
                    .map(|m| *m.addr.ip())
            })
        } else {
            usb_ip.filter(|(s, _)| *s == d.serial).map(|(_, ip)| ip)
        };
        let conn = match d.state.as_str() {
            "device" => Conn::Connected,
            "unauthorized" => Conn::Unauthorized,
            s => Conn::Other(s.to_string()),
        };
        let label = d
            .model()
            .filter(|m| !m.is_empty())
            .map(|m| m.replace('_', " "))
            .unwrap_or_else(|| d.serial.clone());
        // The same headset on USB and over the network: one entry, the cable's serial.
        if let Some(e) = ip.and_then(|ip| list.iter_mut().find(|e| e.ip == Some(ip))) {
            if network {
                e.network = true;
            } else {
                e.serial = Some(d.serial.clone());
                e.usb = true;
                e.label = label;
                e.conn = conn;
            }
            e.quest_like |= d.quest_like();
            continue;
        }
        list.push(FoundQuest {
            label,
            serial: Some(d.serial.clone()),
            connect: None,
            ip,
            usb: !network,
            network,
            api: false,
            adb_open: false,
            conn,
            quest_like: d.quest_like(),
        });
    }
    for m in mdns {
        let ip = *m.addr.ip();
        if let Some(e) = list.iter_mut().find(|e| e.ip == Some(ip)) {
            e.adb_open = true;
            continue;
        }
        list.push(FoundQuest {
            label: ip.to_string(),
            serial: None,
            connect: Some(m.addr.to_string()),
            ip: Some(ip),
            usb: false,
            network: false,
            api: false,
            adb_open: true,
            conn: Conn::NotConnected,
            // Any Android device may offer wireless debugging: a Quest once adb says so.
            quest_like: false,
        });
    }
    for f in found {
        if let Some(e) = list.iter_mut().find(|e| e.ip == Some(f.ip)) {
            e.api |= f.api;
            e.adb_open |= f.adb;
            e.quest_like |= f.api;
            if e.connect.is_none() && e.serial.is_none() && f.adb {
                e.connect = Some(format!("{}:{ADB_PORT}", f.ip));
            }
            continue;
        }
        list.push(FoundQuest {
            label: f.ip.to_string(),
            serial: None,
            connect: f.adb.then(|| format!("{}:{ADB_PORT}", f.ip)),
            ip: Some(f.ip),
            usb: false,
            network: false,
            api: f.api,
            adb_open: f.adb,
            conn: Conn::NotConnected,
            quest_like: f.api,
        });
    }
    let rank = |c: &Conn| match c {
        Conn::Connected => 0,
        Conn::Unauthorized => 1,
        Conn::Other(_) => 2,
        Conn::NotConnected => 3,
    };
    list.sort_by(|a, b| {
        (!a.quest_like, rank(&a.conn), a.ip, &a.label).cmp(&(
            !b.quest_like,
            rank(&b.conn),
            b.ip,
            &b.label,
        ))
    });
    list
}

/// Connects adb to a listed headset that isn't connected yet, at `addr` (`ip:port`), and
/// returns the serial adb gives it.
pub fn connect_to(addr: &str) -> Result<String> {
    let r = adb::exec(&["connect", addr]);
    if !connect_succeeded(&r.output) {
        bail!(
            "Couldn't reach the headset at {addr} over the network: {}",
            r.output.trim()
        );
    }
    Ok(addr.to_string())
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

    fn dev(serial: &str, state: &str, model: Option<&str>) -> Device {
        let mut props = std::collections::BTreeMap::new();
        if let Some(m) = model {
            props.insert("model".to_string(), m.to_string());
        }
        Device {
            serial: serial.into(),
            state: state.into(),
            props,
        }
    }

    #[test]
    fn reads_mdns_services() {
        let out = "List of discovered mdns services\nadb-2G0YC5ZF8R0123-AbCd\t_adb-tls-connect._tcp\t192.168.178.93:37123\nadb-2G0YC5ZF8R0123-AbCd\t_adb-tls-pairing._tcp\t192.168.178.93:40111\n";
        let m = parse_mdns(out);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name, "adb-2G0YC5ZF8R0123-AbCd");
        assert_eq!(m[0].addr.to_string(), "192.168.178.93:37123");
        assert!(parse_mdns("ERROR: mdns discovery disabled").is_empty());
    }

    #[test]
    fn lists_every_headset_once() {
        let q = Ipv4Addr::new(192, 168, 178, 93);
        let other = Ipv4Addr::new(192, 168, 178, 20);
        let tv = Ipv4Addr::new(192, 168, 178, 50);
        let devices = [
            dev("1WMHH8150XX07M", "device", Some("Quest_3")),
            dev("192.168.178.93:5555", "device", Some("Quest_3")),
            dev("2G0YC5ZF8R0123", "unauthorized", None),
            dev("emulator-5554", "device", None),
        ];
        let found = [
            Found {
                ip: q,
                api: true,
                adb: true,
            },
            Found {
                ip: other,
                api: true,
                adb: false,
            },
            Found {
                ip: tv,
                api: false,
                adb: true,
            },
        ];
        let list = merge(&devices, &found, &[], Some(("1WMHH8150XX07M", q)));
        let keys: Vec<String> = list
            .iter()
            .map(|f| {
                let ip = f.ip.map(|ip| ip.to_string()).unwrap_or_default();
                f.serial.clone().or_else(|| f.connect.clone()).unwrap_or(ip)
            })
            .collect();
        assert_eq!(
            keys,
            [
                "1WMHH8150XX07M",
                "192.168.178.20",
                "2G0YC5ZF8R0123",
                "192.168.178.50:5555"
            ]
        );
        // USB and Wi-Fi, one headset, the cable's serial; Echo VR answers there.
        let q3 = &list[0];
        assert!(q3.usb && q3.network && q3.api && q3.conn == Conn::Connected);
        assert_eq!(q3.label, "Quest 3");
        assert_eq!(q3.via(), "USB + Wi-Fi + Echo VR running · 192.168.178.93");
        // Echo VR answers but ADB doesn't: listed, nothing to connect to.
        assert!(list[1].quest_like && list[1].connect.is_none());
        // Its address is its name already.
        assert_eq!(list[1].via(), "Echo VR running");
        assert_eq!(list[2].conn, Conn::Unauthorized);
        // Only ADB's port: maybe a TV; connectable, not taken for a Quest.
        assert!(!list[3].quest_like && list[3].conn == Conn::NotConnected);
    }

    #[test]
    fn a_connected_headset_found_again_is_not_listed_twice() {
        let q = Ipv4Addr::new(10, 0, 0, 7);
        let devices = [dev("10.0.0.7:5555", "device", Some("Quest_2"))];
        let mdns = [Mdns {
            name: "adb-1WMHH-x".into(),
            addr: "10.0.0.7:37000".parse().unwrap(),
        }];
        let list = merge(
            &devices,
            &[Found {
                ip: q,
                api: false,
                adb: true,
            }],
            &mdns,
            None,
        );
        assert_eq!(list.len(), 1);
        assert!(list[0].network && !list[0].usb && list[0].adb_open);
        // Found by mDNS only: connectable at the service's port.
        let list = merge(&[], &[], &mdns, None);
        assert_eq!(list[0].connect.as_deref(), Some("10.0.0.7:37000"));
    }
}
