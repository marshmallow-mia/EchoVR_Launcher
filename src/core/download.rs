//! Resumable downloads with progress, cancellation, mirror selection and optional
//! extraction -- the port of the Java `Downloader`.
//!
//! Fixes over the Java version:
//! * failure is reported as failure (Java ran `onComplete` even after an error, so the
//!   wizard went on to "Installation complete!"), and a failed size probe no longer
//!   silently never calls back;
//! * a resume is only trusted when the server answers `206` with a matching
//!   `Content-Range`; a `200` restarts from zero instead of appending the whole body at
//!   the old offset;
//! * a local file larger than the remote one is discarded rather than accepted as done;
//! * the finished size is verified;
//! * the mirror probe runs both mirrors concurrently and reports "no mirror reachable"
//!   instead of building a `null...` URL;
//! * a file one mirror doesn't have is fetched from the other, and a file no mirror has
//!   says so instead of blaming the connection.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use super::http;

pub const MIRRORS: [&str; 2] = ["https://files.echovr.de/", "https://evr.echo.taxi/"];
const MIRROR_TEST_FILE: &str = "randomDownloadTestFile";

/// Shown for network failures, as in the Java dialogs.
pub const NETWORK_ERROR: &str =
    "Couldn't finish Download. Please check your Ethernet or try again later.";

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Status(String),
    /// 0..=100
    Percent(f64),
    Extracting,
    Extracted,
}

#[derive(Debug, Clone)]
pub struct Job {
    /// Absolute URL, or a path relative to the fastest mirror when `use_mirror` is set.
    pub url: String,
    pub dir: PathBuf,
    pub filename: String,
    pub use_mirror: bool,
    /// Delete an existing file first instead of resuming (personalized patch files).
    pub fresh: bool,
    /// Extract the zip into `dir` afterwards.
    pub extract: bool,
}

impl Job {
    pub fn target(&self) -> PathBuf {
        self.dir.join(&self.filename)
    }
}

/// A server answered that it doesn't have the file (404 or 410).
#[derive(Debug)]
pub struct NotOnServer(pub u16);

impl std::fmt::Display for NotOnServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "This file isn't on the download servers (they answered {}).",
            self.0
        )
    }
}

impl std::error::Error for NotOnServer {}

/// The mirrors that serve the test file, fastest first.
pub fn ranked_mirrors() -> Result<Vec<&'static str>> {
    http::block_on(async {
        let probes = MIRRORS.iter().map(|m| {
            let url = format!("{m}{MIRROR_TEST_FILE}");
            tokio::spawn(async move {
                let start = Instant::now();
                // The test file is 30 MB; the first MiB is plenty to rank two mirrors.
                let fut = async {
                    let resp = http::client()
                        .get(&url)
                        .header(reqwest::header::RANGE, "bytes=0-1048575")
                        .send()
                        .await?
                        .error_for_status()?;
                    resp.bytes().await?;
                    Ok::<_, reqwest::Error>(())
                };
                match tokio::time::timeout(Duration::from_secs(30), fut).await {
                    Ok(Ok(())) => Some(start.elapsed()),
                    Ok(Err(e)) => {
                        tracing::warn!("mirror probe {url} failed: {e}");
                        None
                    }
                    Err(_) => {
                        tracing::warn!("mirror probe {url} timed out");
                        None
                    }
                }
            })
        });
        let mut timed = Vec::new();
        for (mirror, handle) in MIRRORS.iter().zip(probes.collect::<Vec<_>>()) {
            let t = handle.await.ok().flatten();
            if let Some(t) = t {
                tracing::info!("mirror {mirror}: {t:?}");
            }
            timed.push((*mirror, t));
        }
        let ranked = rank(timed);
        if ranked.is_empty() {
            bail!("none of the download servers could be reached");
        }
        Ok(ranked)
    })
}

