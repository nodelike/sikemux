// The notification inbox: review requests, mentions and the rest of what
// GitHub would otherwise email. A notification points at its subject through
// an API address, which is no use to a person, so it is turned back into the
// page they meant.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::client;
use crate::common::MAX_PER_PAGE;
use crate::error::GithubResult;

#[derive(Deserialize)]
struct SubjectRow {
    title: String,
    url: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct RepositoryRow {
    full_name: String,
}

#[derive(Deserialize)]
struct NotificationRow {
    id: String,
    unread: bool,
    reason: String,
    updated_at: String,
    subject: SubjectRow,
    repository: RepositoryRow,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    /// The thread id, which is what marking it read needs.
    pub id: String,
    pub title: String,
    /// `PullRequest`, `Issue`, `Release`, `CheckSuite`, `Discussion` or `Commit`.
    pub kind: String,
    /// Why it arrived: `review_requested`, `mention`, `assign`, `author` and so on.
    pub reason: String,
    pub repo: String,
    /// The number of the pull request or issue, when it has one.
    pub number: Option<u64>,
    pub unread: bool,
    pub updated_at: String,
    /// Where a person would go to read it, or nothing when GitHub gave no address.
    pub url: Option<String>,
}

/// `.../repos/owner/name/pulls/12` is where the API keeps it;
/// `https://github.com/owner/name/pull/12` is where the person reads it.
fn web_url(host: &str, api_url: &str) -> Option<String> {
    let (_, tail) = api_url.split_once("/repos/")?;
    let mut parts = tail.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    let section = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    let page = match section {
        "pulls" => "pull",
        "issues" => "issues",
        "releases" => "releases",
        "commits" => "commit",
        other => other,
    };
    let base = format!("https://{host}/{owner}/{name}/{page}");
    Some(if rest.is_empty() {
        base
    } else {
        format!("{base}/{}", rest.join("/"))
    })
}

/// Only a pull request or an issue is known by its number. A release's
/// address ends in an internal id nobody would recognise.
fn number_of(api_url: &str) -> Option<u64> {
    let (rest, last) = api_url.rsplit_once('/')?;
    let section = rest.rsplit('/').next()?;
    if !matches!(section, "pulls" | "issues") {
        return None;
    }
    last.parse().ok()
}

#[derive(Deserialize)]
struct ReleasePage {
    html_url: String,
}

/// A release notification points at the release by id, and its web page is
/// by tag, so the release is looked up once to find where it lives.
async fn release_page(data_dir: &Path, api_url: &str) -> Option<String> {
    let (_, tail) = api_url.split_once("/repos/")?;
    let release: ReleasePage = client::get(data_dir, &format!("/repos/{tail}"), &[])
        .await
        .ok()?;
    Some(release.html_url)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    /// Include the ones already read, which the inbox hides by default.
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub per_page: Option<u32>,
}

pub async fn list(data_dir: &Path, input: Query) -> GithubResult<Vec<Notification>> {
    let host = crate::config::load(data_dir).host_of(client::chosen().as_deref());
    let rows: Vec<NotificationRow> = client::get(
        data_dir,
        "/notifications",
        &[
            ("all", input.all.to_string()),
            (
                "per_page",
                input
                    .per_page
                    .unwrap_or(50)
                    .clamp(1, MAX_PER_PAGE)
                    .to_string(),
            ),
        ],
    )
    .await?;
    // Every request waits for one of the plugin's few request slots, so these
    // go out together without flooding GitHub.
    let pages = futures::future::join_all(rows.iter().map(|row| async {
        match (row.subject.kind.as_deref(), row.subject.url.as_deref()) {
            (Some("Release"), Some(url)) => release_page(data_dir, url).await,
            (_, Some(url)) => web_url(&host, url),
            _ => None,
        }
    }))
    .await;
    Ok(rows
        .into_iter()
        .zip(pages)
        .map(|(row, url)| Notification {
            url,
            number: row.subject.url.as_deref().and_then(number_of),
            id: row.id,
            title: row.subject.title,
            kind: row.subject.kind.unwrap_or_else(|| "Unknown".into()),
            reason: row.reason,
            repo: row.repository.full_name,
            unread: row.unread,
            updated_at: row.updated_at,
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRef {
    pub id: String,
}

/// Thread ids are numbers GitHub hands out, so anything else is refused rather
/// than pasted into a path.
fn valid_thread(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_digit())
}

pub async fn mark_read(data_dir: &Path, input: ThreadRef) -> GithubResult<()> {
    if !valid_thread(&input.id) {
        return Err(crate::error::GithubError::BadArg(
            "that is not a notification".into(),
        ));
    }
    let path = format!("/notifications/threads/{}", input.id);
    client::act(data_dir, reqwest::Method::PATCH, &path, None).await
}

pub async fn mark_all_read(data_dir: &Path) -> GithubResult<()> {
    client::act(data_dir, reqwest::Method::PUT, "/notifications", None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_an_api_address_into_the_page_a_person_reads() {
        let host = "github.com";
        assert_eq!(
            web_url(
                host,
                "https://api.github.com/repos/nodelike/sikemux/pulls/12"
            )
            .as_deref(),
            Some("https://github.com/nodelike/sikemux/pull/12")
        );
        assert_eq!(
            web_url(
                host,
                "https://api.github.com/repos/nodelike/sikemux/issues/3"
            )
            .as_deref(),
            Some("https://github.com/nodelike/sikemux/issues/3")
        );
        assert_eq!(
            web_url(host, "https://api.github.com/repos/a/b/commits/deadbeef").as_deref(),
            Some("https://github.com/a/b/commit/deadbeef")
        );
    }

    #[test]
    fn keeps_the_company_github_a_notification_came_from() {
        assert_eq!(
            web_url(
                "git.example.com",
                "https://git.example.com/api/v3/repos/team/svc/pulls/4"
            )
            .as_deref(),
            Some("https://git.example.com/team/svc/pull/4")
        );
    }

    #[test]
    fn says_nothing_for_an_address_it_cannot_read() {
        assert_eq!(web_url("github.com", "https://api.github.com/user"), None);
        assert_eq!(web_url("github.com", "nonsense"), None);
    }

    #[test]
    fn reads_the_number_off_the_end_when_there_is_one() {
        assert_eq!(
            number_of("https://api.github.com/repos/a/b/pulls/12"),
            Some(12)
        );
        assert_eq!(number_of("https://api.github.com/repos/a/b/releases"), None);
        assert_eq!(
            number_of("https://api.github.com/repos/a/b/releases/254930215"),
            None
        );
        assert_eq!(
            number_of("https://api.github.com/repos/a/b/issues/7"),
            Some(7)
        );
    }

    #[test]
    fn only_a_number_is_a_thread() {
        assert!(valid_thread("1234567890"));
        assert!(!valid_thread(""));
        assert!(!valid_thread("../../user"));
        assert!(!valid_thread("12a"));
    }
}
