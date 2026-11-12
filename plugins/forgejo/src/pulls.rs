// Pull requests, in the words used for every host's: their changes, commits and
// reviews, and merging, opening, closing and reviewing one.

use std::collections::BTreeMap;
use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_plugin_api::codehost::answers;
use sikemux_plugin_api::codehost::asks::{
    CommitRef, FilesQuery, Merge, NewPull, NewReview, PullEdit, PullQuery, PullRef, SetPullState,
};
use sikemux_plugin_api::codehost::{ChangedFile, Label, Pull, PullCommit, PullStage, Review};
use sikemux_plugin_api::http::Read;

use crate::client;
use crate::error::{ForgejoError, ForgejoResult};
use crate::repo::{avatar_of, login_of, ApiPath, User};

#[derive(Deserialize)]
pub struct LabelRow {
    pub name: String,
    color: Option<String>,
}

pub fn label_of(row: LabelRow) -> Label {
    Label {
        name: row.name,
        color: row
            .color
            .unwrap_or_default()
            .trim_start_matches('#')
            .to_string(),
    }
}

#[derive(Deserialize)]
pub struct Milestone {
    title: Option<String>,
}

#[derive(Deserialize)]
struct RepoName {
    full_name: Option<String>,
}

#[derive(Deserialize)]
struct BranchInfo {
    #[serde(rename = "ref")]
    name: Option<String>,
    sha: Option<String>,
    repo_id: Option<u64>,
    repo: Option<RepoName>,
}

#[derive(Deserialize)]
pub struct PullRow {
    number: u64,
    #[serde(default)]
    title: String,
    body: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    merged: bool,
    user: Option<User>,
    head: Option<BranchInfo>,
    base: Option<BranchInfo>,
    created_at: String,
    updated_at: Option<String>,
    comments: Option<u64>,
    additions: Option<u64>,
    deletions: Option<u64>,
    changed_files: Option<u64>,
    mergeable: Option<bool>,
    labels: Option<Vec<LabelRow>>,
    requested_reviewers: Option<Vec<User>>,
    assignees: Option<Vec<User>>,
    milestone: Option<Milestone>,
    merged_at: Option<String>,
    merged_by: Option<User>,
    merge_commit_sha: Option<String>,
    html_url: String,
}

/// Whether an open pull request can merge, in GitHub's merge-state words. The
/// server only says whether it conflicts.
fn merge_state_of(open: bool, mergeable: Option<bool>) -> (Option<bool>, Option<String>) {
    match (open, mergeable) {
        (true, Some(true)) => (Some(true), Some("clean".into())),
        (true, Some(false)) => (Some(false), Some("dirty".into())),
        _ => (None, None),
    }
}

/// `owner:branch` when the branch lives on a fork.
fn fork_label(head: &BranchInfo, base: Option<&BranchInfo>) -> Option<String> {
    let forked = match (head.repo_id, base.and_then(|base| base.repo_id)) {
        (Some(head), Some(base)) => head != base,
        _ => false,
    };
    if !forked {
        return None;
    }
    let owner = head
        .repo
        .as_ref()
        .and_then(|repo| repo.full_name.as_deref())
        .and_then(|name| name.split_once('/'))
        .map(|(owner, _)| owner)?;
    Some(format!("{owner}:{}", head.name.as_deref()?))
}

