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
    CLIENT.get_or_init(|| build(Duration::from_secs(20)))
}

/// Where only some players may fetch from: dev channels and private mods behind the EchoVRCE
/// sign-in (release.echovr.de's echo-access). Requests there carry the EchoVRCE session.
pub const GATED: &str = "https://release.echovr.de/launcher/gated/";

/// A GET of `url` ([`client_for`]), with the EchoVRCE session when `url` is [`GATED`] (and
/// only then: the session never goes anywhere else).
pub fn get(url: &str) -> reqwest::RequestBuilder {
    signed_in(url, client_for(url).get(url))
}

/// A HEAD of `url`, as [`get`].
pub fn head(url: &str) -> reqwest::RequestBuilder {
    signed_in(url, client_for(url).head(url))
}

fn signed_in(url: &str, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match gated_token(url) {
        Some(token) => req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}")),
        None => req,
    }
}

/// The saved EchoVRCE session for a [`GATED`] `url`, while it is still good (the window and
/// the tray keep it renewed; it is never renewed here, two renewing it would clash).
fn gated_token(url: &str) -> Option<String> {
    if !url.starts_with(GATED) || cfg!(test) {
        return None;
    }
    let t = super::echovrce::store::load()?;
    t.expires()
        .is_none_or(|e| e > super::echovrce::now())
        .then_some(t.token)
}

/// What a server answered instead of the file (in an error's chain: [`status_of`]).
#[derive(Debug)]
pub struct HttpStatus(pub u16);

impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "server responded with {}", self.0)
    }
}

impl std::error::Error for HttpStatus {}

/// The HTTP status an error came from, if it came from one (as its cause or its context).
pub fn status_of(e: &anyhow::Error) -> Option<u16> {
    e.downcast_ref::<HttpStatus>()
        .or_else(|| e.chain().find_map(|c| c.downcast_ref::<HttpStatus>()))
        .map(|s| s.0)
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn a_status_is_found_as_cause_or_context() {
        let cause = anyhow::Error::new(HttpStatus(401)).context("GET x");
        assert_eq!(status_of(&cause), Some(401));
        let context = anyhow!("something").context(HttpStatus(403));
        assert_eq!(status_of(&context), Some(403));
        assert_eq!(status_of(&anyhow!("nothing")), None);
    }
}

/// The client for `url`: one to the CDN gives up on a connection sooner, as
/// release.echovr.de is there to fall back on ([`super::sources`]).
pub fn client_for(url: &str) -> &'static Client {
    static CDN: OnceLock<Client> = OnceLock::new();
    if super::sources::is_cdn(url) {
        CDN.get_or_init(|| build(super::sources::CDN_CONNECT_TIMEOUT))
    } else {
        client()
    }
}

fn build(connect_timeout: Duration) -> Client {
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(connect_timeout)
        .read_timeout(STALL_TIMEOUT)
        .build()
        .expect("HTTP client")
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
    Ok(dated_text(url, timeout)?.map(|(t, _)| t))
}

/// [`raw_text`], with the time the server says it sent it (its `Date`), if it says.
pub(crate) fn dated_text(
    url: &str,
    timeout: Duration,
) -> Result<Option<(String, Option<time::OffsetDateTime>)>> {
    let got = block_on(async {
        let resp = get(url)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let date = resp
            .headers()
            .get(reqwest::header::DATE)
            .and_then(|d| d.to_str().ok())
            .and_then(|d| {
                time::OffsetDateTime::parse(d, &time::format_description::well_known::Rfc2822).ok()
            });
        match resp.status().as_u16() {
            404 | 410 => Ok(None),
            s if (200..300).contains(&s) => Ok(Some((resp.text().await?, date))),
            s => Err(anyhow::Error::new(HttpStatus(s)).context(format!("GET {url}"))),
        }
    });
    if let Err(e) = &got {
        super::sources::failed(url, e);
    }
    got
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
    let done = download_to_inner(url, dest, cancel);
    if let Err(e) = &done {
        super::sources::failed(url, e);
    }
    done
}

