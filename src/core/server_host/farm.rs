//! A server's own game folder (`root`), built before each start from the installed game:
//! the big folders (`content`, `sourcedb`, `_data`, …) are links into it (symlinks; on
//! Windows directory junctions, which need no admin rights), while `bin/win10` is a copy of
//! its files and plugins, so a game or mod update never meets a file a server holds, and a
//! server takes a new nEVR at its next start. `_local` is the server's own: its
//! `config.yaml`, `config.json` and sign-in.
//!
//! nEVR ignores `-port` (5.4.2): the broadcaster binds the port in
//! `netconfig_dedicatedserver.json`, so each server has its own copy of that file, with its
//! port. After evr-docker's farm (Echo-VR-Server-on-Docker).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// The executable's folder in a game root (`bin/win10`, joined by parts: on Windows a
/// path with `/` in it breaks `mklink`).
pub fn bin_in(root: &Path) -> PathBuf {
    root.join("bin").join("win10")
}
/// The file that sets the server's port.
pub const NETCONFIG: &str = "sourcedb/rad15/json/r14/config/netconfig_dedicatedserver.json";
/// Folders in `bin/win10` the server writes to: its own, never links.
const BIN_OWN: &[&str] = &["logs", "_temp", "plugin_logs"];
/// Copied (with what is in them) rather than linked: they are loaded and held. The game's
/// plugins folder ([`crate::core::launcher::mods::plugins_in`]) is one of them while it is
/// in `bin/win10`.
const BIN_COPIED: &[&str] = &["plugins"];
/// Never taken from the game: nEVR refuses to start beside a `dbgcore.dll`.
const BIN_SKIP: &[&str] = &["dbgcore.dll"];
/// The game root's entries that aren't linked.
const ROOT_SKIP: &[&str] = &["bin", "_local", "_temp"];
/// Where the farm notes the game it was built from.
const MARKER: &str = ".farm-from";

/// A server's game folder, to build.
pub struct Farm<'a> {
    /// The installed game's `ready-at-dawn-echo-arena`.
    pub game: &'a Path,
    /// The server's own.
    pub root: &'a Path,
    pub port: u16,
    /// `_local/config.yaml`.
    pub config_yaml: &'a str,
    /// `_local/config.json`.
    pub config_json: &'a str,
}

impl Farm<'_> {
    /// The server's executable folder.
    pub fn bin(&self) -> PathBuf {
        bin_in(self.root)
    }

    pub fn local(&self) -> PathBuf {
        self.root.join("_local")
    }

    /// Builds or refreshes it; keeps what the server wrote itself (its sign-in, logs).
    pub fn build(&self) -> Result<()> {
        if !bin_in(self.game).is_dir() {
            bail!(
                "{} has no bin/win10: is the game installed?",
                self.game.display()
            );
        }
        std::fs::create_dir_all(self.root)
            .with_context(|| format!("create {}", self.root.display()))?;
        // Another game than last time: its links go (they are made again).
        let from = self.game.to_string_lossy().to_string();
        let marker = self.root.join(MARKER);
        if std::fs::read_to_string(&marker).is_ok_and(|t| t != from) {
            remove_links(self.root)?;
        }
        for d in ["_local/r14logs", "_temp"] {
            real_dir(&self.root.join(d))?;
        }
        link_dir(self.game, self.root, ROOT_SKIP)?;
        link_dir(&self.game.join("bin"), &self.root.join("bin"), &["win10"])?;
        self.sync_bin()?;
        self.write_netconfig()?;
        let local = self.local();
        write_atomic(&local.join("config.yaml"), self.config_yaml)?;
        write_atomic(&local.join("config.json"), self.config_json)?;
        std::fs::write(&marker, from)?;
        Ok(())
    }

    /// `bin/win10`: its files and plugins copied, its other folders linked, the server's
    /// own folders real.
    fn sync_bin(&self) -> Result<()> {
        let (src, dst) = (bin_in(self.game), self.bin());
        real_dir(&dst)?;
        for d in BIN_OWN {
            real_dir(&dst.join(d))?;
        }
        let mut wanted = Vec::new();
        for e in std::fs::read_dir(&src).with_context(|| format!("read {}", src.display()))? {
            let e = e?;
            let name = e.file_name().to_string_lossy().to_string();
            let skip = BIN_SKIP.iter().any(|n| n.eq_ignore_ascii_case(&name));
            if BIN_OWN.contains(&name.as_str()) || skip {
                continue;
            }
            wanted.push(name.clone());
            let (s, d) = (e.path(), dst.join(&name));
            let ft = e.file_type()?;
            if ft.is_dir() && BIN_COPIED.contains(&name.as_str()) {
                if is_link(&d) {
                    remove_link(&d)?;
                }
                copy_tree(&s, &d)?;
            } else if ft.is_dir() {
                link(&s, &d, true)?;
            } else {
                if is_link(&d) {
                    remove_link(&d)?;
                }
                copy_if_changed(&s, &d)?;
            }
        }
        // What the game no longer has (a mod's DLL removed, an update's old file).
        for e in std::fs::read_dir(&dst)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().to_string();
            if wanted.contains(&name) || BIN_OWN.contains(&name.as_str()) {
                continue;
            }
            let p = e.path();
            if is_link(&p) {
                remove_link(&p)?;
            } else if e.file_type()?.is_dir() {
                std::fs::remove_dir_all(&p)?;
            } else {
                std::fs::remove_file(&p)?;
            }
        }
        Ok(())
    }

    /// The server's own `netconfig_dedicatedserver.json` with its port (and no retries),
    /// the folders on the way to it real ones linking the rest.
    fn write_netconfig(&self) -> Result<()> {
        let rel = Path::new(NETCONFIG);
        let parts: Vec<_> = rel.parent().unwrap_or(rel).components().collect();
        for i in 0..parts.len() {
            let here: PathBuf = parts[..=i].iter().collect();
            let next = parts
                .get(i + 1)
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_else(|| {
                    rel.file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default()
                });
            link_dir(
                &self.game.join(&here),
                &self.root.join(&here),
                &[next.as_str()],
            )?;
        }
        let orig = std::fs::read_to_string(self.game.join(rel))
            .with_context(|| format!("read the game's {NETCONFIG}"))?;
        let text = with_port(&orig, self.port)
            .with_context(|| format!("no \"port\" in the game's {NETCONFIG}"))?;
        write_atomic(&self.root.join(rel), &text)
    }
}

