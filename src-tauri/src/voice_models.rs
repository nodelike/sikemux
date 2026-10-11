//! The speech models dictation runs on, and the helper that runs them. Every
//! model file is fetched from Hugging Face at the commit recorded when the app
//! was built, the helper from this version's GitHub release, and each is checked
//! against the hash recorded then before it is moved into place.

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
const HELPER_FILE: &str = "sikemux-voice";
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
pub(crate) struct ModelFile {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
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

/// The helper this build was made with. Builds without one recorded can only
/// run a helper that sits beside the app, as `make dev` builds do.
fn published_helper() -> Option<&'static ModelFile> {
    static HELPER: OnceLock<Option<ModelFile>> = OnceLock::new();
    HELPER
        .get_or_init(|| {
            Some(ModelFile {
                path: option_env!("SIKEMUX_VOICE_HELPER_ASSET")?.into(),
                size: option_env!("SIKEMUX_VOICE_HELPER_SIZE")?.parse().ok()?,
                sha256: option_env!("SIKEMUX_VOICE_HELPER_SHA256")?.into(),
            })
        })
        .as_ref()
}

pub fn helper_is_published() -> bool {
    published_helper().is_some()
}

pub fn helper_path(dir: &Path) -> PathBuf {
    dir.join(HELPER_FILE)
}

fn helper_url(helper: &ModelFile) -> String {
    format!(
        "https://github.com/nodelike/sikemux/releases/download/v{}/{}",
        env!("CARGO_PKG_VERSION"),
        helper.path
    )
}

fn models_verified(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(VERIFIED_MARKER)).is_ok_and(|hash| hash == manifest_hash())
}

fn helper_matches(dir: &Path, helper: &ModelFile) -> bool {
    let path = helper_path(dir);
    std::fs::metadata(&path).is_ok_and(|meta| meta.len() == helper.size)
        && hash_file(&path).is_ok_and(|hash| hash == helper.sha256)
}

/// `with_helper` asks for the published helper too, for a build that has no
/// helper of its own beside the app.
pub fn installed(dir: &Path, with_helper: bool) -> bool {
    models_verified(dir)
        && (!with_helper || published_helper().is_some_and(|helper| helper_matches(dir, helper)))
}

/// Makes every model file under `dir` match the manifest, downloading the ones
/// that are missing or differ, and with `with_helper` does the same for the
/// helper. `progress` hears the fraction of bytes in place.
pub async fn ensure(dir: &Path, with_helper: bool, mut progress: impl FnMut(f64)) -> AppResult<()> {
    let helper =
        if with_helper {
            Some(published_helper().ok_or_else(|| {
                AppError::Other("this build has no published voice helper".into())
            })?)
        } else {
            None
        };
    let models_ready = models_verified(dir);
    let helper_to_fetch = match helper {
        Some(helper) => (!already_matches(&helper_path(dir), helper).await).then_some(helper),
        None => None,
    };
    if models_ready && helper_to_fetch.is_none() {
        return Ok(());
    }
    let manifest = manifest();
    let models = if models_ready {
        &[][..]
    } else {
        &manifest.models[..]
    };
    let model_bytes: u64 = models
        .iter()
        .flat_map(|model| &model.files)
        .map(|file| file.size)
        .sum();
    let total = (model_bytes + helper_to_fetch.map_or(0, |helper| helper.size)).max(1);
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
    for model in models {
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
    if !models_ready {
        std::fs::write(dir.join(VERIFIED_MARKER), manifest_hash())?;
    }
    if let Some(helper) = helper_to_fetch {
        let destination = helper_path(dir);
        download(&helper_url(helper), &destination, helper, |bytes| {
            report(done + bytes)
        })
        .await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755))?;
        }
        report(done + helper.size);
    }
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

pub(crate) fn hash_file(path: &Path) -> std::io::Result<String> {
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

pub(crate) async fn download(
    url: &str,
    destination: &Path,
    file: &ModelFile,
    progress: impl FnMut(u64),
) -> AppResult<()> {
    download_as(url, destination, file, progress, false).await
}

/// Downloads a program, which can be run the moment it appears at `destination`.
pub(crate) async fn download_executable(
    url: &str,
    destination: &Path,
    file: &ModelFile,
    progress: impl FnMut(u64),
) -> AppResult<()> {
    download_as(url, destination, file, progress, true).await
}

async fn download_as(
    url: &str,
    destination: &Path,
    file: &ModelFile,
    mut progress: impl FnMut(u64),
    executable: bool,
) -> AppResult<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::Other(format!("{} has no parent", destination.display())))?;
    std::fs::create_dir_all(parent)?;
    let partial = PathBuf::from(format!("{}.partial", destination.display()));
    let mut result = write_verified(url, &partial, file, &mut progress).await;
    #[cfg(unix)]
    if executable && result.is_ok() {
        use std::os::unix::fs::PermissionsExt;
        result = std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))
            .map_err(Into::into);
    }
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

    #[test]
    fn only_the_helper_built_with_the_app_counts_as_installed() {
        let dir = tempfile::tempdir().unwrap();
        let helper = ModelFile {
            path: "sikemux-voice-aarch64-apple-darwin".into(),
            size: 6,
            sha256: hex::encode(Sha256::digest(b"helper")),
        };
        assert!(!helper_matches(dir.path(), &helper));
        std::fs::write(helper_path(dir.path()), b"helper").unwrap();
        assert!(helper_matches(dir.path(), &helper));
        std::fs::write(helper_path(dir.path()), b"helpex").unwrap();
        assert!(!helper_matches(dir.path(), &helper));
    }
}
