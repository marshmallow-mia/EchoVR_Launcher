//! Zip extraction. Every entry name is resolved with `enclosed_name()`, so an entry like
//! `../../Windows/System32/x.dll` (Zip Slip) is rejected instead of written outside the
//! destination -- the Java `UnzipFile` concatenated names blindly, which mattered most for
//! the artwork zip the elevated helper extracts into Program Files.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};

pub fn extract(zip_path: &Path, dest: &Path, cancel: &AtomicBool) -> Result<usize> {
    extract_root(zip_path, None, dest, cancel)
}

/// The one folder every entry of the zip at `zip_path` is in ("name/"), if there is one.
pub fn top_folder(zip_path: &Path) -> Result<Option<String>> {
    let file = File::open(zip_path).with_context(|| format!("open {}", zip_path.display()))?;
    let archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid zip file", zip_path.display()))?;
    let mut top: Option<&str> = None;
    for name in archive.file_names() {
        let Some((first, _)) = name.split_once('/') else {
            return Ok(None);
        };
        match top {
            None => top = Some(first),
            Some(t) if t == first => {}
            Some(_) => return Ok(None),
        }
    }
    Ok(top.filter(|t| !t.is_empty()).map(|t| format!("{t}/")))
}

/// [`extract`], with the archive's one top folder `from` ("name/") put at `dest` instead:
/// only entries under it are taken. Builds packed under their own folder name land where
/// every install has its game (`ready-at-dawn-echo-arena`).
pub fn extract_root(
    zip_path: &Path,
    from: Option<&str>,
    dest: &Path,
    cancel: &AtomicBool,
) -> Result<usize> {
    let file = File::open(zip_path).with_context(|| format!("open {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid zip file", zip_path.display()))?;
    std::fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    tracing::info!("extract {} -> {}", zip_path.display(), dest.display());

    let mut files = 0;
    let mut buf = vec![0u8; 1 << 16];
    for i in 0..archive.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(super::http::Cancelled.into());
        }
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            bail!("The archive contains an unsafe path: {}", entry.name());
        };
        let rel = match from {
            None => rel,
            // Outside the top folder (or the folder itself): not part of the game.
            Some(top) => match rel.strip_prefix(top.trim_end_matches('/')) {
                Ok(r) if !r.as_os_str().is_empty() => r.to_path_buf(),
                _ => continue,
            },
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).with_context(|| format!("create {}", out.display()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let f = File::create(&out).with_context(|| format!("write {}", out.display()))?;
        let mut w = BufWriter::new(f);
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])
                .with_context(|| format!("write {}", out.display()))?;
        }
        w.flush()?;
        files += 1;
    }
    tracing::info!("extracted {files} file(s)");
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        for (name, data) in entries {
            z.start_file(*name, SimpleFileOptions::default()).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn puts_the_top_folder_where_asked() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("h.zip");
        make_zip(
            &zip,
            &[
                ("Echo VR Halloween 2017/bin/win7/EchoArena.exe", b"exe"),
                ("Echo VR Halloween 2017/Play.bat", b"bat"),
                ("stray.txt", b"not the game"),
            ],
        );
        let out = dir.path().join("v/ready-at-dawn-echo-arena");
        let n = extract_root(
            &zip,
            Some("Echo VR Halloween 2017/"),
            &out,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(top_folder(&zip).unwrap(), None, "stray.txt is outside it");
        assert_eq!(
            std::fs::read(out.join("bin/win7/EchoArena.exe")).unwrap(),
            b"exe"
        );
        let one = dir.path().join("one.zip");
        make_zip(
            &one,
            &[
                ("echo-vr-6/bin/win7/x.dll", b"x"),
                ("echo-vr-6/_local/c.json", b"c"),
            ],
        );
        assert_eq!(top_folder(&one).unwrap().as_deref(), Some("echo-vr-6/"));
        assert!(!out.join("stray.txt").exists());
        assert!(!dir.path().join("v/stray.txt").exists());
    }

    #[test]
    fn extracts_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("a.zip");
        make_zip(&zip, &[("a/b/c.txt", b"hi"), ("top.txt", b"x")]);
        let out = dir.path().join("out");
        assert_eq!(extract(&zip, &out, &AtomicBool::new(false)).unwrap(), 2);
        assert_eq!(std::fs::read(out.join("a/b/c.txt")).unwrap(), b"hi");
    }

    #[test]
    fn rejects_zip_slip() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("evil.zip");
        make_zip(&zip, &[("../escaped.txt", b"pwned")]);
        let out = dir.path().join("out");
        assert!(extract(&zip, &out, &AtomicBool::new(false)).is_err());
        assert!(!dir.path().join("escaped.txt").exists());
    }
}
