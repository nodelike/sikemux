//! Request errors that mean the service could not be reached at all, worded
//! for the person, so every plugin says the same thing when the Mac is offline.

use std::error::Error as _;

use crate::Category;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unreachable {
    /// No connection could be made: no network, no DNS, or nothing answering.
    Offline,
    /// The connection was made but nothing came back in time.
    Timeout,
}

/// Words in an error's causes that mean the network, not the service, failed.
const UNREACHABLE_CAUSES: [&str; 7] = [
    "dns error",
    "failed to lookup address",
    "network is unreachable",
    "network is down",
    "no route to host",
    "connection refused",
    "not connected to the internet",
];

/// Whether a request failed for want of the network, and how.
pub fn unreachable(error: &reqwest::Error) -> Option<Unreachable> {
    if error.is_timeout() {
        return Some(Unreachable::Timeout);
    }
    if error.is_connect() {
        return Some(Unreachable::Offline);
    }
    let mut cause = error.source();
    while let Some(inner) = cause {
        let said = inner.to_string().to_lowercase();
        if UNREACHABLE_CAUSES.iter().any(|words| said.contains(words)) {
            return Some(Unreachable::Offline);
        }
        cause = inner.source();
    }
    None
}

/// The plain words for a service that could not be reached.
pub(crate) fn unreachable_message(kind: Unreachable, service: &str) -> String {
    match kind {
        Unreachable::Offline => format!("Can't reach {service}. Check your connection."),
        Unreachable::Timeout => format!("{service} didn't answer in time."),
    }
}

/// The error category a client reads to keep showing what it last loaded.
pub fn category(kind: Unreachable) -> Category {
    match kind {
        Unreachable::Offline => Category::Offline,
        Unreachable::Timeout => Category::Timeout,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_request_to_nowhere_reads_as_offline() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        let nothing_listening = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map(|address| format!("http://{address}/"))
            .expect("a free port");
        let error = client
            .get(nothing_listening)
            .send()
            .await
            .expect_err("nothing answers");
        assert_eq!(unreachable(&error), Some(Unreachable::Offline));
    }
}
