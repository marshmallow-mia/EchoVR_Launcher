//! Single files out of a zip on the server, without downloading the whole zip: the
//! server answers range requests, so the archive's directory and each wanted member are
//! read in blocks. Repairing an install fetches only its broken files this way.
//!
//! The zip is read by its checksum where that is known (the CDN's store, then
//! release.echovr.de's, [`super::sources::by_checksum`]), its own URL last: a block one of
//! them can't send comes from the next. Every file taken out is checked anyway.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::http::{self, Cancelled};

/// How much one range request fetches. Aligned blocks this size hold a big zip's whole
/// end (its directory) in one request, and stream members at a few requests per second.
const BLOCK: u64 = 4 << 20;

/// The file at `urls` (the same file at each) as `Read + Seek`, fetched block by block with
/// range requests.
pub struct RangeReader<'a> {
    urls: Vec<String>,
    /// The one in use.
    at: usize,
    len: u64,
    pos: u64,
    /// The block in hand: where it starts in the file, and its bytes.
    start: u64,
    block: Vec<u8>,
    cancel: &'a AtomicBool,
}

impl<'a> RangeReader<'a> {
    /// The file at the first of `urls` that says how big it is.
    pub fn open_any(urls: &[String], cancel: &'a AtomicBool) -> Result<Self> {
        let mut last = anyhow::anyhow!("no address to read the zip from");
        for (at, url) in urls.iter().enumerate() {
            match size_of(url) {
                Ok(len) => {
                    return Ok(RangeReader {
                        urls: urls.to_vec(),
                        at,
                        len,
                        pos: 0,
                        start: 0,
                        block: Vec::new(),
                        cancel,
                    })
                }
                Err(e) => {
                    tracing::info!("repair: {e:#}; trying the next server");
                    last = e;
                }
            }
        }
        Err(last)
    }

    /// Fetches the aligned block holding byte `at`: from the address in use, else the next
    /// ones (that have a file of the same size).
    fn fetch(&mut self, at: u64) -> io::Result<()> {
        let start = at / BLOCK * BLOCK;
        let end = (start + BLOCK).min(self.len) - 1;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(io::Error::other(Cancelled));
            }
            match block(&self.urls[self.at], start, end) {
                Ok(bytes) => {
                    self.start = start;
                    self.block = bytes;
                    return Ok(());
                }
                Err(e) => {
                    let next = (self.at + 1..self.urls.len())
                        .find(|&i| size_of(&self.urls[i]).is_ok_and(|n| n == self.len));
                    match next {
                        Some(i) => {
                            tracing::info!("repair: {e:#}; reading on from the next server");
                            self.at = i;
                        }
                        None => return Err(io::Error::other(e)),
                    }
                }
            }
        }
    }
}

/// The size the server at `url` reports.
fn size_of(url: &str) -> Result<u64> {
    let got = http::block_on(async {
        let resp = http::client_for(url)
            .head(url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .with_context(|| format!("HEAD {url}"))?;
        if !resp.status().is_success() {
            bail!("HEAD {url}: server responded with {}", resp.status());
        }
        resp.headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
            .with_context(|| format!("{url}: the server doesn't say how big it is"))
    });
    if let Err(e) = &got {
        super::sources::failed(url, e);
    }
    got
}

/// Bytes `start..=end` of the file at `url`.
fn block(url: &str, start: u64, end: u64) -> Result<Vec<u8>> {
    let got = http::block_on(async {
        let resp = http::client_for(url)
            .get(url)
            .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            bail!(
                "{url}: the server didn't send part of the file ({})",
                resp.status()
            );
        }
        Ok(resp.bytes().await?)
    });
    if let Err(e) = &got {
        super::sources::failed(url, e);
    }
    let bytes = got?;
    if bytes.len() as u64 != end - start + 1 {
        bail!(
            "{url}: asked for {} bytes, got {}",
            end - start + 1,
            bytes.len()
        );
    }
    Ok(bytes.to_vec())
}

impl Read for RangeReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let held = self.start..self.start + self.block.len() as u64;
        if !held.contains(&self.pos) {
            self.fetch(self.pos)?;
        }
        let from = (self.pos - self.start) as usize;
        let n = buf.len().min(self.block.len() - from);
        buf[..n].copy_from_slice(&self.block[from..from + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for RangeReader<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::other("seek before the start of the file"))?;
        Ok(self.pos)
    }
}

