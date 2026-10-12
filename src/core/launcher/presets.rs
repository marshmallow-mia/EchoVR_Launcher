//! Presets of a launcher plugin's settings: the ones its description brings, the player's
//! own (`presets.json` in the plugin's folder) and preset files, so players can pass a setup
//! on. A preset is the whole state of the settings presets set: what it doesn't name takes
//! its default when it is picked.
//!
//! A preset file:
//!
//! ```json
//! {"kind": "echovr-launcher-preset", "schema": 1, "plugin": "controllers",
//!  "name": "Steeper hands", "values": {"Pitch": "-10"}}
//! ```
//!
//! Importing one keeps only settings the plugin has and presets set, each moved into its
//! range (or left out when it can't be one of its values): a file can't add a setting.
use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::plugin_settings::Settings;

/// The player's presets in a plugin's folder.
pub const PRESETS_FILE: &str = "presets.json";
/// What a preset file says it is.
pub const KIND: &str = "echovr-launcher-preset";
const SCHEMA: u64 = 1;
/// A preset file is small: anything bigger isn't one.
pub const MAX_FILE: usize = 64 * 1024;
/// A preset's name, at most (characters).
pub const NAME_MAX: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPreset {
    pub name: String,
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UserPresets {
    #[serde(default)]
    schema: u64,
    #[serde(default)]
    presets: Vec<UserPreset>,
}

/// The player's presets in plugin folder `dir`; none when there are none. A file that
/// can't be read is put aside as `presets.json.bad` (file I/O).
pub fn load(dir: &Path) -> Vec<UserPreset> {
    let path = dir.join(PRESETS_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match serde_json::from_str::<UserPresets>(&text) {
        Ok(p) => p.presets,
        Err(e) => {
            tracing::warn!("{}: {e}; put aside as .bad", path.display());
            let _ = std::fs::rename(&path, dir.join(format!("{PRESETS_FILE}.bad")));
            Vec::new()
        }
    }
}

/// Keeps `presets` as the player's in `dir` (file I/O).
pub fn save(dir: &Path, presets: &[UserPreset]) -> Result<()> {
    let text = serde_json::to_string_pretty(&json!({ "schema": SCHEMA, "presets": presets }))?;
    let path = dir.join(PRESETS_FILE);
    let tmp = dir.join(format!("{PRESETS_FILE}.launcher.tmp"));
    std::fs::write(&tmp, text).with_context(|| format!("Couldn't write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't replace {}", path.display())
    })
}

/// Pure: `name` as a preset's name, or why it can't be one.
pub fn check_name(name: &str) -> std::result::Result<String, String> {
    let t = name.trim();
    if t.is_empty() {
        return Err("Give it a name".into());
    }
    if t.chars().count() > NAME_MAX {
        return Err(format!("At most {NAME_MAX} characters"));
    }
    if t.chars().any(char::is_control) {
        return Err("Only letters, digits, spaces and signs".into());
    }
    Ok(t.to_string())
}

/// Pure: `name`, or `name (2)`, `name (3)`… when the player has a preset of that name.
pub fn unique_name(existing: &[UserPreset], name: &str) -> String {
    let taken = |n: &str| existing.iter().any(|p| p.name.eq_ignore_ascii_case(n));
    if !taken(name) {
        return name.to_string();
    }
    (2..)
        .map(|i| format!("{name} ({i})"))
        .find(|n| !taken(n))
        .expect("a free name")
}

/// Which preset a set of values is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// One of the description's (its index).
    Builtin(usize),
    /// One of the player's (its index).
    User(usize),
}

/// Pure: the preset `values` are exactly (the description's first, then the player's).
pub fn matching(
    s: &Settings,
    values: &BTreeMap<String, String>,
    user: &[UserPreset],
) -> Option<Match> {
    let now = s.preset_values(values);
    s.presets
        .iter()
        .position(|p| s.preset_values(&p.values) == now)
        .map(Match::Builtin)
        .or_else(|| {
            user.iter()
                .position(|p| s.preset_values(&p.values) == now)
                .map(Match::User)
        })
}

/// Pure: the changes that make `values` preset `preset` (every preset setting: what the
/// preset doesn't name goes back to its default).
pub fn changes_to(
    s: &Settings,
    values: &BTreeMap<String, String>,
    preset: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let want = s.preset_values(preset);
    let now = s.preset_values(values);
    want.into_iter()
        .filter(|(k, v)| now.get(k) != Some(v))
        .collect()
}

