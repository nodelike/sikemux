// Deploying a folder on this Mac, as `vercel deploy` does: its files as git sees them,
// sent to Vercel by their SHA-1, only the ones Vercel doesn't have already.

use std::path::{Path, PathBuf};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::store;
use sikemux_plugin_api::{reply, PluginResult, StreamSink};

use crate::client::{at, text, Session};
use crate::deployments::deployment;
use crate::error::{VercelError, VercelResult};
use crate::projects::segment;

/// Vercel's own limit on a Hobby deployment's uploaded source.
const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;
const MAX_FILES: usize = 15_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// The path inside the folder, with forward slashes.
    pub name: String,
    pub sha: String,
    pub size: u64,
    pub path: PathBuf,
}

/// Files never sent, whatever git says: secrets and Vercel's own folder.
pub fn left_out(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    name.starts_with(".vercel/")
        || name.starts_with(".git/")
        || file == ".env"
        || file.starts_with(".env.")
        || file == ".DS_Store"
}

/// The folder's files as git sees them: tracked ones and new ones it doesn't ignore.
fn git_files(folder: &Path) -> VercelResult<Vec<String>> {
    let output = sikemux_process::user_environment::command("git")
        .arg("-C")
        .arg(folder)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .map_err(|error| VercelError::BadArg(format!("git could not run: {error}")))?;
    if !output.status.success() {
        return Err(VercelError::BadArg(format!(
            "{} is not a git checkout; Sikemux deploys a checkout so .gitignore says what to leave out",
            folder.display()
        )));
    }
    Ok(output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

/// Reads and hashes the folder's files, keeping to Vercel's limits.
pub fn local_files(folder: &Path, names: Vec<String>) -> VercelResult<Vec<LocalFile>> {
    let mut files = Vec::new();
    let mut total = 0u64;
    for name in names.into_iter().filter(|name| !left_out(name)) {
        let path = folder.join(&name);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        total += bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(VercelError::BadArg(
                "the folder holds more than 100 MB of source; Vercel's limit is 100 MB".into(),
            ));
        }
        files.push(LocalFile {
            sha: sha1_smol::Sha1::from(&bytes).digest().to_string(),
            size: bytes.len() as u64,
            name,
            path,
        });
        if files.len() > MAX_FILES {
            return Err(VercelError::BadArg(format!(
                "the folder holds more than {MAX_FILES} files"
            )));
        }
    }
    if files.is_empty() {
        return Err(VercelError::BadArg(
            "the folder has no files to deploy".into(),
        ));
    }
    Ok(files)
}

pub fn body_of(project: &Value, files: &[LocalFile], production: bool) -> Value {
    let mut body = json!({
        "name": at(project, "/name"),
        "project": at(project, "/id"),
        "files": files.iter().map(|file| json!({ "file": file.name, "sha": file.sha, "size": file.size })).collect::<Vec<_>>(),
        "meta": { "deployedFrom": "sikemux" },
    });
    if production {
        if let Some(object) = body.as_object_mut() {
            object.insert("target".into(), json!("production"));
        }
    }
    body
}

/// The SHA-1s Vercel asked for, when it refused because it lacks some files.
pub fn missing_of(status: u16, answer: &Value) -> Option<Vec<String>> {
    (status == 400 && text(answer, "/error/code") == "missing_files").then(|| {
        at(answer, "/error/missing")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deploy {
    pub project: String,
    #[serde(default)]
    pub team: Option<String>,
    /// The folder on this Mac, a git checkout.
    pub folder: String,
    /// Deploy to production instead of a preview.
    #[serde(default)]
    pub production: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> {
    step: &'a str,
    done: usize,
    total: usize,
    deployment: Option<Value>,
}

fn tell(sink: &StreamSink, step: &str, done: usize, total: usize) -> PluginResult<()> {
    sink.send(reply(Progress {
        step,
        done,
        total,
        deployment: None,
    })?)
}

pub async fn deploy(data_dir: &Path, input: Value, sink: &StreamSink) -> PluginResult<()> {
    let input: Deploy =
        serde_json::from_value(input).map_err(|error| VercelError::BadArg(error.to_string()))?;
    let project = segment("project", &input.project)?;
    let folder = PathBuf::from(&input.folder);
    tell(sink, "Reading files", 0, 0)?;
    let reading = folder.clone();
    let files = store::blocking(move || {
        let names = git_files(&reading)?;
        local_files(&reading, names)
    })
    .await?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let read = session.get(&format!("/v9/projects/{project}"), &[]).await?;
    let body = body_of(&read, &files, input.production);
    let (status, answer) = session
        .send_unchecked(Method::POST, "/v13/deployments", Some(body.clone()))
        .await?;
    let answer = match missing_of(status, &answer) {
        Some(missing) => {
            let wanted: Vec<&LocalFile> = files
                .iter()
                .filter(|file| missing.contains(&file.sha))
                .collect();
            for (done, file) in wanted.iter().enumerate() {
                tell(sink, "Uploading", done, wanted.len())?;
                let bytes = tokio::fs::read(&file.path)
                    .await
                    .map_err(|error| VercelError::BadArg(format!("{}: {error}", file.name)))?;
                session.upload(&file.sha, bytes).await?;
            }
            tell(sink, "Uploading", wanted.len(), wanted.len())?;
            session
                .send(Method::POST, "/v13/deployments", &[], Some(body))
                .await?
        }
        None => crate::client::checked(status, answer)?,
    };
    sink.send(reply(Progress {
        step: "Building",
        done: files.len(),
        total: files.len(),
        deployment: Some(deployment(&answer)),
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_and_vercels_own_folder_are_never_sent() {
        assert!(left_out(".env"));
        assert!(left_out("apps/web/.env.local"));
        assert!(left_out(".env.production"));
        assert!(left_out(".vercel/project.json"));
        assert!(left_out("src/.DS_Store"));
        assert!(!left_out("src/env.ts"));
        assert!(!left_out("docs/.environment.md"));
    }

    #[test]
    fn files_are_hashed_as_vercel_names_them() {
        let dir =
            std::env::temp_dir().join(format!("sikemux-vercel-deploy-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/index.html"), "hello").unwrap();
        std::fs::write(dir.join(".env"), "SECRET=1").unwrap();
        let files = local_files(
            &dir,
            vec!["src/index.html".into(), ".env".into(), "gone.txt".into()],
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "src/index.html");
        assert_eq!(files[0].sha, "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
        assert_eq!(files[0].size, 5);
        assert!(local_files(&dir, vec![".env".into()]).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_deployment_names_the_project_and_each_file() {
        let file = LocalFile {
            name: "index.html".into(),
            sha: "abc".into(),
            size: 5,
            path: PathBuf::from("/x"),
        };
        let body = body_of(
            &json!({ "id": "prj_1", "name": "web" }),
            std::slice::from_ref(&file),
            false,
        );
        assert_eq!(body["project"], "prj_1");
        assert_eq!(
            body["files"],
            json!([{ "file": "index.html", "sha": "abc", "size": 5 }])
        );
        assert!(body.get("target").is_none());
        assert_eq!(
            body_of(&json!({ "id": "prj_1" }), &[file], true)["target"],
            "production"
        );
    }

    #[test]
    fn a_refusal_for_missing_files_names_them() {
        let refused = json!({ "error": { "code": "missing_files", "message": "Missing files", "missing": ["abc", "def"] } });
        assert_eq!(
            missing_of(400, &refused),
            Some(vec!["abc".to_string(), "def".to_string()])
        );
        assert_eq!(
            missing_of(400, &json!({ "error": { "code": "bad_request" } })),
            None
        );
        assert_eq!(missing_of(200, &refused), None);
    }
}
