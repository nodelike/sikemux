// Hardened HTTP runner for Bruno collections. Bruno remains a capable API
// client, but risky local-network and filesystem access must be explicitly
// trusted by the frontend and every request is bounded in time and size.

use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE, COOKIE, PROXY_AUTHORIZATION,
};
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};

use crate::error::{BrunoError, BrunoResult};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;
const MAX_BODY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_HEADERS: usize = 256;
const MAX_HEADER_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BruBodyWire {
    None,
    Raw {
        content_type: Option<String>,
        data: String,
    },
    File {
        path: String,
        content_type: Option<String>,
    },
    Form {
        fields: Vec<(String, String)>,
    },
    Multipart {
        fields: Vec<MultipartField>,
    },
}

#[derive(Deserialize)]
pub struct MultipartField {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub is_file: bool,
}

#[derive(Default, Deserialize)]
pub struct BruTrust {
    #[serde(default)]
    pub allow_private_network: bool,
    #[serde(default)]
    pub allow_file_read: bool,
    #[serde(default)]
    pub allow_insecure_tls: bool,
    /// Canonical collection root. File bodies must remain beneath it.
    pub file_root: Option<String>,
}

#[derive(Deserialize)]
pub struct BruSendRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub body: BruBodyWire,
    #[serde(default)]
    pub timeout_ms: u64,
    #[serde(default)]
    pub skip_tls_verify: bool,
    #[serde(default)]
    pub trust: BruTrust,
}

#[derive(Serialize)]
pub struct BruSendResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub is_binary: bool,
    pub size_bytes: u64,
    pub duration_ms: u64,
}

fn is_non_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240
        }
        IpAddr::V6(ip) => {
            ip.to_ipv4_mapped()
                .is_some_and(|mapped| is_non_public_ip(mapped.into()))
                || ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || ip.segments()[..2] == [0x2001, 0x0db8]
        }
    }
}

fn is_private_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(is_non_public_ip)
}

async fn resolve_target(url: &Url, trust: &BruTrust) -> BrunoResult<Vec<SocketAddr>> {
    let host = url
        .host_str()
        .ok_or(BrunoError::BadArg("URL host is required"))?;
    let port = url
        .port_or_known_default()
        .ok_or(BrunoError::BadArg("URL port is required"))?;
    let addrs: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| BrunoError::Http(format!("failed to resolve request host: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(BrunoError::Http("request host did not resolve".into()));
    }
    if !trust.allow_private_network && addrs.iter().any(|addr| is_non_public_ip(addr.ip())) {
        return Err(BrunoError::BadArg(
            "host resolves to a local/private address and requires trusting this Bruno collection",
        ));
    }
    Ok(addrs)
}

fn validate_url(raw: &str, trust: &BruTrust) -> BrunoResult<Url> {
    let url = Url::parse(raw.trim()).map_err(|_| BrunoError::BadArg("invalid URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(BrunoError::BadArg("only http(s) URLs are supported"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(BrunoError::BadArg(
            "credentials in URLs are not allowed; use an Authorization header",
        ));
    }
    if is_private_host(&url) && !trust.allow_private_network {
        return Err(BrunoError::BadArg(
            "local/private endpoint requires trusting this Bruno collection",
        ));
    }
    Ok(url)
}

fn build_headers(pairs: &[(String, String)], url: &Url) -> BrunoResult<HeaderMap> {
    if pairs.len() > MAX_HEADERS {
        return Err(BrunoError::BadArg("too many request headers"));
    }
    let total_bytes = pairs.iter().try_fold(0_usize, |total, (name, value)| {
        total.checked_add(name.len())?.checked_add(value.len())
    });
    if total_bytes.is_none_or(|total| total > MAX_HEADER_BYTES) {
        return Err(BrunoError::BadArg("request headers exceed 1 MiB limit"));
    }
    let mut map = HeaderMap::new();
    for (k, v) in pairs {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| BrunoError::BadArg("invalid request header name"))?;
        let val = HeaderValue::from_str(v)
            .map_err(|_| BrunoError::BadArg("invalid request header value"))?;
        map.append(name, val);
    }
    if url.scheme() == "http"
        && (map.contains_key(AUTHORIZATION)
            || map.contains_key(PROXY_AUTHORIZATION)
            || map.contains_key(COOKIE))
        && !is_private_host(url)
    {
        return Err(BrunoError::BadArg(
            "refusing to send credentials over plaintext HTTP",
        ));
    }
    Ok(map)
}

