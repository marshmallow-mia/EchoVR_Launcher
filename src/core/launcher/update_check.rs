//! Is there a newer launcher? The latest GitHub release of this repository, compared
//! with the running version. Only the release page is opened; nothing is downloaded.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::core::http;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// "0.11.0"
    pub version: String,
    /// The release page on github.com.
    pub url: String,
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    html_url: String,
}

/// The API address of the latest release of `repository` (the crate's GitHub URL).
fn latest_url(repository: &str) -> Option<String> {
    let path = repository
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")?;
    Some(format!(
        "https://api.github.com/repos/{path}/releases/latest"
    ))
}

/// "v0.10.0" → [0, 10, 0]; `None` for anything else. A suffix is dropped: the releases
/// are tagged with a build number ("v0.9.5-001") that the launcher itself doesn't know,
/// so another build of the same version doesn't count as newer.
fn parse(version: &str) -> Option<Vec<u64>> {
    let v = version.trim().trim_start_matches('v');
    let main = v.split(['-', '+']).next().unwrap_or(v);
    main.split('.').map(|p| p.parse().ok()).collect()
}

/// `candidate` is a newer version than `current` (missing parts count as 0).
fn is_newer(candidate: &str, current: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (parse(candidate), parse(current)) else {
        return false;
    };
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

/// The latest release when it is newer than this launcher; `None` when this is the
/// latest (or nothing is released yet).
pub fn newer() -> Result<Option<Release>> {
    let Some(url) = latest_url(env!("CARGO_PKG_REPOSITORY")) else {
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
            404 => Ok(None),
            s if (200..300).contains(&s) => Ok(Some(resp.text().await?)),
            s => bail!("GET {url}: server responded with {s}"),
        }
    })?;
    let Some(body) = body else {
        return Ok(None);
    };
    let latest: Latest = serde_json::from_str(&body).context("the release data")?;
    let on_github = url::Url::parse(&latest.html_url)
        .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("github.com"));
    if !on_github {
        bail!("unexpected release page {}", latest.html_url);
    }
    Ok(
        is_newer(&latest.tag_name, env!("CARGO_PKG_VERSION")).then(|| Release {
            version: latest.tag_name.trim_start_matches('v').to_string(),
            url: latest.html_url,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.11.0", "0.10.0"));
        assert!(is_newer("0.10.1", "0.10.0"));
        assert!(is_newer("1.0", "0.10.0"));
        assert!(!is_newer("v0.10.0", "0.10.0"));
        assert!(!is_newer("0.10", "0.10.0"));
        assert!(!is_newer("0.9.9", "0.10.0"));
        assert!(!is_newer("nightly", "0.10.0"));
        // The releases' own tags: a build number after the version.
        assert!(!is_newer("v0.9.5-001", "0.10.0"));
        assert!(!is_newer("v0.10.0-002", "0.10.0"));
        assert!(is_newer("v0.11.0-001", "0.10.0"));
        assert!(!is_newer("v0.10.0-beta", "0.10.0"));
    }

    #[test]
    fn finds_the_api_address() {
        assert_eq!(
            latest_url("https://github.com/marshmallow-mia/EchoVR_Launcher").as_deref(),
            Some("https://api.github.com/repos/marshmallow-mia/EchoVR_Launcher/releases/latest")
        );
        assert_eq!(latest_url("https://example.com/x"), None);
    }
}
