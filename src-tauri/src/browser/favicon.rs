//! The site icon a tab shows in the strip. The app window only admits images
//! as `data:` URLs, so an icon is fetched here and handed to the strip inline
//! rather than passed along as an address the window could not draw.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use futures::StreamExt;
use reqwest::{Client, Response};
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, Url, Webview};

use super::{BrowserManager, BLANK_URL, USER_AGENT};

const FETCH_TIMEOUT: Duration = Duration::from_secs(6);
/// Larger than any icon a site needs, and small enough that a strip full of
/// them still crosses to the window cheaply.
const MAX_BYTES: usize = 64 * 1024;
const CACHE_LIMIT: usize = 64;
/// The strip draws the icon at 13 points, so 32 pixels is the first size that
/// still looks sharp on a retina screen.
const WANTED: u32 = 32;

/// Icons already fetched, kept by address so a site is asked once.
pub(super) type IconCache = HashMap<String, Option<String>>;

/// Reads the page's own icon declarations. `rel` is matched as a word so
/// `rel="shortcut icon"` counts and `rel="mask-icon"` does not.
const LINKS: &str = r#"JSON.stringify([...document.querySelectorAll("link[rel][href]")].filter((link) => /(^|\s)(icon|apple-touch-icon)(\s|$)/i.test(link.rel)).map((link) => ({ rel: link.rel.toLowerCase(), href: link.href, sizes: link.getAttribute("sizes") || "", type: (link.type || "").toLowerCase() })))"#;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
struct IconLink {
    #[serde(default)]
    rel: String,
    #[serde(default)]
    href: String,
    #[serde(default)]
    sizes: String,
    #[serde(default, rename = "type")]
    kind: String,
}

/// Asks the tab which icon it declares, fetches it, and tells the strip.
/// Nothing is reported when the page has no usable icon — the pane draws a
/// globe until one arrives.
pub(super) async fn refresh(app: AppHandle, agent_id: String, tab_id: String, view: Webview) {
    let Some(page) = app
        .state::<BrowserManager>()
        .page(&agent_id, &tab_id)
        .filter(|page| page.url != BLANK_URL)
    else {
        return;
    };
    let Ok(page_url) = Url::parse(&page.url) else {
        return;
    };
    let Some(icon) = choose(&declared(&view).await, &page_url) else {
        return;
    };

    let key = icon.to_string();
    let manager = app.state::<BrowserManager>();
    let data = match manager.cached_icon(&key) {
        Some(known) => known,
        None => {
            let fetched = fetch(&icon).await;
            manager.remember_icon(key, fetched.clone());
            fetched
        }
    };
    let Some(data) = data else {
        return;
    };
    manager.note_page(&app, &agent_id, &tab_id, move |page| {
        page.favicon = Some(data);
    });
}

