//! What a release says about itself: its notes, read from the GitHub release,
//! and who made it, read from the commits since the previous release on the
//! same channel. Avatars are fetched here too, because the window only draws
//! images handed to it as `data:` URLs.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use semver::Version;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::http::{client, data_url, fetch_image};

const REPO_API: &str = "https://api.github.com/repos/nodelike/sikemux";
const REPO_WEB: &str = "https://github.com/nodelike/sikemux";
const AVATAR_ORIGIN: &str = "https://avatars.githubusercontent.com/";
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// GitHub lists at most this many commits in one page of a comparison.
const COMMITS_PER_PAGE: usize = 100;
const MAX_COMMIT_PAGES: usize = 20;
/// The modal draws avatars at 30 points, so 64 pixels stays sharp on a retina screen.
const AVATAR_PIXELS: u32 = 64;
const MAX_AVATAR_BYTES: usize = 64 * 1024;
const MAX_AVATARS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contributor {
    login: String,
    name: String,
    commits: u32,
    avatar: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseNotes {
    version: String,
    notes: Option<String>,
    date: Option<String>,
    commits: Option<u32>,
    compare: Option<String>,
    contributors: Vec<Contributor>,
}

#[derive(Deserialize)]
struct GithubRelease {
    body: Option<String>,
    published_at: Option<String>,
}

#[derive(Deserialize)]
struct GithubRef {
    #[serde(rename = "ref")]
    name: String,
}

#[derive(Deserialize)]
struct Comparison {
    total_commits: u32,
    #[serde(default)]
    commits: Vec<ComparedCommit>,
}

#[derive(Deserialize)]
struct ComparedCommit {
    author: Option<Account>,
    commit: CommitDetail,
}

#[derive(Deserialize)]
struct Account {
    login: String,
    avatar_url: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct CommitDetail {
    author: Option<CommitAuthor>,
}

#[derive(Deserialize)]
struct CommitAuthor {
    name: String,
}

/// Credits a release carries in its update manifest, so they show without asking GitHub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseCredits {
    commits: u32,
    compare: String,
    contributors: Vec<Contributor>,
    /// `data:` URLs keyed by each contributor's avatar address.
    avatars: HashMap<String, String>,
}

/// The manifest is not signed, so only links back to this repository and inline images are kept.
pub fn bundled_credits(manifest: &serde_json::Value) -> Option<ReleaseCredits> {
    let mut credits = ReleaseCredits::deserialize(manifest.get("credits")?).ok()?;
    if !credits.compare.starts_with(&format!("{REPO_WEB}/compare/")) {
        return None;
    }
    credits
        .contributors
        .retain(|person| person.avatar.starts_with(AVATAR_ORIGIN));
    credits.avatars.retain(|url, data| {
        url.starts_with(AVATAR_ORIGIN)
            && ["png", "jpeg", "gif", "webp"]
                .iter()
                .any(|kind| data.starts_with(&format!("data:image/{kind};base64,")))
    });
    Some(credits)
}

/// The notes of one published release, and everyone who committed to it.
/// The notes are what matter, so a failed contributor lookup leaves the
/// credits empty rather than failing the whole answer.
pub async fn release_notes(version: String) -> Result<ReleaseNotes, String> {
    let parsed = Version::parse(&version)
        .map_err(|_| "a release version must be a semantic version".to_owned())?;
    if let Some(known) = cached_notes(&version) {
        return Ok(known);
    }
    let tag = format!("v{version}");
    let release: GithubRelease = get_json(&format!("{REPO_API}/releases/tags/{tag}"))
        .await
        .map_err(|error| format!("GitHub has no release {tag}: {error}"))?;
    let credits = match previous_tag(&parsed).await {
        Some(previous) => credits_between(&previous, &tag).await,
        None => None,
    };
    let notes = ReleaseNotes {
        version: version.clone(),
        notes: release.body.filter(|body| !body.trim().is_empty()),
        date: release.published_at,
        commits: credits.as_ref().map(|credits| credits.commits),
        compare: credits.as_ref().map(|credits| credits.compare.clone()),
        contributors: credits
            .map(|credits| credits.contributors)
            .unwrap_or_default(),
    };
    if notes.compare.is_some() {
        remember_notes(&version, notes.clone());
    }
    Ok(notes)
}

async fn previous_tag(version: &Version) -> Option<String> {
    let refs: Vec<GithubRef> = get_json(&format!("{REPO_API}/git/matching-refs/tags/v"))
        .await
        .ok()?;
    let tags = refs
        .iter()
        .filter_map(|reference| reference.name.strip_prefix("refs/tags/"));
    previous_release(version, tags)
}

/// A nightly follows whatever shipped last; a stable release follows the last stable one.
fn previous_release<'a>(version: &Version, tags: impl Iterator<Item = &'a str>) -> Option<String> {
    tags.filter_map(|tag| {
        let candidate = Version::parse(tag.strip_prefix('v')?).ok()?;
        let comparable = !version.pre.is_empty() || candidate.pre.is_empty();
        (comparable && candidate < *version).then_some((candidate, tag))
    })
    .max_by(|(a, _), (b, _)| a.cmp(b))
    .map(|(_, tag)| tag.to_owned())
}

