//! Installing, updating, verifying and removing one PC version in the library.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};

use super::catalog::{Platform, VersionEntry};
use super::store::InstalledVersion;
use crate::core::download::{self, Progress};
use crate::core::manifest::Manifest;
use crate::core::{paths, pc_update, remote_zip};

const ZIP_NAME: &str = "ready-at-dawn-echo-arena.zip";

/// Progress of a long-running version job, for the UI row.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Status(String),
    Percent(f64),
    /// Checking the game files against their checksums, this far (0 to 100).
    Checking(f64),
    /// A page just opened in the browser (Discord's authorization) and the job waits for
    /// what the player does there; the UI says so and offers to open it again.
    Browser(String),
}

pub fn root_for(library: &str, id: &str) -> String {
    paths::normalize(&format!("{}/{id}", paths::normalize(library)))
}

/// A finished install: the version, and why its update failed if it did. The game is in
/// place then, just not up to date; Update in MANAGE tries again.
pub struct Installed {
    pub version: InstalledVersion,
    pub update_failed: Option<String>,
}

/// A finished reinstall: the version, the game files that were broken or missing and
/// were fetched again, and why its update failed if it did.
pub struct Reinstalled {
    pub version: InstalledVersion,
    pub repaired: Vec<String>,
    pub update_failed: Option<String>,
}

