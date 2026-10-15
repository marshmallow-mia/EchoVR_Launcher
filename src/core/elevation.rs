//! Elevation broker for the few admin-only steps (Revive install, artwork into Program Files,
//! Echo VR's entry in Revive's SteamVR library manifest, repairing the game files of an
//! Echo VR in a folder only administrators may write).
//!
//! Operations are first tried in-process. Only if one fails for lack of rights does the
//! app ask for consent and relaunch *itself* elevated as `--admin-helper <pipe> <pid>`;
//! the helper is then reused for the rest of the session (one UAC prompt).
//!
//! Trust model (replacing the Java localhost socket, whose token sat in a user-readable
//! temp file so any same-user process could connect first and run anything as admin):
//! * the unelevated app creates the pipe with a random name, `FILE_FLAG_FIRST_PIPE_INSTANCE`
//!   (nobody can pre-create or share it) and an ACL for only the current user and the
//!   Administrators group; remote clients are rejected;
//! * after the connect, the app checks the client PID is the process it just launched,
//!   and the helper checks the server PID is its parent;
//! * the helper honours a fixed set of operations whose inputs it validates itself: the
//!   installer must hash to the pinned Revive build (checked through a handle that denies
//!   writes while it runs), and the artwork is downloaded by the helper and extracted,
//!   Zip-Slip-safe, into one fixed folder (inside the Meta app's install, as its
//!   administrator-only registry key names it); the library entry only ever touches Echo
//!   VR's own entry in Revive's manifest, built by the helper around an existing game
//!   executable; the licence patch only ever goes into the Meta library's Echo VR (found
//!   from the same registry key, never from the client), from a DLL the helper holds
//!   against writes while checking it is the one the app downloaded; repaired game files
//!   only ever go into a folder holding Echo VR, and only files the build's checksums on
//!   the release server list, with those checksums (held against writes from the check to
//!   the copy), never its settings, and never through a link out of that folder.

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const HELPER_FLAG: &str = "--admin-helper";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Ping,
    /// Run the Revive installer at this path -- only if it is the pinned build.
    RunReviveInstaller {
        path: String,
    },
    InstallArtwork,
    /// Put Echo VR into SteamVR's library, starting this game executable with these
    /// launch options -- or take it out (`exe: None`).
    SetLibraryEntry {
        exe: Option<String>,
        args: String,
    },
    /// Put the licence patch at this path, whose SHA-256 is this, into an Echo VR: the
    /// Meta library's (`bin: None`), or an installed version's bin folder.
    ApplyPatch {
        dll: String,
        sha256: String,
        bin: Option<String>,
    },
    /// Take the licence patch off an Echo VR (as for `ApplyPatch`).
    RemovePatch {
        bin: Option<String>,
    },
    /// Put EchoXR (its pinned zip, at this path) into an Echo VR (as for `ApplyPatch`), and
    /// have it make its copy of the game there.
    InstallEchoXr {
        zip: String,
        bin: Option<String>,
    },
    /// Add this game folder (an installed version's bin folder, nothing else) to Windows
    /// Security's exclusions, so it stops taking nEVR's BugSplat64.dll away.
    ExcludeFromDefender {
        dir: String,
    },
    /// Put the game files fetched again into `staging` over the Echo VR in `game` (the
    /// folder holding `bin/`): each one only if the build's checksums at `manifest_url`
    /// list it, with that checksum.
    RepairGameFiles {
        game: String,
        manifest_url: String,
        staging: String,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    Ok,
    ExitCode(i32),
    Err(String),
}

#[cfg_attr(not(windows), allow(dead_code))]
const MAX_MESSAGE: u32 = 64 * 1024;

#[cfg_attr(not(windows), allow(dead_code))]
pub fn write_msg<T: Serialize>(w: &mut impl Write, msg: &T) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn read_msg<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_MESSAGE {
        bail!("oversized message ({len} bytes)");
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

/// Pipe names are ours only: `\\.\pipe\echovr-launcher-<32 hex>`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn valid_pipe_name(name: &str) -> bool {
    name.strip_prefix(r"\\.\pipe\echovr-launcher-")
        .is_some_and(|rest| rest.len() == 32 && rest.chars().all(|c| c.is_ascii_hexdigit()))
}

/// What the helper does for one request. Platform-independent so it can be tested.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn handle(req: &Request) -> Reply {
    match req {
        Request::Ping | Request::Shutdown => Reply::Ok,
        Request::RunReviveInstaller { path } => match run_pinned_installer(path) {
            Ok(code) => Reply::ExitCode(code),
            Err(e) => Reply::Err(format!("{e:#}")),
        },
        Request::InstallArtwork => {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            match super::revive::install_artwork(&cancel) {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(format!("{e:#}")),
            }
        }
        Request::SetLibraryEntry { exe, args } => match library_entry_for(exe.as_deref(), args)
            .and_then(super::revive::set_library_entry)
        {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(format!("{e:#}")),
        },
        Request::ApplyPatch { dll, sha256, bin } => {
            match target_bin(bin.as_deref()).and_then(|b| apply_patch_in(&b, dll, sha256)) {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(format!("{e:#}")),
            }
        }
        Request::RemovePatch { bin } => {
            match target_bin(bin.as_deref()).and_then(|bin| super::launcher::patch::remove(&bin)) {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(format!("{e:#}")),
            }
        }
        Request::InstallEchoXr { zip, bin } => {
            match target_bin(bin.as_deref()).and_then(|bin| install_meta_echoxr(&bin, zip)) {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(format!("{e:#}")),
            }
        }
        Request::ExcludeFromDefender { dir } => {
            let bins: Vec<std::path::PathBuf> = super::launcher::store::LauncherState::load()
                .versions
                .iter()
                .map(|v| v.bin_dir())
                .collect();
            match known_bin(dir, &bins).and_then(|d| add_defender_exclusion(&d)) {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(format!("{e:#}")),
            }
        }
        Request::RepairGameFiles {
            game,
            manifest_url,
            staging,
        } => match repair_from_staging(game, manifest_url, staging) {
            Ok(n) => {
                tracing::info!("helper: repaired {n} game file(s) in {game}");
                Reply::Ok
            }
            Err(e) => Reply::Err(format!("{e:#}")),
        },
    }
}