pub fn pull_of(row: PullRow) -> Pull {
    let mut avatars = BTreeMap::new();
    let mut people = |users: Option<Vec<User>>| -> Vec<String> {
        users
            .unwrap_or_default()
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
    let reviewers = people(row.requested_reviewers);
    let assignees = people(row.assignees);
    let state = if row.merged || row.merged_at.is_some() {
        PullStage::Merged
    } else if row.state == "open" {
        PullStage::Open
    } else {
        PullStage::Closed
    };
    let (mergeable, merge_state) = merge_state_of(state == PullStage::Open, row.mergeable);
    let head_label = row
        .head
        .as_ref()
        .and_then(|head| fork_label(head, row.base.as_ref()));
    let (head, head_sha) = row
        .head
        .map(|head| (head.name, head.sha))
        .unwrap_or_default();
    Pull {
        number: row.number,
        title: row.title,
        body: row.body.unwrap_or_default(),
        state,
        draft: row.draft,
        author: login_of(row.user.as_ref()),
        avatar_url: avatar_of(row.user.as_ref()),
        author_association: None,
        head,
        head_label,
        base: row.base.and_then(|base| base.name),
        head_sha,
        updated_at: row.updated_at.unwrap_or_else(|| row.created_at.clone()),
        created_at: row.created_at,
        comments: row.comments,
        additions: row.additions,
        deletions: row.deletions,
        changed_files: row.changed_files,
        mergeable,
        merge_state,
        labels: row
            .labels
            .unwrap_or_default()
            .into_iter()
            .map(label_of)
            .collect(),
        reviewers,
        assignees,
        milestone: row.milestone.and_then(|milestone| milestone.title),
        commits: None,
        merged_at: row.merged_at,
        merged_by: login_of(row.merged_by.as_ref()),
        merge_commit_sha: row.merge_commit_sha,
        avatars,
        url: row.html_url,
    }
}

pub async fn list(data_dir: &Path, input: PullQuery) -> ForgejoResult<Vec<Pull>> {
    let wanted = input.state.as_deref().unwrap_or("open");
    let state = match wanted {
        "open" => "open",
        "closed" | "merged" => "closed",
        _ => "all",
    };
    let rows: Vec<PullRow> = client::get_all(
        data_dir,
        &input.repo.path("/pulls")?,
        &[("state", state.into()), ("sort", "recentupdate".into())],
        2,
    )
    .await?;
    Ok(rows
        .into_iter()
        .map(pull_of)
        .filter(|pull| match wanted {
            "merged" => pull.state == PullStage::Merged,
            "closed" => pull.state == PullStage::Closed,
            _ => true,
        })
        .collect())
}

pub async fn get(data_dir: &Path, input: PullRef) -> ForgejoResult<Pull> {
    let row: PullRow = client::API.get(data_dir, &input.path("")?, &[]).await?;
    Ok(pull_of(row))
}

/// The pull request that merged the commit `sha`; the server names only that one.
pub async fn of_commit(data_dir: &Path, input: CommitRef) -> ForgejoResult<Vec<Pull>> {
    let path = input.repo.path(&format!(
        "/commits/{}/pull",
        answers::commit_hash(&input.sha)?
    ))?;
    let answer = client::API
        .send(data_dir, Method::GET, &path, &[], None, Read::ALL)
        .await?;
    if answer.status.as_u16() == 404 {
        return Ok(Vec::new());
    }
    let row: PullRow = answer.succeeded()?.parsed()?;
    Ok(vec![pull_of(row)])
}

#[derive(Deserialize)]
struct FileRow {
    filename: String,
    previous_filename: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
}

fn changed_file(row: FileRow) -> ChangedFile {
    let status = match row.status.as_str() {
        "deleted" => "removed",
        "" => "modified",
        other => other,
    };
    ChangedFile {
        previous_path: row
            .previous_filename
            .filter(|previous| !previous.is_empty() && *previous != row.filename),
        path: row.filename,
        status: status.into(),
        additions: row.additions,
        deletions: row.deletions,
        patch: None,
        note: None,
    }
}

/// Each file's patch in a whole pull request's diff, by the path it ends up at,
/// or the one it had when it is gone. A binary file has none.
pub fn patches(diff: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let diff = diff.strip_prefix("diff --git ").unwrap_or(diff);
    for chunk in diff
        .split("\ndiff --git ")
        .filter(|chunk| !chunk.is_empty())
    {
        let mut lines = chunk.lines();
        let header = lines.next().unwrap_or_default();
        let mut path = header.rsplit_once(" b/").map(|(_, path)| path.to_string());
        let mut hunks: Vec<&str> = Vec::new();
        for line in lines {
            if !hunks.is_empty() || line.starts_with("@@") {
                hunks.push(line);
            } else if let Some(new) = line.strip_prefix("+++ b/") {
                path = Some(new.to_string());
            } else if let Some(old) = line.strip_prefix("--- a/") {
                path.get_or_insert_with(|| old.to_string());
            }
        }
        if let (Some(path), false) = (path, hunks.is_empty()) {
            found.push((path, hunks.join("\n")));
        }
    }
    found
}

pub async fn files(data_dir: &Path, input: FilesQuery) -> ForgejoResult<Vec<ChangedFile>> {
    let rows: Vec<FileRow> =
        client::get_all(data_dir, &input.pull.path("/files")?, &[], 10).await?;
    let mut files: Vec<ChangedFile> = rows.into_iter().map(changed_file).collect();
    if input.patches.full_patches || !input.patches.paths.is_empty() {
        let diff = client::API
            .get_text(data_dir, &input.pull.path(".diff")?, &[])
            .await?;
        let mut by_path: BTreeMap<String, String> = patches(&diff).into_iter().collect();
        for file in &mut files {
            file.patch = by_path.remove(&file.path).or_else(|| {
                file.previous_path
                    .as_ref()
                    .and_then(|previous| by_path.remove(previous))
            });
        }
    }
    answers::pick_patches(files, &input.patches, "pull request")
}

#[derive(Deserialize)]
struct CommitPerson {
    name: Option<String>,
    date: Option<String>,
}

#[derive(Deserialize)]
struct CommitDetail {
    #[serde(default)]
    message: String,
    author: Option<CommitPerson>,
}

#[derive(Deserialize)]
struct CommitRow {
    sha: String,
    commit: Option<CommitDetail>,
    author: Option<User>,
}

/// Oldest first; the server lists them newest first.
pub async fn commits(data_dir: &Path, input: PullRef) -> ForgejoResult<Vec<PullCommit>> {
    let rows: Vec<CommitRow> = client::get_all(
        data_dir,
        &input.path("/commits")?,
        &[
            ("stat", "false".into()),
            ("verification", "false".into()),
            ("files", "false".into()),
        ],
        5,
    )
    .await?;
    let mut commits: Vec<PullCommit> = rows
        .into_iter()
        .map(|row| {
            let (message, written_by, date) = match row.commit {
                Some(detail) => {
                    let (name, date) = detail
                        .author
                        .map(|person| (person.name, person.date))
                        .unwrap_or_default();
                    (detail.message, name, date)
                }
                None => (String::new(), None, None),
            };
            PullCommit {
                sha: row.sha,
                message,
                author: login_of(row.author.as_ref()).or(written_by),
                avatar_url: avatar_of(row.author.as_ref()),
                date,
            }
        })
        .collect();
    commits.reverse();
    Ok(commits)
}

#[derive(Deserialize)]
pub struct ReviewRow {
    pub id: u64,
    pub user: Option<User>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub dismissed: bool,
    pub submitted_at: Option<String>,
}

/// A review's verdict in GitHub's words; none for one still being written or only asked for.
pub fn verdict(review: &ReviewRow) -> Option<&'static str> {
    if review.dismissed {
        return Some("DISMISSED");
    }
    match review.state.as_str() {
        "APPROVED" => Some("APPROVED"),
        "REQUEST_CHANGES" => Some("CHANGES_REQUESTED"),
        "COMMENT" => Some("COMMENTED"),
        _ => None,
    }
}

