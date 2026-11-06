//! Request errors that mean the service could not be reached at all, worded
//! for the person, so every plugin says the same thing when the Mac is offline.

use sikemux_base::http::Unreachable;

use crate::Category;

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
    #[allow(clippy::expect_used)]
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
        assert_eq!(
            sikemux_base::http::unreachable(&error),
            Some(Unreachable::Offline)
        );
    }
}
