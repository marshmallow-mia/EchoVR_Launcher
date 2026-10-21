//! Dev codes (Advanced settings): developers' own, unpublished mods and launcher plugins.
//!
//! Each developer gets a code and a folder on release.echovr.de,
//! `launcher/dev/<code>/`, with a `mods.json` and a `plugins.json` like the published
//! catalogues and the files they list. With codes set, the launcher lists their folders'
//! entries too, over published ones with the same id (the first code's over a later one's).
//! A code is 26 base32 characters (130 bits), never logged. A folder may also have a
//! `channel.json` (`{"name": "…"}`, root's) naming the channel.
//!
//! A third-party developer's channel (restricted, or public) isn't under `launcher/dev/`:
//! it's behind the EchoVRCE sign-in, `launcher/gated/dev/<code>/` ([`crate::core::http::GATED`]).
//! A code whose folder isn't under `launcher/dev/` is looked for there, with the player's
//! EchoVRCE session; which codes are gated is kept (by a hash of the code) in
//! `dev-gated.json`.

use std::collections::BTreeSet;
use std::sync::Mutex;

/// Where the dev folders are.
pub const DEV_ROOT: &str = "https://release.echovr.de/launcher/dev/";
/// Where the gated ones are.
pub const GATED_ROOT: &str = "https://release.echovr.de/launcher/gated/dev/";
/// A code's length, without the dashes it's shown with.
pub const CODE_LEN: usize = 26;

/// The codes launcher.json has, as the settings were last loaded or saved ([`set_all`]).
static CODES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Pure: `input` as a code (dashes and spaces dropped, lowercase), if it is one: 26
/// characters of base32 (a-z, 2-7).
pub fn normalize(input: &str) -> Option<String> {
    let code: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (code.len() == CODE_LEN && code.chars().all(|c| matches!(c, 'a'..='z' | '2'..='7')))
        .then_some(code)
}