/// Pure: the netconfig `text` (JSON with comments, so edited as text) with `port` and no
/// retries; `None` without a `"port"` key.
pub fn with_port(text: &str, port: u16) -> Option<String> {
    let out = replace_number(text, "\"port\"", &port.to_string())?;
    Some(replace_number(&out, "\"retries\"", "0").unwrap_or(out))
}

/// Pure: every `<key> : <digits>` in `text` with `value` for the digits; `None` when the key
/// isn't there.
fn replace_number(text: &str, key: &str, value: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut found = false;
    while let Some(i) = rest.find(key) {
        let after = &rest[i + key.len()..];
        let ws = after.len() - after.trim_start().len();
        let colon = after[ws..].strip_prefix(':');
        let Some(colon) = colon else {
            out.push_str(&rest[..i + key.len()]);
            rest = after;
            continue;
        };
        let ws2 = colon.len() - colon.trim_start().len();
        let digits = colon[ws2..].bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            out.push_str(&rest[..i + key.len()]);
            rest = after;
            continue;
        }
        let head = i + key.len() + ws + 1 + ws2;
        out.push_str(&rest[..head]);
        out.push_str(value);
        rest = &rest[head + digits..];
        found = true;
    }
    out.push_str(rest);
    found.then_some(out)
}

// ---- links and copies ----

/// Whether `p` is a link (a symlink or, on Windows, a junction).
fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// Removes the link `p` (not what it points at).
fn remove_link(p: &Path) -> Result<()> {
    // A junction (and a directory symlink) on Windows goes with remove_dir.
    let r = std::fs::remove_file(p).or_else(|_| std::fs::remove_dir(p));
    r.with_context(|| format!("remove the link {}", p.display()))
}

/// Every link in `dir` and below (not following them): the farm's links, before it is
/// built from another game.
fn remove_links(dir: &Path) -> Result<()> {
    for e in std::fs::read_dir(dir)?.flatten() {
        let p = e.path();
        if is_link(&p) {
            remove_link(&p)?;
        } else if e.file_type().is_ok_and(|t| t.is_dir()) {
            remove_links(&p)?;
        }
    }
    Ok(())
}

