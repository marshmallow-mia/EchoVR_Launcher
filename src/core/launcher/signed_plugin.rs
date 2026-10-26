//! A plugin release in the signed-plugin shape (release-signer's `docs/plugin-release.md`,
//! section 4): three files, and nothing is installed before every one is checked.
//!
//! - `<id>-v<version>-windows.zip`: flat, no folders: `<file>.dll`, `LICENSE`, `NOTICE`
//!   when the repo has one, `<Stem>.plugin.json` when the plugin has settings, and a
//!   `SHA256SUMS` over the other entries;
//! - `echo-mod.json`: the manifest, `"package"` naming the zip;
//! - `SHA256SUMS`: a line for the zip and a line for `echo-mod.json`.
//!
//! [`verify_files`] checks the three downloads against the top `SHA256SUMS`, [`unpack`]
//! checks the zip's entries against its own, and [`check_manifest`] checks the manifest
//! says what the catalogue entry says. Signing replaces the DLL and regenerates both sums
//! files, so a signed release's sums are not the attested ones: what the catalogue pins is
//! decided by the caller. The older layout, a loose DLL, is [`super::mods::install`]'s.

#![allow(dead_code)] // nothing calls this until the install change that follows

use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::core::download;

/// The sums file's name, in the release and in the zip.
pub const SUMS: &str = "SHA256SUMS";
/// The manifest's name in the release.
pub const MANIFEST: &str = "echo-mod.json";
/// What a plugin's zip always holds besides its DLL and the sums.
const LICENSE: &str = "LICENSE";
const NOTICE: &str = "NOTICE";
/// The signer's limit for a DLL (spec section 6); no entry of the zip may be bigger.
const MAX_ENTRY: u64 = 16 << 20;

/// A `SHA256SUMS` file: file name to lowercase hex SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sums(BTreeMap<String, String>);

impl Sums {
    /// `<hex>  <name>` per line (`sha256sum`'s text form; ` *<name>` is read too). A line
    /// that doesn't fit, a name that isn't a plain file name, or a name twice (also in
    /// another case, which Windows would take as one file) refuses the whole file.
    pub fn parse(text: &str) -> Result<Sums> {
        let mut map = BTreeMap::new();
        for (i, line) in text.lines().enumerate() {
            let n = i + 1;
            let Some((hash, rest)) = line.split_once(' ') else {
                bail!("{SUMS} line {n} isn't `<sha256>  <name>`");
            };
            let Some(name) = rest.strip_prefix([' ', '*']) else {
                bail!("{SUMS} line {n} isn't `<sha256>  <name>`");
            };
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("{SUMS} line {n} has no valid sha256");
            }
            if !is_plain_name(name) {
                bail!("{SUMS} line {n} names {name:?}, which isn't a plain file name");
            }
            let dup = map.keys().any(|k: &String| k.eq_ignore_ascii_case(name));
            if dup
                || map
                    .insert(name.to_string(), hash.to_ascii_lowercase())
                    .is_some()
            {
                bail!("{SUMS} lists {name} twice");
            }
        }
        if map.is_empty() {
            bail!("{SUMS} lists nothing");
        }
        Ok(Sums(map))
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

/// Pure: a file name of the plain kind a release uses (no folders, no drive or stream
/// syntax, no leading dot, no control characters).
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.ends_with(['.', ' '])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

/// The three downloads of a release, each by the name `sums` knows it by. Every one must
/// be listed and match; the sums file itself is what is being checked against, so it is
/// not in `files`. Nothing is read from a file whose name `sums` lacks.
pub fn verify_files(sums: &Sums, files: &[(&str, &Path)]) -> Result<()> {
    for (name, path) in files {
        let Some(want) = sums.get(name) else {
            bail!("{SUMS} doesn't list {name}");
        };
        let have = download::sha256_file(path)?;
        if !have.eq_ignore_ascii_case(want) {
            bail!("{name} doesn't match {SUMS} (it is {have}, the sums say {want})");
        }
    }
    Ok(())
}

/// `echo-mod.json`, the fields the launcher checks.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    /// The plugin's file name.
    pub file: String,
    /// The zip's name.
    pub package: String,
    /// A settings description in the zip (`<Stem>.plugin.json`), or inline.
    pub settings: serde_json::Value,
}

