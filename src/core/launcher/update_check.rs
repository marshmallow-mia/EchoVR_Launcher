//! Is there a newer launcher? The newest GitHub release of this repository on the channel
//! the player follows, compared with the running version. Nothing is downloaded here.
//!
//! Channels are the release tags: `v0.11.9` is main, `v0.11.10-beta.1` beta and
//! `v0.12.0-alpha.1` alpha (beta and alpha are published as prereleases, so a launcher
//! from before channels, which asks for the latest release, only ever sees main). Beta
//! gets main's releases too and alpha all three, so a beta that went to main is left
//! for main's release.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::core::http;

/// The releases the launcher updates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// The tested releases.
    #[default]
    Main,
    /// Fixes to try before they go to main.
    Beta,
    /// The newest builds.
    Alpha,
}

/// Unknown names (a channel a later launcher knows) are main.
impl<'de> Deserialize<'de> for Channel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Ok(Channel::ALL
            .into_iter()
            .find(|c| c.name() == name)
            .unwrap_or_default())
    }
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Main, Channel::Beta, Channel::Alpha];

    /// "main": as in launcher.json and the tags.
    pub fn name(self) -> &'static str {
        match self {
            Channel::Main => "main",
            Channel::Beta => "beta",
            Channel::Alpha => "alpha",
        }
    }

    /// "Main"
    pub fn label(self) -> &'static str {
        match self {
            Channel::Main => "Main",
            Channel::Beta => "Beta",
            Channel::Alpha => "Alpha",
        }
    }

    /// What following it means.
    pub fn note(self) -> &'static str {
        match self {
            Channel::Main => "The tested releases everyone gets.",
            Channel::Beta => "Fixes to try before they reach main. Can have rough edges.",
            Channel::Alpha => "The newest builds, as they are made. Things may break.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// "0.11.0", "0.11.10-beta.1"
    pub version: String,
    /// The release page on github.com.
    pub url: String,
    /// Older than the running launcher: the way back from a beta or alpha build to the
    /// channel now followed.
    pub back: bool,
}

/// A release as GitHub's API lists it.
#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
}

/// The API address of the releases of `repository` (the crate's GitHub URL), newest
/// first.
fn releases_url(repository: &str) -> Option<String> {
    let path = repository
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")?;
    Some(format!(
        "https://api.github.com/repos/{path}/releases?per_page=30"
    ))
}

/// Where in a version's life a build is: an alpha comes before the beta, the beta before
/// the release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Alpha,
    Beta,
    Release,
}

/// A launcher version, ordered like semver: 0.11.10-alpha.1 < 0.11.10-beta.1 <
/// 0.11.10-beta.2 < 0.11.10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    numbers: [u64; 3],
    stage: Stage,
    n: u64,
}

impl Version {
    /// "v0.11.10-beta.1", "0.11.9", "1.0"; `None` for anything else, also for other
    /// suffixes (the old build-number tags like "v0.9.5-001" aren't on any channel).
    fn parse(version: &str) -> Option<Version> {
        let v = version.trim().trim_start_matches('v');
        let v = v.split('+').next().unwrap_or(v);
        let (main, suffix) = match v.split_once('-') {
            Some((m, s)) => (m, Some(s)),
            None => (v, None),
        };
        let parts: Vec<u64> = main
            .split('.')
            .map(|p| p.parse().ok())
            .collect::<Option<_>>()?;
        if parts.is_empty() || parts.len() > 3 {
            return None;
        }
        let mut numbers = [0; 3];
        numbers[..parts.len()].copy_from_slice(&parts);
        let (stage, n) = match suffix {
            None => (Stage::Release, 0),
            Some(s) => {
                let (name, n) = match s.split_once('.') {
                    Some((name, n)) => (name, n.parse().ok()?),
                    None => (s, 0),
                };
                match name {
                    "beta" => (Stage::Beta, n),
                    "alpha" => (Stage::Alpha, n),
                    _ => return None,
                }
            }
        };
        Some(Version { numbers, stage, n })
    }

    fn channel(self) -> Channel {
        match self.stage {
            Stage::Release => Channel::Main,
            Stage::Beta => Channel::Beta,
            Stage::Alpha => Channel::Alpha,
        }
    }
}

/// The channel this launcher's own build came from (a beta build is beta's, whatever is
/// followed now).
pub fn running_channel() -> Channel {
    Version::parse(env!("CARGO_PKG_VERSION")).map_or(Channel::Main, Version::channel)
}

