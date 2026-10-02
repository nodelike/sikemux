// Which repository is being looked at. A remote URL names one, and it arrives
// in every shape git accepts; the signed-in account can also simply be asked
// which repositories it has.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::client;
use crate::error::{GithubError, GithubResult};

const MAX_NAME: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// Owner and repository names reach the API inside a path, so only the
/// characters GitHub actually allows in one get through.
fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NAME
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn validate(owner: &str, name: &str) -> GithubResult<()> {
    if valid_name(owner) && valid_name(name) {
        return Ok(());
    }
    Err(GithubError::BadArg(format!(
        "`{owner}/{name}` is not a repository name"
    )))
}

fn split_path(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let (owner, name) = trimmed.split_once('/')?;
    // A nested path means this is not a plain `owner/repo` remote.
    if name.contains('/') || !valid_name(owner) || !valid_name(name) {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

/// The repository a git remote URL points at, or nothing when it points
/// somewhere that is not a GitHub-shaped remote.
pub fn from_remote(url: &str) -> Option<Repo> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (host, path) = if let Some(rest) = trimmed.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        (host.to_string(), path.to_string())
    } else if trimmed.contains("://") {
        let parsed = url::Url::parse(trimmed).ok()?;
        let host = parsed.host_str()?;
        // A port on an https remote is where that GitHub serves its API too;
        // one on an ssh remote is only for git.
        let host = match parsed.port() {
            Some(port) if matches!(parsed.scheme(), "https" | "http") => format!("{host}:{port}"),
            _ => host.to_string(),
        };
        (host, parsed.path().to_string())
    } else {
        // `host:owner/repo`, the scp-like form without a user.
        let (host, path) = trimmed.split_once(':')?;
        (host.to_string(), path.to_string())
    };
    let host = host.trim_start_matches("www.").to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let (owner, name) = split_path(&path)?;
    Some(Repo { host, owner, name })
}

#[derive(Deserialize)]
struct OwnerRow {
    login: String,
}

#[derive(Deserialize)]
struct RepoRow {
    name: String,
    owner: OwnerRow,
    private: bool,
    archived: bool,
    default_branch: Option<String>,
    pushed_at: Option<String>,
    html_url: String,
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

/// The repositories the signed-in account can reach, most recently pushed
/// first, so the ones somebody is actually working on come up.
pub async fn mine(data_dir: &Path, limit: u32) -> GithubResult<Vec<Listing>> {
    let rows: Vec<RepoRow> = client::get(
        data_dir,
        "/user/repos",
        &[
            ("per_page", limit.clamp(1, 100).to_string()),
            ("sort", "pushed".to_string()),
            (
                "affiliation",
                "owner,collaborator,organization_member".to_string(),
            ),
        ],
    )
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Listing {
            slug: format!("{}/{}", row.owner.login, row.name),
            owner: row.owner.login,
            name: row.name,
            private: row.private,
            archived: row.archived,
            default_branch: row.default_branch,
            pushed_at: row.pushed_at,
            url: row.html_url,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(host: &str, owner: &str, name: &str) -> Option<Repo> {
        Some(Repo {
            host: host.into(),
            owner: owner.into(),
            name: name.into(),
        })
    }

    #[test]
    fn reads_every_shape_a_github_remote_comes_in() {
        for url in [
            "https://github.com/nodelike/sikemux.git",
            "https://github.com/nodelike/sikemux",
            "git@github.com:nodelike/sikemux.git",
            "ssh://git@github.com/nodelike/sikemux.git",
            "git://github.com/nodelike/sikemux.git",
            "github.com:nodelike/sikemux",
            "  https://www.github.com/nodelike/sikemux.git  ",
        ] {
            assert_eq!(
                from_remote(url),
                repo("github.com", "nodelike", "sikemux"),
                "{url}"
            );
        }
    }

    #[test]
    fn keeps_the_host_of_a_company_github() {
        assert_eq!(
            from_remote("git@git.Example.COM:team/service.git"),
            repo("git.example.com", "team", "service")
        );
    }

    #[test]
    fn keeps_the_port_of_a_company_github_served_over_https() {
        assert_eq!(
            from_remote("https://ghe.corp:8443/team/service.git"),
            repo("ghe.corp:8443", "team", "service")
        );
        assert_eq!(
            from_remote("https://ghe.corp:443/team/service.git"),
            repo("ghe.corp", "team", "service")
        );
        assert_eq!(
            from_remote("ssh://git@ghe.corp:2222/team/service.git"),
            repo("ghe.corp", "team", "service")
        );
    }

    #[test]
    fn turns_down_remotes_that_are_not_one_repository() {
        for url in [
            "",
            "https://github.com/nodelike",
            "https://github.com/a/b/c",
            "https://github.com/../etc",
            "/var/repos/local.git",
        ] {
            assert_eq!(from_remote(url), None, "{url}");
        }
    }

    #[test]
    fn reads_owner_and_repo_out_of_a_path() {
        assert_eq!(
            split_path("/nodelike/sikemux.git"),
            Some(("nodelike".into(), "sikemux".into()))
        );
        assert_eq!(split_path("nodelike"), None);
        assert_eq!(split_path("node like/x"), None);
    }

    #[test]
    fn validates_names_used_as_path_segments() {
        assert!(validate("nodelike", "sikemux.rs").is_ok());
        assert!(validate("", "x").is_err());
        assert!(validate("a/b", "x").is_err());
        assert!(validate("..", "x").is_err());
        assert!(validate("a", &"x".repeat(101)).is_err());
    }
}
