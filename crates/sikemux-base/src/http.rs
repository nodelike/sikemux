//! HTTP clients for reaching the internet, one that never reaches this
//! machine or its network, and fetching a remote image for a client, which
//! draws images handed to it inline.

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use futures_util::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::{Attempt, Policy};
use reqwest::{Client, ClientBuilder, RequestBuilder};
use url::Url;

/// A server can answer from several addresses and a network can silently drop one.
/// Without this a request waits on that address for its whole timeout instead of trying the next.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: usize = 5;

/// Requests that need a deadline set one with `RequestBuilder::timeout`, so
/// long downloads are only cut off when the server goes quiet.
pub fn client() -> reqwest::Result<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    shared(&CLIENT, builder)
}

/// Names resolve only to public addresses and the connection goes to the
/// address that was checked, so a name cannot be pointed at this machine or
/// its network between the check and the request. An address written into
/// the URL skips name lookup, so callers check it with `names_private_address`.
pub fn public_client() -> reqwest::Result<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    shared(&CLIENT, || {
        builder()
            .no_proxy()
            .dns_resolver(PublicOnly)
            .redirect(Policy::custom(|attempt| {
                let leaves_https = attempt.url().scheme() != "https"
                    && attempt.previous().iter().any(|url| url.scheme() == "https");
                let allowed = !names_private_address(attempt.url()) && !leaves_https;
                follow_if(attempt, allowed)
            }))
    })
}

pub fn builder() -> ClientBuilder {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .user_agent(concat!("sikemux/", env!("CARGO_PKG_VERSION")))
}

pub fn shared(
    cell: &'static OnceLock<Client>,
    build: impl FnOnce() -> ClientBuilder,
) -> reqwest::Result<&'static Client> {
    if let Some(client) = cell.get() {
        return Ok(client);
    }
    let client = build().build()?;
    Ok(cell.get_or_init(|| client))
}

pub fn follow_if(attempt: Attempt<'_>, allowed: bool) -> reqwest::redirect::Action {
    if allowed && attempt.previous().len() <= MAX_REDIRECTS {
        attempt.follow()
    } else {
        attempt.stop()
    }
}

/// The image's bytes and type, or nothing when the answer is not an image or
/// is larger than `max_bytes`. Only the bytes get to say what they are: images
/// are commonly served as octet-stream, and a "page not found" is just as
/// commonly served with the image's own content type on it.
pub async fn fetch_image(
    request: RequestBuilder,
    max_bytes: usize,
) -> Option<(Vec<u8>, &'static str)> {
    let body = fetch_bounded(request, max_bytes).await?;
    let mime = sniff(&body).filter(|mime| mime.starts_with("image/"))?;
    Some((body, mime))
}

/// The answer's bytes, or nothing when it fails or holds more than `max_bytes`.
pub async fn fetch_bounded(request: RequestBuilder, max_bytes: usize) -> Option<Vec<u8>> {
    let response = request.send().await.ok()?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
    {
        return None;
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if body.len() + chunk.len() > max_bytes {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

pub fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

pub fn names_private_address(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => !is_public(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => !is_public(IpAddr::V6(ip)),
        Some(url::Host::Domain(_)) => false,
        None => true,
    }
}

struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let public: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|address| is_public(address.ip()))
                .collect();
            if public.is_empty() {
                return Err(format!("{host} is not on the public internet").into());
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            let shared = first == 100 && (64..128).contains(&second);
            let benchmarking = first == 198 && (second & 0xfe) == 18;
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_documentation()
                || first == 0
                || first >= 240
                || shared
                || benchmarking)
        }
        IpAddr::V6(ip) => {
            if let Some(embedded) = ip.to_ipv4() {
                return is_public(IpAddr::V4(embedded));
            }
            let first = ip.segments()[0];
            let unique_local = (first & 0xfe00) == 0xfc00;
            let link_local = (first & 0xffc0) == 0xfe80;
            let site_local = (first & 0xffc0) == 0xfec0;
            !(ip.is_loopback() || ip.is_multicast() || unique_local || link_local || site_local)
        }
    }
}

/// A single `bytes=start-` or `bytes=start-end` range, which is all media elements ask for.
pub fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => None,
        end => Some(end.parse().ok()?),
    };
    Some((start, end))
}

pub fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = segment
                .get(at + 1..at + 3)
                .filter(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The type a file's name says it is; `application/octet-stream` when the
/// name says nothing.
pub fn content_type(path: &std::path::Path) -> &'static str {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" | "md" | "csv" | "log" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "psd" => "image/vnd.adobe.photoshop",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aif" | "aiff" => "audio/aiff",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "rtf" => "application/rtf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "pages" => "application/vnd.apple.pages",
        "numbers" => "application/vnd.apple.numbers",
        "key" => "application/vnd.apple.keynote",
        "epub" => "application/epub+zip",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// What a file is: what its name says, else what its first bytes announce.
pub fn mime_of(path: &std::path::Path) -> std::io::Result<&'static str> {
    use std::io::Read;
    let named = content_type(path);
    if named != "application/octet-stream" {
        return Ok(named);
    }
    let mut head = Vec::with_capacity(64);
    std::fs::File::open(path)?.take(64).read_to_end(&mut head)?;
    Ok(sniff(&head).unwrap_or(named))
}

