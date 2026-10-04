//! Parsing and interpretation of the `adb devices -l` table.
//!
//! Everything here is a pure function of the command's text, which is what makes the
//! selection logic testable without a headset attached.

use std::collections::BTreeMap;

/// Outcome of interpreting a device table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    Unauthorized,
    Ambiguous,
    None,
}

/// Keys `adb devices -l` appends after the state. States can contain spaces
/// ("no permissions (user in plugdev group); see [http://...]"), these cannot.
const PROP_KEYS: [&str; 5] = ["usb", "product", "model", "device", "transport_id"];

/// Quest board/codenames, matched as prefixes because builds append suffixes.
/// Only used to break a tie between several authorized devices.
const QUEST_CODENAMES: [&str; 6] = [
    "hollywood",
    "eureka",
    "panther",
    "monterey",
    "seacliff",
    "vr_monterey",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    pub state: String,
    pub props: BTreeMap<String, String>,
}

impl Device {
    /// True only for the plain `device` state -- not offline, recovery, sideload.
    pub fn authorized(&self) -> bool {
        self.state == "device"
    }

    pub fn emulator(&self) -> bool {
        self.serial.starts_with("emulator-")
    }

    /// Reached over the network (`adb connect`, wireless debugging) rather than USB.
    pub fn network(&self) -> bool {
        crate::core::launcher::quest_net::is_network_serial(&self.serial)
    }

    pub fn model(&self) -> Option<&str> {
        self.props.get("model").map(String::as_str)
    }

    /// Looks like a Meta headset, by model name or board codename.
    pub fn quest_like(&self) -> bool {
        if let Some(model) = self.model() {
            let m = model.to_ascii_lowercase();
            if m.starts_with("quest") || m.starts_with("oculus") {
                return true;
            }
        }
        ["device", "product"].iter().any(|key| {
            self.props.get(*key).is_some_and(|v| {
                let v = v.to_ascii_lowercase();
                QUEST_CODENAMES.iter().any(|c| v.starts_with(c))
            })
        })
    }

    /// Human label for a picker button: "Quest 3 -- 1WMHH8150XX07M".
    pub fn label(&self) -> String {
        match self.model() {
            Some(m) if !m.is_empty() => format!("{} -- {}", m.replace('_', " "), self.serial),
            _ => format!("{} ({})", self.serial, self.state),
        }
    }

    /// One dense log line.
    pub fn describe(&self) -> String {
        let mut s = format!("{}  {}", self.serial, self.state);
        for key in ["model", "product", "device", "transport_id"] {
            if let Some(v) = self.props.get(key) {
                s.push_str(&format!("  {key}:{v}"));
            }
        }
        if self.emulator() {
            s.push_str("  [EMULATOR]");
        }
        if self.quest_like() {
            s.push_str("  [QUEST?]");
        }
        s
    }
}

/// The interpretation of one device table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub serial: Option<String>,
    pub all: Vec<Device>,
    pub reason: String,
    pub status: Status,
}

impl Selection {
    /// The devices a picker may offer: authorized, and not an emulator.
    pub fn pickable(&self) -> Vec<Device> {
        self.all
            .iter()
            .filter(|d| d.authorized() && !d.emulator())
            .cloned()
            .collect()
    }
}

/// Parses the output of `adb devices -l`. Tolerates the header, blank lines and
/// the `* daemon not running *` banners.
pub fn parse(output: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    for raw in output.lines() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with('*')
            || line.starts_with("List of devices")
            || line.starts_with("adb:")
            || line.starts_with("error:")
        {
            continue;
        }
        let Some(split) = line.find(char::is_whitespace) else {
            continue; // a lone token is not a row
        };
        let serial = &line[..split];
        let rest = line[split..].trim();
        if rest.is_empty() {
            continue;
        }
        let mut props = BTreeMap::new();
        let mut state = Vec::new();
        for token in rest.split_whitespace() {
            match token.split_once(':') {
                Some((k, v)) if !k.is_empty() && PROP_KEYS.contains(&k) => {
                    props.insert(k.to_string(), v.to_string());
                }
                _ => state.push(token),
            }
        }
        devices.push(Device {
            serial: serial.to_string(),
            state: state.join(" "),
            props,
        });
    }
    devices
}