/// Reads the manifest and checks it is the release the catalogue entry names: its `id`,
/// `version` and DLL `file`, and the zip it names, `package`, is the one downloaded.
pub fn check_manifest(
    bytes: &[u8],
    id: &str,
    version: &str,
    file: &str,
    package: &str,
) -> Result<Manifest> {
    let m: Manifest = serde_json::from_slice(bytes).context("echo-mod.json isn't valid")?;
    for (what, have, want) in [
        ("id", &m.id, id),
        ("version", &m.version, version),
        ("file", &m.file, file),
        ("package", &m.package, package),
    ] {
        if have != want {
            bail!("echo-mod.json's {what} is {have:?}, the catalogue's is {want:?}");
        }
    }
    Ok(m)
}

/// What a release's zip held, written into a staging folder after every entry checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpacked {
    /// The plugin's DLL.
    pub dll: PathBuf,
    /// Its `<Stem>.plugin.json`, when the zip has one.
    pub settings: Option<PathBuf>,
}

/// Checks `zip` and writes the DLL (and its settings file) into `staging`, which is made
/// empty first. The zip is refused unless it holds exactly `dll`, `LICENSE`, the sums,
/// and optionally `NOTICE` and `<Stem>.plugin.json`: flat, no folders, nothing that isn't
/// a plain file, each at most 16 MiB; and its own `SHA256SUMS` lists exactly the other
/// entries, each matching. Nothing is written when anything fails.
pub fn unpack(zip: impl Read + Seek, dll: &str, staging: &Path) -> Result<Unpacked> {
    let stem = dll
        .strip_suffix(".dll")
        .filter(|s| !s.is_empty() && super::mods::is_plugin_file(dll))
        .with_context(|| format!("{dll:?} isn't a plugin file name"))?;
    let settings_name = format!("{stem}.plugin.json");
    let mut archive = zip::ZipArchive::new(zip).context("the release's zip isn't a zip")?;
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if entry.is_dir() || entry.is_symlink() || !entry.is_file() {
            bail!("the zip has an entry that isn't a plain file: {name:?}");
        }
        if !is_plain_name(&name) {
            bail!("the zip has an entry that isn't a plain file name: {name:?}");
        }
        let known = [dll, LICENSE, NOTICE, SUMS, settings_name.as_str()];
        if !known.contains(&name.as_str()) {
            bail!("the zip has a file it shouldn't: {name}");
        }
        if entries.keys().any(|k| k.eq_ignore_ascii_case(&name)) {
            bail!("the zip has {name} twice");
        }
        if entry.size() > MAX_ENTRY {
            bail!("{name} is bigger than {} MiB", MAX_ENTRY >> 20);
        }
        let mut bytes = Vec::new();
        entry.take(MAX_ENTRY + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_ENTRY {
            bail!("{name} is bigger than {} MiB", MAX_ENTRY >> 20);
        }
        entries.insert(name, bytes);
    }
    for need in [dll, LICENSE, SUMS] {
        if !entries.contains_key(need) {
            bail!("the zip lacks {need}");
        }
    }
    let inner = Sums::parse(&String::from_utf8_lossy(&entries[SUMS]))?;
    for name in entries.keys().filter(|n| n.as_str() != SUMS) {
        let Some(want) = inner.get(name) else {
            bail!("the zip's {SUMS} doesn't list {name}");
        };
        let have = download::sha256_reader(&mut entries[name].as_slice())?;
        if have != want {
            bail!("{name} doesn't match the zip's {SUMS}");
        }
    }
    if let Some(extra) = inner.names().find(|n| !entries.contains_key(*n)) {
        bail!("the zip's {SUMS} lists {extra}, which the zip lacks");
    }

    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).with_context(|| format!("create {}", staging.display()))?;
    let write = |name: &str| -> Result<PathBuf> {
        let path = staging.join(name);
        std::fs::write(&path, &entries[name])
            .with_context(|| format!("write {}", path.display()))?;
        Ok(path)
    };
    Ok(Unpacked {
        dll: write(dll)?,
        settings: entries
            .contains_key(&settings_name)
            .then(|| write(&settings_name))
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn sha(bytes: &[u8]) -> String {
        download::sha256_reader(&mut &bytes[..]).unwrap()
    }

    fn sums_of(files: &[(&str, &[u8])]) -> String {
        files
            .iter()
            .map(|(n, b)| format!("{}  {n}\n", sha(b)))
            .collect()
    }

    fn zip_of(files: &[(&str, &[u8])]) -> Cursor<Vec<u8>> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(bytes).unwrap();
        }
        let mut c = w.finish().unwrap();
        c.set_position(0);
        c
    }

    type Files = Vec<(String, Vec<u8>)>;

    /// A release zip as the spec has it: the DLL, LICENSE and the inner sums.
    fn good(extra: &[(&str, &[u8])]) -> Files {
        let mut files: Vec<(&str, &[u8])> = vec![("Foo.dll", b"MZ-foo"), ("LICENSE", b"MIT")];
        files.extend_from_slice(extra);
        let sums = sums_of(&files);
        let mut out: Files = files
            .iter()
            .map(|(n, b)| (n.to_string(), b.to_vec()))
            .collect();
        out.push((SUMS.into(), sums.into_bytes()));
        out
    }

    fn zip_owned(files: &Files) -> Cursor<Vec<u8>> {
        let v: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), &b[..])).collect();
        zip_of(&v)
    }

    #[test]
    fn sums_parse() {
        let h = "ab".repeat(32);
        let s = Sums::parse(&format!("{h}  a.zip\n{h} *echo-mod.json\n")).unwrap();
        assert_eq!(s.get("a.zip"), Some(h.as_str()));
        assert_eq!(s.get("echo-mod.json"), Some(h.as_str()));
        // Upper case is read as lower case.
        let up = Sums::parse(&format!("{}  a.zip\n", h.to_uppercase())).unwrap();
        assert_eq!(up.get("a.zip"), Some(h.as_str()));
    }

    #[test]
    fn sums_refuse_what_is_not_plain() {
        let h = "ab".repeat(32);
        for bad in [
            String::new(),
            format!("{h} a.zip\n"),
            format!("{}  a.zip\n", &h[..63]),
            format!("{h}  ../a.zip\n"),
            format!("{h}  dir/a.zip\n"),
            format!("{h}  a.zip\n{h}  a.zip\n"),
            format!("{h}  a.zip\n{h}  A.ZIP\n"),
            format!("{h}  a.zip \n"),
            format!("{h}  \n"),
        ] {
            assert!(Sums::parse(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn files_are_checked_against_the_sums() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("x-v1.0.0-windows.zip");
        let man = dir.path().join(MANIFEST);
        std::fs::write(&zip, b"zip bytes").unwrap();
        std::fs::write(&man, b"{}").unwrap();
        let sums = Sums::parse(&sums_of(&[
            ("x-v1.0.0-windows.zip", b"zip bytes"),
            (MANIFEST, b"{}"),
        ]))
        .unwrap();
        let files = [
            ("x-v1.0.0-windows.zip", zip.as_path()),
            (MANIFEST, man.as_path()),
        ];
        verify_files(&sums, &files).unwrap();

        // A changed zip.
        std::fs::write(&zip, b"zip bytez").unwrap();
        let e = verify_files(&sums, &files).unwrap_err().to_string();
        assert!(e.contains("x-v1.0.0-windows.zip doesn't match"), "{e}");
        // A file the sums don't list.
        let only = Sums::parse(&sums_of(&[(MANIFEST, b"{}")])).unwrap();
        assert!(verify_files(&only, &files).is_err());
    }

    #[test]
    fn manifest_must_be_the_catalogues_release() {
        let m = br#"{"id":"foo","version":"1.0.0","file":"Foo.dll","package":"foo-v1.0.0-windows.zip"}"#;
        let ok = check_manifest(m, "foo", "1.0.0", "Foo.dll", "foo-v1.0.0-windows.zip").unwrap();
        assert_eq!(ok.file, "Foo.dll");
        for (id, ver, file, pkg) in [
            ("bar", "1.0.0", "Foo.dll", "foo-v1.0.0-windows.zip"),
            ("foo", "1.0.1", "Foo.dll", "foo-v1.0.0-windows.zip"),
            ("foo", "1.0.0", "Bar.dll", "foo-v1.0.0-windows.zip"),
            ("foo", "1.0.0", "Foo.dll", "foo-v1.0.1-windows.zip"),
        ] {
            assert!(
                check_manifest(m, id, ver, file, pkg).is_err(),
                "{id} {ver} {file} {pkg}"
            );
        }
        assert!(check_manifest(b"not json", "foo", "1.0.0", "Foo.dll", "p").is_err());
    }

    #[test]
    fn unpack_writes_the_dll_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("stage");
        let files = good(&[(NOTICE, b"notice"), ("Foo.plugin.json", b"{}")]);
        let got = unpack(zip_owned(&files), "Foo.dll", &staging).unwrap();
        assert_eq!(std::fs::read(&got.dll).unwrap(), b"MZ-foo");
        assert_eq!(std::fs::read(got.settings.unwrap()).unwrap(), b"{}");
        // Only the plugin comes out: not the license, notice or sums.
        let mut left: Vec<_> = std::fs::read_dir(&staging)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["Foo.dll", "Foo.plugin.json"]);

        let plain = unpack(zip_owned(&good(&[])), "Foo.dll", &staging).unwrap();
        assert!(plain.settings.is_none());
    }

    /// Each of these zips breaks one rule; none writes anything.
    #[test]
    fn unpack_refuses_a_zip_that_breaks_a_rule() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("stage");
        let mut cases: Vec<(&str, Files)> = Vec::new();

        // The DLL changed after the sums were made.
        let mut tampered = good(&[]);
        tampered[0].1 = b"MZ-evil".to_vec();
        cases.push(("changed dll", tampered));
        // No inner sums.
        let mut no_sums = good(&[]);
        no_sums.pop();
        cases.push(("no sums", no_sums));
        // No license.
        let mut no_license = good(&[]);
        no_license.remove(1);
        cases.push(("no license", no_license));
        // A file the sums don't cover.
        let mut uncovered = good(&[]);
        uncovered.push((NOTICE.into(), b"n".to_vec()));
        cases.push(("uncovered file", uncovered));
        // A sums line for a file that isn't there.
        let mut ghost = good(&[]);
        let extra = format!("{}  NOTICE\n", sha(b"n"));
        ghost
            .last_mut()
            .unwrap()
            .1
            .extend_from_slice(extra.as_bytes());
        cases.push(("ghost line", ghost));
        // Another DLL, and a folder.
        let mut other = good(&[("Other.dll", b"MZ")]);
        cases.push(("another dll", other.clone()));
        other = good(&[]);
        other.push(("plugins/Foo.dll".into(), b"MZ".to_vec()));
        cases.push(("a folder", other));
        // Names that climb out, or collide on Windows.
        let mut climb = good(&[]);
        climb.push(("../Foo.dll".into(), b"MZ".to_vec()));
        cases.push(("climbing", climb));
        let mut case = good(&[]);
        case.push(("license".into(), b"MIT".to_vec()));
        cases.push(("a name differing in case", case));

        for (what, files) in cases {
            let r = unpack(zip_owned(&files), "Foo.dll", &staging);
            assert!(r.is_err(), "{what} was taken");
            assert!(
                !staging.join("Foo.dll").exists(),
                "{what} wrote a file before refusing"
            );
        }
        // Not a zip, and a DLL name that isn't one.
        assert!(unpack(Cursor::new(b"nope".to_vec()), "Foo.dll", &staging).is_err());
        assert!(unpack(zip_owned(&good(&[])), "Foo.txt", &staging).is_err());
        assert!(unpack(zip_owned(&good(&[])), "../Foo.dll", &staging).is_err());
    }

    #[test]
    fn unpack_refuses_an_oversized_entry() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![0u8; (MAX_ENTRY + 1) as usize];
        let mut with_big: Files = vec![("Foo.dll".into(), big.clone())];
        with_big.push(("LICENSE".into(), b"MIT".to_vec()));
        let sums = format!("{}  Foo.dll\n{}  LICENSE\n", sha(&big), sha(b"MIT"));
        with_big.push((SUMS.into(), sums.into_bytes()));
        let r = unpack(zip_owned(&with_big), "Foo.dll", &dir.path().join("s"));
        assert!(r.unwrap_err().to_string().contains("bigger than 16 MiB"));
    }
}