/// Whether two addresses belong to the same site, which is what decides if a
/// tab keeps showing its icon while the next page loads.
pub(super) fn same_site(a: &str, b: &str) -> bool {
    let host = |url: &str| {
        Url::parse(url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
    };
    match (host(a), host(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

async fn declared(view: &Webview) -> Vec<IconLink> {
    let Ok(raw) = super::tools::eval(view, LINKS).await else {
        return Vec::new();
    };
    parse_links(&raw)
}

/// The page answers with a JSON string, so the text comes back wrapped in one
/// more layer of quoting than the list it holds.
fn parse_links(raw: &str) -> Vec<IconLink> {
    let outer: Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let inner = match outer {
        Value::String(text) => serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
        other => other,
    };
    serde_json::from_value(inner).unwrap_or_default()
}

/// The best icon the page declares, else the address every site answers at.
/// Apple's touch icon is only taken when the page declares nothing else, since
/// it is a home-screen tile rather than the mark the site chose for a tab.
fn choose(links: &[IconLink], page: &Url) -> Option<Url> {
    let own: Vec<&IconLink> = links
        .iter()
        .filter(|link| !link.rel.contains("apple"))
        .collect();
    let group = if own.is_empty() {
        links.iter().collect()
    } else {
        own
    };
    group
        .into_iter()
        .filter_map(|link| Url::parse(&link.href).ok().map(|url| (score(link), url)))
        .max_by_key(|(score, _)| *score)
        .map(|(_, url)| url)
        .or_else(|| page.join("/favicon.ico").ok())
}

/// A drawing first, then the smallest icon that is at least `WANTED` pixels
/// across, then one that never said how big it is, then the biggest of those
/// too small to look right.
fn score(link: &IconLink) -> i32 {
    let path = link
        .href
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if link.kind.contains("svg") || path.ends_with(".svg") || link.sizes.eq_ignore_ascii_case("any")
    {
        return 1000;
    }
    match widest(&link.sizes) {
        Some(pixels) if pixels >= WANTED => 900 - (pixels - WANTED).min(600) as i32,
        Some(pixels) => 100 + pixels as i32,
        None => 200,
    }
}

/// `sizes="16x16 32x32"` describes one file holding both, so the larger one is
/// what it can be drawn at.
fn widest(sizes: &str) -> Option<u32> {
    sizes
        .split_whitespace()
        .filter_map(|token| token.split(['x', 'X']).next()?.parse::<u32>().ok())
        .max()
}

async fn fetch(url: &Url) -> Option<String> {
    if url.scheme() == "data" {
        let inline = url.to_string();
        return (inline.starts_with("data:image/") && inline.len() <= MAX_BYTES).then_some(inline);
    }
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let response = client().get(url.clone()).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = read_capped(response).await?;
    // Only the bytes get to say what they are. Icons are commonly served as
    // octet-stream, and a "page not found" is just as commonly served with the
    // icon's own content type on it.
    let mime = sniff(&bytes)?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

async fn read_capped(response: Response) -> Option<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return None;
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if body.len() + chunk.len() > MAX_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    (!body.is_empty()).then_some(body)
}

fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let head = &bytes[..bytes.len().min(64)];
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if head.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if head.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if head.starts_with(b"\x00\x00\x01\x00") || head.starts_with(b"\x00\x00\x02\x00") {
        return Some("image/x-icon");
    }
    if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start().to_ascii_lowercase();
    if text.starts_with("<svg")
        || (text.starts_with("<?xml") && String::from_utf8_lossy(bytes).contains("<svg"))
    {
        return Some("image/svg+xml");
    }
    None
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(FETCH_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_default()
    })
}

impl BrowserManager {
    fn cached_icon(&self, url: &str) -> Option<Option<String>> {
        self.icons
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(url)
            .cloned()
    }

    /// A failed fetch is remembered too, so a site that has no icon is not
    /// asked for one on every page it serves.
    fn remember_icon(&self, url: String, data: Option<String>) {
        let mut icons = self
            .icons
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if icons.len() >= CACHE_LIMIT {
            icons.clear();
        }
        icons.insert(url, data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(rel: &str, href: &str, sizes: &str, kind: &str) -> IconLink {
        IconLink {
            rel: rel.into(),
            href: href.into(),
            sizes: sizes.into(),
            kind: kind.into(),
        }
    }

    fn page() -> Url {
        Url::parse("https://a.test/deep/page?x=1").unwrap()
    }

    #[test]
    fn a_page_that_declares_nothing_falls_back_to_the_site_root() {
        assert_eq!(
            choose(&[], &page()).unwrap().as_str(),
            "https://a.test/favicon.ico"
        );
    }

    #[test]
    fn a_drawing_wins_then_the_smallest_icon_big_enough_to_stay_sharp() {
        let links = [
            link("icon", "https://a.test/16.png", "16x16", "image/png"),
            link("icon", "https://a.test/32.png", "32x32", "image/png"),
            link("icon", "https://a.test/512.png", "512x512", "image/png"),
        ];
        assert_eq!(
            choose(&links, &page()).unwrap().as_str(),
            "https://a.test/32.png"
        );
        let with_drawing = [
            links[1].clone(),
            link("icon", "https://a.test/mark.svg?v=2", "", ""),
        ];
        assert_eq!(
            choose(&with_drawing, &page()).unwrap().as_str(),
            "https://a.test/mark.svg?v=2"
        );
    }

    #[test]
    fn an_icon_of_unsaid_size_beats_one_too_small_to_look_right() {
        let links = [
            link("shortcut icon", "https://a.test/favicon.ico", "", ""),
            link("icon", "https://a.test/16.png", "16x16", "image/png"),
        ];
        assert_eq!(
            choose(&links, &page()).unwrap().as_str(),
            "https://a.test/favicon.ico"
        );
    }

    #[test]
    fn the_home_screen_tile_is_a_last_resort() {
        let both = [
            link("apple-touch-icon", "https://a.test/tile.png", "180x180", ""),
            link("icon", "https://a.test/favicon.ico", "", ""),
        ];
        assert_eq!(
            choose(&both, &page()).unwrap().as_str(),
            "https://a.test/favicon.ico"
        );
        let tile_only = [both[0].clone()];
        assert_eq!(
            choose(&tile_only, &page()).unwrap().as_str(),
            "https://a.test/tile.png"
        );
    }

    #[test]
    fn declarations_come_back_wrapped_as_a_json_string() {
        let raw = serde_json::to_string(
            r#"[{"rel":"icon","href":"https://a.test/i.png","sizes":"32x32","type":"image/png"}]"#,
        )
        .unwrap();
        assert_eq!(
            parse_links(&raw),
            vec![link("icon", "https://a.test/i.png", "32x32", "image/png")]
        );
        assert!(parse_links("not json").is_empty());
        assert!(parse_links("\"[]\"").is_empty());
    }

    #[test]
    fn bytes_name_themselves_and_a_page_of_html_names_nothing() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n rest"), Some("image/png"));
        assert_eq!(sniff(b"\x00\x00\x01\x00rest"), Some("image/x-icon"));
        assert_eq!(
            sniff(b"  <svg viewBox=\"0 0 1 1\"/>"),
            Some("image/svg+xml")
        );
        assert_eq!(sniff(b"<!doctype html><html>"), None);
        assert_eq!(sniff(b""), None);
    }

    /// Answers one request with the given body and hangs up.
    async fn serve(content_type: &str, body: Vec<u8>) -> Url {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        crate::install_tls_crypto();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/icon", listener.local_addr().unwrap())).unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut scratch = [0_u8; 1024];
            let _ = stream.read(&mut scratch).await;
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
            let _ = stream.shutdown().await;
        });
        url
    }

    #[tokio::test]
    async fn a_fetched_icon_comes_back_inline_under_the_name_its_bytes_give_it() {
        let png = b"\x89PNG\r\n\x1a\nbody".to_vec();
        let url = serve("application/octet-stream", png.clone()).await;
        assert_eq!(
            fetch(&url).await,
            Some(format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&png)
            ))
        );
    }

    #[tokio::test]
    async fn a_page_of_html_and_an_oversized_file_are_both_refused() {
        let html = serve("image/x-icon", b"<!doctype html><html>gone</html>".to_vec()).await;
        assert_eq!(fetch(&html).await, None);
        let huge = serve("image/png", vec![b'x'; MAX_BYTES + 1]).await;
        assert_eq!(fetch(&huge).await, None);
    }

    #[test]
    fn an_icon_survives_the_next_page_of_the_same_site_only() {
        assert!(same_site("https://a.test/one", "https://a.test/two"));
        assert!(!same_site("https://a.test/one", "https://b.test/one"));
        assert!(!same_site("about:blank", "https://a.test/one"));
    }
}
