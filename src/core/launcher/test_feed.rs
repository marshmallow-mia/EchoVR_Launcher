//! Debug builds only: `ECHOVR_TEST_FEED_DIR=<folder>` reads the third-party mods' files from
//! that folder instead of release.echovr.de: `catalogue.json`, `unlisted/<id>.json`,
//! `alerts.json`, `rollback.json`. A file missing there is missing (404). For trying blocks,
//! prereleases and alerts without publishing anything.

/// The test folder's copy of `name` (`Some(None)`: not there), or `None` when there is no
/// test folder (release builds never have one).
pub fn read(name: &str) -> Option<Option<String>> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let dir = std::env::var_os("ECHOVR_TEST_FEED_DIR")?;
    if name.contains("..") {
        return Some(None);
    }
    Some(std::fs::read_to_string(std::path::Path::new(&dir).join(name)).ok())
}
