//! A dev code (Advanced settings): a developer's own, unpublished mods and launcher plugins.
//!
//! Each developer gets a code and a folder on release.echovr.de,
//! `launcher/dev/<code>/`, with a `mods.json` and a `plugins.json` like the published
//! catalogues and the files they list. With the code set, the
//! launcher lists that folder's entries too, over published ones with the same id. The
//! code is the only thing that keeps the folder private: 26 base32 characters (130 bits),
//! never logged.

use std::sync::Mutex;

/// Where the dev folders are.
pub const DEV_ROOT: &str = "https://release.echovr.de/launcher/dev/";
/// A code's length, without the dashes it's shown with.
pub const CODE_LEN: usize = 26;

/// The code launcher.json has, as the settings were last loaded or saved ([`set`]).
static CODE: Mutex<Option<String>> = Mutex::new(None);

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
pub fn display(code: &str) -> String {
    code.as_bytes()
        .chunks(5)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Takes launcher.json's dev code; called whenever the settings are loaded or saved.
pub fn set(code: Option<&str>) {
    *CODE.lock().unwrap_or_else(|p| p.into_inner()) = code.and_then(normalize);
}

/// The dev code in use, if any.
pub fn current() -> Option<String> {
    CODE.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Pure: `code`'s folder, with a trailing slash.
pub fn folder(code: &str) -> String {
    format!("{DEV_ROOT}{code}/")
}

/// Pure: where `code`'s catalogue `name` ("mods.json", "plugins.json") is.
pub fn catalog_url(code: &str, name: &str) -> String {
    format!("{}{name}", folder(code))
}

/// Pure: a dev catalogue entry's download `url` as the launcher fetches it: a relative one
/// (`files/x.dll`) inside `code`'s folder, an absolute one only when it is in there too.
/// `None` for anything else, so a dev folder can't point the launcher elsewhere.
pub fn resolve(code: &str, url: &str) -> Option<String> {
    let base = folder(code);
    let full = if url.contains("://") {
        url.to_string()
    } else {
        if !crate::core::manifest::is_safe_path(url) {
            return None;
        }
        format!("{base}{url}")
    };
    let inside = full.strip_prefix(&base)?;
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
    match current() {
        Some(code) => hide(&out, &code),
        None => out,
    }
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

/// What the dev folder of the code in use held when last read (for Advanced settings).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    /// No code set.
    #[default]
    Off,
    /// Not read yet.
    Checking,
    /// No folder for this code.
    NotFound,
    /// The folder's catalogues: how many mods and plugins they list.
    Found { mods: usize, plugins: usize },
    /// The server couldn't be asked (the message, without the code).
    Failed(String),
}

impl Status {
    /// The line Advanced settings shows.
    pub fn line(&self) -> String {
        match self {
            Status::Off => "No dev code: only published mods and plugins.".into(),
            Status::Checking => "Checking the code...".into(),
            Status::NotFound => "No dev folder has this code.".into(),
            Status::Found { mods, plugins } => format!(
                "Your dev folder: {mods} {}, {plugins} {}.",
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

/// The dev catalogue `name` of the code in use, fresh (kept for [`kept`]) or, when the
/// server can't be asked, the one kept last: `None` without a code or without such a
/// catalogue in its folder.
pub fn load(name: &str) -> Option<(String, String)> {
    let code = current()?;
    let path = crate::core::paths::data_dir().join(cache_name(&code, name));
    match fetch(&code, &format!("{name}.json")) {
        Ok(Some(text)) => {
            let _ = std::fs::create_dir_all(crate::core::paths::data_dir());
            let _ = std::fs::write(&path, &text);
            Some((code, text))
        }
        Ok(None) => {
            let _ = std::fs::remove_file(&path);
            None
        }
        Err(e) => {
            tracing::info!("dev {name} catalogue unavailable ({e:#}); using the last one kept");
            kept(name)
        }
    }
}

/// The dev catalogue `name` of the code in use as kept last (no network).
pub fn kept(name: &str) -> Option<(String, String)> {
    if cfg!(test) {
        return None;
    }
    let code = current()?;
    let path = crate::core::paths::data_dir().join(cache_name(&code, name));
    std::fs::read_to_string(path).ok().map(|t| (code, t))
}

/// Fetches `code`'s catalogue `name`: `Ok(None)` when its folder has none (or there's no
/// such folder). Errors never name the code.
pub fn fetch(code: &str, name: &str) -> anyhow::Result<Option<String>> {
    crate::core::http::get_text_opt(&catalog_url(code, name))
        .map_err(|e| anyhow::anyhow!(redact(&format!("{e:#}"))))
}

/// Reads `code`'s folder: how many mods and plugins it lists (the Advanced settings check).
pub fn check(code: &str) -> Status {
    let mods = match fetch(code, "mods.json") {
        Ok(t) => t,
        Err(e) => return Status::Failed(e.to_string()),
    };
    let plugins = match fetch(code, "plugins.json") {
        Ok(t) => t,
        Err(e) => return Status::Failed(e.to_string()),
    };
    if mods.is_none() && plugins.is_none() {
        return Status::NotFound;
    }
    Status::Found {
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
                plugins: 2
            }
            .line(),
            "Your dev folder: 1 mod, 2 plugins."
        );
        assert_eq!(Status::NotFound.line(), "No dev folder has this code.");
    }
}