/// Downloads, verifies and extracts `entry` into `<library>/<id>`, then applies its update
/// manifest. The multi-GB zip is deleted afterwards (the launcher keeps no cache copy).
/// Game files left there by an install that stopped after extracting are taken over
/// instead of downloaded again.
pub fn install(
    entry: &VersionEntry,
    library: &str,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<Installed> {
    if entry.platform != Platform::Pc {
        bail!("Quest versions are installed from the Quest side of the Play page.");
    }
    if !entry.downloadable() {
        bail!(
            "Couldn't download {}: this build isn't on the download servers yet.",
            entry.name
        );
    }
    let root = root_for(library, &entry.id);
    let version = from_entry(entry, &entry.id, &root);
    if version.present() {
        on(Step::Status(
            "Found the game files of an earlier install".into(),
        ));
    } else {
        download_and_extract(entry, &root, cancel, on)?;
        if !version.present() {
            bail!(
                "The download did not contain Echo VR ({} is missing).",
                version.exe_name()
            );
        }
    }
    finish(entry, version, cancel, on)
}

/// Takes the copy of `entry` at `root` (installed some other way: the Meta app, an older
/// installer) into the library as version `id`, instead of downloading it: its files are
/// checked against the build's checksums (a few broken ones come again), then it gets its
/// update. A folder holding another build is refused, never overwritten. A licence patch
/// in it stays.
pub fn adopt(
    entry: &VersionEntry,
    root: &str,
    id: &str,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<Installed> {
    if entry.platform != Platform::Pc {
        bail!("Quest versions are installed from the Quest side of the Play page.");
    }
    let root = paths::normalize(root);
    let mut version = from_entry(entry, id, &root);
    version.external = true;
    // Its own executable, whatever the catalogue says: the checksums tell the build.
    version.exe = paths::find_exe(&root)
        .filter(|e| *e != paths::DEFAULT_EXE)
        .map(str::to_string);
    if !version.present() {
        bail!("There is no Echo VR in {root}.");
    }
    version.patched = super::patch::is_applied(&version.bin_dir());
    finish(entry, version, cancel, on)
}

/// Version `id` of `entry` at `root`, as installed now.
fn from_entry(entry: &VersionEntry, id: &str, root: &str) -> InstalledVersion {
    InstalledVersion {
        id: id.to_string(),
        name: entry.name.clone(),
        root: root.to_string(),
        external: false,
        catalog_id: Some(entry.id.clone()),
        update_manifest: entry.update_manifest.clone(),
        installed_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok(),
        patched: false,
        exe: entry.exe.clone(),
        publisher_lock: entry.publisher_lock.clone(),
    }
}

/// The rest of an install once the game files are in place: every file against the
/// build's checksums, whichever mirror the zip came from (their archives differ, their
/// files don't), the broken ones again from the archive; then its patch and update. A
/// copy installed elsewhere (`external`) that is mostly another build is refused.
fn finish(
    entry: &VersionEntry,
    version: InstalledVersion,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<Installed> {
    if let Some(url) = &entry.files_manifest {
        match Checksums::fetch(url, &version, version.patched) {
            Ok(sums) => {
                let bad = sums.check(cancel, on)?;
                if version.external && mostly_bad(bad.len(), sums.files().count()) {
                    bail!(
                        "{} holds another build of Echo VR than {}, so it wasn't added as that. Add it with \"Add existing folder\" in your library instead, or download {} here.",
                        version.root,
                        entry.name,
                        entry.name
                    );
                }
                sums.repair(&bad, cancel, on)?;
            }
            Err(e) if version.external => return Err(e),
            Err(e) => tracing::warn!("{} installed unchecked: {e:#}", entry.id),
        }
    }
    relay_patch(&version, cancel, on)?;
    let update_failed = update_after(entry, &version, cancel, on)?;
    Ok(Installed {
        version,
        update_failed,
    })
}

/// Pure: most of a build's `total` files are broken or missing (`bad`): another build,
/// or most of it gone.
fn mostly_bad(bad: usize, total: usize) -> bool {
    bad * 2 > total
}

/// Reinstalls `v` (installed from `entry`) without downloading it all again: every game
/// file is checked against the build's checksums on the server, and only the broken or
/// missing ones are fetched again, out of the build's archive. Then its update.
/// `keep_patch` leaves a licence-patched `pnsovr.dll` alone; otherwise the original
/// comes back. When most of the game is gone, the whole archive is downloaded instead.
pub fn reinstall(
    entry: &VersionEntry,
    v: &InstalledVersion,
    keep_patch: bool,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<Reinstalled> {
    let Some(url) = &entry.files_manifest else {
        bail!(
            "The server has no checksums for {}, so it can't be checked. Remove it and install it again instead.",
            entry.name
        );
    };
    on(Step::Status("Reading the build's checksums...".into()));
    let sums = Checksums::fetch(url, v, keep_patch)?;
    let mut bad = sums.check(cancel, on)?;
    if mostly_bad(bad.len(), sums.files().count()) {
        on(Step::Status(
            "Most game files are missing: downloading all of them".into(),
        ));
        download_and_extract(entry, &v.root, cancel, on)?;
        bad = sums.check(cancel, on)?;
    }
    sums.repair(&bad, cancel, on)?;
    let version = InstalledVersion {
        patched: keep_patch && v.patched,
        publisher_lock: entry.publisher_lock.clone(),
        ..v.clone()
    };
    relay_patch(&version, cancel, on)?;
    let update_failed = update_after(entry, &version, cancel, on)?;
    Ok(Reinstalled {
        version,
        repaired: bad.into_iter().map(|(path, _)| path).collect(),
        update_failed,
    })
}

/// An event build gets EchoRelay's patch, so it can log in on the classic lobbies server.
fn relay_patch(v: &InstalledVersion, cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<()> {
    if v.publisher_lock.is_none() {
        return Ok(());
    }
    on(Step::Status("Adding EchoRelay's patch...".into()));
    super::relay::apply_patch(v, cancel)
}

/// Applies `entry`'s update to `v` after an install; why it failed, if it did (the game
/// is in place then, just not up to date).
fn update_after(
    entry: &VersionEntry,
    v: &InstalledVersion,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<Option<String>> {
    let Some(m) = &entry.update_manifest else {
        return Ok(None);
    };
    let m = pc_update::manifest_for(m);
    on(Step::Status("Applying update...".into()));
    match pc_update::apply_skipping(&m, &v.bin_dir(), keep(v), cancel, &mut |s| {
        on(Step::Status(s))
    }) {
        Ok(()) => Ok(None),
        Err(e) if crate::core::http::is_cancelled(&e) => Err(e),
        Err(e) => {
            tracing::warn!("{} installed, but its update failed: {e:#}", entry.id);
            Ok(Some(format!("{e:#}")))
        }
    }
}

/// Pure: whether `path` (in the build's terms, lowercase) is the game's own data: its
/// settings (`_local/`) and crash dumps (`_temp/`). The live build's archive happens to
/// carry some from whoever packed it; a reinstall must never put those back over yours.
fn game_data(path: &str) -> bool {
    path.starts_with("_local/") || path.starts_with("_temp/")
}

/// A build's checksums for one install: its file manifest, where its files are on disk,
/// and the files that aren't the build's to check (the update replaces them; a kept
/// licence patch).
struct Checksums {
    manifest: Manifest,
    /// The archive's folder in the install (`<root>/ready-at-dawn-echo-arena`).
    game: PathBuf,
    /// Paths as the manifest has them, lowercase.
    skip: HashSet<String>,
}

impl Checksums {
    fn fetch(url: &str, v: &InstalledVersion, keep_patch: bool) -> Result<Checksums> {
        let manifest = Manifest::fetch(url)
            .with_context(|| "Couldn't read the build's checksums from the server")?;
        if manifest.archive_root.is_none() {
            bail!("The build's checksums on the server don't name its folder ({url})");
        }
        // Whatever the archive's folder is called, the game is unpacked into this one.
        let game = Path::new(&v.root).join(paths::ARENA_DIR);
        // The update's files, in the build's terms: `bin/win10/<path>`.
        let bin = v.bin_dir();
        let bin = bin
            .strip_prefix(&game)
            .map(|b| b.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| "bin/win10".into());
        let mut skip = HashSet::new();
        if let Some(update) = manifest_url(v) {
            let update = Manifest::fetch(&update)
                .with_context(|| "Couldn't read the update's checksums from the server")?;
            // What the update replaces, and what it takes out (the build's own
            // dbgcore.dll, which nEVR won't start beside): neither is the build's file.
            skip.extend(
                update
                    .adds()
                    .chain(update.dels())
                    .map(|e| format!("{bin}/{}", e.path).to_ascii_lowercase()),
            );
        }
        if keep_patch {
            skip.insert(format!("{bin}/{}", super::patch::DLL).to_ascii_lowercase());
        }
        // An event build's EchoRelay patch replaces one of its files.
        if v.publisher_lock.is_some() {
            skip.insert(format!("{bin}/{}", super::relay::patch_file(v)).to_ascii_lowercase());
        }
        Ok(Checksums {
            manifest,
            game,
            skip,
        })
    }

    /// The build's files to check: not the game's own (`game_data`), nor the skipped.
    fn files(&self) -> impl Iterator<Item = &crate::core::manifest::Entry> {
        self.manifest.adds().filter(|e| {
            let path = e.path.to_ascii_lowercase();
            !self.skip.contains(&path) && !game_data(&path)
        })
    }

    /// The files that are missing or don't match, with their checksums.
    fn check(
        &self,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Step),
    ) -> Result<Vec<(String, String)>> {
        let files: Vec<_> = self.files().collect();
        let mut bad = Vec::new();
        for (i, e) in files.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::core::http::Cancelled.into());
            }
            on(Step::Checking(100.0 * i as f64 / files.len().max(1) as f64));
            let sha = e.sha256.clone().unwrap_or_default();
            let f = self.game.join(&e.path);
            if !f.is_file() || !download::sha256_matches(&f, &sha) {
                bad.push((e.path.clone(), sha));
            }
        }
        if !bad.is_empty() {
            tracing::info!(
                "{} game file(s) to repair: {:?}",
                bad.len(),
                bad.iter().map(|b| &b.0).collect::<Vec<_>>()
            );
        }
        Ok(bad)
    }

    /// Fetches `bad` again, out of the build's archive on the server.
    fn repair(
        &self,
        bad: &[(String, String)],
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Step),
    ) -> Result<()> {
        if bad.is_empty() {
            return Ok(());
        }
        let (Some(url), Some(root)) = (self.manifest.archive_url(), &self.manifest.archive_root)
        else {
            bail!("The build's checksums on the server don't name its archive");
        };
        let n = bad.len();
        remote_zip::extract_members(&url, root, bad, &self.game, cancel, &mut |i, path| {
            on(Step::Status(format!("Repairing {}/{n}: {path}", i + 1)))
        })
    }
}

/// Downloads `entry`'s zip into `root`, checks it and extracts it there.
fn download_and_extract(
    entry: &VersionEntry,
    root: &str,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<()> {
    let job = download::Job {
        url: entry.url.clone(),
        dir: PathBuf::from(root),
        filename: ZIP_NAME.into(),
        use_mirror: entry.uses_mirror(),
        fresh: false,
        extract: false,
    };
    let zip = download::run(&job, cancel, &mut |p| {
        on(match p {
            Progress::Status(s) => Step::Status(s),
            Progress::Percent(v) => Step::Percent(v),
            Progress::Extracting => Step::Status("Extracting...".into()),
            Progress::Extracted => Step::Status("Extraction complete".into()),
        })
    })?;
    if let Some(sha) = &entry.sha256 {
        on(Step::Status("Verifying download...".into()));
        if !download::sha256_matches(&zip, sha) {
            let _ = std::fs::remove_file(&zip);
            bail!("The downloaded game files are corrupt (checksum mismatch). Please try again.");
        }
    }
    on(Step::Status("Extracting...".into()));
    // Builds are packed under one folder of their own name ("echo-vr-6/", "Echo VR
    // Halloween 2017/"); every install keeps its game in the same one.
    let game = Path::new(root).join(paths::ARENA_DIR);
    match crate::core::zip::top_folder(&zip)? {
        Some(top) => crate::core::zip::extract_root(&zip, Some(&top), &game, cancel)?,
        None => crate::core::zip::extract(&zip, Path::new(root), cancel)?,
    };
    let _ = std::fs::remove_file(&zip);
    Ok(())
}

/// `v`'s update, if it gets one: its own, or the live build's for a live install that
/// doesn't name one (an added folder). Event builds and old folders get none.
fn manifest_url(v: &InstalledVersion) -> Option<String> {
    let live = v.publisher_lock.is_none() && v.bin_dir().ends_with("win10");
    v.update_manifest
        .as_deref()
        .or(live.then_some(pc_update::PC_MANIFEST_URL))
        .map(pc_update::manifest_for)
}

/// Whether `v` gets updates (event builds don't).
pub fn has_updates(v: &InstalledVersion) -> bool {
    manifest_url(v).is_some()
}

fn no_updates(v: &InstalledVersion) -> anyhow::Error {
    anyhow::anyhow!("{} doesn't get updates: its build is final.", v.name)
}

/// Files an update must leave alone: the licence patch of a patched version.
fn keep(v: &InstalledVersion) -> &'static [&'static str] {
    if v.patched {
        &[super::patch::DLL]
    } else {
        &[]
    }
}

pub fn update(v: &InstalledVersion, cancel: &AtomicBool, on: &mut dyn FnMut(Step)) -> Result<()> {
    ensure_present(v)?;
    let url = manifest_url(v).ok_or_else(|| no_updates(v))?;
    pc_update::apply_skipping(&url, &v.bin_dir(), keep(v), cancel, &mut |s| {
        on(Step::Status(s))
    })
}

/// Files of the update manifest that are missing or differ on disk.
pub fn verify(
    v: &InstalledVersion,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<Vec<String>> {
    ensure_present(v)?;
    let m = Manifest::fetch(&manifest_url(v).ok_or_else(|| no_updates(v))?)?;
    let bin = v.bin_dir();
    let adds: Vec<_> = m
        .adds()
        .filter(|e| !pc_update::skipped(&e.path, keep(v)))
        .collect();
    let mut bad = Vec::new();
    for (i, e) in adds.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::core::http::Cancelled.into());
        }
        on(Step::Percent(100.0 * i as f64 / adds.len().max(1) as f64));
        let f = bin.join(&e.path);
        if !f.is_file() || !download::sha256_matches(&f, e.sha256.as_deref().unwrap_or_default()) {
            bad.push(e.path.clone());
        }
    }
    Ok(bad)
}