async fn credits_between(previous: &str, tag: &str) -> Option<ReleaseCredits> {
    let mut commits = Vec::new();
    let mut total = 0;
    for page in 1..=MAX_COMMIT_PAGES {
        let comparison: Comparison = get_json(&format!(
            "{REPO_API}/compare/{previous}...{tag}?per_page={COMMITS_PER_PAGE}&page={page}"
        ))
        .await
        .ok()?;
        total = comparison.total_commits;
        let fetched = comparison.commits.len();
        commits.extend(comparison.commits);
        if fetched < COMMITS_PER_PAGE || commits.len() >= total as usize {
            break;
        }
    }
    Some(ReleaseCredits {
        commits: total,
        compare: format!("{REPO_WEB}/compare/{previous}...{tag}"),
        contributors: tally(commits),
        avatars: HashMap::new(),
    })
}

/// People ordered by how many commits they made. Commits with no GitHub
/// account behind them, and those made by bots, credit nobody.
fn tally(commits: Vec<ComparedCommit>) -> Vec<Contributor> {
    let mut people: Vec<Contributor> = Vec::new();
    for commit in commits {
        let Some(account) = commit.author.filter(|account| account.kind == "User") else {
            continue;
        };
        match people
            .iter_mut()
            .find(|person| person.login == account.login)
        {
            Some(person) => person.commits += 1,
            None => people.push(Contributor {
                name: commit
                    .commit
                    .author
                    .map(|author| author.name)
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| account.login.clone()),
                login: account.login,
                commits: 1,
                avatar: account.avatar_url,
            }),
        }
    }
    people.sort_by_key(|person| std::cmp::Reverse(person.commits));
    people
}

async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    let response = client()
        .map_err(|error| error.to_string())?
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(response.status().to_string());
    }
    response.json().await.map_err(|error| error.to_string())
}

fn notes_cache() -> &'static Mutex<HashMap<String, ReleaseNotes>> {
    static CACHE: OnceLock<Mutex<HashMap<String, ReleaseNotes>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cached_notes(version: &str) -> Option<ReleaseNotes> {
    notes_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(version)
        .cloned()
}

fn remember_notes(version: &str, notes: ReleaseNotes) {
    notes_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(version.to_owned(), notes);
}

/// Avatars keyed by the address GitHub gave. One that fails to load is left
/// out, and the modal draws the contributor's initial instead.
pub async fn release_avatars(urls: Vec<String>) -> HashMap<String, String> {
    let mut fetching = JoinSet::new();
    for url in urls
        .into_iter()
        .filter(|url| url.starts_with(AVATAR_ORIGIN))
        .take(MAX_AVATARS)
    {
        fetching.spawn(async move {
            if let Some(known) = cached_avatar(&url) {
                return (url, Some(known));
            }
            let data = fetch_avatar(&url).await;
            if let Some(data) = &data {
                remember_avatar(&url, data.clone());
            }
            (url, data)
        });
    }
    let mut found = HashMap::new();
    while let Some(Ok((url, data))) = fetching.join_next().await {
        if let Some(data) = data {
            found.insert(url, data);
        }
    }
    found
}

fn avatar_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cached_avatar(url: &str) -> Option<String> {
    avatar_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(url)
        .cloned()
}

fn remember_avatar(url: &str, data: String) {
    avatar_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(url.to_owned(), data);
}

fn sized(url: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}s={AVATAR_PIXELS}")
}

