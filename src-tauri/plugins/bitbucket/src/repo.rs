// Which repository a call is about, the people Bitbucket names, and the
// repositories and branches an account can see.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::client;
use crate::error::{BitbucketError, BitbucketResult};

pub const HOST: &str = "bitbucket.org";

/// A workspace and a repository in it. Bitbucket's own word for the first is
/// workspace; the Git pane calls it the owner, as every host does.
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 128
        && slug != "."
        && slug != ".."
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl RepoRef {
    pub fn path(&self, rest: &str) -> BitbucketResult<String> {
        if !valid_slug(&self.owner) || !valid_slug(&self.name) {
            return Err(BitbucketError::BadArg(
                "that is not a Bitbucket workspace and repository".into(),
            ));
        }
        Ok(format!("/repositories/{}/{}{rest}", self.owner, self.name))
    }

    pub fn web(&self, rest: &str) -> String {
        format!("https://{HOST}/{}/{}{rest}", self.owner, self.name)
    }
}

/// Bitbucket names pipelines and steps by `{uuid}`, braces and all, which
/// have to be escaped to sit in a path.
pub fn uuid_segment(raw: &str) -> BitbucketResult<String> {
    let inner = raw.trim().trim_start_matches('{').trim_end_matches('}');
    let valid = !inner.is_empty()
        && inner.len() <= 64
        && inner.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if !valid {
        return Err(BitbucketError::BadArg(format!(
            "`{raw}` is not a Bitbucket id"
        )));
    }
    Ok(format!("%7B{inner}%7D"))
}

#[derive(Deserialize, Default, Clone)]
pub struct Link {
    pub href: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
pub struct Links {
    #[serde(default)]
    pub avatar: Link,
    #[serde(default)]
    pub html: Link,
}

/// A person or an app as Bitbucket describes them. Usernames are gone from
/// its API, so the nickname stands in for the login other hosts have.
#[derive(Deserialize, Default, Clone)]
pub struct User {
    pub uuid: Option<String>,
    pub display_name: Option<String>,
    pub nickname: Option<String>,
    #[serde(default)]
    pub links: Links,
}

impl User {
    pub fn login(&self) -> Option<String> {
        self.nickname
            .clone()
            .or_else(|| self.display_name.clone())
            .filter(|name| !name.is_empty())
    }

    pub fn avatar(&self) -> Option<String> {
        self.links.avatar.href.clone()
    }
}

pub fn login_of(user: Option<&User>) -> Option<String> {
    user.and_then(User::login)
}

pub fn avatar_of(user: Option<&User>) -> Option<String> {
    user.and_then(User::avatar)
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

/// The repository a git remote points at, in any of the ways a remote can be
/// written: scp-like `git@host:ws/repo.git`, or an `https`, `ssh` or `git` URL.
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
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let (owner, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || host.is_empty() || !valid_slug(owner) || !valid_slug(name) {
        return None;
    }
    Some(Repo {
        host: host.to_ascii_lowercase(),
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

#[derive(Deserialize)]
struct RepoRow {
    full_name: String,
    #[serde(default)]
    is_private: bool,
    mainbranch: Option<Branch>,
    updated_on: Option<String>,
    #[serde(default)]
    links: Links,
}

#[derive(Deserialize)]
pub struct Branch {
    pub name: String,
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

pub async fn mine(data_dir: &Path, limit: u32) -> BitbucketResult<Vec<Listing>> {
    let limit = limit.clamp(1, 200);
    let rows: Vec<RepoRow> = client::get_all(
        data_dir,
        "/repositories",
        &[
            ("role", "member".into()),
            ("sort", "-updated_on".into()),
            ("pagelen", limit.min(100).to_string()),
        ],
        limit.div_ceil(100),
    )
    .await?;
    Ok(rows
        .into_iter()
        .take(limit as usize)
        .filter_map(|row| {
            let (owner, name) = row.full_name.split_once('/')?;
            Some(Listing {
                owner: owner.to_string(),
                name: name.to_string(),
                slug: row.full_name.clone(),
                private: row.is_private,
                archived: false,
                default_branch: row.mainbranch.map(|branch| branch.name),
                pushed_at: row.updated_on,
                url: row
                    .links
                    .html
                    .href
                    .unwrap_or_else(|| format!("https://{HOST}/{}", row.full_name)),
            })
        })
        .collect())
}

pub async fn branches(data_dir: &Path, repo: RepoRef) -> BitbucketResult<Vec<String>> {
    let rows: Vec<Branch> = client::get_all(
        data_dir,
        &repo.path("/refs/branches")?,
        &[("pagelen", "100".into()), ("sort", "-target.date".into())],
        5,
    )
    .await?;
    Ok(rows.into_iter().map(|branch| branch.name).collect())
}

#[derive(Deserialize)]
struct RepoDetail {
    mainbranch: Option<Branch>,
}

pub async fn default_branch(data_dir: &Path, repo: &RepoRef) -> BitbucketResult<String> {
    let detail: RepoDetail = client::get(data_dir, &repo.path("")?, &[]).await?;
    detail
        .mainbranch
        .map(|branch| branch.name)
        .ok_or_else(|| BitbucketError::NotFound("the repository has no main branch yet".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(remote: &str) -> Option<String> {
        from_remote(remote).map(|repo| format!("{} {}", repo.host, repo.slug()))
    }

    #[test]
    fn reads_every_way_a_bitbucket_remote_is_written() {
        for remote in [
            "git@bitbucket.org:swishx/api-docs.git",
            "https://nodelike@bitbucket.org/swishx/api-docs.git",
            "https://bitbucket.org/swishx/api-docs",
            "ssh://git@bitbucket.org/swishx/api-docs.git",
            "https://bitbucket.org/swishx/api-docs/",
        ] {
            assert_eq!(
                slug(remote).as_deref(),
                Some("bitbucket.org swishx/api-docs"),
                "{remote}"
            );
        }
    }

    #[test]
    fn a_path_that_is_not_workspace_and_repository_is_nothing() {
        for remote in [
            "/srv/local.git",
            "https://bitbucket.org/only",
            "git@host:a/b/c.git",
            "",
        ] {
            assert!(from_remote(remote).is_none(), "{remote}");
        }
    }

    #[test]
    fn a_path_cannot_climb_out_of_the_repository() {
        let bad = RepoRef {
            owner: "..".into(),
            name: "x".into(),
        };
        assert!(bad.path("").is_err());
        let slashed = RepoRef {
            owner: "a/b".into(),
            name: "x".into(),
        };
        assert!(slashed.path("").is_err());
    }

    #[test]
    fn uuids_keep_their_braces_escaped() -> BitbucketResult<()> {
        assert_eq!(
            uuid_segment("{0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0}")?,
            "%7B0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0%7D"
        );
        assert_eq!(uuid_segment("abc-123")?, "%7Babc-123%7D");
        assert!(uuid_segment("{../x}").is_err());
        assert!(uuid_segment("").is_err());
        Ok(())
    }
}