/// Writes `members` (paths inside the archive's folder `root`, each with its SHA-256) of
/// the zip at `url` (whose SHA-256 is `sha`, when known) into `dest`. Each is checked before
/// it replaces the file there; `on` hears which one is next.
pub fn extract_members(
    url: &str,
    sha: Option<&str>,
    root: &str,
    members: &[(String, String)],
    dest: &Path,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(usize, &str),
) -> Result<()> {
    let mut urls = sha.map_or_else(Vec::new, |sha| super::sources::by_checksum(url, sha));
    urls.push(url.to_string());
    let reader = RangeReader::open_any(&urls, cancel)?;
    let mut zip = zip::ZipArchive::new(reader)
        .with_context(|| format!("{url} isn't a zip the launcher can read"))?;
    for (i, (path, sha)) in members.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        on(i, path);
        let name = format!("{root}{path}");
        let mut entry = zip
            .by_name(&name)
            .with_context(|| format!("{name} isn't in {url}"))?;
        let out = dest.join(path);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let part = out.with_file_name(format!(
            "{}.part",
            out.file_name().unwrap_or_default().to_string_lossy()
        ));
        let copied = std::fs::File::create(&part).and_then(|mut f| io::copy(&mut entry, &mut f));
        if let Err(e) = copied {
            let _ = std::fs::remove_file(&part);
            if e.get_ref().is_some_and(|c| c.is::<Cancelled>()) {
                return Err(Cancelled.into());
            }
            return Err(e).with_context(|| format!("Couldn't download {path}"));
        }
        if !super::download::sha256_matches(&part, sha) {
            let _ = std::fs::remove_file(&part);
            bail!("{path} from the server doesn't match its checksum. Please try again.");
        }
        std::fs::rename(&part, &out).with_context(|| {
            format!(
                "Couldn't replace {}. Please close Echo VR and try again.",
                out.display()
            )
        })?;
        tracing::info!("repaired {path}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// Serves `body` on a local port, answering range requests (as files.echovr.de does).
    fn serve(body: Vec<u8>) -> String {
        serve_for(body, usize::MAX)
    }

    /// [`serve`] for `n` requests, then failing (503).
    fn serve_for(body: Vec<u8>, n: usize) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/build.zip", server.server_addr());
        std::thread::spawn(move || {
            for (i, req) in server.incoming_requests().enumerate() {
                if i >= n {
                    let _ = req.respond(tiny_http::Response::empty(503));
                    continue;
                }
                let range = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Range"))
                    .and_then(|h| {
                        let (a, b) = h.value.as_str().strip_prefix("bytes=")?.split_once('-')?;
                        Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                    });
                let resp = match range {
                    Some((a, b)) => {
                        tiny_http::Response::from_data(body[a..=b].to_vec()).with_status_code(206)
                    }
                    None => tiny_http::Response::from_data(body.clone()),
                };
                // Its size said, however big (as a file server does).
                let _ = req.respond(resp.with_chunked_threshold(usize::MAX));
            }
        });
        url
    }

    fn sha(bytes: &[u8]) -> String {
        crate::core::download::sha256_reader(&mut &bytes[..]).unwrap()
    }

    /// The live PC build: one small file out of the 5 GB `pc.zip`, by its manifest.
    #[test]
    #[ignore = "network"]
    fn reads_one_file_out_of_the_live_build() {
        let m = crate::core::manifest::Manifest::fetch("https://release.echovr.de/pc.zip.manifest")
            .unwrap();
        let e = m
            .adds()
            .find(|e| e.path == "bin/win10/dbgcore.dll")
            .unwrap();
        let members = vec![(e.path.clone(), e.sha256.clone().unwrap())];
        let dir = tempfile::tempdir().unwrap();
        let url = m.archive_url().unwrap();
        let root = m.archive_root.clone().unwrap();
        extract_members(
            &url,
            m.archive_sha.as_deref(),
            &root,
            &members,
            dir.path(),
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap();
        assert!(dir.path().join("bin/win10/dbgcore.dll").is_file());
    }

    #[test]
    fn repairs_single_files_from_a_remote_zip() {
        let big: Vec<u8> = (0..(BLOCK as usize + 1000))
            .map(|i| (i % 251) as u8)
            .collect();
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, data) in [
            ("game/bin/a.dll", &b"good a"[..]),
            ("game/data/big.pak", &big[..]),
            ("game/other.txt", &b"untouched"[..]),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        let url = serve(zip.finish().unwrap().into_inner());

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin/a.dll"), b"broken").unwrap();
        let members = vec![
            ("bin/a.dll".to_string(), sha(b"good a")),
            ("data/big.pak".to_string(), sha(&big)),
        ];
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        extract_members(
            &url,
            None,
            "game/",
            &members,
            dir.path(),
            &cancel,
            &mut |i, _| seen.push(i),
        )
        .unwrap();
        assert_eq!(seen, [0, 1]);
        assert_eq!(
            std::fs::read(dir.path().join("bin/a.dll")).unwrap(),
            b"good a"
        );
        assert_eq!(std::fs::read(dir.path().join("data/big.pak")).unwrap(), big);
        assert!(
            !dir.path().join("other.txt").exists(),
            "only what was asked for"
        );

        // A checksum that doesn't match leaves the file as it was.
        let wrong = vec![("bin/a.dll".to_string(), sha(b"something else"))];
        std::fs::write(dir.path().join("bin/a.dll"), b"broken").unwrap();
        assert!(extract_members(
            &url,
            None,
            "game/",
            &wrong,
            dir.path(),
            &cancel,
            &mut |_, _| {}
        )
        .is_err());
        assert_eq!(
            std::fs::read(dir.path().join("bin/a.dll")).unwrap(),
            b"broken"
        );
        assert!(!dir.path().join("bin/a.dll.part").exists());
    }

    #[test]
    fn a_server_that_goes_away_midway_is_read_on_from_the_next() {
        let big: Vec<u8> = (0..(3 * BLOCK as usize)).map(|i| (i % 249) as u8).collect();
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let stored =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("game/data/big.pak", stored).unwrap();
        zip.write_all(&big).unwrap();
        let body = zip.finish().unwrap().into_inner();
        // Its size and the zip's end, then failing; an address with a different file is
        // passed over.
        let flaky = serve_for(body.clone(), 2);
        let other = serve(b"not the same file".to_vec());
        let good = serve(body);
        let cancel = AtomicBool::new(false);
        let mut reader = RangeReader::open_any(&[flaky, other, good], &cancel).unwrap();
        let mut all = Vec::new();
        reader.read_to_end(&mut all).unwrap();
        assert_eq!(reader.at, 2, "{:?}", reader.urls);
        let mut zip = zip::ZipArchive::new(io::Cursor::new(all)).unwrap();
        let mut got = Vec::new();
        zip.by_name("game/data/big.pak")
            .unwrap()
            .read_to_end(&mut got)
            .unwrap();
        assert_eq!(got, big);
    }
}
