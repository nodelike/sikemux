// Pull requests: listing and reading them, the files and commits in one, what
// happened on it, and approving, merging or declining one.

use std::collections::BTreeMap;
use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client;
use crate::error::{BitbucketError, BitbucketResult};
use crate::repo::{self, avatar_of, login_of, Links, RepoRef, User};

#[derive(Deserialize, Default)]
struct Commit {
    hash: Option<String>,
}

#[derive(Deserialize, Default)]
struct FullName {
    full_name: Option<String>,
}

#[derive(Deserialize, Default)]
struct Endpoint {
    branch: Option<repo::Branch>,
    #[serde(default)]
    commit: Commit,
    repository: Option<FullName>,
}

#[derive(Deserialize)]
struct Participant {
    user: Option<User>,
    role: Option<String>,
    #[serde(default)]
    approved: bool,
    state: Option<String>,
    participated_on: Option<String>,
}

#[derive(Deserialize)]
struct PullRow {
    id: u64,
    title: String,
    description: Option<String>,
    state: String,
    #[serde(default)]
    draft: bool,
    author: Option<User>,
    #[serde(default)]
    source: Endpoint,
    #[serde(default)]
    destination: Endpoint,
    created_on: String,
    updated_on: String,
    comment_count: Option<u64>,
    merge_commit: Option<Commit>,
    closed_by: Option<User>,
    #[serde(default)]
    reviewers: Vec<User>,
    #[serde(default)]
    participants: Vec<Participant>,
    #[serde(default)]
    links: Links,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pull {
    pub number: u64,
    pub title: String,
    pub body: String,
    /// `open`, `merged`, or `closed` for one declined or superseded.
    pub state: &'static str,
    pub draft: bool,
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub author_association: Option<String>,
    pub head: Option<String>,
    /// `workspace:branch`, which names the fork a branch lives on.
    pub head_label: Option<String>,
    pub base: Option<String>,
    pub head_sha: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub comments: Option<u64>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub changed_files: Option<u64>,
    pub mergeable: Option<bool>,
    pub merge_state: Option<String>,
    pub labels: Vec<Value>,
    pub reviewers: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    pub commits: Option<u64>,
    pub merged_at: Option<String>,
    pub merged_by: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub avatars: BTreeMap<String, String>,
    pub url: String,
}

fn state_of(raw: &str) -> &'static str {
    match raw {
        "OPEN" => "open",
        "MERGED" => "merged",
        _ => "closed",
    }
}

impl Pull {
    fn from_row(repo: &RepoRef, row: PullRow) -> Self {
        let mut avatars = BTreeMap::new();
        let mut reviewers = Vec::new();
        let people = row.reviewers.iter().chain(
            row.participants
                .iter()
                .filter(|participant| participant.role.as_deref() == Some("REVIEWER"))
                .filter_map(|participant| participant.user.as_ref()),
        );
        for user in people {
            if let Some(login) = user.login() {
                if let Some(avatar) = user.avatar() {
                    avatars.insert(login.clone(), avatar);
                }
                if !reviewers.contains(&login) {
                    reviewers.push(login);
                }
            }
        }
        let head = row.source.branch.map(|branch| branch.name);
        let fork = row
            .source
            .repository
            .and_then(|repository| repository.full_name)
            .and_then(|name| name.split_once('/').map(|(owner, _)| owner.to_string()));
        let merged = row.state == "MERGED";
        Pull {
            number: row.id,
            title: row.title,
            body: row.description.unwrap_or_default(),
            state: state_of(&row.state),
            draft: row.draft,
            author: login_of(row.author.as_ref()),
            avatar_url: avatar_of(row.author.as_ref()),
            author_association: None,
            head_label: head
                .as_ref()
                .map(|branch| format!("{}:{branch}", fork.unwrap_or_else(|| repo.owner.clone()))),
            head,
            base: row.destination.branch.map(|branch| branch.name),
            head_sha: row.source.commit.hash,
            created_at: row.created_on,
            merged_at: merged.then(|| row.updated_on.clone()),
            updated_at: row.updated_on,
            comments: row.comment_count,
            additions: None,
            deletions: None,
            changed_files: None,
            mergeable: None,
            merge_state: None,
            labels: Vec::new(),
            reviewers,
            assignees: Vec::new(),
            milestone: None,
            commits: None,
            merged_by: merged.then(|| login_of(row.closed_by.as_ref())).flatten(),
            merge_commit_sha: row.merge_commit.and_then(|commit| commit.hash),
            avatars,
            url: row
                .links
                .html
                .href
                .unwrap_or_else(|| repo.web(&format!("/pull-requests/{}", row.id))),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(default = "open")]
    pub state: String,
}

fn open() -> String {
    "open".into()
}

fn states_for(state: &str) -> &'static [&'static str] {
    match state {
        "closed" => &["MERGED", "DECLINED", "SUPERSEDED"],
        "all" => &["OPEN", "MERGED", "DECLINED", "SUPERSEDED"],
        _ => &["OPEN"],
    }
}

