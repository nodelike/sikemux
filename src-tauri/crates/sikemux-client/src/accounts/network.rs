//! `GET /v1/network`: the relays hosts and phones meet through, and the oldest
//! app the accounts server still works with.
//!
//! A host and the phones that dial it must use the same relay, so a failed
//! fetch falls back to the last copy and then to [`DEFAULT_RELAY`], never to
//! iroh's public relays.

use std::sync::Arc;
use std::time::Duration;

use iroh::{RelayConfig, RelayMap, RelayUrl};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;

use super::protocol::{Channel, ChannelVersions, Network, Relay};

pub const DEFAULT_RELAY: &str = "https://relay.sikemux.com/";
pub const DEFAULT_QUIC_PORT: u16 = 7842;

const FETCH_TIMEOUT: Duration = Duration::from_secs(3);
const LARGEST_RESPONSE: u64 = 64 * 1024;

pub fn default_relays() -> Vec<Relay> {
    vec![Relay {
        url: DEFAULT_RELAY.into(),
        region: "default".into(),
        quic_port: Some(DEFAULT_QUIC_PORT.into()),
    }]
}

/// The relays iroh can use, best first. A relay whose address does not parse
/// is left out.
pub fn relay_configs(relays: &[Relay]) -> Vec<RelayConfig> {
    relays
        .iter()
        .filter_map(|relay| {
            let url: RelayUrl = relay.url.parse().ok()?;
            let mut config = RelayConfig::from(url);
            match relay.quic_port.and_then(|port| u16::try_from(port).ok()) {
                Some(port) => {
                    if let Some(quic) = config.quic.as_mut() {
                        quic.port = port;
                    }
                }
                None => config.quic = None,
            }
            Some(config)
        })
        .collect()
}

pub fn relay_map(relays: &[Relay]) -> RelayMap {
    relay_configs(relays).into_iter().collect()
}

/// The relays to use from a copy of the network, or `None` when it names none
/// iroh can use.
pub fn usable_relays(network: &Network) -> Option<Vec<Relay>> {
    let usable: Vec<Relay> = network
        .relays
        .iter()
        .filter(|relay| relay.url.parse::<RelayUrl>().is_ok())
        .cloned()
        .collect();
    (!usable.is_empty()).then_some(usable)
}

/// A version with a prerelease tag, like `0.5.0-nightly.3`, is a nightly.
pub fn channel_of(version: &str) -> Channel {
    if version.contains('-') {
        Channel::Nightly
    } else {
        Channel::Stable
    }
}

/// The oldest version allowed for `version`'s channel, when `version` is older
/// than it. A version that does not parse is never too old, and a minimum of
/// `0.0.0`, the server's default, allows every version.
pub fn too_old(version: &str, minimum: &ChannelVersions) -> Option<String> {
    let oldest = match channel_of(version) {
        Channel::Nightly => &minimum.nightly,
        _ => &minimum.stable,
    };
    let current = semver::Version::parse(version).ok()?;
    let allowed = semver::Version::parse(oldest).ok()?;
    let any = semver::Version::new(0, 0, 0);
    (allowed != any && current < allowed).then(|| oldest.clone())
}

/// Reads `/v1/network` from the accounts API at `base`, giving up after three
/// seconds.
pub async fn fetch(base: &str) -> Option<Network> {
    let url = format!("{}/v1/network", base.trim_end_matches('/'));
    let body = tokio::time::timeout(FETCH_TIMEOUT, get(&url))
        .await
        .ok()??;
    serde_json::from_slice(&body).ok()
}

async fn get(url: &str) -> Option<Vec<u8>> {
    let url = url::Url::parse(url).ok()?;
    let host = url.host_str()?.to_owned();
    let port = url.port_or_known_default()?;
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.clone(),
    };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {authority}\r\nAccept: application/json\r\nUser-Agent: sikemux\r\nConnection: close\r\n\r\n",
        url.path()
    );
    let tcp = TcpStream::connect((host.trim_matches(['[', ']']), port))
        .await
        .ok()?;
    match url.scheme() {
        "https" => {
            let name = ServerName::try_from(host).ok()?;
            let connector = tokio_rustls::TlsConnector::from(super::tls_config()?);
            let mut tls = connector.connect(name, tcp).await.ok()?;
            exchange(&mut tls, &request).await
        }
        "http" => {
            let mut tcp = tcp;
            exchange(&mut tcp, &request).await
        }
        _ => None,
    }
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    request: &str,
) -> Option<Vec<u8>> {
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut response = Vec::new();
    // A server that closes without TLS's goodbye ends the read with an error;
    // what arrived before it is still the whole answer when its length checks out.
    let _ = (&mut *stream)
        .take(LARGEST_RESPONSE)
        .read_to_end(&mut response)
        .await;
    body_of(&response)
}

/// The body of a complete `200 OK`, sized or chunked.
fn body_of(response: &[u8]) -> Option<Vec<u8>> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?;
    let head = std::str::from_utf8(response.get(..split)?).ok()?;
    let body = response.get(split + 4..)?;
    let mut lines = head.split("\r\n");
    let status = lines.next()?;
    if status.split(' ').nth(1) != Some("200") {
        return None;
    }
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.parse::<usize>().ok()?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case("chunked");
        }
    }
    if chunked {
        return dechunk(body);
    }
    match length {
        Some(length) => body.get(..length).map(<[u8]>::to_vec),
        None => Some(body.to_vec()),
    }
}