/// Pure: the mirrors that answered, fastest first.
fn rank(timed: Vec<(&'static str, Option<Duration>)>) -> Vec<&'static str> {
    let mut up: Vec<_> = timed
        .into_iter()
        .filter_map(|(m, t)| t.map(|t| (m, t)))
        .collect();
    up.sort_by_key(|(_, t)| *t);
    up.into_iter().map(|(m, _)| m).collect()
}

/// Tries `urls` in order, moving on to the next when a server doesn't have the file, or
/// can't resume a partial one while another might (`fetch` gets whether it is the last
/// try, where a restart from zero is allowed); any other failure ends it.
fn from_first_that_has(
    urls: &[String],
    mut fetch: impl FnMut(&str, bool) -> Result<()>,
) -> Result<()> {
    let mut missing = None;
    for (i, url) in urls.iter().enumerate() {
        match fetch(url, i + 1 == urls.len()) {
            Ok(()) => return Ok(()),
            Err(e) if e.downcast_ref::<NotOnServer>().is_some() => {
                tracing::info!("{} doesn't have this file", redact(url));
                missing = Some(e);
            }
            Err(e) if e.downcast_ref::<CantResume>().is_some() => {
                tracing::info!(
                    "{} can't resume the partial file; trying the next mirror",
                    redact(url)
                );
            }
            Err(e) => {
                if !http::is_cancelled(&e) {
                    tracing::warn!("download from {} failed: {e:#}", redact(url));
                }
                return Err(e);
            }
        }
    }
    Err(missing.unwrap_or_else(|| anyhow!("nothing to download")))
}

/// A mirror answered a resume request with the whole file while another mirror is left
/// to try (the partial file is kept).
#[derive(Debug)]
struct CantResume;

impl std::fmt::Display for CantResume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the server can't resume this download")
    }
}

impl std::error::Error for CantResume {}

/// How to continue from a partial local file, decided from the size probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Already complete.
    Done,
    /// Start (or restart) at zero.
    FromStart,
    /// Continue at this offset.
    From(u64),
}

/// Pure: what to do with `local` bytes on disk when the remote file has `remote` bytes.
pub fn plan_resume(local: u64, remote: u64) -> Resume {
    if local == 0 || local > remote {
        Resume::FromStart
    } else if local == remote {
        Resume::Done
    } else {
        Resume::From(local)
    }
}

/// Pure: whether a `206` answer really continues at `offset`.
pub fn content_range_matches(content_range: Option<&str>, offset: u64) -> bool {
    content_range
        .and_then(|v| v.trim().strip_prefix("bytes "))
        .and_then(|v| v.split('-').next())
        .and_then(|start| start.trim().parse::<u64>().ok())
        == Some(offset)
}

/// Runs the job on the calling (worker) thread.
pub fn run(job: &Job, cancel: &AtomicBool, on: &mut dyn FnMut(Progress)) -> Result<PathBuf> {
    let urls: Vec<String> = if job.use_mirror {
        on(Progress::Status("Preparing Download...".into()));
        ranked_mirrors()
            .context(NETWORK_ERROR)?
            .into_iter()
            .map(|m| format!("{m}{}", job.url.trim_start_matches('/')))
            .collect()
    } else {
        vec![job.url.clone()]
    };
    tracing::info!(
        "download {} -> {}",
        redact(&urls[0]),
        job.target().display()
    );

    std::fs::create_dir_all(&job.dir).with_context(|| {
        format!(
            "Couldn't create the folder {}.\nIt may need administrator rights -- pick a different folder or restart the installer as administrator.",
            job.dir.display()
        )
    })?;
    let target = job.target();
    if job.fresh && target.exists() {
        std::fs::remove_file(&target)
            .with_context(|| format!("couldn't delete the old {}", target.display()))?;
    }

    from_first_that_has(&urls, |url, last| {
        fetch_from(url, &target, cancel, on, last)
    })?;

    if job.extract {
        on(Progress::Extracting);
        super::zip::extract(&target, &job.dir, cancel)?;
        on(Progress::Extracted);
    }
    Ok(target)
}