pub async fn review_rows(data_dir: &Path, input: &PullRef) -> ForgejoResult<Vec<ReviewRow>> {
    client::get_all(data_dir, &input.path("/reviews")?, &[], 5).await
}

pub async fn reviews(data_dir: &Path, input: PullRef) -> ForgejoResult<Vec<Review>> {
    Ok(review_rows(data_dir, &input)
        .await?
        .into_iter()
        .filter_map(|row| {
            Some(Review {
                state: verdict(&row)?.into(),
                author: login_of(row.user.as_ref()),
                avatar_url: avatar_of(row.user.as_ref()),
                body: row.body,
                submitted_at: row.submitted_at,
            })
        })
        .collect())
}

/// Forgejo reads the merge's fields by their old capitalised names and Gitea by
/// snake case, so both are sent. `sha` is the head the person saw: the server
/// refuses the merge if the branch has moved since.
pub fn merge_body(method: &str, sha: Option<&str>, message: Option<&str>) -> ForgejoResult<Value> {
    if !matches!(method, "merge" | "squash" | "rebase") {
        return Err(ForgejoError::MergeMethod(format!(
            "Forgejo cannot merge by {method}"
        )));
    }
    let mut body = Map::new();
    body.insert("Do".into(), json!(method));
    body.insert("do".into(), json!(method));
    if let Some(sha) = sha.filter(|sha| !sha.is_empty()) {
        body.insert("head_commit_id".into(), json!(sha));
    }
    if let Some(message) = message.filter(|message| !message.trim().is_empty()) {
        body.insert("MergeMessageField".into(), json!(message));
        body.insert("merge_message_field".into(), json!(message));
    }
    Ok(Value::Object(body))
}

