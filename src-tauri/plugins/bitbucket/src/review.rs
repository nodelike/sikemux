// What reviewers asked of a pull request, general and inline, and answering
// each of them in its own thread.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::ReviewFeedback;

use crate::client;
use crate::error::{BitbucketError, BitbucketResult};
use crate::pulls::Thread;
use crate::repo::{Links, User};

#[derive(Deserialize)]
struct Author {
    #[serde(flatten)]
    user: User,
    /// `user` for a person; apps and bots have others.
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Content {
    raw: Option<String>,
}

#[derive(Deserialize)]
struct Inline {
    path: Option<String>,
    to: Option<u64>,
    from: Option<u64>,
}

#[derive(Deserialize)]
struct CommentRow {
    id: u64,
    content: Option<Content>,
    user: Option<Author>,
    updated_on: String,
    inline: Option<Inline>,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    links: Links,
}

#[derive(Deserialize)]
struct Participant {
    user: Option<User>,
    state: Option<String>,
    participated_on: Option<String>,
}

#[derive(Deserialize)]
struct PullRow {
    #[serde(default)]
    participants: Vec<Participant>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackQuery {
    #[serde(flatten)]
    pub thread: Thread,
    /// Only what changed at or after this time, as an earlier answer gave it.
    pub since: Option<String>,
}

fn pull_path(thread: &Thread, rest: &str) -> BitbucketResult<String> {
    thread
        .repo
        .path(&format!("/pullrequests/{}{rest}", thread.number))
}

pub async fn feedback(
    data_dir: &Path,
    input: FeedbackQuery,
) -> BitbucketResult<Vec<ReviewFeedback>> {
    let mut query = vec![
        ("pagelen", "100".to_owned()),
        ("sort", "updated_on".to_owned()),
    ];
    if let Some(since) = &input.since {
        query.push(("q", format!("updated_on >= {since}")));
    }
    let comments_path = pull_path(&input.thread, "/comments")?;
    let pull_path = pull_path(&input.thread, "")?;
    let (comments, pull) = futures::try_join!(
        client::get_all::<CommentRow>(data_dir, &comments_path, &query, 10),
        client::get::<PullRow>(data_dir, &pull_path, &[]),
    )?;
    let comments = comments
        .into_iter()
        .filter(|row| !row.deleted)
        .map(|row| ReviewFeedback {
            id: format!(
                "{}/{}",
                if row.inline.is_some() {
                    "inline"
                } else {
                    "comment"
                },
                row.id
            ),
            bot: row
                .user
                .as_ref()
                .is_some_and(|author| author.kind.as_deref().is_some_and(|kind| kind != "user")),
            author: row.user.and_then(|author| author.user.login()),
            body: row
                .content
                .and_then(|content| content.raw)
                .unwrap_or_default(),
            path: row.inline.as_ref().and_then(|inline| inline.path.clone()),
            line: row.inline.and_then(|inline| inline.to.or(inline.from)),
            state: None,
            at: row.updated_on,
            url: row.links.html.href,
        });
    let asked = pull.participants.into_iter().filter_map(|participant| {
        let at = participant.participated_on?;
        let newer = input.since.as_ref().is_none_or(|since| &at >= since);
        (participant.state.as_deref() == Some("changes_requested") && newer).then(|| {
            let author = participant.user.as_ref().and_then(User::login);
            let who = participant
                .user
                .and_then(|user| user.uuid)
                .or(author.clone())
                .unwrap_or_default();
            ReviewFeedback {
                id: format!("review/{who}@{at}"),
                author,
                bot: false,
                body: String::new(),
                path: None,
                line: None,
                state: Some("CHANGES_REQUESTED".to_owned()),
                at,
                url: None,
            }
        })
    });
    Ok(comments.chain(asked).collect())
}

#[derive(Deserialize)]
pub struct Reply {
    to: String,
    body: String,
}

#[derive(Deserialize)]
pub struct Answers {
    #[serde(flatten)]
    pub thread: Thread,
    pub replies: Vec<Reply>,
}

/// Bitbucket has no asking for a review again, so a reply in each thread is
/// what tells reviewers to look.
pub async fn answer(data_dir: &Path, input: Answers) -> BitbucketResult<()> {
    let path = pull_path(&input.thread, "/comments")?;
    for reply in &input.replies {
        let parent = match reply.to.split_once('/') {
            Some(("comment" | "inline", id)) => Some(id.parse::<u64>().map_err(|_| {
                BitbucketError::BadArg(format!("`{}` is not a comment to answer", reply.to))
            })?),
            _ => None,
        };
        let body = match parent {
            Some(parent) => json!({ "content": { "raw": reply.body }, "parent": { "id": parent } }),
            None => json!({ "content": { "raw": reply.body } }),
        };
        let _: Value = client::send_json(data_dir, Method::POST, &path, &body).await?;
    }
    Ok(())
}
