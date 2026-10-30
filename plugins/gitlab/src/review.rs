// What reviewers asked of a merge request, general and on a line, and
// answering each in its own discussion.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::asks::{Answers, FeedbackQuery};
use sikemux_plugin_api::codehost::ReviewFeedback;

use crate::client;
use crate::error::{GitlabError, GitlabResult};
use crate::repo::{login_of, ApiPath, User};

#[derive(Deserialize)]
struct Author {
    #[serde(flatten)]
    user: User,
    #[serde(default)]
    bot: bool,
}

#[derive(Deserialize)]
struct Position {
    new_path: Option<String>,
    old_path: Option<String>,
    new_line: Option<u64>,
    old_line: Option<u64>,
}

#[derive(Deserialize)]
struct Note {
    id: u64,
    #[serde(default)]
    body: String,
    author: Option<Author>,
    updated_at: String,
    #[serde(default)]
    system: bool,
    position: Option<Position>,
}

#[derive(Deserialize)]
struct Discussion {
    id: String,
    #[serde(default)]
    notes: Vec<Note>,
}

#[derive(Deserialize)]
struct Reviewer {
    user: Option<User>,
    state: Option<String>,
    created_at: Option<String>,
}

const DISCUSSION_PAGES: u32 = 10;

/// The id names the discussion a note is in, since that is what a reply goes to.
fn note_feedback(discussion: &str, note: Note) -> ReviewFeedback {
    let kind = match note.position {
        Some(_) => "inline",
        None => "comment",
    };
    let (path, line) = note
        .position
        .map(|at| (at.new_path.or(at.old_path), at.new_line.or(at.old_line)))
        .unwrap_or_default();
    ReviewFeedback {
        id: format!("{kind}/{discussion}/{}", note.id),
        bot: note.author.as_ref().is_some_and(|author| author.bot),
        author: note.author.and_then(|author| login_of(Some(&author.user))),
        body: note.body,
        path,
        line,
        state: None,
        at: note.updated_at,
        url: None,
    }
}

pub async fn feedback(data_dir: &Path, input: FeedbackQuery) -> GitlabResult<Vec<ReviewFeedback>> {
    let discussions_path = input.pull.path("/discussions")?;
    let reviewers_path = input.pull.path("/reviewers")?;
    let (discussions, reviewers) = futures::try_join!(
        client::get_all::<Discussion>(data_dir, &discussions_path, &[], DISCUSSION_PAGES),
        client::get::<Vec<Reviewer>>(data_dir, &reviewers_path, &[]),
    )?;
    let newer = |at: &str| input.since.as_deref().is_none_or(|since| at >= since);
    let notes = discussions.into_iter().flat_map(|discussion| {
        let id = discussion.id;
        discussion
            .notes
            .into_iter()
            .filter(|note| !note.system && newer(&note.updated_at))
            .map(|note| note_feedback(&id, note))
            .collect::<Vec<_>>()
    });
    let asked = reviewers.into_iter().filter_map(|reviewer| {
        let at = reviewer.created_at?;
        (reviewer.state.as_deref() == Some("requested_changes") && newer(&at)).then(|| {
            let author = login_of(reviewer.user.as_ref());
            ReviewFeedback {
                id: format!("review/{}@{at}", author.clone().unwrap_or_default()),
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
    Ok(notes.chain(asked).collect())
}

/// The discussion a feedback id names, which only hex digits make up.
fn discussion_of(to: &str) -> GitlabResult<Option<&str>> {
    let mut parts = to.split('/');
    match (parts.next(), parts.next()) {
        (Some("comment" | "inline"), Some(id)) if id.chars().all(|c| c.is_ascii_hexdigit()) => {
            Ok(Some(id))
        }
        (Some("review"), _) => Ok(None),
        _ => Err(GitlabError::BadArg(format!(
            "`{to}` is not a comment to answer"
        ))),
    }
}

/// A reply in each discussion is what tells its reviewer to look again; one to
/// a reviewer who asked for changes starts a discussion of its own.
pub async fn answer(data_dir: &Path, input: Answers) -> GitlabResult<()> {
    for reply in &input.replies {
        let path = match discussion_of(&reply.to)? {
            Some(discussion) => input
                .pull
                .path(&format!("/discussions/{discussion}/notes"))?,
            None => input.pull.path("/discussions")?,
        };
        let _: Value = client::send_json(
            data_dir,
            Method::POST,
            &path,
            &json!({ "body": reply.body }),
        )
        .await?;
    }
    Ok(())
}