/// The type a file's first bytes announce, for files whose name does not say
/// and downloads whose server cannot be trusted to.
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    let starts = |magic: &[u8]| head.starts_with(magic);
    let at = |offset: usize, magic: &[u8]| head.get(offset..offset + magic.len()) == Some(magic);
    if starts(b"%PDF-") {
        return Some("application/pdf");
    }
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if starts(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some("image/gif");
    }
    if starts(b"RIFF") {
        return match head.get(8..12) {
            Some(b"WEBP") => Some("image/webp"),
            Some(b"WAVE") => Some("audio/wav"),
            Some(b"AVI ") => Some("video/x-msvideo"),
            _ => None,
        };
    }
    if at(4, b"ftyp") {
        return Some(match head.get(8..12) {
            Some(b"heic" | b"heix" | b"mif1" | b"msf1") => "image/heic",
            Some(b"avif") => "image/avif",
            Some(b"M4A " | b"M4B ") => "audio/mp4",
            Some(b"qt  ") => "video/quicktime",
            _ => "video/mp4",
        });
    }
    if starts(b"\x00\x00\x01\x00") || starts(b"\x00\x00\x02\x00") {
        return Some("image/x-icon");
    }
    if starts(b"ID3") || starts(b"\xff\xfb") || starts(b"\xff\xf3") || starts(b"\xff\xf2") {
        return Some("audio/mpeg");
    }
    if starts(b"fLaC") {
        return Some("audio/flac");
    }
    if starts(b"OggS") {
        return Some("audio/ogg");
    }
    if starts(b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) {
        return Some("audio/aiff");
    }
    if starts(b"\x1a\x45\xdf\xa3") {
        return Some("video/webm");
    }
    if starts(b"wOFF") {
        return Some("font/woff");
    }
    if starts(b"wOF2") {
        return Some("font/woff2");
    }
    if starts(b"OTTO") {
        return Some("font/otf");
    }
    if starts(b"\x00\x01\x00\x00\x00") {
        return Some("font/ttf");
    }
    if starts(b"8BPS") {
        return Some("image/vnd.adobe.photoshop");
    }
    if starts(b"II*\x00") || starts(b"MM\x00*") {
        return Some("image/tiff");
    }
    if starts(b"{\\rtf") {
        return Some("application/rtf");
    }
    if starts(b"PK\x03\x04") {
        return Some("application/zip");
    }
    let text = head.trim_ascii_start();
    let svg = |bytes: &[u8]| bytes.eq_ignore_ascii_case(b"<svg");
    if text.get(..4).is_some_and(svg) || (text.starts_with(b"<?xml") && head.windows(4).any(svg)) {
        return Some("image/svg+xml");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_ranges_parse() {
        assert_eq!(parse_range("bytes=0-"), Some((0, None)));
        assert_eq!(parse_range("bytes=10-19"), Some((10, Some(19))));
        assert_eq!(parse_range("bytes=-5"), None);
        assert_eq!(parse_range("items=0-1"), None);
    }

    #[test]
    fn percent_escapes_decode() {
        assert_eq!(percent_decode("a%20b"), Some("a b".into()));
        assert_eq!(percent_decode("%2FUsers%2Fme"), Some("/Users/me".into()));
        assert_eq!(percent_decode("%zz"), None);
        assert_eq!(percent_decode("%+f"), None);
    }

    #[test]
    fn names_decide_the_type() {
        use std::path::Path;
        assert_eq!(content_type(Path::new("/a/b.PDF")), "application/pdf");
        assert_eq!(content_type(Path::new("/a/song.m4a")), "audio/mp4");
        assert_eq!(
            content_type(Path::new("/a/noext")),
            "application/octet-stream"
        );
    }

    #[test]
    fn only_public_addresses_count_as_the_internet() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "fe80::1",
            "fd00::1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        for public in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn first_bytes_decide_the_type_of_unnamed_files() {
        assert_eq!(sniff(b"%PDF-1.7\n"), Some("application/pdf"));
        assert_eq!(sniff(b"\0\0\0\x20ftypheic"), Some("image/heic"));
        assert_eq!(sniff(b"\0\0\0\x20ftypisom"), Some("video/mp4"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some("audio/wav"));
        assert_eq!(sniff(b"ID3\x04"), Some("audio/mpeg"));
        assert_eq!(sniff(b"\x7fELF"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn bytes_name_themselves_and_a_page_of_html_names_nothing() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n rest"), Some("image/png"));
        assert_eq!(sniff(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(sniff(b"\x00\x00\x01\x00rest"), Some("image/x-icon"));
        assert_eq!(
            sniff(b"  <svg viewBox=\"0 0 1 1\"/>"),
            Some("image/svg+xml")
        );
        assert_eq!(sniff(b"<!doctype html><html>"), None);
    }
}
