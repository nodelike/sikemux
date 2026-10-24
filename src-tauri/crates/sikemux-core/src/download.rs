//! Files a build fetches once and keeps, such as the helpers each release
//! publishes. Each is checked against the size and hash recorded when the app
//! was built before it is moved into place, and a download that was cut off
//! carries on from where it stopped.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::http::client;

/// Why a download stopped: the request itself failed, which the caller can tell a
/// lost network from, or the file came wrong or could not be kept.
#[derive(Debug)]
pub enum DownloadError {
    Request(reqwest::Error),
    Failed(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Request(error) => error.fmt(f),
            DownloadError::Failed(message) => f.write_str(message),
        }
    }
}

impl From<reqwest::Error> for DownloadError {
    fn from(error: reqwest::Error) -> Self {
        DownloadError::Request(error)
    }
}

impl From<std::io::Error> for DownloadError {
    fn from(error: std::io::Error) -> Self {
        DownloadError::Failed(error.to_string())
    }
}

/// A file as the build recorded it: where it is published, its size and its SHA-256.
#[derive(Clone, Debug, Deserialize)]
pub struct PinnedFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

pub fn hash_file(path: &Path) -> std::io::Result<String> {
    Ok(hex::encode(hasher_over(path)?.finalize()))
}

fn hasher_over(path: &Path) -> std::io::Result<Sha256> {
    let mut reader = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher);
        }
        hasher.update(&buffer[..read]);
    }
}

/// Whether the file at `path` is `file`, hashing it off the async threads.
pub async fn matches(path: &Path, file: &PinnedFile) -> bool {
    if std::fs::metadata(path).map(|meta| meta.len()).ok() != Some(file.size) {
        return false;
    }
    let path = path.to_path_buf();
    let hash = tokio::task::spawn_blocking(move || hash_file(&path)).await;
    matches!(hash, Ok(Ok(hash)) if hash == file.sha256)
}

pub async fn download(
    url: &str,
    destination: &Path,
    file: &PinnedFile,
    progress: impl FnMut(u64),
) -> Result<(), DownloadError> {
    download_as(url, destination, file, progress, false).await
}

/// Downloads a program, which can be run the moment it appears at `destination`.
pub async fn download_executable(
    url: &str,
    destination: &Path,
    file: &PinnedFile,
    progress: impl FnMut(u64),
) -> Result<(), DownloadError> {
    download_as(url, destination, file, progress, true).await
}

async fn download_as(
    url: &str,
    destination: &Path,
    file: &PinnedFile,
    mut progress: impl FnMut(u64),
    executable: bool,
) -> Result<(), DownloadError> {
    let parent = destination
        .parent()
        .ok_or_else(|| DownloadError::Failed(format!("{} has no parent", destination.display())))?;
    std::fs::create_dir_all(parent)?;
    let partial = PathBuf::from(format!("{}.partial", destination.display()));
    write_verified(url, &partial, file, &mut progress).await?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(std::fs::rename(&partial, destination)?)
}

/// Downloads into `partial`, carrying on from whatever an earlier try left
/// there. A file that turns out wrong is deleted so the next try starts clean.
async fn write_verified(
    url: &str,
    partial: &Path,
    file: &PinnedFile,
    progress: &mut impl FnMut(u64),
) -> Result<(), DownloadError> {
    let kept = std::fs::metadata(partial)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let mut request = client()?.get(url);
    if kept > 0 && kept < file.size {
        request = request.header(reqwest::header::RANGE, format!("bytes={kept}-"));
    }
    let response = request.send().await?;
    let resumed = kept > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let response = response.error_for_status()?;
    let (mut out, mut hasher, mut written) = if resumed {
        let earlier = partial.to_path_buf();
        let hasher = tokio::task::spawn_blocking(move || hasher_over(&earlier))
            .await
            .map_err(|error| DownloadError::Failed(error.to_string()))??;
        let out = std::fs::OpenOptions::new().append(true).open(partial)?;
        (out, hasher, kept)
    } else {
        (File::create(partial)?, Sha256::new(), 0)
    };
    progress(written);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        written += chunk.len() as u64;
        if written > file.size {
            let _ = std::fs::remove_file(partial);
            return Err(DownloadError::Failed(format!(
                "{} is larger than expected",
                file.path
            )));
        }
        hasher.update(&chunk);
        out.write_all(&chunk)?;
        progress(written);
    }
    out.sync_all()?;
    if written < file.size {
        return Err(DownloadError::Failed(format!(
            "{} stopped downloading early",
            file.path
        )));
    }
    if hex::encode(hasher.finalize()) != file.sha256 {
        let _ = std::fs::remove_file(partial);
        return Err(DownloadError::Failed(format!(
            "{} did not match the expected hash",
            file.path
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_download_cut_off_halfway_carries_on_from_where_it_stopped() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let body: Vec<u8> = (0..64 * 1024).map(|index| (index % 251) as u8).collect();
        let half = body.len() / 2;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
        let served = body.clone();
        let server = tokio::spawn(async move {
            let mut asked = Vec::new();
            for attempt in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0u8; 4096];
                let read = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..read]).to_lowercase();
                let range = request
                    .lines()
                    .find_map(|line| line.strip_prefix("range: bytes="))
                    .map(|from| from.trim_end_matches('-').parse::<usize>().unwrap());
                asked.push(range);
                let head = match range {
                    Some(from) => format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{}/{}\r\n\r\n",
                        served.len() - from,
                        served.len() - 1,
                        served.len()
                    ),
                    None => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", served.len()),
                };
                socket.write_all(head.as_bytes()).await.unwrap();
                let from = range.unwrap_or(0);
                let to = if attempt == 0 { half } else { served.len() };
                socket.write_all(&served[from..to]).await.unwrap();
                socket.shutdown().await.unwrap();
            }
            asked
        });

        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("model.bin");
        let file = PinnedFile {
            path: "model.bin".into(),
            size: body.len() as u64,
            sha256: hex::encode(Sha256::digest(&body)),
        };
        assert!(download(&url, &destination, &file, |_| {}).await.is_err());
        let partial = dir.path().join("model.bin.partial");
        assert_eq!(std::fs::metadata(&partial).unwrap().len(), half as u64);

        let mut first = None;
        download(&url, &destination, &file, |bytes| {
            first.get_or_insert(bytes);
        })
        .await
        .unwrap();
        assert_eq!(first, Some(half as u64));
        assert_eq!(std::fs::read(&destination).unwrap(), body);
        assert!(!partial.exists());
        assert_eq!(server.await.unwrap(), vec![None, Some(half)]);
    }

    #[tokio::test]
    async fn a_file_that_differs_from_the_record_is_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vocab.json");
        std::fs::write(&path, b"{}").unwrap();
        let file = PinnedFile {
            path: "vocab.json".into(),
            size: 2,
            sha256: hex::encode(Sha256::digest(b"{}")),
        };
        assert!(matches(&path, &file).await);
        let tampered = PinnedFile {
            sha256: hex::encode(Sha256::digest(b"[]")),
            ..file
        };
        assert!(!matches(&path, &tampered).await);
    }
}
