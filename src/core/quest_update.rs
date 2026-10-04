//! Quest counterpart to `pc_update`: applies the same manifest format over adb.
//!
//! A Quest update is only safe against the exact APK the manifest was built for, so every
//! run is gated by [`check_version`]. Discord-personalized APKs are repacked -- their
//! SHA-256 can never equal the manifest's `BASE_APK` hash -- so a marker file written at
//! install time records which base version an install corresponds to. The marker lives on
//! the headset, so an update works from any machine. See [`decide`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use super::adb::{self, devices::Status, PACKAGE};
use super::download::{redact, sha256_file, sha256_matches};
use super::error::UiError;
use super::http::{self, Cancelled};
use super::manifest::Manifest;
use super::quest_install::with_reconnect;

pub const QUEST_MANIFEST_URL: &str = "https://files.echovr.de/updates/quest/update.manifest";

pub fn marker_path() -> String {
    format!("/sdcard/Android/media/{PACKAGE}/.echo_installer_version")
}

/// Cap on one batched `sha256sum` invocation, in paths and in characters.
const HASH_BATCH_PATHS: usize = 50;
const HASH_BATCH_CHARS: usize = 3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionCheck {
    Ok,
    NotInstalled,
    Mismatch,
    NoDevice,
    ManifestError,
}

#[derive(Debug, Clone)]
pub struct CheckStatus {
    pub result: VersionCheck,
    /// Human-readable reason, shown verbatim in the mismatch dialog.
    pub detail: String,
    pub manifest: Option<Manifest>,
    /// A missing marker should be back-filled (stock base APK recognised).
    pub self_heal_marker: bool,
}

impl CheckStatus {
    fn new(result: VersionCheck, detail: impl Into<String>) -> Self {
        CheckStatus {
            result,
            detail: detail.into(),
            manifest: None,
            self_heal_marker: false,
        }
    }

    pub fn is_ok(&self) -> bool {
        self.result == VersionCheck::Ok
    }
}

/// On-device record of which base version an install came from.
///
/// Android wipes `/sdcard/Android/media/<pkg>` when the app is uninstalled, so a marker
/// can never outlive its install.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marker {
    pub base_apk: Option<String>,
    pub base_sha256: Option<String>,
    pub installed_sha256: Option<String>,
    pub patched: bool,
    pub installed_at: Option<String>,
    pub installer_version: Option<String>,
}

impl Marker {
    /// Tolerant parser: any line without '=' is ignored, so adb/cat noise is harmless.
    pub fn parse(text: &str) -> Option<Marker> {
        let mut m = Marker::default();
        let mut saw_any = false;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if key.is_empty() {
                continue;
            }
            let v = || (!value.is_empty()).then(|| value.to_string());
            match key {
                "base_apk" => {
                    m.base_apk = v();
                    saw_any = true;
                }
                "base_sha256" => {
                    m.base_sha256 = v();
                    saw_any = true;
                }
                "installed_sha256" => {
                    m.installed_sha256 = v();
                    saw_any = true;
                }
                "patched" => {
                    m.patched = value.eq_ignore_ascii_case("true");
                    saw_any = true;
                }
                "installed_at" => m.installed_at = v(),
                "installer_version" => m.installer_version = v(),
                _ => {}
            }
        }
        saw_any.then_some(m)
    }

    pub fn serialize(&self) -> String {
        let s = |o: &Option<String>| o.clone().unwrap_or_default();
        format!(
            "version=1\nbase_apk={}\nbase_sha256={}\ninstalled_sha256={}\npatched={}\ninstalled_at={}\ninstaller_version={}\n",
            s(&self.base_apk),
            s(&self.base_sha256),
            s(&self.installed_sha256),
            self.patched,
            s(&self.installed_at),
            s(&self.installer_version)
        )
    }
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// First whitespace-delimited token of `output`, if it is a 64-char hex hash.
pub fn first_hash_token(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_whitespace().next())
        .find(|t| is_sha256_hex(t))
        .map(str::to_ascii_lowercase)
}

/// Parses batched `sha256sum` output into path -> hash; noise lines are skipped.
pub fn parse_hash_listing(output: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut parts = line.splitn(2, char::is_whitespace);
        let (Some(hash), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        if is_sha256_hex(hash) {
            map.insert(path.trim().to_string(), hash.to_ascii_lowercase());
        }
    }
    map
}

