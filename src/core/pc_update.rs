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
use super::manifest::{Action, Entry as ManifestEntry, Manifest};
use super::nevr_release::{self, Release};

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
/// The Quest update, as installs and catalogues may have noted it before the Quest tree.
const OLD_QUEST_MANIFESTS: [&str; 2] = [
    "https://files.echovr.de/updates/quest/update.manifest",
    "https://release.echovr.de/updates/quest/update.manifest",
];
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
    for o in overlays_of(url) {
        let t = http::get_text_opt(o)
            .with_context(|| format!("Could not download the channel's update ({o})"))?;
        if let Some(t) = t {
            over.push((o, t));
        }
    }
    let (mut m, mut text) = with_overlays(url, &text, &over)?;
    // A channel of nEVR's is the release it has now, in the text too: a new one there is an
    // update.
    if let Some(name) = nevr_channel(&m) {
        let v = channel_release(&name, &AtomicBool::new(false))?;
        m.set_nevr_release(nevr_release::PLATFORM, &v);
        text.push_str(&channel_line(&name, &v));
    }
    Ok((m, text))
}

/// The channel of nEVR's `m` names for this platform, when it names one and no version.
fn nevr_channel(m: &Manifest) -> Option<String> {
    m.nevr_release(nevr_release::PLATFORM)
        .filter(|v| !nevr_release::semver_ok(v))
        .map(str::to_string)
}

/// Pure: the line the release channel `name` has now (`version`) adds to an update's text.
fn channel_line(name: &str, version: &str) -> String {
    format!(
        "\n# nEVR {} from {name}: {version}\n",
        nevr_release::PLATFORM
    )
}

/// The release nEVR's channel `name` has now.
fn channel_release(name: &str, cancel: &AtomicBool) -> Result<String> {
    nevr_release::channel_version(name, cancel).map_err(|e| {
        tracing::warn!(
            "UpdateService: nEVR's {name} channel refused at step {}: {}",
            e.step,
            e.detail
        );
        anyhow!(e).context(format!("nEVR's {name} channel couldn't be read"))
    })
}

/// The overlays the update at `url` gets: the followed channel's for the live PC update,
/// none for any other.
pub fn overlays_of(url: &str) -> Vec<&'static str> {
    if url != PC_MANIFEST_URL {
        return Vec::new();
    }
    let channel = *LAUNCHER_CHANNEL.lock().unwrap_or_else(|p| p.into_inner());
    overlays(channel).to_vec()
}

/// Pure: `text` (the manifest at `url`) with the overlays `(url, text)` on top, in order.
/// The text is what [`fetch_manifest`] gives (and an update check hashes).
pub(crate) fn with_overlays(
    url: &str,
    text: &str,
    overlays: &[(&str, String)],
) -> Result<(Manifest, String)> {
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
    // files.echovr.de update channel is read from the mirror; the Quest's from its tree.
    let moved = if OLD_PC_MANIFESTS.contains(&url) {
        Some(PC_MANIFEST_URL.to_string())
    } else if OLD_QUEST_MANIFESTS.contains(&url) {
        Some(super::quest_update::QUEST_MANIFEST_URL.to_string())
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
            UpdateError::HashMismatch(p) => format!("A download came out damaged: {p}"),
            UpdateError::FileInUse { .. } => "Please close Echo VR first".into(),
        }
    }
}

/// Dialog title for any error returned by [`apply`].
pub fn error_title(e: &anyhow::Error) -> String {
    match e.downcast_ref::<UpdateError>() {
        Some(u) => u.title(),
        None => "Couldn't update".into(),
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
    apply_keeping(manifest_url, bin_path, skip, None, cancel, status)
}

/// What an update put aside in its `previous` folder ([`apply_keeping`]), to go back.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Previous {
    /// The sha256 of the update manifest (with the channel's overlays) it brought: going
    /// back holds that one ([`crate::core::updates`] doesn't offer it again).
    pub manifest_sha256: String,
    /// nEVR's version before and after, when the update changed it.
    pub nevr_from: Option<String>,
    pub nevr_to: Option<String>,
    /// When (RFC 3339).
    pub at: Option<String>,
    /// Files it replaced or deleted: kept under `files/`.
    pub replaced: Vec<String>,
    /// Files it added that weren't there: deleted again.
    pub added: Vec<String>,
}