pub async fn list(data_dir: &Path, input: ListQuery) -> BitbucketResult<Vec<Pull>> {
    let mut query: Vec<(&str, String)> = states_for(&input.state)
        .iter()
        .map(|state| ("state", state.to_string()))
        .collect();
    query.push(("pagelen", "50".into()));
    query.push(("sort", "-updated_on".into()));
    let rows: Vec<PullRow> =
        client::get_all(data_dir, &input.repo.path("/pullrequests")?, &query, 1).await?;
    Ok(rows
        .into_iter()
        .map(|row| Pull::from_row(&input.repo, row))
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
}

fn pull_path(repo: &RepoRef, number: u64, rest: &str) -> BitbucketResult<String> {
    repo.path(&format!("/pullrequests/{number}{rest}"))
}

#[derive(Deserialize)]
struct DiffStat {
    status: Option<String>,
    #[serde(default)]
    lines_added: u64,
    #[serde(default)]
    lines_removed: u64,
    old: Option<FilePath>,
    new: Option<FilePath>,
}

#[derive(Deserialize)]
struct FilePath {
    path: String,
}

async fn diffstat(data_dir: &Path, repo: &RepoRef, number: u64) -> BitbucketResult<Vec<DiffStat>> {
    client::get_all(
        data_dir,
        &pull_path(repo, number, "/diffstat")?,
        &[("pagelen", "100".into())],
        30,
    )
    .await
}

/// Bitbucket gives a pull request's head as a short hash. Checks and merges
/// need all of it, which the commit itself has.
async fn full_hash(data_dir: &Path, repo: &RepoRef, short: &str) -> BitbucketResult<String> {
    if short.len() >= 40 || !short.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(short.to_string());
    }
    let commit: Commit =
        client::get(data_dir, &repo.path(&format!("/commit/{short}"))?, &[]).await?;
    Ok(commit.hash.unwrap_or_else(|| short.to_string()))
}

async fn row(data_dir: &Path, repo: &RepoRef, number: u64) -> BitbucketResult<PullRow> {
    client::get(data_dir, &pull_path(repo, number, "")?, &[]).await
}