/// Decides which device to target. Pure: no I/O.
///
/// `preferred` is a serial the user picked earlier; while that device is still attached
/// and authorized it wins over every heuristic (the Java version overwrote the user's
/// choice on the next probe, so picking a device never actually stuck).
pub fn select(devices: Vec<Device>, preferred: Option<&str>) -> Selection {
    let candidates: Vec<&Device> = devices
        .iter()
        .filter(|d| d.authorized() && !d.emulator())
        .collect();

    if let Some(p) = preferred {
        if let Some(d) = candidates.iter().find(|d| d.serial == p) {
            let reason = format!("user-chosen device: {}", d.label());
            let serial = d.serial.clone();
            return Selection {
                serial: Some(serial),
                all: devices,
                reason,
                status: Status::Ready,
            };
        }
    }

    if candidates.len() == 1 {
        let only = candidates[0];
        let reason = format!("one authorized device: {}", only.label());
        let serial = only.serial.clone();
        return Selection {
            serial: Some(serial),
            all: devices,
            reason,
            status: Status::Ready,
        };
    }

    if candidates.len() > 1 {
        let quests: Vec<&&Device> = candidates.iter().filter(|d| d.quest_like()).collect();
        // One headset plugged in by USB and reached over the network too: use the cable.
        let usb: Vec<&&&Device> = quests.iter().filter(|d| !d.network()).collect();
        if quests.len() > 1 && usb.len() == 1 {
            let quest = usb[0];
            let reason = format!(
                "picked {} on USB over the same headset's network connection",
                quest.label()
            );
            return Selection {
                serial: Some(quest.serial.clone()),
                all: devices,
                reason,
                status: Status::Ready,
            };
        }
        if quests.len() == 1 {
            let quest = quests[0];
            let mut reason = format!(
                "picked {} as the only Quest-like device; skipped",
                quest.label()
            );
            for d in candidates.iter().filter(|d| d.serial != quest.serial) {
                reason.push_str(&format!(" {};", d.label()));
            }
            let serial = quest.serial.clone();
            return Selection {
                serial: Some(serial),
                all: devices,
                reason,
                status: Status::Ready,
            };
        }
        let mut reason = format!(
            "{} usable devices and no way to tell which is the headset:",
            candidates.len()
        );
        for d in &candidates {
            reason.push_str(&format!(" {};", d.label()));
        }
        return Selection {
            serial: None,
            all: devices,
            reason,
            status: Status::Ambiguous,
        };
    }

    // Nothing usable. Say why, so the log distinguishes the cases.
    for d in &devices {
        let no_permissions = d.state.starts_with("no permissions");
        if d.state == "unauthorized" || no_permissions || d.state.contains("developer.android.com")
        {
            let reason = if no_permissions {
                format!(
                    "device {} is not accessible to this user (udev rules)",
                    d.serial
                )
            } else {
                format!("device {} has not allowed this PC yet", d.serial)
            };
            return Selection {
                serial: None,
                all: devices,
                reason,
                status: Status::Unauthorized,
            };
        }
    }
    if let Some(d) = devices.iter().find(|d| !d.emulator()) {
        let reason = format!("device {} is in state '{}'", d.serial, d.state);
        return Selection {
            serial: None,
            all: devices,
            reason,
            status: Status::None,
        };
    }
    let reason = if devices.is_empty() {
        "no devices attached"
    } else {
        "only emulators are attached"
    };
    Selection {
        serial: None,
        all: devices,
        reason: reason.into(),
        status: Status::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q3: &str = "1WMHH8150XX07M         device usb:1-1 product:eureka model:Quest_3 device:eureka transport_id:3";
    const PHONE: &str = "R58M123ABC             device usb:1-2 product:beyond1 model:SM_G973F device:beyond1 transport_id:4";

    fn table(rows: &[&str]) -> String {
        let mut s = String::from("* daemon not running; starting now at tcp:5037\n* daemon started successfully\nList of devices attached\n");
        for r in rows {
            s.push_str(r);
            s.push('\n');
        }
        s.push('\n');
        s
    }

    #[test]
    fn parses_props_and_state() {
        let d = parse(&table(&[Q3]));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].serial, "1WMHH8150XX07M");
        assert_eq!(d[0].state, "device");
        assert_eq!(d[0].model(), Some("Quest_3"));
        assert!(d[0].quest_like());
        assert_eq!(d[0].label(), "Quest 3 -- 1WMHH8150XX07M");
    }

    #[test]
    fn parses_multi_word_state() {
        let d = parse("ABC no permissions (user in plugdev group); see [http://developer.android.com/tools/device.html] usb:1-1 transport_id:1\n");
        assert_eq!(d.len(), 1);
        assert!(d[0].state.starts_with("no permissions"));
        assert_eq!(d[0].props.get("usb").map(String::as_str), Some("1-1"));
        assert_eq!(select(d, None).status, Status::Unauthorized);
    }

    #[test]
    fn single_device_is_ready() {
        let s = select(parse(&table(&[Q3])), None);
        assert_eq!(s.status, Status::Ready);
        assert_eq!(s.serial.as_deref(), Some("1WMHH8150XX07M"));
    }

    #[test]
    fn usb_wins_over_the_same_quest_on_the_network() {
        let net = "192.168.178.45:5555   device product:eureka model:Quest_3 device:eureka transport_id:5";
        let s = select(parse(&table(&[net, Q3])), None);
        assert_eq!(s.status, Status::Ready);
        assert_eq!(s.serial.as_deref(), Some("1WMHH8150XX07M"));
        // Unplugged: the network connection alone is used.
        let s = select(parse(&table(&[net])), None);
        assert_eq!(s.serial.as_deref(), Some("192.168.178.45:5555"));
    }

    #[test]
    fn quest_wins_tie_with_phone() {
        let s = select(parse(&table(&[PHONE, Q3])), None);
        assert_eq!(s.status, Status::Ready);
        assert_eq!(s.serial.as_deref(), Some("1WMHH8150XX07M"));
    }

    #[test]
    fn emulator_ignored() {
        let s = select(
            parse(&table(&["emulator-5554 device product:sdk model:sdk", Q3])),
            None,
        );
        assert_eq!(s.status, Status::Ready);
        assert_eq!(s.serial.as_deref(), Some("1WMHH8150XX07M"));
        let only_emu = select(parse(&table(&["emulator-5554 device"])), None);
        assert_eq!(only_emu.status, Status::None);
        assert_eq!(only_emu.reason, "only emulators are attached");
    }

    #[test]
    fn two_phones_are_ambiguous_until_user_picks() {
        let other = PHONE.replace("R58M123ABC", "R58M999XYZ");
        let t = table(&[PHONE, &other]);
        let s = select(parse(&t), None);
        assert_eq!(s.status, Status::Ambiguous);
        assert_eq!(s.pickable().len(), 2);
        // The user's choice sticks across later probes.
        let s = select(parse(&t), Some("R58M999XYZ"));
        assert_eq!(s.status, Status::Ready);
        assert_eq!(s.serial.as_deref(), Some("R58M999XYZ"));
    }

    #[test]
    fn preferred_device_gone_falls_back_to_heuristics() {
        let s = select(parse(&table(&[Q3])), Some("GONE"));
        assert_eq!(s.serial.as_deref(), Some("1WMHH8150XX07M"));
    }

    #[test]
    fn unauthorized_and_none() {
        let s = select(
            parse(&table(&[
                "1WMHH8150XX07M unauthorized usb:1-1 transport_id:2",
            ])),
            None,
        );
        assert_eq!(s.status, Status::Unauthorized);
        let s = select(parse(&table(&["1WMHH8150XX07M offline"])), None);
        assert_eq!(s.status, Status::None);
        let s = select(parse(&table(&[])), None);
        assert_eq!(s.reason, "no devices attached");
    }
}
