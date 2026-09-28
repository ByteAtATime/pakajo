use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::color::strip_controls;

const AUR_RPC_URL: &str = "https://aur.archlinux.org/rpc/v5";
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
