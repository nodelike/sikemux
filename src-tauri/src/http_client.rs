//! The app's HTTP clients, and fetching a remote image for the window, which
//! only draws images handed to it as `data:` URLs.

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use futures::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::{Attempt, Policy};
use reqwest::{Client, ClientBuilder, RequestBuilder};
use url::Url;

use crate::error::AppResult;
use crate::file_serving::sniff;

/// A server can answer from several addresses and a network can silently drop one.
/// Without this a request waits on that address for its whole timeout instead of trying the next.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: usize = 5;

/// Requests that need a deadline set one with `RequestBuilder::timeout`, so
/// long downloads are only cut off when the server goes quiet.
pub fn client() -> AppResult<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    shared(&CLIENT, builder)
}

/// Names resolve only to public addresses and the connection goes to the
/// address that was checked, so a name cannot be pointed at this machine or
/// its network between the check and the request. An address written into
/// the URL skips name lookup, so callers check it with `names_private_address`.
pub fn public_client() -> AppResult<&'static Client> {
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
) -> AppResult<&'static Client> {
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
    let mime = sniff(&body).filter(|mime| mime.starts_with("image/"))?;
    Some((body, mime))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
