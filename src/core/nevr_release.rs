//! nEVR from its own signed releases: `manifest.json` and its minisign signature at
//! `https://evr.echo.taxi/nevr-runtime/`, checked the way nevr-runtime's
//! `docs/reference/verifying-a-release.md` says, step by step:
//!
//! 1. both files, capped (1 MiB, 1 KiB), from exactly that URL;
//! 2. the signature, before anything parses the manifest: hashed (`ED`) only, by a pinned
//!    key that no manifest has revoked;
//! 3. `schema_version` 1 and `product` `nevr-runtime`;
//! 4. `manifest_version` never below the highest one seen (nor the same one with other
//!    bytes);
//! 5. not past `expires`;
//! 6. then the manifest, its signature and the state are stored (the state last);
//! 7. the release this launcher's update names ([`crate::core::manifest::Manifest::nevr_release`]),
//!    which has to be in the manifest's `releases` (or a channel's: the one its `channels`
//!    names, [`channel_version`]);
//! 8. its Windows zip: from the manifest's own folder, its size and sha256, every entry the
//!    manifest lists and nothing else, each with its size and sha256.
//!
//! Which release players get is ours to say (the update's `# nEVR <platform>:` line), so a
//! new nEVR reaches them only once it is allowed there; nEVR's signature says what the
//! files of that release are.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use super::launcher::{mods, nevr};

/// Where nEVR's releases are; `ECHOVR_NEVR_RELEASES` names another (a mirror: only what
/// the pinned keys signed passes anyway).
pub const BASE: &str = "https://evr.echo.taxi/nevr-runtime";
pub const BASE_OVERRIDE: &str = "ECHOVR_NEVR_RELEASES";

/// The `manifest_version` that was current when this launcher was built: nothing older is
/// taken, even on a first run.
pub const MIN_MANIFEST_VERSION: u64 = 1;

/// nEVR's pinned release keys (minisign), from `verifying-a-release.md`: K1 and K2 sign,
/// KR is the recovery key.
pub const PINNED: [&str; 3] = [
    "RWTA7pyFRwI9NPbeZVbRJxtiA0YsG3jolqhV8mxuCEP0AUb24092wkuy",
    "RWQ+cq9F86dyVc7yFNVU+YAVnSl5pJFun+A68lujJ1p3GsMb2v3vrK0F",
    "RWRBDKzhAOWXf+lhvvVo22yLwhwmABcky2IkqcvmLWtpG8Pm9/ux8dYx",
];

/// The platform an update's `# nEVR <platform>:` line is for: the launcher's (the game runs
/// under Proton on Linux, which a release has to be allowed for on its own).
pub const PLATFORM: &str = if cfg!(target_os = "linux") {
    "linux"
} else {
    "windows"
};

/// The files the launcher installs from a release: nEVR's host and, from 5.8.1 on, its
/// runtime.
pub const FILES: [&str; 2] = [mods::SLOT, nevr::RUNTIME];

const MANIFEST_MAX: u64 = 1 << 20;
const SIG_MAX: u64 = 1024;
const STATE: &str = "state.json";
const MANIFEST: &str = "manifest.json";
const SIG: &str = "manifest.json.sig";

/// Why nEVR's release information, or a file of a release, wasn't used: the step of the
/// guide it failed at, and the detail for the log.
#[derive(Debug, thiserror::Error)]
#[error("{} ({detail})", say(*step))]
pub struct Refused {
    pub step: u8,
    pub detail: String,
}

fn refused(step: u8, detail: impl Into<String>) -> Refused {
    Refused {
        step,
        detail: detail.into(),
    }
}

/// What the player is told for a refusal at `step` (the guide's table).
fn say(step: u8) -> &'static str {
    match step {
        1 => "Couldn't get nEVR's release information. Check your connection and try again. Nothing was changed.",
        2 => "nEVR's release information isn't signed by nEVR, so it wasn't used. Nothing was changed.",
        3 => "nEVR's release information needs a newer launcher. Nothing was changed.",
        4 => "nEVR's release information is older than what this launcher has already seen, so it wasn't used. Nothing was changed.",
        5 => "nEVR's release information is out of date. Check your computer's clock or try again later. Nothing was changed.",
        6 => "The launcher couldn't read or save what it knows about nEVR's releases. Nothing was changed.",
        7 => "nEVR's release information doesn't have the nEVR this update brings. Nothing was changed.",
        _ => "A file from nEVR's release didn't match it, so it was deleted and nothing was installed.",
    }
}

/// One file of a release's zip, as its manifest lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// A release's Windows zip, from a manifest that passed steps 2 to 6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub contents: Vec<Member>,
}

impl Release {
    /// The zip's member `path` (one of [`FILES`]), if the release has it.
    pub fn member(&self, path: &str) -> Option<&Member> {
        self.contents
            .iter()
            .find(|m| m.path.eq_ignore_ascii_case(path))
    }

    /// The files the launcher installs from it ([`FILES`] it has).
    pub fn files(&self) -> impl Iterator<Item = &Member> {
        FILES.iter().filter_map(|f| self.member(f))
    }

    /// Step 8: the zip to `to`, at most its size, then its size, sha256 and every entry
    /// checked ([`check_zip`]). `to` is gone again when it doesn't pass.
    pub fn download(&self, to: &Path, cancel: &AtomicBool) -> Result<(), Refused> {
        fetch_checked(&self.url, self.size, &self.sha256, to, cancel, &mut |_| {})?;
        check_zip(to, &self.contents).inspect_err(|_| {
            let _ = std::fs::remove_file(to);
        })
    }
}

/// One file of a release that isn't a zip (the Quest's APK): where, its size and sha256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

impl Artifact {
    /// Step 8: the file to `to`, at most its size, then its size and sha256; `on` gets how
    /// far it is (0 to 100). `to` is gone again when it doesn't pass.
    pub fn download(
        &self,
        to: &Path,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(f64),
    ) -> Result<(), Refused> {
        let size = self.size.max(1);
        fetch_checked(&self.url, self.size, &self.sha256, to, cancel, &mut |n| {
            on(100.0 * n as f64 / size as f64)
        })
    }
}

