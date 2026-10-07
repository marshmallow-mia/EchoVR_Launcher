//! The game's graphics preset on Linux. Echo rates the GPU only on its first start (no
//! settings file yet) and saves the preset it picked; under Proton that rating failed
//! (dxvk-nvapi has no base clock, the game's AGS can't reach the AMD driver), so every
//! Linux install saved Low. NvrGpuRating now answers those questions, but a saved Low
//! stays: once per settings file, a saved Low preset is moved aside before a start so the
//! game rates the GPU again, and afterwards its non-graphics settings (the "game" section)
//! are copied back into the file the game wrote.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::store::InstalledVersion;

/// The plugin that answers the game's GPU questions under Proton.
pub const PLUGIN: &str = "NvrGpuRating.dll";
/// The live build's settings (the 2017-2019 event builds read the older file).
const LIVE: &str = "settings_mp_v2.json";
const EVENT: &str = "settings_mp.json";
/// The settings file as it was before the re-rate, kept beside it.
const ASIDE: &str = ".before-rerate";
/// Beside a settings file once it was re-rated: never again.
const DONE: &str = ".rerated";

/// The settings file `v` reads, in the game's settings folder.
pub fn settings_file(v: &InstalledVersion) -> &'static str {
    if v.publisher_lock.is_some() {
        EVENT
    } else {
        LIVE
    }
}

/// Pure: whether `settings` holds the Low preset the failed rating saved: quality level
/// Low (1) and, where given, textures, meshes, lights and effects at their lowest.
pub fn is_low_preset(settings: &Value) -> bool {
    let g = &settings["graphics"];
    let lowest = |k: &str| g["quality"].get(k).is_none_or(|q| q.as_i64() == Some(0));
    g["qualitylevel"].as_i64() == Some(1)
        && ["textures", "meshes", "lights", "fx"]
            .iter()
            .all(|k| lowest(k))
}

fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    file.with_file_name(name)
}

/// A re-rate under way: the settings file moved aside for one start.
#[derive(Debug)]
pub struct Rerate {
    file: PathBuf,
    aside: PathBuf,
}

/// Before a start: moves `dir/name` aside when it holds the saved Low preset and wasn't
/// re-rated yet, so the game rates the GPU again. `None` when there is nothing to do.
pub fn begin(dir: &Path, name: &str) -> Option<Rerate> {
    let file = dir.join(name);
    if with_suffix(&file, DONE).exists() {
        return None;
    }
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(&file).ok()?).ok()?;
    if !is_low_preset(&settings) {
        return None;
    }
    let aside = with_suffix(&file, ASIDE);
    match std::fs::rename(&file, &aside) {
        Ok(()) => {
            tracing::info!(
                "{} holds the saved Low preset: the game rates the GPU again",
                file.display()
            );
            Some(Rerate { file, aside })
        }
        Err(e) => {
            tracing::warn!("couldn't move {} aside: {e}", file.display());
            None
        }
    }
}

impl Rerate {
    /// After the game ended: copies the old file's "game" section into the one the game
    /// wrote and marks the file re-rated. Without a new file (the game didn't get that far)
    /// the old one goes back, for another try at the next start.
    pub fn finish(self) -> Result<()> {
        if !self.file.exists() {
            std::fs::rename(&self.aside, &self.file)
                .with_context(|| format!("put {} back", self.file.display()))?;
            tracing::info!(
                "the game wrote no {}: the old one is back",
                self.file.display()
            );
            return Ok(());
        }
        let old: Value = serde_json::from_str(&std::fs::read_to_string(&self.aside)?)?;
        let mut new: Value = serde_json::from_str(&std::fs::read_to_string(&self.file)?)?;
        if let (Some(game), Some(obj)) = (old.get("game"), new.as_object_mut()) {
            obj.insert("game".into(), game.clone());
        }
        std::fs::write(&self.file, serde_json::to_string_pretty(&new)?)
            .with_context(|| format!("write {}", self.file.display()))?;
        std::fs::write(with_suffix(&self.file, DONE), b"")?;
        tracing::info!(
            "{} re-rated: quality level {}, your game settings kept",
            self.file.display(),
            new["graphics"]["qualitylevel"]
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn low() -> Value {
        json!({"game": {"sfx": 7, "HUD": false},
               "graphics": {"qualitylevel": 1, "quality": {"textures": 0, "meshes": 0, "lights": 0, "fx": 0}}})
    }

    #[test]
    fn only_the_low_preset() {
        assert!(is_low_preset(&low()));
        assert!(!is_low_preset(&json!({"graphics": {"qualitylevel": 3}})));
        // Low, but with a texture choice of the player's own: theirs, kept.
        assert!(!is_low_preset(
            &json!({"graphics": {"qualitylevel": 1, "quality": {"textures": 2}}})
        ));
        assert!(!is_low_preset(&json!({})));
    }

    #[test]
    fn rerated_once_keeping_the_game_settings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(LIVE);
        std::fs::write(&file, low().to_string()).unwrap();
        let r = begin(dir.path(), LIVE).unwrap();
        assert!(!file.exists());
        // The game rated the GPU: a new file, High, with default game settings.
        std::fs::write(
            &file,
            json!({"game": {"sfx": 10}, "graphics": {"qualitylevel": 3}}).to_string(),
        )
        .unwrap();
        r.finish().unwrap();
        let now: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(now["graphics"]["qualitylevel"], 3);
        assert_eq!(now["game"], low()["game"]);
        assert!(with_suffix(&file, ASIDE).exists());
        // Low again later (the player's choice): left alone.
        std::fs::write(&file, low().to_string()).unwrap();
        assert!(begin(dir.path(), LIVE).is_none());
    }

    #[test]
    fn the_old_file_goes_back_when_the_game_wrote_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(EVENT);
        std::fs::write(&file, low().to_string()).unwrap();
        begin(dir.path(), EVENT).unwrap().finish().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), low().to_string());
        assert!(!with_suffix(&file, DONE).exists());
        assert!(begin(dir.path(), EVENT).is_some());
    }
}