/// The helper's repair: the build's checksums from the release server (its own fetch),
/// then [`apply_staged`].
fn repair_from_staging(game: &str, manifest_url: &str, staging: &str) -> Result<usize> {
    let trusted =
        manifest_url.starts_with("https://") && super::launcher::catalog::is_safe_url(manifest_url);
    if !trusted {
        bail!("refusing checksums from {manifest_url}");
    }
    let manifest = super::manifest::Manifest::fetch(manifest_url)?;
    apply_staged(Path::new(game), &manifest, Path::new(staging))
}

/// Puts the files in `staging` (laid out as in the game folder) over the Echo VR in
/// `game`, after checking them all: `game` holds the game's executable; each file is one
/// `manifest` lists (not the game's settings, `_local/`, nor crash dumps, `_temp/`), with
/// its checksum, read through a handle that denies writes until it is copied; and none
/// lands outside `game` through a link. Nothing is written unless all of them pass. How
/// many it put in place.
fn apply_staged(
    game: &Path,
    manifest: &super::manifest::Manifest,
    staging: &Path,
) -> Result<usize> {
    let game =
        std::fs::canonicalize(game).with_context(|| format!("{} isn't there", game.display()))?;
    let has_exe = super::paths::BIN_DIRS.iter().any(|dir| {
        super::paths::GAME_EXES
            .iter()
            .any(|exe| game.join(dir).join(exe).is_file())
    });
    if !has_exe {
        bail!("{} isn't an Echo VR folder", game.display());
    }
    let mut files = Vec::new();
    staged_files(staging, "", &mut files)?;
    let mut checked = Vec::new();
    for rel in files {
        let lower = rel.to_ascii_lowercase();
        if !super::manifest::is_safe_path(&rel)
            || lower.starts_with("_local/")
            || lower.starts_with("_temp/")
        {
            bail!("refusing {rel}: not a game file");
        }
        let Some(want) = manifest
            .adds()
            .find(|e| e.path.eq_ignore_ascii_case(&rel))
            .and_then(|e| e.sha256.clone())
        else {
            bail!("refusing {rel}: the build's checksums don't list it");
        };
        let mut f = open_locked(&staging.join(&rel).to_string_lossy())?;
        let got = super::download::sha256_reader(&mut f)?;
        if !got.eq_ignore_ascii_case(&want) {
            bail!("refusing {rel}: it isn't the build's (sha256 {got})");
        }
        let target = game.join(&rel);
        within(&game, &target)?;
        checked.push((rel, f, target));
    }
    let n = checked.len();
    for (rel, mut f, target) in checked {
        use std::io::Seek;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
            within(&game, parent)?;
        }
        f.rewind()?;
        let part = target.with_file_name(format!(
            "{}.part",
            target.file_name().unwrap_or_default().to_string_lossy()
        ));
        let copied =
            std::fs::File::create(&part).and_then(|mut out| std::io::copy(&mut f, &mut out));
        if let Err(e) = copied {
            let _ = std::fs::remove_file(&part);
            return Err(e).with_context(|| format!("write {}", part.display()));
        }
        std::fs::rename(&part, &target).with_context(|| {
            let _ = std::fs::remove_file(&part);
            format!("Couldn't replace {rel}. Please close Echo VR and try again.")
        })?;
    }
    Ok(n)
}