fn canonical_upload(path: &str, trust: &BruTrust) -> BrunoResult<PathBuf> {
    if !trust.allow_file_read {
        return Err(BrunoError::BadArg(
            "file upload requires trusting this Bruno collection",
        ));
    }
    let root = trust
        .file_root
        .as_deref()
        .ok_or(BrunoError::BadArg("trusted collection root missing"))?;
    let root =
        std::fs::canonicalize(root).map_err(|_| BrunoError::BadArg("invalid collection root"))?;
    let file = std::fs::canonicalize(path)
        .map_err(|_| BrunoError::BadArg("upload file does not exist"))?;
    if !file.starts_with(&root) || !Path::new(&file).is_file() {
        return Err(BrunoError::BadArg(
            "upload file must be a regular file inside the collection",
        ));
    }
    let len = std::fs::metadata(&file)?.len();
    if len > MAX_BODY_BYTES {
        return Err(BrunoError::BadArg("upload file exceeds 32 MiB limit"));
    }
    Ok(file)
}

fn read_upload(path: &str, trust: &BruTrust) -> BrunoResult<(PathBuf, Vec<u8>)> {
    let path = canonical_upload(path, trust)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_BODY_BYTES {
        return Err(BrunoError::BadArg("upload file exceeds 32 MiB limit"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_BODY_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BODY_BYTES {
        return Err(BrunoError::BadArg("upload file exceeds 32 MiB limit"));
    }
    Ok((path, bytes))
}

/// How many distinct targets keep a warm client. A collection run hits a
/// handful of hosts; past that the oldest set is simply dropped.
const MAX_CACHED_CLIENTS: usize = 16;

fn client_cache() -> &'static Mutex<HashMap<String, Client>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Client>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A client per target rather than per request. Building one each time meant a
/// fresh connection pool and a fresh TLS handshake for every call, so running a
/// folder of requests reused nothing.
///
/// The key carries everything the builder is configured from, including the
/// pinned addresses, so a different DNS answer never reuses the old client.
fn client(
    trust: &BruTrust,
    skip_tls_verify: bool,
    url: &Url,
    addrs: &[SocketAddr],
) -> BrunoResult<Client> {
    if skip_tls_verify && !trust.allow_insecure_tls {
        return Err(BrunoError::BadArg(
            "invalid TLS certificates require explicit trust",
        ));
    }
    let host = url
        .host_str()
        .ok_or(BrunoError::BadArg("URL host is required"))?;

    let mut pinned: Vec<String> = addrs.iter().map(SocketAddr::to_string).collect();
    pinned.sort();
    let key = format!("{skip_tls_verify}|{host}|{}", pinned.join(","));
    let mut cache = client_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(client) = cache.get(&key) {
        return Ok(client.clone());
    }

    let client = Client::builder()
        .danger_accept_invalid_certs(skip_tls_verify)
        // Return redirects to the collection instead of following them. This
        // prevents cross-origin credential forwarding and DNS-rebinding SSRF.
        .redirect(reqwest::redirect::Policy::none())
        // Pin this request to the addresses checked by resolve_target so the
        // connection cannot perform a second, different DNS resolution.
        .resolve_to_addrs(host, addrs)
        // Deliberately no process-wide cookie jar: collections must send explicit
        // Cookie headers, preventing cross-collection ambient credential leaks.
        .user_agent("sikemux-bruno/0.1")
        .build()?;
    if cache.len() >= MAX_CACHED_CLIENTS {
        cache.clear();
    }
    cache.insert(key, client.clone());
    Ok(client)
}

pub async fn send(req: BruSendRequest) -> BrunoResult<BruSendResponse> {
    let method = Method::from_bytes(req.method.to_uppercase().as_bytes())
        .map_err(|_| BrunoError::BadArg("invalid HTTP method"))?;
    let url = validate_url(&req.url, &req.trust)?;
    let mut headers = build_headers(&req.headers, &url)?;
    let addrs = resolve_target(&url, &req.trust).await?;
    let cl = client(&req.trust, req.skip_tls_verify, &url, &addrs)?;
    let timeout_ms = if req.timeout_ms == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        req.timeout_ms.min(MAX_TIMEOUT_MS)
    };
    let mut builder = cl
        .request(method, url)
        .timeout(Duration::from_millis(timeout_ms));

    match req.body {
        BruBodyWire::None => {}
        BruBodyWire::Raw { content_type, data } => {
            if data.len() as u64 > MAX_BODY_BYTES {
                return Err(BrunoError::BadArg("request body exceeds 32 MiB limit"));
            }
            set_content_type(&mut headers, content_type);
            builder = builder.body(data);
        }
        BruBodyWire::File { path, content_type } => {
            set_content_type(&mut headers, content_type);
            let (_, bytes) = read_upload(&path, &req.trust)?;
            builder = builder.body(bytes);
        }
        BruBodyWire::Form { fields } => {
            if fields.len() > 10_000 {
                return Err(BrunoError::BadArg("too many form fields"));
            }
            let encoded = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(
                    fields
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.as_str())),
                )
                .finish();
            if encoded.len() as u64 > MAX_BODY_BYTES {
                return Err(BrunoError::BadArg("form body exceeds 32 MiB limit"));
            }
            set_content_type(
                &mut headers,
                Some("application/x-www-form-urlencoded".into()),
            );
            builder = builder.body(encoded);
        }
        BruBodyWire::Multipart { fields } => {
            if fields.len() > 1_000 {
                return Err(BrunoError::BadArg("too many multipart fields"));
            }
            let mut form = reqwest::multipart::Form::new();
            let mut total = 0_u64;
            for f in fields {
                if f.is_file {
                    let (path, bytes) = read_upload(&f.value, &req.trust)?;
                    total = total
                        .checked_add(f.name.len() as u64)
                        .and_then(|n| n.checked_add(bytes.len() as u64))
                        .ok_or(BrunoError::BadArg("multipart body exceeds 32 MiB limit"))?;
                    if total > MAX_BODY_BYTES {
                        return Err(BrunoError::BadArg("multipart body exceeds 32 MiB limit"));
                    }
                    let filename = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("file")
                        .to_string();
                    form = form.part(
                        f.name,
                        reqwest::multipart::Part::bytes(bytes).file_name(filename),
                    );
                } else {
                    total = total
                        .checked_add(f.name.len() as u64)
                        .and_then(|n| n.checked_add(f.value.len() as u64))
                        .ok_or(BrunoError::BadArg("multipart body exceeds 32 MiB limit"))?;
                    if total > MAX_BODY_BYTES {
                        return Err(BrunoError::BadArg("multipart body exceeds 32 MiB limit"));
                    }
                    form = form.text(f.name, f.value);
                }
            }
            builder = builder.multipart(form);
        }
    }

    builder = builder.headers(headers);
    let started = Instant::now();
    let resp = builder
        .send()
        .await
        .map_err(|e| BrunoError::Http(e.to_string()))?;
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES)
    {
        return Err(BrunoError::Http("response exceeds 32 MiB limit".into()));
    }
    let status = resp.status();
    let status_text = status.canonical_reason().unwrap_or("").to_string();
    let response_header_bytes = resp
        .headers()
        .iter()
        .try_fold(0_usize, |total, (name, value)| {
            total
                .checked_add(name.as_str().len())?
                .checked_add(value.as_bytes().len())
        });
    if response_header_bytes.is_none_or(|total| total > MAX_HEADER_BYTES) {
        return Err(BrunoError::Http(
            "response headers exceed 1 MiB limit".into(),
        ));
    }
    let resp_headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let mut stream = resp.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| BrunoError::Http(e.to_string()))?;
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES as usize {
            return Err(BrunoError::Http("response exceeds 32 MiB limit".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let size_bytes = bytes.len() as u64;
    let is_binary = std::str::from_utf8(&bytes).is_err();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    Ok(BruSendResponse {
        status: status.as_u16(),
        status_text,
        headers: resp_headers,
        body,
        is_binary,
        size_bytes,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn set_content_type(headers: &mut HeaderMap, value: Option<String>) {
    if !headers.contains_key(CONTENT_TYPE) {
        if let Some(value) = value.and_then(|v| HeaderValue::from_str(&v).ok()) {
            headers.insert(CONTENT_TYPE, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn trust(private: bool) -> BruTrust {
        BruTrust {
            allow_private_network: private,
            ..Default::default()
        }
    }

    #[test]
    fn rejects_unsafe_schemes_and_url_credentials() {
        assert!(validate_url("file:///etc/passwd", &trust(true)).is_err());
        assert!(validate_url("https://user:pass@example.com", &trust(false)).is_err());
    }

    #[test]
    fn private_endpoints_require_explicit_trust() {
        assert!(validate_url("http://127.0.0.1:8080", &trust(false)).is_err());
        assert!(validate_url("http://localhost:8080", &trust(true)).is_ok());
        assert!(validate_url("https://example.com", &trust(false)).is_ok());
    }

    #[test]
    fn classifies_non_public_address_ranges() {
        for raw in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.1.1",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_non_public_ip(raw.parse().unwrap()), "{raw}");
        }
        assert!(!is_non_public_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_non_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn plaintext_remote_credentials_are_blocked() {
        let url = Url::parse("http://example.com").unwrap();
        assert!(build_headers(&[("Authorization".into(), "Bearer secret".into())], &url).is_err());
    }

    #[test]
    fn oversized_headers_are_blocked() {
        let url = Url::parse("https://example.com").unwrap();
        let huge = "x".repeat(MAX_HEADER_BYTES + 1);
        assert!(build_headers(&[("X-Large".into(), huge)], &url).is_err());
    }

    #[test]
    fn uploads_cannot_escape_collection_root() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("body.txt");
        std::fs::write(&inside, "ok").unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let t = BruTrust {
            allow_file_read: true,
            file_root: Some(root.path().to_string_lossy().into()),
            ..Default::default()
        };
        assert!(canonical_upload(inside.to_str().unwrap(), &t).is_ok());
        assert!(canonical_upload(outside.path().to_str().unwrap(), &t).is_err());
    }
}