/// What [`fetch_checked`] writes, counted for its progress.
struct Counted<'a, W> {
    inner: W,
    done: u64,
    on: &'a mut dyn FnMut(u64),
}

impl<W: std::io::Write> std::io::Write for Counted<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.done += n as u64;
        (self.on)(self.done);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Step 8 for one file: at most `size` bytes of `url` to `to`, then exactly `size` and
/// `sha256`; `on` gets the bytes written so far. `to` is deleted when it doesn't pass.
fn fetch_checked(
    url: &str,
    size: u64,
    sha256: &str,
    to: &Path,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(u64),
) -> Result<(), Refused> {
    let got = (|| {
        let file =
            std::fs::File::create(to).map_err(|e| refused(8, format!("{}: {e}", to.display())))?;
        let mut file = Counted {
            inner: std::io::BufWriter::new(file),
            done: 0,
            on,
        };
        let (got, sha) = super::http::get_capped(url, size, &mut file, Some(cancel))
            .map_err(|e| refused(8, format!("{e:#}")))?;
        if got != size {
            return Err(refused(
                8,
                format!("{url} is {got} bytes, the manifest says {size}"),
            ));
        }
        if sha != sha256 {
            return Err(refused(
                8,
                format!("{url}'s sha256 differs from the manifest"),
            ));
        }
        std::io::Write::flush(&mut file).map_err(|e| refused(8, format!("{}: {e}", to.display())))
    })();
    if got.is_err() {
        let _ = std::fs::remove_file(to);
    }
    got
}

/// nEVR's Echo VR for the Quest in a release: its APK (the one build every player gets,
/// signed by nEVR) and the game data it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestRelease {
    pub version: String,
    pub apk: Artifact,
    /// The APK's `versionCode`, as Android reports it once installed.
    pub version_code: u64,
    pub signer_cert_sha256: String,
    pub data: GameData,
}

/// The game data a Quest release reads: a zip whose files go under the app's media folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameData {
    /// The zip in nEVR's release folder (`data/echovr-data-<12 hex>.zip`).
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// Its files (`_data/<data version>/rad15/android/...`), each with its size and sha256.
    pub contents: Vec<Member>,
}

/// Steps 1 to 8 up to the APK: the Quest release `which` names, a version or one of nEVR's
/// channels (the release it has now).
pub fn quest_release(which: &str, cancel: &AtomicBool) -> Result<QuestRelease, Refused> {
    let (m, base) = checked(cancel)?;
    let version = if channel_ok(which) {
        channel_in(&m, which)?
    } else {
        which.to_string()
    };
    quest_in(&m, &version, &base)
}

/// The release whose Quest APK has `version_code`, from the manifest checked last (no
/// network): what the headset has installed, by name.
pub fn quest_version_of(version_code: u64) -> Option<String> {
    let m = stored(&state_dir()).ok()?;
    m["releases"].as_object()?.iter().find_map(|(v, r)| {
        quest_apks(r)
            .iter()
            .any(|a| a["android"]["version_code"].as_u64() == Some(version_code))
            .then(|| v.clone())
    })
}
/// Where the launcher keeps what it knows about nEVR's releases.
pub fn state_dir() -> PathBuf {
    super::paths::data_dir().join("nevr-releases")
}

fn base() -> String {
    std::env::var(BASE_OVERRIDE)
        .ok()
        .filter(|b| b.starts_with("https://") || b.starts_with("http://127.0.0.1"))
        .unwrap_or_else(|| BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Steps 1 to 8 up to the zip: nEVR `version`'s release from nEVR's signed manifest.
pub fn resolve(version: &str, cancel: &AtomicBool) -> Result<Release, Refused> {
    let (m, base) = checked(cancel)?;
    release_in(&m, version, &base)
}

/// Steps 1 to 6, then the release nEVR's channel `name` has now in the checked manifest.
/// nEVR's manifest lists only its channels' latest releases, so an update follows a channel
/// rather than naming a version that soon isn't there.
pub fn channel_version(name: &str, cancel: &AtomicBool) -> Result<String, Refused> {
    let (m, _) = checked(cancel)?;
    channel_in(&m, name)
}

/// Pure: a channel's name as an update may give it, `[a-z]+` (`none` is none).
pub fn channel_ok(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_lowercase())
}

/// Pure: the release channel `name` of the checked manifest `m` has.
fn channel_in(m: &Value, name: &str) -> Result<String, Refused> {
    match m["channels"][name].as_str() {
        Some(v) if semver_ok(v) => Ok(v.to_string()),
        _ => {
            let have: Vec<&str> = m["channels"]
                .as_object()
                .map(|o| o.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let have = if have.is_empty() {
                "none".into()
            } else {
                have.join(", ")
            };
            Err(refused(
                7,
                format!("it has no channel {name} (it has {have})"),
            ))
        }
    }
}

/// Steps 1 to 6: nEVR's signed manifest, checked and stored, and its folder. A signature
/// that doesn't verify is fetched again twice, 3 seconds apart (a mirror between the two
/// files), before it counts.
fn checked(cancel: &AtomicBool) -> Result<(Value, String), Refused> {
    let base = base();
    let dir = state_dir();
    let mut tries = 0;
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(refused(1, "cancelled"));
        }
        let (bytes, sig) = fetch(&base)?;
        match accept(&bytes, &sig, &PINNED, &dir, &now()?) {
            Err(e) if e.step == 2 && tries < 2 => {
                tracing::info!("nEVR releases: {e}; fetching again");
                tries += 1;
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
            Err(e) => return Err(e),
            Ok(id) => {
                tracing::info!("nEVR releases: manifest signed by key {id} taken");
                break;
            }
        }
    }
    Ok((stored(&dir)?, base))
}

/// Step 1.
fn fetch(base: &str) -> Result<(Vec<u8>, String), Refused> {
    let get = |name: &str, max: u64| {
        let mut body = Vec::new();
        super::http::get_capped(&format!("{base}/{name}"), max, &mut body, None)
            .map(|_| body)
            .map_err(|e| refused(1, format!("{e:#}")))
    };
    let bytes = get(MANIFEST, MANIFEST_MAX)?;
    let sig = String::from_utf8(get(SIG, SIG_MAX)?)
        .map_err(|_| refused(2, "the signature file is not UTF-8"))?;
    Ok((bytes, sig))
}

/// The time now, as the manifest writes times.
fn now() -> Result<String, Refused> {
    let t = time::OffsetDateTime::now_utc();
    let now = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    );
    utc_ok(&now)
        .then_some(now)
        .ok_or_else(|| refused(5, "the clock doesn't read as a UTC time"))
}

