//! `ReleaseSource` over the GitHub API and release downloads of the official repository.
//!
//! Where hookah updates from is fixed: the repository in [`REPOSITORY`], on `api.github.com` and
//! `github.com`, over HTTPS. The latest release comes from the API; asset URLs are built here
//! from the release tag and the asset name, never taken from the API's answer. Every request
//! (and every redirect hop, such as the one from `github.com` to the CDN) must be HTTPS.
//!
//! The connection trusts the operating system's certificate store, so a corporate proxy that
//! re-signs TLS works once its root is installed, and it follows `HTTPS_PROXY`, `HTTP_PROXY`,
//! `ALL_PROXY`, and `NO_PROXY`. Connecting gives up after 5 s, a whole request after 60 s.
//!
//! Tests can point the source at a local server, but only in a debug build
//! (`cfg(debug_assertions)`): [`GithubReleases::at`] and the `HOOKAH_UPDATE_TEST_BASE` variable
//! do not exist in a release build, so a distributed binary cannot be redirected.

use std::time::Duration;

use hookah_application::ports::{Release, ReleaseSource, UpdateError};
use hookah_domain::update::{REPOSITORY, Version, VersionError};
use serde::Deserialize;
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig};

/// The most an archive may weigh: far above a real release, far below a disk-filling answer.
const MAX_ASSET_BYTES: u64 = 67_108_864;
/// The most the release description may weigh.
const MAX_API_BYTES: u64 = 4_194_304;

/// Debug builds only: a base URL that replaces both GitHub hosts.
#[cfg(debug_assertions)]
pub const TEST_BASE_VAR: &str = "HOOKAH_UPDATE_TEST_BASE";

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
}

/// Fetches releases of the official repository.
pub struct GithubReleases {
    agent: Agent,
    /// URL of the latest-release document.
    latest: String,
    /// Asset URL prefix: `<prefix>/<tag>/<name>`.
    downloads: String,
}

impl GithubReleases {
    /// The official repository on GitHub. In a debug build, `HOOKAH_UPDATE_TEST_BASE` redirects
    /// it to a test server.
    pub fn official() -> Self {
        #[cfg(debug_assertions)]
        if let Some(base) = test_base(std::env::var(TEST_BASE_VAR).ok()) {
            return Self::at(&base);
        }
        Self::new(
            format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"),
            format!("https://github.com/{REPOSITORY}/releases/download"),
            true,
        )
    }

    /// A server at `base` (plain HTTP allowed): `<base>/latest` answers like the API and assets
    /// are under `<base>/download/<tag>/<name>`. Debug builds only.
    #[cfg(debug_assertions)]
    pub fn at(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self::new(format!("{base}/latest"), format!("{base}/download"), false)
    }

    fn new(latest: String, downloads: String, https_only: bool) -> Self {
        let agent: Agent = Agent::config_builder()
            .https_only(https_only)
            // A test server on loopback must not be sent through a proxy from the environment.
            .proxy(if https_only {
                ureq::Proxy::try_from_env()
            } else {
                None
            })
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(concat!("hookah/", env!("CARGO_PKG_VERSION")))
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .into();
        Self {
            agent,
            latest,
            downloads,
        }
    }

    fn get(&self, url: &str, accept: &str, limit: u64) -> Result<Vec<u8>, UpdateError> {
        let mut response = self
            .agent
            .get(url)
            .header("Accept", accept)
            .call()
            .map_err(|e| UpdateError::failed(format!("cannot fetch {}: {e}", url)))?;
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| UpdateError::failed(format!("cannot read {}: {e}", url)))
    }
}

impl ReleaseSource for GithubReleases {
    fn latest(&self) -> Result<Release, UpdateError> {
        let body = self.get(&self.latest, "application/vnd.github+json", MAX_API_BYTES)?;
        let release: ApiRelease = serde_json::from_slice(&body)
            .map_err(|e| UpdateError::failed(format!("unexpected release document: {e}")))?;
        parse_release(release)
    }

    fn asset(&self, tag: &str, name: &str) -> Result<Vec<u8>, UpdateError> {
        // The tag came from `latest`, which only lets canonical tags through, and the name from
        // the domain, so neither can steer the URL to another path.
        let url = format!("{}/{tag}/{name}", self.downloads);
        self.get(&url, "application/octet-stream", MAX_ASSET_BYTES)
    }
}

