//! A pull request's or issue's conversation: its opening post, then everything its
//! timeline records, comments and reviews as posts and the rest as one line each.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use gpui_transcript::{Markdown, Pen};
use sikemux_wire::codehost::{PullCommit, TimelineItem};

use super::ago;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::toast;
use crate::ui;

/// Something someone wrote in the thread, the opening description included.
pub struct Post {
    pub author: Option<String>,
    pub association: Option<String>,
    pub at: Option<String>,
    /// A review's verdict in capitals, when the post is a review.
    pub review: Option<String>,
    pub avatar: Option<String>,
    pub body: Rc<Markdown>,
    pub empty: bool,
}

impl Post {
    pub fn new(
        author: Option<String>,
        association: Option<String>,
        at: Option<String>,
        review: Option<String>,
        body: &str,
    ) -> Self {
        Self {
            author,
            association,
            at,
            review,
            avatar: None,
            body: Rc::new(crate::editor::preview::render(body)),
            empty: body.trim().is_empty(),
        }
    }

    pub fn pictured(mut self, avatar: Option<String>) -> Self {
        self.avatar = avatar;
        self
    }
}

/// Someone's picture by their login and, when the host gave one, its address.
pub type Faces<'a> = &'a dyn Fn(Option<&str>, Option<&str>, f32, &App) -> AnyElement;

/// One step of the thread: a post, commits pushed together, or something that happened.
pub enum Block {
    Post(Post),
    Commits(Vec<TimelineItem>),
    Event(Box<TimelineItem>),
}

/// The timeline's items as the thread shows them: comments and reviews with words as
/// posts, commits pushed one after another together, and the rest as they are.
pub fn blocks(items: Vec<TimelineItem>) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for item in items {
        let worded = item
            .body
            .as_deref()
            .is_some_and(|body| !body.trim().is_empty());
        match item.kind.as_str() {
            "committed" => match blocks.last_mut() {
                Some(Block::Commits(commits)) => commits.push(item),
                _ => blocks.push(Block::Commits(vec![item])),
            },
            "commented" => blocks.push(Block::Post(
                Post::new(
                    item.actor,
                    item.association,
                    item.at,
                    None,
                    item.body.as_deref().unwrap_or_default(),
                )
                .pictured(item.avatar_url),
            )),
            "reviewed" if worded => blocks.push(Block::Post(
                Post::new(
                    item.actor,
                    item.association,
                    item.at,
                    Some(item.state.as_deref().unwrap_or("commented").to_uppercase()),
                    item.body.as_deref().unwrap_or_default(),
                )
                .pictured(item.avatar_url),
            )),
            _ => blocks.push(Block::Event(Box::new(item))),
        }
    }
    blocks
}

/// Who pushed each commit, by its hash, as the host matched them to an account.
pub type Pushers = HashMap<String, String>;

pub fn pushers(commits: &[PullCommit]) -> Pushers {
    commits
        .iter()
        .filter_map(|commit| Some((commit.sha.clone(), commit.author.clone()?)))
        .collect()
}

/// How many of the thread's steps are things people said.
pub fn said(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .filter(|block| matches!(block, Block::Post(_)))
        .count()
}

pub fn thread(
    opening: Option<&Post>,
    blocks: &[Block],
    pushers: &Pushers,
    base: Option<&str>,
    faces: Faces,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let pen = Pen::new(Rc::new(crate::editor::preview::reading_style(
        13.,
        Tokens::get(cx),
    )));
    let mut thread = div().flex().flex_col().gap(px(14.)).px(px(16.)).py(px(16.));
    if let Some(opening) = opening {
        thread = thread.child(post(&pen, "post-opening", opening, faces, window, cx));
    }
    for (index, block) in blocks.iter().enumerate() {
        thread = thread.child(match block {
            Block::Post(said) => post(&pen, ("post", index), said, faces, window, cx),
            Block::Commits(commits) => pushed(index, commits, pushers, faces, cx),
            Block::Event(item) => event(item, base, faces, cx),
        });
    }
    thread
}

fn role(association: &str) -> Option<&'static str> {
    Some(match association {
        "OWNER" => "Owner",
        "MEMBER" => "Member",
        "COLLABORATOR" => "Collaborator",
        "CONTRIBUTOR" => "Contributor",
        "FIRST_TIME_CONTRIBUTOR" | "FIRST_TIMER" => "First-time contributor",
        _ => return None,
    })
}

fn review_words(verdict: &str) -> String {
    match verdict {
        "APPROVED" => "approved these changes".into(),
        "CHANGES_REQUESTED" => "requested changes".into(),
        "COMMENTED" => "reviewed".into(),
        "DISMISSED" => "had a review dismissed".into(),
        other => other.to_lowercase(),
    }
}

fn verdict_ink(verdict: &str, t: &Tokens) -> Hsla {
    match verdict {
        "APPROVED" => t.live,
        "CHANGES_REQUESTED" => t.danger,
        _ => t.text_tertiary,
    }
}