/// The files under `dir` (paths relative to the staging folder, with `/`), refusing links.
fn staged_files(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = std::fs::symlink_metadata(entry.path())?.file_type();
        if kind.is_symlink() {
            bail!("refusing {rel}: a link");
        } else if kind.is_dir() {
            staged_files(&entry.path(), &rel, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// That `path` (or, while it doesn't exist yet, its nearest folder that does) is inside
/// `game` (resolved) once links are followed.
fn within(game: &Path, path: &Path) -> Result<()> {
    let existing = path
        .ancestors()
        .find(|p| p.exists())
        .with_context(|| format!("{} has no folder", path.display()))?;
    let real = std::fs::canonicalize(existing)?;
    if !real.starts_with(game) {
        bail!(
            "refusing {}: it leads out of the game folder",
            path.display()
        );
    }
    Ok(())
}

/// The Echo VR bin folder a request is about: the Meta library's (`None`, found from its
/// administrator-only registry key), or `bin` when it is an installed version's (the
/// launcher's own list) with the game's executable in it.
fn target_bin(bin: Option<&str>) -> Result<std::path::PathBuf> {
    let Some(bin) = bin else {
        return meta_base().and_then(|base| meta_bin_in(&base));
    };
    let bins: Vec<std::path::PathBuf> = super::launcher::store::LauncherState::load()
        .versions
        .iter()
        .map(|v| v.bin_dir())
        .collect();
    let dir = known_bin(bin, &bins)?;
    if !super::paths::GAME_EXES
        .iter()
        .any(|e| dir.join(e).is_file())
    {
        bail!("{} holds no Echo VR", dir.display());
    }
    Ok(dir)
}

/// `dir` when it is one of `bins` (the installed versions' bin folders), as it is on disk:
/// the helper adds no other folder to Windows Security's exclusions.
fn known_bin(dir: &str, bins: &[std::path::PathBuf]) -> Result<std::path::PathBuf> {
    let dir = std::fs::canonicalize(dir).with_context(|| format!("{dir} isn't there"))?;
    if bins
        .iter()
        .filter_map(|b| std::fs::canonicalize(b).ok())
        .any(|b| b == dir)
    {
        Ok(dir)
    } else {
        bail!("{} isn't an Echo VR folder the launcher has", dir.display())
    }
}

/// Pure: `s` as a PowerShell string literal (single quotes: nothing in it is expanded).
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Adds the folder `dir` to Windows Security's exclusions (needs administrator rights).
fn add_defender_exclusion(dir: &std::path::Path) -> Result<()> {
    if !cfg!(windows) {
        bail!("Windows Security is Windows'");
    }
    // canonicalize gives `\\?\C:\...`: Windows Security wants the plain path.
    let plain = dir.to_string_lossy();
    let plain = plain.strip_prefix(r"\\?\").unwrap_or(&plain);
    let out = super::process::command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!(
            "Add-MpPreference -ExclusionPath {}",
            ps_quote(plain)
        ))
        .output()
        .context("couldn't run PowerShell")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!(
            "Windows Security didn't take the exclusion ({}): {}",
            out.status,
            err.trim()
        );
    }
    tracing::info!("added {plain} to Windows Security's exclusions");
    Ok(())
}

/// The helper's EchoXR: the zip at `zip` into the Meta library's Echo VR (`bin`), if it is
/// the pinned build (held against writes from the check to the copy), then EchoXR's copy
/// of the game (EchoXR brings its Platform SDK stand-in).
fn install_meta_echoxr(bin: &std::path::Path, zip: &str) -> Result<()> {
    if !std::path::Path::new(zip).is_absolute() {
        bail!("refusing EchoXR at {zip:?}");
    }
    let mut f = open_locked(zip)?;
    if !super::echoxr::is_pinned_zip(&mut f)? {
        bail!("refusing an EchoXR zip that isn't the pinned build");
    }
    super::echoxr::install_zip(f, bin)?;
    super::echoxr::refresh_openxr_exe(bin)?;
    super::echoxr::make_openxr_exe(bin)
}

/// The Meta (Oculus) app's folder, as its administrator-only registry key names it.
fn meta_base() -> Result<String> {
    match super::platform::oculus_base_path() {
        Some(base) => Ok(base),
        None => bail!("the Meta Quest app isn't installed"),
    }
}

/// The bin folder of Echo VR in the Meta library under the Meta app's folder `base`,
/// resolved, after checking it is one: the game's executable in it, and no link leading
/// out of `base`.
fn meta_bin_in(base: &str) -> Result<std::path::PathBuf> {
    let sep = if base.ends_with(['\\', '/']) { "" } else { "/" };
    let root = format!("{base}{sep}Software/Software");
    let exe = super::paths::exe_in(&root, super::paths::GAME_EXES[0]);
    let Some(bin) = exe.parent().filter(|_| exe.is_file()) else {
        bail!("Echo VR isn't installed in the Meta library");
    };
    let bin = std::fs::canonicalize(bin)?;
    if !bin.starts_with(std::fs::canonicalize(base)?)
        || !bin.join(super::paths::GAME_EXES[0]).is_file()
    {
        bail!("refusing the Meta library's Echo VR at {}", bin.display());
    }
    Ok(bin)
}

/// The helper's licence patch: the DLL at `dll` into the Echo VR in `bin`, if it is a
/// DLL of a sane size named like the patch and hashes to `sha256` (what the app
/// downloaded). The file is held against writes from the check to the copy.
fn apply_patch_in(bin: &Path, dll: &str, sha256: &str) -> Result<()> {
    use std::io::{Read, Seek};
    let path = std::path::Path::new(dll);
    let named = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(super::launcher::patch::DLL));
    if !path.is_absolute() || !named {
        bail!("refusing a licence patch at {dll:?}");
    }
    let mut f = open_locked(dll)?;
    let mut magic = [0u8; 2];
    if f.metadata()?.len() > super::launcher::patch::MAX_SIZE
        || f.read_exact(&mut magic).is_err()
        || &magic != b"MZ"
    {
        bail!("refusing a licence patch that isn't a DLL");
    }
    f.rewind()?;
    if !super::download::sha256_reader(&mut f)?.eq_ignore_ascii_case(sha256) {
        bail!("refusing a licence patch that changed since it was downloaded");
    }
    f.rewind()?;
    super::launcher::patch::install(bin, &mut f)
}

/// Opens `path` for reading, denying writes to everyone else while it is open (Windows).
fn open_locked(path: &str) -> Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        opts.share_mode(FILE_SHARE_READ);
    }
    Ok(opts.open(path)?)
}

