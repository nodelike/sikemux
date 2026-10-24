//! The speech models dictation runs on, and the helper that runs them. Every
//! model file is fetched from Hugging Face at the commit recorded when the app
//! was built, the helper from this version's GitHub release, and each is checked
//! against the hash recorded then before it is moved into place.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use sikemux_core::download::{self, hash_file, DownloadError, PinnedFile};

const MANIFEST: &str = include_str!("../../../voice-models.json");
/// Holds the hash of the manifest the models were last checked against.
const VERIFIED_MARKER: &str = "models.verified";
const HELPER_FILE: &str = "sikemux-voice";
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
    files: Vec<PinnedFile>,
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
fn published_helper() -> Option<&'static PinnedFile> {
    static HELPER: OnceLock<Option<PinnedFile>> = OnceLock::new();
    HELPER
        .get_or_init(|| {
            Some(PinnedFile {
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

fn helper_url(helper: &PinnedFile) -> String {
    format!(
        "https://github.com/nodelike/sikemux/releases/download/v{}/{}",
        env!("CARGO_PKG_VERSION"),
        helper.path
    )
}

fn models_verified(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(VERIFIED_MARKER)).is_ok_and(|hash| hash == manifest_hash())
}

fn helper_matches(dir: &Path, helper: &PinnedFile) -> bool {
    let path = helper_path(dir);
    std::fs::metadata(&path).is_ok_and(|meta| meta.len() == helper.size)
        && hash_file(&path).is_ok_and(|hash| hash == helper.sha256)
}

/// Why the models or the helper are not in place.
#[derive(Debug)]
pub enum Missing {
    /// The network is unreachable; the download carries on when it is back.
    Offline,
    Failed(String),
}

impl From<DownloadError> for Missing {
    fn from(error: DownloadError) -> Self {
        match &error {
            DownloadError::Request(request)
                if sikemux_plugin_api::unreachable(request).is_some() =>
            {
                Missing::Offline
            }
            _ => Missing::Failed(error.to_string()),
        }
    }
}

impl From<std::io::Error> for Missing {
    fn from(error: std::io::Error) -> Self {
        Missing::Failed(error.to_string())
    }
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
pub async fn ensure(
    dir: &Path,
    with_helper: bool,
    mut progress: impl FnMut(f64),
) -> Result<(), Missing> {
    let helper =
        if with_helper {
            Some(published_helper().ok_or_else(|| {
                Missing::Failed("this build has no published voice helper".into())
            })?)
        } else {
            None
        };
    let models_ready = models_verified(dir);
    let helper_to_fetch = match helper {
        Some(helper) => (!download::matches(&helper_path(dir), helper).await).then_some(helper),
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
            if !download::matches(&destination, file).await {
                let url = format!(
                    "https://huggingface.co/{}/resolve/{}/{}",
                    model.repo, model.revision, file.path
                );
                download::download(&url, &destination, file, |bytes| report(done + bytes)).await?;
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
        download::download_executable(&helper_url(helper), &destination, helper, |bytes| {
            report(done + bytes)
        })
        .await?;
        report(done + helper.size);
    }
    Ok(())
}

/// A relative path that cannot climb out of, or jump away from, the directory it is joined to.
fn plain_relative(path: &str) -> Result<PathBuf, Missing> {
    let relative = Path::new(path);
    let plain = !path.is_empty()
        && !path.contains('\\')
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if plain {
        Ok(relative.to_path_buf())
    } else {
        Err(Missing::Failed(format!(
            "voice model path {path:?} is not a plain relative path"
        )))
    }
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

    #[test]
    fn only_the_helper_built_with_the_app_counts_as_installed() {
        let dir = tempfile::tempdir().unwrap();
        let helper = PinnedFile {
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
