// What a run left behind, and putting one on disk. GitHub serves an artifact
// as a zip behind a redirect, and stops serving it at all once it expires.

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sikemux_plugin_api::{reply, PluginResult, StreamSink};

use crate::client;
use crate::common::LIST_PAGES;
use crate::error::{GithubError, GithubResult};
use crate::runs::RunRef;
use crate::workflows::RepoRef;

#[derive(Deserialize)]
struct ArtifactRow {
    id: u64,
    name: String,
    size_in_bytes: u64,
    expired: bool,
    created_at: Option<String>,
    expires_at: Option<String>,
}

#[derive(Deserialize)]
struct ArtifactList {
    artifacts: Vec<ArtifactRow>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub id: u64,
    pub name: String,
    pub size_bytes: u64,
    /// True once GitHub has stopped keeping it, which makes it undownloadable.
    pub expired: bool,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
}

pub async fn list(data_dir: &Path, input: RunRef) -> GithubResult<Vec<Artifact>> {
    let path = input
        .repo
        .path(&format!("/actions/runs/{}/artifacts", input.run_id))?;
    let artifacts = client::get_all(data_dir, &path, &[], LIST_PAGES, |list: ArtifactList| {
        list.artifacts
    })
    .await?;
    Ok(artifacts
        .into_iter()
        .map(|row| Artifact {
            id: row.id,
            name: row.name,
            size_bytes: row.size_in_bytes,
            expired: row.expired,
            created_at: row.created_at,
            expires_at: row.expires_at,
        })
        .collect())
}

