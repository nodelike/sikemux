//! The speech models dictation runs on. Every file is fetched from Hugging Face
//! at the commit recorded when the app was built, checked against the hash
//! recorded then, and only then moved into place for the voice helper to load.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

const MANIFEST: &str = include_str!("../voice-models.json");
/// Holds the hash of the manifest the models were last checked against.
const VERIFIED_MARKER: &str = "models.verified";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const PROGRESS_STEP: f64 = 0.005;

#[derive(Deserialize)]
struct Manifest {
    models: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    repo: String,
    revision: String,
    folder: String,
    files: Vec<ModelFile>,
}

#[derive(Deserialize)]
struct ModelFile {
    path: String,
    size: u64,
    sha256: String,
}

fn manifest() -> &'static Manifest {
    static PARSED: OnceLock<Manifest> = OnceLock::new();
    PARSED.get_or_init(|| {
        serde_json::from_str(MANIFEST).expect("voice-models.json is generated and always parses")
    })
}

fn manifest_hash() -> String {
    hex::encode(Sha256::digest(MANIFEST.as_bytes()))
}

pub fn installed(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(VERIFIED_MARKER)).is_ok_and(|hash| hash == manifest_hash())
}

/// Makes every model file under `dir` match the manifest, downloading the ones
/// that are missing or differ. `progress` hears the fraction of bytes in place.
pub async fn ensure(dir: &Path, mut progress: impl FnMut(f64)) -> AppResult<()> {
    if installed(dir) {
        return Ok(());
    }
    let manifest = manifest();
    let total: u64 = manifest
        .models
        .iter()
        .flat_map(|model| &model.files)
        .map(|file| file.size)
        .sum::<u64>()
        .max(1);
    let mut done = 0u64;
    let mut reported = -1.0;
    let mut report = |bytes: u64| {
        let fraction = bytes as f64 / total as f64;
        if fraction - reported >= PROGRESS_STEP || fraction >= 1.0 {
            reported = fraction;
            progress(fraction.min(1.0));
        }
    };
    report(0);
    for model in &manifest.models {
        let folder = dir.join(plain_relative(&model.folder)?);
        for file in &model.files {
            let destination = folder.join(plain_relative(&file.path)?);
            if !already_matches(&destination, file).await {
                let url = format!(
                    "https://huggingface.co/{}/resolve/{}/{}",
                    model.repo, model.revision, file.path
                );
                download(&url, &destination, file, |bytes| report(done + bytes)).await?;
            }
            done += file.size;
            report(done);
        }
    }
    std::fs::write(dir.join(VERIFIED_MARKER), manifest_hash())?;
    Ok(())
}

/// A relative path that cannot climb out of, or jump away from, the directory it is joined to.
fn plain_relative(path: &str) -> AppResult<PathBuf> {
    let relative = Path::new(path);
    let plain = !path.is_empty()
        && !path.contains('\\')
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if plain {
        Ok(relative.to_path_buf())
    } else {
        Err(AppError::Other(format!(
            "voice model path {path:?} is not a plain relative path"
        )))
    }
}

async fn already_matches(path: &Path, file: &ModelFile) -> bool {
    if std::fs::metadata(path).map(|meta| meta.len()).ok() != Some(file.size) {
        return false;
    }
    let path = path.to_path_buf();
    let hash = tokio::task::spawn_blocking(move || hash_file(&path)).await;
    matches!(hash, Ok(Ok(hash)) if hash == file.sha256)
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut reader = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

async fn download(
    url: &str,
    destination: &Path,
    file: &ModelFile,
    mut progress: impl FnMut(u64),
) -> AppResult<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::Other(format!("{} has no parent", destination.display())))?;
    std::fs::create_dir_all(parent)?;
    let partial = PathBuf::from(format!("{}.partial", destination.display()));
    let result = write_verified(url, &partial, file, &mut progress).await;
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result?;
    std::fs::rename(&partial, destination)?;
    Ok(())
}

async fn write_verified(
    url: &str,
    partial: &Path,
    file: &ModelFile,
    progress: &mut impl FnMut(u64),
) -> AppResult<()> {
    let response = client().get(url).send().await?.error_for_status()?;
    let mut out = File::create(partial)?;
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        written += chunk.len() as u64;
        if written > file.size {
            return Err(AppError::Http(format!(
                "{} is larger than expected",
                file.path
            )));
        }
        hasher.update(&chunk);
        out.write_all(&chunk)?;
        progress(written);
    }
    out.sync_all()?;
    if written != file.size || hex::encode(hasher.finalize()) != file.sha256 {
        return Err(AppError::Http(format!(
            "{} did not match the expected hash",
            file.path
        )));
    }
    Ok(())
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .user_agent(concat!("sikemux/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_is_pinned_to_a_commit_and_hashed() {
        let manifest = manifest();
        assert!(!manifest.models.is_empty());
        for model in &manifest.models {
            assert_eq!(model.revision.len(), 40);
            assert!(model.revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
            plain_relative(&model.folder).unwrap();
            for file in &model.files {
                plain_relative(&file.path).unwrap();
                assert_eq!(file.sha256.len(), 64);
                assert!(file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            }
        }
    }

    #[test]
    fn paths_that_leave_the_models_directory_are_refused() {
        for path in [
            "",
            "/etc/passwd",
            "../LaunchAgents/x.plist",
            "a/../../b",
            "./a",
            "a\\..\\b",
        ] {
            assert!(plain_relative(path).is_err(), "{path} was accepted");
        }
        assert_eq!(
            plain_relative("Encoder.mlmodelc/weights/weight.bin").unwrap(),
            PathBuf::from("Encoder.mlmodelc/weights/weight.bin")
        );
    }

    #[tokio::test]
    async fn a_file_that_differs_from_the_manifest_is_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vocab.json");
        std::fs::write(&path, b"{}").unwrap();
        let file = ModelFile {
            path: "vocab.json".into(),
            size: 2,
            sha256: hex::encode(Sha256::digest(b"{}")),
        };
        assert!(already_matches(&path, &file).await);
        let tampered = ModelFile {
            sha256: hex::encode(Sha256::digest(b"[]")),
            ..file
        };
        assert!(!already_matches(&path, &tampered).await);
    }
}
