// Which project a call is about, the people GitLab names, and the projects and
// branches an account can see. A GitLab project sits in a group that may sit in
// other groups, so the owner the Git pane names can hold slashes.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::client;
use crate::error::{GitlabError, GitlabResult};

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    /// The group path, `group` or `group/subgroup`.
    pub owner: String,
    pub name: String,
}

fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 255
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_owner(owner: &str) -> bool {
    owner.split('/').all(valid_segment)
}

/// A project's full path as one path segment, which is how GitLab takes it in place of a number.
pub fn encoded(full_path: &str) -> String {
    full_path.replace('/', "%2F")
}

impl RepoRef {
    pub fn full_path(&self) -> GitlabResult<String> {
        if !valid_owner(&self.owner) || !valid_segment(&self.name) {
            return Err(GitlabError::BadArg(
                "that is not a GitLab group and project".into(),
            ));
        }
        Ok(format!("{}/{}", self.owner, self.name))
    }

    pub fn path(&self, rest: &str) -> GitlabResult<String> {
        Ok(format!("/projects/{}{rest}", encoded(&self.full_path()?)))
    }
}

/// A person as GitLab describes them.
#[derive(Deserialize, Default, Clone, Debug)]
pub struct User {
    pub id: Option<u64>,
    pub username: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

pub fn login_of(user: Option<&User>) -> Option<String> {
    user.and_then(|user| user.username.clone())
        .filter(|name| !name.is_empty())
}

pub fn avatar_of(user: Option<&User>) -> Option<String> {
    user.and_then(|user| user.avatar_url.clone())
        .filter(|url| !url.is_empty())
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl Repo {
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// The project a git remote points at, in any of the ways a remote can be
/// written: scp-like `git@host:group/project.git`, or an `https`, `ssh` or `git` URL.
pub fn from_remote(remote: &str) -> Option<Repo> {
    let remote = remote.trim();
    let (host, path) = if let Some((_, rest)) = remote.split_once("://") {
        let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
        let (authority, path) = rest.split_once('/')?;
        (authority.split(':').next()?.to_string(), path.to_string())
    } else {
        let rest = remote.split_once('@').map_or(remote, |(_, after)| after);
        let (host, path) = rest.split_once(':')?;
        (host.to_string(), path.to_string())
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let (owner, name) = path.rsplit_once('/')?;
    if host.is_empty() || !valid_owner(owner) || !valid_segment(name) {
        return None;
    }
    Some(Repo {
        host: host.to_ascii_lowercase(),
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

#[derive(Deserialize)]
struct ProjectRow {
    path_with_namespace: String,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    archived: bool,
    default_branch: Option<String>,
    last_activity_at: Option<String>,
    web_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub owner: String,
    pub name: String,
    pub slug: String,
    pub private: bool,
    pub archived: bool,
    pub default_branch: Option<String>,
    pub pushed_at: Option<String>,
    pub url: String,
}

fn listing(row: ProjectRow) -> Option<Listing> {
    let (owner, name) = row.path_with_namespace.rsplit_once('/')?;
    Some(Listing {
        owner: owner.to_string(),
        name: name.to_string(),
        slug: row.path_with_namespace.clone(),
        private: row.visibility.as_deref() != Some("public"),
        archived: row.archived,
        default_branch: row.default_branch,
        pushed_at: row.last_activity_at,
        url: row.web_url,
    })
}

pub async fn mine(data_dir: &Path, limit: u32) -> GitlabResult<Vec<Listing>> {
    let limit = limit.clamp(1, 200);
    let rows: Vec<ProjectRow> = client::get_all(
        data_dir,
        "/projects",
        &[
            ("membership", "true".into()),
            ("order_by", "last_activity_at".into()),
            ("simple", "true".into()),
        ],
        limit.div_ceil(100),
    )
    .await?;
    Ok(rows
        .into_iter()
        .take(limit as usize)
        .filter_map(listing)
        .collect())
}

#[derive(Deserialize)]
pub struct Branch {
    pub name: String,
}

pub async fn branches(data_dir: &Path, repo: RepoRef) -> GitlabResult<Vec<String>> {
    let rows: Vec<Branch> = client::get_all(
        data_dir,
        &repo.path("/repository/branches")?,
        &[("sort", "updated_desc".into())],
        5,
    )
    .await?;
    Ok(rows.into_iter().map(|branch| branch.name).collect())
}

#[derive(Deserialize)]
struct ProjectDetail {
    default_branch: Option<String>,
}

pub async fn default_branch(data_dir: &Path, repo: &RepoRef) -> GitlabResult<String> {
    let detail: ProjectDetail = client::get(data_dir, &repo.path("")?, &[]).await?;
    detail
        .default_branch
        .ok_or_else(|| GitlabError::NotFound("the project has no default branch yet".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(remote: &str) -> Option<String> {
        from_remote(remote).map(|repo| format!("{} {}", repo.host, repo.slug()))
    }

    #[test]
    fn reads_every_way_a_gitlab_remote_is_written() {
        for remote in [
            "git@gitlab.com:swishx/api-docs.git",
            "https://oauth2:token@gitlab.com/swishx/api-docs.git",
            "https://gitlab.com/swishx/api-docs",
            "ssh://git@gitlab.com:2222/swishx/api-docs.git",
            "https://gitlab.com/swishx/api-docs/",
        ] {
            assert_eq!(
                slug(remote).as_deref(),
                Some("gitlab.com swishx/api-docs"),
                "{remote}"
            );
        }
    }

    #[test]
    fn a_project_in_nested_groups_keeps_every_group() {
        assert_eq!(
            slug("git@gitlab.acme.dev:platform/payments/billing-api.git").as_deref(),
            Some("gitlab.acme.dev platform/payments/billing-api")
        );
    }

    #[test]
    fn a_path_that_is_not_group_and_project_is_nothing() {
        for remote in [
            "/srv/local.git",
            "https://gitlab.com/only",
            "git@host:a/../b.git",
            "",
        ] {
            assert!(from_remote(remote).is_none(), "{remote}");
        }
    }

    #[test]
    fn a_project_is_named_in_a_path_by_its_encoded_full_path() -> GitlabResult<()> {
        let repo = RepoRef {
            owner: "platform/payments".into(),
            name: "billing-api".into(),
        };
        assert_eq!(
            repo.path("/merge_requests")?,
            "/projects/platform%2Fpayments%2Fbilling-api/merge_requests"
        );
        let bad = RepoRef {
            owner: "a/..".into(),
            name: "x".into(),
        };
        assert!(bad.path("").is_err());
        Ok(())
    }

    #[test]
    fn a_listed_project_splits_into_group_and_name() {
        let row: ProjectRow = serde_json::from_value(serde_json::json!({
            "path_with_namespace": "platform/payments/billing-api",
            "visibility": "internal",
            "archived": false,
            "default_branch": "main",
            "last_activity_at": "2026-10-08T10:00:00Z",
            "web_url": "https://gitlab.acme.dev/platform/payments/billing-api"
        }))
        .expect("parses");
        let listed = listing(row).expect("listed");
        assert_eq!(
            (listed.owner.as_str(), listed.name.as_str()),
            ("platform/payments", "billing-api")
        );
        assert!(listed.private);
    }
}