/// Anything that could steer the file out of the folder it is meant to land
/// in is dropped, so a name GitHub accepted cannot become a path. The name is
/// split on everything a filename may not hold, and the dot-only pieces that
/// walk up a directory go with it.
fn safe_file_name(name: &str) -> String {
    let joined = name
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .filter(|part| !part.is_empty() && !part.chars().all(|c| c == '.'))
        .collect::<Vec<_>>()
        .join("-");
    let trimmed = joined.trim_matches(['.', '-']);
    if trimmed.is_empty() {
        "artifact".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

/// Where a download lands: the person's Downloads folder when there is one,
/// and the plugin's own folder otherwise.
fn download_dir(data_dir: &Path) -> PathBuf {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .map(|home| home.join("Downloads"))
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(|| data_dir.to_path_buf())
}

/// An artifact always arrives as a zip, which its name may already say.
pub fn file_name(name: &str, extension: Option<&str>) -> String {
    let safe = safe_file_name(name);
    match extension {
        Some(extension)
            if !safe
                .to_ascii_lowercase()
                .ends_with(&format!(".{extension}")) =>
        {
            format!("{safe}.{extension}")
        }
        _ => safe,
    }
}

fn saving(error: std::io::Error) -> GithubError {
    GithubError::Transport(format!("saving the download: {error}"))
}

/// Takes the first free name in `dir` by creating the file, so two downloads
/// of the same thing never write into one. A second copy lands beside the
/// first as `name-2.ext`; nothing already there is replaced.
fn claim(dir: &Path, name: &str) -> GithubResult<PathBuf> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    for copy in 1..1000u32 {
        let candidate = if copy == 1 {
            dir.join(name)
        } else {
            dir.join(format!("{stem}-{copy}{extension}"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(saving(error)),
        }
    }
    Err(GithubError::Transport(
        "saving the download: every name for it is taken".into(),
    ))
}

/// A download on its way to disk. The bytes go to a `.part` file beside the
/// claimed name, so one cut short never looks finished. Dropped before it is
/// done, by an error or by the download being stopped, it removes both.
struct Landing {
    target: PathBuf,
    part: PathBuf,
    done: bool,
}

impl Landing {
    fn new(target: PathBuf) -> Self {
        let mut part = target.clone().into_os_string();
        part.push(".part");
        Self {
            target,
            part: part.into(),
            done: false,
        }
    }
}

impl Drop for Landing {
    fn drop(&mut self) {
        if !self.done {
            std::fs::remove_file(&self.part).ok();
            std::fs::remove_file(&self.target).ok();
        }
    }
}

const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// How far a download has got. The last one carries where it was saved.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    received: u64,
    total: Option<u64>,
    saved: Option<Saved>,
}

/// Writes a file GitHub hands over, an artifact or a release asset, into the
/// downloads folder as it arrives, however large it is.
pub async fn save(
    data_dir: &Path,
    path: &str,
    accept: &str,
    name: &str,
    sink: &StreamSink,
) -> PluginResult<()> {
    let response = client::open_download(data_dir, path, accept).await?;
    let total = response.content_length();
    let dir = download_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(saving)?;
    let mut landing = Landing::new(claim(&dir, name)?);
    let mut file = std::fs::File::create(&landing.part).map_err(saving)?;
    let mut stream = response.bytes_stream();
    let mut received: u64 = 0;
    let mut told = Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(GithubError::from)?;
        file.write_all(&chunk).map_err(saving)?;
        received += chunk.len() as u64;
        if told.elapsed() >= PROGRESS_EVERY {
            told = Instant::now();
            sink.send(reply(Progress {
                received,
                total,
                saved: None,
            })?)?;
        }
    }
    file.flush().map_err(saving)?;
    drop(file);
    std::fs::rename(&landing.part, &landing.target).map_err(saving)?;
    landing.done = true;
    sink.send(reply(Progress {
        received,
        total,
        saved: Some(Saved {
            path: landing.target.to_string_lossy().into_owned(),
            bytes: received,
        }),
    })?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub artifact_id: u64,
    /// What to call the file, which is not the repository's `name`.
    pub file_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub path: String,
    pub bytes: u64,
}

pub async fn download(data_dir: &Path, input: Download, sink: &StreamSink) -> PluginResult<()> {
    let path = input
        .repo
        .path(&format!("/actions/artifacts/{}/zip", input.artifact_id))?;
    let name = file_name(&input.file_name, Some("zip"));
    save(data_dir, &path, "application/vnd.github+json", &name, sink).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_name_and_the_repository_name_arrive_apart() {
        let input: Download = serde_json::from_value(serde_json::json!({
            "owner": "nodelike", "name": "sikemux", "artifactId": 7, "fileName": "build",
        }))
        .expect("parses");
        assert_eq!(input.repo.name, "sikemux");
        assert_eq!(input.file_name, "build");
    }

    #[test]
    fn a_name_can_never_become_a_path() {
        assert_eq!(safe_file_name("build output"), "build-output");
        assert_eq!(safe_file_name("../../etc/passwd"), "etc-passwd");
        assert_eq!(safe_file_name("dist/app.tar.gz"), "dist-app.tar.gz");
        assert_eq!(safe_file_name("..."), "artifact");
        assert_eq!(safe_file_name(""), "artifact");
    }

    #[test]
    fn keeps_a_plain_name_as_it_is() {
        assert_eq!(safe_file_name("coverage-report"), "coverage-report");
        assert_eq!(safe_file_name("sikemux_0.4.2.dmg"), "sikemux_0.4.2.dmg");
    }

    #[test]
    fn a_long_name_is_cut_rather_than_refused() {
        assert_eq!(safe_file_name(&"a".repeat(400)).len(), 120);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sikemux-gh-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_second_download_lands_beside_the_first() {
        let dir = scratch("claim");
        let first = claim(&dir, "build.zip").expect("claims");
        assert!(first.ends_with("build.zip"));
        assert!(claim(&dir, "build.zip")
            .expect("claims")
            .ends_with("build-2.zip"));
        assert!(claim(&dir, "Sikemux.dmg")
            .expect("claims")
            .ends_with("Sikemux.dmg"));
        assert!(claim(&dir, "Sikemux.dmg")
            .expect("claims")
            .ends_with("Sikemux-2.dmg"));
        assert!(claim(&dir, "notes").expect("claims").ends_with("notes"));
        assert!(claim(&dir, "notes").expect("claims").ends_with("notes-2"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_artifact_named_as_a_zip_is_not_zipped_twice() {
        assert_eq!(file_name("build.zip", Some("zip")), "build.zip");
        assert_eq!(file_name("build.ZIP", Some("zip")), "build.ZIP");
        assert_eq!(file_name("build", Some("zip")), "build.zip");
        assert_eq!(
            file_name("Sikemux_aarch64.dmg", None),
            "Sikemux_aarch64.dmg"
        );
    }

    #[test]
    fn a_download_stopped_part_way_leaves_nothing_behind() {
        let dir = scratch("landing");
        let landing = Landing::new(claim(&dir, "big.dmg").expect("claims"));
        std::fs::write(&landing.part, b"half").expect("write");
        let (target, part) = (landing.target.clone(), landing.part.clone());
        assert!(part.ends_with("big.dmg.part"));
        drop(landing);
        assert!(!target.exists() && !part.exists());

        let mut finished = Landing::new(claim(&dir, "big.dmg").expect("claims"));
        std::fs::write(&finished.target, b"all").expect("write");
        finished.done = true;
        let target = finished.target.clone();
        drop(finished);
        assert!(target.exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