/// The test server override, when the variable is set to something.
#[cfg(debug_assertions)]
fn test_base(value: Option<String>) -> Option<String> {
    value.filter(|base| !base.is_empty())
}

/// Checks the release is a published stable one with a canonical tag, and keeps what the update
/// needs from it.
fn parse_release(release: ApiRelease) -> Result<Release, UpdateError> {
    if release.draft || release.prerelease {
        return Err(UpdateError::failed(
            "the latest release is a draft or a prerelease",
        ));
    }
    let version = Version::parse(&release.tag_name).map_err(|e| match e {
        VersionError::Prerelease => UpdateError::failed("the latest release is a prerelease"),
        VersionError::Malformed => UpdateError::failed(format!(
            "the latest release tag {:?} is not a version",
            release.tag_name
        )),
    })?;
    let canonical =
        release.tag_name == format!("v{version}") || release.tag_name == version.to_string();
    if !canonical {
        return Err(UpdateError::failed(format!(
            "the latest release tag {:?} is not a plain version tag",
            release.tag_name
        )));
    }
    Ok(Release {
        version,
        tag: release.tag_name,
        assets: release.assets.into_iter().map(|a| a.name).collect(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn api(json: &str) -> Result<Release, UpdateError> {
        parse_release(serde_json::from_str(json).unwrap())
    }

    #[test]
    fn a_stable_release_keeps_its_tag_and_asset_names() {
        let release = api(r#"{"tag_name":"v1.2.3","draft":false,"prerelease":false,
                "assets":[{"name":"a.tar.gz","size":5},{"name":"SHA256SUMS"}]}"#)
        .unwrap();
        assert_eq!(release.version.to_string(), "1.2.3");
        assert_eq!(release.tag, "v1.2.3");
        assert_eq!(release.assets, ["a.tar.gz", "SHA256SUMS"]);
        assert_eq!(api(r#"{"tag_name":"1.2.3"}"#).unwrap().tag, "1.2.3");
    }

    #[test]
    fn drafts_and_prereleases_are_refused() {
        for json in [
            r#"{"tag_name":"v1.2.3","draft":true}"#,
            r#"{"tag_name":"v1.2.3","prerelease":true}"#,
            r#"{"tag_name":"v1.2.3-rc.1"}"#,
        ] {
            let e = api(json).unwrap_err();
            assert!(
                e.message().contains("prerelease") || e.message().contains("draft"),
                "{e}"
            );
        }
    }

    #[test]
    fn a_tag_that_is_not_a_plain_version_is_refused() {
        for tag in [
            "latest",
            "v1.2",
            "release-1.2.3",
            "v1.2.3 ",
            "V1.2.3",
            "vv1.2.3",
        ] {
            let json = format!(r#"{{"tag_name":{tag:?}}}"#);
            assert!(api(&json).is_err(), "{tag:?}");
        }
        // Whitespace around a parsable version must not reach a URL.
        assert!(api(r#"{"tag_name":" v1.2.3"}"#).is_err());
    }

    #[test]
    fn the_official_source_is_https_only_and_fixed() {
        let official = GithubReleases::official();
        // A test run is a debug build: only skip the check when the override is set.
        #[cfg(debug_assertions)]
        if std::env::var_os(TEST_BASE_VAR).is_some() {
            return;
        }
        assert_eq!(
            official.latest,
            "https://api.github.com/repos/izevg/hookah/releases/latest"
        );
        assert_eq!(
            official.downloads,
            "https://github.com/izevg/hookah/releases/download"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn an_empty_test_base_is_no_override() {
        assert_eq!(test_base(None), None);
        assert_eq!(test_base(Some(String::new())), None);
        assert_eq!(test_base(Some("http://x".into())), Some("http://x".into()));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_test_base_replaces_both_urls() {
        let source = GithubReleases::at("http://127.0.0.1:9/x/");
        assert_eq!(source.latest, "http://127.0.0.1:9/x/latest");
        assert_eq!(source.downloads, "http://127.0.0.1:9/x/download");
    }

    #[test]
    fn an_unreachable_server_is_a_failure_not_a_panic() {
        // Port 9 (discard) on loopback refuses the connection at once.
        #[cfg(debug_assertions)]
        {
            let source = GithubReleases::at("http://127.0.0.1:9");
            assert!(matches!(source.latest(), Err(UpdateError::Failed(_))));
            assert!(matches!(
                source.asset("v1.0.0", "x"),
                Err(UpdateError::Failed(_))
            ));
        }
    }
}
