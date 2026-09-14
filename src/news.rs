use std::borrow::Cow;
use std::time::Duration;

use anyhow::Context as _;

pub const NEWS_FEED_URL: &str = "https://archlinux.org/feeds/news/";
pub const NEWS_ITEMS: usize = 3;

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const NEWS_TITLE_CHARS: usize = 90;

#[derive(Clone, Debug)]
pub struct NewsItem {
    pub title: String,
    pub url: String,
    pub published: i64,
}

#[derive(Clone, Debug)]
pub struct NewsRow {
    pub title: String,
    pub url: String,
    pub age: String,
}

#[derive(Clone, Debug)]
pub enum NewsState {
    Loading,
    Ready(Vec<NewsRow>),
    Unavailable,
}

pub fn build_news_rows(items: Vec<NewsItem>, now: i64) -> Vec<NewsRow> {
    items
        .into_iter()
        .map(|item| {
            let elapsed = now.saturating_sub(item.published).max(0) as u64;
            NewsRow {
                title: clip_title(&item.title),
                url: item.url,
                age: crate::utils::humanize_age(elapsed),
            }
        })
        .collect()
}

fn clip_title(title: &str) -> String {
    if title.chars().count() <= NEWS_TITLE_CHARS {
        return title.to_string();
    }
    let clipped: String = title.chars().take(NEWS_TITLE_CHARS).collect();
    format!("{clipped}\u{2026}")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NewsField {
    Title,
    Link,
    Published,
}

#[derive(Default)]
struct PartialNews {
    title: String,
    link: String,
    date: String,
}

fn field_for(name: &str) -> Option<NewsField> {
    match name {
        "title" => Some(NewsField::Title),
        "link" => Some(NewsField::Link),
        "pubDate" => Some(NewsField::Published),
        _ => None,
    }
}

fn append_text(partial: &mut PartialNews, field: NewsField, text: &str) {
    match field {
        NewsField::Title => partial.title.push_str(text),
        NewsField::Link => partial.link.push_str(text),
        NewsField::Published => partial.date.push_str(text),
    }
}

fn finish_item(partial: PartialNews, items: &mut Vec<NewsItem>) {
    let title = partial.title.trim().to_string();
    let url = partial.link.trim().to_string();
    if url.is_empty() {
        return;
    }
    let Some(published) = parse_pubdate(partial.date.trim()) else {
        return;
    };
    items.push(NewsItem {
        title,
        url,
        published,
    });
}

fn parse_pubdate(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(raw)
        .map(|date| date.timestamp())
        .ok()
}

fn resolve_reference<'a>(event: &'a quick_xml::events::BytesRef<'a>) -> Cow<'a, str> {
    let text: &str = event;
    match text {
        "lt" => Cow::Borrowed("<"),
        "gt" => Cow::Borrowed(">"),
        "amp" => Cow::Borrowed("&"),
        "apos" => Cow::Borrowed("'"),
        "quot" => Cow::Borrowed("\""),
        _ => match event.resolve_char_ref() {
            Ok(Some(found)) => Cow::Owned(found.to_string()),
            _ => Cow::Owned(format!("&{text};")),
        },
    }
}

pub fn parse_news_rss(xml: &str) -> Vec<NewsItem> {
    let mut reader = quick_xml::reader::Reader::from_str(xml);
    let mut items = Vec::new();
    let mut buf = Vec::new();
    let mut partial = PartialNews::default();
    let mut in_item = false;
    let mut field: Option<NewsField> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(event)) => match event.name().into_inner() {
                "item" => {
                    partial = PartialNews::default();
                    in_item = true;
                    field = None;
                }
                name => {
                    if in_item {
                        field = field_for(name);
                    }
                }
            },
            Ok(quick_xml::events::Event::Text(event)) => {
                if let Some(active) = field {
                    append_text(&mut partial, active, &event.xml10_content());
                }
            }
            Ok(quick_xml::events::Event::CData(event)) => {
                if let Some(active) = field {
                    let text: &str = &event;
                    append_text(&mut partial, active, text);
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(event)) => {
                if let Some(active) = field {
                    append_text(&mut partial, active, &resolve_reference(&event));
                }
            }
            Ok(quick_xml::events::Event::End(event)) => match event.name().into_inner() {
                "item" => {
                    in_item = false;
                    field = None;
                    finish_item(std::mem::take(&mut partial), &mut items);
                }
                name => {
                    if in_item && field == field_for(name) {
                        field = None;
                    }
                }
            },
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("[pakajo] news feed parse error: {e}");
                break;
            }
        }
        buf.clear();
    }
    items
}

pub fn fetch_news() -> anyhow::Result<Vec<NewsItem>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(NEWS_FEED_URL)
        .call()
        .with_context(|| format!("failed to fetch {NEWS_FEED_URL}"))?;
    let body = response
        .body_mut()
        .read_to_string()
        .context("failed to read news feed body")?;
    let items = parse_news_rss(&body);
    if items.is_empty() {
        anyhow::bail!("news feed contained no usable items");
    }
    Ok(items.into_iter().take(NEWS_ITEMS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
<channel>
<title>Arch Linux News</title>
<link>https://archlinux.org/news/</link>
<item>
<title>First headline</title>
<link>https://archlinux.org/news/first-headline/</link>
<pubDate>Mon, 01 Sep 2025 10:00:00 +0000</pubDate>
</item>
<item>
<title>Second &amp; improved headline</title>
<link>https://archlinux.org/news/second-headline/</link>
<pubDate>Sun, 31 Aug 2025 10:00:00 +0000</pubDate>
</item>
<item>
<title>Undated item</title>
<link>https://archlinux.org/news/undated-item/</link>
</item>
<item>
<title><![CDATA[Third <headline>]]></title>
<link>https://archlinux.org/news/third-headline/</link>
<pubDate>Sat, 30 Aug 2025 10:00:00 +0000</pubDate>
</item>
</channel>
</rss>"#;

    #[test]
    fn parses_feed_order_skips_undated_and_unescapes_entities() {
        let items = parse_news_rss(SAMPLE_FEED);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].title, "First headline");
        assert_eq!(items[0].url, "https://archlinux.org/news/first-headline/");
        assert_eq!(items[1].title, "Second & improved headline");
        assert_eq!(items[1].url, "https://archlinux.org/news/second-headline/");
        assert_eq!(items[2].title, "Third <headline>");
        assert_eq!(items[2].url, "https://archlinux.org/news/third-headline/");
        assert!(items[0].published > items[1].published);
        assert!(items[1].published > items[2].published);
        assert!(items.iter().all(|item| item.published > 0));
    }

    #[test]
    fn skips_item_without_link() {
        let feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
<channel>
<item>
<title>Linkless headline</title>
<pubDate>Mon, 01 Sep 2025 10:00:00 +0000</pubDate>
</item>
<item>
<title>Linked headline</title>
<link>https://archlinux.org/news/linked-headline/</link>
<pubDate>Mon, 01 Sep 2025 10:00:00 +0000</pubDate>
</item>
</channel>
</rss>"#;
        let items = parse_news_rss(feed);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Linked headline");
        assert_eq!(items[0].url, "https://archlinux.org/news/linked-headline/");
    }

    #[test]
    fn unknown_named_entity_passes_through_literally() {
        let feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
<channel>
<item>
<title>Headline &foobar; here</title>
<link>https://archlinux.org/news/entity-headline/</link>
<pubDate>Mon, 01 Sep 2025 10:00:00 +0000</pubDate>
</item>
</channel>
</rss>"#;
        let items = parse_news_rss(feed);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Headline &foobar; here");
    }
}
