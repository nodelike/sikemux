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
use sikemux_plugin_api::codehost::{Mention, PullCommit, TimelineItem};

use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3H, SPACE_4, TEXT_2XS_H,
    TEXT_3XS,
};
use crate::toast;
use crate::ui;
use crate::ui::time::ago_written as ago;

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

/// Who pushed each commit, by its hash, as the host matched them to an account, with
/// their picture.
pub type Pushers = HashMap<String, Pusher>;

#[derive(Clone, PartialEq)]
pub struct Pusher {
    login: String,
    picture: Option<String>,
}

pub fn pushers(commits: &[PullCommit]) -> Pushers {
    commits
        .iter()
        .filter_map(|commit| {
            let pusher = Pusher {
                login: commit.author.clone()?,
                picture: commit.avatar_url.clone(),
            };
            Some((commit.sha.clone(), pusher))
        })
        .collect()
}

/// How many of the thread's steps are things people said.
pub fn said(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .filter(|block| matches!(block, Block::Post(_)))
        .count()
}

#[allow(clippy::too_many_arguments)]
pub fn thread(
    opening: Option<&Post>,
    blocks: &[Block],
    pushers: &Pushers,
    base: Option<&str>,
    faces: Faces,
    bench: &gpui_kit::WeakEntity<crate::workbench::Workbench>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let pen = Pen::new(Rc::new(crate::editor::preview::reading_linked(
        TEXT, bench, cx,
    )));
    let mut items: Vec<Item> = Vec::new();
    if let Some(opening) = opening {
        let opened = post(&pen, "post-opening", opening, faces, window, cx);
        items.push(Item::new(opened, GAP, 0., None));
    }
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Post(said) => {
                let said = post(&pen, ("post", index), said, faces, window, cx);
                items.push(Item::new(said, GAP, 0., None));
            }
            Block::Commits(commits) => items.extend(pushed(index, commits, pushers, faces, cx)),
            Block::Event(item) => items.extend(
                event(item, base, faces, cx).map(|line| Item::new(line, GAP, 0., Some(BADGE))),
            ),
        }
    }
    let last = items.len().saturating_sub(1);
    let ink = Tokens::get(cx).border;
    let spine = |top: f32, bottom: Option<f32>| {
        let line = div()
            .absolute()
            .left(px(SPINE - 0.5))
            .w(px(1.))
            .top(px(top))
            .bg(ink);
        match bottom {
            Some(height) => line.h(px(height)),
            None => line.bottom_0(),
        }
    };
    div()
        .flex()
        .flex_col()
        .px(SPACE_4)
        .py(SPACE_4)
        .children(items.into_iter().enumerate().map(|(at, item)| {
            let space = if at == 0 { 0. } else { item.space };
            div()
                .relative()
                .mt(px(space))
                .when(at > 0, |row| {
                    row.child(spine(-space, Some(space + item.mark.0)))
                })
                .when_some(item.mark.1.filter(|_| at < last), |row, bottom| {
                    row.child(spine(bottom, None))
                })
                .child(item.shown)
        }))
}

/// One thing in the thread: the space above it, and where the line through the thread meets
/// it and leaves it again, at its badge or commit, or at the top and bottom of a post's box,
/// which sits on the line.
struct Item {
    shown: AnyElement,
    space: f32,
    mark: (f32, Option<f32>),
}

impl Item {
    fn new(shown: AnyElement, space: f32, top: f32, bottom: Option<f32>) -> Self {
        Self {
            shown,
            space,
            mark: (top, bottom),
        }
    }
}

/// A post's face leads its box's header; the line runs into the box straight above it, and
/// through the events' badges and the commits.
const AVATAR: f32 = 28.;
/// Everything the thread says is one size, so a post's words, a commit's message and an
/// event's line read as one conversation.
const TEXT: f32 = 13.;
const HEADER_INSET: f32 = 12.;
const SPINE: f32 = BORDER + HEADER_INSET + AVATAR / 2.;
const BORDER: f32 = 1.;
const BADGE: f32 = 22.;
const GAP: f32 = 14.;
const COMMIT_ROW: f32 = 22.;
const COMMIT_GLYPH: f32 = 13.;

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
            .text_size(crate::theme::ui_text(TEXT))
            .italic()
            .text_color(t.text_tertiary)
            .child("No description provided.")
            .into_any_element()
    } else {
        pen.markdown(id, &post.body, window, cx)
    };
    div()
        .flex()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .rounded(theme::RADIUS_2)
                .border_1()
                .border_color(t.border)
                .bg(t.surface_composer)
                .child(
                    div()
                        .px(px(HEADER_INSET))
                        .py(px(7.))
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .border_b_1()
                        .border_color(t.border)
                        .bg(t.surface_sunken.opacity(0.4))
                        .text_size(crate::theme::ui_text(TEXT))
                        .child(div().mr(SPACE_0).child(faces(
                            Some(author),
                            post.avatar.as_deref(),
                            AVATAR,
                            cx,
                        )))
                        .child(
                            ui::text(author.to_owned(), TEXT, FontWeight::SEMIBOLD)
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
                                .px(SPACE_1H)
                                .rounded_full()
                                .border_1()
                                .border_color(t.border)
                                .text_size(crate::theme::ui_text(TEXT_3XS))
                                .text_color(t.text_tertiary)
                                .child(role)
                        })),
                )
                .child(div().px(SPACE_3H).py(SPACE_2H).child(body)),
        )
        .into_any_element()
}

/// One thing that happened, as a line beside the thread with its badge.
fn line(glyph: Icon, ink: Option<Hsla>, words: Vec<AnyElement>, cx: &App) -> AnyElement {
    line_over(glyph, ink, words, None, cx)
}

