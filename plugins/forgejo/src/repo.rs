// Which repository a call is about, the people the server names, and the
// repositories and branches an account can see.

use std::path::Path;

use serde::Deserialize;
use sikemux_plugin_api::codehost::answers;
use sikemux_plugin_api::codehost::asks::{IssueRef, PullRef, RepoRef, ThreadRef};
use sikemux_plugin_api::codehost::{Repo, RepoListing};

use crate::client;
use crate::error::{ForgejoError, ForgejoResult};

fn valid_segment(segment: &str) -> bool {
    answers::valid_name(segment, 255)
}

/// Where what a call names sits in the API, below `/api/v1`.
pub trait ApiPath {
    fn path(&self, rest: &str) -> ForgejoResult<String>;
}

impl ApiPath for RepoRef {
    fn path(&self, rest: &str) -> ForgejoResult<String> {
        if !valid_segment(&self.owner) || !valid_segment(&self.name) {
            return Err(ForgejoError::BadArg(
                "that is not an owner and repository".into(),
            ));
        }
        Ok(format!("/repos/{}/{}{rest}", self.owner, self.name))
    }
}

/// Pull requests and issues share one count, and the issue routes answer for both.
fn issue_path(repo: &RepoRef, number: u64, rest: &str) -> ForgejoResult<String> {
    repo.path(&format!("/issues/{number}{rest}"))
}

impl ApiPath for ThreadRef {
    fn path(&self, rest: &str) -> ForgejoResult<String> {
        issue_path(&self.repo, self.number, rest)
    }
}

impl ApiPath for IssueRef {
    fn path(&self, rest: &str) -> ForgejoResult<String> {
        issue_path(&self.repo, self.number, rest)
    }
}

impl ApiPath for PullRef {
    fn path(&self, rest: &str) -> ForgejoResult<String> {
        self.repo.path(&format!("/pulls/{}{rest}", self.number))
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
pub struct User {
    pub id: Option<u64>,
    pub login: Option<String>,
    pub full_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl User {
    pub fn display_name(&self) -> Option<String> {
        self.full_name.clone().filter(|name| !name.is_empty())
    }
}

pub fn login_of(user: Option<&User>) -> Option<String> {
    user.and_then(|user| user.login.clone())
        .filter(|name| !name.is_empty())
}

pub fn avatar_of(user: Option<&User>) -> Option<String> {
    user.and_then(|user| user.avatar_url.clone())
        .filter(|url| !url.is_empty())
}

pub fn slug(repo: &Repo) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

pub fn from_remote(remote: &str) -> Option<Repo> {
    let (host, path) = answers::split_remote(remote)?;
    let (owner, name) = path.split_once('/')?;
    if !valid_segment(owner) || !valid_segment(name) {
        return None;
    }
    Some(Repo {
        host,
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

#[derive(Deserialize)]
pub struct RepoRow {
    full_name: String,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    archived: bool,
    default_branch: Option<String>,
    updated_at: Option<String>,
    html_url: String,
}

fn listing(row: RepoRow) -> Option<RepoListing> {
    let (owner, name) = row.full_name.split_once('/')?;
    Some(RepoListing {
        owner: owner.to_string(),
        name: name.to_string(),
        slug: row.full_name.clone(),
        private: row.private,
        archived: row.archived,
        default_branch: row.default_branch,
        pushed_at: row.updated_at,
        url: row.html_url,
    })
}

pub async fn mine(data_dir: &Path, limit: u32) -> ForgejoResult<Vec<RepoListing>> {
    let limit = limit.clamp(1, 200);
    let mut rows: Vec<RepoRow> = client::get_all(
        data_dir,
        "/user/repos",
        &[],
        limit.div_ceil(client::PER_PAGE),
    )
    .await?;
    rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(rows
        .into_iter()
        .take(limit as usize)
        .filter_map(listing)
        .collect())
}

#[derive(Deserialize)]
struct BranchCommit {
    timestamp: Option<String>,
}

#[derive(Deserialize)]
struct Branch {
    name: String,
    commit: Option<BranchCommit>,
}

/// The repository's branches, the most recently committed to first.
pub async fn branches(data_dir: &Path, repo: RepoRef) -> ForgejoResult<Vec<String>> {
    let mut rows: Vec<Branch> =
        client::get_all(data_dir, &repo.path("/branches")?, &[], 10).await?;
    let when = |branch: &Branch| {
        branch
            .commit
            .as_ref()
            .and_then(|commit| commit.timestamp.clone())
    };
    rows.sort_by_key(|branch| std::cmp::Reverse(when(branch)));
    Ok(rows.into_iter().map(|branch| branch.name).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(remote: &str) -> Option<String> {
        from_remote(remote).map(|repo| format!("{} {}", repo.host, super::slug(&repo)))
    }

    #[test]
    fn reads_every_way_a_remote_is_written() {
        for remote in [
            "git@codeberg.org:forgejo/forgejo.git",
            "https://codeberg.org/forgejo/forgejo.git",
            "ssh://git@codeberg.org:2222/forgejo/forgejo.git",
            "https://codeberg.org/forgejo/forgejo/",
        ] {
            assert_eq!(
                slug(remote).as_deref(),
                Some("codeberg.org forgejo/forgejo"),
                "{remote}"
            );
        }
        for remote in ["https://codeberg.org/only", "git@host:a/b/c.git", ""] {
            assert!(from_remote(remote).is_none(), "{remote}");
        }
    }

    #[test]
    fn pull_requests_and_issues_share_the_issue_routes() -> ForgejoResult<()> {
        let repo = RepoRef {
            owner: "forgejo".into(),
            name: "runner".into(),
        };
        let thread = ThreadRef {
            repo: repo.clone(),
            number: 7,
            of: Default::default(),
        };
        assert_eq!(
            thread.path("/comments")?,
            "/repos/forgejo/runner/issues/7/comments"
        );
        let pull = PullRef { repo, number: 7 };
        assert_eq!(pull.path("/files")?, "/repos/forgejo/runner/pulls/7/files");
        Ok(())
    }
}