/// `p` as a real folder (a link there goes first).
fn real_dir(p: &Path) -> Result<()> {
    if is_link(p) {
        remove_link(p)?;
    } else if p.is_file() {
        bail!("{} is a file, not a folder", p.display());
    }
    std::fs::create_dir_all(p).with_context(|| format!("create {}", p.display()))
}

/// `dst` a real folder of links to what `src` has, but `skip`; a real entry of the
/// server's own in `dst` stays, and links to what `src` no longer has go.
fn link_dir(src: &Path, dst: &Path, skip: &[&str]) -> Result<()> {
    real_dir(dst)?;
    let mut wanted = Vec::new();
    if let Ok(entries) = std::fs::read_dir(src) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if skip.contains(&name.as_str()) {
                continue;
            }
            let target = dst.join(&name);
            wanted.push(name);
            if target.exists() && !is_link(&target) {
                continue;
            }
            link(&e.path(), &target, e.file_type()?.is_dir())?;
        }
    }
    for e in std::fs::read_dir(dst)?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !wanted.contains(&name) && is_link(&e.path()) {
            remove_link(&e.path())?;
        }
    }
    Ok(())
}

/// A link at `link` to `target` (kept when it is one already).
fn link(target: &Path, link: &Path, dir: bool) -> Result<()> {
    if is_link(link) {
        match std::fs::read_link(link) {
            Ok(t) if same_target(&t, target) => return Ok(()),
            _ => remove_link(link)?,
        }
    } else if link.exists() {
        return Ok(());
    }
    make_link(target, link, dir)
        .with_context(|| format!("link {} to {}", link.display(), target.display()))
}

/// Pure: whether a link's target (as read back) is `want`. A junction reads back as
/// `\\?\C:\…`.
fn same_target(read: &Path, want: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        s.trim_start_matches("//?/")
            .trim_start_matches("/??/")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    norm(read) == norm(want)
}

