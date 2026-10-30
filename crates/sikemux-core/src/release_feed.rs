//! The feeds that name each channel's newest release, and the key every
//! release is signed with. The core reads them to update the app it runs in,
//! and root reads them to update a host's core.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use futures_util::StreamExt;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::Deserialize;
use serde_json::Value;

use crate::protocol::UpdateChannel;

const STABLE_FEED: &str =
    "https://github.com/nodelike/sikemux/releases/latest/download/latest.json";
const NIGHTLY_FEED: &str =
    "https://github.com/nodelike/sikemux/releases/download/nightly/latest.json";
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// The release scripts check updates against the key the app's Cargo.toml
/// names, so the core reads the same one.
const APP_MANIFEST: &str = include_str!("../../../app/Cargo.toml");

#[derive(Deserialize)]
pub struct Feed {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub platforms: HashMap<String, Asset>,
}

#[derive(Clone, Deserialize)]
pub struct Asset {
    pub signature: String,
    pub url: String,
}

/// A feed's newest release, and the feed as it was, which carries the
/// release's credits.
pub struct Newest {
    pub version: Version,
    pub feed: Feed,
    pub raw: Value,
}

/// What the app on this machine is called in a feed's platforms.
pub fn platform() -> String {
    let system = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{system}-{}", std::env::consts::ARCH)
}

/// What a host's core on this machine is called in a feed's platforms.
pub fn host_platform() -> String {
    format!("host-{}", platform())
}

fn feeds(channel: UpdateChannel) -> &'static [&'static str] {
    match channel {
        UpdateChannel::Stable => &[STABLE_FEED],
        // A stable release can overtake the newest nightly, so nightly follows both.
        UpdateChannel::Nightly => &[NIGHTLY_FEED, STABLE_FEED],
    }
}

async fn read_feed(url: &'static str) -> Result<Newest, String> {
    let client = crate::http::client().map_err(|error| error.to_string())?;
    let raw: Value = client
        .get(url)
        .header("Accept", "application/json")
        .timeout(CHECK_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let feed: Feed = serde_json::from_value(raw.clone()).map_err(|error| error.to_string())?;
    let version = Version::parse(feed.version.trim_start_matches('v'))
        .map_err(|error| format!("the feed's version: {error}"))?;
    Ok(Newest { version, feed, raw })
}

/// The newest release on `channel`. One feed that does not answer must not
/// hide a release the other one offers; it is an error only when none answers.
pub async fn newest(channel: UpdateChannel) -> Result<Newest, String> {
    let reads: Vec<_> = feeds(channel)
        .iter()
        .map(|url| tokio::spawn(read_feed(url)))
        .collect();
    let mut newest: Option<Newest> = None;
    let mut failure = None;
    for read in reads {
        match read
            .await
            .map_err(|error| error.to_string())
            .and_then(|read| read)
        {
            Ok(read)
                if newest
                    .as_ref()
                    .is_none_or(|best| read.version > best.version) =>
            {
                newest = Some(read);
            }
            Ok(_) => {}
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    newest.ok_or_else(|| failure.unwrap_or_else(|| "no feed answered".into()))
}

/// The key every release is signed with.
pub fn release_key() -> Result<PublicKey, String> {
    let manifest: toml::Value = toml::from_str(APP_MANIFEST).map_err(|error| error.to_string())?;
    let encoded = manifest
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|metadata| metadata.get("sikemux"))
        .and_then(|app| app.get("updater-key"))
        .and_then(toml::Value::as_str)
        .ok_or("the app's Cargo.toml has no update key")?;
    let text = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| error.to_string())?;
    PublicKey::decode(&String::from_utf8_lossy(&text)).map_err(|error| error.to_string())
}

/// Downloads `url` to `to`, checking it against the release's `signature` as
/// it comes, and reports each step as bytes done of the whole. Answers with
/// the signature's trusted comment, which names the file and version it was
/// made for.
pub async fn fetch_verified(
    url: &str,
    signature: &str,
    key: &PublicKey,
    to: &Path,
    max_bytes: Option<u64>,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<String, String> {
    let text = base64::engine::general_purpose::STANDARD
        .decode(signature.trim())
        .map_err(|error| format!("the release's signature: {error}"))?;
    let signature = Signature::decode(&String::from_utf8_lossy(&text))
        .map_err(|error| format!("the release's signature: {error}"))?;
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|_| "the release is signed with another key".to_owned())?;
    let client = crate::http::client().map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("download: {error}"))?;
    let total = response.content_length();
    let too_big = || "the download is larger than any release would be".to_owned();
    if total.zip(max_bytes).is_some_and(|(total, max)| total > max) {
        return Err(too_big());
    }
    let mut file = std::fs::File::create(to).map_err(|error| error.to_string())?;
    let mut done = 0u64;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| format!("download: {error}"))?;
        done += chunk.len() as u64;
        if max_bytes.is_some_and(|max| done > max) {
            return Err(too_big());
        }
        verifier.update(&chunk);
        file.write_all(&chunk).map_err(|error| error.to_string())?;
        progress(done, total);
    }
    file.flush().map_err(|error| error.to_string())?;
    verifier
        .finalize()
        .map_err(|_| "the download does not match the release's signature".to_owned())?;
    Ok(signature.trusted_comment().to_owned())
}

/// Whether a trusted comment says its signature was made for `file` at
/// `version`, as the release scripts sign every file.
pub fn signed_for(trusted_comment: &str, file: &str, version: &Version) -> bool {
    let fields: Vec<&str> = trusted_comment.split('\t').collect();
    fields.contains(&format!("file:{file}").as_str())
        && fields.contains(&format!("version:{version}").as_str())
}