pub async fn merge(data_dir: &Path, input: Merge) -> ForgejoResult<()> {
    let method = input.method.as_deref().unwrap_or("merge");
    let body = merge_body(method, input.sha.as_deref(), input.message.as_deref())?;
    client::API
        .write(
            data_dir,
            Method::POST,
            &input.pull.path("/merge")?,
            Some(&body),
        )
        .await
}

/// A draft is marked by its title, as the server's own pages mark one.
pub fn titled(title: &str, draft: bool) -> String {
    let title = title.trim();
    let marked = ["wip:", "[wip]"]
        .iter()
        .any(|mark| title.to_ascii_lowercase().starts_with(mark));
    if draft && !marked {
        format!("WIP: {title}")
    } else {
        title.to_string()
    }
}

pub async fn create(data_dir: &Path, input: NewPull) -> ForgejoResult<Pull> {
    if input.title.trim().is_empty() {
        return Err(ForgejoError::BadArg("a pull request needs a title".into()));
    }
    let body = json!({
        "head": input.head.trim(),
        "base": input.base.trim(),
        "title": titled(&input.title, input.draft),
        "body": input.body,
    });
    let row: PullRow = client::API
        .send_json(data_dir, Method::POST, &input.repo.path("/pulls")?, &body)
        .await?;
    Ok(pull_of(row))
}

pub async fn edit(data_dir: &Path, input: PullEdit) -> ForgejoResult<Pull> {
    let mut body = Map::new();
    if let Some(title) = input.title.filter(|title| !title.trim().is_empty()) {
        body.insert("title".into(), title.trim().into());
    }
    if let Some(text) = input.body {
        body.insert("body".into(), text.into());
    }
    if let Some(base) = input.base {
        body.insert("base".into(), base.into());
    }
    let row: PullRow = client::API
        .send_json(
            data_dir,
            Method::PATCH,
            &input.pull.path("")?,
            &Value::Object(body),
        )
        .await?;
    Ok(pull_of(row))
}

pub async fn set_state(data_dir: &Path, input: SetPullState) -> ForgejoResult<()> {
    let state = if input.state == "open" {
        "open"
    } else {
        "closed"
    };
    client::API
        .write(
            data_dir,
            Method::PATCH,
            &input.pull.path("")?,
            Some(&json!({ "state": state })),
        )
        .await
}

