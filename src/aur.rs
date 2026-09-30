use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::color::strip_controls;

const AUR_RPC_URL: &str = "https://aur.archlinux.org/rpc/v5";
const AUR_WEB_URL: &str = "https://aur.archlinux.org";
const COMMENTS_PER_PAGE: usize = 250;
const MAX_COMMENT_PAGES: usize = 40;
const MAX_BATCH: usize = 200;

fn de_text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.map(|s| strip_controls(&s).into_owned()))
}

fn de_texts<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let raw = Vec::<String>::deserialize(d)?;
    Ok(raw.iter().map(|s| strip_controls(s).into_owned()).collect())
}

#[derive(Debug, Deserialize)]
pub struct RpcResponse<T> {
    pub version: u8,
    #[serde(rename = "type")]
    pub type_: String,
    pub resultcount: u64,
    pub results: Vec<T>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AurInfo {
    #[serde(rename = "ID")]
    pub id: u64,
    pub name: String,
    #[serde(rename = "PackageBaseID")]
    pub package_base_id: u64,
    pub package_base: String,
    pub version: String,
    #[serde(default, deserialize_with = "de_text")]
    pub description: Option<String>,
    #[serde(rename = "URL", default, deserialize_with = "de_text")]
    pub url: Option<String>,
    pub num_votes: u64,
    pub popularity: f64,
    pub out_of_date: Option<i64>,
    #[serde(default, deserialize_with = "de_text")]
    pub maintainer: Option<String>,
    pub first_submitted: i64,
    pub last_modified: i64,
    #[serde(rename = "URLPath", default)]
    pub url_path: Option<String>,
    #[serde(default)]
    pub submitter: Option<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub depends: Vec<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub make_depends: Vec<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub check_depends: Vec<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub opt_depends: Vec<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub conflicts: Vec<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub provides: Vec<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub replaces: Vec<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub groups: Vec<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub license: Vec<String>,
    #[serde(default, deserialize_with = "de_texts")]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub co_maintainers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AurComment {
    pub author: String,
    pub posted: String,
    pub body: String,
    pub pinned: bool,
}

fn clean_header_text(raw: &str) -> String {
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    strip_controls(&normalized).into_owned()
}

fn comment_author(header: scraper::ElementRef<'_>, date_selector: &scraper::Selector) -> String {
    let full = header.text().collect::<String>();
    if let Some((author, _)) = full.split_once(" commented on ") {
        return clean_header_text(author);
    }
    if let Some(link) = header.select(date_selector).next() {
        return clean_header_text(&link.text().collect::<String>());
    }
    clean_header_text(&full)
}

fn comment_posted(header: scraper::ElementRef<'_>, date_selector: &scraper::Selector) -> String {
    if let Some(link) = header.select(date_selector).next() {
        return clean_header_text(&link.text().collect::<String>());
    }
    let full = header.text().collect::<String>();
    if let Some((_, posted)) = full.split_once(" on ") {
        return clean_header_text(posted);
    }
    String::new()
}

fn comment_body(body: scraper::ElementRef<'_>, block_selector: &scraper::Selector) -> String {
    let blocks: Vec<String> = body
        .select(block_selector)
        .map(|block| block.text().collect::<String>())
        .collect();
    let raw = if blocks.is_empty() {
        body.text().collect::<String>()
    } else {
        blocks.join("\n")
    };
    let mut lines = Vec::new();
    let mut blank = false;
    for line in raw.lines() {
        let trimmed = strip_controls(line.trim()).into_owned();
        if trimmed.is_empty() {
            if !lines.is_empty() && !blank {
                lines.push(String::new());
            }
            blank = true;
        } else {
            lines.push(trimmed);
            blank = false;
        }
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

pub fn parse_comments(html: &str) -> Vec<AurComment> {
    let Ok(section_selector) = scraper::Selector::parse("div.comments.package-comments") else {
        return Vec::new();
    };
    let Ok(heading_selector) = scraper::Selector::parse("h3 span.text") else {
        return Vec::new();
    };
    let Ok(header_selector) = scraper::Selector::parse("h4.comment-header") else {
        return Vec::new();
    };
    let Ok(body_selector) = scraper::Selector::parse("div.article-content") else {
        return Vec::new();
    };
    let Ok(date_selector) = scraper::Selector::parse("a.date") else {
        return Vec::new();
    };
    let Ok(block_selector) = scraper::Selector::parse("p, pre, li, blockquote") else {
        return Vec::new();
    };
    let document = scraper::Html::parse_document(html);
    let mut comments = Vec::new();
    for section in document.select(&section_selector) {
        let pinned = section
            .select(&heading_selector)
            .next()
            .is_some_and(|heading| {
                heading
                    .text()
                    .collect::<String>()
                    .trim_start()
                    .starts_with("Pinned")
            });
        let headers: Vec<_> = section.select(&header_selector).collect();
        let bodies: Vec<_> = section.select(&body_selector).collect();
        debug_assert_eq!(headers.len(), bodies.len());
        for (header, body) in headers.into_iter().zip(bodies) {
            comments.push(AurComment {
                author: comment_author(header, &date_selector),
                posted: comment_posted(header, &date_selector),
                body: comment_body(body, &block_selector),
                pinned,
            });
        }
    }
    comments
}

pub struct AurClient {
    agent: ureq::Agent,
}

impl AurClient {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Self { agent }
    }

    pub fn info(&self, name: &str) -> anyhow::Result<Option<AurInfo>> {
        let mut results = self.info_many(&[name.to_string()])?;
        Ok(results.pop())
    }

    pub fn info_many(&self, names: &[String]) -> anyhow::Result<Vec<AurInfo>> {
        let mut all = Vec::with_capacity(names.len());
        for chunk in names.chunks(MAX_BATCH) {
            let mut request = self.agent.get(&format!("{AUR_RPC_URL}/info"));
            for name in chunk {
                request = request.query("arg[]", name);
            }
            let mut response = request.call().context("AUR RPC request failed")?;
            let parsed: RpcResponse<AurInfo> = response
                .body_mut()
                .read_json()
                .context("failed to parse AUR response")?;
            if parsed.type_ == "error" {
                anyhow::bail!("AUR RPC error: {}", parsed.error.unwrap_or_default());
            }
            all.extend(parsed.results);
        }
        Ok(all)
    }

    fn rpc_search(&self, agent: &ureq::Agent, arg: &str, by: &str) -> anyhow::Result<Vec<AurInfo>> {
        let mut response = agent
            .get(&format!("{AUR_RPC_URL}/search"))
            .query("arg", arg)
            .query("by", by)
            .call()
            .context("AUR RPC request failed")?;
        let parsed: RpcResponse<AurInfo> = response
            .body_mut()
            .read_json()
            .context("failed to parse AUR response")?;
        if parsed.type_ == "error" {
            anyhow::bail!("AUR RPC error: {}", parsed.error.unwrap_or_default());
        }
        Ok(parsed.results)
    }

    pub(crate) fn search_by(&self, name: &str, by: &str) -> anyhow::Result<Vec<AurInfo>> {
        self.rpc_search(&self.agent, name, by)
    }

    pub fn comments(&self, package_base: &str) -> anyhow::Result<Vec<AurComment>> {
        if package_base.is_empty()
            || package_base
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '?' | '#'))
        {
            anyhow::bail!("invalid package base: {package_base}");
        }
        let mut all = Vec::new();
        for page in 0..MAX_COMMENT_PAGES {
            let offset = page * COMMENTS_PER_PAGE;
            let url =
                format!("{AUR_WEB_URL}/packages/{package_base}?PP={COMMENTS_PER_PAGE}&O={offset}");
            let mut response = self
                .agent
                .get(&url)
                .call()
                .with_context(|| format!("failed to fetch comments for {package_base}"))?;
            let html = response
                .body_mut()
                .read_to_string()
                .with_context(|| format!("failed to fetch comments for {package_base}"))?;
            let parsed = parse_comments(&html);
            if parsed.is_empty() {
                break;
            }
            let fresh = if page == 0 {
                parsed.len()
            } else {
                parsed.iter().filter(|comment| !comment.pinned).count()
            };
            if page == 0 {
                all.extend(parsed);
            } else {
                all.extend(parsed.into_iter().filter(|comment| !comment.pinned));
            }
            if fresh < COMMENTS_PER_PAGE {
                break;
            }
        }
        Ok(all)
    }
}