/// Downloads `url` to `target`, resuming a partial file when the server supports it.
/// With `may_restart` false, a server that can't resume is a [`CantResume`] error instead
/// of a restart from zero.
fn fetch_from(
    url: &str,
    target: &Path,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Progress),
    may_restart: bool,
) -> Result<()> {
    http::block_on(async {
        let head = http::client()
            .head(url)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| anyhow!("{NETWORK_ERROR} (ERR1)\n\n{e}"))?;
        if matches!(head.status().as_u16(), 404 | 410) {
            return Err(NotOnServer(head.status().as_u16()).into());
        }
        if !head.status().is_success() {
            bail!(
                "{NETWORK_ERROR} (ERR1)\n\nThe server answered {}.",
                head.status()
            );
        }
        let remote = head
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0);

        let local = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);
        let mut offset = match remote {
            Some(remote) => match plan_resume(local, remote) {
                Resume::Done => {
                    tracing::info!("already downloaded: {}", target.display());
                    on(Progress::Percent(100.0));
                    return Ok(());
                }
                Resume::FromStart => 0,
                Resume::From(o) => o,
            },
            // No size: we can't resume or verify, so always start clean.
            None => 0,
        };

        let mut req = http::client().get(url);
        if offset > 0 {
            tracing::info!(
                "resuming {} at {offset} of {} bytes",
                target.display(),
                remote.unwrap_or(0)
            );
            req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let mut resp = req
            .send()
            .await
            .map_err(|e| anyhow!("{NETWORK_ERROR} (ERR2)\n\n{e}"))?;
        match resp.status().as_u16() {
            206 if offset > 0 => {
                let range = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok());
                if !content_range_matches(range, offset) {
                    bail!("{NETWORK_ERROR} (ERR2)\n\nThe server resumed at the wrong position.");
                }
            }
            200 => {
                if offset > 0 && !may_restart {
                    return Err(CantResume.into());
                }
                if offset > 0 {
                    tracing::info!("server ignored the Range request; restarting from zero");
                }
                offset = 0;
            }
            s => bail!("{NETWORK_ERROR} (ERR2)\n\nThe server answered {s}."),
        }

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(target)
            .with_context(|| format!("Couldn't write {}", target.display()))?;
        file.set_len(offset)?;
        file.seek(SeekFrom::Start(offset))?;

        let total = remote;
        let mut written = offset;
        let mut last_report = Instant::now() - Duration::from_secs(1);
        let mut buf = std::io::BufWriter::with_capacity(1 << 20, file);
        while let Some(chunk) = http::next_chunk(&mut resp, Some(cancel))
            .await
            .map_err(|e| {
                if http::is_cancelled(&e) {
                    e
                } else {
                    anyhow!("{NETWORK_ERROR} (ERR2)\n\n{e}")
                }
            })?
        {
            buf.write_all(&chunk)
                .with_context(|| format!("Couldn't write {}", target.display()))?;
            written += chunk.len() as u64;
            if last_report.elapsed() >= Duration::from_millis(100) {
                last_report = Instant::now();
                if let Some(t) = total {
                    on(Progress::Percent(100.0 * written as f64 / t as f64));
                }
            }
        }
        let file = buf
            .into_inner()
            .map_err(|e| anyhow!("Couldn't write {}: {e}", target.display()))?;
        file.sync_all().ok();

        if let Some(t) = total {
            if written != t {
                tracing::warn!(
                    "download of {} ended at {written} of {t} bytes",
                    target.display()
                );
                bail!(
                    "{NETWORK_ERROR} (ERR2)\n\nThe download ended early ({written} of {t} bytes)."
                );
            }
        }
        on(Progress::Percent(100.0));
        Ok(())
    })
}

/// Personalized patch URLs are credentials of a sort; keep only scheme and host in logs.
pub fn redact(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u)
            if u.host_str() == Some("files.echovr.de")
                && !u.path().starts_with("/updates")
                && !u.path().starts_with("/stuff")
                && u.path().matches('/').count() > 1 =>
        {
            format!("{}://{}/...", u.scheme(), u.host_str().unwrap_or(""))
        }
        Ok(u) if u.query().is_some() => format!(
            "{}://{}{}?...",
            u.scheme(),
            u.host_str().unwrap_or(""),
            u.path()
        ),
        _ => url.to_string(),
    }
}

/// Downloads `url` (relative: from the fastest mirror) into `dir` as `name` and checks it
/// against `sha256`; a file that doesn't match is deleted.
pub fn fetch_pinned(
    url: &str,
    dir: &Path,
    name: &str,
    sha256: &str,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Progress),
) -> Result<PathBuf> {
    let job = Job {
        url: url.into(),
        dir: dir.to_path_buf(),
        filename: name.into(),
        use_mirror: !url.contains("://"),
        fresh: false,
        extract: false,
    };
    let file = run(&job, cancel, on)?;
    if !sha256_matches(&file, sha256) {
        let _ = std::fs::remove_file(&file);
        bail!("{name} didn't download correctly (checksum mismatch). Please try again.");
    }
    Ok(file)
}