async fn fetch_avatar(url: &str) -> Option<String> {
    let request = client().ok()?.get(sized(url)).timeout(FETCH_TIMEOUT);
    let (bytes, mime) = fetch_image(request, MAX_AVATAR_BYTES).await?;
    Some(data_url(mime, &bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TAGS: [&str; 7] = [
        "v0.3.5",
        "v0.4.0-nightly.8",
        "v0.4.0-nightly.9",
        "v0.4.0-nightly.10",
        "v0.4.0",
        "v0.4.1-nightly.1",
        "nightly",
    ];

    fn previous(version: &str) -> Option<String> {
        previous_release(&Version::parse(version).unwrap(), TAGS.into_iter())
    }

    #[test]
    fn a_nightly_follows_whatever_shipped_last() {
        assert_eq!(
            previous("0.4.0-nightly.10").as_deref(),
            Some("v0.4.0-nightly.9")
        );
        assert_eq!(previous("0.4.1-nightly.1").as_deref(), Some("v0.4.0"));
    }

    #[test]
    fn a_stable_release_follows_the_last_stable_one() {
        assert_eq!(previous("0.4.0").as_deref(), Some("v0.3.5"));
        assert_eq!(previous("0.3.5"), None);
    }

    fn commit(login: Option<(&str, &str)>, name: &str) -> ComparedCommit {
        serde_json::from_value(json!({
            "author": login.map(|(login, kind)| json!({
                "login": login,
                "avatar_url": format!("https://avatars.githubusercontent.com/{login}"),
                "type": kind
            })),
            "commit": { "author": { "name": name } }
        }))
        .unwrap()
    }

    #[test]
    fn credits_people_by_commit_count_and_skips_bots_and_strangers() {
        let people = tally(vec![
            commit(Some(("octocat", "User")), "The Octocat"),
            commit(Some(("nodelike", "User")), "NØDE"),
            commit(Some(("nodelike", "User")), "nodelike"),
            commit(Some(("dependabot[bot]", "Bot")), "dependabot"),
            commit(None, "someone offline"),
        ]);
        assert_eq!(
            people,
            vec![
                Contributor {
                    login: "nodelike".into(),
                    name: "NØDE".into(),
                    commits: 2,
                    avatar: "https://avatars.githubusercontent.com/nodelike".into(),
                },
                Contributor {
                    login: "octocat".into(),
                    name: "The Octocat".into(),
                    commits: 1,
                    avatar: "https://avatars.githubusercontent.com/octocat".into(),
                },
            ]
        );
    }

    #[test]
    fn keeps_only_repository_links_and_inline_avatars_from_a_manifest() {
        let avatar = "https://avatars.githubusercontent.com/u/1?v=4";
        let manifest = json!({
            "credits": {
                "commits": 3,
                "compare": "https://github.com/nodelike/sikemux/compare/v0.4.1...v0.4.2",
                "contributors": [
                    { "login": "nodelike", "name": "NØDE", "commits": 2, "avatar": avatar },
                    { "login": "stranger", "name": "Stranger", "commits": 1, "avatar": "https://example.com/a.png" }
                ],
                "avatars": {
                    avatar: "data:image/png;base64,AAAA",
                    "https://avatars.githubusercontent.com/u/2": "https://example.com/tracker.png"
                }
            }
        });
        let credits = bundled_credits(&manifest).expect("credits");
        assert_eq!(credits.commits, 3);
        assert_eq!(
            credits
                .contributors
                .iter()
                .map(|person| person.login.as_str())
                .collect::<Vec<_>>(),
            ["nodelike"]
        );
        assert_eq!(credits.avatars.keys().collect::<Vec<_>>(), [avatar]);

        let mut elsewhere = manifest.clone();
        elsewhere["credits"]["compare"] = json!("https://example.com/compare/a...b");
        assert_eq!(bundled_credits(&elsewhere), None);
        assert_eq!(bundled_credits(&json!({ "version": "0.4.2" })), None);
    }

    #[test]
    fn asks_github_for_a_small_avatar() {
        assert_eq!(
            sized("https://avatars.githubusercontent.com/u/1?v=4"),
            "https://avatars.githubusercontent.com/u/1?v=4&s=64"
        );
        assert_eq!(
            sized("https://avatars.githubusercontent.com/u/1"),
            "https://avatars.githubusercontent.com/u/1?s=64"
        );
    }

    #[test]
    #[ignore = "requires network access"]
    fn release_avatars_reads_github() {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let urls = vec![
            "https://avatars.githubusercontent.com/u/95223229?v=4".to_owned(),
            "https://avatars.githubusercontent.com/u/108696612?v=4".to_owned(),
        ];
        let found = runtime.block_on(release_avatars(urls.clone()));
        for url in &urls {
            assert!(
                found
                    .get(url)
                    .is_some_and(|data| data.starts_with("data:image/")),
                "{url}"
            );
        }
    }

    // Network check, excluded from the normal suite. Run with
    // `cargo test -p sikemux-core --lib -- --ignored release_notes_reads_github`.
    #[test]
    #[ignore = "requires network access"]
    fn release_notes_reads_github() {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let notes = runtime
            .block_on(release_notes("0.4.0-nightly.10".into()))
            .expect("notes for a published nightly");
        assert!(notes.notes.is_some_and(|body| body.contains("nightly")));
        assert_eq!(notes.commits, Some(8));
        assert!(notes
            .contributors
            .iter()
            .any(|person| person.login == "nodelike"));
    }
}