impl Default for AurClient {
    fn default() -> Self {
        Self::new()
    }
}

pub fn friendly_search_error(err: &anyhow::Error) -> String {
    let msg = format!("{err:#}");
    if msg.contains("Too many package results") {
        "Too many results! Please narrow your search".to_string()
    } else {
        msg.strip_prefix("AUR RPC error: ")
            .unwrap_or(&msg)
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::drives_terminal;

    #[test]
    fn parses_multiinfo_sample() {
        let sample = r#"{
            "version": 5,
            "type": "multiinfo",
            "resultcount": 1,
            "results": [
                {
                    "ID": 65262,
                    "Name": "google-chrome",
                    "PackageBaseID": 52825,
                    "PackageBase": "google-chrome",
                    "Version": "131.0.6778.87-1",
                    "Description": "The popular web trusted web browser by Google",
                    "URL": "https://www.google.com/chrome",
                    "NumVotes": 3500,
                    "Popularity": 12.34,
                    "OutOfDate": null,
                    "Maintainer": "someone",
                    "Submitter": "submitter",
                    "FirstSubmitted": 1318000000,
                    "LastModified": 1732000000,
                    "URLPath": "/cgit/aur.git/snapshot/google-chrome.tar.gz",
                    "Depends": ["alsa-lib", "gtk3"]
                }
            ]
        }"#;

        let parsed: RpcResponse<AurInfo> = serde_json::from_str(sample).unwrap();
        assert_eq!(parsed.results.len(), 1);
        let pkg = &parsed.results[0];
        assert_eq!(pkg.name, "google-chrome");
        assert_eq!(pkg.version, "131.0.6778.87-1");
        assert_eq!(pkg.num_votes, 3500);
        assert_eq!(
            pkg.url_path.as_deref(),
            Some("/cgit/aur.git/snapshot/google-chrome.tar.gz")
        );
        assert_eq!(pkg.depends, vec!["alsa-lib", "gtk3"]);
    }

    #[test]
    #[ignore]
    fn live_info() {
        let client = AurClient::new();
        let pkg = client
            .info("google-chrome")
            .expect("AUR info should succeed");
        assert!(pkg.is_some());
        assert_eq!(pkg.unwrap().name, "google-chrome");
    }

    #[test]
    fn parses_search_response_without_submitter() {
        let sample = r#"{
            "version": 5,
            "type": "search",
            "resultcount": 1,
            "results": [
                {
                    "ID": 65262,
                    "Name": "google-chrome",
                    "PackageBaseID": 52825,
                    "PackageBase": "google-chrome",
                    "Version": "131.0.6778.87-1",
                    "Description": "The popular web trusted web browser by Google",
                    "URL": "https://www.google.com/chrome",
                    "NumVotes": 3500,
                    "Popularity": 12.34,
                    "OutOfDate": null,
                    "Maintainer": "someone",
                    "FirstSubmitted": 1318000000,
                    "LastModified": 1732000000,
                    "URLPath": "/cgit/aur.git/snapshot/google-chrome.tar.gz"
                }
            ]
        }"#;

        let parsed: RpcResponse<AurInfo> = serde_json::from_str(sample).unwrap();
        assert_eq!(parsed.results.len(), 1);
        let pkg = &parsed.results[0];
        assert_eq!(pkg.name, "google-chrome");
        assert_eq!(pkg.submitter, None);
    }

    #[test]
    fn parses_comment_fixture() {
        let html = include_str!("aur_comments_fixture.html");
        let comments = parse_comments(html);
        assert_eq!(comments.len(), 3);

        assert_eq!(comments[0].author, "gromit");
        assert_eq!(comments[0].posted, "2023-04-15 08:22 (UTC)");
        assert!(comments[0].pinned);
        assert_eq!(
            comments[0].body,
            "When reporting this package as outdated make sure there is indeed a new version for Linux Desktop. You can have a look at the \"Stable updates\" tag in Release blog for this.\nYou can also run this command to obtain the version string for the latest chrome version:\n$ curl -sSf https://dl.google.com/linux/chrome/deb/dists/stable/main/binary-amd64/Packages | \\\ngrep -A1 \"Package: google-chrome-stable\" | \\\nawk '/Version/{print $2}' | \\\ncut -d '-' -f1\n\nDo not report updates for ChromeOS, Android or other platforms stable versions as updates here."
        );

        assert_eq!(comments[1].author, "tioguda");
        assert_eq!(comments[1].posted, "2026-09-23 09:29 (UTC)");
        assert!(!comments[1].pinned);
        assert_eq!(
            comments[1].body,
            "@ZappyBoy comment out the gtk-modules line in the file specified here, this will fix your problem.\nEdit: Actually, appmenu-gtk-module-git fixes the gtk-modules issues."
        );

        assert_eq!(comments[2].author, "gromit");
        assert_eq!(comments[2].posted, "2026-09-23 08:45 (UTC)");
        assert!(!comments[2].pinned);
        assert_eq!(
            comments[2].body,
            "@ZappyBoy it does not crash on my machine :o"
        );
    }

    #[test]
    fn parse_comments_empty_page() {
        let html = "<html><body><div><p>package not found</p></div></body></html>";
        assert_eq!(parse_comments(html), Vec::new());
    }

    #[test]
    fn comment_body_strips_terminal_escapes() {
        let hostile = "\u{1b}]52;c;QVRUQUNL\u{7} hello \u{1b}[?25l world";
        assert!(drives_terminal(hostile));
        let html = format!(
            "<div class=\"comments package-comments\"><div class=\"comments-header\"><h3><span class=\"text\">Latest Comments</span></h3></div><h4 class=\"comment-header\">alice commented on <a class=\"date\">2026-01-02 03:04 (UTC)</a></h4><div class=\"article-content\"><div><p>{hostile}</p></div></div></div>"
        );
        let comments = parse_comments(&html);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "alice");
        assert!(!comments[0].pinned);
        assert_eq!(comments[0].body, "]52;c;QVRUQUNL hello [?25l world");
        assert!(!drives_terminal(&comments[0].body));
    }

    #[test]
    fn comment_header_strips_terminal_escapes() {
        let hostile_author = "\u{1b}[8mHIDDEN\u{1b}[28m mallory";
        let hostile_date = "2026-01-02 03:04 \u{1b}]52;c;QVRUQUNL\u{7}(UTC)";
        assert!(drives_terminal(hostile_author));
        assert!(drives_terminal(hostile_date));
        let html = format!(
            "<div class=\"comments package-comments\"><div class=\"comments-header\"><h3><span class=\"text\">Latest Comments</span></h3></div><h4 class=\"comment-header\">{hostile_author} commented on <a class=\"date\">{hostile_date}</a></h4><div class=\"article-content\"><div><p>hi</p></div></div></div>"
        );
        let comments = parse_comments(&html);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "[8mHIDDEN[28m mallory");
        assert_eq!(comments[0].posted, "2026-01-02 03:04 ]52;c;QVRUQUNL(UTC)");
        assert!(!drives_terminal(&comments[0].author));
        assert!(!drives_terminal(&comments[0].posted));
    }

    #[test]
    fn comments_rejects_invalid_package_base() {
        let client = AurClient::new();
        for base in ["", "foo/bar", "foo?x=1", "foo#frag", "foo\u{7}bar"] {
            assert!(client.comments(base).is_err(), "accepted: {base:?}");
        }
    }

    #[test]
    #[ignore]
    fn live_comments() {
        let client = AurClient::new();
        let comments = client
            .comments("google-chrome")
            .expect("AUR comments should succeed");
        assert!(!comments.is_empty());
        assert!(comments.iter().all(|c| !c.author.is_empty()));
    }

    #[test]
    fn rpc_metadata_cannot_drive_the_terminal() {
        let payload = "\u{1b}]52;c;QVRUQUNL\u{7} \u{1b}[?25l \u{1b}[8mHIDDEN\u{1b}[28m";
        let hostile = serde_json::to_string(&payload).unwrap();
        let sample = format!(
            r#"{{"version":5,"type":"multiinfo","resultcount":1,"results":[{{"ID":1,"Name":"cava","PackageBaseID":1,"PackageBase":"cava","Version":"1.0-1","FirstSubmitted":1,"LastModified":2,"NumVotes":0,"Popularity":0.0,"Description":{h},"URL":{h},"Maintainer":{h},"Depends":[{h}]}}]}}"#,
            h = hostile
        );

        let parsed: RpcResponse<AurInfo> = serde_json::from_str(&sample).unwrap();
        let pkg = &parsed.results[0];
        assert_eq!(
            pkg.description.as_deref(),
            Some("]52;c;QVRUQUNL [?25l [8mHIDDEN[28m")
        );
        for text in [
            pkg.url.as_deref().unwrap_or_default(),
            pkg.maintainer.as_deref().unwrap_or_default(),
            &pkg.depends[0],
        ] {
            assert!(!drives_terminal(text), "still hostile: {text:?}");
        }
    }
}
