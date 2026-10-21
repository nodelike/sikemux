//! The site icon a tab shows in the strip. The app window only admits images
//! as `data:` URLs, so an icon is fetched here and handed to the strip inline
//! rather than passed along as an address the window could not draw.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, Url, Webview};

use super::{BrowserManager, BLANK_URL, USER_AGENT};
use crate::error::AppResult;
use crate::http_client::{
    builder, data_url, fetch_image, follow_if, names_private_address, public_client, shared,
};

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

    let key = if same_host(&icon, &page_url) {
        format!("own {icon}")
    } else {
        icon.to_string()
    };
    let manager = app.state::<BrowserManager>();
    let data = match manager.cached_icon(&key) {
        Some(known) => known,
        None => {
            let fetched = fetch(&icon, &page_url).await;
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

/// A page may only have its icon fetched from its own host or from the public
/// internet, so a page cannot use the app to reach this machine or its network.
async fn fetch(url: &Url, page: &Url) -> Option<String> {
    if url.scheme() == "data" {
        let inline = url.to_string();
        return (inline.starts_with("data:image/") && inline.len() <= MAX_BYTES).then_some(inline);
    }
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let client = if same_host(url, page) {
        own_host_client()
    } else if names_private_address(url) {
        return None;
    } else {
        public_client()
    };
    let request = client
        .ok()?
        .get(url.clone())
        .timeout(FETCH_TIMEOUT)
        .header(reqwest::header::USER_AGENT, USER_AGENT);
    let (bytes, mime) = fetch_image(request, MAX_BYTES).await?;
    Some(data_url(mime, &bytes))
}

fn same_host(a: &Url, b: &Url) -> bool {
    a.host_str().is_some() && a.host_str() == b.host_str()
}

/// For an icon on the page's own host, which the page can already reach, as
/// long as every redirect stays there too.
fn own_host_client() -> AppResult<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    shared(&CLIENT, || {
        builder().no_proxy().redirect(Policy::custom(|attempt| {
            let home = attempt.previous().first().and_then(Url::host_str);
            let stays = home.is_some() && attempt.url().host_str() == home;
            follow_if(attempt, stays)
        }))
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

    /// Answers one request with the given body and hangs up.
    async fn serve(content_type: &str, body: Vec<u8>) -> Url {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        answer_once(head, body).await
    }

    async fn answer_once(head: String, body: Vec<u8>) -> Url {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        crate::install_tls_crypto();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/icon", listener.local_addr().unwrap())).unwrap();
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
        assert_eq!(fetch(&url, &url).await, Some(data_url("image/png", &png)));
    }

    #[tokio::test]
    async fn a_page_of_html_and_an_oversized_file_are_both_refused() {
        let html = serve("image/x-icon", b"<!doctype html><html>gone</html>".to_vec()).await;
        assert_eq!(fetch(&html, &html).await, None);
        let huge = serve("image/png", vec![b'x'; MAX_BYTES + 1]).await;
        assert_eq!(fetch(&huge, &huge).await, None);
    }

    #[tokio::test]
    async fn a_page_elsewhere_cannot_have_this_machine_fetched() {
        let png = b"\x89PNG\r\n\x1a\nbody".to_vec();
        let local = serve("image/png", png.clone()).await;
        assert_eq!(fetch(&local, &page()).await, None);
        let mut by_name = serve("image/png", png).await;
        by_name.set_host(Some("localhost")).unwrap();
        assert_eq!(fetch(&by_name, &page()).await, None);
        for address in [
            "http://169.254.169.254/icon",
            "http://[::1]/icon",
            "http://10.0.0.1/icon",
        ] {
            assert_eq!(fetch(&Url::parse(address).unwrap(), &page()).await, None);
        }
    }

    #[tokio::test]
    async fn an_icon_on_the_pages_own_host_may_not_redirect_off_it() {
        let png = b"\x89PNG\r\n\x1a\nbody".to_vec();
        let mut elsewhere = serve("image/png", png).await;
        elsewhere.set_host(Some("localhost")).unwrap();
        let head = format!(
            "HTTP/1.1 302 Found\r\nLocation: {elsewhere}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let icon = answer_once(head, Vec::new()).await;
        assert_eq!(fetch(&icon, &icon).await, None);
    }

    #[test]
    fn an_icon_survives_the_next_page_of_the_same_site_only() {
        assert!(same_site("https://a.test/one", "https://a.test/two"));
        assert!(!same_site("https://a.test/one", "https://b.test/one"));
        assert!(!same_site("about:blank", "https://a.test/one"));
    }
}