/// What the launcher keeps between runs (`state.json`).
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct State {
    #[serde(default)]
    highest_manifest_version: Option<u64>,
    #[serde(default)]
    last_manifest_sha256: Option<String>,
    #[serde(default)]
    revoked_keys: Vec<String>,
}

/// A missing state is a first run; one that can't be read, or doesn't hold what it should,
/// is a failure (never an empty state).
fn read_state(dir: &Path) -> Result<State, Refused> {
    let path = dir.join(STATE);
    match std::fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|e| refused(6, format!("{} is not a state file: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(refused(6, format!("{}: {e}", path.display()))),
    }
}

/// Steps 2 to 6 for `bytes` and its signature `sig` with the keys `pinned`, the state in
/// `dir` and the time `now`: stored when they pass. Returns the signing key's id.
fn accept(
    bytes: &[u8],
    sig: &str,
    pinned: &[&str],
    dir: &Path,
    now: &str,
) -> Result<String, Refused> {
    use base64::Engine;
    use minisign_verify::{PublicKey, Signature};
    let b64 = base64::engine::general_purpose::STANDARD;
    if !utc_ok(now) {
        return Err(refused(
            5,
            "the current time is not UTC written as YYYY-MM-DDTHH:MM:SSZ",
        ));
    }
    if bytes.len() as u64 > MANIFEST_MAX || sig.len() as u64 > SIG_MAX {
        return Err(refused(
            1,
            "manifest.json is over 1 MiB or its signature over 1 KiB",
        ));
    }
    let state = read_state(dir)?;

    // Step 2, before any JSON parser sees the manifest.
    let blob = sig
        .lines()
        .nth(1)
        .and_then(|l| b64.decode(l.trim()).ok())
        .unwrap_or_default();
    if blob.len() != 74 {
        return Err(refused(2, "the signature is not 74 bytes"));
    }
    if &blob[..2] != b"ED" {
        return Err(refused(2, "the signature is not hashed (ED)"));
    }
    let id = hex::encode(&blob[2..10]);
    if state.revoked_keys.contains(&id) {
        return Err(refused(2, format!("signing key {id} is revoked")));
    }
    let key = pinned
        .iter()
        .find(|k| {
            b64.decode(k)
                .is_ok_and(|b| b.len() == 42 && hex::encode(&b[2..10]) == id)
        })
        .ok_or_else(|| refused(2, format!("signing key {id} is not pinned")))?;
    let key = PublicKey::from_base64(key).map_err(|e| refused(2, format!("pinned key: {e}")))?;
    let signature =
        Signature::decode(sig).map_err(|e| refused(2, format!("signature file: {e}")))?;
    key.verify(bytes, &signature, false)
        .map_err(|e| refused(2, format!("the signature does not verify: {e}")))?;

    // Step 3: the same bytes, now parsed.
    let m: Value =
        serde_json::from_slice(bytes).map_err(|e| refused(3, format!("manifest.json: {e}")))?;
    if m["schema_version"] != 1 || m["product"] != "nevr-runtime" {
        return Err(refused(
            3,
            "schema_version is not 1 or product is not nevr-runtime",
        ));
    }

    // Step 4.
    let version = m["manifest_version"]
        .as_u64()
        .ok_or_else(|| refused(3, "manifest_version is not a whole number"))?;
    let stored = state.highest_manifest_version.unwrap_or(0);
    let have = stored.max(MIN_MANIFEST_VERSION);
    let sha = super::pc_update::sha256_hex(bytes);
    if version < have {
        return Err(refused(
            4,
            format!("manifest version went down: got {version}, have {have}"),
        ));
    }
    if version == stored && state.last_manifest_sha256.as_deref() != Some(sha.as_str()) {
        return Err(refused(4, "same manifest_version, different bytes"));
    }

    // Step 5: whole-second UTC times with a Z compare as text.
    let expires = m["expires"]
        .as_str()
        .filter(|e| utc_ok(e))
        .ok_or_else(|| refused(5, "expires is not UTC written as YYYY-MM-DDTHH:MM:SSZ"))?;
    if now >= expires {
        return Err(refused(5, format!("stale since {expires}")));
    }

    // Step 6: a revocation list that's there has to be read; then the files, the state last.
    let mut revoked = state.revoked_keys;
    match m.get("revoked_keys") {
        None => {}
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => {
            for k in a.iter().filter_map(Value::as_str) {
                if !revoked.iter().any(|r| r == k) {
                    revoked.push(k.to_string());
                }
            }
        }
        Some(_) => {
            return Err(refused(
                6,
                "revoked_keys in the manifest is not a list of strings",
            ))
        }
    }
    revoked.sort();
    let state = State {
        highest_manifest_version: Some(version),
        last_manifest_sha256: Some(sha),
        revoked_keys: revoked,
    };
    let write = |name: &str, data: &[u8]| {
        let path = dir.join(name);
        let tmp = dir.join(format!("{name}.new"));
        std::fs::write(&tmp, data)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| refused(6, format!("{}: {e}", path.display())))
    };
    std::fs::create_dir_all(dir).map_err(|e| refused(6, format!("{}: {e}", dir.display())))?;
    write(MANIFEST, bytes)?;
    write(SIG, sig.as_bytes())?;
    write(
        STATE,
        &serde_json::to_vec_pretty(&state).map_err(|e| refused(6, e.to_string()))?,
    )?;
    Ok(id)
}

