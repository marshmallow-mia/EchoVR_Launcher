//! Manifest-based update engine for the PC install (port of `UpdateService`).
//!
//! Processes all `del` entries first, then downloads and SHA-256-verifies each `add`
//! entry into the `bin/win10` folder. Files whose on-disk hash already matches are skipped.
//! Unlike the Java version, a failure reaches the caller as an error (the wizard used to
//! stay stuck "in progress" because nothing was called back), and the message says what
//! actually went wrong instead of always blaming a running Echo VR.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Context, Result};

use super::download::{redact, sha256_matches};
use super::http::{self, Cancelled};
use super::launcher::update_check::Channel;
use super::manifest::Manifest;

/// The launcher's live PC update: the community update with the mod loader, nEVR runtime
/// (`BugSplat64.dll`), which every mod needs, in place of the old loader (EchoLoader's
/// `dbgcore.dll`, which it deletes). The old installer keeps its own channel, `updates/` on
/// files.echovr.de (mirrored to release.echovr.de), without nEVR.
pub const PC_MANIFEST_URL: &str = "https://release.echovr.de/updates-nevr/update.manifest";
/// The launcher channels' overlays on the live PC update. A channel replaces main's files
/// only where it has a replacement: its folder holds just those files and a manifest of
/// just their lines; a channel without a folder (404) is exactly main. Alpha takes beta's
/// replacements too, its own on top.
pub const BETA_OVERLAY: &str = "https://release.echovr.de/updates-nevr-beta/update.manifest";
pub const ALPHA_OVERLAY: &str = "https://release.echovr.de/updates-nevr-alpha/update.manifest";

/// Where the update channels were read before; installs keep such a URL.
const FILES_UPDATES: &str = "https://files.echovr.de/updates/";
const RELEASE_UPDATES: &str = "https://release.echovr.de/updates/";
/// The PC channel without nEVR, as installs may have noted it (either host).
const OLD_PC_MANIFESTS: [&str; 2] = [
    "https://files.echovr.de/updates/update.manifest",
    "https://release.echovr.de/updates/update.manifest",
];
/// Where to try a PC update before it goes live: another manifest on release.echovr.de or
/// files.echovr.de (e.g. `https://release.echovr.de/updates-nevr/update.manifest`) in place
/// of the live one.
pub const MANIFEST_OVERRIDE: &str = "ECHOVR_UPDATE_MANIFEST";

/// The channel launcher.json's `update_manifest` names, as the launcher's settings were
/// last loaded or saved ([`set_channel`]).
static CHANNEL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The launcher channel launcher.json follows ([`set_launcher_channel`]): which overlays
/// the live PC update gets.
static LAUNCHER_CHANNEL: std::sync::Mutex<Channel> = std::sync::Mutex::new(Channel::Main);

/// Takes launcher.json's launcher channel; called whenever the settings are loaded or saved.
pub fn set_launcher_channel(channel: Channel) {
    *LAUNCHER_CHANNEL.lock().unwrap_or_else(|p| p.into_inner()) = channel;
}

/// The overlays on the live PC update for `channel`, in the order they apply (the last
/// wins).
pub fn overlays(channel: Channel) -> &'static [&'static str] {
    match channel {
        Channel::Main => &[],
        Channel::Beta => &[BETA_OVERLAY],
        Channel::Alpha => &[BETA_OVERLAY, ALPHA_OVERLAY],
    }
}

/// The update manifest at `url`, parsed and as text. For the live PC update (no test
/// channel chosen, [`manifest_for`] left it as is) that's main's with the followed
/// channel's overlays on top. The text is what an update check hashes, so an overlay that
/// changes, or a channel switch, shows up as an update.
pub fn fetch_manifest(url: &str) -> Result<(Manifest, String)> {
    let text = http::get_text(url)
        .with_context(|| format!("Could not download the update manifest ({url})"))?;
    let mut over = Vec::new();
    if url == PC_MANIFEST_URL {
        let channel = *LAUNCHER_CHANNEL.lock().unwrap_or_else(|p| p.into_inner());
        for o in overlays(channel) {
            let t = http::get_text_opt(o)
                .with_context(|| format!("Could not download the channel's update ({o})"))?;
            if let Some(t) = t {
                over.push((*o, t));
            }
        }
    }
    with_overlays(url, &text, &over)
}

