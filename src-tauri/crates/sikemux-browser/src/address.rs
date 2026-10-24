//! What someone means by what they type in the address bar or hand to
//! `browser_navigate`: a file on this computer, a web address, or a search.

use std::net::IpAddr;
use std::path::PathBuf;

use url::Url;

use super::BLANK_URL;

#[derive(Debug, PartialEq)]
pub enum Address {
    Local(PathBuf),
    Url(String),
    Search(String),
}

pub fn resolve(input: &str) -> Address {
    let value = input.trim();
    if value.is_empty() {
        return Address::Url(BLANK_URL.into());
    }
    if value.len() >= 5 && value[..5].eq_ignore_ascii_case("file:") {
        return match Url::parse(value)
            .ok()
            .and_then(|url| url.to_file_path().ok())
        {
            Some(path) => Address::Local(path),
            None => Address::Search(super::history::search_url(value)),
        };
    }
    if value == "~" || value.starts_with("~/") || value.starts_with('/') {
        return Address::Local(sikemux_process::paths::expand_home(value));
    }
    if value == BLANK_URL || value.contains("://") {
        return Address::Url(value.to_owned());
    }
    if value.contains(char::is_whitespace) {
        return Address::Search(super::history::search_url(value));
    }
    let host = host_of(value);
    if is_local_host(host) {
        return Address::Url(format!("http://{value}"));
    }
    if host.contains('.') {
        return Address::Url(format!("https://{value}"));
    }
    Address::Search(super::history::search_url(value))
}

/// The host of a typed address with no scheme, without its port.
fn host_of(value: &str) -> &str {
    let authority = value.split(['/', '?', '#']).next().unwrap_or_default();
    if let Some(bracketed) = authority.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or_default();
    }
    authority.split(':').next().unwrap_or_default()
}

/// A name or address that only reaches this computer or its local network,
/// whose servers rarely speak https.
pub fn is_local_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            ip.is_loopback() || ip.is_unspecified() || ip.is_private() || ip.is_link_local()
        }
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve, Address};
    use std::path::PathBuf;

    #[test]
    fn typed_addresses_name_files_pages_or_searches() {
        let url = |text: &str| Address::Url(text.into());
        assert_eq!(resolve("  "), url(super::BLANK_URL));
        assert_eq!(resolve("example.com"), url("https://example.com"));
        assert_eq!(resolve("localhost:3000/x"), url("http://localhost:3000/x"));
        assert_eq!(resolve("0.0.0.0:8080"), url("http://0.0.0.0:8080"));
        assert_eq!(
            resolve("192.168.1.20:5173"),
            url("http://192.168.1.20:5173")
        );
        assert_eq!(resolve("[::1]:3000"), url("http://[::1]:3000"));
        assert_eq!(resolve("http://a.b"), url("http://a.b"));
        assert_eq!(
            resolve("rust lifetimes"),
            Address::Search("https://www.google.com/search?q=rust+lifetimes".into())
        );
        assert_eq!(
            resolve("file:///tmp/a%20b.html"),
            Address::Local(PathBuf::from("/tmp/a b.html"))
        );
        assert_eq!(
            resolve("FILE:///tmp/x"),
            Address::Local(PathBuf::from("/tmp/x"))
        );
        assert_eq!(resolve(" /tmp/x "), Address::Local(PathBuf::from("/tmp/x")));
        assert!(
            matches!(resolve("~/x"), Address::Local(path) if path.ends_with("x") && path.is_absolute())
        );
        assert!(matches!(resolve("relative/path.html"), Address::Search(_)));
    }
}