#[cfg(unix)]
fn make_link(target: &Path, link: &Path, _dir: bool) -> Result<()> {
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[cfg(windows)]
fn make_link(target: &Path, link: &Path, dir: bool) -> Result<()> {
    if !dir {
        // A file: a hard link on the same drive, else a copy.
        if std::fs::hard_link(target, link).is_ok() {
            return Ok(());
        }
        std::fs::copy(target, link)?;
        return Ok(());
    }
    // A directory junction: no admin rights or developer mode needed. mklink takes only
    // backslashes (`bin/win10` would make it fail).
    let win = |p: &Path| p.to_string_lossy().replace('/', "\\");
    let out = crate::core::process::command("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(win(link))
        .arg(win(target))
        .output()
        .context("mklink")?;
    if !out.status.success() {
        bail!(
            "mklink /J failed: {} {}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Copies `src` to `dst` unless `dst` has its size and time already.
fn copy_if_changed(src: &Path, dst: &Path) -> Result<()> {
    let m = std::fs::metadata(src)?;
    let same = std::fs::metadata(dst)
        .is_ok_and(|d| d.len() == m.len() && d.modified().ok() == m.modified().ok());
    if same {
        return Ok(());
    }
    let part = dst.with_extension("farm-part");
    std::fs::copy(src, &part).with_context(|| format!("copy {}", src.display()))?;
    if let Ok(t) = m.modified() {
        let f = std::fs::OpenOptions::new().write(true).open(&part)?;
        f.set_modified(t)?;
    }
    if dst.exists() {
        std::fs::remove_file(dst).with_context(|| format!("replace {}", dst.display()))?;
    }
    std::fs::rename(&part, dst).with_context(|| format!("replace {}", dst.display()))?;
    Ok(())
}

/// `dst` as a copy of the folder `src` (only what changed is copied; what `src` no longer
/// has goes).
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    let mut wanted = Vec::new();
    for e in std::fs::read_dir(src)?.flatten() {
        let name = e.file_name();
        wanted.push(name.clone());
        let (s, d) = (e.path(), dst.join(&name));
        if e.file_type()?.is_dir() {
            copy_tree(&s, &d)?;
        } else {
            copy_if_changed(&s, &d)?;
        }
    }
    for e in std::fs::read_dir(dst)?.flatten() {
        if wanted.contains(&e.file_name()) {
            continue;
        }
        let p = e.path();
        if e.file_type()?.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// Writes `text` to `path` unless it holds it already (through a `.part` and a rename).
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if is_link(path) {
        remove_link(path)?;
    }
    if std::fs::read_to_string(path).is_ok_and(|t| t == text) {
        return Ok(());
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let part = path.with_extension("part");
    std::fs::write(&part, text).with_context(|| format!("write {}", part.display()))?;
    if path.exists() {
        // Windows won't rename over a hidden file.
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&part, path).with_context(|| format!("replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET: &str = "{\n  // the dedicated server\n  \"port\" : 6792,\n  \"retries\": 3,\n  \"other_port\": 1\n}\n";

    #[test]
    fn sets_the_port_as_text() {
        let out = with_port(NET, 6800).unwrap();
        assert!(out.contains("\"port\" : 6800,"));
        assert!(out.contains("\"retries\": 0,"));
        assert!(out.contains("\"other_port\": 1"));
        assert!(out.contains("// the dedicated server"));
        assert_eq!(with_port("{\"x\": 1}", 1), None);
    }

    fn game(dir: &Path) -> PathBuf {
        let g = dir.join("game");
        for d in [
            "bin/win10/plugins/sub",
            "bin/win10/content/pack",
            "bin/win10/logs",
            "_data",
            "sourcedb/rad15/json/r14/config",
            "sourcedb/rad15/other",
            "_local",
        ] {
            std::fs::create_dir_all(g.join(d)).unwrap();
        }
        for (f, t) in [
            ("bin/win10/echovr.exe", "exe"),
            ("bin/win10/BugSplat64.dll", "nevr"),
            ("bin/win10/dbgcore.dll", "old"),
            ("bin/win10/plugins/A.dll", "a"),
            ("bin/win10/plugins/sub/s.json", "s"),
            ("_local/config.yaml", "player's"),
            (NETCONFIG, NET),
            ("sourcedb/rad15/json/r14/config/other.json", "{}"),
        ] {
            std::fs::write(g.join(f), t).unwrap();
        }
        g
    }

    #[test]
    fn builds_a_servers_own_game_folder() {
        let dir = tempfile::tempdir().unwrap();
        let g = game(dir.path());
        let root = dir.path().join("root");
        let farm = Farm {
            game: &g,
            root: &root,
            port: 6801,
            config_yaml: "services: {}\n",
            config_json: "{}",
        };
        farm.build().unwrap();
        let bin = bin_in(&root);
        assert_eq!(
            std::fs::read_to_string(bin.join("echovr.exe")).unwrap(),
            "exe"
        );
        assert!(!is_link(&bin.join("echovr.exe")));
        assert!(!bin.join("dbgcore.dll").exists());
        let plugins = crate::core::launcher::mods::plugins_in(&bin);
        assert!(!is_link(&plugins));
        assert!(plugins.join("sub/s.json").is_file());
        assert!(is_link(&bin.join("content")));
        assert!(bin.join("logs").is_dir() && !is_link(&bin.join("logs")));
        assert!(is_link(&root.join("_data")));
        assert!(!is_link(&root.join("sourcedb")));
        assert!(is_link(&root.join("sourcedb/rad15/other")));
        // A file's link: a hard link on Windows (no symlink rights needed).
        let other = root.join("sourcedb/rad15/json/r14/config/other.json");
        assert!(is_link(&other) || (cfg!(windows) && other.is_file()));
        let net = std::fs::read_to_string(root.join(NETCONFIG)).unwrap();
        assert!(net.contains("\"port\" : 6801"));
        assert_eq!(
            std::fs::read_to_string(root.join("_local/config.yaml")).unwrap(),
            "services: {}\n"
        );
        assert!(root.join("_local/r14logs").is_dir());

        // An update: a plugin gone, nEVR changed; the server's sign-in stays.
        std::fs::write(root.join("_local/.credentials.json"), "{}").unwrap();
        std::fs::remove_file(g.join("bin/win10/plugins/A.dll")).unwrap();
        std::fs::write(g.join("bin/win10/BugSplat64.dll"), "nevr 2").unwrap();
        farm.build().unwrap();
        assert!(!plugins.join("A.dll").exists());
        assert_eq!(
            std::fs::read_to_string(bin.join("BugSplat64.dll")).unwrap(),
            "nevr 2"
        );
        assert!(root.join("_local/.credentials.json").is_file());
        // The game itself never changed.
        assert_eq!(
            std::fs::read_to_string(g.join("_local/config.yaml")).unwrap(),
            "player's"
        );
        assert_eq!(std::fs::read_to_string(g.join(NETCONFIG)).unwrap(), NET);
    }
}