/// The library entry for `exe` with `args`, after checking both: an existing game
/// executable by absolute path, and launch options of a sane size without control
/// characters. `None` (take the entry out) for no `exe`.
fn library_entry_for(exe: Option<&str>, args: &str) -> Result<Option<serde_json::Value>> {
    let Some(exe) = exe else {
        return Ok(None);
    };
    let path = std::path::Path::new(exe);
    let named = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
        super::paths::GAME_EXES
            .iter()
            .any(|g| g.eq_ignore_ascii_case(n))
    });
    if !path.is_absolute() || !named || exe.contains('"') || !path.is_file() {
        bail!("refusing a library entry for {exe:?}: not an Echo VR executable");
    }
    if args.len() > 2000 || args.chars().any(char::is_control) {
        bail!("refusing a library entry with these launch options");
    }
    Ok(Some(super::revive::library_entry(path, args)))
}

/// Opens the installer so nobody can modify it, verifies it is the pinned Revive build,
/// then runs it silently while still holding the file.
fn run_pinned_installer(path: &str) -> Result<i32> {
    let mut f = open_locked(path)?;
    if !super::download::sha256_reader(&mut f)?
        .eq_ignore_ascii_case(super::revive::REVIVE_INSTALLER_SHA256)
    {
        bail!("refusing to run an installer that is not the pinned Revive build");
    }
    let status = super::process::command(path).arg("/S").status()?;
    drop(f);
    Ok(status.code().unwrap_or(-1))
}

// ---------------------------------------------------------------------------------------
// Client side (the normal, unelevated app)
// ---------------------------------------------------------------------------------------

/// Runs the Revive installer, elevating (after `consent`) if launching it needs admin.
pub fn run_revive_installer(
    path: &std::path::Path,
    consent: &mut dyn FnMut() -> bool,
) -> Result<i32> {
    match run_pinned_installer(&path.to_string_lossy()) {
        Ok(code) => Ok(code),
        Err(e) if super::revive::needs_elevation(&e) => {
            tracing::info!("installer needs elevation ({e:#}); using the helper");
            match broker::request(
                &Request::RunReviveInstaller {
                    path: path.to_string_lossy().into(),
                },
                consent,
            )? {
                Reply::ExitCode(c) => Ok(c),
                Reply::Ok => Ok(0),
                Reply::Err(m) => bail!("{m}"),
            }
        }
        Err(e) => Err(e),
    }
}

/// Installs the Revive artwork, elevating (after `consent`) if Program Files is locked.
pub fn install_artwork(consent: &mut dyn FnMut() -> bool) -> Result<()> {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    match super::revive::install_artwork(&cancel) {
        Ok(()) => Ok(()),
        Err(e) if super::revive::needs_elevation(&e) => {
            tracing::info!("artwork needs elevation ({e:#}); using the helper");
            match broker::request(&Request::InstallArtwork, consent)? {
                Reply::Err(m) => bail!("{m}"),
                _ => Ok(()),
            }
        }
        Err(e) => Err(e),
    }
}

/// Puts Echo VR into SteamVR's library (or takes it out: `exe` `None`), elevating (after
/// `consent`) if Revive's folder is locked.
pub fn set_library_entry(
    exe: Option<&std::path::Path>,
    args: &str,
    consent: &mut dyn FnMut() -> bool,
) -> Result<()> {
    let exe = exe.map(|p| p.to_string_lossy().into_owned());
    match library_entry_for(exe.as_deref(), args).and_then(super::revive::set_library_entry) {
        Ok(()) => Ok(()),
        Err(e) if super::revive::needs_elevation(&e) => {
            tracing::info!("library entry needs elevation ({e:#}); using the helper");
            let req = Request::SetLibraryEntry {
                exe,
                args: args.to_string(),
            };
            match broker::request(&req, consent)? {
                Reply::Err(m) => bail!("{m}"),
                _ => Ok(()),
            }
        }
        Err(e) => Err(e),
    }
}

/// How a request names the Echo VR in `bin`: none for the Meta library's (the helper
/// finds that one itself), else the folder (one of the launcher's versions').
fn bin_for_helper(bin: &std::path::Path) -> Option<String> {
    (!is_meta_bin(bin)).then(|| bin.to_string_lossy().into_owned())
}

/// Puts the licence patch `dll` into the game's bin folder `bin`. When that needs
/// administrator rights, the helper does it (after `consent`).
pub fn apply_patch(
    bin: &std::path::Path,
    dll: &std::path::Path,
    consent: &mut dyn FnMut() -> bool,
) -> Result<()> {
    match super::launcher::patch::apply(bin, dll) {
        Err(e) if super::revive::needs_elevation(&e) => {
            tracing::info!("licence patch needs elevation ({e:#}); using the helper");
            let req = Request::ApplyPatch {
                dll: dll.to_string_lossy().into(),
                sha256: super::download::sha256_file(dll)?,
                bin: bin_for_helper(bin),
            };
            match broker::request(&req, consent)? {
                Reply::Err(m) => bail!("{m}"),
                _ => Ok(()),
            }
        }
        r => r,
    }
}

/// Takes the licence patch off the game's bin folder `bin`, as [`apply_patch`] puts it on.
pub fn remove_patch(bin: &std::path::Path, consent: &mut dyn FnMut() -> bool) -> Result<()> {
    match super::launcher::patch::remove(bin) {
        Err(e) if super::revive::needs_elevation(&e) => {
            tracing::info!("removing the licence patch needs elevation ({e:#}); using the helper");
            let req = Request::RemovePatch {
                bin: bin_for_helper(bin),
            };
            match broker::request(&req, consent)? {
                Reply::Err(m) => bail!("{m}"),
                _ => Ok(()),
            }
        }
        r => r,
    }
}

