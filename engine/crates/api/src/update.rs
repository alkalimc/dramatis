//! The update check: compare the running version with the latest GitHub release. Nothing
//! is downloaded or installed; the person is told and given the release page.

use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const LATEST_URL: &str = "https://api.github.com/repos/alkalimc/dramatis/releases/latest";
/// How long a result stands before an automatic check asks again.
pub const INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;

/// The last successful check, as kept in the home directory. Whether it is newer is worked
/// out against the running version each time, so an upgrade clears the notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub at_ms: i64,
    pub latest: String,
    pub url: String,
}

/// `(major, minor, patch, is_release)`: a pre-release sorts below its release.
fn parse(version: &str) -> Option<(u64, u64, u64, bool)> {
    let v = version.trim().trim_start_matches(['v', 'V']);
    let v = v.split('+').next()?;
    let (core, release) = match v.split_once('-') {
        Some((core, _)) => (core, false),
        None => (v, true),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let found = (parts.next()??, parts.next()??, parts.next()??, release);
    parts.next().is_none().then_some(found)
}

/// Whether `latest` is a higher version than `current`. A name that is not a version is
/// never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

/// Whether a result from `at_ms` is still good at `now_ms`.
pub fn fresh(at_ms: i64, now_ms: i64) -> bool {
    (0..INTERVAL_MS).contains(&(now_ms - at_ms))
}

/// The version (without a leading `v`) and release page out of a "latest release" reply.
/// The page must be under this project; anything else falls back to the releases list.
pub fn parse_release(body: &[u8]) -> Result<(String, String), String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        html_url: Option<String>,
    }
    let release: Release = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    parse(&release.tag_name).ok_or_else(|| "release tag is not a version".to_string())?;
    let url = release
        .html_url
        .filter(|u| u.starts_with(&format!("{REPOSITORY}/")))
        .unwrap_or_else(|| format!("{REPOSITORY}/releases"));
    Ok((
        release.tag_name.trim_start_matches(['v', 'V']).to_owned(),
        url,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_not_text() {
        assert!(is_newer("v1.0.1", "1.0.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("v0.9.0", "1.0.0"));
        // A pre-release is below its release, and a build tag is ignored.
        assert!(is_newer("1.1.0", "1.1.0-rc.1"));
        assert!(!is_newer("1.1.0-rc.1", "1.1.0"));
        assert!(!is_newer("1.1.0+7", "1.1.0"));
        // Not versions: never newer.
        assert!(!is_newer("nightly", "1.0.0"));
        assert!(!is_newer("1.0", "0.9.0"));
        assert!(!is_newer("1.0.0.1", "1.0.0"));
    }

    #[test]
    fn a_release_reply_gives_version_and_page() {
        let body = br#"{"url":"x","tag_name":"v1.2.3","name":"1.2.3","draft":false,
            "html_url":"https://github.com/alkalimc/dramatis/releases/tag/v1.2.3","assets":[]}"#;
        assert_eq!(
            parse_release(body),
            Ok((
                "1.2.3".into(),
                "https://github.com/alkalimc/dramatis/releases/tag/v1.2.3".into()
            ))
        );
        // A page outside the project is not passed on.
        let body = br#"{"tag_name":"v1.2.3","html_url":"https://evil.example/x"}"#;
        assert_eq!(
            parse_release(body).unwrap().1,
            "https://github.com/alkalimc/dramatis/releases"
        );
        assert!(parse_release(br#"{"tag_name":"nightly"}"#).is_err());
        assert!(parse_release(br#"{"message":"Not Found"}"#).is_err());
        assert!(parse_release(b"<html>").is_err());
    }

    #[test]
    fn a_result_is_fresh_for_a_day() {
        assert!(fresh(1_000, 1_000 + INTERVAL_MS - 1));
        assert!(!fresh(1_000, 1_000 + INTERVAL_MS));
        // A clock set back since the check does not freeze the next one.
        assert!(!fresh(5_000, 1_000));
    }
}
