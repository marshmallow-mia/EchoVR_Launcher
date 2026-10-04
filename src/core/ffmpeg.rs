//! ffmpeg, which converts a video into the background's format (see
//! `launcher::background`). One the user has installed (on the PATH) when there is one;
//! on 64-bit Windows otherwise a pinned build, downloaded on first use into the cache and
//! checked against its SHA-256. Only its `ffmpeg.exe` (and licence) is kept.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};

use super::{download, paths};

const VERSION: &str = "9.0.2";
const URL: &str =
    "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip";
const SHA256: &str = "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba";
const IN_ZIP: [(&str, &str); 2] = [
    ("ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe", "ffmpeg.exe"),
    ("ffmpeg-9.0.2-essentials_build/LICENSE", "LICENSE"),
];
/// The download's size, for asking first.
pub const DOWNLOAD_MB: u64 = 115;

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

/// Where the downloaded build goes ("Delete cache" removes it; it comes back when needed).
fn ours() -> PathBuf {
    paths::downloads_dir()
        .join(format!("ffmpeg-{VERSION}"))
        .join(exe_name())
}

/// An ffmpeg that is here already: the downloaded one, one on the PATH, or Homebrew's
/// (an app started from the Finder doesn't get Homebrew's PATH).
pub fn find() -> Option<PathBuf> {
    let ours = ours();
    if ours.is_file() {
        return Some(ours);
    }
    let runs = |p: &Path| {
        super::process::command(p)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let installed = [
        exe_name(),
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
    ];
    installed
        .iter()
        .map(PathBuf::from)
        .filter(|p| cfg!(not(windows)) || p.components().count() == 1)
        .find(|p| runs(p))
}

/// Whether the pinned build fits this system (it is for 64-bit Windows).
pub fn can_download() -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
}

/// Downloads the pinned build, checks it, and unpacks `ffmpeg.exe`; returns its path.
pub fn download(cancel: &AtomicBool, on: &mut dyn FnMut(download::Progress)) -> Result<PathBuf> {
    let job = download::Job {
        url: URL.into(),
        dir: paths::downloads_dir(),
        filename: format!("ffmpeg-{VERSION}.zip"),
        use_mirror: false,
        fresh: false,
        extract: false,
    };
    let zip_path = download::run(&job, cancel, on)?;
    if !download::sha256_matches(&zip_path, SHA256) {
        let _ = std::fs::remove_file(&zip_path);
        bail!("The downloaded converter (ffmpeg) failed its integrity check. Please try again.");
    }
    on(download::Progress::Extracting);
    let exe = ours();
    let dir = exe.parent().context("ffmpeg folder")?;
    unpack(&zip_path, dir)?;
    let _ = std::fs::remove_file(&zip_path);
    tracing::info!("ffmpeg {VERSION} ready at {}", exe.display());
    Ok(exe)
}

/// Writes the files in `IN_ZIP` from the archive into `dir` (nothing else from it).
fn unpack(zip_path: &Path, dir: &Path) -> Result<()> {
    let file = File::open(zip_path).with_context(|| format!("open {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("the ffmpeg download is not a zip")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    for (inside, name) in IN_ZIP {
        let mut entry = archive
            .by_name(inside)
            .with_context(|| format!("{inside} is missing from the ffmpeg download"))?;
        let tmp = dir.join(format!("{name}.part"));
        {
            let mut out = BufWriter::new(
                File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?,
            );
            std::io::copy(&mut entry, &mut out)?;
            out.flush()?;
        }
        std::fs::rename(&tmp, dir.join(name))?;
    }
    Ok(())
}
