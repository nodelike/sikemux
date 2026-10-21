// What reviewers asked of a pull request, general and inline, and answering
// each of them in its own thread.

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::ReviewFeedback;

use crate::client;
use crate::common::LIST_PAGES;
use crate::error::{GithubError, GithubResult};
use crate::pulls::PullRef;

#[derive(Deserialize)]
struct Author {
    login: String,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Row {
    id: u64,
    user: Option<Author>,
    body: Option<String>,
    html_url: Option<String>,
    updated_at: Option<String>,
    submitted_at: Option<String>,
    state: Option<String>,
    path: Option<String>,
    line: Option<u64>,
    original_line: Option<u64>,
}

impl Row {
    fn feedback(self, kind: &str) -> ReviewFeedback {
        ReviewFeedback {
            id: format!("{kind}/{}", self.id),
            bot: self
                .user
                .as_ref()
                .is_some_and(|user| user.kind.as_deref() == Some("Bot")),
            author: self.user.map(|user| user.login),
            body: self.body.unwrap_or_default(),
            line: self.line.or(self.original_line),
            path: self.path,
            state: self.state,
            at: self.updated_at.or(self.submitted_at).unwrap_or_default(),
            url: self.html_url,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackQuery {
    #[serde(flatten)]
    pub pull: PullRef,
    /// Only what changed at or after this time, as an earlier answer gave it.
    pub since: Option<String>,
}

pub async fn feedback(data_dir: &Path, input: FeedbackQuery) -> GithubResult<Vec<ReviewFeedback>> {
    let PullRef { repo, number } = &input.pull;
    let since: Vec<(&str, String)> = input.since.iter().map(|at| ("since", at.clone())).collect();
    let general = repo.path(&format!("/issues/{number}/comments"))?;
    let inline = repo.path(&format!("/pulls/{number}/comments"))?;
    let reviews = repo.path(&format!("/pulls/{number}/reviews"))?;
    let rows = |path| client::get_all(data_dir, path, &since, LIST_PAGES, |rows: Vec<Row>| rows);
    let (general, inline, reviews) = futures::try_join!(
        rows(&general),
        rows(&inline),
        client::get_all(data_dir, &reviews, &[], LIST_PAGES, |rows: Vec<Row>| rows),
    )?;
    let reviews = reviews
        .into_iter()
        .filter(|row| match row.state.as_deref() {
            Some("CHANGES_REQUESTED") => true,
            Some("COMMENTED") => row
                .body
                .as_deref()
                .is_some_and(|body| !body.trim().is_empty()),
            _ => false,
        })
        .filter(|row| match (&input.since, &row.submitted_at) {
            (Some(since), Some(at)) => at >= since,
            _ => true,
        });
    Ok(general
        .into_iter()
        .map(|row| row.feedback("comment"))
        .chain(inline.into_iter().map(|row| row.feedback("inline")))
        .chain(reviews.map(|row| row.feedback("review")))
        .collect())
}

#[derive(Deserialize)]
pub struct Reply {
    to: String,
    body: String,
    /// What the reply answers, quoted where it cannot sit in that thread.
    #[serde(default)]
    quote: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answers {
    #[serde(flatten)]
    pub pull: PullRef,
    pub replies: Vec<Reply>,
    /// Asked to review again, once every reply is in.
    #[serde(default)]
    pub reviewers: Vec<String>,
}

fn quoted(reply: &Reply) -> String {
    let quote: Vec<String> = reply
        .quote
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(3)
        .map(|line| format!("> {line}"))
        .collect();
    if quote.is_empty() {
        reply.body.clone()
    } else {
        format!("{}\n\n{}", quote.join("\n"), reply.body)
    }
}

pub async fn answer(data_dir: &Path, input: Answers) -> GithubResult<()> {
    let PullRef { repo, number } = &input.pull;
    for reply in &input.replies {
        let (kind, id) = reply
            .to
            .split_once('/')
            .filter(|(_, id)| id.parse::<u64>().is_ok())
            .ok_or_else(|| {
                GithubError::BadArg(format!("`{}` is not a comment to answer", reply.to))
            })?;
        let (path, body) = if kind == "inline" {
            (
                repo.path(&format!("/pulls/{number}/comments/{id}/replies"))?,
                reply.body.clone(),
            )
        } else {
            (
                repo.path(&format!("/issues/{number}/comments"))?,
                quoted(reply),
            )
        };
        client::post_empty(data_dir, &path, Some(&json!({ "body": body }))).await?;
    }
    if input.reviewers.is_empty() {
        return Ok(());
    }
    let pull: Value = client::get(data_dir, &repo.path(&format!("/pulls/{number}"))?, &[]).await?;
    let author = pull.pointer("/user/login").and_then(Value::as_str);
    let reviewers: Vec<&String> = input
        .reviewers
        .iter()
        .filter(|login| Some(login.as_str()) != author)
        .collect();
    if reviewers.is_empty() {
        return Ok(());
    }
    let path = repo.path(&format!("/pulls/{number}/requested_reviewers"))?;
    client::post_empty(data_dir, &path, Some(&json!({ "reviewers": reviewers }))).await
}