/// Deletes a managed version. External installs are never deleted.
pub fn remove(v: &InstalledVersion, library: &str) -> Result<()> {
    if v.external {
        bail!(
            "{} was added from an existing folder; the launcher only forgets it.",
            v.name
        );
    }
    let root = Path::new(&v.root);
    let lib = Path::new(library);
    // Guard against a corrupted state file pointing somewhere unexpected: only a folder in
    // the library, or one named after the version (installed before the library moved).
    let inside = root.parent().is_some_and(|p| {
        paths::normalize(&p.to_string_lossy()) == paths::normalize(&lib.to_string_lossy())
    });
    let named = root.file_name().is_some_and(|n| n == v.id.as_str());
    if !(inside || named) || !root.join(paths::ARENA_DIR).is_dir() {
        bail!(
            "Refusing to delete {}: it is not a version folder inside the library.",
            v.root
        );
    }
    std::fs::remove_dir_all(root).with_context(|| format!("delete {}", v.root))
}

fn ensure_present(v: &InstalledVersion) -> Result<()> {
    if !v.present() {
        bail!("Echo VR was not found at {}.", v.root);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_build_is_mostly_bad() {
        assert!(!mostly_bad(0, 100));
        assert!(!mostly_bad(50, 100));
        assert!(mostly_bad(51, 100));
        assert!(!mostly_bad(0, 0));
    }

    #[test]
    fn adopts_only_a_folder_with_the_game() {
        let dir = tempfile::tempdir().unwrap();
        let root = paths::normalize(&dir.path().to_string_lossy());
        let entry = VersionEntry {
            id: "pc-latest".into(),
            name: "Echo VR".into(),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let err = adopt(&entry, &root, "pc-latest", &cancel, &mut |_| {}).err();
        assert!(err.is_some_and(|e| e.to_string().contains("no Echo VR")));

        // No checksums to go by: taken as it is, as the launcher's version of the entry.
        let bin = dir.path().join(paths::ARENA_DIR).join("bin/win10");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(paths::DEFAULT_EXE), b"MZ").unwrap();
        std::fs::write(bin.join("pnsovr.dll.orig"), b"original").unwrap();
        let i = adopt(&entry, &root, "pc-latest-2", &cancel, &mut |_| {}).unwrap();
        assert_eq!(i.version.id, "pc-latest-2");
        assert_eq!(i.version.catalog_id.as_deref(), Some("pc-latest"));
        assert!(i.version.external && i.version.patched);
        assert_eq!(i.version.exe, None);
    }

    /// Every event build on the server: its checksums read, they name the archive the
    /// catalogue downloads, and the archive opens over ranges with the build's executable
    /// in it. One small file comes out of Halloween 2017's, checked.
    #[test]
    #[ignore = "network"]
    fn the_event_builds_are_on_the_server() {
        let cancel = AtomicBool::new(false);
        let builtin = super::super::catalog::Catalog::builtin();
        for e in builtin
            .versions
            .iter()
            .filter(|e| e.publisher_lock.is_some())
        {
            let m = Manifest::fetch(e.files_manifest.as_ref().unwrap()).unwrap();
            let (url, root) = (m.archive_url().unwrap(), m.archive_root.clone().unwrap());
            assert_eq!(url, e.url, "{}", e.id);
            assert_eq!(m.archive_size, e.size, "{}", e.id);
            let exe = format!(
                "bin/win7/{}",
                e.exe.as_deref().unwrap_or(paths::DEFAULT_EXE)
            );
            assert!(m.adds().any(|a| a.path == exe), "{exe} in {}", e.id);
            let reader = remote_zip::RangeReader::open(&url, &cancel).unwrap();
            let mut zip = zip::ZipArchive::new(reader).unwrap();
            assert!(zip.by_name(&format!("{root}{exe}")).is_ok(), "{}", e.id);
            if e.id == "pc-halloween-2017" {
                let play = m.adds().find(|a| a.path == "Play.bat").unwrap();
                let members = vec![(play.path.clone(), play.sha256.clone().unwrap())];
                let dir = tempfile::tempdir().unwrap();
                remote_zip::extract_members(
                    &url,
                    &root,
                    &members,
                    dir.path(),
                    &cancel,
                    &mut |_, _| {},
                )
                .unwrap();
                assert!(dir.path().join("Play.bat").is_file());
            }
        }
    }

    /// Installs Halloween 2017 (1.3 GB) into the library in `ECHOVR_TEST_LIBRARY`, as the
    /// launcher does, and checks what an event build gets: the usual layout, EchoRelay's
    /// patch, its config, and a reinstall that finds nothing to fetch.
    #[test]
    #[ignore = "network, 1.3 GB; needs ECHOVR_TEST_LIBRARY"]
    fn installs_an_event_build() {
        let library = std::env::var("ECHOVR_TEST_LIBRARY").unwrap();
        let entry = super::super::catalog::Catalog::builtin()
            .versions
            .into_iter()
            .find(|e| e.id == "pc-halloween-2017")
            .unwrap();
        let cancel = AtomicBool::new(false);
        let mut last = String::new();
        let done = install(&entry, &library, &cancel, &mut |s| {
            if let Step::Status(s) = s {
                if s != last {
                    eprintln!("{s}");
                    last = s;
                }
            }
        })
        .unwrap();
        let v = done.version;
        assert!(done.update_failed.is_none());
        assert_eq!(v.publisher_lock.as_deref(), Some("release4_5"));
        let bin = v.bin_dir();
        assert!(
            bin.ends_with("ready-at-dawn-echo-arena/bin/win7"),
            "{}",
            bin.display()
        );
        assert!(v.present());
        assert!(download::sha256_matches(
            &bin.join("dbghelp.dll"),
            super::super::relay::PATCH_SHA256
        ));
        assert!(bin.join("dbghelp_orig.dll").is_file());
        let account = super::super::store::RelayAccount {
            name: "Tester".into(),
            password: "pw".into(),
        };
        super::super::relay::write_config(&v, super::super::relay::DEFAULT_SERVER, &account)
            .unwrap();
        let config = Path::new(&v.root).join("ready-at-dawn-echo-arena/_local/config.json");
        let c: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(c["publisher_lock"], "release4_5");
        assert!(config.with_file_name("config.json.orig").is_file());
        let again = reinstall(&entry, &v, false, &cancel, &mut |_| {}).unwrap();
        assert!(again.repaired.is_empty(), "{:?}", again.repaired);
    }

    #[test]
    fn the_games_own_data_is_never_checked() {
        assert!(game_data("_local/config.json"));
        assert!(game_data("_temp/crashes/x.dmp"));
        assert!(!game_data("bin/win10/echovr.exe"));
        assert!(!game_data("_data/5932408047/rad15/win10/manifests/x"));
    }

    /// Reinstalls the install whose root (holding `ready-at-dawn-echo-arena`) is in
    /// `ECHOVR_TEST_INSTALL`, against the live build: break a file there first to see it
    /// fetched again. It changes that folder: point it at a copy.
    #[test]
    #[ignore = "network; needs ECHOVR_TEST_INSTALL"]
    fn reinstalls_a_copy_against_the_live_build() {
        let root = std::env::var("ECHOVR_TEST_INSTALL").unwrap();
        let entry = super::super::catalog::Catalog::builtin().versions[0].clone();
        let v = InstalledVersion {
            id: entry.id.clone(),
            name: entry.name.clone(),
            root,
            catalog_id: Some(entry.id.clone()),
            update_manifest: entry.update_manifest.clone(),
            ..Default::default()
        };
        let mut steps = Vec::new();
        let r = reinstall(&entry, &v, false, &AtomicBool::new(false), &mut |s| {
            if let Step::Status(s) = s {
                steps.push(s)
            }
        })
        .unwrap();
        eprintln!("repaired: {:?}\nsteps: {steps:?}", r.repaired);
        assert!(r.update_failed.is_none());
        // Everything is right now: a second pass fetches nothing.
        let again = reinstall(&entry, &v, false, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(again.repaired.is_empty());
    }

    fn fake_version(lib: &Path, id: &str) -> InstalledVersion {
        let root = lib.join(id);
        let bin = root.join("ready-at-dawn-echo-arena/bin/win10");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("echovr.exe"), b"").unwrap();
        InstalledVersion {
            id: id.into(),
            root: paths::normalize(&root.to_string_lossy()),
            ..Default::default()
        }
    }

    #[test]
    fn remove_only_managed_version_folders() {
        let dir = tempfile::tempdir().unwrap();
        let lib = paths::normalize(&dir.path().join("lib").to_string_lossy());
        let v = fake_version(Path::new(&lib), "a");
        let mut ext = v.clone();
        ext.external = true;
        assert!(remove(&ext, &lib).is_err());
        // A folder outside the library that isn't named after the version.
        let mut elsewhere = fake_version(dir.path(), "outside");
        elsewhere.id = "pc-latest".into();
        assert!(remove(&elsewhere, &lib).is_err());
        assert!(Path::new(&elsewhere.root).exists());
        remove(&v, &lib).unwrap();
        assert!(!Path::new(&v.root).exists());
        // Installed into an earlier library: still its own folder.
        let old = fake_version(&dir.path().join("old-lib"), "pc-1");
        remove(&old, &lib).unwrap();
        assert!(!Path::new(&old.root).exists());
    }

    #[test]
    fn roots() {
        assert_eq!(
            root_for("C:\\EchoVR\\versions\\", "pc-1"),
            "C:/EchoVR/versions/pc-1"
        );
    }
}
