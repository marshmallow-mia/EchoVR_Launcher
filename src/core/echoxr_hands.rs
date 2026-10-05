//! EchoXR Hands (github.com/EchoTools/EchoXR-Hands, by heisthecat31 and marshmallow-mia):
//! your own fingers on Echo VR's hands, from OpenXR hand tracking. Its package goes into the
//! game's `bin/win10/EchoXR/Hands`: an OpenXR API layer (`layer/`) that reads your fingers in
//! the game's own OpenXR session, the nEVR plugin and the settings window. The launcher puts
//! the plugin into `plugins/`, where nEVR loads it like the update's plugins, takes it out
//! again when hand tracking is off, and enables the layer for each start ([`layer_env`]).
//! It plays with EchoXR, on every runtime with hand tracking (SteamVR, WiVRn, Monado).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};

use super::launcher::versions::Step;
use super::{download, paths};

pub const VERSION: &str = "0.4.0";
const ZIP: &str = "EchoXR-Hands-v0.4.0-nevr.zip";
const URL: &str = "https://release.echovr.de/launcher/plugins/EchoXR-Hands-v0.4.0-nevr.zip";
const SHA256: &str = "5871538e6b2bd8832a13c9acd648bc5faaee4f52665ae1bf97050ba35f0807a6";
/// Who made it, as the Mods page credits it.
pub const AUTHORS: &str = "heisthecat31, marshmallow-mia";
/// The plugin, as nEVR loads it from `plugins/`.
pub const PLUGIN: &str = "EchoXRHands.dll";
/// Its settings beside it (re-read while the game runs).
const SETTINGS: &str = "EchoXRHands.txt";
/// Where the package goes under `bin/win10`, and its plugin inside it.
const DIR: &str = "EchoXR/Hands";
const PACKAGED_PLUGIN: &str = "plugin";
/// The OpenXR API layer's folder (its DLL and manifest) and name.
const LAYER_DIR: &str = "EchoXR/Hands/layer";
const LAYER_NAME: &str = "XR_APILAYER_ECHOTOOLS_echoxr_hands";
pub const SETTINGS_APP: &str = "EchoXRSettings.exe";

fn zip_path() -> PathBuf {
    paths::data_dir().join("echoxr-hands").join(ZIP)
}

/// The environment that enables the hand tracking layer in the game's OpenXR loader:
/// `XR_API_LAYER_PATH` names its folder as the game sees it (`windows_path`: a Wine path on
/// Linux), `XR_ENABLE_API_LAYERS` the layer.
pub fn layer_env(bin: &Path, windows_path: impl Fn(&Path) -> String) -> [(String, String); 2] {
    [
        (
            "XR_API_LAYER_PATH".into(),
            windows_path(&bin.join(LAYER_DIR)),
        ),
        ("XR_ENABLE_API_LAYERS".into(), LAYER_NAME.into()),
    ]
}

/// Whether the package is downloaded (and still the pinned one).
pub fn is_fetched() -> bool {
    zip_path().is_file()
}

/// Downloads the pinned package, once.
pub fn fetch(cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<()> {
    let keep = zip_path();
    if download::sha256_matches(&keep, SHA256) {
        return Ok(());
    }
    on(Step::Status(format!(
        "Downloading EchoXR Hands {VERSION}..."
    )));
    let dir = paths::downloads_dir().join("echoxr-hands");
    let zip = download::fetch_pinned(URL, &dir, ZIP, SHA256, cancel, &mut |p| {
        if let download::Progress::Percent(v) = p {
            on(Step::Percent(v));
        }
    })?;
    std::fs::create_dir_all(keep.parent().unwrap_or(Path::new(".")))?;
    std::fs::copy(&zip, &keep).context("keep EchoXR Hands")?;
    let _ = std::fs::remove_file(zip);
    Ok(())
}

/// The package's folder in the game's `bin`.
pub fn dir_in(bin: &Path) -> PathBuf {
    bin.join(DIR)
}

/// Whether the plugin is in the game's `plugins/` (hand tracking on for that version).
pub fn installed_in(bin: &Path) -> bool {
    bin.join("plugins").join(PLUGIN).is_file()
}

/// Puts the package into `bin` and the plugin into `plugins/`. Its settings go there
/// too the first time, with finger sharing off (`Network = 0`): sharing sends your
/// display name and your match's player names to the hand tracking relay, so it is
/// yours to turn on.
pub fn install_into(bin: &Path) -> Result<()> {
    let zip = std::fs::File::open(zip_path()).context("EchoXR Hands isn't downloaded")?;
    let mut archive = zip::ZipArchive::new(zip)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            bail!("EchoXR Hands' zip has an unsafe path: {}", entry.name());
        };
        if !rel.starts_with(DIR) {
            bail!(
                "EchoXR Hands' zip has a file outside {DIR}: {}",
                rel.display()
            );
        }
        let out = bin.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes)?;
        if std::fs::read(&out).is_ok_and(|have| have == bytes) {
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, &bytes).with_context(|| format!("write {}", out.display()))?;
    }
    let from = dir_in(bin).join(PACKAGED_PLUGIN);
    let plugins = bin.join("plugins");
    std::fs::create_dir_all(&plugins)?;
    let dll = plugins.join(PLUGIN);
    let ours = std::fs::read(from.join(PLUGIN)).context("the package has no plugin")?;
    if std::fs::read(&dll).ok().as_deref() != Some(&ours) {
        std::fs::write(&dll, &ours).with_context(|| {
            format!(
                "Couldn't put {} in place (is Echo VR running?)",
                dll.display()
            )
        })?;
    }
    let settings = plugins.join(SETTINGS);
    if !settings.exists() {
        let text = std::fs::read_to_string(from.join(SETTINGS)).unwrap_or_default();
        std::fs::write(&settings, with_sharing(&text, false))
            .with_context(|| format!("write {}", settings.display()))?;
    }
    Ok(())
}