/// The manifest that passed step 2, read back from `dir`: the one `state.json` names.
fn stored(dir: &Path) -> Result<Value, Refused> {
    let state = read_state(dir)?;
    let bytes = std::fs::read(dir.join(MANIFEST))
        .map_err(|e| refused(8, format!("no checked manifest in {}: {e}", dir.display())))?;
    let sha = super::pc_update::sha256_hex(&bytes);
    if state.last_manifest_sha256.as_deref() != Some(sha.as_str()) {
        return Err(refused(
            8,
            "the stored manifest is not the one that passed step 2",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|e| refused(8, format!("stored manifest: {e}")))
}

/// Step 7: release `version` of the checked manifest `m`.
fn release_of<'a>(m: &'a Value, version: &str) -> Result<&'a Value, Refused> {
    if !semver_ok(version) {
        return Err(refused(7, format!("{version} is not a version X.Y.Z")));
    }
    let r = &m["releases"][version];
    if !r.is_object() || r.get("version").is_some_and(|v| v != version) {
        // A manifest lists only the releases of its channels: say which, for the allow line.
        let offered: Vec<&str> = m["releases"]
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let offered = if offered.is_empty() {
            "none".into()
        } else {
            offered.join(", ")
        };
        return Err(refused(
            7,
            format!("{version} is not in releases (it has {offered})"),
        ));
    }
    Ok(r)
}

/// A release's Quest APKs (normally one).
fn quest_apks(r: &Value) -> Vec<&Value> {
    r["artifacts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|x| x["kind"] == "quest-apk" && x["platform"] == "android-arm64")
                .collect()
        })
        .unwrap_or_default()
}

/// A release's game data for the Quest (normally one).
fn game_datas(r: &Value) -> Vec<&Value> {
    r["artifacts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|x| x["kind"] == "game-data" && x["platform"] == "android-arm64")
                .collect()
        })
        .unwrap_or_default()
}

/// Step 7 and step 8's lines for the Quest: release `version`'s APK (nEVR's package, with
/// the versionCode and signer it is installed with, or refused) and its game data, or,
/// when it lists none, the newest release's (one copy serves every release).
fn quest_in(m: &Value, version: &str, base: &str) -> Result<QuestRelease, Refused> {
    let r = release_of(m, version)?;
    let [apk] = quest_apks(r)[..] else {
        return Err(refused(
            8,
            format!("release {version} has no Quest APK, or two"),
        ));
    };
    let path = apk["path"].as_str().unwrap_or_default();
    if !path_ok(version, path) || !path.ends_with(".apk") {
        return Err(refused(
            8,
            "the APK's path is not v<version>/nevr-runtime-v<version>-<name>.apk",
        ));
    }
    let (sha256, size) = sha_and_size(apk)?;
    let android = &apk["android"];
    if android["package"] != super::adb::PACKAGE {
        return Err(refused(
            8,
            format!("the APK is not for {}", super::adb::PACKAGE),
        ));
    }
    let version_code = android["version_code"]
        .as_u64()
        .filter(|c| *c > 0)
        .ok_or_else(|| refused(8, "the APK has no versionCode"))?;
    let signer = android["signer_cert_sha256"]
        .as_str()
        .filter(|s| is_sha256_hex(s))
        .ok_or_else(|| refused(8, "the APK names no signer to check it against"))?;
    Ok(QuestRelease {
        version: version.to_string(),
        apk: Artifact {
            url: format!("{base}/{path}"),
            sha256,
            size,
        },
        version_code,
        signer_cert_sha256: signer.to_string(),
        data: game_data_for(m, version, r, base)?,
    })
}

/// Release `version`'s (`r`) game data, or the newest release's that lists one.
fn game_data_for(m: &Value, version: &str, r: &Value, base: &str) -> Result<GameData, Refused> {
    fn one<'a>(r: &'a Value, v: &str) -> Result<Option<&'a Value>, Refused> {
        match game_datas(r)[..] {
            [] => Ok(None),
            [d] => Ok(Some(d)),
            _ => Err(refused(8, format!("release {v} lists two game data"))),
        }
    }
    let data = match one(r, version)? {
        Some(d) => d,
        None => {
            let mut all: Vec<(&String, &Value)> = m["releases"]
                .as_object()
                .map(|o| o.iter().filter(|(v, _)| semver_ok(v)).collect())
                .unwrap_or_default();
            all.sort_by_key(|(v, _)| semver_key(v));
            let mut newest = None;
            for (v, r) in all.into_iter().rev() {
                if let Some(d) = one(r, v)? {
                    newest = Some(d);
                    break;
                }
            }
            newest.ok_or_else(|| refused(8, "no release lists the Quest's game data"))?
        }
    };
    let path = data["path"].as_str().unwrap_or_default();
    if !game_data_zip_ok(path) {
        return Err(refused(
            8,
            "the game data's path is not data/echovr-data-<12 hex>.zip",
        ));
    }
    let (sha256, size) = sha_and_size(data)?;
    let mut contents: Vec<Member> = Vec::new();
    for c in data["contents"].as_array().into_iter().flatten() {
        let path = c["path"].as_str().unwrap_or_default();
        if !data_path_ok(path) {
            return Err(refused(
                8,
                format!("the game data lists a path that is not allowed: {path}"),
            ));
        }
        if contents.iter().any(|m| m.path == path) {
            return Err(refused(8, format!("the game data lists {path} twice")));
        }
        let (sha256, size) = sha_and_size(c)?;
        contents.push(Member {
            path: path.to_string(),
            sha256,
            size,
        });
    }
    if contents.is_empty() {
        return Err(refused(8, "the game data lists no files"));
    }
    Ok(GameData {
        url: format!("{base}/{path}"),
        sha256,
        size,
        contents,
    })
}