pub const PREVIOUS_RECORD: &str = "previous.json";
const PREVIOUS_FILES: &str = "files";

impl Previous {
    pub fn read(dir: &Path) -> Option<Previous> {
        let t = std::fs::read_to_string(dir.join(PREVIOUS_RECORD)).ok()?;
        serde_json::from_str(&t).ok()
    }
}

/// [`apply_skipping`], keeping what it replaces or deletes in `previous` (one update back,
/// written only when the update changes a file) so that [`go_back`] can put it back.
pub fn apply_keeping(
    manifest_url: &str,
    bin_path: &Path,
    skip: &[&str],
    previous: Option<&Path>,
    cancel: &AtomicBool,
    status: &mut dyn FnMut(String),
) -> Result<()> {
    tracing::info!("UpdateService: downloading manifest {manifest_url}");
    let (manifest, text) = fetch_manifest(manifest_url)?;
    apply_manifest(&manifest, &text, bin_path, skip, previous, cancel, status)
}

/// [`apply_keeping`] for a manifest at hand (`text`: what it was built from, recorded as
/// the update's checksum): an earlier version from release.echovr.de's history too
/// ([`super::launcher::history`]), whose files come by their checksums.
pub fn apply_manifest(
    manifest: &Manifest,
    text: &str,
    bin_path: &Path,
    skip: &[&str],
    previous: Option<&Path>,
    cancel: &AtomicBool,
    status: &mut dyn FnMut(String),
) -> Result<()> {
    if let Some(v) = manifest.nevr_release(nevr_release::PLATFORM) {
        status(format!("Checking nEVR {v}'s release..."));
    }
    let (manifest, release) = with_nevr_release(manifest, cancel)?;
    let manifest = &manifest;
    let dels: Vec<_> = manifest
        .dels()
        .filter(|e| !skipped(&e.path, skip))
        .collect();
    let mut adds: Vec<_> = manifest
        .adds()
        .filter(|e| !skipped(&e.path, skip))
        .collect();
    // nEVR's host first: where its runtime goes depends on it.
    adds.sort_by_key(|e| place_order(&e.path) != 2);
    let total = dels.len() + adds.len();
    let mut current = 0;
    // What this update puts aside: gathered beside the last one, which it replaces once
    // something has changed.
    let mut kept = previous.map(|dir| Keeping {
        new: dir.with_extension("new"),
        record: Previous {
            manifest_sha256: sha256_hex(text.as_bytes()),
            nevr_from: nevr_version(&bin_path.join(super::launcher::mods::SLOT)),
            at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .ok(),
            ..Previous::default()
        },
        dir: dir.to_path_buf(),
    });
    if let Some(k) = &kept {
        let _ = std::fs::remove_dir_all(&k.new);
    }

    let result = (|| -> Result<()> {
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
            let gone = match &mut kept {
                Some(k) if target.exists() => k.put_aside(&e.path, &target),
                _ => std::fs::remove_file(&target),
            };
            match gone {
                Ok(()) => tracing::info!("UpdateService: deleted {}", e.path),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) if is_locked(&err) => {
                    return Err(UpdateError::FileInUse {
                        path: e.path.clone(),
                        source: err,
                    }
                    .into())
                }
                Err(err) => {
                    return Err(anyhow!(err).context(format!("delete {}", target.display())))
                }
            }
        }

        // Every file is downloaded and checked first, so a failed download changes nothing.
        // Each with its place in the game folder (nEVR's runtime: where its host loads it).
        let mut staged: Vec<(String, std::path::PathBuf, std::path::PathBuf)> = Vec::new();
        // A nEVR release's zip, once one of its files is needed.
        let mut zip: Option<std::path::PathBuf> = None;
        let fetched = (|| -> Result<()> {
            for e in &adds {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Cancelled.into());
                }
                current += 1;
                status(format!("Updating {current}/{total}: {}...", e.path));
                let path = if e.path.eq_ignore_ascii_case(super::launcher::nevr::RUNTIME) {
                    super::launcher::nevr::runtime_entry(&host_after(&staged, bin_path)).to_string()
                } else {
                    e.path.clone()
                };
                let target = bin_path.join(&path);
                let sha = e.sha256.as_deref().unwrap_or_default();
                if target.exists() && sha256_matches(&target, sha) {
                    tracing::info!("UpdateService: skipping {} (already up to date)", e.path);
                    continue;
                }
                let parent = target.parent().unwrap_or(bin_path);
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
                // Staged next to the target so the final rename never crosses filesystems.
                let file = parent.join(format!(".{}.download", file_name(&e.path)));
                let got = match release
                    .as_ref()
                    .zip(release.as_ref().and_then(|r| r.member(&e.path)))
                {
                    Some((r, member)) => {
                        let z = match &zip {
                            Some(z) => z.clone(),
                            None => {
                                let z = bin_path
                                    .join(format!(".nevr-runtime-v{}.zip.download", r.version));
                                tracing::info!(
                                    "UpdateService: downloading nEVR {} from {}",
                                    r.version,
                                    r.url
                                );
                                r.download(&z, cancel)?;
                                zip = Some(z.clone());
                                z
                            }
                        };
                        nevr_release::extract(&z, member, &file)
                    }
                    None => {
                        let url = manifest.url_for(e);
                        tracing::info!("UpdateService: downloading {}", redact(&url));
                        http::download_checked(&url, sha, &file, Some(cancel)).and_then(|()| {
                            if sha256_matches(&file, sha) {
                                Ok(())
                            } else {
                                Err(UpdateError::HashMismatch(e.path.clone()).into())
                            }
                        })
                    }
                };
                if got.is_err() {
                    let _ = std::fs::remove_file(&file);
                }
                got?;
                staged.push((path, target, file));
            }
            Ok(())
        })();
        if let Some(z) = &zip {
            let _ = std::fs::remove_file(z);
        }
        if let Err(e) = fetched {
            for (_, _, file) in &staged {
                let _ = std::fs::remove_file(file);
            }
            return Err(e);
        }
        // Then in place: nEVR's runtime before its host, so the host never starts beside
        // a runtime of another version.
        let root_host = super::launcher::nevr::is_root_host(&host_after(&staged, bin_path));
        staged.sort_by_key(|(path, _, _)| place_order(path));
        let mut left = staged.into_iter();
        while let Some((path, target, file)) = left.next() {
            let locked = |err: std::io::Error| -> anyhow::Error {
                if is_locked(&err) {
                    UpdateError::FileInUse {
                        path: path.clone(),
                        source: err,
                    }
                    .into()
                } else {
                    anyhow!(err).context(format!("replace {}", target.display()))
                }
            };
            let placed = match &mut kept {
                Some(k) if target.exists() => {
                    k.put_aside(&path, &target).map_err(locked).and_then(|()| {
                        std::fs::rename(&file, &target).map_err(|err| {
                            // Put the old one back where it was.
                            k.take_back(&path, &target);
                            locked(err)
                        })
                    })
                }
                Some(k) => std::fs::rename(&file, &target)
                    .map_err(locked)
                    .map(|()| k.record.added.push(path.clone())),
                None => std::fs::rename(&file, &target).map_err(locked),
            };
            if let Err(err) = placed {
                let _ = std::fs::remove_file(&file);
                for (_, _, file) in left {
                    let _ = std::fs::remove_file(file);
                }
                return Err(err);
            }
            tracing::info!("UpdateService: placed {path}");
        }
        // A root host never loads a runtime beside the exe: the old layout's is moved aside
        // (into what Undo puts back, else as install.ps1 does it).
        let beside = bin_path.join(super::launcher::nevr::RUNTIME);
        if root_host && beside.is_file() {
            let moved = match &mut kept {
                Some(k) => k.put_aside(super::launcher::nevr::RUNTIME, &beside),
                None => std::fs::rename(&beside, legacy_name(&beside)),
            };
            match moved {
                Ok(()) => tracing::info!(
                    "UpdateService: moved aside the {} beside the exe (nEVR loads it from its root)",
                    super::launcher::nevr::RUNTIME
                ),
                Err(err) => tracing::warn!(
                    "UpdateService: couldn't move aside {}: {err}",
                    beside.display()
                ),
            }
        }
        Ok(())
    })();

    // Whatever changed (even before a failure) is what going back undoes.
    if let Some(mut k) = kept {
        if !k.record.replaced.is_empty() || !k.record.added.is_empty() {
            k.record.nevr_to = nevr_version(&bin_path.join(super::launcher::mods::SLOT));
            if let Err(e) = k.finish() {
                tracing::warn!("UpdateService: couldn't keep the previous files: {e:#}");
            }
        } else {
            let _ = std::fs::remove_dir_all(&k.new);
        }
    }
    result?;
    // What it shipped into plugins/ is verified by its checksum from now on.
    super::launcher::mods::note_shipped(bin_path, adds.iter().copied());
    tracing::info!("UpdateService: update complete");
    Ok(())
}

