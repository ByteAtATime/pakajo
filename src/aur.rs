use std::time::Duration;

use anyhow::Context as _;
use serde::Deserialize;

const AUR_RPC_URL: &str = "https://aur.archlinux.org/rpc/v5";
const MAX_BATCH: usize = 200;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct RpcResponse<T> {
    pub version: u8,
    #[serde(rename = "type")]
    pub type_: String,
    pub resultcount: u64,
    pub results: Vec<T>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
pub struct AurInfo {
    #[serde(rename = "ID")]
    pub id: u64,
    pub name: String,
    #[serde(rename = "PackageBaseID")]
    pub package_base_id: u64,
    pub package_base: String,
    pub version: String,
    pub description: Option<String>,
    #[serde(rename = "URL")]
    pub url: Option<String>,
    pub num_votes: u64,
    pub popularity: f64,
    pub out_of_date: Option<i64>,
    pub maintainer: Option<String>,
    pub first_submitted: i64,
    pub last_modified: i64,
    #[serde(rename = "URLPath")]
    pub url_path: Option<String>,
    #[serde(default)]
    pub submitter: Option<String>,
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub make_depends: Vec<String>,
    #[serde(default)]
    pub check_depends: Vec<String>,
    #[serde(default)]
    pub opt_depends: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub replaces: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub license: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub co_maintainers: Vec<String>,
}

pub struct AurClient {
    agent: ureq::Agent,
    search_agent: ureq::Agent,
}

impl AurClient {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        let search_agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(8)))
            .build()
            .into();
        Self {
            agent,
            search_agent,
        }
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

    pub fn search(&self, arg: &str, by: &str) -> anyhow::Result<Vec<AurInfo>> {
        self.rpc_search(&self.search_agent, arg, by)
    }

    pub fn search_by_provides(&self, name: &str) -> anyhow::Result<Vec<AurInfo>> {
        self.rpc_search(&self.agent, name, "provides")
    }
}

impl Default for AurClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    #[ignore]
    fn live_search() {
        let client = AurClient::new();
        let results = client
            .search("google", "name-desc")
            .expect("AUR search should succeed");
        assert!(!results.is_empty());
    }
}