/// Takes the plugin out of `plugins/` (hand tracking off); its settings stay.
pub fn remove_from(bin: &Path) -> Result<()> {
    let dll = bin.join("plugins").join(PLUGIN);
    match std::fs::remove_file(&dll) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e)
            .with_context(|| format!("Couldn't remove {} (is Echo VR running?)", dll.display())),
    }
}

/// Before a start: the plugin in `plugins/` when hand tracking is `on` (from the
/// downloaded package), out when it's off.
pub fn apply(bin: &Path, on: bool) -> Result<()> {
    if on {
        if !is_fetched() {
            bail!("EchoXR Hands isn't downloaded yet: switch it off and on on the Mods page");
        }
        install_into(bin)
    } else {
        remove_from(bin)
    }
}

/// Whether finger sharing is on in `bin`'s settings.
pub fn sharing(bin: &Path) -> bool {
    std::fs::read_to_string(bin.join("plugins").join(SETTINGS))
        .ok()
        .and_then(|t| setting(&t, "Network"))
        .is_some_and(|v| v != "0")
}

/// Turns finger sharing on or off in `bin`'s settings (the plugin picks it up within
/// half a second, also in a running game).
pub fn set_sharing(bin: &Path, on: bool) -> Result<()> {
    let path = bin.join("plugins").join(SETTINGS);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, with_sharing(&text, on))
        .with_context(|| format!("write {}", path.display()))
}

/// Pure: a `Key = value` setting's value.
fn setting(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.split(['#', ';']).next().unwrap_or("").trim().to_string())
    })
}

/// Pure: `text` with `Network` set to 1 or 0 (added when missing).
fn with_sharing(text: &str, on: bool) -> String {
    let value = if on { "1" } else { "0" };
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| match l.split_once('=') {
            Some((k, _)) if k.trim() == "Network" => {
                found = true;
                format!("Network = {value}")
            }
            _ => l.to_string(),
        })
        .collect();
    if !found {
        out.push(format!("Network = {value}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enables_its_layer_by_name_and_folder() {
        let env = layer_env(Path::new("/g/bin/win10"), |p| format!("X:{}", p.display()));
        assert_eq!(env[0].0, "XR_API_LAYER_PATH");
        assert_eq!(env[0].1, "X:/g/bin/win10/EchoXR/Hands/layer");
        assert_eq!(env[1], ("XR_ENABLE_API_LAYERS".into(), LAYER_NAME.into()));
    }

    #[test]
    fn finger_sharing_setting() {
        let text = "# hands\nSmoothing = 0.5\nNetwork = 1   # share\nRelayUrl = wss://x\n";
        assert_eq!(setting(text, "Network").as_deref(), Some("1"));
        let off = with_sharing(text, false);
        assert_eq!(setting(&off, "Network").as_deref(), Some("0"));
        assert!(off.contains("RelayUrl = wss://x") && off.contains("Smoothing = 0.5"));
        let added = with_sharing("Smoothing = 1", true);
        assert_eq!(setting(&added, "Network").as_deref(), Some("1"));
    }

    #[test]
    fn plugin_in_and_out() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        assert!(!installed_in(bin));
        remove_from(bin).unwrap();
        std::fs::create_dir_all(bin.join("plugins")).unwrap();
        std::fs::write(bin.join("plugins").join(PLUGIN), b"x").unwrap();
        std::fs::write(bin.join("plugins").join(SETTINGS), "Network = 1\n").unwrap();
        assert!(installed_in(bin) && sharing(bin));
        set_sharing(bin, false).unwrap();
        assert!(!sharing(bin));
        remove_from(bin).unwrap();
        assert!(!installed_in(bin));
        assert!(bin.join("plugins").join(SETTINGS).exists());
    }
}
