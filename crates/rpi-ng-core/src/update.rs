//! Update check against the project's GitHub releases.
//!
//! The check is a single GET to the releases API — nothing else is sent (no
//! telemetry, no identifiers; the request carries only the app's User-Agent).
//! Versions come from the release tag (`v0.0.2`) and are compared with a
//! conservative numeric parser: anything that does not parse cleanly is
//! treated as "not newer" so a malformed tag can never nag the user.

use serde::Deserialize;

/// Releases API endpoint for the latest published release.
pub const RELEASES_API_URL: &str =
    "https://api.github.com/repos/AldySan/Remote-Package-Installer-NG/releases/latest";

/// Browser-facing releases page (used when the API omits `html_url`).
pub const RELEASES_PAGE: &str = "https://github.com/AldySan/Remote-Package-Installer-NG/releases";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// Normalized version (`v` prefix stripped), e.g. `0.0.3`.
    pub version: String,
    /// The release tag exactly as published, e.g. `v0.0.3`.
    pub tag: String,
    /// Human-facing release page URL.
    pub url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("unexpected API status {status}: {message}")]
    Api { status: u16, message: String },
    #[error("GitHub API rate limit exceeded — try again later")]
    RateLimited,
    #[error("malformed release response: {0}")]
    Malformed(&'static str),
}

/// A client suitable for the update check (User-Agent is mandatory for the
/// GitHub API; without one it answers 403).
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(format!("Remote-Package-Installer-NG/{}", crate::VERSION))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("TLS client with default roots builds")
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
}

/// Fetch the latest release and report whether it is newer than `current`.
///
/// Returns `Ok(None)` when the endpoint reports no releases yet (404).
pub async fn check_latest(
    client: &reqwest::Client,
    api_url: &str,
    current: &str,
) -> Result<Option<UpdateInfo>, UpdateError> {
    let resp = client
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return match status.as_u16() {
            404 => Ok(None),
            403 | 429 => Err(UpdateError::RateLimited),
            other => Err(UpdateError::Api {
                status: other,
                message: resp
                    .text()
                    .await
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect(),
            }),
        };
    }
    let release: GithubRelease = resp
        .json()
        .await
        .map_err(|_| UpdateError::Malformed("expected a release object"))?;
    if release.tag_name.trim().is_empty() {
        return Err(UpdateError::Malformed("empty tag_name"));
    }
    let tag = release.tag_name.trim().to_string();
    let version = normalize_version(&tag);
    let url = if release.html_url.is_empty() {
        format!("{RELEASES_PAGE}/tag/{tag}")
    } else {
        release.html_url
    };
    if is_newer(current, &version) {
        Ok(Some(UpdateInfo { version, tag, url }))
    } else {
        Ok(None)
    }
}

/// Strip the conventional `v`/`V` prefix from a version or tag.
pub fn normalize_version(raw: &str) -> String {
    let raw = raw.trim();
    raw.strip_prefix(['v', 'V']).unwrap_or(raw).to_string()
}

/// True only when `latest` is *strictly* newer than `current`.
///
/// Numeric, dot-separated segments compared left to right (`0.0.10` beats
/// `0.0.2`). A trailing `-something` suffix makes a version *older* than the
/// same number without one (pre-releases). Anything unparseable is never
/// newer — an upstream tagging mistake must not produce update nags.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse(current), parse(latest)) {
        (Some(cur), Some(new)) => {
            if cur.numbers != new.numbers {
                return new.numbers > cur.numbers;
            }
            // Same release number: a pre-release is not newer than the
            // release; the release is newer than a pre-release.
            match (cur.pre, new.pre) {
                (None, Some(_)) => false,
                (Some(_), None) => true,
                _ => false,
            }
        }
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    numbers: Vec<u64>,
    pre: Option<String>,
}

fn parse(raw: &str) -> Option<Parsed> {
    let raw = normalize_version(raw);
    let (main, pre) = match raw.split_once('-') {
        Some((main, pre)) => (main, Some(pre.to_string())),
        None => (raw.as_str(), None),
    };
    if main.is_empty() {
        return None;
    }
    let mut numbers = Vec::with_capacity(main.split('.').count());
    for segment in main.split('.') {
        if segment.is_empty() || !segment.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        numbers.push(segment.parse().ok()?);
    }
    // Cap the shape: 1 to 4 numeric segments, sane magnitudes.
    if numbers.is_empty() || numbers.len() > 4 || numbers.iter().any(|&n| n > 1_000_000) {
        return None;
    }
    Some(Parsed { numbers, pre })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_versions_compare_numerically() {
        assert!(is_newer("0.0.1", "0.0.2"));
        assert!(is_newer("0.0.2", "0.0.10")); // not lexicographic
        assert!(is_newer("0.0.9", "0.1.0"));
        assert!(is_newer("0.9.9", "1.0.0"));
        assert!(!is_newer("0.0.2", "0.0.2"));
        assert!(!is_newer("0.0.3", "0.0.2"));
    }

    #[test]
    fn v_prefix_and_padding_are_normalized() {
        assert!(is_newer("v0.0.1", "v0.0.2"));
        assert!(is_newer("0.0.1", "v0.0.2"));
        assert!(is_newer("v0.1", "0.1.1")); // 0.1 == 0.1.0.0 padded
        assert!(!is_newer("0.1.0", "0.1"));
    }

    #[test]
    fn pre_releases_rank_below_releases() {
        assert!(!is_newer("0.0.2", "0.0.2-beta.1"));
        assert!(is_newer("0.0.2-beta.1", "0.0.2"));
        assert!(is_newer("0.0.1", "0.0.2-beta.1")); // different number still wins
    }

    #[test]
    fn garbage_never_counts_as_newer() {
        assert!(!is_newer("0.0.2", "latest"));
        assert!(!is_newer("0.0.2", ""));
        assert!(!is_newer("0.0.2", "0.0.x"));
        assert!(!is_newer("0.0.2", "v"));
        assert!(!is_newer("0.0.2", "0.0.2.0.0.0.0")); // too many segments
        assert!(!is_newer("not-a-version", "0.0.3"));
    }

    #[test]
    fn normalize_strips_prefix_once() {
        assert_eq!(normalize_version("v1.2.3"), "1.2.3");
        assert_eq!(normalize_version("V1.2.3"), "1.2.3");
        assert_eq!(normalize_version("1.2.3"), "1.2.3");
        assert_eq!(normalize_version(" v1.2.3 "), "1.2.3");
    }
}
