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
use super::manifest::Manifest;

pub const PC_MANIFEST_URL: &str = "https://files.echovr.de/updates/update.manifest";
/// Where to try a PC update before it goes live: another manifest on files.echovr.de
/// (e.g. `https://files.echovr.de/updates-nevr/update.manifest`) in place of the live one.
pub const MANIFEST_OVERRIDE: &str = "ECHOVR_UPDATE_MANIFEST";

/// `url`, or for the live PC update the one `ECHOVR_UPDATE_MANIFEST` names (only on
/// files.echovr.de).
pub fn manifest_for(url: &str) -> String {
    if url == PC_MANIFEST_URL {
        if let Ok(o) = std::env::var(MANIFEST_OVERRIDE) {
            if o.starts_with("https://files.echovr.de/") && o.ends_with(".manifest") {
                tracing::warn!("PC update from {o} ({MANIFEST_OVERRIDE}) instead of the live one");
                return o;
            }
            tracing::warn!("{MANIFEST_OVERRIDE} ignored: not a manifest on files.echovr.de");
        }
    }
    url.to_string()
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
    let manifest = Manifest::fetch(manifest_url)?;
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