/// Gets the game's bin folder `bin` ready for EchoXR: EchoXR in it (with its Platform SDK
/// stand-in), and its copy of the game current. When that needs administrator rights, the
/// helper does it (after `consent`).
pub fn prepare_echoxr(bin: &std::path::Path, consent: &mut dyn FnMut() -> bool) -> Result<()> {
    match super::echoxr::prepare(bin) {
        Err(e) if super::revive::needs_elevation(&e) => {
            tracing::info!("EchoXR needs elevation ({e:#}); using the helper");
            let req = Request::InstallEchoXr {
                zip: super::echoxr::zip_file().to_string_lossy().into(),
                bin: bin_for_helper(bin),
            };
            match broker::request(&req, consent)? {
                Reply::Err(m) => bail!("{m}"),
                _ => Ok(()),
            }
        }
        r => r,
    }
}

/// Adds the game's bin folder `bin` to Windows Security's exclusions: always through the
/// helper (after `consent`), which takes only an installed version's bin folder.
pub fn exclude_from_defender(
    bin: &std::path::Path,
    consent: &mut dyn FnMut() -> bool,
) -> Result<()> {
    let req = Request::ExcludeFromDefender {
        dir: bin.to_string_lossy().into(),
    };
    match broker::request(&req, consent)? {
        Reply::Err(m) => bail!("{m}"),
        _ => Ok(()),
    }
}

/// Puts the game files fetched again into `staging` over the Echo VR in `game` through
/// the helper (after `consent`), which checks them against the build's checksums at
/// `manifest_url` itself: for a game folder only administrators may write.
pub fn repair_game_files(
    game: &std::path::Path,
    manifest_url: &str,
    staging: &std::path::Path,
    consent: &mut dyn FnMut() -> bool,
) -> Result<()> {
    let req = Request::RepairGameFiles {
        game: game.to_string_lossy().into(),
        manifest_url: manifest_url.into(),
        staging: staging.to_string_lossy().into(),
    };
    match broker::request(&req, consent)? {
        Reply::Err(m) => bail!("{m}"),
        _ => Ok(()),
    }
}

/// Whether `bin` is the Meta library's Echo VR (the only folder the helper patches).
fn is_meta_bin(bin: &std::path::Path) -> bool {
    let meta = meta_base().and_then(|base| meta_bin_in(&base));
    matches!((meta, std::fs::canonicalize(bin)), (Ok(m), Ok(b)) if m == b)
}

/// Tells a running helper to exit (on app shutdown).
pub fn shutdown() {
    broker::shutdown();
}

#[cfg(not(windows))]
mod broker {
    use super::{Reply, Request};
    use anyhow::{bail, Result};

    pub fn request(_req: &Request, _consent: &mut dyn FnMut() -> bool) -> Result<Reply> {
        bail!("Administrator elevation is only supported on Windows.")
    }

    pub fn shutdown() {}
}

#[cfg(windows)]
mod broker {
    use super::{read_msg, valid_pipe_name, write_msg, Reply, Request, HELPER_FLAG};
    use anyhow::{anyhow, bail, Context, Result};
    use std::fs::File;
    use std::os::windows::io::FromRawHandle;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Security::Authorization::*;
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::Storage::FileSystem::*;
    use windows_sys::Win32::System::Pipes::*;
    use windows_sys::Win32::System::Threading::*;
    use windows_sys::Win32::UI::Shell::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    pub(super) fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Conn {
        pipe: File,
    }

    static CONN: Mutex<Option<Conn>> = Mutex::new(None);

    pub fn request(req: &Request, consent: &mut dyn FnMut() -> bool) -> Result<Reply> {
        let mut guard = CONN.lock().unwrap_or_else(|p| p.into_inner());
        // Reuse a live helper.
        if let Some(c) = guard.as_mut() {
            match exchange(&mut c.pipe, req) {
                Ok(r) => return Ok(r),
                Err(e) => {
                    tracing::warn!("elevated helper lost ({e:#}); relaunching");
                    *guard = None;
                }
            }
        }
        if !consent() {
            bail!("Administrator rights were declined.");
        }
        let pipe = launch().context("Couldn't start the elevated helper")?;
        *guard = Some(Conn { pipe });
        exchange(&mut guard.as_mut().expect("just set").pipe, req)
    }

    fn exchange(pipe: &mut File, req: &Request) -> Result<Reply> {
        write_msg(pipe, req)?;
        read_msg(pipe)
    }

    pub fn shutdown() {
        let mut guard = CONN.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = guard.as_mut() {
            let _ = write_msg(&mut c.pipe, &Request::Shutdown);
        }
        *guard = None;
    }