/// `manifest` with nEVR from its own signed release where it names one for this platform
/// ([`Manifest::nevr_release`]): that release's files in place of its nEVR lines, and the
/// release to take them from. Its release information is checked here, before anything
/// changes; a refusal is the error.
pub fn with_nevr_release(
    manifest: &Manifest,
    cancel: &AtomicBool,
) -> Result<(Manifest, Option<Release>)> {
    let Some(v) = manifest.nevr_release(nevr_release::PLATFORM) else {
        return Ok((manifest.clone(), None));
    };
    // A channel's line not yet read (an update not from [`fetch_manifest`]).
    let channel = nevr_channel(manifest)
        .map(|name| channel_release(&name, cancel))
        .transpose()?;
    let v = channel.as_deref().unwrap_or(v);
    let release = nevr_release::resolve(v, cancel).map_err(|e| {
        tracing::warn!(
            "UpdateService: nEVR {v}'s release refused at step {}: {}",
            e.step,
            e.detail
        );
        anyhow!(e).context(format!("nEVR {v} couldn't be installed"))
    })?;
    Ok((in_place_of(manifest, &release), Some(release)))
}

/// Pure: `manifest` with `release`'s files in place of its lines for nEVR's files.
fn in_place_of(manifest: &Manifest, release: &Release) -> Manifest {
    let mut m = manifest.clone();
    m.entries.retain(|e| {
        !nevr_release::FILES
            .iter()
            .any(|f| e.path.eq_ignore_ascii_case(f))
    });
    m.entries.extend(release.files().map(|f| ManifestEntry {
        action: Action::Add,
        path: f.path.clone(),
        sha256: Some(f.sha256.clone()),
        base: None,
    }));
    m
}