pub async fn get(data_dir: &Path, input: Thread) -> BitbucketResult<Pull> {
    let (row, stats) = futures::try_join!(
        row(data_dir, &input.repo, input.number),
        diffstat(data_dir, &input.repo, input.number)
    )?;
    let mut pull = Pull::from_row(&input.repo, row);
    if let Some(short) = pull.head_sha.clone() {
        pull.head_sha = Some(full_hash(data_dir, &input.repo, &short).await?);
    }
    pull.additions = Some(stats.iter().map(|stat| stat.lines_added).sum());
    pull.deletions = Some(stats.iter().map(|stat| stat.lines_removed).sum());
    pull.changed_files = Some(stats.len() as u64);
    Ok(pull)
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// Says which part of the patch this is and how to read the rest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

const PATCH_LINES: usize = 400;
const MAX_PATCH_LINES: usize = 1000;
const PATCH_BYTES: usize = 32 * 1024;

/// Agents get the file list alone unless they name `paths`, and then a window
/// of each patch. The pull request view asks for `fullPatches`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchQuery {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub offset: usize,
    pub lines: Option<usize>,
    #[serde(default)]
    pub full_patches: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesQuery {
    #[serde(flatten)]
    pub pull: Thread,
    #[serde(flatten)]
    pub patches: PatchQuery,
}

fn cut_at_char(text: &str, most: usize) -> &str {
    text.get(..text.floor_char_boundary(most))
        .unwrap_or_default()
}

/// Up to `lines` lines of `patch` from `offset`, and at most `PATCH_BYTES` of
/// them, with a note whenever any of the patch is left out.
fn window(patch: &str, offset: usize, lines: Option<usize>) -> (String, Option<String>) {
    let all: Vec<&str> = patch.split('\n').collect();
    let total = all.len();
    if offset >= total {
        return (
            String::new(),
            Some(format!(
                "The patch has {total} lines; offset {offset} is past its end."
            )),
        );
    }
    let most = lines.unwrap_or(PATCH_LINES).clamp(1, MAX_PATCH_LINES);
    let mut shown = String::new();
    let mut end = offset;
    let mut line_cut = false;
    for line in all.iter().skip(offset).take(most) {
        let room = PATCH_BYTES.saturating_sub(shown.len() + 1);
        if line.len() > room {
            if end == offset {
                shown.push_str(cut_at_char(line, room));
                line_cut = true;
                end += 1;
            }
            break;
        }
        if end > offset {
            shown.push('\n');
        }
        shown.push_str(line);
        end += 1;
    }
    if offset == 0 && end == total && !line_cut {
        return (shown, None);
    }
    let mut note = format!("Lines {}-{end} of {total}.", offset + 1);
    if line_cut {
        note.push_str(&format!(
            " Line {end} was cut at {} KB.",
            PATCH_BYTES / 1024
        ));
    }
    if end < total {
        note.push_str(&format!(
            " {} more lines left out: pass offset {end} for the next part, or run git diff locally.",
            total - end
        ));
    }
    (shown, Some(note))
}

fn is_named(file: &ChangedFile, path: &str) -> bool {
    file.path == path || file.previous_path.as_deref() == Some(path)
}

/// The files to answer with, carrying the patches `query` asks for.
fn pick(files: Vec<ChangedFile>, query: &PatchQuery) -> BitbucketResult<Vec<ChangedFile>> {
    if query.full_patches {
        return Ok(files);
    }
    if let Some(unknown) = query
        .paths
        .iter()
        .find(|path| !files.iter().any(|file| is_named(file, path)))
    {
        return Err(BitbucketError::BadArg(format!(
            "`{unknown}` is not among the files this pull request changes"
        )));
    }
    Ok(files
        .into_iter()
        .filter_map(|mut file| {
            if query.paths.is_empty() {
                file.patch = None;
                return Some(file);
            }
            if !query.paths.iter().any(|path| is_named(&file, path)) {
                return None;
            }
            match file.patch.take() {
                Some(patch) => {
                    let (shown, note) = window(&patch, query.offset, query.lines);
                    file.patch = Some(shown);
                    file.note = note;
                }
                None => {
                    file.note = Some(
                        "Bitbucket shows no patch for this file: it is binary or only renamed. Run git diff locally."
                            .into(),
                    );
                }
            }
            Some(file)
        })
        .collect())
}

/// Each file's hunks out of one unified diff, keyed by the path it ends up
/// at, or the path it had when it was deleted. A binary file has no hunks.
fn patches_of(diff: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for section in diff
        .split("\ndiff --git ")
        .map(|section| section.strip_prefix("diff --git ").unwrap_or(section))
    {
        let mut old = None;
        let mut new = None;
        let mut hunks = None;
        for (index, line) in section.lines().enumerate() {
            if let Some(path) = line.strip_prefix("--- a/") {
                old = Some(path.to_string());
            } else if let Some(path) = line.strip_prefix("+++ b/") {
                new = Some(path.to_string());
            } else if line.starts_with("@@") {
                hunks = Some(section.lines().skip(index).collect::<Vec<_>>().join("\n"));
                break;
            }
        }
        if let (Some(path), Some(hunks)) = (new.or(old), hunks) {
            found.insert(path, hunks);
        }
    }
    found
}

fn changed_file(stat: DiffStat) -> Option<ChangedFile> {
    let old = stat.old.map(|file| file.path);
    let path = stat.new.map(|file| file.path).or_else(|| old.clone())?;
    let status = match stat.status.as_deref() {
        Some("added") => "added",
        Some("removed") => "removed",
        Some("renamed") => "renamed",
        _ => "modified",
    };
    Some(ChangedFile {
        previous_path: old.filter(|old| *old != path),
        path,
        status: status.into(),
        additions: stat.lines_added,
        deletions: stat.lines_removed,
        patch: None,
        note: None,
    })
}

pub async fn files(data_dir: &Path, input: FilesQuery) -> BitbucketResult<Vec<ChangedFile>> {
    let FilesQuery {
        pull,
        patches: query,
    } = input;
    if query.paths.is_empty() && !query.full_patches {
        let stats = diffstat(data_dir, &pull.repo, pull.number).await?;
        return Ok(stats.into_iter().filter_map(changed_file).collect());
    }
    let diff_path = pull_path(&pull.repo, pull.number, "/diff")?;
    let (stats, diff) = futures::try_join!(
        diffstat(data_dir, &pull.repo, pull.number),
        client::get_text(data_dir, &diff_path, &[])
    )?;
    let mut patches = patches_of(&diff);
    let files = stats
        .into_iter()
        .filter_map(changed_file)
        .map(|mut file| {
            file.patch = patches.remove(&file.path);
            file
        })
        .collect();
    pick(files, &query)
}

#[derive(Deserialize)]
struct CommitAuthor {
    raw: Option<String>,
    user: Option<User>,
}

#[derive(Deserialize)]
struct CommitRow {
    hash: String,
    message: Option<String>,
    date: Option<String>,
    author: Option<CommitAuthor>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullCommit {
    pub sha: String,
    pub message: String,
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub date: Option<String>,
}

/// `Ada Lovelace <ada@example.com>` without the address.
fn name_of_raw(raw: &str) -> Option<String> {
    let name = raw.split('<').next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

impl From<CommitRow> for PullCommit {
    fn from(row: CommitRow) -> Self {
        let user = row.author.as_ref().and_then(|author| author.user.as_ref());
        PullCommit {
            author: login_of(user).or_else(|| {
                row.author
                    .as_ref()
                    .and_then(|author| author.raw.as_deref())
                    .and_then(name_of_raw)
            }),
            avatar_url: avatar_of(user),
            sha: row.hash,
            message: row.message.unwrap_or_default().trim_end().to_string(),
            date: row.date,
        }
    }
}

async fn commit_rows(
    data_dir: &Path,
    repo: &RepoRef,
    number: u64,
) -> BitbucketResult<Vec<PullCommit>> {
    let rows: Vec<CommitRow> = client::get_all(
        data_dir,
        &pull_path(repo, number, "/commits")?,
        &[("pagelen", "100".into())],
        5,
    )
    .await?;
    let mut commits: Vec<PullCommit> = rows.into_iter().map(PullCommit::from).collect();
    commits.reverse();
    Ok(commits)
}

/// `ada@example.com` out of `Ada Lovelace <ada@example.com>`.
fn email_of_raw(raw: &str) -> Option<String> {
    let (_, rest) = raw.split_once('<')?;
    let email = rest.split('>').next()?.trim();
    email.contains('@').then(|| email.to_ascii_lowercase())
}

/// Who wrote a commit, by the email in it, and the account Bitbucket matched that email to.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownAuthor {
    pub email: String,
    pub login: String,
    pub avatar_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitsRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    /// A branch, tag or commit to list from; every branch when absent.
    pub git_ref: Option<String>,
}

fn known_authors(rows: Vec<CommitRow>) -> Vec<KnownAuthor> {
    let mut known: Vec<KnownAuthor> = Vec::new();
    for author in rows.into_iter().filter_map(|row| row.author) {
        let (Some(email), Some(user)) = (author.raw.as_deref().and_then(email_of_raw), author.user)
        else {
            continue;
        };
        let (Some(login), Some(avatar_url)) = (user.login(), user.avatar()) else {
            continue;
        };
        if known.iter().all(|seen| seen.email != email) {
            known.push(KnownAuthor {
                email,
                login,
                avatar_url,
            });
        }
    }
    known
}

/// The accounts behind the latest hundred commits' emails, so a git history
/// read locally, which has only names and emails, shows the same faces as the
/// pull requests and pipelines do.
pub async fn commit_authors(
    data_dir: &Path,
    input: CommitsRef,
) -> BitbucketResult<Vec<KnownAuthor>> {
    let mut query = vec![("pagelen", "100".to_string())];
    if let Some(git_ref) = input.git_ref.filter(|git_ref| !git_ref.is_empty()) {
        query.push(("include", git_ref));
    }
    let rows: Vec<CommitRow> =
        client::get_all(data_dir, &input.repo.path("/commits")?, &query, 1).await?;
    Ok(known_authors(rows))
}

/// Oldest first, as the other hosts list them; Bitbucket gives them newest first.
pub async fn commits(data_dir: &Path, input: Thread) -> BitbucketResult<Vec<PullCommit>> {
    commit_rows(data_dir, &input.repo, input.number).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub state: &'static str,
    pub body: String,
    pub submitted_at: Option<String>,
}

fn verdict(approved: bool, state: Option<&str>) -> Option<&'static str> {
    match state {
        Some("changes_requested") => Some("CHANGES_REQUESTED"),
        Some("approved") => Some("APPROVED"),
        _ if approved => Some("APPROVED"),
        _ => None,
    }
}

/// Bitbucket keeps where each reviewer stands now rather than a history of
/// reviews, so each reviewer has one.
pub async fn reviews(data_dir: &Path, input: Thread) -> BitbucketResult<Vec<Review>> {
    let row = row(data_dir, &input.repo, input.number).await?;
    Ok(row
        .participants
        .into_iter()
        .filter_map(|participant| {
            Some(Review {
                state: verdict(participant.approved, participant.state.as_deref())?,
                author: login_of(participant.user.as_ref()),
                avatar_url: avatar_of(participant.user.as_ref()),
                body: String::new(),
                submitted_at: participant.participated_on,
            })
        })
        .collect())
}

#[derive(Deserialize)]
struct Content {
    raw: Option<String>,
}

#[derive(Deserialize)]
struct CommentRow {
    id: u64,
    content: Option<Content>,
    user: Option<User>,
    created_on: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    links: Links,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: u64,
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub author_association: Option<String>,
    pub body: String,
    pub created_at: String,
    pub url: Option<String>,
}

pub async fn comments(data_dir: &Path, input: Thread) -> BitbucketResult<Vec<Comment>> {
    let rows: Vec<CommentRow> = client::get_all(
        data_dir,
        &pull_path(&input.repo, input.number, "/comments")?,
        &[("pagelen", "100".into()), ("sort", "created_on".into())],
        10,
    )
    .await?;
    Ok(rows
        .into_iter()
        .filter(|row| !row.deleted)
        .map(|row| Comment {
            id: row.id,
            author: login_of(row.user.as_ref()),
            avatar_url: avatar_of(row.user.as_ref()),
            author_association: None,
            body: row
                .content
                .and_then(|content| content.raw)
                .unwrap_or_default(),
            created_at: row.created_on,
            url: row.links.html.href,
        })
        .collect())
}

#[derive(Deserialize)]
pub struct NewComment {
    #[serde(flatten)]
    pub thread: Thread,
    pub body: String,
}

pub async fn add_comment(data_dir: &Path, input: NewComment) -> BitbucketResult<()> {
    let path = pull_path(&input.thread.repo, input.thread.number, "/comments")?;
    let _: Value = client::send_json(
        data_dir,
        Method::POST,
        &path,
        &json!({ "content": { "raw": input.body } }),
    )
    .await?;
    Ok(())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    pub kind: &'static str,
    pub id: Option<u64>,
    pub actor: Option<String>,
    pub avatar_url: Option<String>,
    pub association: Option<String>,
    pub at: Option<String>,
    pub body: Option<String>,
    pub state: Option<String>,
    pub sha: Option<String>,
    pub message: Option<String>,
    pub subject: Option<String>,
}

#[derive(Deserialize)]
struct Verdict {
    date: Option<String>,
    user: Option<User>,
}

#[derive(Deserialize)]
struct Update {
    state: Option<String>,
    title: Option<String>,
    date: Option<String>,
    author: Option<User>,
    #[serde(default)]
    changes: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct Activity {
    comment: Option<CommentRow>,
    approval: Option<Verdict>,
    changes_requested: Option<Verdict>,
    update: Option<Update>,
}

fn timeline_of(activity: Activity, merge_sha: Option<&str>) -> Option<TimelineItem> {
    if let Some(comment) = activity.comment.filter(|comment| !comment.deleted) {
        return Some(TimelineItem {
            kind: "commented",
            id: Some(comment.id),
            actor: login_of(comment.user.as_ref()),
            avatar_url: avatar_of(comment.user.as_ref()),
            at: Some(comment.created_on),
            body: comment.content.and_then(|content| content.raw),
            ..TimelineItem::default()
        });
    }
    let reviewed = |verdict: Verdict, state: &str| TimelineItem {
        kind: "reviewed",
        actor: login_of(verdict.user.as_ref()),
        avatar_url: avatar_of(verdict.user.as_ref()),
        at: verdict.date,
        state: Some(state.into()),
        ..TimelineItem::default()
    };
    if let Some(approval) = activity.approval {
        return Some(reviewed(approval, "approved"));
    }
    if let Some(asked) = activity.changes_requested {
        return Some(reviewed(asked, "changes_requested"));
    }
    let update = activity.update?;
    let event = |kind| TimelineItem {
        kind,
        actor: login_of(update.author.as_ref()),
        avatar_url: avatar_of(update.author.as_ref()),
        at: update.date.clone(),
        ..TimelineItem::default()
    };
    match update.state.as_deref() {
        Some("MERGED") => Some(TimelineItem {
            sha: merge_sha.map(str::to_string),
            ..event("merged")
        }),
        Some("DECLINED" | "SUPERSEDED") => Some(event("closed")),
        _ if update.changes.contains_key("title") => Some(TimelineItem {
            subject: update.title.clone(),
            ..event("renamed")
        }),
        _ => None,
    }
}

/// What happened on a pull request, oldest first, with its commits among the
/// comments and verdicts the way the other hosts tell it.
pub async fn timeline(data_dir: &Path, input: Thread) -> BitbucketResult<Vec<TimelineItem>> {
    let activity_path = pull_path(&input.repo, input.number, "/activity")?;
    let activity_query = [("pagelen", "50".to_string())];
    let (row, activity, commits) = futures::try_join!(
        row(data_dir, &input.repo, input.number),
        client::get_all::<Activity>(data_dir, &activity_path, &activity_query, 10),
        commit_rows(data_dir, &input.repo, input.number)
    )?;
    let merge_sha = row.merge_commit.and_then(|commit| commit.hash);
    let mut items: Vec<TimelineItem> = activity
        .into_iter()
        .filter_map(|activity| timeline_of(activity, merge_sha.as_deref()))
        .chain(commits.into_iter().map(|commit| TimelineItem {
            kind: "committed",
            actor: commit.author,
            avatar_url: commit.avatar_url,
            at: commit.date,
            sha: Some(commit.sha),
            message: Some(commit.message),
            ..TimelineItem::default()
        }))
        .collect();
    items.sort_by(|a, b| a.at.cmp(&b.at));
    Ok(items)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Merge {
    #[serde(flatten)]
    pub thread: Thread,
    pub method: String,
    /// The head commit the person saw; the merge is refused if the branch has moved since.
    pub sha: String,
}

fn strategy_of(method: &str) -> BitbucketResult<&'static str> {
    match method {
        "merge" => Ok("merge_commit"),
        "squash" => Ok("squash"),
        _ => Err(BitbucketError::Unsupported("merge that way")),
    }
}

/// Bitbucket's merge takes no expected head, so the head is checked first.
/// That leaves a moment in which a push can still slip in, which is the most
/// its API allows.
pub async fn merge(data_dir: &Path, input: Merge) -> BitbucketResult<()> {
    let strategy = strategy_of(&input.method)?;
    let current = row(data_dir, &input.thread.repo, input.thread.number).await?;
    let head = current.source.commit.hash.unwrap_or_default();
    if head.is_empty() || !input.sha.starts_with(&head) {
        return Err(BitbucketError::Http {
            status: 409,
            message: "the branch has moved since you looked; read the new commits first".into(),
        });
    }
    let path = pull_path(&input.thread.repo, input.thread.number, "/merge")?;
    client::post_empty(
        data_dir,
        &path,
        Some(&json!({ "merge_strategy": strategy })),
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPull {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub title: String,
    pub head: String,
    pub base: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub draft: bool,
}

pub async fn create(data_dir: &Path, input: NewPull) -> BitbucketResult<Pull> {
    let body = json!({
        "title": input.title,
        "description": input.body,
        "source": { "branch": { "name": input.head } },
        "destination": { "branch": { "name": input.base } },
        "draft": input.draft,
    });
    let row: PullRow = client::send_json(
        data_dir,
        Method::POST,
        &input.repo.path("/pullrequests")?,
        &body,
    )
    .await?;
    Ok(Pull::from_row(&input.repo, row))
}

#[derive(Deserialize)]
pub struct SetState {
    #[serde(flatten)]
    pub thread: Thread,
    pub state: String,
}

/// Closing a pull request is declining it, which Bitbucket does not undo.
pub async fn set_state(data_dir: &Path, input: SetState) -> BitbucketResult<()> {
    if input.state != "closed" {
        return Err(BitbucketError::Unsupported(
            "reopen a declined pull request",
        ));
    }
    let path = pull_path(&input.thread.repo, input.thread.number, "/decline")?;
    client::post_empty(data_dir, &path, None).await
}

#[derive(Deserialize)]
pub struct ReviewInput {
    #[serde(flatten)]
    pub thread: Thread,
    pub event: String,
    #[serde(default)]
    pub body: String,
}

/// A verdict and a comment are separate things on Bitbucket, so a review
/// with words is the verdict followed by a comment.
pub async fn review(data_dir: &Path, input: ReviewInput) -> BitbucketResult<()> {
    let action = match input.event.as_str() {
        "APPROVE" => Some("/approve"),
        "REQUEST_CHANGES" => Some("/request-changes"),
        "COMMENT" => None,
        _ => {
            return Err(BitbucketError::BadArg(
                "that is not a kind of review".into(),
            ))
        }
    };
    if let Some(action) = action {
        let path = pull_path(&input.thread.repo, input.thread.number, action)?;
        client::post_empty(data_dir, &path, None).await?;
    }
    if !input.body.trim().is_empty() {
        add_comment(
            data_dir,
            NewComment {
                thread: input.thread,
                body: input.body,
            },
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoRef {
        RepoRef {
            owner: "swishx".into(),
            name: "api-docs".into(),
        }
    }

    fn row(value: Value) -> PullRow {
        serde_json::from_value(value).expect("a pull request row")
    }

    #[test]
    fn a_merged_pull_request_reads_as_merged_and_a_declined_one_as_closed() {
        let merged = Pull::from_row(
            &repo(),
            row(json!({
                "id": 4, "title": "t", "state": "MERGED",
                "created_on": "2026-01-01T00:00:00Z", "updated_on": "2026-01-02T00:00:00Z",
                "closed_by": { "nickname": "ada" },
                "merge_commit": { "hash": "abc123" }
            })),
        );
        assert_eq!(merged.state, "merged");
        assert_eq!(merged.merged_at.as_deref(), Some("2026-01-02T00:00:00Z"));
        assert_eq!(merged.merged_by.as_deref(), Some("ada"));
        assert_eq!(merged.merge_commit_sha.as_deref(), Some("abc123"));
        let declined = Pull::from_row(
            &repo(),
            row(
                json!({ "id": 5, "title": "t", "state": "DECLINED", "created_on": "x", "updated_on": "y" }),
            ),
        );
        assert_eq!(declined.state, "closed");
        assert!(declined.merged_at.is_none());
    }

    #[test]
    fn a_branch_from_a_fork_is_labelled_with_the_fork() {
        let pull = Pull::from_row(
            &repo(),
            row(json!({
                "id": 1, "title": "t", "state": "OPEN", "created_on": "x", "updated_on": "y",
                "source": { "branch": { "name": "fix" }, "commit": { "hash": "0123456789ab" }, "repository": { "full_name": "someone/api-docs" } },
                "destination": { "branch": { "name": "main" } },
                "author": { "nickname": "someone", "links": { "avatar": { "href": "https://a/1.png" } } }
            })),
        );
        assert_eq!(pull.head_label.as_deref(), Some("someone:fix"));
        assert_eq!(pull.base.as_deref(), Some("main"));
        assert_eq!(pull.author.as_deref(), Some("someone"));
        assert_eq!(
            pull.url,
            "https://bitbucket.org/swishx/api-docs/pull-requests/1"
        );
    }

    #[test]
    fn reviewers_are_named_once_with_their_pictures() {
        let pull = Pull::from_row(
            &repo(),
            row(json!({
                "id": 1, "title": "t", "state": "OPEN", "created_on": "x", "updated_on": "y",
                "reviewers": [{ "nickname": "bo", "links": { "avatar": { "href": "https://a/bo.png" } } }],
                "participants": [
                    { "user": { "nickname": "bo" }, "role": "REVIEWER", "approved": true },
                    { "user": { "nickname": "cy" }, "role": "PARTICIPANT" }
                ]
            })),
        );
        assert_eq!(pull.reviewers, vec!["bo".to_string()]);
        assert_eq!(
            pull.avatars.get("bo").map(String::as_str),
            Some("https://a/bo.png")
        );
    }

    fn changed(path: &str, patch: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            status: "modified".into(),
            additions: 1,
            deletions: 1,
            previous_path: None,
            patch: patch.map(Into::into),
            note: None,
        }
    }

    fn asking(paths: &[&str]) -> PatchQuery {
        PatchQuery {
            paths: paths.iter().map(|path| path.to_string()).collect(),
            ..PatchQuery::default()
        }
    }

    #[test]
    fn lists_the_files_without_their_patches_by_default() {
        let mut renamed = changed("new.rs", Some("@@ -1 +1 @@\n-a\n+b"));
        renamed.status = "renamed".into();
        renamed.previous_path = Some("old.rs".into());
        let listed = pick(
            vec![renamed, changed("b.rs", Some("@@"))],
            &PatchQuery::default(),
        )
        .unwrap();
        assert!(listed
            .iter()
            .all(|file| file.patch.is_none() && file.note.is_none()));
        assert_eq!(
            serde_json::to_value(&listed[0]).unwrap(),
            json!({ "path": "new.rs", "status": "renamed", "additions": 1, "deletions": 1, "previousPath": "old.rs" })
        );
        assert_eq!(
            serde_json::to_value(&listed[1]).unwrap(),
            json!({ "path": "b.rs", "status": "modified", "additions": 1, "deletions": 1 })
        );
    }

    #[test]
    fn named_paths_come_back_alone_with_their_patches() {
        let files = vec![
            changed("a.rs", Some("@@ -1 +1 @@\n-a\n+b")),
            changed("b.rs", Some("@@")),
            changed("logo.png", None),
        ];
        let picked = pick(files.clone(), &asking(&["a.rs", "logo.png"])).unwrap();
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0].patch.as_deref(), Some("@@ -1 +1 @@\n-a\n+b"));
        assert_eq!(picked[0].note, None);
        assert!(picked[1].patch.is_none());
        assert!(picked[1].note.as_deref().unwrap().contains("git diff"));

        assert!(matches!(
            pick(files.clone(), &asking(&["missing.rs"])),
            Err(BitbucketError::BadArg(_))
        ));
        let full = PatchQuery {
            full_patches: true,
            ..PatchQuery::default()
        };
        assert_eq!(pick(files.clone(), &full).unwrap(), files);
    }

    #[test]
    fn a_diffstat_row_names_the_file_and_any_old_path() {
        let stat: DiffStat = serde_json::from_value(json!({
            "status": "renamed", "lines_added": 2, "lines_removed": 1,
            "old": { "path": "old.rs" }, "new": { "path": "new.rs" },
        }))
        .unwrap();
        let file = changed_file(stat).unwrap();
        assert_eq!(
            (file.path.as_str(), file.status.as_str()),
            ("new.rs", "renamed")
        );
        assert_eq!(file.previous_path.as_deref(), Some("old.rs"));
        let gone: DiffStat =
            serde_json::from_value(json!({ "status": "removed", "old": { "path": "gone.rs" } }))
                .unwrap();
        let gone = changed_file(gone).unwrap();
        assert_eq!((gone.path.as_str(), gone.previous_path), ("gone.rs", None));
    }

    #[test]
    fn reads_the_patch_arguments_beside_the_pull_request() {
        let query: FilesQuery = serde_json::from_value(json!({
            "owner": "a", "name": "b", "number": 3, "paths": ["x.rs"], "offset": 400, "lines": 50,
        }))
        .unwrap();
        assert_eq!(query.pull.number, 3);
        assert_eq!(query.patches.paths, ["x.rs"]);
        assert_eq!((query.patches.offset, query.patches.lines), (400, Some(50)));
        assert!(!query.patches.full_patches);
        let view: FilesQuery = serde_json::from_value(
            json!({ "owner": "a", "name": "b", "number": 3, "fullPatches": true }),
        )
        .unwrap();
        assert!(view.patches.full_patches);
    }

    #[test]
    fn a_rename_is_found_by_its_old_path_too() {
        let mut renamed = changed("new.rs", Some("@@"));
        renamed.previous_path = Some("old.rs".into());
        let picked = pick(vec![renamed, changed("b.rs", None)], &asking(&["old.rs"])).unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].path, "new.rs");
    }

    #[test]
    fn a_long_patch_is_cut_and_says_how_to_read_the_rest() {
        let patch: Vec<String> = (1..=1000).map(|line| format!("+line {line}")).collect();
        let patch = patch.join("\n");
        let (shown, note) = window(&patch, 0, None);
        assert_eq!(shown.lines().count(), PATCH_LINES);
        assert_eq!(shown.lines().last(), Some("+line 400"));
        assert_eq!(
            note.as_deref(),
            Some("Lines 1-400 of 1000. 600 more lines left out: pass offset 400 for the next part, or run git diff locally.")
        );

        let (shown, note) = window(&patch, 990, None);
        assert_eq!(shown.lines().next(), Some("+line 991"));
        assert_eq!(note.as_deref(), Some("Lines 991-1000 of 1000."));

        let (shown, _) = window(&patch, 0, Some(5));
        assert_eq!(shown.lines().count(), 5);

        let (shown, note) = window(&patch, 5000, None);
        assert!(shown.is_empty());
        assert!(note.unwrap().contains("past its end"));
    }

    #[test]
    fn a_patch_stops_at_its_byte_budget() {
        let wide = format!("+{}", "é".repeat(PATCH_BYTES));
        let patch = format!("@@ -1 +1 @@\n{wide}\n+after");
        let (shown, note) = window(&patch, 0, None);
        assert_eq!(shown, "@@ -1 +1 @@");
        assert!(note.unwrap().contains("pass offset 1"));

        let (shown, note) = window(&patch, 1, None);
        assert!(shown.len() <= PATCH_BYTES);
        assert!(shown.starts_with("+é"));
        assert!(note.unwrap().contains("Line 2 was cut at 32 KB"));
    }

    #[test]
    fn splits_one_diff_into_each_files_hunks() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\ndiff --git a/logo.png b/logo.png\nBinary files differ\n";
        let patches = patches_of(diff);
        assert_eq!(
            patches.get("src/a.rs").map(String::as_str),
            Some("@@ -1 +1 @@\n-old\n+new")
        );
        assert_eq!(
            patches.get("gone.txt").map(String::as_str),
            Some("@@ -1 +0,0 @@\n-bye")
        );
        assert!(!patches.contains_key("logo.png"));
    }

    #[test]
    fn a_commit_without_an_account_is_named_from_its_author_line() {
        let commit = PullCommit::from(CommitRow {
            hash: "abc".into(),
            message: Some("Fix it\n".into()),
            date: None,
            author: Some(CommitAuthor {
                raw: Some("Ada Lovelace <ada@example.com>".into()),
                user: None,
            }),
        });
        assert_eq!(commit.author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(commit.message, "Fix it");
    }

    #[test]
    fn commit_emails_are_matched_to_the_accounts_bitbucket_knows() {
        let rows: Vec<CommitRow> = serde_json::from_value(json!([
            { "hash": "a", "author": { "raw": "Kishore Gunalan <Kishore@Example.com>",
                "user": { "nickname": "kishore", "links": { "avatar": { "href": "https://a/kg.png" } } } } },
            { "hash": "b", "author": { "raw": "Kishore Gunalan <kishore@example.com>",
                "user": { "nickname": "kishore", "links": { "avatar": { "href": "https://a/kg.png" } } } } },
            { "hash": "c", "author": { "raw": "Nobody Known <nobody@example.com>" } },
            { "hash": "d", "author": { "raw": "no email here", "user": { "nickname": "x" } } }
        ]))
        .expect("commit rows");
        let known = known_authors(rows);
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].email, "kishore@example.com");
        assert_eq!(known[0].login, "kishore");
        assert_eq!(known[0].avatar_url, "https://a/kg.png");
    }

    #[test]
    fn reviewer_verdicts_use_the_words_every_host_shares() {
        assert_eq!(verdict(true, None), Some("APPROVED"));
        assert_eq!(
            verdict(false, Some("changes_requested")),
            Some("CHANGES_REQUESTED")
        );
        assert_eq!(verdict(false, None), None);
    }

    #[test]
    fn activity_becomes_timeline_events() {
        let activity =
            |value: Value| -> Activity { serde_json::from_value(value).expect("activity") };
        let comment = timeline_of(
            activity(json!({ "comment": { "id": 9, "content": { "raw": "hi" }, "user": { "nickname": "ada" }, "created_on": "t1" } })),
            None,
        )
        .expect("an item");
        assert_eq!(
            (comment.kind, comment.body.as_deref()),
            ("commented", Some("hi"))
        );
        let approval = timeline_of(
            activity(json!({ "approval": { "date": "t2", "user": { "nickname": "bo" } } })),
            None,
        )
        .expect("an item");
        assert_eq!(
            (approval.kind, approval.state.as_deref()),
            ("reviewed", Some("approved"))
        );
        let merged = timeline_of(
            activity(json!({ "update": { "state": "MERGED", "date": "t3", "author": { "nickname": "cy" } } })),
            Some("f00d"),
        )
        .expect("an item");
        assert_eq!(
            (merged.kind, merged.sha.as_deref()),
            ("merged", Some("f00d"))
        );
        let pushed = timeline_of(
            activity(json!({ "update": { "state": "OPEN", "date": "t4" } })),
            None,
        );
        assert!(pushed.is_none());
    }

    #[test]
    fn only_merge_commits_and_squashes_are_offered() {
        assert_eq!(strategy_of("merge").ok(), Some("merge_commit"));
        assert_eq!(strategy_of("squash").ok(), Some("squash"));
        assert!(strategy_of("rebase").is_err());
    }
}