/// `candidate` is a newer version than `current` (missing parts count as 0).
#[cfg(test)]
fn is_newer(candidate: &str, current: &str) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// What to offer a launcher at `current` following `channel`: the newest release on it,
/// when it's newer, or (when the running build is from a less steady channel than the
/// one followed now) when it's another one at all.
fn pick(releases: &[GhRelease], channel: Channel, current: &str) -> Option<Release> {
    let current = Version::parse(current)?;
    let (best, r) = releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| Some((Version::parse(&r.tag_name)?, r)))
        .filter(|(v, _)| v.channel() <= channel)
        .max_by_key(|(v, _)| *v)?;
    let leaving = current.channel() > channel;
    let offer = if leaving {
        best != current
    } else {
        best > current
    };
    offer.then(|| Release {
        version: r.tag_name.trim_start_matches('v').to_string(),
        url: r.html_url.clone(),
        back: best < current,
    })
}

/// The release to update to on `channel`; `None` when this launcher is the newest there
/// (or nothing is released yet).
pub fn newer(channel: Channel) -> Result<Option<Release>> {
    // Debug builds: `ECHOVR_FAKE_LAUNCHER_UPDATE=0.99.0` (or `0.99.0-beta.1`, seen on beta
    // and alpha) says that version is out (to try the launcher's own update; with
    // `ECHOVR_UPDATE_MIRROR` to fetch it from a test server).
    if let Some(v) = std::env::var("ECHOVR_FAKE_LAUNCHER_UPDATE")
        .ok()
        .filter(|_| cfg!(debug_assertions))
    {
        let fake = GhRelease {
            tag_name: v,
            html_url: format!("{}/releases", env!("CARGO_PKG_REPOSITORY")),
            draft: false,
        };
        return Ok(pick(&[fake], channel, env!("CARGO_PKG_VERSION")));
    }
    let Some(url) = releases_url(env!("CARGO_PKG_REPOSITORY")) else {
        bail!("no GitHub repository");
    };
    let body = http::block_on(async {
        let resp = http::client()
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        match resp.status().as_u16() {
            s if (200..300).contains(&s) => Ok(resp.text().await?),
            s => bail!("GET {url}: server responded with {s}"),
        }
    })?;
    let releases: Vec<GhRelease> = serde_json::from_str(&body).context("the release data")?;
    let Some(release) = pick(&releases, channel, env!("CARGO_PKG_VERSION")) else {
        return Ok(None);
    };
    let on_github = url::Url::parse(&release.url)
        .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("github.com"));
    if !on_github {
        bail!("unexpected release page {}", release.url);
    }
    Ok(Some(release))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            html_url: format!("https://github.com/x/y/releases/tag/{tag}"),
            draft: false,
        }
    }

    fn picked(releases: &[GhRelease], channel: Channel, current: &str) -> Option<(String, bool)> {
        pick(releases, channel, current).map(|r| (r.version, r.back))
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.11.0", "0.10.0"));
        assert!(is_newer("0.10.1", "0.10.0"));
        assert!(is_newer("1.0", "0.10.0"));
        assert!(!is_newer("v0.10.0", "0.10.0"));
        assert!(!is_newer("0.10", "0.10.0"));
        assert!(!is_newer("0.9.9", "0.10.0"));
        assert!(!is_newer("nightly", "0.10.0"));
        // The old tags with a build number aren't on any channel.
        assert!(!is_newer("v0.9.5-001", "0.10.0"));
        assert!(!is_newer("v0.11.0-001", "0.10.0"));
        assert!(!is_newer("v0.10.0-beta", "0.10.0"));
        // Betas and alphas: before their release, after the one before.
        assert!(is_newer("0.11.10", "0.11.10-beta.1"));
        assert!(is_newer("0.11.10-beta.1", "0.11.9"));
        assert!(is_newer("0.11.10-beta.2", "0.11.10-beta.1"));
        assert!(is_newer("0.11.10-beta.1", "0.11.10-alpha.3"));
        assert!(!is_newer("0.11.10-beta.1", "0.11.10-beta.1"));
        assert!(!is_newer("0.11.10-gamma.1", "0.11.9"));
    }

    #[test]
    fn picks_the_newest_on_the_channel() {
        let list = [
            rel("v0.12.0-alpha.1"),
            rel("v0.11.10-beta.1"),
            rel("v0.11.9"),
            rel("v0.11.8"),
            rel("v0.9.5-001"),
        ];
        assert_eq!(
            picked(&list, Channel::Main, "0.11.8"),
            Some(("0.11.9".into(), false))
        );
        assert_eq!(
            picked(&list, Channel::Beta, "0.11.9"),
            Some(("0.11.10-beta.1".into(), false))
        );
        assert_eq!(
            picked(&list, Channel::Alpha, "0.11.9"),
            Some(("0.12.0-alpha.1".into(), false))
        );
        assert_eq!(picked(&list, Channel::Main, "0.11.9"), None);
        assert_eq!(picked(&list, Channel::Beta, "0.11.10-beta.1"), None);
        // Nothing on alpha yet: alpha gets beta's (and main's).
        assert_eq!(
            picked(&list[1..], Channel::Alpha, "0.11.9"),
            Some(("0.11.10-beta.1".into(), false))
        );
        // A beta that went to main: beta moves on to main's release.
        let later = [rel("v0.11.10"), rel("v0.11.10-beta.1"), rel("v0.11.9")];
        assert_eq!(
            picked(&later, Channel::Beta, "0.11.10-beta.1"),
            Some(("0.11.10".into(), false))
        );
    }

    #[test]
    fn leaves_drafts_and_other_tags_out() {
        let mut draft = rel("v0.11.11");
        draft.draft = true;
        let list = [draft, rel("v0.11.10-rc.1"), rel("v0.11.9")];
        assert_eq!(picked(&list, Channel::Alpha, "0.11.9"), None);
    }

    #[test]
    fn goes_back_to_a_steadier_channel() {
        let list = [rel("v0.11.10-beta.1"), rel("v0.11.9")];
        // On the beta build, main picked again: main's release, though it's older.
        assert_eq!(
            picked(&list, Channel::Main, "0.11.10-beta.1"),
            Some(("0.11.9".into(), true))
        );
        // A newer main release is just an update.
        let list = [rel("v0.11.10"), rel("v0.11.10-beta.1")];
        assert_eq!(
            picked(&list, Channel::Main, "0.11.10-beta.1"),
            Some(("0.11.10".into(), false))
        );
        // Beta from an alpha build: the newest beta or main release.
        let list = [
            rel("v0.12.0-alpha.1"),
            rel("v0.11.10-beta.1"),
            rel("v0.11.9"),
        ];
        assert_eq!(
            picked(&list, Channel::Beta, "0.12.0-alpha.1"),
            Some(("0.11.10-beta.1".into(), true))
        );
    }

    #[test]
    fn channel_names_round_trip() {
        for c in Channel::ALL {
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(json, format!("\"{}\"", c.name()));
            assert_eq!(serde_json::from_str::<Channel>(&json).unwrap(), c);
        }
        assert_eq!(
            serde_json::from_str::<Channel>("\"nightly\"").unwrap(),
            Channel::Main
        );
    }

    #[test]
    fn finds_the_api_address() {
        assert_eq!(
            releases_url("https://github.com/marshmallow-mia/EchoVR_Launcher").as_deref(),
            Some(
                "https://api.github.com/repos/marshmallow-mia/EchoVR_Launcher/releases?per_page=30"
            )
        );
        assert_eq!(releases_url("https://example.com/x"), None);
    }

    /// The live releases (after publishing): each channel finds a release of its own, and
    /// its files on the mirror match the SHA256SUMS there.
    #[test]
    #[ignore = "network: GitHub's API and release.echovr.de"]
    fn live_channels() {
        let body = http::block_on(async {
            let url = releases_url(env!("CARGO_PKG_REPOSITORY")).unwrap();
            let resp = http::client()
                .get(&url)
                .header("Accept", "application/vnd.github+json")
                .send()
                .await?;
            anyhow::Ok(resp.error_for_status()?.text().await?)
        })
        .unwrap();
        let releases: Vec<GhRelease> = serde_json::from_str(&body).unwrap();
        for channel in Channel::ALL {
            let r = pick(&releases, channel, "0.0.1").expect("a release");
            println!("{}: {}", channel.name(), r.version);
            let v = Version::parse(&r.version).unwrap();
            assert!(v.channel() <= channel);
            if channel == Channel::Main {
                assert_eq!(v.channel(), Channel::Main);
            }
            let dir = std::env::temp_dir().join(format!("evr-live-{}", channel.name()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            super::super::self_update::check_on_mirror(&r.version, &dir).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
