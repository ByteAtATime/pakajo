use std::time::Duration;

use anyhow::Context as _;

pub const AUR_META_URL: &str = "https://aur.archlinux.org/packages-meta-ext-v1.json.gz";

const FETCH_TIMEOUT: Duration = Duration::from_secs(600);

pub enum FetchOutcome {
    NotModified,
    Updated(DecompressedDump),
}

pub struct DecompressedDump {
    resp: ureq::http::Response<ureq::Body>,
    last_modified: String,
}

impl DecompressedDump {
    pub fn reader(&mut self) -> impl std::io::BufRead + use<'_> {
        std::io::BufReader::new(flate2::read::GzDecoder::new(
            self.resp.body_mut().as_reader(),
        ))
    }

    pub fn last_modified(&self) -> &str {
        &self.last_modified
    }
}

pub fn fetch(url: &str, if_modified_since: Option<&str>) -> anyhow::Result<FetchOutcome> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into();

    let mut request = agent.get(url);
    if let Some(since) = if_modified_since {
        request = request.header("If-Modified-Since", since);
    }

    let response = request
        .call()
        .with_context(|| format!("failed to fetch {url}"))?;

    if response.status().as_u16() == 304 {
        return Ok(FetchOutcome::NotModified);
    }

    let last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|value| value.to_str().ok())
        .map(|raw| raw.to_string())
        .with_context(|| format!("{url} response is missing a Last-Modified header"))?;

    Ok(FetchOutcome::Updated(DecompressedDump {
        resp: response,
        last_modified,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn live_conditional_fetch_round_trip() {
        let first = fetch(AUR_META_URL, None).expect("initial fetch should succeed");
        let last_modified = match first {
            FetchOutcome::Updated(dump) => dump.last_modified().to_string(),
            FetchOutcome::NotModified => panic!("initial fetch should return Updated"),
        };

        let second =
            fetch(AUR_META_URL, Some(&last_modified)).expect("conditional fetch should succeed");
        match second {
            FetchOutcome::NotModified => {}
            FetchOutcome::Updated(_) => {
                panic!("conditional fetch should return NotModified for a fresh Last-Modified");
            }
        }
    }
}