fn download_to_inner(url: &str, dest: &Path, cancel: Option<&AtomicBool>) -> Result<()> {
    use std::io::Write;
    block_on(async {
        let mut resp = get(url)
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

/// [`download_to`] for a file that must have `sha256`: by its checksum first (the CDN's
/// store, then release.echovr.de's, [`super::sources::by_checksum`]), kept only when it
/// matches; else from `url` (which the caller checks, as before).
pub fn download_checked(
    url: &str,
    sha256: &str,
    dest: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    for from in super::sources::by_checksum(url, sha256) {
        match download_to(&from, dest, cancel) {
            Ok(()) if super::download::sha256_matches(dest, sha256) => return Ok(()),
            Ok(()) => tracing::warn!(
                "{} from {from} isn't what its checksum says; trying the next server",
                file_of(url)
            ),
            Err(e) if is_cancelled(&e) => return Err(e),
            Err(e) => tracing::info!("{} by checksum: {e:#}", file_of(url)),
        }
        let _ = std::fs::remove_file(dest);
    }
    download_to(url, dest, cancel)
}

fn file_of(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

/// The client for launcher plugins' requests: it follows a redirect only on the same
/// server (scheme, host and port), so a server can't send a plugin's request, and what it
/// carries, somewhere else; and it keeps no cookies.
fn plugin_client() -> &'static Client {
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
            .redirect(reqwest::redirect::Policy::custom(|a| {
                let same = a
                    .previous()
                    .first()
                    .is_some_and(|first| first.origin() == a.url().origin());
                if a.previous().len() > 5 {
                    a.error("too many redirects")
                } else if same {
                    a.follow()
                } else {
                    let to = a.url().origin().ascii_serialization();
                    a.error(format!("it redirects to another server ({to})"))
                }
            }))
            .build()
            .expect("HTTP client")
    })
}

/// A launcher plugin's request that its server redirected to another one ([`plugin_get`],
/// [`plugin_post`]): not followed.
#[derive(Debug, thiserror::Error)]
#[error("its server sends it on to another server, which a plugin's request doesn't follow")]
pub struct RedirectRefused;

/// A plugin's request that got no answer: [`RedirectRefused`] for a redirect elsewhere.
fn plugin_error(method: &str, url: &str, e: reqwest::Error) -> anyhow::Error {
    if e.is_redirect() {
        anyhow::Error::new(RedirectRefused).context(format!("{method} {url}"))
    } else {
        anyhow::Error::new(e).context(format!("{method} {url}"))
    }
}

/// A launcher plugin's GET: the answer's text, or an error ("server responded with
/// <status>" for an error answer, as [`get_text`] says it).
pub fn plugin_get(url: &str) -> Result<String> {
    block_on(async {
        let resp = plugin_client()
            .get(url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| plugin_error("GET", url, e))?;
        if !resp.status().is_success() {
            bail!("GET {url}: server responded with {}", resp.status());
        }
        Ok(resp.text().await?)
    })
}

/// A launcher plugin's POST of JSON: (status, body), as [`post_json`].
pub fn plugin_post(url: &str, body: &serde_json::Value) -> Result<(u16, String)> {
    block_on(async {
        let resp = plugin_client()
            .post(url)
            .timeout(Duration::from_secs(120))
            .json(body)
            .send()
            .await
            .map_err(|e| plugin_error("POST", url, e))?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().await.unwrap_or_default()))
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A server answering `/ok` with `name`, `/same` with a redirect to its own `/ok`, and
    /// `/away` with one to `away`.
    fn serve(name: &'static str, away: Option<String>) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let own = base.clone();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let to = match req.url() {
                    "/same" => Some(format!("{own}/ok")),
                    "/away" => away.clone(),
                    _ => None,
                };
                let resp = match to {
                    Some(to) => tiny_http::Response::from_string("")
                        .with_status_code(307)
                        .with_header(tiny_http::Header::from_bytes("Location", to).unwrap()),
                    None => tiny_http::Response::from_string(name),
                };
                let _ = req.respond(resp);
            }
        });
        base
    }

    #[test]
    fn a_plugins_request_follows_redirects_on_its_server_only() {
        let other = serve("other", None);
        let mine = serve("mine", Some(format!("{other}/ok")));
        assert_eq!(plugin_get(&format!("{mine}/same")).unwrap(), "mine");
        let err = plugin_get(&format!("{mine}/away")).unwrap_err();
        assert!(err.downcast_ref::<RedirectRefused>().is_some(), "{err:#}");
        assert!(format!("{err:#}").contains("another server"), "{err:#}");
        // A POST (whose body a 307 would carry along) the same.
        let (status, text) =
            plugin_post(&format!("{mine}/same"), &serde_json::json!({"a": 1})).unwrap();
        assert_eq!((status, text.as_str()), (200, "mine"));
        let err = plugin_post(
            &format!("{mine}/away"),
            &serde_json::json!({"password": "pw"}),
        )
        .unwrap_err();
        assert!(err.downcast_ref::<RedirectRefused>().is_some(), "{err:#}");
    }
}