fn dechunk(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let end = body.windows(2).position(|window| window == b"\r\n")?;
        let size = std::str::from_utf8(body.get(..end)?).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        body = body.get(end + 2..)?;
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(body.get(..size)?);
        body = body.get(size + 2..)?;
    }
}

/// What to change on a running endpoint to move it from `old` to `new`.
pub fn relay_changes(old: &[Relay], new: &[Relay]) -> (Vec<RelayUrl>, Vec<Arc<RelayConfig>>) {
    let old = relay_configs(old);
    let new = relay_configs(new);
    let removed = old
        .iter()
        .filter(|config| !new.iter().any(|kept| kept.url == config.url))
        .map(|config| config.url.clone())
        .collect();
    let added = new
        .into_iter()
        .filter(|config| !old.contains(config))
        .map(Arc::new)
        .collect();
    (removed, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(url: &str, quic_port: Option<i64>) -> Relay {
        Relay {
            url: url.into(),
            region: "test".into(),
            quic_port,
        }
    }

    fn versions(nightly: &str, stable: &str) -> ChannelVersions {
        ChannelVersions {
            nightly: nightly.into(),
            stable: stable.into(),
        }
    }

    #[test]
    fn relays_keep_their_quic_port_or_go_without() {
        let configs = relay_configs(&[
            relay("https://relay.sikemux.com/", Some(7900)),
            relay("https://other.example/", None),
            relay("not a url", Some(1)),
        ]);
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].quic.as_ref().map(|quic| quic.port), Some(7900));
        assert_eq!(configs[1].quic, None);
        assert_eq!(relay_map(&default_relays()).len(), 1);
    }

    #[test]
    fn a_network_without_a_usable_relay_is_not_used() {
        let network = |relays| Network {
            relays,
            minimum_versions: crate::accounts::protocol::MinimumVersions {
                macos: versions("0.0.0", "0.0.0"),
                ios: versions("0.0.0", "0.0.0"),
                android: versions("0.0.0", "0.0.0"),
            },
        };
        assert_eq!(usable_relays(&network(vec![])), None);
        assert_eq!(usable_relays(&network(vec![relay("nope", None)])), None);
        let kept = usable_relays(&network(vec![
            relay("nope", None),
            relay(DEFAULT_RELAY, None),
        ]));
        assert_eq!(kept, Some(vec![relay(DEFAULT_RELAY, None)]));
    }

    #[test]
    fn versions_compare_within_their_channel() {
        let minimum = versions("0.5.0-nightly.4", "0.4.2");
        assert_eq!(
            too_old("0.5.0-nightly.3", &minimum),
            Some("0.5.0-nightly.4".into())
        );
        assert_eq!(too_old("0.5.0-nightly.10", &minimum), None);
        assert_eq!(too_old("0.4.1", &minimum), Some("0.4.2".into()));
        assert_eq!(too_old("0.4.2", &minimum), None);
        assert_eq!(too_old("0.0.0-test", &versions("0.0.0", "0.0.0")), None);
        assert_eq!(too_old("", &minimum), None);
        assert_eq!(channel_of("0.5.0-nightly.1"), Channel::Nightly);
        assert_eq!(channel_of("0.5.0"), Channel::Stable);
    }

    #[test]
    fn moving_relays_removes_only_what_left() {
        let old = [
            relay("https://a.example/", Some(7842)),
            relay("https://b.example/", None),
        ];
        let new = [
            relay("https://b.example/", None),
            relay("https://c.example/", None),
        ];
        let (removed, added) = relay_changes(&old, &new);
        assert_eq!(
            removed,
            vec!["https://a.example/".parse::<RelayUrl>().unwrap()]
        );
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].url.as_str(), "https://c.example/");
        let (removed, added) = relay_changes(&new, &new);
        assert!(removed.is_empty() && added.is_empty());
    }

    #[test]
    fn bodies_read_sized_or_chunked() {
        let sized = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}extra";
        assert_eq!(body_of(sized), Some(b"{}".to_vec()));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{\"a\r\n3\r\n\":1\r\n1\r\n}\r\n0\r\n\r\n";
        assert_eq!(body_of(chunked), Some(br#"{"a":1}"#.to_vec()));
        assert_eq!(body_of(b"HTTP/1.1 304 Not Modified\r\n\r\n"), None);
        assert_eq!(
            body_of(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n{}"),
            None
        );
    }

    /// Against a stand-in accounts server on this computer.
    #[tokio::test]
    async fn fetches_the_network_over_plain_http() {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let fixture = include_str!("../../../../../server/protocol/fixtures/Network/default.json");
        let serving = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 1024];
            let read = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{fixture}",
                fixture.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        let network = fetch(&base).await.expect("the network");
        assert_eq!(network.relays[0].url, DEFAULT_RELAY);
        assert_eq!(network.relays[0].quic_port, Some(7842));
        assert!(serving
            .await
            .unwrap()
            .starts_with("GET /v1/network HTTP/1.1\r\n"));
    }
}