    /// "S-1-5-21-..." of the current user.
    fn current_user_sid() -> Result<String> {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                bail!("OpenProcessToken failed ({})", GetLastError());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                bail!("GetTokenInformation failed ({})", GetLastError());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut s: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
                bail!("ConvertSidToStringSidW failed ({})", GetLastError());
            }
            let mut n = 0;
            while *s.add(n) != 0 {
                n += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
            LocalFree(s.cast());
            Ok(sid)
        }
    }

    fn create_pipe(name: &str) -> Result<HANDLE> {
        let sddl = format!("D:P(A;;GA;;;{})(A;;GA;;;BA)", current_user_sid()?);
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(&sddl).as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            ) == 0
            {
                bail!("security descriptor ({})", GetLastError());
            }
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            };
            let h = CreateNamedPipeW(
                wide(name).as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                &sa,
            );
            LocalFree(sd);
            if h == INVALID_HANDLE_VALUE {
                bail!("CreateNamedPipeW failed ({})", GetLastError());
            }
            Ok(h)
        }
    }

    /// Launches the elevated helper and returns the verified pipe connected to it.
    fn launch() -> Result<File> {
        let name = format!(
            r"\\.\pipe\echovr-launcher-{}",
            hex::encode(rand::random::<[u8; 16]>())
        );
        debug_assert!(valid_pipe_name(&name));
        let pipe = create_pipe(&name)?;
        // SAFETY: we own the handle; File closes it.
        let pipe_file = unsafe { File::from_raw_handle(pipe as _) };

        let exe = std::env::current_exe()?;
        let params = format!("{HELPER_FLAG} {name} {}", std::process::id());
        let verb = wide("runas");
        let file = wide(&exe.to_string_lossy());
        let params_w = wide(&params);
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params_w.as_ptr();
        info.nShow = SW_HIDE;
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_CANCELLED {
                bail!("The Windows administrator prompt was declined.");
            }
            bail!("ShellExecuteExW failed ({err})");
        }
        let child = info.hProcess;
        let child_pid = unsafe { GetProcessId(child) };

        // ConnectNamedPipe blocks; run it on a thread so a helper that dies (or never
        // starts) can be noticed. We unblock it by connecting to ourselves if needed.
        let raw = pipe as usize;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let ok = unsafe { ConnectNamedPipe(raw as HANDLE, std::ptr::null_mut()) } != 0
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            let _ = tx.send(ok);
        });
        let deadline = Instant::now() + Duration::from_secs(60);
        let connected = loop {
            if let Ok(ok) = rx.recv_timeout(Duration::from_millis(200)) {
                break ok;
            }
            let exited = unsafe { WaitForSingleObject(child, 0) } == WAIT_OBJECT_0;
            if exited || Instant::now() >= deadline {
                // Unblock the connect thread, then reject whatever connected.
                let _ = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&name);
                let _ = rx.recv_timeout(Duration::from_secs(2));
                unsafe { CloseHandle(child) };
                bail!("the elevated helper did not start");
            }
        };
        if !connected {
            unsafe { CloseHandle(child) };
            bail!("ConnectNamedPipe failed");
        }
        let mut client_pid = 0u32;
        let ok = unsafe { GetNamedPipeClientProcessId(pipe, &mut client_pid) } != 0;
        unsafe { CloseHandle(child) };
        if !ok || client_pid != child_pid {
            bail!("an unexpected process connected to the helper pipe (pid {client_pid})");
        }
        let mut f = pipe_file;
        match exchange(&mut f, &Request::Ping)? {
            Reply::Ok => Ok(f),
            other => Err(anyhow!("unexpected helper reply {other:?}")),
        }
    }

    /// The elevated side: connect, verify the server is our parent, serve requests.
    pub fn helper_main(pipe_name: &str, parent_pid: u32) -> Result<()> {
        if !valid_pipe_name(pipe_name) {
            bail!("invalid pipe name");
        }
        // Exit as soon as the parent goes away.
        let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent_pid) };
        if parent.is_null() {
            bail!("parent process {parent_pid} not found");
        }
        let parent_raw = parent as usize;
        std::thread::spawn(move || {
            unsafe { WaitForSingleObject(parent_raw as HANDLE, INFINITE) };
            tracing::info!("parent gone -- exiting");
            std::process::exit(0);
        });

        let mut pipe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe_name)
            .context("open helper pipe")?;
        let mut server_pid = 0u32;
        let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&pipe);
        if unsafe { GetNamedPipeServerProcessId(handle as HANDLE, &mut server_pid) } == 0
            || server_pid != parent_pid
        {
            bail!("pipe server is not our parent (pid {server_pid})");
        }
        tracing::info!("helper connected to {pipe_name}");
        loop {
            let req: Request = match read_msg(&mut pipe) {
                Ok(r) => r,
                Err(_) => return Ok(()), // parent closed the pipe
            };
            tracing::info!("helper op: {req:?}");
            let reply = super::handle(&req);
            write_msg(&mut pipe, &reply)?;
            if req == Request::Shutdown {
                return Ok(());
            }
        }
    }
}