/// The files an update puts aside while it runs ([`apply_keeping`]).
struct Keeping {
    dir: std::path::PathBuf,
    new: std::path::PathBuf,
    record: Previous,
}

impl Keeping {
    fn kept(&self, path: &str) -> std::path::PathBuf {
        self.new.join(PREVIOUS_FILES).join(path)
    }

    /// Moves `target` (manifest path `path`) aside.
    fn put_aside(&mut self, path: &str, target: &Path) -> std::io::Result<()> {
        let to = self.kept(path);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::rename(target, &to).or_else(|e| {
            // Another drive (a game folder linked elsewhere): a copy, then the delete.
            if is_locked(&e) {
                return Err(e);
            }
            std::fs::copy(target, &to)?;
            std::fs::remove_file(target)
        })?;
        self.record.replaced.push(path.to_string());
        Ok(())
    }

    fn take_back(&mut self, path: &str, target: &Path) {
        if std::fs::rename(self.kept(path), target).is_ok() {
            self.record.replaced.retain(|p| p != path);
        }
    }

    /// Replaces the last update's folder with this one's.
    fn finish(self) -> Result<()> {
        write_json(&self.new.join(PREVIOUS_RECORD), &self.record)?;
        let _ = std::fs::remove_dir_all(&self.dir);
        std::fs::rename(&self.new, &self.dir)
            .with_context(|| format!("rename to {}", self.dir.display()))
    }
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("write {}", path.display()))
}

/// The host nEVR's slot will hold once the files `staged` so far are in place in `bin_path`.
fn host_after(
    staged: &[(String, std::path::PathBuf, std::path::PathBuf)],
    bin_path: &Path,
) -> Vec<u8> {
    let slot = super::launcher::mods::SLOT;
    let file = staged
        .iter()
        .find(|(path, _, _)| path.eq_ignore_ascii_case(slot))
        .map_or_else(|| bin_path.join(slot), |(_, _, file)| file.clone());
    std::fs::read(file).unwrap_or_default()
}