/// `_data/<data version>/rad15/android/(manifests|packages)/<name>`, the name of a-z 0-9 _.
/// The one shape a game data zip's path has: a file right under `data/`.
fn game_data_zip_ok(path: &str) -> bool {
    path.strip_prefix("data/echovr-data-")
        .and_then(|n| n.strip_suffix(".zip"))
        .is_some_and(|h| h.len() == 12 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

fn data_path_ok(path: &str) -> bool {
    let p: Vec<&str> = path.split('/').collect();
    let name_ok = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'))
    };
    matches!(p[..], ["_data", v, "rad15", "android", "manifests" | "packages", name]
        if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) && name_ok(name))
}

/// `X.Y.Z` (checked with [`semver_ok`]) as numbers, for ordering.
fn semver_key(v: &str) -> (u64, u64, u64) {
    let mut n = v.split('.').map(|p| p.parse().unwrap_or(0));
    (
        n.next().unwrap_or(0),
        n.next().unwrap_or(0),
        n.next().unwrap_or(0),
    )
}

/// Step 7 and the zip's line for step 8: release `version` of the checked manifest `m`,
/// whose folder is `base`.
fn release_in(m: &Value, version: &str, base: &str) -> Result<Release, Refused> {
    let r = release_of(m, version)?;
    let zips: Vec<&Value> = r["artifacts"]
        .as_array()
        .map(|a| a.iter().filter(|x| x["kind"] == "windows-zip").collect())
        .unwrap_or_default();
    let [zip] = zips[..] else {
        return Err(refused(
            8,
            format!("release {version} has no windows-zip, or two"),
        ));
    };
    let path = zip["path"].as_str().unwrap_or_default();
    if !path_ok(version, path) {
        return Err(refused(
            8,
            "the zip's path is not v<version>/nevr-runtime-v<version>-<name>",
        ));
    }
    let (sha256, size) = sha_and_size(zip)?;
    let mut contents: Vec<Member> = Vec::new();
    for c in zip["contents"].as_array().into_iter().flatten() {
        let path = c["path"].as_str().unwrap_or_default();
        if path.is_empty() || path.split(['/', '\\']).any(|s| s == "..") || path.starts_with('/') {
            return Err(refused(
                8,
                format!("the zip lists a path that is not allowed: {path}"),
            ));
        }
        if contents.iter().any(|m| m.path == path) {
            return Err(refused(8, format!("the zip lists {path} twice")));
        }
        let (sha256, size) = sha_and_size(c)?;
        contents.push(Member {
            path: path.to_string(),
            sha256,
            size,
        });
    }
    let release = Release {
        version: version.to_string(),
        url: format!("{base}/{path}"),
        sha256,
        size,
        contents,
    };
    if release.member(mods::SLOT).is_none() {
        return Err(refused(
            8,
            format!("release {version}'s zip has no {}", mods::SLOT),
        ));
    }
    Ok(release)
}

fn sha_and_size(v: &Value) -> Result<(String, u64), Refused> {
    let sha = v["sha256"]
        .as_str()
        .filter(|s| is_sha256_hex(s))
        .ok_or_else(|| refused(8, "a sha256 in the manifest is not 64 hex digits"))?;
    let size = v["size"]
        .as_u64()
        .ok_or_else(|| refused(8, "a size in the manifest is not a whole number"))?;
    Ok((sha.to_string(), size))
}

/// Step 8 for the zip at `zip`: every entry is one `contents` lists, once, a regular file
/// of its size and sha256; and every listed one is there. Nothing is extracted before.
pub fn check_zip(zip: &Path, contents: &[Member]) -> Result<(), Refused> {
    let fail = |d: String| refused(8, d);
    let file = std::fs::File::open(zip).map_err(|e| fail(format!("{}: {e}", zip.display())))?;
    let mut archive = ::zip::ZipArchive::new(file).map_err(|e| fail(format!("not a zip: {e}")))?;
    let mut seen: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| fail(format!("entry {i}: {e}")))?;
        let name = entry.name().to_string();
        if seen.contains(&name) {
            return Err(fail(format!("{name} is in the zip twice")));
        }
        let Some(want) = contents.iter().find(|m| m.path == name) else {
            return Err(fail(format!(
                "the zip holds {name}, which the manifest doesn't list"
            )));
        };
        // 0 when the zip records no type.
        let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
        if entry.is_dir() || !matches!(kind, 0 | 0o100000) {
            return Err(fail(format!("{name} is not a regular file")));
        }
        let (size, sha) = sha_capped(entry, want.size).map_err(|e| fail(format!("{name}: {e}")))?;
        if size != want.size || sha != want.sha256 {
            return Err(fail(format!("{name} is not the file the manifest lists")));
        }
        seen.push(name);
    }
    if let Some(m) = contents.iter().find(|m| !seen.contains(&m.path)) {
        return Err(fail(format!(
            "the manifest lists {}, which the zip doesn't hold",
            m.path
        )));
    }
    Ok(())
}

/// The size and sha256 of at most `max` + 1 bytes of `r` (one more shows an oversized one).
fn sha_capped(r: impl Read, max: u64) -> std::io::Result<(u64, String)> {
    use sha2::{Digest, Sha256};
    let mut sha = Sha256::new();
    let mut r = r.take(max + 1);
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok((size, hex::encode(sha.finalize())));
        }
        sha.update(&buf[..n]);
        size += n as u64;
    }
}

/// `member` of the zip at `zip` (checked with [`check_zip`]) to `to`, checked once more.
pub fn extract(zip: &Path, member: &Member, to: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    let file = std::fs::File::open(zip).with_context(|| format!("open {}", zip.display()))?;
    let mut archive = ::zip::ZipArchive::new(file)?;
    let mut entry = archive.by_name(&member.path)?;
    let mut out = std::fs::File::create(to).with_context(|| format!("create {}", to.display()))?;
    std::io::copy(&mut (&mut entry).take(member.size + 1), &mut out)
        .with_context(|| format!("write {}", to.display()))?;
    drop(out);
    if !super::download::sha256_matches(to, &member.sha256) {
        let _ = std::fs::remove_file(to);
        return Err(refused(8, format!("{} changed after its check", member.path)).into());
    }
    Ok(())
}