/// Entry point for `--admin-helper <pipe> <parent-pid>`.
pub fn helper_main(args: &[String]) -> i32 {
    #[cfg(windows)]
    {
        let (Some(pipe), Some(pid)) = (args.get(2), args.get(3).and_then(|p| p.parse().ok()))
        else {
            return 2;
        };
        match broker::helper_main(pipe, pid) {
            Ok(()) => 0,
            Err(e) => {
                tracing::error!("helper: {e:#}");
                3
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::manifest::Manifest;
    use std::path::PathBuf;

    /// A game folder with its exe, staging beside it, and the build's checksums for
    /// `files` (path, content).
    fn repair_rig(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, PathBuf, Manifest) {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(game.join("bin/win10")).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(game.join("bin/win10/echovr.exe"), "MZ").unwrap();
        let mut text = String::from("# Archive: x.zip\n");
        for (path, body) in files {
            let sha = super::super::download::sha256_reader(&mut body.as_bytes()).unwrap();
            text.push_str(&format!("add {path} {sha}\n"));
        }
        let m = Manifest::parse(&text, "https://release.echovr.de/x.manifest").unwrap();
        (dir, game, staging, m)
    }

    fn stage(staging: &Path, path: &str, body: &str) {
        let f = staging.join(path);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, body).unwrap();
    }

    const MANIFEST: &str = "_data/5932408047/rad15/win10/manifests/48037dc70b0ecab2";

    #[test]
    fn the_helper_repairs_only_the_builds_files() {
        let files = [
            (MANIFEST, "stock"),
            ("_local/config.json", "{}"),
            ("bin/win10/a.dll", "MZ a"),
        ];
        let (_dir, game, staging, m) = repair_rig(&files);
        stage(&game, MANIFEST, "repacked");
        stage(&staging, MANIFEST, "stock");
        stage(&staging, "bin/win10/a.dll", "MZ a");
        assert_eq!(apply_staged(&game, &m, &staging).unwrap(), 2);
        assert_eq!(
            std::fs::read_to_string(game.join(MANIFEST)).unwrap(),
            "stock"
        );
        assert_eq!(
            std::fs::read_to_string(game.join("bin/win10/a.dll")).unwrap(),
            "MZ a"
        );
        assert!(!game.join(format!("{MANIFEST}.part")).exists());

        // Any one file refused: nothing is written.
        let refuse = |path: &str, body: &str, why: &str| {
            stage(&game, MANIFEST, "repacked");
            let _ = std::fs::remove_dir_all(&staging);
            stage(&staging, MANIFEST, "stock");
            stage(&staging, path, body);
            let err = apply_staged(&game, &m, &staging).unwrap_err();
            assert_eq!(
                std::fs::read_to_string(game.join(MANIFEST)).unwrap(),
                "repacked",
                "{why}: {err:#}"
            );
        };
        refuse("bin/win10/a.dll", "MZ evil", "another checksum");
        refuse("bin/win10/b.dll", "MZ b", "a file the build doesn't list");
        refuse("_local/config.json", "{}", "the game's settings");
        refuse("bin/win10/a.dll.part", "x", "an unlisted name");

        // A folder without Echo VR's executable.
        std::fs::remove_file(game.join("bin/win10/echovr.exe")).unwrap();
        let _ = std::fs::remove_dir_all(&staging);
        stage(&staging, MANIFEST, "stock");
        assert!(apply_staged(&game, &m, &staging).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_helper_follows_no_link() {
        let (dir, game, staging, m) = repair_rig(&[(MANIFEST, "stock")]);
        // A link in the staging folder.
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("x"), "stock").unwrap();
        stage(&staging, "_data/5932408047/rad15/win10/manifests/.keep", "");
        std::fs::remove_file(staging.join("_data/5932408047/rad15/win10/manifests/.keep")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("x"), staging.join(MANIFEST)).unwrap();
        assert!(apply_staged(&game, &m, &staging).is_err());
        // A link in the game folder leading out of it.
        let _ = std::fs::remove_dir_all(&staging);
        stage(&staging, MANIFEST, "stock");
        std::os::unix::fs::symlink(&elsewhere, game.join("_data")).unwrap();
        assert!(apply_staged(&game, &m, &staging).is_err());
        assert!(!elsewhere.join("5932408047").exists());
    }

    #[test]
    fn a_repair_request_fits_the_pipe() {
        let long = "C:\\".to_string() + &"x".repeat(1000);
        let req = Request::RepairGameFiles {
            game: long.clone(),
            manifest_url: format!("https://release.echovr.de/{}", "y".repeat(1000)),
            staging: long,
        };
        let mut buf = Vec::new();
        write_msg(&mut buf, &req).unwrap();
        assert!(buf.len() < MAX_MESSAGE as usize / 8);
        assert_eq!(read_msg::<Request>(&mut buf.as_slice()).unwrap(), req);
        // Checksums only from the release host.
        for url in [
            "http://release.echovr.de/x.manifest",
            "https://evil.example/x.manifest",
            "x.manifest",
        ] {
            assert!(
                repair_from_staging("C:/game", url, "C:/s").is_err(),
                "{url}"
            );
        }
    }

    #[test]
    fn framing_round_trip() {
        let mut buf = Vec::new();
        let req = Request::RunReviveInstaller {
            path: "C:\\x\\ReviveInstaller.exe".into(),
        };
        write_msg(&mut buf, &req).unwrap();
        let back: Request = read_msg(&mut buf.as_slice()).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn library_entries_only_for_game_executables() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("echovr.exe");
        std::fs::write(&exe, b"").unwrap();
        let exe = exe.to_string_lossy().into_owned();
        assert!(library_entry_for(Some(&exe), "-windowed")
            .unwrap()
            .is_some());
        assert!(library_entry_for(None, "").unwrap().is_none());
        let other = dir.path().join("cmd.exe");
        std::fs::write(&other, b"").unwrap();
        for bad in [
            other.to_string_lossy().into_owned(),
            "echovr.exe".to_string(),
            dir.path()
                .join("missing/echovr.exe")
                .to_string_lossy()
                .into_owned(),
        ] {
            assert!(library_entry_for(Some(&bad), "").is_err(), "{bad}");
        }
        assert!(library_entry_for(Some(&exe), "-a\n-b").is_err());
    }

    #[test]
    fn excludes_only_installed_game_folders() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("Echo/ready-at-dawn-echo-arena/bin/win10");
        let other = dir.path().join("Windows");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let bins = [bin.clone()];
        assert_eq!(
            known_bin(&bin.to_string_lossy(), &bins).unwrap(),
            std::fs::canonicalize(&bin).unwrap()
        );
        // The same folder by another spelling.
        let dotted = dir
            .path()
            .join("Echo/ready-at-dawn-echo-arena/bin/win10/../win10");
        assert!(known_bin(&dotted.to_string_lossy(), &bins).is_ok());
        for bad in [
            other.to_string_lossy().into_owned(),
            dir.path().to_string_lossy().into_owned(),
            dir.path().join("missing").to_string_lossy().into_owned(),
        ] {
            assert!(known_bin(&bad, &bins).is_err(), "{bad}");
        }
        // Nothing in a folder name runs in PowerShell.
        assert_eq!(ps_quote(r"D:\Games\Echo"), r"'D:\Games\Echo'");
        assert_eq!(ps_quote("D:\\it's $(x)"), "'D:\\it''s $(x)'");
    }

    #[test]
    fn rejects_oversized_messages() {
        let mut buf = (MAX_MESSAGE + 1).to_le_bytes().to_vec();
        buf.extend(vec![b' '; 16]);
        assert!(read_msg::<Request>(&mut buf.as_slice()).is_err());
    }

    #[test]
    fn pipe_name_validation() {
        assert!(valid_pipe_name(
            r"\\.\pipe\echovr-launcher-0123456789abcdef0123456789abcdef"
        ));
        assert!(!valid_pipe_name(r"\\.\pipe\other"));
        assert!(!valid_pipe_name(r"\\.\pipe\echovr-launcher-xyz"));
    }

    /// A Meta app folder with Echo VR in its library; returns it and the bin folder.
    fn meta_install(dir: &std::path::Path) -> (String, std::path::PathBuf) {
        let base = dir.join("Oculus");
        let bin = base.join("Software/Software/ready-at-dawn-echo-arena/bin/win10");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("echovr.exe"), b"").unwrap();
        std::fs::write(bin.join("pnsovr.dll"), b"MZ original").unwrap();
        (base.to_string_lossy().into_owned(), bin)
    }

    fn sha(bytes: &[u8]) -> String {
        crate::core::download::sha256_reader(&mut &bytes[..]).unwrap()
    }

    #[test]
    fn helper_patches_only_the_meta_install_with_the_downloaded_dll() {
        let dir = tempfile::tempdir().unwrap();
        let (base, bin) = meta_install(dir.path());
        let dl = dir.path().join("downloads");
        std::fs::create_dir_all(&dl).unwrap();
        let dll = dl.join("pnsovr.dll");
        std::fs::write(&dll, b"MZ patched").unwrap();
        let path = dll.to_string_lossy().into_owned();

        // Changed since the download, not a DLL, another name, a relative path.
        assert!(meta_bin_in(&base)
            .and_then(|b| apply_patch_in(&b, &path, &sha(b"MZ other")))
            .is_err());
        let not_dll = dl.join("x/pnsovr.dll");
        std::fs::create_dir_all(not_dll.parent().unwrap()).unwrap();
        std::fs::write(&not_dll, b"#!/bin/sh").unwrap();
        let not_dll = not_dll.to_string_lossy().into_owned();
        assert!(meta_bin_in(&base)
            .and_then(|b| apply_patch_in(&b, &not_dll, &sha(b"#!/bin/sh")))
            .is_err());
        let other = dl.join("evil.dll");
        std::fs::write(&other, b"MZ patched").unwrap();
        let other = other.to_string_lossy().into_owned();
        assert!(meta_bin_in(&base)
            .and_then(|b| apply_patch_in(&b, &other, &sha(b"MZ patched")))
            .is_err());
        assert!(meta_bin_in(&base)
            .and_then(|b| apply_patch_in(&b, "pnsovr.dll", &sha(b"MZ patched")))
            .is_err());
        assert_eq!(
            std::fs::read(bin.join("pnsovr.dll")).unwrap(),
            b"MZ original"
        );

        meta_bin_in(&base)
            .and_then(|b| apply_patch_in(&b, &path, &sha(b"MZ patched")))
            .unwrap();
        assert_eq!(
            std::fs::read(bin.join("pnsovr.dll")).unwrap(),
            b"MZ patched"
        );
        assert_eq!(
            std::fs::read(bin.join("pnsovr.dll.orig")).unwrap(),
            b"MZ original"
        );

        // No Echo VR in the library: nothing to patch.
        std::fs::remove_file(bin.join("echovr.exe")).unwrap();
        assert!(meta_bin_in(&base)
            .and_then(|b| apply_patch_in(&b, &path, &sha(b"MZ patched")))
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn helper_follows_no_link_out_of_the_meta_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (base, _) = meta_install(&dir.path().join("elsewhere"));
        let outside = std::path::Path::new(&base).join("Software/Software");
        let linked = dir.path().join("Oculus");
        std::fs::create_dir_all(linked.join("Software")).unwrap();
        std::os::unix::fs::symlink(&outside, linked.join("Software/Software")).unwrap();
        assert!(meta_bin_in(&linked.to_string_lossy()).is_err());
        assert!(meta_bin_in(&base).is_ok());
    }

    #[test]
    fn helper_refuses_unpinned_installer() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("ReviveInstaller.exe");
        std::fs::write(&fake, b"not revive").unwrap();
        match handle(&Request::RunReviveInstaller {
            path: fake.to_string_lossy().into(),
        }) {
            Reply::Err(m) => assert!(m.contains("pinned")),
            other => panic!("expected refusal, got {other:?}"),
        }
    }
}