pub async fn review(data_dir: &Path, input: NewReview) -> ForgejoResult<()> {
    let body = input.body.trim();
    let event = match input.event.as_str() {
        "APPROVE" => "APPROVED",
        "REQUEST_CHANGES" => "REQUEST_CHANGES",
        "COMMENT" if !body.is_empty() => "COMMENT",
        "COMMENT" => return Err(ForgejoError::BadArg("a comment needs some text".into())),
        other => {
            return Err(ForgejoError::BadArg(format!(
                "`{other}` is not a review verdict"
            )))
        }
    };
    client::API
        .write(
            data_dir,
            Method::POST,
            &input.pull.path("/reviews")?,
            Some(&json!({ "event": event, "body": body })),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(value: Value) -> PullRow {
        serde_json::from_value(value).expect("pull request parses")
    }

    #[test]
    fn a_pull_request_from_a_fork_names_its_branch_there() {
        let pull = pull_of(row(json!({
            "number": 42, "title": "Fix the VAT rounding", "body": "Rounds per line.", "state": "open", "draft": false,
            "user": { "login": "ankit", "avatar_url": "https://codeberg.org/avatars/a" },
            "head": { "ref": "fix/vat", "sha": "9c41e2a0", "repo_id": 8, "repo": { "full_name": "ankit/api" } },
            "base": { "ref": "main", "repo_id": 7 },
            "created_at": "t1", "updated_at": "t2", "comments": 3, "mergeable": true,
            "labels": [ { "name": "backend", "color": "428bca" } ],
            "requested_reviewers": [ { "login": "irwan", "avatar_url": "https://codeberg.org/avatars/i" } ],
            "assignees": null, "milestone": { "title": "Q4" },
            "html_url": "https://codeberg.org/acme/api/pulls/42"
        })));
        assert_eq!(
            (
                pull.number,
                pull.state,
                pull.head.as_deref(),
                pull.base.as_deref()
            ),
            (42, PullStage::Open, Some("fix/vat"), Some("main"))
        );
        assert_eq!(pull.head_label.as_deref(), Some("ankit:fix/vat"));
        assert_eq!(pull.merge_state.as_deref(), Some("clean"));
        assert_eq!(pull.reviewers, ["irwan"]);
        assert!(pull.assignees.is_empty());
    }

    #[test]
    fn a_merged_one_reads_as_merged_however_its_state_is_written() {
        let pull = pull_of(row(json!({
            "number": 7, "state": "closed", "merged": true, "created_at": "t1",
            "merged_by": { "login": "irwan" }, "merge_commit_sha": "abc", "mergeable": false,
            "html_url": "u"
        })));
        assert_eq!(pull.state, PullStage::Merged);
        assert_eq!(pull.merged_by.as_deref(), Some("irwan"));
        assert_eq!((pull.mergeable, pull.merge_state), (None, None));
    }

    #[test]
    fn a_whole_diff_splits_into_each_files_patch() {
        let diff = "diff --git a/src/vat.ts b/src/tax/vat.ts\nsimilarity index 80%\nrename from src/vat.ts\nrename to src/tax/vat.ts\n--- a/src/vat.ts\n+++ b/src/tax/vat.ts\n@@ -1,2 +1,2 @@\n-const a = 1;\n+const a = 2;\n\ndiff --git a/old.md b/old.md\ndeleted file mode 100644\n--- a/old.md\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\ndiff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n";
        let found = patches(diff);
        assert_eq!(
            found,
            [
                (
                    "src/tax/vat.ts".to_string(),
                    "@@ -1,2 +1,2 @@\n-const a = 1;\n+const a = 2;".to_string()
                ),
                ("old.md".to_string(), "@@ -1 +0,0 @@\n-gone".to_string()),
            ]
        );
    }

    #[test]
    fn merging_names_the_way_in_both_spellings() -> ForgejoResult<()> {
        let body = merge_body("squash", Some("abc"), None)?;
        assert_eq!(
            body,
            json!({ "Do": "squash", "do": "squash", "head_commit_id": "abc" })
        );
        assert!(merge_body("fast-forward", None, None).is_err());
        Ok(())
    }

    #[test]
    fn a_draft_is_marked_in_its_title_once() {
        assert_eq!(titled(" Fix VAT ", true), "WIP: Fix VAT");
        assert_eq!(titled("[WIP] Fix VAT", true), "[WIP] Fix VAT");
        assert_eq!(titled("Fix VAT", false), "Fix VAT");
    }
}