/// Decides whether an update may proceed. Pure -- no device access.
///
/// | Installed | Marker | Condition                              | Result          |
/// |-----------|--------|----------------------------------------|-----------------|
/// | no        | -      | -                                      | NotInstalled    |
/// | yes       | yes    | base matches and installed hash agrees | Ok              |
/// | yes       | yes    | base matches, installed hash differs   | Mismatch        |
/// | yes       | yes    | base differs                           | Mismatch        |
/// | yes       | no     | installed hash == manifest base        | Ok + self-heal  |
/// | yes       | no     | otherwise                              | Mismatch        |
pub fn decide(
    manifest: Option<Manifest>,
    marker: Option<Marker>,
    installed: bool,
    installed_sha: Option<String>,
) -> CheckStatus {
    let Some(manifest) = manifest else {
        return CheckStatus::new(
            VersionCheck::ManifestError,
            "The update manifest could not be downloaded.",
        );
    };
    let with = |result, detail: &str, heal| CheckStatus {
        result,
        detail: detail.to_string(),
        manifest: Some(manifest.clone()),
        self_heal_marker: heal,
    };
    if !installed {
        return with(
            VersionCheck::NotInstalled,
            "Echo VR is not installed on your Quest.",
            false,
        );
    }
    let Some(base) = manifest.base_apk_sha.as_deref() else {
        // No BASE_APK header: nothing to gate on.
        return with(VersionCheck::Ok, "", false);
    };

    if let Some(m) = &marker {
        if let Some(marker_base) = &m.base_sha256 {
            if !base.eq_ignore_ascii_case(marker_base) {
                let detail = match &m.base_apk {
                    Some(apk) => format!(
                        "The Echo VR version on your Quest is older than this update (installed from {apk})."
                    ),
                    None => "The Echo VR version on your Quest is older than this update.".into(),
                };
                return with(VersionCheck::Mismatch, &detail, false);
            }
            if let (Some(sha), Some(recorded)) = (&installed_sha, &m.installed_sha256) {
                if !sha.eq_ignore_ascii_case(recorded) {
                    return with(
                        VersionCheck::Mismatch,
                        "The Echo VR app on your Quest was replaced since it was installed.",
                        false,
                    );
                }
            }
            return with(VersionCheck::Ok, "", false);
        }
    }

    // No marker: only a stock base APK can be recognised, and we back-fill the marker.
    if installed_sha
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case(base))
    {
        return with(VersionCheck::Ok, "", true);
    }
    with(
        VersionCheck::Mismatch,
        "The Echo VR version installed on your Quest could not be matched to this update.",
        false,
    )
}

// ---- device queries ----

/// Absolute path of the installed base APK, keyed off a `package:` line rather than the
/// exit code (builds disagree on what "not installed" exits with).
pub fn installed_apk_path() -> Option<String> {
    let out = adb::exec(&["shell", "pm", "path", PACKAGE]);
    out.output
        .lines()
        .filter_map(|l| l.trim().strip_prefix("package:"))
        .map(str::trim)
        .find(|p| !p.is_empty())
        .map(str::to_string)
}

