// The workflows a repository has, and starting one by hand.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::client;
use crate::common::LIST_PAGES;
use crate::error::{GithubError, GithubResult};
use crate::repo::{self, Repo};

#[derive(Deserialize)]
struct WorkflowRow {
    id: u64,
    name: String,
    path: String,
    state: String,
    html_url: String,
}

#[derive(Deserialize)]
struct WorkflowList {
    workflows: Vec<WorkflowRow>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: u64,
    pub name: String,
    pub path: String,
    /// `active`, or one of the several ways GitHub says a workflow is switched off.
    pub state: String,
    pub active: bool,
    pub url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    pub fn checked(&self) -> GithubResult<&Self> {
        repo::validate(&self.owner, &self.name)?;
        Ok(self)
    }

    pub fn path(&self, rest: &str) -> GithubResult<String> {
        let checked = self.checked()?;
        Ok(format!("/repos/{}/{}{rest}", checked.owner, checked.name))
    }
}

impl From<&Repo> for RepoRef {
    fn from(repo: &Repo) -> Self {
        Self {
            owner: repo.owner.clone(),
            name: repo.name.clone(),
        }
    }
}

pub async fn list(data_dir: &Path, repo: RepoRef) -> GithubResult<Vec<Workflow>> {
    let workflows = client::get_all(
        data_dir,
        &repo.path("/actions/workflows")?,
        &[],
        LIST_PAGES,
        |list: WorkflowList| list.workflows,
    )
    .await?;
    Ok(workflows
        .into_iter()
        .map(|row| Workflow {
            active: row.state == "active",
            id: row.id,
            name: row.name,
            path: row.path,
            state: row.state,
            url: row.html_url,
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub workflow_id: u64,
    /// The branch or tag to run on.
    pub git_ref: String,
    #[serde(default)]
    pub inputs: Map<String, Value>,
}

/// Every `workflow_dispatch` input reaches GitHub as a string, whatever the
/// person typed it as.
fn as_strings(inputs: Map<String, Value>) -> Map<String, Value> {
    inputs
        .into_iter()
        .map(|(name, value)| {
            let text = match value {
                Value::String(text) => text,
                Value::Null => String::new(),
                other => other.to_string(),
            };
            (name, Value::String(text))
        })
        .collect()
}

fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.chars().any(|c| {
            c.is_ascii_whitespace()
                || c.is_ascii_control()
                || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
}

pub async fn dispatch(data_dir: &Path, input: Dispatch) -> GithubResult<()> {
    let git_ref = input.git_ref.trim();
    if !valid_ref(git_ref) {
        return Err(GithubError::BadArg(format!(
            "`{git_ref}` is not a branch or tag name"
        )));
    }
    let path = input.repo.path(&format!(
        "/actions/workflows/{}/dispatches",
        input.workflow_id
    ))?;
    let body = json!({ "ref": git_ref, "inputs": as_strings(input.inputs) });
    client::post_empty(data_dir, &path, Some(&body)).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub workflow_id: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFile {
    pub path: String,
    pub text: String,
}

/// A workflow somebody wrote lives under `.github`. GitHub's own, such as code
/// scanning, has a path like `dynamic/github-code-scanning/codeql` instead.
pub fn is_in_repository(path: &str) -> bool {
    path.starts_with(".github/")
}

/// A file's path as it goes into an address: `#`, `?` and spaces would
/// otherwise end or break it, so each piece between slashes is escaped.
fn encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The YAML a run came from, read at the default branch. A workflow's own
/// record carries the path, so the file is fetched in two steps.
pub async fn file(data_dir: &Path, input: FileRef) -> GithubResult<WorkflowFile> {
    let row: WorkflowRow = client::get(
        data_dir,
        &input
            .repo
            .path(&format!("/actions/workflows/{}", input.workflow_id))?,
        &[],
    )
    .await?;
    if !is_in_repository(&row.path) {
        return Err(GithubError::NotFound(
            "GitHub runs this workflow itself, so the repository has no file for it".into(),
        ));
    }
    let contents = input
        .repo
        .path(&format!("/contents/{}", encode_path(&row.path)))?;
    let (bytes, _) =
        client::download_as(data_dir, &contents, "application/vnd.github.raw", false).await?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(WorkflowFile {
        path: row.path,
        text,
    })
}

#[derive(Deserialize)]
struct BranchRow {
    name: String,
}

pub async fn branches(data_dir: &Path, repo: RepoRef) -> GithubResult<Vec<String>> {
    let rows: Vec<BranchRow> = client::get_all(
        data_dir,
        &repo.path("/branches")?,
        &[],
        LIST_PAGES,
        |rows| rows,
    )
    .await?;
    Ok(rows.into_iter().map(|row| row.name).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoRef {
        RepoRef {
            owner: "nodelike".into(),
            name: "sikemux".into(),
        }
    }

    #[test]
    fn builds_a_path_under_the_repository() -> GithubResult<()> {
        assert_eq!(
            repo().path("/actions/runs")?,
            "/repos/nodelike/sikemux/actions/runs"
        );
        Ok(())
    }

    #[test]
    fn a_repository_that_could_escape_the_path_is_refused() {
        let escaping = RepoRef {
            owner: "..".into(),
            name: "x".into(),
        };
        assert!(escaping.path("/actions/runs").is_err());
    }

    #[test]
    fn every_dispatch_input_goes_over_as_a_string() {
        let inputs: Map<String, Value> = serde_json::from_value(
            json!({ "level": "debug", "count": 3, "dry": true, "none": null }),
        )
        .unwrap_or_default();
        let sent = as_strings(inputs);
        assert_eq!(sent.get("level"), Some(&json!("debug")));
        assert_eq!(sent.get("count"), Some(&json!("3")));
        assert_eq!(sent.get("dry"), Some(&json!("true")));
        assert_eq!(sent.get("none"), Some(&json!("")));
    }

    #[test]
    fn a_workflow_path_cannot_end_the_address_early() {
        assert_eq!(
            encode_path(".github/workflows/ci.yml"),
            ".github/workflows/ci.yml"
        );
        assert_eq!(
            encode_path(".github/workflows/a #1?.yml"),
            ".github/workflows/a%20%231%3F.yml"
        );
        assert_eq!(encode_path("é"), "%C3%A9");
    }

    #[test]
    fn only_a_workflow_under_github_has_a_file() {
        assert!(is_in_repository(".github/workflows/ci.yml"));
        assert!(!is_in_repository("dynamic/github-code-scanning/codeql"));
    }

    #[test]
    fn refuses_refs_that_git_would_not_accept() {
        assert!(valid_ref("main"));
        assert!(valid_ref("release/0.4"));
        assert!(valid_ref("v1.2.3"));
        assert!(!valid_ref(""));
        assert!(!valid_ref("my branch"));
        assert!(!valid_ref("a..b"));
        assert!(!valid_ref("-x"));
        assert!(!valid_ref("feat^2"));
    }
}