/// Exactly `YYYY-MM-DDTHH:MM:SSZ`, and a real calendar time.
fn utc_ok(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.len() == 20 && b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && b[10] == b'T'
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19)
        && b[19] == b'Z')
    {
        return false;
    }
    let n = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(99);
    let (y, mo, d, h, mi, se) = (n(0..4), n(5..7), n(8..10), n(11..13), n(14..16), n(17..19));
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    (1..=days).contains(&d) && h < 24 && mi < 60 && se < 60
}

/// Plain `X.Y.Z`, no leading zeros.
pub fn semver_ok(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (*p == "0" || !p.starts_with('0'))
        })
}

/// `v<version>/nevr-runtime-v<version>-<name>`, the name of a-z 0-9 . - only.
fn path_ok(version: &str, path: &str) -> bool {
    semver_ok(version)
        && path
            .strip_prefix(&format!("v{version}/nevr-runtime-v{version}-"))
            .is_some_and(|name| {
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-'))
            })
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key that signs upstream's fixtures (docs/reference/release-verification/), never a release.
    const TEST_KEY: &str = "RWTHthwA6C/csRKFPrZPxjAVZFImiwbgTWoK+P0qiEHQ4mnKdzhbzOqA";
    const BEFORE: &str = "2026-10-26T12:00:00Z";

    fn fixture(name: &str) -> (Vec<u8>, String) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/core/testdata/nevr-release")
            .join(name);
        (
            std::fs::read(dir.join(MANIFEST)).unwrap(),
            std::fs::read_to_string(dir.join(SIG)).unwrap(),
        )
    }

    fn step(r: Result<String, Refused>) -> u8 {
        r.expect_err("should be refused").step
    }

    #[test]
    fn takes_upstreams_good_fixture_and_stores_it() {
        let dir = tempfile::tempdir().unwrap();
        let (m, s) = fixture("good");
        assert_eq!(
            accept(&m, &s, &[TEST_KEY], dir.path(), BEFORE).unwrap(),
            "c7b61c00e82fdcb1"
        );
        // Again: the same bytes are nothing new, not a refusal.
        accept(&m, &s, &[TEST_KEY], dir.path(), BEFORE).unwrap();
        let r = release_in(&stored(dir.path()).unwrap(), "5.5.1", BASE).unwrap();
        assert_eq!(
            r.url,
            "https://evr.echo.taxi/nevr-runtime/v5.5.1/nevr-runtime-v5.5.1-windows.zip"
        );
        assert_eq!((r.size, r.contents.len()), (4914229, 8));
        // 5.5.1 has no runtime yet: only its host is installed.
        assert_eq!(
            r.files().map(|m| m.path.as_str()).collect::<Vec<_>>(),
            [mods::SLOT]
        );
        assert_eq!(
            step(release_in(&stored(dir.path()).unwrap(), "5.5.2", BASE).map(|_| String::new())),
            7
        );
    }

    #[test]
    fn takes_the_live_manifest_with_the_pinned_keys() {
        let dir = tempfile::tempdir().unwrap();
        let (m, s) = fixture("live");
        accept(&m, &s, &PINNED, dir.path(), BEFORE).unwrap();
        let r = release_in(&stored(dir.path()).unwrap(), "5.8.1", BASE).unwrap();
        assert_eq!(
            r.files().map(|m| m.path.as_str()).collect::<Vec<_>>(),
            FILES
        );
        // A version the manifest no longer lists: refused, naming what it has.
        let r = release_in(&stored(dir.path()).unwrap(), "5.2.1", BASE).unwrap_err();
        assert!(r.step == 7 && r.detail.contains("(it has 5.8.1)"), "{r:?}");
        // The fixtures are not signed by a pinned key.
        let (m, s) = fixture("good");
        assert_eq!(
            step(accept(
                &m,
                &s,
                &PINNED,
                tempfile::tempdir().unwrap().path(),
                BEFORE
            )),
            2
        );
    }

    #[test]
    fn refuses_what_upstream_says_to_refuse() {
        let fresh = || tempfile::tempdir().unwrap();
        let (good, sig) = fixture("good");
        for (name, why) in [("legacy-ed", "not hashed"), ("unpinned-key", "not pinned")] {
            let (m, s) = fixture(name);
            let r = accept(&m, &s, &[TEST_KEY], fresh().path(), BEFORE).unwrap_err();
            assert!(r.step == 2 && r.detail.contains(why), "{name}: {r:?}");
        }
        // One byte of the manifest, or of the trusted comment.
        let mut tampered = good.clone();
        tampered[100] ^= 1;
        assert_eq!(
            step(accept(&tampered, &sig, &[TEST_KEY], fresh().path(), BEFORE)),
            2
        );
        let comment = sig.replace("manifest_version=1", "manifest_version=2");
        assert_eq!(
            step(accept(&good, &comment, &[TEST_KEY], fresh().path(), BEFORE)),
            2
        );
        // Same version, other bytes, once the good one was taken.
        let dir = fresh();
        accept(&good, &sig, &[TEST_KEY], dir.path(), BEFORE).unwrap();
        let (m, s) = fixture("same-version-other-bytes");
        assert_eq!(step(accept(&m, &s, &[TEST_KEY], dir.path(), BEFORE)), 4);
        // A lower version than seen.
        let dir = fresh();
        std::fs::write(dir.path().join(STATE), r#"{"highest_manifest_version": 2}"#).unwrap();
        let r = accept(&good, &sig, &[TEST_KEY], dir.path(), BEFORE).unwrap_err();
        assert_eq!(
            (r.step, r.detail.as_str()),
            (4, "manifest version went down: got 1, have 2")
        );
        // Expired, and clocks that aren't times.
        assert_eq!(
            step(accept(
                &good,
                &sig,
                &[TEST_KEY],
                fresh().path(),
                "2026-11-08T00:00:00Z"
            )),
            5
        );
        for now in [
            "",
            "yesterday",
            "2026-10-26T12:00:00",
            "2026-10-26T12:00:00Z\n",
            "2026-02-30T12:00:00Z",
        ] {
            assert_eq!(
                step(accept(&good, &sig, &[TEST_KEY], fresh().path(), now)),
                5,
                "{now:?}"
            );
        }
        // A revoked key, and a state that can't be read.
        let dir = fresh();
        std::fs::write(
            dir.path().join(STATE),
            r#"{"revoked_keys": ["c7b61c00e82fdcb1"]}"#,
        )
        .unwrap();
        assert_eq!(
            step(accept(&good, &sig, &[TEST_KEY], dir.path(), BEFORE)),
            2
        );
        for bad in [
            "not json",
            r#"{"revoked_keys": "abc"}"#,
            r#"{"highest_manifest_version": "1"}"#,
        ] {
            let dir = fresh();
            std::fs::write(dir.path().join(STATE), bad).unwrap();
            assert_eq!(
                step(accept(&good, &sig, &[TEST_KEY], dir.path(), BEFORE)),
                6,
                "{bad}"
            );
        }
        let dir = fresh();
        std::fs::create_dir(dir.path().join(STATE)).unwrap();
        assert_eq!(
            step(accept(&good, &sig, &[TEST_KEY], dir.path(), BEFORE)),
            6
        );
        // Nothing is stored for a refusal.
        let dir = fresh();
        let _ = accept(&tampered, &sig, &[TEST_KEY], dir.path(), BEFORE);
        assert!(!dir.path().join(STATE).exists());
    }

    #[test]
    fn a_stored_manifest_edited_afterwards_is_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let (m, s) = fixture("good");
        accept(&m, &s, &[TEST_KEY], dir.path(), BEFORE).unwrap();
        let mut edited = m.clone();
        edited[100] ^= 1;
        std::fs::write(dir.path().join(MANIFEST), edited).unwrap();
        assert_eq!(stored(dir.path()).unwrap_err().step, 8);
    }

    #[test]
    fn a_channel_is_the_release_it_has_now() {
        let m = serde_json::json!({"channels": {"beta": "6.0.0", "odd": "6.0"}});
        assert_eq!(channel_in(&m, "beta").unwrap(), "6.0.0");
        let e = channel_in(&m, "stable").unwrap_err();
        assert_eq!(e.step, 7);
        assert!(
            e.detail.contains("no channel stable (it has beta, odd)"),
            "{e}"
        );
        assert_eq!(channel_in(&m, "odd").unwrap_err().step, 7);
        assert!(channel_ok("beta") && !channel_ok("Beta") && !channel_ok("5.8.1"));
    }

    #[test]
    fn refuses_paths_out_of_the_release_folder() {
        for bad in [
            "../v5.5.1/nevr-runtime-v5.5.1-windows.zip",
            "/v5.5.1/nevr-runtime-v5.5.1-windows.zip",
            "https://elsewhere/v5.5.1/nevr-runtime-v5.5.1-windows.zip",
            "v5.5.1/nevr-runtime-v5.5.1-../x",
            "v5.5.1/nevr-runtime-v5.5.1-a/b.zip",
            "v5.5.1/nevr-runtime-v5.5.1-WINDOWS.zip",
            "v5.5.2/nevr-runtime-v5.5.2-windows.zip",
            "v5.5.1/nevr-runtime-v5.5.1-",
        ] {
            assert!(!path_ok("5.5.1", bad), "{bad}");
        }
        assert!(path_ok("5.5.1", "v5.5.1/nevr-runtime-v5.5.1-windows.zip"));
        for bad in ["5.5", "05.5.1", "5.5.1.0", "5.5.x", "", "5..1"] {
            assert!(!semver_ok(bad), "{bad}");
        }
    }

    #[test]
    fn takes_the_quest_release_of_the_live_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let (m, s) = fixture("quest");
        accept(&m, &s, &PINNED, dir.path(), BEFORE).unwrap();
        let m = stored(dir.path()).unwrap();
        let q = quest_in(&m, &channel_in(&m, "beta").unwrap(), BASE).unwrap();
        assert_eq!(q.version, "6.2.0");
        assert_eq!(
            q.apk.url,
            "https://evr.echo.taxi/nevr-runtime/v6.2.0/nevr-runtime-v6.2.0-quest.apk"
        );
        assert_eq!(
            (q.apk.size, q.version_code, &q.apk.sha256[..8]),
            (175496180, 60200999, "f75c3d69")
        );
        assert_eq!(&q.signer_cert_sha256[..8], "3bcb5868");
        // The same data as the launcher's _data.zip.
        assert!(q.data.sha256.starts_with("fc2eedeacc50"));
        assert!(q.data.url.ends_with("/data/echovr-data-fc2eedeacc50.zip"));
        assert_eq!((q.data.size, q.data.contents.len()), (937395400, 4));
    }

    #[test]
    fn refuses_a_quest_apk_it_cant_check() {
        let dir = tempfile::tempdir().unwrap();
        let (m, s) = fixture("quest");
        accept(&m, &s, &PINNED, dir.path(), BEFORE).unwrap();
        let good = stored(dir.path()).unwrap();
        fn apk(m: &mut Value) -> &mut Value {
            m["releases"]["6.2.0"]["artifacts"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|a| a["kind"] == "quest-apk")
                .unwrap()
        }
        type Edit = fn(&mut Value);
        let edits: [(&str, Edit); 5] = [
            ("no signer", |a| {
                a["android"]
                    .as_object_mut()
                    .unwrap()
                    .remove("signer_cert_sha256");
            }),
            ("not for", |a| {
                a["android"]["package"] = "com.example".into()
            }),
            ("no versionCode", |a| {
                a["android"]["version_code"] = 0.into()
            }),
            ("path", |a| a["path"] = "../x.apk".into()),
            ("sha256", |a| a["sha256"] = "abc".into()),
        ];
        for (why, edit) in edits {
            let mut m = good.clone();
            edit(apk(&mut m));
            let e = quest_in(&m, "6.2.0", BASE).unwrap_err();
            assert!(e.step == 8 && e.detail.contains(why), "{why}: {e:?}");
        }
        // Two APKs.
        let mut m = good.clone();
        let a = apk(&mut m).clone();
        m["releases"]["6.2.0"]["artifacts"]
            .as_array_mut()
            .unwrap()
            .push(a);
        assert!(quest_in(&m, "6.2.0", BASE)
            .unwrap_err()
            .detail
            .contains("or two"));
        // A data path out of the media folder.
        let mut m = good.clone();
        for r in ["6.1.0", "6.2.0"] {
            for a in m["releases"][r]["artifacts"].as_array_mut().unwrap() {
                if a["kind"] == "game-data" {
                    a["contents"][0]["path"] = "_data/1/../../x".into();
                }
            }
        }
        assert!(quest_in(&m, "6.2.0", BASE)
            .unwrap_err()
            .detail
            .contains("not allowed"));
    }

    #[test]
    fn game_data_comes_from_the_newest_release_that_lists_it() {
        let dir = tempfile::tempdir().unwrap();
        let (m, s) = fixture("quest");
        accept(&m, &s, &PINNED, dir.path(), BEFORE).unwrap();
        let mut m = stored(dir.path()).unwrap();
        let drop_data = |m: &mut Value, r: &str| {
            m["releases"][r]["artifacts"]
                .as_array_mut()
                .unwrap()
                .retain(|a| a["kind"] != "game-data")
        };
        drop_data(&mut m, "6.2.0");
        let q = quest_in(&m, "6.2.0", BASE).unwrap();
        assert!(q.data.sha256.starts_with("fc2eedeacc50"));
        drop_data(&mut m, "6.1.0");
        assert!(quest_in(&m, "6.2.0", BASE)
            .unwrap_err()
            .detail
            .contains("no release lists"));
        assert!(data_path_ok(
            "_data/5932408047/rad15/android/packages/48037dc70b0ecab2_0"
        ));
        for bad in [
            "_data/x/rad15/android/packages/a",
            "_data/1/rad15/android/other/a",
            "_data/1/rad15/android/packages/A",
            "_data/1/rad15/android/packages/a/b",
            "/_data/1/rad15/android/packages/a",
        ] {
            assert!(!data_path_ok(bad), "{bad}");
        }
    }

    /// A zip of `files`, with its manifest contents.
    fn zip_of(dir: &Path, files: &[(&str, &[u8])]) -> (PathBuf, Vec<Member>) {
        use std::io::Write;
        let path = dir.join("r.zip");
        let mut z = ::zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let mut contents = Vec::new();
        for (name, body) in files {
            z.start_file(*name, ::zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(body).unwrap();
            contents.push(Member {
                path: name.to_string(),
                sha256: super::super::pc_update::sha256_hex(body),
                size: body.len() as u64,
            });
        }
        z.finish().unwrap();
        (path, contents)
    }

    #[test]
    fn checks_every_entry_of_the_zip() {
        let dir = tempfile::tempdir().unwrap();
        let files: [(&str, &[u8]); 2] = [
            ("BugSplat64.dll", b"host"),
            ("nevr-runtime.dll", b"runtime"),
        ];
        let (zip, contents) = zip_of(dir.path(), &files);
        check_zip(&zip, &contents).unwrap();
        let out = dir.path().join("out");
        extract(&zip, &contents[1], &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"runtime");
        // An entry the manifest doesn't list, one it lists that isn't there, another file.
        assert_eq!(check_zip(&zip, &contents[..1]).unwrap_err().step, 8);
        let mut more = contents.clone();
        more.push(Member {
            path: "README.txt".into(),
            ..contents[0].clone()
        });
        assert!(check_zip(&zip, &more)
            .unwrap_err()
            .detail
            .contains("README.txt"));
        let mut other = contents.clone();
        other[0].sha256 = other[1].sha256.clone();
        assert_eq!(check_zip(&zip, &other).unwrap_err().step, 8);
        // A directory entry.
        let mut z = ::zip::ZipWriter::new_append(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&zip)
                .unwrap(),
        )
        .unwrap();
        z.add_directory("plugins/", ::zip::write::SimpleFileOptions::default())
            .unwrap();
        z.finish().unwrap();
        let mut with_dir = contents.clone();
        with_dir.push(Member {
            path: "plugins/".into(),
            sha256: "0".repeat(64),
            size: 0,
        });
        assert!(check_zip(&zip, &with_dir)
            .unwrap_err()
            .detail
            .contains("not a regular file"));
    }

    /// The live release a beta launcher would install. `cargo test -- --ignored` (network).
    #[test]
    #[ignore]
    fn the_live_release_downloads_and_checks() {
        let dir = tempfile::tempdir().unwrap();
        let (bytes, sig) = fetch(BASE).unwrap();
        accept(&bytes, &sig, &PINNED, dir.path(), &now().unwrap()).unwrap();
        let m = stored(dir.path()).unwrap();
        let v = m["channels"]["stable"]
            .as_str()
            .or(m["channels"]["beta"].as_str())
            .unwrap();
        let r = release_in(&m, v, BASE).unwrap();
        let zip = dir.path().join("r.zip");
        r.download(&zip, &AtomicBool::new(false)).unwrap();
        for f in r.files() {
            extract(&zip, f, &dir.path().join(&f.path)).unwrap();
        }
        let host = std::fs::read(dir.path().join(mods::SLOT)).unwrap();
        assert_eq!(nevr::version_in(&host).as_deref(), Some(v));
    }
}
