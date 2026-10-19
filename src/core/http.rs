//! One shared HTTP client with sane timeouts, driven from plain worker threads.
//!
//! The Java code used bare `URL.openStream()` everywhere, which has no connect or read
//! timeout: a stalled mirror froze a wizard forever. Every request here has a connect
//! timeout, and bodies are read chunk by chunk with a stall timeout rather than a total
//! one, so a multi-gigabyte download is never cut off for merely being large -- and a
//! cancel request is noticed within a quarter second even while the network is silent.

use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::{Client, Response};

pub const USER_AGENT: &str = concat!("EchoVR-Launcher/", env!("CARGO_PKG_VERSION"));

/// How long a body may go without delivering a byte before the transfer is abandoned.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(60);

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("http")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Runs a future to completion from a (non-async) worker thread.
pub fn block_on<F: Future>(f: F) -> F::Output {
    runtime().block_on(f)
}

pub fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(STALL_TIMEOUT)
            .build()
            .expect("HTTP client")
    })
}

/// Error for a transfer the user cancelled. Callers check for it with `is_cancelled`.
#[derive(Debug, thiserror::Error)]
#[error("cancelled")]
pub struct Cancelled;

pub fn is_cancelled(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Cancelled>().is_some()
}

/// Reads the next body chunk, giving up after [`STALL_TIMEOUT`] of silence and returning
/// early with [`Cancelled`] once `cancel` is set.
pub async fn next_chunk(
    resp: &mut Response,
    cancel: Option<&AtomicBool>,
) -> Result<Option<bytes::Bytes>> {
    let mut waited = Duration::ZERO;
    let tick = Duration::from_millis(250);
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Cancelled.into());
        }
        match tokio::time::timeout(tick, resp.chunk()).await {
            Ok(r) => return Ok(r?),
            Err(_) => {
                waited += tick;
                if waited >= STALL_TIMEOUT {
                    bail!(
                        "the download stalled (no data for {}s)",
                        STALL_TIMEOUT.as_secs()
                    );
                }
            }
        }
    }
}

/// Fetches a small text resource (manifests) in full. One on release.echovr.de may come
/// from the CDN, checked ([`super::sources::text`]).
pub fn get_text(url: &str) -> Result<String> {
    super::sources::text(url, Duration::from_secs(60))?
        .ok_or_else(|| anyhow!("GET {url}: server responded with 404 Not Found"))
}

/// [`get_text`], but a resource that isn't there (404 or 410) is `None` instead of an error:
/// for optional files such as a channel's overlay.
pub fn get_text_opt(url: &str) -> Result<Option<String>> {
    super::sources::text(url, Duration::from_secs(60))
}

/// [`get_text_opt`] from exactly `url`, within `timeout`.
pub(crate) fn raw_text(url: &str, timeout: Duration) -> Result<Option<String>> {
    block_on(async {
        let resp = client()
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        match resp.status().as_u16() {
            404 | 410 => Ok(None),
            s if (200..300).contains(&s) => Ok(Some(resp.text().await?)),
            _ => bail!("GET {url}: server responded with {}", resp.status()),
        }
    })
}

/// Fetches a small binary resource (feed images) in full.
pub fn get_bytes(url: &str) -> Result<Vec<u8>> {
    block_on(async {
        let resp = client()
            .get(url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            bail!("GET {url}: server responded with {}", resp.status());
        }
        Ok(resp.bytes().await?.to_vec())
    })
}

/// Streams a URL straight to `dest`. For small update files; no progress reporting.
pub fn download_to(url: &str, dest: &Path, cancel: Option<&AtomicBool>) -> Result<()> {
    use std::io::Write;
    block_on(async {
        let mut resp = client()
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            bail!("GET {url}: server responded with {}", resp.status());
        }
        let mut f =
            std::fs::File::create(dest).with_context(|| format!("create {}", dest.display()))?;
        while let Some(chunk) = next_chunk(&mut resp, cancel).await? {
            f.write_all(&chunk)
                .with_context(|| format!("write {}", dest.display()))?;
        }
        f.sync_all().ok();
        Ok(())
    })
}

/// [`download_to`] for a file that must have `sha256`: from the CDN first when it has a copy
/// of `url` and that copy matches, else from `url` (which the caller checks, as before).
pub fn download_checked(
    url: &str,
    sha256: &str,
    dest: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    if let Some(cdn) = super::sources::cdn_url(url).filter(|_| !sha256.is_empty()) {
        match download_to(&cdn, dest, cancel) {
            Ok(()) if super::download::sha256_matches(dest, sha256) => return Ok(()),
            Ok(()) => tracing::info!(
                "the CDN's {} is behind; from release.echovr.de",
                file_of(url)
            ),
            Err(e) if is_cancelled(&e) => return Err(e),
            Err(e) => tracing::info!("CDN {}: {e:#}; from release.echovr.de", file_of(url)),
        }
        let _ = std::fs::remove_file(dest);
    }
    download_to(url, dest, cancel)
}

fn file_of(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

/// POSTs JSON and returns (status, body).
pub fn post_json(url: &str, body: &serde_json::Value) -> Result<(u16, String)> {
    block_on(async {
        let resp = client()
            .post(url)
            .timeout(Duration::from_secs(120))
            .json(body)
            .send()
            .await
            .map_err(|e| anyhow!("POST {url}: {e}"))?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().await.unwrap_or_default()))
    })
}
