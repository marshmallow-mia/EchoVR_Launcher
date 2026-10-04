//! The licence patch: a personal `pnsovr.dll` in the game's bin folder, in place of its
//! own. New players need it; owners may use it. The original is kept as
//! `pnsovr.dll.orig` so the patch can be taken off again.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{anyhow, bail, Context, Result};

use super::versions::Step;
use crate::core::{download, oauth, paths};

pub const DLL: &str = "pnsovr.dll";
/// A patch from a link is staged under its own name, so it is never taken for the one
/// Discord built for you.
pub const LINK_DLL: &str = "pnsovr-link.dll";
const ORIG: &str = "pnsovr.dll.orig";
/// The largest patch file taken (the real ones are a few MB).
pub const MAX_SIZE: u64 = 64 << 20;

/// Where the personal patch comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Authorize with Discord; the server builds a patch for this account.
    Discord,
    /// A patch link the user already has.
    Url(String),
    /// A patch already downloaded (while its version was installing).
    Staged(PathBuf),
}

#[derive(Debug)]
pub enum FetchError {
    OAuth(oauth::OAuthError),
    Other(anyhow::Error),
}

/// Whether the bin folder `bin` has a licence patch in (its original kept beside it).
pub fn is_applied(bin: &Path) -> bool {
    bin.join(ORIG).is_file()
}

/// The staged patch from Discord in the download dir (deleted when the app exits).
pub fn staged() -> PathBuf {
    paths::downloads_dir().join(DLL)
}

/// Whether `dll` is a staged patch from a link (deleted once it is in).
pub fn from_link(dll: &Path) -> bool {
    dll.file_name().is_some_and(|n| n == LINK_DLL)
}

/// Gets a personal patch file into the download dir.
pub fn fetch(
    source: &Source,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<PathBuf, FetchError> {
    let (url, filename) = match source {
        Source::Staged(dll) => return Ok(dll.clone()),
        Source::Url(u) => (
            oauth::validate_dll_url(u.trim()).ok_or_else(|| {
                FetchError::Other(anyhow!(
                    "That link is not a licence patch link. Please check it and try again."
                ))
            })?,
            LINK_DLL,
        ),
        Source::Discord => (
            oauth::run(oauth::FileType::Dll, cancel, on).map_err(FetchError::OAuth)?,
            DLL,
        ),
    };
    on(Step::Status("Downloading your patch...".into()));
    let job = download::Job {
        url,
        dir: paths::downloads_dir(),
        filename: filename.into(),
        use_mirror: false,
        fresh: true,
        extract: false,
    };
    download::run(&job, cancel, &mut |p| {
        if let download::Progress::Percent(v) = p {
            on(Step::Percent(v));
        }
    })
    .map_err(FetchError::Other)
}

/// Copies the patch `dll` into the game's bin folder `bin`, backing up the original once.
pub fn apply(bin: &Path, dll: &Path) -> Result<()> {
    let mut src =
        std::fs::File::open(dll).with_context(|| format!("Couldn't open {}", dll.display()))?;
    install(bin, &mut src)
}

/// [`apply`] from an open patch file (the admin helper's, which it holds against writes).
pub fn install(bin: &Path, src: &mut impl Read) -> Result<()> {
    if !bin.is_dir() {
        bail!("Couldn't find the game's folder {}.", bin.display());
    }
    let dst = bin.join(DLL);
    let orig = bin.join(ORIG);
    if dst.is_file() && !orig.exists() {
        std::fs::copy(&dst, &orig).context("Couldn't back up the original pnsovr.dll")?;
    }
    let written = std::fs::File::create(&dst).and_then(|mut out| {
        std::io::copy(src, &mut out)?;
        out.flush()
    });
    written.with_context(|| format!("Couldn't write {}", dst.display()))?;
    tracing::info!("licence patch applied in {}", bin.display());
    Ok(())
}

/// Puts the original `pnsovr.dll` back into the bin folder `bin`.
pub fn remove(bin: &Path) -> Result<()> {
    let orig = bin.join(ORIG);
    if !orig.is_file() {
        bail!(
            "There is no original pnsovr.dll to restore. Verify or reinstall the version instead."
        );
    }
    std::fs::rename(&orig, bin.join(DLL)).context("Couldn't restore the original pnsovr.dll")?;
    tracing::info!("licence patch removed in {}", bin.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_and_remove_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let patch = dir.path().join("patch.dll");
        std::fs::write(&patch, b"patched").unwrap();
        assert!(apply(&bin, &patch).is_err(), "no bin dir");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(DLL), b"original").unwrap();

        apply(&bin, &patch).unwrap();
        assert_eq!(std::fs::read(bin.join(DLL)).unwrap(), b"patched");
        assert_eq!(std::fs::read(bin.join(ORIG)).unwrap(), b"original");
        // A second patch keeps the first backup.
        apply(&bin, &patch).unwrap();
        assert_eq!(std::fs::read(bin.join(ORIG)).unwrap(), b"original");

        remove(&bin).unwrap();
        assert_eq!(std::fs::read(bin.join(DLL)).unwrap(), b"original");
        assert!(!bin.join(ORIG).exists());
        assert!(remove(&bin).is_err(), "nothing to restore");
    }

    #[test]
    fn staged_patches_are_kept_apart() {
        let dll = PathBuf::from("/tmp/x/pnsovr.dll");
        let cancel = AtomicBool::new(false);
        let got = fetch(&Source::Staged(dll.clone()), &cancel, &mut |_| {});
        assert_eq!(got.ok(), Some(dll));
        assert!(from_link(&paths::downloads_dir().join(LINK_DLL)));
        assert!(!from_link(&staged()));
    }

    #[test]
    fn updates_skip_the_patch() {
        use crate::core::pc_update::skipped;
        assert!(skipped("pnsovr.dll", &[DLL]));
        assert!(skipped("PNSOVR.DLL", &[DLL]));
        assert!(!skipped("echovr.exe", &[DLL]));
        assert!(!skipped("pnsovr.dll", &[]));
    }
}
