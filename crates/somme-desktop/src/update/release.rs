//! What GitHub says about Somme's newest release, and which of its files
//! this computer needs.

use std::fmt;
use std::sync::LazyLock;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use reqwest::header;
use serde::Deserialize;

/// Where Somme is published.
pub const REPO: &str = "kahflane/Somme";
const API: &str = "https://api.github.com";
/// More than any release's details need.
const MAX_JSON: usize = 1024 * 1024;
/// What a tag keeps as it is in a link.
const TAG: &AsciiSet = &NON_ALPHANUMERIC.remove(b'.').remove(b'-').remove(b'_');
/// How much of the release notes the popover shows.
const NOTE_LINES: usize = 10;
const NOTE_CHARS: usize = 600;

/// A plain `major.minor.patch` version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// Reads `1.2.3` or `v1.2.3`. Anything else, like `1.2.3-beta`, is None.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.').map(number);
        let version = Version(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    /// The version of this copy of Somme.
    pub fn running() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }
}

/// Digits only, so `+1` or ` 1` don't count.
fn number(part: &str) -> Option<u64> {
    if part.is_empty() || part.len() > 9 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A file attached to a release.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub browser_download_url: String,
}

#[derive(Deserialize)]
struct Raw {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// What's new, as short plain text.
    pub notes: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// Reads GitHub's answer. None for drafts, pre-releases and tags that
    /// are not a plain version.
    pub fn parse(json: &str) -> Option<Self> {
        let raw: Raw = serde_json::from_str(json).ok()?;
        if raw.draft || raw.prerelease {
            return None;
        }
        Some(Self {
            version: Version::parse(&raw.tag_name)?,
            notes: notes(raw.body.as_deref().unwrap_or_default()),
            tag: raw.tag_name,
            assets: raw.assets,
        })
    }

    /// The release on github.com. Made here rather than taken from the
    /// answer, so it can only lead to Somme's own page.
    pub fn page(&self) -> String {
        format!("https://github.com/{REPO}/releases/tag/{}", utf8_percent_encode(&self.tag, TAG))
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.name == name)
    }

    /// The zipped app for this Mac.
    pub fn mac_zip(&self) -> String {
        format!("Somme-{}-macos-{}.zip", self.version, std::env::consts::ARCH)
    }
}

/// Where GitHub's API is. UI check builds may point it at a fake on this computer.
fn api_base() -> String {
    #[cfg(feature = "snapshot")]
    if let Some(fake) = crate::github::api::fake_api() {
        return fake;
    }
    API.to_owned()
}

/// Asks GitHub for the newest release. None when there is none yet, or it
/// is not one Somme can use.
pub async fn latest(client: &reqwest::Client) -> anyhow::Result<Option<Release>> {
    let url = format!("{}/repos/{REPO}/releases/latest", api_base());
    let mut response = client
        .get(url)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    response = response.error_for_status()?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_JSON {
            anyhow::bail!("the release details are too long");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Release::parse(&String::from_utf8_lossy(&body)))
}

/// Release notes as a few lines of plain text: no Markdown marks, links as
/// their words, and cut short when long.
pub fn notes(body: &str) -> String {
    static IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").expect("image pattern"));
    static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").expect("link pattern"));
    static HTML: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("tag pattern"));
    let mut lines: Vec<String> = Vec::new();
    for line in body.lines() {
        let line = IMAGE.replace_all(line, "");
        let line = LINK.replace_all(&line, "$1");
        let line = HTML.replace_all(&line, "");
        let line = line.replace("**", "").replace("__", "").replace('`', "");
        let mut line = line.trim().to_owned();
        // A heading is `#` to `######` and a space.
        let hashes = line.len() - line.trim_start_matches('#').len();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            line = line[hashes..].trim_start().to_owned();
            // The popover has its own "What's new" title.
            if lines.is_empty() && line.to_lowercase().starts_with("what") {
                continue;
            }
        }
        for bullet in ["- ", "* ", "+ "] {
            if let Some(rest) = line.strip_prefix(bullet) {
                line = format!("• {}", rest.trim_start());
            }
        }
        if line.is_empty() && lines.last().is_none_or(String::is_empty) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let cut = lines.len() > NOTE_LINES;
    lines.truncate(NOTE_LINES);
    let mut text = lines.join("\n");
    if text.chars().count() > NOTE_CHARS {
        text = text.chars().take(NOTE_CHARS).collect::<String>().trim_end().to_owned();
        text.push('…');
    } else if cut {
        text.push_str("\n…");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(Version::parse("v0.10.0"), Some(Version(0, 10, 0)));
        assert_eq!(Version::parse("1.2.3"), Some(Version(1, 2, 3)));
        assert!(Version::parse("v0.10.0") > Version::parse("v0.9.9"));
        assert!(Version::parse("v1.0.0") > Version::parse("0.99.99"));
        for bad in ["", "v", "1.2", "1.2.3.4", "1.2.3-beta", "v1.x.3", "1.+2.3", "1. 2.3", "vv1.2.3", "10000000000.0.0"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(Version::running().to_string(), env!("CARGO_PKG_VERSION"));
    }

    fn release_json(tag: &str, draft: bool, prerelease: bool) -> String {
        serde_json::json!({
            "tag_name": tag,
            "draft": draft,
            "prerelease": prerelease,
            "html_url": "https://evil.example.test/",
            "body": "## What's new\n\n- Faster **SFTP**\n- See [the docs](https://example.test/docs)\n",
            "assets": [
                {"name": "SHA256SUMS", "size": 120, "browser_download_url": "https://github.com/kahflane/Somme/releases/download/v0.2.0/SHA256SUMS"},
            ],
        })
        .to_string()
    }

    #[test]
    fn reads_a_release_and_skips_drafts_and_prereleases() {
        let release = Release::parse(&release_json("v0.2.0", false, false)).unwrap();
        assert_eq!(release.version, Version(0, 2, 0));
        assert_eq!(release.notes, "• Faster SFTP\n• See the docs");
        assert_eq!(release.asset("SHA256SUMS").map(|asset| asset.size), Some(120));
        assert_eq!(release.asset("SHA256SUMS.sig"), None);
        // The page is Somme's own, whatever the answer says.
        assert_eq!(release.page(), "https://github.com/kahflane/Somme/releases/tag/v0.2.0");
        assert!(release.mac_zip().starts_with("Somme-0.2.0-macos-"));
        assert_eq!(Release::parse(&release_json("v0.2.0", true, false)), None);
        assert_eq!(Release::parse(&release_json("v0.2.0", false, true)), None);
        assert_eq!(Release::parse(&release_json("nightly", false, false)), None);
        assert_eq!(Release::parse("not json"), None);
    }

    #[test]
    fn notes_are_short_plain_text() {
        assert_eq!(notes(""), "");
        assert_eq!(notes("# Title\n\n\n\nText with `code` and ![pic](x.png)<br>\n\n"), "Title\n\nText with code and");
        assert_eq!(notes("#12 is fixed"), "#12 is fixed");
        assert_eq!(notes("## What's Changed\n* Fix by @ann in #3\n"), "• Fix by @ann in #3");
        assert_eq!(notes("Intro\n## What's new\n"), "Intro\nWhat's new");
        let long: String = (1..=20).map(|n| format!("- item {n}\n")).collect();
        let short = notes(&long);
        assert_eq!(short.lines().count(), NOTE_LINES + 1);
        assert!(short.ends_with('…'));
        let wide = notes(&"word ".repeat(400));
        assert!(wide.chars().count() <= NOTE_CHARS + 1);
        assert!(wide.ends_with('…'));
    }
}