/// Pure: `text` (the manifest at `url`) with the overlays `(url, text)` on top, in order.
fn with_overlays(url: &str, text: &str, overlays: &[(&str, String)]) -> Result<(Manifest, String)> {
    let mut m = Manifest::parse(text, url)?;
    let mut all = text.to_string();
    for (o, t) in overlays {
        m.overlay(
            Manifest::parse(t, o)
                .with_context(|| format!("The channel's update manifest is broken ({o})"))?,
        );
        all.push_str(&format!("\n# overlay: {o}\n{t}"));
    }
    Ok((m, all))
}

/// Takes launcher.json's `update_manifest` (the live PC update from another channel, e.g. a
/// test one); called whenever the settings are loaded or saved. Said once per change.
pub fn set_channel(url: Option<&str>) {
    let url = url
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string);
    let mut c = CHANNEL.lock().unwrap_or_else(|p| p.into_inner());
    if *c == url {
        return;
    }
    match &url {
        Some(u) if override_ok(u) => {
            tracing::warn!("PC update from {u} (launcher.json update_manifest) instead of the live one")
        }
        Some(u) => tracing::warn!(
            "launcher.json update_manifest {u} ignored: not https://release.echovr.de/updates-*/update.manifest"
        ),
        None => tracing::info!("PC update from the live channel again"),
    }
    *c = url;
}

/// Where the live PC update comes from instead of the live channel, and who says so
/// (`None`: the live one): `ECHOVR_UPDATE_MANIFEST` first, then launcher.json's
/// `update_manifest`; only one on release.echovr.de or files.echovr.de counts.
pub fn channel_override() -> Option<(String, &'static str)> {
    if let Ok(o) = std::env::var(MANIFEST_OVERRIDE) {
        // Asked for every frame: said once.
        static SAID: std::sync::Once = std::sync::Once::new();
        let ok = override_ok(&o);
        SAID.call_once(|| {
            if ok {
                tracing::warn!("PC update from {o} ({MANIFEST_OVERRIDE}) instead of the live one");
            } else {
                tracing::warn!(
                    "{MANIFEST_OVERRIDE} ignored: not https://release.echovr.de/updates-*/update.manifest"
                );
            }
        });
        if ok {
            return Some((o, MANIFEST_OVERRIDE));
        }
    }
    let c = CHANNEL.lock().unwrap_or_else(|p| p.into_inner()).clone();
    pick_channel(None, c.as_deref())
}

/// Pure: the channel from the environment variable's value (`env`), else from
/// launcher.json's (`config`), each only when it is a staging manifest we take.
fn pick_channel(env: Option<&str>, config: Option<&str>) -> Option<(String, &'static str)> {
    env.filter(|u| override_ok(u))
        .map(|u| (u.to_string(), MANIFEST_OVERRIDE))
        .or_else(|| {
            config
                .filter(|u| override_ok(u))
                .map(|u| (u.to_string(), "launcher.json"))
        })
}

/// `url`, or for the live PC update the channel [`channel_override`] names.
pub fn manifest_for(url: &str) -> String {
    // The PC channel without nEVR: the launcher's live one now has it. Any other
    // files.echovr.de update channel (the Quest's) is read from the mirror.
    let moved = if OLD_PC_MANIFESTS.contains(&url) {
        Some(PC_MANIFEST_URL.to_string())
    } else {
        url.strip_prefix(FILES_UPDATES)
            .map(|rest| format!("{RELEASE_UPDATES}{rest}"))
    };
    let url = moved.as_deref().unwrap_or(url);
    if url == PC_MANIFEST_URL {
        if let Some((o, _)) = channel_override() {
            return o;
        }
    }
    url.to_string()
}

/// A staging manifest: `https://{release,files}.echovr.de/updates-*/update.manifest`.
fn override_ok(url: &str) -> bool {
    (url.starts_with("https://release.echovr.de/updates-")
        || url.starts_with("https://files.echovr.de/updates-"))
        && url.ends_with("/update.manifest")
}

/// A failure worth its own dialog title.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(
        "The downloaded file {0} does not match the expected SHA-256 checksum.\nUpdate aborted."
    )]
    HashMismatch(String),
    #[error("Couldn't replace {path}. Please close Echo VR before updating.\n\n{source}")]
    FileInUse {
        path: String,
        source: std::io::Error,
    },
}