/// Pure: `code` as people type and read it, in groups of five ("abcde-fghij-...").
#[cfg(test)]
pub fn display(code: &str) -> String {
    code.as_bytes()
        .chunks(5)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Pure: `codes` normalized, without the ones that aren't codes and without repeats, in order.
pub fn normalize_all(codes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in codes.iter().filter_map(|c| normalize(c)) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Takes launcher.json's dev codes; called whenever the settings are loaded or saved.
pub fn set_all(codes: &[String]) {
    *CODES.lock().unwrap_or_else(|p| p.into_inner()) = normalize_all(codes);
}

/// The dev codes in use, in their order.
pub fn codes() -> Vec<String> {
    CODES.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Pure: how a mod's source names `code` without giving it away (launcher.json's
/// `mod_sources`, an install's `from`): `dev:<12 hex of its sha256>`, as [`cache_name`].
pub fn source_tag(code: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "dev:{}",
        &hex::encode(Sha256::digest(code.as_bytes()))[..12]
    )
}

/// `code`'s folder, with a trailing slash: under [`GATED_ROOT`] when it was found there.
pub fn folder(code: &str) -> String {
    if is_gated(code) {
        format!("{GATED_ROOT}{code}/")
    } else {
        format!("{DEV_ROOT}{code}/")
    }
}

/// The codes found behind the sign-in, by [`source_tag`] (`None`: not read yet).
static GATED: Mutex<Option<BTreeSet<String>>> = Mutex::new(None);

fn gated_path() -> std::path::PathBuf {
    crate::core::paths::data_dir().join("dev-gated.json")
}

fn with_gated<T>(f: impl FnOnce(&mut BTreeSet<String>) -> T) -> T {
    let mut held = GATED.lock().unwrap_or_else(|p| p.into_inner());
    let set = held.get_or_insert_with(|| {
        if cfg!(test) {
            return BTreeSet::new();
        }
        std::fs::read_to_string(gated_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    });
    f(set)
}

/// Whether `code`'s folder is behind the sign-in.
pub fn is_gated(code: &str) -> bool {
    with_gated(|s| s.contains(&source_tag(code)))
}

fn note_gated(code: &str, gated: bool) {
    let tag = source_tag(code);
    let changed = with_gated(|s| {
        let changed = if gated { s.insert(tag) } else { s.remove(&tag) };
        changed.then(|| serde_json::to_string(&*s).unwrap_or_default())
    });
    if let (Some(text), false) = (changed, cfg!(test)) {
        let _ = std::fs::create_dir_all(crate::core::paths::data_dir());
        let _ = std::fs::write(gated_path(), text);
    }
}

/// Pure: a dev catalogue entry's download `url` as the launcher fetches it: a relative one
/// (`files/x.dll`) inside `code`'s folder, an absolute one only when it is in there too.
/// `None` for anything else, so a dev folder can't point the launcher elsewhere.
pub fn resolve(code: &str, url: &str) -> Option<String> {
    resolve_in(&folder(code), url)
}

/// Pure: [`resolve`] in the folder at `base` (with a trailing slash).
pub fn resolve_in(base: &str, url: &str) -> Option<String> {
    let full = if url.contains("://") {
        url.to_string()
    } else {
        if !crate::core::manifest::is_safe_path(url) {
            return None;
        }
        format!("{base}{url}")
    };
    let inside = full.strip_prefix(base)?;
    (crate::core::manifest::is_safe_path(inside) && !inside.contains('?') && !inside.contains('#'))
        .then_some(full)
}

/// `text` without dev codes: every `launcher/dev/<code>` path segment and the code in use
/// (however it's written) become `***`. For logs and error messages.
pub fn redact(text: &str) -> String {
    const MARK: &str = "launcher/dev/";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(MARK) {
        out.push_str(&rest[..i + MARK.len()]);
        rest = &rest[i + MARK.len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(rest.len());
        if end > 0 {
            out.push_str("***");
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    codes().iter().fold(out, |text, code| hide(&text, code))
}

/// Pure: `text` with `code` as `***` wherever it is: in any case, and with dashes or
/// spaces between its characters (groups of five, or any other).
fn hide(text: &str, code: &str) -> String {
    let want: Vec<char> = code.chars().collect();
    let Some(&first) = want.first() else {
        return text.to_string();
    };
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied) = (0, 0);
    while i < chars.len() {
        if chars[i].1.eq_ignore_ascii_case(&first) {
            // The code from here, separators skipped; `last`: its last character.
            let (mut j, mut k, mut last) = (i, 0, i);
            while j < chars.len() && k < want.len() {
                let c = chars[j].1;
                if c.eq_ignore_ascii_case(&want[k]) {
                    (k, last) = (k + 1, j);
                } else if !(c == '-' || c == ' ') {
                    break;
                }
                j += 1;
            }
            if k == want.len() {
                let end = chars[last].0 + chars[last].1.len_utf8();
                out.push_str(&text[copied..chars[i].0]);
                out.push_str("***");
                (copied, i) = (end, last + 1);
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// What a code's dev folder held when last read (for Advanced settings).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    /// Not looked at.
    #[default]
    Off,
    /// Not read yet.
    Checking,
    /// No folder for this code.
    NotFound,
    /// A channel behind the EchoVRCE sign-in, and there's no sign-in (or it ran out).
    SignIn,
    /// A channel behind the sign-in this EchoVRCE account isn't allowed on (or none has this
    /// code).
    NotAllowed,
    /// The folder's catalogues: how many mods and plugins they list, and the channel's name
    /// (its `channel.json`).
    Found {
        mods: usize,
        plugins: usize,
        name: Option<String>,
    },
    /// The server couldn't be asked (the message, without the code).
    Failed(String),
}

impl Status {
    /// The line Advanced settings shows.
    pub fn line(&self) -> String {
        match self {
            Status::Off => "Not checked yet.".into(),
            Status::Checking => "Checking the code...".into(),
            Status::NotFound => "No dev folder has this code.".into(),
            Status::SignIn => {
                "This dev channel needs your EchoVRCE sign-in: sign in, then check the code again."
                    .into()
            }
            Status::NotAllowed => "Your EchoVRCE account isn't allowed on this dev channel (its \
                 developer adds you in their access.json), or no channel has this code."
                .into(),
            Status::Found {
                mods,
                plugins,
                name,
            } => format!(
                "{}: {mods} {}, {plugins} {}.",
                name.as_deref().unwrap_or("Dev folder"),
                if *mods == 1 { "mod" } else { "mods" },
                if *plugins == 1 { "plugin" } else { "plugins" }
            ),
            Status::Failed(e) => format!("Couldn't check the code: {e}"),
        }
    }
}

/// Pure: the file a dev catalogue of `code` is kept in, `<name>-dev-<12 hex>.json` (a hash
/// of the code, so another code never reads it and the file name doesn't give it away).
pub fn cache_name(code: &str, name: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(code.as_bytes()));
    format!("{name}-dev-{}.json", &hash[..12])
}

/// The dev catalogue `name` of every code in use, in their order: each fresh (kept for
/// [`kept`]) or, when the server can't be asked, the one kept last. A code whose folder has
/// no such catalogue is left out.
pub fn load(name: &str) -> Vec<(String, String)> {
    codes()
        .into_iter()
        .filter_map(|code| load_one(&code, name))
        .collect()
}

fn load_one(code: &str, name: &str) -> Option<(String, String)> {
    let path = crate::core::paths::data_dir().join(cache_name(code, name));
    match fetch(code, &format!("{name}.json")) {
        Ok(Some(text)) => {
            let _ = std::fs::create_dir_all(crate::core::paths::data_dir());
            let _ = std::fs::write(&path, &text);
            Some((code.to_string(), text))
        }
        Ok(None) => {
            let _ = std::fs::remove_file(&path);
            None
        }
        // Signed out, or not allowed any more: nothing from it (what's installed stays).
        Err(e) if matches!(crate::core::http::status_of(&e), Some(401 | 403)) => {
            tracing::info!("dev {name} catalogue: {}", failed(&e).line());
            None
        }
        Err(e) => {
            tracing::info!("dev {name} catalogue unavailable ({e:#}); using the last one kept");
            kept_one(code, name)
        }
    }
}

/// The dev catalogue `name` of every code in use as kept last (no network), in their order.
pub fn kept(name: &str) -> Vec<(String, String)> {
    codes()
        .into_iter()
        .filter_map(|code| kept_one(&code, name))
        .collect()
}

fn kept_one(code: &str, name: &str) -> Option<(String, String)> {
    if cfg!(test) {
        return None;
    }
    let path = crate::core::paths::data_dir().join(cache_name(code, name));
    std::fs::read_to_string(path)
        .ok()
        .map(|t| (code.to_string(), t))
}

/// The channel name `code`'s folder gives itself (`channel.json`), as kept last (no
/// network; [`check`] keeps it).
pub fn channel_name(code: &str) -> Option<String> {
    kept_one(code, "channel").and_then(|(_, t)| parse_channel(&t))
}

/// Pure: the `name` in a `channel.json`.
fn parse_channel(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let name = v.get("name")?.as_str()?.trim();
    (!name.is_empty() && name.len() <= 64).then(|| name.to_string())
}

/// Fetches `code`'s catalogue `name`: `Ok(None)` when its folder has none (or there's no
/// such folder). Under `launcher/dev/`, else behind the sign-in (a 401 or 403 there is an
/// error [`crate::core::http::status_of`] tells). Errors never name the code.
pub fn fetch(code: &str, name: &str) -> anyhow::Result<Option<String>> {
    let get = |root: &str| crate::core::http::get_text_opt(&format!("{root}{code}/{name}"));
    let hide = |e: anyhow::Error| {
        let status = crate::core::http::status_of(&e);
        let e = anyhow::anyhow!(redact(&format!("{e:#}")));
        match status {
            Some(s) => e.context(crate::core::http::HttpStatus(s)),
            None => e,
        }
    };
    let (first, second, gated_first) = if is_gated(code) {
        (GATED_ROOT, DEV_ROOT, true)
    } else {
        (DEV_ROOT, GATED_ROOT, false)
    };
    match get(first).map_err(hide)? {
        Some(t) => Ok(Some(t)),
        None => {
            let t = get(second).map_err(hide)?;
            if t.is_some() {
                note_gated(code, !gated_first);
            }
            Ok(t)
        }
    }
}

/// Pure: the status an error from [`fetch`] means.
fn failed(e: &anyhow::Error) -> Status {
    match crate::core::http::status_of(e) {
        Some(401) => Status::SignIn,
        Some(403) => Status::NotAllowed,
        _ => Status::Failed(e.to_string()),
    }
}

/// Reads `code`'s folder: how many mods and plugins it lists (the Advanced settings check).
pub fn check(code: &str) -> Status {
    let mods = match fetch(code, "mods.json") {
        Ok(t) => t,
        Err(e) => return failed(&e),
    };
    let plugins = match fetch(code, "plugins.json") {
        Ok(t) => t,
        Err(e) => return failed(&e),
    };
    if mods.is_none() && plugins.is_none() {
        return Status::NotFound;
    }
    let name = load_one(code, "channel").and_then(|(_, t)| parse_channel(&t));
    Status::Found {
        name,
        mods: mods
            .and_then(|t| super::mods::ModCatalog::parse_dev(&t, code).ok())
            .map_or(0, |c| c.mods.len()),
        plugins: plugins
            .and_then(|t| super::plugins::PluginCatalog::parse_dev(&t, code).ok())
            .map_or(0, |c| c.plugins.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "abcdefghijklmnopqrstuvwxyz";

    #[test]
    fn takes_codes_as_typed() {
        assert_eq!(normalize(CODE).as_deref(), Some(CODE));
        assert_eq!(normalize(&display(CODE)).as_deref(), Some(CODE));
        assert_eq!(
            normalize(" ABCDE-FGHIJ klmno-pqrst-uvwxy-z ").as_deref(),
            Some(CODE)
        );
        assert_eq!(display(CODE), "abcde-fghij-klmno-pqrst-uvwxy-z");
        // Wrong length or characters outside base32 (0, 1, 8, 9, symbols).
        for bad in [
            "",
            "abc",
            &CODE[1..],
            &format!("{CODE}a"),
            "abcdefghijklmnopqrstuvwxy0",
            "abcdefghijklmnopqrstuvwxy/",
        ] {
            assert_eq!(normalize(bad), None, "{bad}");
        }
    }

    #[test]
    fn dev_entries_download_from_their_own_folder_only() {
        let base = format!("{DEV_ROOT}{CODE}/");
        assert_eq!(
            resolve(CODE, "files/NvrTest.dll").as_deref(),
            Some(format!("{base}files/NvrTest.dll").as_str())
        );
        assert_eq!(
            resolve(CODE, &format!("{base}files/x.zip")).as_deref(),
            Some(format!("{base}files/x.zip").as_str())
        );
        for bad in [
            "../other/files/x.dll".to_string(),
            "/launcher/mods.json".to_string(),
            "https://release.echovr.de/launcher/mods/NvrGpuRating-1.0.0.dll".to_string(),
            format!("{DEV_ROOT}zzzzzzzzzzzzzzzzzzzzzzzzzz/files/x.dll"),
            format!("{base}../mods.json"),
            format!("{base}files/x.dll?y=1"),
            "https://evil.example/x.dll".to_string(),
            format!("http://release.echovr.de/launcher/dev/{CODE}/files/x.dll"),
        ] {
            assert_eq!(resolve(CODE, &bad), None, "{bad}");
        }
    }

    #[test]
    fn keeps_codes_out_of_logs() {
        let url = format!("GET {DEV_ROOT}{CODE}/mods.json: server responded with 500");
        assert_eq!(
            redact(&url),
            "GET https://release.echovr.de/launcher/dev/***/mods.json: server responded with 500"
        );
        assert_eq!(
            redact("launcher/dev/abc-def/files/x.dll and /launcher/dev/"),
            "launcher/dev/***/files/x.dll and /launcher/dev/"
        );
        assert_eq!(redact("nothing here"), "nothing here");
    }

    #[test]
    fn hides_the_code_however_it_is_written() {
        for written in [
            CODE.to_string(),
            display(CODE),
            CODE.to_uppercase(),
            "abcde-fghij-klmno-pqrst-uvwxyz".into(),
            "abcd efgh ijkl mnop qrst uvwx yz".into(),
        ] {
            assert_eq!(
                hide(&format!("code={written}, again: {written}."), CODE),
                "code=***, again: ***.",
                "{written}"
            );
        }
        // Part of it, or broken by anything else, stays.
        for kept in ["abcdefghijklm", "abcde_fghij-klmno-pqrst-uvwxy-z", "näbcdé"] {
            assert_eq!(hide(kept, CODE), kept);
        }
        assert_eq!(hide("x", ""), "x");
    }

    #[test]
    fn says_what_the_folder_holds() {
        assert_eq!(
            Status::Found {
                mods: 1,
                plugins: 2,
                name: None
            }
            .line(),
            "Dev folder: 1 mod, 2 plugins."
        );
        assert_eq!(
            Status::Found {
                mods: 0,
                plugins: 1,
                name: Some("cat's channel".into())
            }
            .line(),
            "cat's channel: 0 mods, 1 plugin."
        );
    }

    #[test]
    fn several_codes() {
        let other = "bbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
        assert_eq!(
            normalize_all(&[display(CODE), "junk".into(), CODE.into(), other.clone()]),
            vec![CODE.to_string(), other.clone()]
        );
        assert_eq!(source_tag(CODE).len(), 16);
        assert_ne!(source_tag(CODE), source_tag(&other));
        assert!(source_tag(CODE).starts_with("dev:"));
        assert_eq!(
            parse_channel(r#"{"name": " cat "}"#).as_deref(),
            Some("cat")
        );
        assert_eq!(parse_channel(r#"{"name": ""}"#), None);
        assert_eq!(parse_channel("nope"), None);
    }

    #[test]
    fn hides_every_code_in_use() {
        let other = "bbbbbbbbbbbbbbbbbbbbbbbbbb";
        set_all(&[CODE.into(), other.into()]);
        assert_eq!(
            redact(&format!("{CODE} and {}", display(other))),
            "*** and ***"
        );
        set_all(&[]);
    }
}