fn post(
    pen: &Pen,
    id: impl Into<gpui_kit::ElementId>,
    post: &Post,
    faces: Faces,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let author = post.author.as_deref().unwrap_or("someone");
    let saying = match &post.review {
        Some(verdict) => div()
            .text_color(verdict_ink(verdict, &t))
            .child(review_words(verdict)),
        None => div().text_color(t.text_tertiary).child("commented"),
    };
    let body = if post.empty {
        div()
            .text_size(crate::theme::ui_text(12.))
            .italic()
            .text_color(t.text_tertiary)
            .child("No description provided.")
            .into_any_element()
    } else {
        pen.markdown(id, &post.body, window, cx)
    };
    div()
        .flex()
        .gap(px(12.))
        .child(faces(Some(author), post.avatar.as_deref(), 28., cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .rounded(theme::RADIUS_2)
                .border_1()
                .border_color(t.border)
                .child(
                    div()
                        .px(px(12.))
                        .py(px(7.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .border_b_1()
                        .border_color(t.border)
                        .bg(t.surface_sunken.opacity(0.4))
                        .text_size(crate::theme::ui_text(12.))
                        .child(
                            ui::text(author.to_owned(), 12., FontWeight::SEMIBOLD)
                                .text_color(t.text_primary),
                        )
                        .child(saying)
                        .children(
                            post.at
                                .as_deref()
                                .map(|at| div().text_color(t.text_tertiary).child(ago(at))),
                        )
                        .child(div().flex_1())
                        .children(post.association.as_deref().and_then(role).map(|role| {
                            div()
                                .px(px(6.))
                                .rounded_full()
                                .border_1()
                                .border_color(t.border)
                                .text_size(crate::theme::ui_text(10.))
                                .text_color(t.text_tertiary)
                                .child(role)
                        })),
                )
                .child(div().px(px(14.)).py(px(10.)).child(body)),
        )
        .into_any_element()
}

/// One thing that happened, as a line beside the thread with its badge.
fn line(glyph: Icon, ink: Option<Hsla>, words: Vec<AnyElement>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .pl(px(6.))
        .min_w_0()
        .text_size(crate::theme::ui_text(12.))
        .text_color(t.text_secondary)
        .child(
            div()
                .size(px(22.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(ink.map_or(t.surface_active, |ink| ink.opacity(0.16)))
                .child(ui::icon(glyph, 12.).text_color(ink.unwrap_or(t.text_tertiary))),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(5.))
                .min_w_0()
                .children(words),
        )
        .into_any_element()
}

fn who(login: Option<&str>, url: Option<&str>, faces: Faces, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let login = login.unwrap_or("someone");
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(faces(Some(login), url, 16., cx))
        .child(ui::text(login.to_owned(), 12., FontWeight::SEMIBOLD).text_color(t.text_primary))
        .into_any_element()
}

fn dim(words: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_color(Tokens::get(cx).text_tertiary)
        .child(words.into())
        .into_any_element()
}

fn chip(words: impl Into<SharedString>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(px(6.))
        .rounded(theme::RADIUS_1)
        .bg(t.surface_active)
        .text_color(t.text_primary)
        .child(words.into())
        .into_any_element()
}

fn event(item: &TimelineItem, base: Option<&str>, faces: Faces, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let actor = who(item.actor.as_deref(), item.avatar_url.as_deref(), faces, cx);
    let when = dim(item.at.as_deref().map(ago).unwrap_or_default(), cx);
    let short = item
        .sha
        .as_deref()
        .map(|sha| sha.chars().take(7).collect::<String>());
    let subject = item.subject.clone().unwrap_or_default();
    let (glyph, ink, mut words): (Icon, Option<Hsla>, Vec<AnyElement>) = match item.kind.as_str() {
        "reviewed" => {
            let verdict = item.state.as_deref().unwrap_or("commented").to_uppercase();
            let glyph = if verdict == "APPROVED" {
                Icon::Check
            } else {
                Icon::Eye
            };
            let ink = matches!(verdict.as_str(), "APPROVED" | "CHANGES_REQUESTED")
                .then(|| verdict_ink(&verdict, &t));
            let said = div()
                .text_color(verdict_ink(&verdict, &t))
                .child(review_words(&verdict))
                .into_any_element();
            (glyph, ink, vec![actor, said])
        }
        "review_requested" => (
            Icon::Eye,
            None,
            vec![
                actor,
                dim("requested a review from", cx),
                who(Some(&subject), None, faces, cx),
            ],
        ),
        "review_request_removed" => (
            Icon::Eye,
            None,
            vec![
                actor,
                dim("removed the review request for", cx),
                who(Some(&subject), None, faces, cx),
            ],
        ),
        "head_ref_force_pushed" => {
            let to = short
                .as_ref()
                .map(|short| format!(" to {short}"))
                .unwrap_or_default();
            (
                Icon::Push,
                None,
                vec![actor, dim(format!("force-pushed the branch{to}"), cx)],
            )
        }
        "merged" => {
            let mut words = vec![actor, dim("merged", cx)];
            if let Some(short) = &short {
                words.extend([dim("commit", cx), chip(short.clone(), cx)]);
            }
            words.extend([
                dim("into", cx),
                chip(base.unwrap_or("the base branch").to_owned(), cx),
            ]);
            (Icon::Merge, Some(t.accent), words)
        }
        "closed" => {
            let unplanned = item.state.as_deref() == Some("not_planned");
            let words = if unplanned {
                "closed this as not planned"
            } else {
                "closed this"
            };
            (
                Icon::Close,
                (!unplanned).then_some(t.accent),
                vec![actor, dim(words, cx)],
            )
        }
        "reopened" => (
            Icon::Issues,
            Some(t.live),
            vec![actor, dim("reopened this", cx)],
        ),
        "labeled" | "unlabeled" => {
            let done = if item.kind == "labeled" {
                "added the"
            } else {
                "removed the"
            };
            (
                Icon::Releases,
                None,
                vec![actor, dim(done, cx), chip(subject, cx), dim("label", cx)],
            )
        }
        "assigned" | "unassigned" => {
            let done = if item.kind == "assigned" {
                "assigned"
            } else {
                "unassigned"
            };
            (
                Icon::User,
                None,
                vec![actor, dim(done, cx), who(Some(&subject), None, faces, cx)],
            )
        }
        "renamed" => (
            Icon::Pencil,
            None,
            vec![actor, dim("changed the title to", cx), chip(subject, cx)],
        ),
        "ready_for_review" => (
            Icon::Eye,
            None,
            vec![actor, dim("marked this ready for review", cx)],
        ),
        "convert_to_draft" => (
            Icon::Pencil,
            None,
            vec![actor, dim("marked this as a draft", cx)],
        ),
        "head_ref_deleted" => (Icon::Git, None, vec![actor, dim("deleted the branch", cx)]),
        "head_ref_restored" => (Icon::Git, None, vec![actor, dim("restored the branch", cx)]),
        "referenced" => {
            let in_commit = short
                .map(|short| format!(" in commit {short}"))
                .unwrap_or_default();
            (
                Icon::Commit,
                None,
                vec![actor, dim(format!("referenced this{in_commit}"), cx)],
            )
        }
        "cross-referenced" => (
            Icon::Issues,
            None,
            vec![actor, dim("mentioned this in", cx), chip(subject, cx)],
        ),
        _ => return div().into_any_element(),
    };
    words.push(when);
    line(glyph, ink, words, cx)
}

/// Commits pushed together: who pushed them and when, then each by its message.
fn pushed(
    index: usize,
    commits: &[TimelineItem],
    pushers: &Pushers,
    faces: Faces,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let pusher = |commit: &TimelineItem| {
        commit
            .sha
            .as_ref()
            .and_then(|sha| pushers.get(sha))
            .cloned()
            .or_else(|| commit.actor.clone())
    };
    let mut authors: Vec<String> = Vec::new();
    for author in commits.iter().filter_map(pusher) {
        if !authors.contains(&author) {
            authors.push(author);
        }
    }
    let count = commits.len();
    let mut words = vec![who(authors.first().map(String::as_str), None, faces, cx)];
    if authors.len() > 1 {
        words.push(dim("and others", cx));
    }
    let plural = if count == 1 { "" } else { "s" };
    let when = commits
        .first()
        .and_then(|commit| commit.at.as_deref())
        .map(ago)
        .unwrap_or_default();
    words.push(dim(format!("added {count} commit{plural} {when}"), cx));
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(line(Icon::Push, None, words, cx))
        .children(commits.iter().enumerate().map(|(at, commit)| {
            let message = commit.message.as_deref().unwrap_or_default();
            let first = message.lines().next().unwrap_or_default().to_owned();
            let sha = commit.sha.clone().unwrap_or_default();
            let short: String = sha.chars().take(7).collect();
            let author = pusher(commit).unwrap_or_default();
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(36.))
                .min_w_0()
                .text_size(crate::theme::ui_text(12.))
                .child(ui::icon(Icon::Commit, 13.).text_color(t.text_tertiary))
                .child(faces(Some(&author), None, 16., cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_secondary)
                        .child(first),
                )
                .when(!sha.is_empty(), |row| {
                    row.child(
                        div()
                            .id(("pushed-sha", index * 1000 + at))
                            .font_family(theme::MONO)
                            .text_size(crate::theme::ui_text(11.))
                            .text_color(t.text_tertiary)
                            .cursor_pointer()
                            .hover(move |style| style.text_color(t.text_primary))
                            .tooltip(ui::tooltip("Copy the full commit"))
                            .on_click(move |_, window, cx| {
                                use gpui_kit::component::WindowExt;
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    sha.clone(),
                                ));
                                let note = toast::success(format!(
                                    "Copied {}",
                                    sha.chars().take(7).collect::<String>()
                                ));
                                window.push_notification(note, cx);
                            })
                            .child(short),
                    )
                })
        }))
        .into_any_element()
}