impl UpdateError {
    pub fn title(&self) -> String {
        match self {
            UpdateError::HashMismatch(p) => format!("Hash mismatch for {p}"),
            UpdateError::FileInUse { .. } => "Please close Echo VR first".into(),
        }
    }
}

/// Dialog title for any error returned by [`apply`].
pub fn error_title(e: &anyhow::Error) -> String {
    match e.downcast_ref::<UpdateError>() {
        Some(u) => u.title(),
        None => "Update Failed".into(),
    }
}

/// Windows reports a locked file as a sharing violation (32) or lock violation (33);
/// everything else is a genuine I/O problem.
pub(crate) fn is_locked(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32) | Some(33))
        || e.kind() == std::io::ErrorKind::PermissionDenied
}

/// True when the manifest path names one of the `skip` files (case-insensitive).
pub fn skipped(path: &str, skip: &[&str]) -> bool {
    skip.iter().any(|s| path.eq_ignore_ascii_case(s))
}

/// Applies the update at `manifest_url` to `bin_path`, leaving the files in `skip` alone
/// (a licence-patched `pnsovr.dll`).
pub fn apply_skipping(
    manifest_url: &str,
    bin_path: &Path,
    skip: &[&str],
    cancel: &AtomicBool,
    status: &mut dyn FnMut(String),
) -> Result<()> {
    tracing::info!("UpdateService: downloading manifest {manifest_url}");
    let (manifest, _) = fetch_manifest(manifest_url)?;
    let dels: Vec<_> = manifest
        .dels()
        .filter(|e| !skipped(&e.path, skip))
        .collect();
    let adds: Vec<_> = manifest
        .adds()
        .filter(|e| !skipped(&e.path, skip))
        .collect();
    let total = dels.len() + adds.len();
    let mut current = 0;

    for e in &dels {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        current += 1;
        status(format!(
            "Updating {current}/{total}: deleting {}...",
            e.path
        ));
        let target = bin_path.join(&e.path);
        match std::fs::remove_file(&target) {
            Ok(()) => tracing::info!("UpdateService: deleted {}", e.path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) if is_locked(&err) => {
                return Err(UpdateError::FileInUse {
                    path: e.path.clone(),
                    source: err,
                }
                .into())
            }
            Err(err) => return Err(anyhow!(err).context(format!("delete {}", target.display()))),
        }
    }

    for e in &adds {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        current += 1;
        status(format!("Updating {current}/{total}: {}...", e.path));
        let target = bin_path.join(&e.path);
        let sha = e.sha256.as_deref().unwrap_or_default();
        if target.exists() && sha256_matches(&target, sha) {
            tracing::info!("UpdateService: skipping {} (already up to date)", e.path);
            continue;
        }

        let parent = target.parent().unwrap_or(bin_path);
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        // Stage next to the target so the final rename never crosses filesystems.
        let staged = parent.join(format!(".{}.download", file_name(&e.path)));
        let url = manifest.url_for(e);
        tracing::info!("UpdateService: downloading {}", redact(&url));
        let result = http::download_to(&url, &staged, Some(cancel)).and_then(|()| {
            if !sha256_matches(&staged, sha) {
                return Err(UpdateError::HashMismatch(e.path.clone()).into());
            }
            std::fs::rename(&staged, &target).map_err(|err| {
                if is_locked(&err) {
                    UpdateError::FileInUse {
                        path: e.path.clone(),
                        source: err,
                    }
                    .into()
                } else {
                    anyhow!(err).context(format!("replace {}", target.display()))
                }
            })
        });
        if result.is_err() {
            let _ = std::fs::remove_file(&staged);
        }
        result?;
        tracing::info!("UpdateService: placed {}", e.path);
    }

    tracing::info!("UpdateService: update complete");
    Ok(())
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {

    #[test]
    fn reads_files_echovr_channels_from_the_mirror() {
        // The PC channel without nEVR, from either host: the one with it.
        for old in OLD_PC_MANIFESTS {
            assert_eq!(manifest_for(old), PC_MANIFEST_URL);
        }
        assert!(PC_MANIFEST_URL.contains("/updates-nevr/"));
        assert_eq!(
            manifest_for("https://files.echovr.de/updates/quest/update.manifest"),
            "https://release.echovr.de/updates/quest/update.manifest"
        );
        assert_eq!(
            manifest_for("https://release.echovr.de/halloween2017.zip.manifest"),
            "https://release.echovr.de/halloween2017.zip.manifest"
        );
    }
    use super::*;

    /// A test channel from the environment first, else from launcher.json; anything not
    /// on release.echovr.de or files.echovr.de as updates-*/update.manifest is ignored.
    #[test]
    fn picks_the_update_channel() {
        let test = "https://release.echovr.de/updates-nevr-test/update.manifest";
        let other = "https://files.echovr.de/updates-staging/update.manifest";
        assert_eq!(pick_channel(None, None), None);
        assert_eq!(
            pick_channel(None, Some(test)),
            Some((test.into(), "launcher.json"))
        );
        assert_eq!(
            pick_channel(Some(other), Some(test)),
            Some((other.into(), MANIFEST_OVERRIDE))
        );
        assert_eq!(
            pick_channel(
                Some("https://evil.example/updates-x/update.manifest"),
                Some(test)
            ),
            Some((test.into(), "launcher.json"))
        );
        assert_eq!(
            pick_channel(None, Some("https://release.echovr.de/pc.zip.manifest")),
            None
        );
    }

    #[test]
    fn channel_overlays_replace_main_only_where_they_have_a_replacement() {
        let sha = |c: char| c.to_string().repeat(64);
        let main = format!(
            "add BugSplat64.dll {}\nadd plugins/NvrAssetPatches.dll {}\ndel dbgcore.dll\n",
            sha('a'),
            sha('b')
        );
        // No overlay: main as it is.
        let (m, text) = with_overlays(PC_MANIFEST_URL, &main, &[]).unwrap();
        assert_eq!(text, main);
        assert!(m.entries.iter().all(|e| e.base.is_none()));
        // Beta replaces BugSplat64.dll and adds a file, from its own folder.
        let beta = format!(
            "add bugsplat64.dll {}\nadd new.dll {}\n",
            sha('c'),
            sha('d')
        );
        let (m, text) =
            with_overlays(PC_MANIFEST_URL, &main, &[(BETA_OVERLAY, beta.clone())]).unwrap();
        let by = |p: &str| m.entries.iter().find(|e| e.path == p).unwrap();
        assert_eq!(m.entries.len(), 4);
        assert!(!m.entries.iter().any(|e| e.path == "BugSplat64.dll"));
        assert_eq!(by("bugsplat64.dll").sha256, Some(sha('c')));
        assert_eq!(
            m.url_for(by("bugsplat64.dll")),
            "https://release.echovr.de/updates-nevr-beta/bugsplat64.dll"
        );
        assert_eq!(
            m.url_for(by("plugins/NvrAssetPatches.dll")),
            "https://release.echovr.de/updates-nevr/plugins/NvrAssetPatches.dll"
        );
        assert!(text.starts_with(&main) && text.contains(BETA_OVERLAY));
        // Alpha: beta's, then its own on top.
        let alpha = format!("add new.dll {}\n", sha('e'));
        let (m, _) = with_overlays(
            PC_MANIFEST_URL,
            &main,
            &[(BETA_OVERLAY, beta), (ALPHA_OVERLAY, alpha)],
        )
        .unwrap();
        let new = m.entries.iter().find(|e| e.path == "new.dll").unwrap();
        assert_eq!(new.sha256, Some(sha('e')));
        assert_eq!(
            m.url_for(new),
            "https://release.echovr.de/updates-nevr-alpha/new.dll"
        );
        assert_eq!(overlays(Channel::Main), &[] as &[&str]);
        assert_eq!(overlays(Channel::Beta), &[BETA_OVERLAY]);
        assert_eq!(overlays(Channel::Alpha), &[BETA_OVERLAY, ALPHA_OVERLAY]);
    }

    #[test]
    fn staging_manifests_on_our_hosts_only() {
        assert!(override_ok(
            "https://release.echovr.de/updates-nevr/update.manifest"
        ));
        assert!(override_ok(
            "https://files.echovr.de/updates-nevr/update.manifest"
        ));
        for bad in [
            "https://release.echovr.de/updates/update.manifest",
            "http://release.echovr.de/updates-nevr/update.manifest",
            "https://release.echovr.de.evil.example/updates-nevr/update.manifest",
            "https://evr.echo.taxi/updates-nevr/update.manifest",
            "https://release.echovr.de/updates-nevr/other.manifest",
        ] {
            assert!(!override_ok(bad), "{bad}");
        }
    }
}
