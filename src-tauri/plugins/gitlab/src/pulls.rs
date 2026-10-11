// Merge requests, in the words the Git pane uses for every host's pull
// requests: their changes, commits, approvals and history, and merging,
// opening, closing and approving one.

use std::collections::BTreeMap;
use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client;
use crate::error::{GitlabError, GitlabResult};
use crate::notes::{Thread, ThreadOf};
use crate::repo::{avatar_of, login_of, RepoRef, User};

#[derive(Deserialize)]
#[serde(untagged)]
pub enum LabelRow {
    Named(String),
    Detailed { name: String, color: Option<String> },
}

#[derive(Deserialize)]
struct Milestone {
    title: Option<String>,
}

#[derive(Deserialize)]
pub struct MergeRequestRow {
    iid: u64,
    #[serde(default)]
    title: String,
    description: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    draft: bool,
    author: Option<User>,
    source_branch: Option<String>,
    target_branch: Option<String>,
    sha: Option<String>,
    created_at: String,
    updated_at: Option<String>,
    user_notes_count: Option<u64>,
    changes_count: Option<String>,
    detailed_merge_status: Option<String>,
    #[serde(default)]
    has_conflicts: bool,
    #[serde(default)]
    labels: Vec<LabelRow>,
    #[serde(default)]
    reviewers: Vec<User>,
    #[serde(default)]
    assignees: Vec<User>,
    milestone: Option<Milestone>,
    merged_at: Option<String>,
    merge_user: Option<User>,
    merged_by: Option<User>,
    merge_commit_sha: Option<String>,
    squash_commit_sha: Option<String>,
    source_project_id: Option<u64>,
    target_project_id: Option<u64>,
    web_url: String,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Label {
    pub name: String,
    pub color: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Pull {
    pub number: u64,
    pub title: String,
    pub body: String,
    /// `open`, `merged`, or `closed`.
    pub state: &'static str,
    pub draft: bool,
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub author_association: Option<String>,
    pub head: Option<String>,
    /// `group:branch` when the branch lives in a fork.
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
    pub labels: Vec<Label>,
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
        "opened" | "locked" => "open",
        "merged" => "merged",
        _ => "closed",
    }
}

/// GitLab's reasons a merge request can or cannot merge, in GitHub's merge-state words.
pub fn merge_state_of(
    detailed: Option<&str>,
    has_conflicts: bool,
) -> (Option<bool>, Option<String>) {
    if has_conflicts {
        return (Some(false), Some("dirty".into()));
    }
    let state = match detailed {
        Some("mergeable") => "clean",
        Some("conflict" | "broken_status") => "dirty",
        Some("need_rebase") => "behind",
        Some("draft_status") => "draft",
        Some("ci_still_running" | "ci_must_pass") => "unstable",
        Some("checking" | "unchecked" | "preparing") | None => return (None, None),
        Some(_) => "blocked",
    };
    (Some(state == "clean"), Some(state.into()))
}

pub fn label_of(row: LabelRow) -> Label {
    match row {
        LabelRow::Named(name) => Label {
            name,
            color: String::new(),
        },
        LabelRow::Detailed { name, color } => Label {
            name,
            color: color
                .unwrap_or_default()
                .trim_start_matches('#')
                .to_string(),
        },
    }
}

impl Pull {
    pub fn from_row(row: MergeRequestRow) -> Self {
        let mut avatars = BTreeMap::new();
        let mut people = |users: &[User]| -> Vec<String> {
            users
                .iter()
                .filter_map(|user| {
                    let login = login_of(Some(user))?;
                    if let Some(avatar) = avatar_of(Some(user)) {
                        avatars.insert(login.clone(), avatar);
                    }
                    Some(login)
                })
                .collect()
        };
        let reviewers = people(&row.reviewers);
        let assignees = people(&row.assignees);
        let fork = match (row.source_project_id, row.target_project_id) {
            (Some(source), Some(target)) if source != target => Some(source),
            _ => None,
        };
        let (mergeable, merge_state) =
            merge_state_of(row.detailed_merge_status.as_deref(), row.has_conflicts);
        let merger = row.merge_user.as_ref().or(row.merged_by.as_ref());
        Pull {
            number: row.iid,
            title: row.title,
            body: row.description.unwrap_or_default(),
            state: state_of(&row.state),
            draft: row.draft,
            author: login_of(row.author.as_ref()),
            avatar_url: avatar_of(row.author.as_ref()),
            author_association: None,
            head_label: match (fork, &row.source_branch) {
                (Some(source), Some(branch)) => Some(format!("project {source}:{branch}")),
                _ => None,
            },
            head: row.source_branch,
            base: row.target_branch,
            head_sha: row.sha,
            updated_at: row.updated_at.unwrap_or_else(|| row.created_at.clone()),
            created_at: row.created_at,
            comments: row.user_notes_count,
            additions: None,
            deletions: None,
            changed_files: row
                .changes_count
                .and_then(|count| count.trim_end_matches('+').parse().ok()),
            mergeable,
            merge_state,
            labels: row.labels.into_iter().map(label_of).collect(),
            reviewers,
            assignees,
            milestone: row.milestone.and_then(|milestone| milestone.title),
            commits: None,
            merged_at: row.merged_at,
            merged_by: login_of(merger),
            merge_commit_sha: row.merge_commit_sha.or(row.squash_commit_sha),
            avatars,
            url: row.web_url,
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

pub async fn list(data_dir: &Path, input: ListQuery) -> GitlabResult<Vec<Pull>> {
    let state = match input.state.as_str() {
        "open" => "opened",
        "closed" => "closed",
        "merged" => "merged",
        _ => "all",
    };
    let rows: Vec<MergeRequestRow> = client::get_all(
        data_dir,
        &input.repo.path("/merge_requests")?,
        &[
            ("state", state.into()),
            ("order_by", "updated_at".into()),
            ("with_labels_details", "true".into()),
        ],
        2,
    )
    .await?;
    Ok(rows.into_iter().map(Pull::from_row).collect())
}

fn pull_thread(repo: RepoRef, number: u64) -> Thread {
    Thread {
        repo,
        number,
        of: ThreadOf::Pull,
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PullRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
}

impl PullRef {
    fn path(&self, rest: &str) -> GitlabResult<String> {
        pull_thread(self.repo.clone(), self.number).path(rest)
    }
}

pub async fn get(data_dir: &Path, input: PullRef) -> GitlabResult<Pull> {
    let row: MergeRequestRow = client::get(
        data_dir,
        &input.path("")?,
        &[("with_labels_details", "true".into())],
    )
    .await?;
    Ok(Pull::from_row(row))
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

#[derive(Deserialize)]
pub struct DiffRow {
    old_path: String,
    new_path: String,
    #[serde(default)]
    new_file: bool,
    #[serde(default)]
    renamed_file: bool,
    #[serde(default)]
    deleted_file: bool,
    #[serde(default)]
    diff: String,
}

pub fn changed_file(row: DiffRow) -> ChangedFile {
    let (additions, deletions) = row.diff.lines().fold((0, 0), |(added, removed), line| {
        if line.starts_with('+') && !line.starts_with("+++") {
            (added + 1, removed)
        } else if line.starts_with('-') && !line.starts_with("---") {
            (added, removed + 1)
        } else {
            (added, removed)
        }
    });
    let status = if row.new_file {
        "added"
    } else if row.deleted_file {
        "removed"
    } else if row.renamed_file {
        "renamed"
    } else {
        "modified"
    };
    let path = if row.deleted_file {
        row.old_path.clone()
    } else {
        row.new_path.clone()
    };
    ChangedFile {
        previous_path: (row.renamed_file && row.old_path != row.new_path).then_some(row.old_path),
        path,
        status: status.into(),
        additions,
        deletions,
        patch: (!row.diff.is_empty()).then_some(row.diff),
        note: None,
    }
}

const PATCH_LINES: usize = 400;
const MAX_PATCH_LINES: usize = 1000;
const PATCH_BYTES: usize = 32 * 1024;

/// Agents get the file list alone unless they name `paths`, and then a window
/// of each patch. The merge request view asks for `fullPatches`.
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
    pub pull: PullRef,
    #[serde(flatten)]
    pub patches: PatchQuery,
}

fn cut_at_char(text: &str, most: usize) -> &str {
    text.get(..text.floor_char_boundary(most))
        .unwrap_or_default()
}

/// Up to `lines` lines of `patch` from `offset`, and at most `PATCH_BYTES` of
/// them, with a note whenever any of the patch is left out.
pub fn window(patch: &str, offset: usize, lines: Option<usize>) -> (String, Option<String>) {
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
        note.push_str(&format!(" {} more lines left out: pass offset {end} for the next part, or run git diff locally.", total - end));
    }
    (shown, Some(note))
}

fn is_named(file: &ChangedFile, path: &str) -> bool {
    file.path == path || file.previous_path.as_deref() == Some(path)
}

/// The files to answer with, carrying the patches `query` asks for.
pub fn pick(files: Vec<ChangedFile>, query: &PatchQuery) -> GitlabResult<Vec<ChangedFile>> {
    if query.full_patches {
        return Ok(files);
    }
    if let Some(unknown) = query
        .paths
        .iter()
        .find(|path| !files.iter().any(|file| is_named(file, path)))
    {
        return Err(GitlabError::BadArg(format!(
            "`{unknown}` is not among the files this merge request changes"
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
                    file.note = Some("GitLab shows no patch for this file: it is binary, too large, or only renamed. Run git diff locally.".into());
                }
            }
            Some(file)
        })
        .collect())
}

pub async fn files(data_dir: &Path, input: FilesQuery) -> GitlabResult<Vec<ChangedFile>> {
    let rows: Vec<DiffRow> =
        client::get_all(data_dir, &input.pull.path("/diffs")?, &[], 10).await?;
    pick(rows.into_iter().map(changed_file).collect(), &input.patches)
}

#[derive(Deserialize)]
struct CommitRow {
    id: String,
    message: Option<String>,
    title: Option<String>,
    author_name: Option<String>,
    authored_date: Option<String>,
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

pub async fn commits(data_dir: &Path, input: PullRef) -> GitlabResult<Vec<PullCommit>> {
    let rows: Vec<CommitRow> = client::get_all(data_dir, &input.path("/commits")?, &[], 5).await?;
    let mut commits: Vec<PullCommit> = rows
        .into_iter()
        .map(|row| PullCommit {
            sha: row.id,
            message: row.message.or(row.title).unwrap_or_default(),
            author: row.author_name,
            avatar_url: None,
            date: row.authored_date,
        })
        .collect();
    commits.reverse();
    Ok(commits)
}

#[derive(Deserialize)]
struct Approver {
    user: Option<User>,
}

#[derive(Deserialize)]
struct Approvals {
    #[serde(default)]
    approved_by: Vec<Approver>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

pub async fn reviews(data_dir: &Path, input: PullRef) -> GitlabResult<Vec<Review>> {
    let approvals: Approvals = client::get(data_dir, &input.path("/approvals")?, &[]).await?;
    Ok(approvals
        .approved_by
        .into_iter()
        .map(|approver| Review {
            author: login_of(approver.user.as_ref()),
            avatar_url: avatar_of(approver.user.as_ref()),
            state: "APPROVED".into(),
            body: String::new(),
            submitted_at: None,
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Merge {
    #[serde(flatten)]
    pub pull: PullRef,
    pub method: String,
    pub sha: Option<String>,
}

/// `sha` is the head the person saw: GitLab refuses the merge if the branch has moved since.
pub fn merge_body(method: &str, sha: Option<&str>) -> GitlabResult<Value> {
    let squash = match method {
        "squash" => true,
        "merge" => false,
        other => {
            return Err(GitlabError::BadArg(format!(
                "GitLab cannot merge by {other}"
            )))
        }
    };
    let mut body = json!({ "squash": squash });
    if let (Some(sha), Some(fields)) = (sha.filter(|sha| !sha.is_empty()), body.as_object_mut()) {
        fields.insert("sha".into(), json!(sha));
    }
    Ok(body)
}

pub async fn merge(data_dir: &Path, input: Merge) -> GitlabResult<()> {
    let body = merge_body(&input.method, input.sha.as_deref())?;
    client::write(
        data_dir,
        Method::PUT,
        &input.pull.path("/merge")?,
        Some(&body),
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

/// GitLab marks a draft by its title.
pub fn titled(title: &str, draft: bool) -> String {
    let title = title.trim();
    if draft && !title.to_ascii_lowercase().starts_with("draft:") {
        format!("Draft: {title}")
    } else {
        title.to_string()
    }
}

pub async fn create(data_dir: &Path, input: NewPull) -> GitlabResult<Pull> {
    if input.title.trim().is_empty() {
        return Err(GitlabError::BadArg("a merge request needs a title".into()));
    }
    let body = json!({
        "source_branch": input.head.trim(),
        "target_branch": input.base.trim(),
        "title": titled(&input.title, input.draft),
        "description": input.body,
    });
    let row: MergeRequestRow = client::send_json(
        data_dir,
        Method::POST,
        &input.repo.path("/merge_requests")?,
        &body,
    )
    .await?;
    Ok(Pull::from_row(row))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetState {
    #[serde(flatten)]
    pub pull: PullRef,
    pub state: String,
}

pub async fn set_state(data_dir: &Path, input: SetState) -> GitlabResult<()> {
    let event = if input.state == "open" {
        "reopen"
    } else {
        "close"
    };
    client::write(
        data_dir,
        Method::PUT,
        &input.pull.path("")?,
        Some(&json!({ "state_event": event })),
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewInput {
    #[serde(flatten)]
    pub pull: PullRef,
    pub event: String,
    #[serde(default)]
    pub body: String,
}

/// Approving is GitLab's approval; a written verdict goes on as a comment
/// beside it, once the verdict itself has gone through.
pub async fn review(data_dir: &Path, input: ReviewInput) -> GitlabResult<()> {
    let body = input.body.trim();
    match input.event.as_str() {
        "APPROVE" => {
            client::write(data_dir, Method::POST, &input.pull.path("/approve")?, None).await?
        }
        "COMMENT" if !body.is_empty() => {}
        "COMMENT" => return Err(GitlabError::BadArg("a comment needs some text".into())),
        _ => {
            return Err(GitlabError::Unsupported(
                "ask for changes on a merge request; leave a comment instead",
            ))
        }
    }
    if body.is_empty() {
        return Ok(());
    }
    client::write(
        data_dir,
        Method::POST,
        &input.pull.path("/notes")?,
        Some(&json!({ "body": body })),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(value: Value) -> MergeRequestRow {
        serde_json::from_value(value).expect("merge request parses")
    }

    #[test]
    fn a_merge_request_reads_as_a_pull_request() {
        let pull = Pull::from_row(row(json!({
            "iid": 42, "title": "Fix the VAT rounding", "description": "Rounds per line.", "state": "opened", "draft": false,
            "author": { "username": "ankit", "avatar_url": "https://a/a.png" },
            "source_branch": "fix/vat", "target_branch": "main", "sha": "9c41e2a0",
            "created_at": "t1", "updated_at": "t2", "user_notes_count": 3, "changes_count": "12",
            "detailed_merge_status": "mergeable", "has_conflicts": false,
            "labels": [ { "name": "backend", "color": "#428BCA" } ],
            "reviewers": [ { "username": "irwan", "avatar_url": "https://a/i.png" } ],
            "assignees": [ { "username": "ankit" } ],
            "milestone": { "title": "Q4" },
            "source_project_id": 7, "target_project_id": 7,
            "web_url": "https://gitlab.com/acme/api/-/merge_requests/42"
        })));
        assert_eq!(
            (
                pull.number,
                pull.state,
                pull.head.as_deref(),
                pull.base.as_deref()
            ),
            (42, "open", Some("fix/vat"), Some("main"))
        );
        assert_eq!(
            (pull.mergeable, pull.merge_state.as_deref()),
            (Some(true), Some("clean"))
        );
        assert_eq!(pull.changed_files, Some(12));
        assert_eq!(
            pull.labels,
            [Label {
                name: "backend".into(),
                color: "428BCA".into()
            }]
        );
        assert_eq!(pull.reviewers, ["irwan"]);
        assert_eq!(
            pull.avatars.get("irwan").map(String::as_str),
            Some("https://a/i.png")
        );
        assert_eq!(pull.head_label, None);
    }

    #[test]
    fn a_merged_one_from_a_fork_says_who_merged_it_and_where_its_branch_lives() {
        let pull = Pull::from_row(row(json!({
            "iid": 7, "title": "x", "state": "merged", "created_at": "t1", "source_branch": "patch-1",
            "source_project_id": 8, "target_project_id": 7, "references": { "full": "acme/api!7" },
            "merge_user": { "username": "irwan" }, "squash_commit_sha": "abc", "changes_count": "1000+",
            "labels": ["plain"], "web_url": "u"
        })));
        assert_eq!(pull.state, "merged");
        assert_eq!(pull.merged_by.as_deref(), Some("irwan"));
        assert_eq!(pull.merge_commit_sha.as_deref(), Some("abc"));
        assert_eq!(pull.head_label.as_deref(), Some("project 8:patch-1"));
        assert_eq!(pull.changed_files, Some(1000));
        assert_eq!(pull.labels[0].color, "");
    }

    #[test]
    fn why_a_merge_request_cannot_merge_reads_in_githubs_words() {
        assert_eq!(
            merge_state_of(Some("need_rebase"), false),
            (Some(false), Some("behind".into()))
        );
        assert_eq!(
            merge_state_of(Some("not_approved"), false),
            (Some(false), Some("blocked".into()))
        );
        assert_eq!(
            merge_state_of(Some("mergeable"), true),
            (Some(false), Some("dirty".into()))
        );
        assert_eq!(merge_state_of(Some("checking"), false), (None, None));
    }

    #[test]
    fn a_change_counts_its_lines_and_says_what_happened_to_the_file() {
        let file = changed_file(
            serde_json::from_value(json!({
                "old_path": "src/vat.ts", "new_path": "src/tax/vat.ts", "renamed_file": true,
                "diff": "@@ -1,3 +1,3 @@\n-const a = 1;\n+const a = 2;\n+const b = 3;\n context"
            }))
            .expect("diff parses"),
        );
        assert_eq!(
            (file.path.as_str(), file.status.as_str()),
            ("src/tax/vat.ts", "renamed")
        );
        assert_eq!(file.previous_path.as_deref(), Some("src/vat.ts"));
        assert_eq!((file.additions, file.deletions), (2, 1));
        let gone = changed_file(serde_json::from_value(json!({ "old_path": "old.md", "new_path": "old.md", "deleted_file": true, "diff": "" })).expect("parses"));
        assert_eq!(
            (gone.path.as_str(), gone.status.as_str(), gone.patch),
            ("old.md", "removed", None)
        );
    }

    #[test]
    fn an_agent_gets_a_window_of_a_patch_and_is_told_the_rest() -> GitlabResult<()> {
        let patch = (1..=10)
            .map(|n| format!("+line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let file = ChangedFile {
            path: "a.ts".into(),
            status: "modified".into(),
            additions: 10,
            deletions: 0,
            previous_path: None,
            patch: Some(patch),
            note: None,
        };
        let listed = pick(vec![file.clone()], &PatchQuery::default())?;
        assert_eq!(listed[0].patch, None);
        let windowed = pick(
            vec![file.clone()],
            &PatchQuery {
                paths: vec!["a.ts".into()],
                offset: 2,
                lines: Some(3),
                full_patches: false,
            },
        )?;
        assert_eq!(
            windowed[0].patch.as_deref(),
            Some("+line 3\n+line 4\n+line 5")
        );
        assert!(windowed[0]
            .note
            .as_deref()
            .is_some_and(|note| note.contains("pass offset 5")));
        assert!(pick(
            vec![file],
            &PatchQuery {
                paths: vec!["b.ts".into()],
                ..PatchQuery::default()
            }
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn a_draft_is_marked_in_its_title_once() {
        assert_eq!(titled(" Fix VAT ", true), "Draft: Fix VAT");
        assert_eq!(titled("Draft: Fix VAT", true), "Draft: Fix VAT");
        assert_eq!(titled("Fix VAT", false), "Fix VAT");
    }
}