/// A line with what it was about `below` its words, under them rather than under the badge.
fn line_over(
    glyph: Icon,
    ink: Option<Hsla>,
    words: Vec<AnyElement>,
    below: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_start()
        .gap(SPACE_2)
        .pl(px(SPINE - BADGE / 2.))
        .min_w_0()
        .text_size(crate::theme::ui_text(TEXT))
        .text_color(t.text_secondary)
        .child(
            div()
                .size(px(BADGE))
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
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .child(
                    div()
                        .min_h(px(BADGE))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(5.))
                        .min_w_0()
                        .children(words),
                )
                .children(below),
        )
        .into_any_element()
}

/// How a mentioning issue or pull request stands: its mark, colour and word.
pub(super) fn mention_state(source: &Mention, t: &Tokens) -> (Icon, Hsla, &'static str) {
    let glyph = match (source.pull, source.merged) {
        (true, true) => Icon::Merge,
        (true, false) => Icon::PullRequest,
        (false, _) => Icon::Issues,
    };
    match (source.merged, source.state.as_deref()) {
        (true, _) => (glyph, t.accent, "Merged"),
        (_, Some("closed")) => (glyph, t.danger, "Closed"),
        _ => (glyph, t.live, "Open"),
    }
}

/// The issue or pull request that mentioned this one: its title and where it is, with how
/// it stands, as GitHub shows a mention.
fn mentioned_by(subject: String, source: &Mention, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let (glyph, ink, word) = mention_state(source, t);
    let place = match &source.repo {
        Some(repo) => format!("{repo}#{}", source.number),
        None => format!("#{}", source.number),
    };
    div()
        .flex()
        .items_start()
        .gap(SPACE_2)
        .min_w_0()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    ui::text(subject, TEXT, FontWeight::SEMIBOLD)
                        .whitespace_normal()
                        .text_color(t.text_primary),
                )
                .child(dim(place, cx)),
        )
        .child(
            div()
                .flex_none()
                .h(px(22.))
                .px(SPACE_2)
                .flex()
                .items_center()
                .gap(SPACE_1)
                .rounded_full()
                .bg(ink.opacity(0.16))
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .text_color(ink)
                .child(ui::icon(glyph, 12.).text_color(ink))
                .child(word),
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
        .child(ui::text(login.to_owned(), TEXT, FontWeight::SEMIBOLD).text_color(t.text_primary))
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
        .px(SPACE_1H)
        .rounded(theme::RADIUS_1)
        .bg(t.surface_active)
        .text_color(t.text_primary)
        .child(words.into())
        .into_any_element()
}

fn event(item: &TimelineItem, base: Option<&str>, faces: Faces, cx: &App) -> Option<AnyElement> {
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
        "cross-referenced" => {
            let below = match &item.source {
                Some(source) => mentioned_by(subject, source, cx),
                None => chip(subject, cx),
            };
            let words = vec![actor, dim("mentioned this", cx), when];
            return Some(line_over(Icon::Link, None, words, Some(below), cx));
        }
        _ => return None,
    };
    words.push(when);
    Some(line(glyph, ink, words, cx))
}

/// Commits pushed together: who pushed them and when, then each by its message on the
/// thread's line, as GitHub lists them.
fn pushed(
    index: usize,
    commits: &[TimelineItem],
    pushers: &Pushers,
    faces: Faces,
    cx: &App,
) -> Vec<Item> {
    let t = Tokens::get(cx).clone();
    let pusher = |commit: &TimelineItem| {
        let named = commit
            .sha
            .as_ref()
            .and_then(|sha| pushers.get(sha))
            .cloned();
        named.or_else(|| {
            Some(Pusher {
                login: commit.actor.clone()?,
                picture: commit.avatar_url.clone(),
            })
        })
    };
    let mut authors: Vec<Pusher> = Vec::new();
    for author in commits.iter().filter_map(pusher) {
        if !authors.iter().any(|known| known.login == author.login) {
            authors.push(author);
        }
    }
    let count = commits.len();
    let first = authors.first();
    let mut words = vec![who(
        first.map(|author| author.login.as_str()),
        first.and_then(|author| author.picture.as_deref()),
        faces,
        cx,
    )];
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
    let glyph_top = (COMMIT_ROW - COMMIT_GLYPH) / 2.;
    let head = Item::new(line(Icon::Push, None, words, cx), GAP, 0., Some(BADGE));
    std::iter::once(head)
        .chain(commits.iter().enumerate().map(|(at, commit)| {
            let message = commit.message.as_deref().unwrap_or_default();
            let first = message.lines().next().unwrap_or_default().to_owned();
            let sha = commit.sha.clone().unwrap_or_default();
            let short: String = sha.chars().take(7).collect();
            let author = pusher(commit);
            let row = div()
                .h(px(COMMIT_ROW))
                .flex()
                .items_center()
                .gap(SPACE_2)
                .min_w_0()
                .text_size(crate::theme::ui_text(TEXT))
                .pl(px(SPINE - COMMIT_GLYPH / 2.))
                .child(
                    ui::icon(Icon::Commit, COMMIT_GLYPH)
                        .flex_none()
                        .text_color(t.text_tertiary),
                )
                .child(faces(
                    author.as_ref().map(|author| author.login.as_str()),
                    author.as_ref().and_then(|author| author.picture.as_deref()),
                    16.,
                    cx,
                ))
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
                            .text_size(crate::theme::ui_text(TEXT - 1.))
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
                });
            let space = if at == 0 { 8. } else { 4. };
            Item::new(
                row.into_any_element(),
                space,
                glyph_top,
                Some(glyph_top + COMMIT_GLYPH),
            )
        }))
        .collect()
}