/// Pure: a preset file of plugin `plugin` holding `values` (its preset settings only).
pub fn export_text(
    s: &Settings,
    plugin: &str,
    name: &str,
    values: &BTreeMap<String, String>,
) -> String {
    let doc = json!({
        "kind": KIND,
        "schema": SCHEMA,
        "plugin": plugin,
        "name": name,
        "values": s.preset_values(values),
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n"
}

/// A preset read from a file, and what had to be changed in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub name: String,
    pub values: BTreeMap<String, String>,
    /// Settings left out: this plugin has none of that name, or the value can't be one.
    pub dropped: Vec<String>,
    /// Settings moved into their range.
    pub clamped: Vec<String>,
}

/// Pure: the preset in file text `text` for plugin `plugin` with settings `s`, or why it
/// isn't one.
pub fn import_text(
    text: &str,
    plugin: &str,
    s: &Settings,
) -> std::result::Result<Imported, String> {
    if text.len() > MAX_FILE {
        return Err("This file is too big to be a preset.".into());
    }
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|_| "This file isn't a preset.".to_string())?;
    if v.get("kind").and_then(Value::as_str) != Some(KIND) {
        return Err("This file isn't a preset.".into());
    }
    if v.get("schema").and_then(Value::as_u64).unwrap_or(SCHEMA) > SCHEMA {
        return Err("This preset is from a newer launcher: update it in Settings.".into());
    }
    let from = v.get("plugin").and_then(Value::as_str).unwrap_or_default();
    if from != plugin {
        return Err(format!(
            "This preset is for the {from} plugin, not this one."
        ));
    }
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .and_then(|n| check_name(n).ok())
        .unwrap_or_else(|| "Imported".into());
    let mut out = Imported {
        name,
        values: BTreeMap::new(),
        dropped: Vec::new(),
        clamped: Vec::new(),
    };
    let Some(values) = v.get("values").and_then(Value::as_object) else {
        return Err("This preset holds no settings.".into());
    };
    for (k, raw) in values {
        let text = match raw {
            Value::String(t) => t.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => {
                out.dropped.push(k.clone());
                continue;
            }
        };
        let Some(f) = s.field(k).filter(|f| f.in_presets()) else {
            out.dropped.push(k.clone());
            continue;
        };
        match f.clamp(&text) {
            Some(kept) => {
                if f.check(&text).is_err() {
                    out.clamped.push(f.label.clone());
                }
                out.values.insert(k.clone(), kept);
            }
            None => out.dropped.push(k.clone()),
        }
    }
    if out.values.is_empty() {
        return Err("Nothing in this preset is a setting of this plugin.".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::launcher::plugin_settings::parse_settings;

    fn settings() -> Settings {
        parse_settings(&json!({
            "store": {"kind": "file", "file": "EchoXR.txt", "format": "keyvalue", "live": true, "echoxr": true},
            "sections": [{"id": "hands", "title": "Hands", "fields": [
                {"key": "HandPose", "type": "choice", "default": "grip",
                 "choices": [{"value": "grip", "label": "Meta"}, {"value": "aim", "label": "Revive"}]},
                {"key": "Pitch", "type": "number", "min": -45, "max": 45, "step": 1, "default": 0},
                {"key": "Secret", "type": "secret"},
                {"key": "PoseLog", "type": "bool", "values": ["1", "0"], "default": false, "preset": false}
            ]}],
            "presets": [
                {"id": "meta", "name": "Default (Meta)", "values": {}},
                {"id": "steep", "name": "Steeper hands", "values": {"Pitch": -10, "PoseLog": "1", "Speed": 3}}
            ]
        }))
        .0
        .expect("settings")
    }

    #[test]
    fn presets_in_a_description() {
        let (s, w) = parse_settings(&json!({
            "sections": [{"fields": [{"key": "Pitch", "type": "number", "min": -45, "max": 45, "default": 0},
                                      {"key": "Log", "type": "bool", "preset": false}]}],
            "presets": [{"name": "Steeper", "values": {"Pitch": -10, "Log": true, "Nope": 1}},
                        {"name": "Steeper", "values": {}},
                        {"values": {}},
                        {"name": "Bad", "values": {"Pitch": 99}}]
        }));
        let s = s.expect("settings");
        assert_eq!(s.presets.len(), 2, "{w:?}");
        assert_eq!(s.presets[0].id, "steeper");
        assert_eq!(
            s.presets[0].values,
            BTreeMap::from([("Pitch".into(), "-10".into())])
        );
        assert!(s.presets[1].values.is_empty());
        for part in [
            "Log isn't",
            "Nope isn't",
            "appears twice",
            "no name",
            "Pitch: From",
        ] {
            assert!(w.iter().any(|x| x.contains(part)), "{part}: {w:?}");
        }
    }

    #[test]
    fn a_preset_is_the_whole_state() {
        let s = settings();
        // Only the preset settings, defaults filled in: a secret and a "preset": false are out.
        let full = s.preset_values(&BTreeMap::new());
        assert_eq!(
            full,
            BTreeMap::from([
                ("HandPose".into(), "grip".into()),
                ("Pitch".into(), "0".into())
            ])
        );
        let steep = &s.presets[1];
        assert_eq!(
            steep.values,
            BTreeMap::from([("Pitch".into(), "-10".into())])
        );

        let mut values = BTreeMap::from([("PoseLog".into(), "1".into())]);
        assert_eq!(matching(&s, &values, &[]), Some(Match::Builtin(0)));
        values.insert("Pitch".into(), "-10".into());
        assert_eq!(matching(&s, &values, &[]), Some(Match::Builtin(1)));
        values.insert("Pitch".into(), "-3".into());
        assert_eq!(matching(&s, &values, &[]), None);
        let mine = UserPreset {
            name: "Mine".into(),
            values: BTreeMap::from([("Pitch".into(), "-3".into())]),
        };
        assert_eq!(matching(&s, &values, &[mine]), Some(Match::User(0)));

        // Picking the default preset puts Pitch back, and leaves PoseLog alone.
        assert_eq!(
            changes_to(&s, &values, &s.presets[0].values),
            vec![("Pitch".to_string(), "0".to_string())]
        );
    }

    #[test]
    fn export_and_import() {
        let s = settings();
        let values = BTreeMap::from([
            ("Pitch".into(), "-12".into()),
            ("PoseLog".into(), "1".into()),
            ("Secret".into(), "hunter2".into()),
        ]);
        let text = export_text(&s, "controllers", "Mine", &values);
        assert!(
            !text.contains("hunter2") && !text.contains("PoseLog"),
            "{text}"
        );
        let back = import_text(&text, "controllers", &s).expect("imports");
        assert_eq!(back.name, "Mine");
        assert_eq!(back.values, s.preset_values(&values));
        assert!(back.dropped.is_empty() && back.clamped.is_empty());

        let odd = json!({"kind": KIND, "schema": 1, "plugin": "controllers", "name": "  ",
            "values": {"Pitch": 90, "HandPose": "palm", "Speed": "3", "PoseLog": "1", "Secret": "x"}});
        let got = import_text(&odd.to_string(), "controllers", &s).expect("imports");
        assert_eq!(got.name, "Imported");
        assert_eq!(got.values, BTreeMap::from([("Pitch".into(), "45".into())]));
        assert_eq!(got.clamped, vec!["Pitch".to_string()]);
        let mut dropped = got.dropped.clone();
        dropped.sort();
        assert_eq!(dropped, vec!["HandPose", "PoseLog", "Secret", "Speed"]);

        for (bad, why) in [
            ("{}".to_string(), "isn't a preset"),
            ("not json".to_string(), "isn't a preset"),
            (
                json!({"kind": KIND, "plugin": "hands", "values": {"Pitch": 1}}).to_string(),
                "for the hands plugin",
            ),
            (
                json!({"kind": KIND, "schema": 9, "plugin": "controllers", "values": {}})
                    .to_string(),
                "newer launcher",
            ),
            (
                json!({"kind": KIND, "plugin": "controllers", "values": {"Speed": 2}}).to_string(),
                "Nothing in this preset",
            ),
            (" ".repeat(MAX_FILE + 1), "too big"),
        ] {
            let e = import_text(&bad, "controllers", &s).expect_err(&bad);
            assert!(e.contains(why), "{e}");
        }
    }

    #[test]
    fn names() {
        assert_eq!(check_name("  Mia's hands "), Ok("Mia's hands".into()));
        assert!(check_name(" ").is_err());
        assert!(check_name(&"x".repeat(NAME_MAX + 1)).is_err());
        assert!(check_name("a\tb").is_err());
        let have = vec![
            UserPreset {
                name: "Mine".into(),
                values: BTreeMap::new(),
            },
            UserPreset {
                name: "mine (2)".into(),
                values: BTreeMap::new(),
            },
        ];
        assert_eq!(unique_name(&have, "Other"), "Other");
        assert_eq!(unique_name(&have, "Mine"), "Mine (3)");
    }

    #[test]
    fn user_presets_on_disk() {
        let dir = tempfile::tempdir().expect("dir");
        assert!(load(dir.path()).is_empty());
        let p = vec![UserPreset {
            name: "Mine".into(),
            values: BTreeMap::from([("Pitch".into(), "-3".into())]),
        }];
        save(dir.path(), &p).expect("saves");
        assert_eq!(load(dir.path()), p);
        std::fs::write(dir.path().join(PRESETS_FILE), "{ broken").expect("write");
        assert!(load(dir.path()).is_empty());
        assert!(dir.path().join("presets.json.bad").is_file());
        assert!(!dir.path().join(PRESETS_FILE).exists());
    }
}