/// SHA-256 of the installed APK, hashed on the device; falls back to pulling it.
pub fn installed_apk_sha(apk_path: &str) -> Option<String> {
    if let Some(h) = first_hash_token(&adb::exec(&["shell", "sha256sum", apk_path]).output) {
        return Some(h);
    }
    tracing::info!("on-device sha256sum unusable, pulling the APK to hash it locally");
    let dir = tempfile_dir().ok()?;
    let local = dir.join("installed.apk");
    let result = if adb::pull(apk_path, &local) {
        sha256_file(&local).ok()
    } else {
        None
    };
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn tempfile_dir() -> Result<std::path::PathBuf> {
    let d = super::paths::temp_dir().join(format!("quest-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

pub fn read_marker() -> Option<Marker> {
    Marker::parse(&adb::exec(&["shell", "cat", &marker_path()]).output)
}

/// Writes the marker by pushing a locally built file.
pub fn write_marker(marker: &Marker) -> bool {
    let Ok(dir) = tempfile_dir() else {
        return false;
    };
    let local = dir.join("marker.txt");
    let ok = std::fs::write(&local, marker.serialize()).is_ok() && {
        adb::shell(&format!("mkdir -p /sdcard/Android/media/{PACKAGE}"));
        adb::push_file(&local, &marker_path())
    };
    let _ = std::fs::remove_dir_all(&dir);
    tracing::info!("marker write {}", if ok { "ok" } else { "FAILED" });
    ok
}

pub fn write_marker_after_install(
    base_apk: Option<String>,
    base_sha: Option<String>,
    installed_sha: Option<String>,
    patched: bool,
) -> bool {
    let now = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .ok();
    write_marker(&Marker {
        base_apk,
        base_sha256: base_sha,
        installed_sha256: installed_sha,
        patched,
        installed_at: now,
        installer_version: Some(crate::version::VERSION_TITLE.to_string()),
    })
}

/// The pre-flight gate. Runs on a worker thread.
pub fn check_version(manifest_url: &str, status: &mut dyn FnMut(String)) -> CheckStatus {
    status("Checking your Quest...".into());
    if let Err(e) = adb::bundle::binary() {
        return CheckStatus::new(
            VersionCheck::ManifestError,
            format!("Could not check for updates:\n{e:#}"),
        );
    }
    match adb::probe().status {
        Status::Ready => {}
        Status::Ambiguous => {
            return CheckStatus::new(
                VersionCheck::NoDevice,
                "More than one device is plugged in, so the updater can't tell which one is your Quest.\nUnplug the others and try again.",
            )
        }
        Status::Unauthorized => {
            return CheckStatus::new(
                VersionCheck::NoDevice,
                "Your Quest hasn't allowed this PC yet.\nPut the headset on, accept the USB debugging prompt, and try again.",
            )
        }
        Status::None => return CheckStatus::new(VersionCheck::NoDevice, "Your Quest is no longer connected."),
    }

    status("Checking for updates...".into());
    let manifest = match Manifest::fetch(manifest_url) {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!("could not fetch {manifest_url}: {e:#}");
            None
        }
    };
    status("Checking your Echo VR version...".into());
    let apk = installed_apk_path();
    let installed_sha = apk.as_deref().and_then(installed_apk_sha);
    let marker = if apk.is_some() { read_marker() } else { None };
    let st = decide(manifest, marker, apk.is_some(), installed_sha.clone());
    if st.is_ok() && st.self_heal_marker {
        tracing::info!("no marker found, back-filling from the base APK");
        let m = st.manifest.as_ref();
        write_marker_after_install(
            m.and_then(|m| m.base_apk_name.clone()),
            m.and_then(|m| m.base_apk_sha.clone()),
            installed_sha,
            false,
        );
    }
    st
}

/// Applies `manifest` to the headset: `del` entries first, then `add` entries whose
/// on-device hash differs.
pub fn apply(
    manifest: &Manifest,
    cancel: &AtomicBool,
    status: &mut dyn FnMut(String),
) -> Result<()> {
    let Some(root) = manifest.target_root.clone() else {
        return Err(UiError::new(
            "Update Failed",
            "The Quest update manifest is missing its target location.\nUpdate aborted.",
        )
        .into());
    };
    adb::bundle::binary()?;

    let dels: Vec<_> = manifest.dels().collect();
    let adds: Vec<_> = manifest.adds().collect();
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
        let script = format!("rm -rf {root}/{}", e.path);
        if !with_reconnect(&format!("rm {}", e.path), status, || {
            adb::shell(&script).success()
        }) {
            tracing::warn!("could not delete {}, continuing", e.path);
        }
    }

    let remote_hashes = remote_hashes(&root, &adds, status);
    let staging = tempfile_dir()?;
    let result = (|| -> Result<()> {
        let mut last_parent: Option<String> = None;
        for e in &adds {
            if cancel.load(Ordering::Relaxed) {
                return Err(Cancelled.into());
            }
            current += 1;
            let sha = e.sha256.as_deref().unwrap_or_default();
            let remote = format!("{root}/{}", e.path);
            if remote_hashes
                .get(&e.path)
                .is_some_and(|h| h.eq_ignore_ascii_case(sha))
            {
                status(format!(
                    "Updating {current}/{total}: {} (up to date)",
                    e.path
                ));
                continue;
            }
            status(format!("Updating {current}/{total}: {}...", e.path));

            let local = staging.join("file.tmp");
            let url = manifest.url_for(e);
            tracing::info!("downloading {}", redact(&url));
            http::download_to(&url, &local, Some(cancel))
                .with_context(|| format!("Downloading {} failed", e.path))?;
            if !sha256_matches(&local, sha) {
                return Err(UiError::new(
                    format!("Hash mismatch for {}", e.path),
                    format!(
                        "The downloaded file {} does not match the expected SHA-256 checksum.\nUpdate aborted.",
                        e.path
                    ),
                )
                .into());
            }
            let parent = remote
                .rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or(root.clone());
            if last_parent.as_ref() != Some(&parent) {
                adb::shell(&format!("mkdir -p {parent}"));
                last_parent = Some(parent);
            }
            if !with_reconnect(&format!("push {}", e.path), status, || {
                adb::push_file(&local, &remote)
            }) {
                return Err(UiError::new(
                    "Transfer Failed",
                    format!("Failed to copy {} to your Quest.\nUpdate aborted.", e.path),
                )
                .into());
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result?;

    // /sdcard is a FUSE mount; chmod may be a no-op or fail. Never fatal.
    let chmod = adb::shell(&format!("chmod -R 777 {root}"));
    if !chmod.success() {
        tracing::info!("chmod on {root} returned {:?} (ignored)", chmod.code);
    }
    tracing::info!("Quest update complete");
    Ok(())
}

/// Hashes the manifest's target files on the device in as few adb round-trips as possible.
fn remote_hashes(
    root: &str,
    adds: &[&super::manifest::Entry],
    status: &mut dyn FnMut(String),
) -> HashMap<String, String> {
    let mut hashes = HashMap::new();
    if adds.is_empty() {
        return hashes;
    }
    if first_hash_token(&adb::exec(&["shell", "sha256sum", "/system/build.prop"]).output).is_none()
    {
        tracing::info!("sha256sum unavailable on device, pushing all files");
        return hashes;
    }
    status("Checking existing files...".into());
    let mut seen = HashSet::new();
    let mut batch: Vec<&str> = Vec::new();
    let mut chars = 0;
    let flush = |batch: &mut Vec<&str>, hashes: &mut HashMap<String, String>| {
        // Paths are validated by the manifest parser: no shell metacharacters.
        let script = format!("cd {root} && sha256sum {} 2>/dev/null", batch.join(" "));
        hashes.extend(parse_hash_listing(&adb::shell(&script).output));
        batch.clear();
    };
    for e in adds {
        if !seen.insert(e.path.as_str()) {
            continue;
        }
        if !batch.is_empty()
            && (batch.len() >= HASH_BATCH_PATHS || chars + e.path.len() > HASH_BATCH_CHARS)
        {
            flush(&mut batch, &mut hashes);
            chars = 0;
        }
        batch.push(&e.path);
        chars += e.path.len() + 1;
    }
    if !batch.is_empty() {
        flush(&mut batch, &mut hashes);
    }
    hashes
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "0a7fa5f9d2b8c1e4f6a3b5c7d9e1f2a4b6c8d0e2f4a6b8c0d2e4f6a8b0c2d4e6";
    const OTHER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const PATCHED: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    fn manifest() -> Manifest {
        Manifest::parse(
            &format!("# BASE_APK: echo.apk {BASE}\n# Target: /sdcard/Android/media/com.readyatdawn.r15\n"),
            "https://x/y/update.manifest",
        )
        .unwrap()
    }

    fn marker(base: &str, installed: &str) -> Marker {
        Marker {
            base_apk: Some("echo.apk".into()),
            base_sha256: Some(base.into()),
            installed_sha256: Some(installed.into()),
            patched: true,
            ..Default::default()
        }
    }

    #[test]
    fn decision_table() {
        assert_eq!(
            decide(None, None, true, None).result,
            VersionCheck::ManifestError
        );
        assert_eq!(
            decide(Some(manifest()), None, false, None).result,
            VersionCheck::NotInstalled
        );

        let ok = decide(
            Some(manifest()),
            Some(marker(BASE, PATCHED)),
            true,
            Some(PATCHED.into()),
        );
        assert!(ok.is_ok() && !ok.self_heal_marker);

        let replaced = decide(
            Some(manifest()),
            Some(marker(BASE, PATCHED)),
            true,
            Some(OTHER.into()),
        );
        assert_eq!(replaced.result, VersionCheck::Mismatch);
        assert!(replaced.detail.contains("replaced"));

        let older = decide(
            Some(manifest()),
            Some(marker(OTHER, PATCHED)),
            true,
            Some(PATCHED.into()),
        );
        assert_eq!(older.result, VersionCheck::Mismatch);
        assert!(older.detail.contains("installed from echo.apk"));

        let heal = decide(Some(manifest()), None, true, Some(BASE.to_uppercase()));
        assert!(heal.is_ok() && heal.self_heal_marker);

        assert_eq!(
            decide(Some(manifest()), None, true, Some(OTHER.into())).result,
            VersionCheck::Mismatch
        );
        assert_eq!(
            decide(Some(manifest()), None, true, None).result,
            VersionCheck::Mismatch
        );

        let no_header = Manifest::parse("", "u/m").unwrap();
        assert!(decide(Some(no_header), None, true, None).is_ok());
    }

    #[test]
    fn marker_round_trip_and_noise() {
        let m = marker(BASE, PATCHED);
        assert_eq!(Marker::parse(&m.serialize()), Some(m));
        assert_eq!(
            Marker::parse("cat: /sdcard/x: No such file or directory"),
            None
        );
        assert_eq!(Marker::parse(""), None);
    }

    #[test]
    fn hash_parsing() {
        assert_eq!(
            first_hash_token(&format!("{BASE}  /data/app/base.apk\n")),
            Some(BASE.into())
        );
        assert_eq!(first_hash_token("sha256sum: not found"), None);
        let listing = format!(
            "{BASE}  a/b.pak\nsha256sum: c: No such file\n{}  d e.pak\n",
            OTHER.to_uppercase()
        );
        let m = parse_hash_listing(&listing);
        assert_eq!(m.get("a/b.pak").map(String::as_str), Some(BASE));
        assert_eq!(m.get("d e.pak").map(String::as_str), Some(OTHER));
        assert_eq!(m.len(), 2);
    }
}