/// `<file>.legacy-<time>`, as nEVR's install.ps1 names a runtime it moves aside.
fn legacy_name(file: &Path) -> std::path::PathBuf {
    let stamp = time::OffsetDateTime::now_utc().unix_timestamp();
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    file.with_file_name(format!("{name}.legacy-{stamp}"))
}

/// Where a file goes in the order an update places its files: nEVR's runtime first, its
/// host (the slot) last.
fn place_order(path: &str) -> u8 {
    let name = file_name(path);
    if name.eq_ignore_ascii_case(super::launcher::nevr::RUNTIME) {
        0
    } else if name.eq_ignore_ascii_case(super::launcher::mods::SLOT) {
        2
    } else {
        1
    }
}

fn nevr_version(dll: &Path) -> Option<String> {
    super::launcher::nevr::version_in(&std::fs::read(dll).ok()?)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Puts back what the last update kept in `previous` ([`apply_keeping`]) into `bin_path`:
/// the files it replaced or deleted, and deletes the ones it added. The folder is gone
/// afterwards. Returns its record.
pub fn go_back(previous: &Path, bin_path: &Path) -> Result<Previous> {
    let record = Previous::read(previous)
        .ok_or_else(|| anyhow!("There is no earlier update to go back to."))?;
    let files = previous.join(PREVIOUS_FILES);
    // Everything first checked, so a running game stops it before anything moved.
    for p in record.replaced.iter().chain(&record.added) {
        let target = bin_path.join(p);
        if target.exists() {
            if let Err(err) = std::fs::OpenOptions::new().write(true).open(&target) {
                if is_locked(&err) {
                    return Err(UpdateError::FileInUse {
                        path: p.clone(),
                        source: err,
                    }
                    .into());
                }
            }
        }
    }
    for p in &record.added {
        let target = bin_path.join(p);
        match std::fs::remove_file(&target) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(anyhow!(e).context(format!("delete {p}")))
            }
            _ => {}
        }
        // A folder it came in (nevr-root/) goes too once it is empty.
        if let Some(dir) = target.parent().filter(|d| *d != bin_path) {
            let _ = std::fs::remove_dir(dir);
        }
    }
    for p in &record.replaced {
        let target = bin_path.join(p);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let from = files.join(p);
        std::fs::rename(&from, &target)
            .or_else(|_| std::fs::copy(&from, &target).map(|_| ()))
            .with_context(|| format!("put back {p}"))?;
    }
    let _ = std::fs::remove_dir_all(previous);
    // The plugins put back aren't the ones the update noted.
    super::launcher::mods::forget_shipped(bin_path);
    tracing::info!(
        "UpdateService: went back ({} files put back, {} removed)",
        record.replaced.len(),
        record.added.len()
    );
    Ok(record)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    /// A server for `files` (path, body); anything else is a 404.
    fn serve(files: Vec<(&'static str, &'static str)>) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let resp = match files.iter().find(|(p, _)| req.url() == format!("/{p}")) {
                    Some((_, body)) => tiny_http::Response::from_string(*body),
                    None => tiny_http::Response::from_string("").with_status_code(404),
                };
                let _ = req.respond(resp);
            }
        });
        base
    }

    fn sha(body: &str) -> String {
        super::sha256_hex(body.as_bytes())
    }

    /// nEVR from 5.8 on: its host and runtime arrive together, or not at all.
    #[test]
    fn an_update_places_nevrs_pair_or_nothing() {
        use std::sync::atomic::AtomicBool;
        let (host, runtime) = ("new host", "new runtime");
        let base = serve(vec![
            ("BugSplat64.dll", host),
            ("nevr-runtime.dll", runtime),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let previous = dir.path().join("previous-update");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("BugSplat64.dll"), "old nevr").unwrap();
        let apply = |text: &str| {
            let m = super::Manifest::parse(text, &format!("{base}/update.manifest")).unwrap();
            super::apply_manifest(
                &m,
                text,
                &bin,
                &[],
                Some(&previous),
                &AtomicBool::new(false),
                &mut |_| {},
            )
        };
        // A file that can't be had: nothing changed, nothing left staged.
        let broken = format!(
            "add BugSplat64.dll {}\nadd nevr-runtime.dll {}\nadd Gone.dll {}\n",
            sha(host),
            sha(runtime),
            sha("gone")
        );
        assert!(apply(&broken).is_err());
        assert_eq!(
            std::fs::read_to_string(bin.join("BugSplat64.dll")).unwrap(),
            "old nevr"
        );
        assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 1);
        // Both, and going back takes both away again.
        let whole = format!(
            "add BugSplat64.dll {}\nadd nevr-runtime.dll {}\n",
            sha(host),
            sha(runtime)
        );
        apply(&whole).unwrap();
        assert_eq!(
            std::fs::read_to_string(bin.join("BugSplat64.dll")).unwrap(),
            host
        );
        assert_eq!(
            std::fs::read_to_string(bin.join("nevr-runtime.dll")).unwrap(),
            runtime
        );
        super::go_back(&previous, &bin).unwrap();
        assert_eq!(
            std::fs::read_to_string(bin.join("BugSplat64.dll")).unwrap(),
            "old nevr"
        );
        assert!(!bin.join("nevr-runtime.dll").exists());
    }

    /// A host that loads its runtime from nEVR's root (nevr-runtime #1104 on): its runtime
    /// goes to the folder's copy for the root, the old one beside the exe is moved aside,
    /// and going back puts the old layout back.
    #[test]
    fn a_root_hosts_runtime_goes_to_the_copy_for_the_root() {
        use crate::core::launcher::nevr::{ROOT_COPY, RUNTIME};
        use std::sync::atomic::AtomicBool;
        // A host by its identity alone, and a root host by its root's name.
        let host: &'static str = "MZ nevr_root_override\0NEVR-BUILD 5.17.0 abcdef0\0";
        let runtime = "new runtime";
        let base = serve(vec![
            ("BugSplat64.dll", host),
            ("nevr-runtime.dll", runtime),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let previous = dir.path().join("previous-update");
        std::fs::create_dir_all(&bin).unwrap();
        // The old pair beside the exe.
        std::fs::write(bin.join("BugSplat64.dll"), "old host").unwrap();
        std::fs::write(bin.join(RUNTIME), "old runtime").unwrap();
        let text = format!(
            "add BugSplat64.dll {}\nadd nevr-runtime.dll {}\n",
            sha(host),
            sha(runtime)
        );
        let apply = |previous: Option<&std::path::Path>| {
            let m = super::Manifest::parse(&text, &format!("{base}/update.manifest")).unwrap();
            super::apply_manifest(
                &m,
                &text,
                &bin,
                &[],
                previous,
                &AtomicBool::new(false),
                &mut |_| {},
            )
        };
        apply(Some(&previous)).unwrap();
        assert_eq!(
            std::fs::read_to_string(bin.join("BugSplat64.dll")).unwrap(),
            host
        );
        assert_eq!(
            std::fs::read_to_string(bin.join(ROOT_COPY)).unwrap(),
            runtime
        );
        assert!(!bin.join(RUNTIME).exists());
        // Again: nothing to do, nothing moved.
        apply(None).unwrap();
        assert_eq!(
            std::fs::read_to_string(bin.join(ROOT_COPY)).unwrap(),
            runtime
        );
        // Undo: the old host and the runtime beside it, and no copy for the root.
        super::go_back(&previous, &bin).unwrap();
        assert_eq!(
            std::fs::read_to_string(bin.join("BugSplat64.dll")).unwrap(),
            "old host"
        );
        assert_eq!(
            std::fs::read_to_string(bin.join(RUNTIME)).unwrap(),
            "old runtime"
        );
        assert!(!bin.join("nevr-root").exists());
        // Without a record to go back to, a runtime beside a root host is renamed as install.ps1 does.
        apply(None).unwrap();
        let names: Vec<String> = std::fs::read_dir(&bin)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!bin.join(RUNTIME).exists());
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("nevr-runtime.dll.legacy-")),
            "{names:?}"
        );
    }

    #[test]
    fn a_nevr_release_replaces_the_updates_nevr_lines() {
        let sha = |c: char| c.to_string().repeat(64);
        let m = Manifest::parse(
            &format!(
                "# nEVR windows: 5.8.1\nadd LibOVRRT64_1.dll {}\nadd BugSplat64.dll {}\nadd nevr-runtime.dll {}\ndel dbgcore.dll\n",
                sha('a'), sha('b'), sha('c')
            ),
            "https://h/u/update.manifest",
        )
        .unwrap();
        let member = |path: &str, c| nevr_release::Member {
            path: path.into(),
            sha256: sha(c),
            size: 1,
        };
        let release = Release {
            version: "5.8.1".into(),
            url: "https://evr.echo.taxi/nevr-runtime/v5.8.1/nevr-runtime-v5.8.1-windows.zip".into(),
            sha256: sha('0'),
            size: 1,
            contents: vec![
                member("BugSplat64.dll", 'd'),
                member("README.txt", 'e'),
                member("nevr-runtime.dll", 'f'),
            ],
        };
        let got = in_place_of(&m, &release);
        let lines: Vec<_> = got
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.sha256.clone().unwrap_or_default()))
            .collect();
        assert_eq!(
            lines,
            [
                ("LibOVRRT64_1.dll", sha('a')),
                ("dbgcore.dll", String::new()),
                ("BugSplat64.dll", sha('d')),
                ("nevr-runtime.dll", sha('f')),
            ]
        );
    }

    /// An update naming nEVR `ECHOVR_TEST_NEVR` for this platform, applied to the game folder
    /// `ECHOVR_TEST_BIN` (what it replaces kept beside it); `ECHOVR_TEST_BACK=1` goes back
    /// instead. For a game copy on the box: `cargo test -- --ignored nevr_release_into`.
    #[test]
    #[ignore]
    fn nevr_release_into_a_game_folder() {
        use std::sync::atomic::AtomicBool;
        let bin = std::path::PathBuf::from(std::env::var("ECHOVR_TEST_BIN").unwrap());
        let previous = bin.join(".nevr-release-test-previous");
        if std::env::var_os("ECHOVR_TEST_BACK").is_some() {
            let r = go_back(&previous, &bin).unwrap();
            println!(
                "back: {:?} -> {:?}, put back {:?}, removed {:?}",
                r.nevr_to, r.nevr_from, r.replaced, r.added
            );
            return;
        }
        // A whole update manifest (ECHOVR_TEST_MANIFEST, its URL: a development pair), or
        // just a release of nEVR's (ECHOVR_TEST_NEVR).
        let (m, text) = match std::env::var("ECHOVR_TEST_MANIFEST") {
            Ok(url) => fetch_manifest(&url).unwrap(),
            Err(_) => {
                let v = std::env::var("ECHOVR_TEST_NEVR").unwrap();
                let text = format!("# nEVR {}: {v}\n", nevr_release::PLATFORM);
                let m = Manifest::parse(
                    &text,
                    "https://release.echovr.de/updates-nevr/update.manifest",
                )
                .unwrap();
                (m, text)
            }
        };
        apply_manifest(
            &m,
            &text,
            &bin,
            &[],
            Some(&previous),
            &AtomicBool::new(false),
            &mut |s| println!("{s}"),
        )
        .unwrap();
        let r = Previous::read(&previous).unwrap();
        println!(
            "nEVR {:?} -> {:?}, replaced {:?}, added {:?}",
            r.nevr_from, r.nevr_to, r.replaced, r.added
        );
    }

    #[test]
    fn places_the_runtime_before_its_host() {
        let mut paths = vec![
            "BugSplat64.dll",
            "plugins/X.dll",
            "NEVR-RUNTIME.DLL",
            "a.bin",
        ];
        paths.sort_by_key(|p| super::place_order(p));
        assert_eq!(
            paths,
            [
                "NEVR-RUNTIME.DLL",
                "plugins/X.dll",
                "a.bin",
                "BugSplat64.dll"
            ]
        );
    }

    #[test]
    fn going_back_puts_back_what_an_update_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let previous = dir.path().join("_local/previous-update");
        std::fs::create_dir_all(crate::core::launcher::mods::plugins_in(&bin)).unwrap();
        std::fs::write(bin.join("BugSplat64.dll"), "old nevr").unwrap();
        std::fs::write(bin.join("plugins/Gone.dll"), "deleted by the update").unwrap();
        // What apply_keeping does: one replaced, one deleted, one new.
        let mut k = Keeping {
            dir: previous.clone(),
            new: previous.with_extension("new"),
            record: Previous {
                manifest_sha256: "abc".into(),
                ..Previous::default()
            },
        };
        k.put_aside("BugSplat64.dll", &bin.join("BugSplat64.dll"))
            .unwrap();
        std::fs::write(bin.join("BugSplat64.dll"), "new nevr").unwrap();
        k.put_aside("plugins/Gone.dll", &bin.join("plugins/Gone.dll"))
            .unwrap();
        std::fs::write(bin.join("plugins/New.dll"), "new").unwrap();
        k.record.added.push("plugins/New.dll".into());
        k.finish().unwrap();
        assert!(!previous.with_extension("new").exists());
        assert_eq!(Previous::read(&previous).unwrap().replaced.len(), 2);

        let r = go_back(&previous, &bin).unwrap();
        assert_eq!(r.manifest_sha256, "abc");
        assert_eq!(
            std::fs::read_to_string(bin.join("BugSplat64.dll")).unwrap(),
            "old nevr"
        );
        assert_eq!(
            std::fs::read_to_string(bin.join("plugins/Gone.dll")).unwrap(),
            "deleted by the update"
        );
        assert!(!bin.join("plugins/New.dll").exists());
        assert!(!previous.exists());
        assert!(go_back(&previous, &bin).is_err());
    }

    #[test]
    fn a_new_update_replaces_what_the_last_one_kept() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let previous = dir.path().join("previous-update");
        std::fs::create_dir_all(&bin).unwrap();
        for (body, sha) in [("one", "1"), ("two", "2")] {
            std::fs::write(bin.join("a.dll"), body).unwrap();
            let mut k = Keeping {
                dir: previous.clone(),
                new: previous.with_extension("new"),
                record: Previous {
                    manifest_sha256: sha.into(),
                    ..Previous::default()
                },
            };
            k.put_aside("a.dll", &bin.join("a.dll")).unwrap();
            std::fs::write(bin.join("a.dll"), "next").unwrap();
            k.finish().unwrap();
        }
        assert_eq!(Previous::read(&previous).unwrap().manifest_sha256, "2");
        go_back(&previous, &bin).unwrap();
        assert_eq!(std::fs::read_to_string(bin.join("a.dll")).unwrap(), "two");
    }

    #[test]
    fn reads_files_echovr_channels_from_the_mirror() {
        // The PC channel without nEVR, from either host: the one with it.
        for old in OLD_PC_MANIFESTS {
            assert_eq!(manifest_for(old), PC_MANIFEST_URL);
        }
        assert!(PC_MANIFEST_URL.contains("/updates-nevr/"));
        // The Quest's, from either host: the Quest tree's.
        for old in OLD_QUEST_MANIFESTS {
            assert_eq!(
                manifest_for(old),
                "https://release.echovr.de/quest/updates/update.manifest"
            );
        }
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
    fn an_overlay_may_follow_a_channel_of_nevrs() {
        let main = format!("add BugSplat64.dll {}\n", "a".repeat(64));
        let alpha = format!(
            "# nEVR windows: beta\n# nEVR linux: beta\n# nEVR {}: beta\n",
            nevr_release::PLATFORM
        );
        let (m, text) = with_overlays(PC_MANIFEST_URL, &main, &[(ALPHA_OVERLAY, alpha)]).unwrap();
        assert_eq!(nevr_channel(&m).as_deref(), Some("beta"));
        // The release it has goes into the text an update check hashes, as a line that is
        // nothing when the text is read again.
        let a = format!("{text}{}", channel_line("beta", "6.0.0"));
        let b = format!("{text}{}", channel_line("beta", "6.0.1"));
        assert_ne!(a, b);
        let again = Manifest::parse(&a, PC_MANIFEST_URL).unwrap();
        assert_eq!(again.nevr_release(nevr_release::PLATFORM), Some("beta"));
        let mut m = m;
        m.set_nevr_release(nevr_release::PLATFORM, "6.0.0");
        assert_eq!(nevr_channel(&m), None);
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