/// Lowercase hex SHA-256 of a file, streamed.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    Ok(sha256_reader(&mut f)?)
}

/// Lowercase hex SHA-256 of everything `r` yields.
pub fn sha256_reader(r: &mut impl std::io::Read) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn sha256_matches(path: &Path, expected: &str) -> bool {
    sha256_file(path).is_ok_and(|h| h.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_plan() {
        assert_eq!(plan_resume(0, 100), Resume::FromStart);
        assert_eq!(plan_resume(40, 100), Resume::From(40));
        assert_eq!(plan_resume(100, 100), Resume::Done);
        // A bigger local file is some other/stale file, not "already downloaded".
        assert_eq!(plan_resume(150, 100), Resume::FromStart);
    }

    #[test]
    fn ranks_mirrors_that_answered() {
        let ms = |n| Some(Duration::from_millis(n));
        assert_eq!(rank(vec![("a", ms(300)), ("b", ms(100))]), ["b", "a"]);
        assert_eq!(rank(vec![("a", None), ("b", ms(100))]), ["b"]);
        assert!(rank(vec![("a", None), ("b", None)]).is_empty());
    }

    #[test]
    fn moves_on_only_when_a_mirror_lacks_the_file() {
        let urls = ["https://a/f".to_string(), "https://b/f".to_string()];
        // The first mirror doesn't have it: the second is asked.
        let mut asked = Vec::new();
        let r = from_first_that_has(&urls, |u, _| {
            asked.push(u.to_string());
            if u.starts_with("https://a") {
                Err(NotOnServer(404).into())
            } else {
                Ok(())
            }
        });
        assert!(r.is_ok());
        assert_eq!(asked, urls);
        // Any other failure ends it.
        let mut asked = 0;
        let r = from_first_that_has(&urls, |_, _| {
            asked += 1;
            Err(anyhow!("connection reset"))
        });
        assert!(r.is_err());
        assert_eq!(asked, 1);
        // Nobody has it: that is what the error says.
        let e = from_first_that_has(&urls, |_, _| Err(NotOnServer(404).into())).unwrap_err();
        assert!(e.downcast_ref::<NotOnServer>().is_some());
        assert!(!format!("{e:#}").contains("Ethernet"));
    }

    #[test]
    fn a_mirror_that_cant_resume_hands_over_to_the_next() {
        let urls = ["https://a/f".to_string(), "https://b/f".to_string()];
        // Only the last mirror may restart from zero; the first one passes it on.
        let mut asked = Vec::new();
        let r = from_first_that_has(&urls, |u, last| {
            asked.push((u.to_string(), last));
            if last {
                Ok(())
            } else {
                Err(CantResume.into())
            }
        });
        assert!(r.is_ok());
        assert_eq!(
            asked,
            [
                ("https://a/f".to_string(), false),
                ("https://b/f".to_string(), true)
            ]
        );
    }

    #[test]
    fn content_range_check() {
        assert!(content_range_matches(Some("bytes 40-99/100"), 40));
        assert!(!content_range_matches(Some("bytes 0-99/100"), 40));
        assert!(!content_range_matches(None, 40));
        assert!(!content_range_matches(Some("garbage"), 40));
    }

    /// Mirror probe, full download, then a resume of a truncated file (needs network).
    #[test]
    #[ignore]
    fn live_download_and_resume() {
        let mirror = ranked_mirrors().unwrap()[0];
        let url = format!("{mirror}{MIRROR_TEST_FILE}");
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("t");
        let cancel = AtomicBool::new(false);
        fetch_from(&url, &target, &cancel, &mut |_| {}, true).unwrap();
        let full = std::fs::metadata(&target).unwrap().len();
        let hash = sha256_file(&target).unwrap();
        let f = OpenOptions::new().write(true).open(&target).unwrap();
        f.set_len(full / 3).unwrap();
        drop(f);
        fetch_from(&url, &target, &cancel, &mut |_| {}, true).unwrap();
        assert_eq!(std::fs::metadata(&target).unwrap().len(), full);
        assert_eq!(sha256_file(&target).unwrap(), hash);
    }

    #[test]
    fn sha256_is_zero_padded_lowercase() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(sha256_matches(
            &p,
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        ));
    }
}
