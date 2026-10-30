//! One pull request opened from the list or a link: its head and actions over tabs for its
//! conversation, its changed files' diffs, its commits, and its checks and merge.

use std::collections::HashSet;

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Div, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use serde::Deserialize;
use serde_json::json;
use sikemux_core::git::diff::DiffRow;
use sikemux_plugin_api::codehost::{
    ChangedFile, Check, CheckTally, MergeReadiness, OpenPull, Outcome, PullCommit, PullState,
    ReviewVerdict,
};

use super::diffs::{Changed, Diffs};
use super::thread::{self, Block, Post, Pushers};
use super::{branch_chip, outcome_word, pull_mark, tally_summary, HostArea, Read};
use crate::git::pane::ROW_HEIGHT;
use crate::git::Diff;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H,
    SPACE_4, TEXT_2XS, TEXT_2XS_H, TEXT_3XS_H, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui::time::ago_written as ago;
use crate::ui::{self, Button, IconButton, Variant};
use gpui_kit::component::input::{Textarea, TextareaState};

pub const VIEW: &str = "host-pull";
pub const FILES: &str = "host-pull-files";

/// What the view shows, picked from its one row of tabs.
#[derive(Clone, Copy, PartialEq)]
pub enum Section {
    Conversation,
    Files,
    Commits,
    Checks,
}

/// Below this width the Files tab has no room for the file list beside the diffs.
const NARROW_BELOW: f32 = 760.;
/// The widest the head, the conversation, the commits and the checks grow, centred in a
/// wider pane so their lines stay readable.
const READABLE: f32 = 860.;
/// How wide the file list is beside the diffs.
const FILE_LIST: f32 = 260.;

/// What merging by `method` is called: on the button that does it, and in the menu that
/// picks it.
fn method_words(method: &str) -> (&str, &str) {
    match method {
        "squash" => ("Squash and merge", "Squash and merge"),
        "merge" => ("Merge pull request", "Create a merge commit"),
        "rebase" => ("Rebase and merge", "Rebase and merge"),
        other => (other, other),
    }
}

/// A changed file as the core answers it: the file, and its patch as a diff's rows.
#[derive(Deserialize)]
struct Pulled {
    #[serde(flatten)]
    file: ChangedFile,
    rows: Vec<DiffRow>,
}

pub struct Opened {
    pub number: u64,
    pub view: Read<OpenPull>,
    pub files: Read<Diffs>,
    pub commits: Option<Vec<PullCommit>>,
    pub thread: Read<Vec<Block>>,
    /// The description, kept as markdown while it reads the same.
    opening: Option<(String, Post)>,
    pub section: Section,
    /// Too narrow for the file list beside the diffs, as last drawn.
    narrow: bool,
    /// The file whose diff was last asked for.
    focused: Option<String>,
    /// The file list is folded to its head.
    list_folded: bool,
    /// The runs in the checks folded or unfolded against how each starts: open while one
    /// of its jobs fails or is still going, shut otherwise.
    turned_runs: HashSet<String>,
    pub method: usize,
    pub busy: bool,
    /// The comment being written, made once the conversation first shows.
    draft: Option<Entity<TextareaState>>,
    /// The review or comment on its way, while one is.
    sending: Option<&'static str>,
}

impl Opened {
    pub fn new(number: u64) -> Self {
        Self {
            number,
            view: None,
            files: None,
            commits: None,
            thread: None,
            opening: None,
            section: Section::Conversation,
            narrow: false,
            focused: None,
            list_folded: false,
            turned_runs: HashSet::new(),
            method: 0,
            busy: false,
            draft: None,
            sending: None,
        }
    }

    /// Takes the pull request as the core read it again, saying whether its head or its
    /// conversation moved since the last reading.
    pub fn take_view(&mut self, view: Read<OpenPull>) -> bool {
        let mark = |view: &Read<OpenPull>| {
            let pull = &view.as_ref()?.as_ref().ok()?.pull;
            Some((pull.head_sha.clone(), pull.updated_at.clone()))
        };
        let moved = matches!((mark(&self.view), mark(&view)), (Some(was), Some(now)) if was != now);
        if let Some(Ok(view)) = &view {
            let pull = &view.pull;
            if self.opening.as_ref().map(|(body, _)| body) != Some(&pull.body) {
                let post = Post::new(
                    pull.author.clone(),
                    pull.author_association.clone(),
                    Some(pull.created_at.clone()),
                    None,
                    &pull.body,
                )
                .pictured(pull.avatar_url.clone());
                self.opening = Some((pull.body.clone(), post));
            }
        }
        self.view = view;
        moved
    }

    pub fn take_files(&mut self, files: Result<Vec<serde_json::Value>, SharedString>) {
        self.files = Some(files.map(|files| {
            Diffs::new(
                files
                    .into_iter()
                    .filter_map(|file| serde_json::from_value::<Pulled>(file).ok())
                    .map(|pulled| Changed::new(pulled.file, Some(Diff::new(pulled.rows))))
                    .collect(),
            )
        }));
    }

    /// Shows the diffs, scrolled to `path`'s.
    fn focus(&mut self, path: &str) {
        self.section = Section::Files;
        self.focused = Some(path.to_owned());
        if let Some(Ok(diffs)) = &mut self.files {
            diffs.reveal(path);
        }
    }
}

impl HostArea {
    /// Opens pull request `number`, asking the core for it and the host for its commits
    /// and conversation.
    pub(super) fn open_pull(&mut self, number: u64, cx: &mut Context<Self>) {
        self.pull = Some(Opened::new(number));
        self.watch();
        self.read_pull_extras(number, cx);
        cx.notify();
    }

    pub(super) fn read_pull_extras(&self, number: u64, cx: &mut Context<Self>) {
        let commits = self.plugin("pullCommits", json!({ "number": number }));
        let timeline = self.plugin("timeline", json!({ "number": number, "of": "pull" }));
        cx.spawn(async move |area, cx| {
            let (commits, timeline) = (commits.await, timeline.await);
            area.update(cx, |area, cx| {
                let Some(opened) = area.pull.as_mut().filter(|opened| opened.number == number)
                else {
                    return;
                };
                opened.commits = commits
                    .ok()
                    .and_then(|commits| serde_json::from_value(commits).ok());
                opened.thread = Some(timeline.and_then(|items| {
                    serde_json::from_value(items)
                        .map(thread::blocks)
                        .map_err(|error| error.to_string().into())
                }));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn merge(&mut self, cx: &mut Context<Self>) {
        let methods = &self.host.pulls.merge_methods;
        let Some((number, sha, method)) = self.pull.as_ref().and_then(|opened| {
            let view = opened.view.as_ref()?.as_ref().ok()?;
            let sha = view.pull.head_sha.clone().unwrap_or_default();
            Some((opened.number, sha, methods.get(opened.method)?.clone()))
        }) else {
            return;
        };
        let params = json!({ "number": number, "method": method, "sha": sha });
        self.write("mergePull", params, format!("Merged #{number}"), cx);
    }

    fn set_state(&mut self, open: bool, cx: &mut Context<Self>) {
        let Some(number) = self.pull.as_ref().map(|opened| opened.number) else {
            return;
        };
        let state = if open { "open" } else { "closed" };
        let said = if open {
            format!("Reopened #{number}")
        } else {
            format!("Closed #{number}")
        };
        self.write(
            "setPullState",
            json!({ "number": number, "state": state }),
            said,
            cx,
        );
    }

    /// Closes the pull request, saying what was written first. A host that cannot open it
    /// again asks before, and then closes it without the comment.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(opened) = &self.pull else {
            return;
        };
        if !self.host.pulls.reopen {
            let number = opened.number;
            let title = (opened.view.as_ref())
                .and_then(|view| view.as_ref().ok())
                .map(|view| view.pull.title.clone())
                .unwrap_or_default();
            let asking = super::Asking::new(
                format!("Close #{number} without merging?"),
                format!("{title}\n\nIt cannot be opened again."),
                "Close pull request",
                true,
                |area, cx| area.set_state(false, cx),
            );
            self.ask(asking, cx);
            return;
        }
        let written =
            (opened.draft.as_ref()).is_some_and(|draft| !draft.read(cx).value().trim().is_empty());
        if written {
            self.send(None, window, cx);
        }
        self.set_state(false, cx);
    }

    /// Checks the pull request out in the project, as the core does it.
    fn checkout(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = &mut self.pull else {
            return;
        };
        opened.busy = true;
        let number = opened.number;
        let params = json!({ "project": self.project, "number": number });
        let answer = self.core.request("codeHostCheckout", params);
        cx.spawn(async move |area, cx| {
            let answer = answer.await;
            area.update(cx, |area, cx| {
                if let Some(opened) = &mut area.pull {
                    opened.busy = false;
                }
                let checked = answer
                    .map_err(SharedString::from)
                    .and_then(|value| super::answer::<serde_json::Value>(&value));
                let said = match checked {
                    Ok(_) => super::Said::Done(format!("Checked out #{number}").into()),
                    Err(error) => super::Said::Failed(
                        format!("Could not check out #{number}: {error}").into(),
                    ),
                };
                area.say(said, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Sends what was written as a comment, or with a review's verdict.
    fn send(&mut self, event: Option<&'static str>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(opened) = &mut self.pull else {
            return;
        };
        let Some(draft) = opened.draft.clone() else {
            return;
        };
        let body = draft.read(cx).value().trim().to_owned();
        let number = opened.number;
        opened.sending = Some(event.unwrap_or("comment"));
        let (method, more, done) = match event {
            None => (
                "addComment",
                json!({ "number": number, "body": body, "of": "pull" }),
                "Comment added".to_owned(),
            ),
            Some(event) => {
                let done = match event {
                    "APPROVE" => format!("Approved #{number}"),
                    "REQUEST_CHANGES" => format!("Asked for changes on #{number}"),
                    _ => format!("Reviewed #{number}"),
                };
                (
                    "reviewPull",
                    json!({ "number": number, "event": event, "body": body }),
                    done,
                )
            }
        };
        let answer = self.plugin(method, more);
        cx.spawn_in(window, async move |area, cx| {
            let answer = answer.await;
            area.update_in(cx, |area, window, cx| {
                if let Some(opened) = &mut area.pull {
                    opened.sending = None;
                }
                match answer {
                    Ok(_) => {
                        draft.update(cx, |draft, cx| draft.set_value("", window, cx));
                        area.read_pull_extras(number, cx);
                        area.say(super::Said::Done(done.into()), cx);
                    }
                    Err(error) => area.say(super::Said::Failed(error), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Asks the host to change the pull request, saying `done` once it has.
    fn write(
        &mut self,
        method: &'static str,
        more: serde_json::Value,
        done: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(opened) = &mut self.pull {
            opened.busy = true;
        }
        let answer = self.plugin(method, more);
        cx.spawn(async move |area, cx| {
            let answer = answer.await;
            area.update(cx, |area, cx| {
                if let Some(opened) = &mut area.pull {
                    opened.busy = false;
                }
                match answer {
                    Ok(_) => area.say(super::Said::Done(done.into()), cx),
                    Err(error) => area.say(super::Said::Failed(error), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

pub fn view(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    let Some(opened) = &area.pull else {
        return div().into_any_element();
    };
    if opened.section == Section::Files {
        if let Some(Some(Ok(diffs))) = area.pull.as_mut().map(|opened| opened.files.as_mut()) {
            diffs.colour(cx, |area| area.pull.as_mut()?.files.as_mut()?.as_mut().ok());
        }
    }
    let shown = page_view(area, window, cx);
    let entity = cx.entity().downgrade();
    div()
        .size_full()
        .on_children_prepainted(move |bounds, _, cx| {
            let Some(width) = bounds.first().map(|drawn| f32::from(drawn.size.width)) else {
                return;
            };
            entity
                .update(cx, |area, cx| {
                    if let Some(opened) = &mut area.pull {
                        if opened.narrow != (width < NARROW_BELOW) {
                            opened.narrow = !opened.narrow;
                            cx.notify();
                        }
                    }
                })
                .ok();
        })
        .child(shown)
        .into_any_element()
}

/// The pull request's head with its actions, one row of tabs, then the tab's page, in a
/// column centred in a wide pane.
fn page_view(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let opened = area.pull.as_ref().expect("a pull request is open");
    let found = match &opened.view {
        None => return super::notice("Loading pull request", None, cx),
        Some(Err(error)) => return super::notice("Could not read it", Some(error.clone()), cx),
        Some(Ok(found)) => found,
    };
    let pull = &found.pull;
    let said = opened
        .thread
        .as_ref()
        .and_then(|thread| thread.as_ref().ok())
        .map(|blocks| (thread::said(blocks) + 1) as u64);
    let count = |count: Option<u64>| count.map(|count| tally(count, cx));
    let checks_mark = found.checks.as_ref().map(|tally| {
        ui::run_mark(
            format!("pull-checks-{}", pull.number),
            outcome_word(tally.outcome),
            12.,
            cx,
        )
    });
    let sections = [
        (
            Section::Conversation,
            Icon::Message,
            "Conversation",
            count(said),
        ),
        (
            Section::Files,
            Icon::File,
            "Files",
            count(pull.changed_files),
        ),
        (
            Section::Commits,
            Icon::Commit,
            "Commits",
            count(pull.commits),
        ),
        (Section::Checks, Icon::Actions, "Checks", checks_mark),
    ];
    let chosen = opened.section;
    let top = div()
        .px(SPACE_4)
        .pt(SPACE_3H)
        .pb(SPACE_3)
        .child(header(area, found, cx));
    let folding = (chosen == Section::Files)
        .then(|| fold_all(opened, cx))
        .flatten();
    let tabs = ui::page_tabs(
        sections.into_iter().map(|(section, glyph, label, mark)| {
            ui::page_tab("pull-tab", glyph, label, mark, chosen == section, cx).on_click(
                cx.listener(move |area, _, _, cx| {
                    if let Some(opened) = &mut area.pull {
                        opened.section = section;
                    }
                    cx.notify();
                }),
            )
        }),
        folding,
    );
    let footer = merge_footer(area, found, cx);
    let page = |body: AnyElement| {
        div()
            .id("pull-page")
            .size_full()
            .overflow_y_scroll()
            .child(centred(body))
            .into_any_element()
    };
    let body = match chosen {
        Section::Conversation => conversation(area, window, cx),
        Section::Files if opened.narrow => diffs(area, window, cx),
        Section::Files => {
            let list = div()
                .id("pull-file-list")
                .w(px(FILE_LIST))
                .flex_none()
                .h_full()
                .border_r_1()
                .border_color(t.border)
                .bg(t.surface_composer)
                .overflow_y_scroll()
                .child(files_page(opened, cx));
            div()
                .size_full()
                .flex()
                .child(list)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(diffs(area, window, cx)),
                )
                .into_any_element()
        }
        Section::Commits => page(commits_page(opened, cx)),
        Section::Checks => page(merge_page(area, cx)),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(super::band(centred(div().child(top).child(tabs)), cx))
        .child(div().flex_1().min_h_0().child(body))
        .children(footer)
        .into_any_element()
}

fn centred(inner: impl IntoElement) -> Div {
    super::centred(READABLE, inner)
}

fn back(cx: &mut Context<HostArea>) -> IconButton {
    IconButton::new("pull-back", Icon::Chevron, "Back to the pull requests")
        .turned(std::f32::consts::PI)
        .small()
        .on_click(cx.listener(|area, _, _, cx| {
            area.pull = None;
            area.elsewhere = None;
            area.watch();
            cx.notify();
        }))
}

/// Checks the pull request out in the project, or says it is checked out already.
fn checkout_button(area: &HostArea, view: &OpenPull, cx: &mut Context<HostArea>) -> Button {
    let (label, here, can) = checkout_state(area, view);
    Button::new("pull-checkout", label, Variant::Outline)
        .icon(if here { Icon::Check } else { Icon::Checkout })
        .disabled(!can)
        .on_click(cx.listener(|area, _, _, cx| area.checkout(cx)))
}

/// What checking the pull request out says, whether it is checked out here already, and
/// whether it can be now.
fn checkout_state(area: &HostArea, view: &OpenPull) -> (&'static str, bool, bool) {
    let busy = area.pull.as_ref().is_some_and(|opened| opened.busy);
    let checkout = view.checkout.clone().filter(|_| area.elsewhere.is_none());
    let current = area.pulls.branch.as_ref().map(|(branch, _)| branch.clone());
    let here = checkout.is_some() && checkout == current;
    let label = if here {
        "Checked out"
    } else if busy {
        "Checking out…"
    } else {
        "Check out"
    };
    (label, here, !here && checkout.is_some() && !busy)
}

/// Merges by the method picked, as GitHub's split button does: the method's name merges,
/// asking first, and the chevron beside it picks another method.
/// `compact` says only Merge, for a narrow pane, and lists Check out in its menu too.
fn merge_button(
    area: &HostArea,
    view: &OpenPull,
    compact: bool,
    cx: &mut Context<HostArea>,
) -> AnyElement {
    let blocked = !mergeable(view);
    let number = view.pull.number;
    let title = view.pull.title.clone();
    let base = view
        .pull
        .base
        .clone()
        .unwrap_or_else(|| "the base branch".into());
    let opened = area.pull.as_ref().expect("a pull request is open");
    let methods = area.host.pulls.merge_methods.clone();
    let chosen = opened.method;
    let says = match compact {
        true => "Merge",
        false => methods
            .get(chosen)
            .map_or("Merge pull request", |method| method_words(method).0),
    }
    .to_owned();
    let (checking_out, _, can_check_out) = checkout_state(area, view);
    let off = opened.busy || blocked;
    let view = cx.entity().downgrade();
    let picked = view.clone();
    let menu: Option<ui::SplitMenu> = (methods.len() > 1 || compact).then(|| {
        std::rc::Rc::new(
            move |mut popup: PopupMenu, _: &mut Window, cx: &mut Context<PopupMenu>| {
                for (index, method) in methods.iter().enumerate() {
                    let picked = picked.clone();
                    popup = popup.item(
                        ui::MenuRow::new(method_words(method).1.to_owned())
                            .item(cx)
                            .checked(index == chosen)
                            .on_click(move |_, _, cx| {
                                picked
                                    .update(cx, |area, cx| {
                                        if let Some(opened) = &mut area.pull {
                                            opened.method = index;
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                            }),
                    );
                }
                if compact {
                    let picked = picked.clone();
                    popup = popup.separator().item(
                        ui::MenuRow::new(checking_out)
                            .icon(Icon::Checkout)
                            .item(cx)
                            .disabled(!can_check_out)
                            .on_click(move |_, _, cx| {
                                picked.update(cx, |area, cx| area.checkout(cx)).ok();
                            }),
                    );
                }
                popup
            },
        ) as ui::SplitMenu
    });
    ui::split_button(
        "pull-merge",
        says,
        Some(Icon::Merge),
        off,
        true,
        move |_, _, cx| {
            let (title, base) = (title.clone(), base.clone());
            view.update(cx, |area, cx| {
                area.ask(
                    super::Asking::new(
                        format!("Merge #{number} into {base}?"),
                        title,
                        "Merge",
                        false,
                        |area, cx| area.merge(cx),
                    ),
                    cx,
                );
            })
            .ok();
        },
        menu,
        cx,
    )
}

/// What the reviews come to so far.
fn reviews(view: &OpenPull) -> (Outcome, &'static str) {
    match view.verdict {
        Some(ReviewVerdict::Approved) => (Outcome::Success, "Approved"),
        Some(ReviewVerdict::ChangesRequested) => (Outcome::Failure, "Changes requested"),
        None if view.reviewed => (Outcome::Queued, "No approving review yet"),
        None => (Outcome::Queued, "No reviews yet"),
    }
}

/// Whether it can merge as the host says, and what to do about it when not; none from a
/// host that does not say.
fn readiness(view: &OpenPull, area: &HostArea) -> Option<(Outcome, String, Option<String>)> {
    let host = area.host.label.as_str();
    let base = view.pull.base.as_deref().unwrap_or("the base branch");
    if view.pull.draft {
        return Some((
            Outcome::Queued,
            "This is a draft".to_owned(),
            Some(format!(
                "Mark it ready for review on {host} before merging."
            )),
        ));
    }
    if !area.host.pulls.mergeability {
        return None;
    }
    Some(match view.merge {
        MergeReadiness::Conflicts => (
            Outcome::Failure,
            format!("Conflicts with {base}"),
            Some("Resolve the conflicts before merging.".to_owned()),
        ),
        MergeReadiness::Blocked => (
            Outcome::Blocked,
            "Merging is blocked".to_owned(),
            Some(format!(
                "{host} is waiting on the required reviews and checks."
            )),
        ),
        MergeReadiness::Behind => (
            Outcome::Blocked,
            format!("Behind {base}"),
            Some("Bring the branch up to date before merging.".to_owned()),
        ),
        MergeReadiness::Ready => (Outcome::Success, "Ready to merge".to_owned(), None),
        MergeReadiness::Checking => (
            Outcome::Queued,
            "Checking whether it can merge".to_owned(),
            None,
        ),
    })
}

/// What stands between an open pull request and its merge, on a footer pinned under its
/// page as the chat's composer is: its reviews, its checks and whether it can merge, then
/// Check out and Merge. A narrow pane says each in a word and folds Check out into Merge.
fn merge_footer(area: &HostArea, view: &OpenPull, cx: &mut Context<HostArea>) -> Option<Div> {
    if view.pull.state != "open" {
        return None;
    }
    let narrow = area.pull.as_ref().is_some_and(|opened| opened.narrow);
    let t = Tokens::get(cx).clone();
    let (outcome, said) = reviews(view);
    let short = match view.verdict {
        Some(ReviewVerdict::Approved) => "Approved",
        Some(ReviewVerdict::ChangesRequested) => "Changes requested",
        None if view.reviewed => "Not approved",
        None => "No reviews",
    };
    let mut statuses = vec![(outcome, said.to_owned(), short.to_owned())];
    if let Some(tally) = &view.checks {
        let ran = tally.total - tally.skipped;
        let passed = tally.passed - tally.skipped;
        statuses.push((
            tally.outcome,
            checks_said(tally),
            format!("{passed}/{ran} checks"),
        ));
    }
    if let Some((outcome, title, _)) = readiness(view, area) {
        let short = match view.merge {
            _ if view.pull.draft => "Draft",
            MergeReadiness::Conflicts => "Conflicts",
            MergeReadiness::Blocked => "Blocked",
            MergeReadiness::Behind => "Behind",
            MergeReadiness::Ready => "Ready",
            MergeReadiness::Checking => "Checking",
        };
        statuses.push((outcome, title, short.to_owned()));
    }
    let statuses = statuses
        .into_iter()
        .enumerate()
        .map(|(at, (outcome, long, short))| {
            let stops = matches!(outcome, Outcome::Failure | Outcome::Blocked);
            div()
                .id(("pull-status", at))
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .whitespace_nowrap()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(if stops { t.danger } else { t.text_primary })
                .cursor_pointer()
                .on_click(cx.listener(|area, _, _, cx| {
                    if let Some(opened) = &mut area.pull {
                        opened.section = Section::Checks;
                    }
                    cx.notify();
                }))
                .child(ui::status_of_run(outcome_word(outcome), 13., cx))
                .child(if narrow { short } else { long })
        })
        .collect::<Vec<_>>();
    let row = div()
        .px(SPACE_4)
        .py(SPACE_2H)
        .flex()
        .items_center()
        .gap(px(if narrow { 12. } else { 16. }))
        .children(statuses)
        .child(
            div()
                .ml_auto()
                .flex()
                .gap(SPACE_1H)
                .when(!narrow, |acts| acts.child(checkout_button(area, view, cx)))
                .child(merge_button(area, view, narrow, cx)),
        );
    Some(
        div()
            .flex_none()
            .when(!crate::see_through::clear(cx), |foot| foot.bg(t.surface))
            .border_t_1()
            .border_color(t.border)
            .child(centred(row)),
    )
}

/// Whether the host would take a merge now: not while it conflicts, its rules block it or
/// it is still working that out.
fn mergeable(view: &OpenPull) -> bool {
    matches!(view.merge, MergeReadiness::Ready | MergeReadiness::Behind)
}

/// The pull request's head: how it stands and its number, with its actions and the way to
/// it on the host, then its title, then who wants to merge what into what.
fn header(area: &HostArea, found: &OpenPull, cx: &mut Context<HostArea>) -> Div {
    let t = Tokens::get(cx).clone();
    let host = area.host.label.as_str();
    let narrow = area.pull.as_ref().is_some_and(|opened| opened.narrow);
    let pull = &found.pull;
    let author = pull.author.clone().unwrap_or_else(|| "someone".into());
    let url = pull.url.clone();
    let dot = || div().text_color(t.text_tertiary).child("·");
    let branches = || {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .children(
                pull.base
                    .as_ref()
                    .map(|base| branch_chip(base, t.text_primary, cx)),
            )
            .child("←")
            .children(
                pull.head
                    .as_ref()
                    .map(|head| branch_chip(head, t.text_primary, cx)),
            )
    };
    let line = || {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_tertiary)
    };
    let who = div()
        .text_color(t.text_primary)
        .font_weight(FontWeight::MEDIUM)
        .child(author.clone());
    let doing = match found.state {
        PullState::Merged => "merged into",
        _ => "wants to merge into",
    };
    let meta = line()
        .child(state_pill(found.state, cx))
        .child(ui::mono(format!("#{}", pull.number), 11.5, cx).text_color(t.text_tertiary))
        .child(dot())
        .child(who)
        .map(|meta| match narrow {
            true => meta.child(dot()),
            false => meta.child(doing),
        })
        .child(branches())
        .child(dot())
        .child(ago(&pull.updated_at))
        .children(pull.labels.iter().map(|label| super::label_chip(label, cx)));
    let back = area.desk.is_none().then(|| {
        let at = div().h(px(AVATAR)).flex().items_center().child(back(cx));
        match narrow {
            true => at,
            false => at.absolute().top_0().left(px(-30.)),
        }
    });
    div().flex().flex_col().gap(SPACE_3).child(
        div()
            .relative()
            .flex()
            .items_start()
            .gap(px(AVATAR_GAP))
            .children(back)
            .child(area.face(Some(&author), pull.avatar_url.as_deref(), AVATAR, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .child(
                        ui::text(pull.title.clone(), 16., FontWeight::SEMIBOLD)
                            .whitespace_normal()
                            .text_color(t.text_primary),
                    )
                    .child(meta),
            )
            .child(
                IconButton::new("pull-open-host", Icon::Globe, format!("Open on {host}"))
                    .small()
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            ),
    )
}

/// The author's face beside the title, and the gap between them.
const AVATAR: f32 = 36.;
const AVATAR_GAP: f32 = 12.;

/// How the pull request stands, as a pill in its colour.
fn state_pill(state: PullState, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let (glyph, ink, word) = match state {
        PullState::Open => (Icon::PullRequest, t.live, "Open"),
        PullState::Draft => (Icon::PullRequest, t.text_tertiary, "Draft"),
        PullState::Closed => (Icon::PullRequest, t.danger, "Closed"),
        PullState::Merged => (Icon::Merge, t.accent, "Merged"),
    };
    div()
        .flex_none()
        .h(px(20.))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_1)
        .rounded_full()
        .bg(ink.opacity(0.16))
        .text_size(crate::theme::ui_text(TEXT_2XS_H))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ink)
        .child(ui::icon(glyph, 12.).text_color(ink))
        .child(word)
}

/// Folds every changed file's diff to its header, or unfolds them all, while they show.
fn fold_all(opened: &Opened, cx: &mut Context<HostArea>) -> Option<AnyElement> {
    let Some(Ok(diffs)) = &opened.files else {
        return None;
    };
    let folded = diffs.all_folded();
    let (glyph, label) = match folded {
        true => (Icon::ExpandAll, "Expand all diffs"),
        false => (Icon::CollapseAll, "Collapse all diffs"),
    };
    Some(
        IconButton::new("pull-fold-all", glyph, label)
            .small()
            .on_click(cx.listener(move |area, _, _, cx| {
                if let Some(Some(Ok(diffs))) =
                    area.pull.as_mut().map(|opened| opened.files.as_mut())
                {
                    diffs.fold_all(!folded);
                }
                cx.notify();
            }))
            .into_any_element(),
    )
}

/// A tab's count, quieter than its name.
fn tally(count: u64, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .text_size(crate::theme::ui_text(TEXT_2XS_H))
        .font_features(theme::tabular())
        .text_color(t.text_tertiary)
        .child(count.to_string())
        .into_any_element()
}

/// The changed files as the Git tab lists its changes: one group that folds, its files
/// hanging from it by the tree's line, each by its name with its folder after.
fn files_page(opened: &Opened, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let files = match &opened.files {
        None => return super::notice("Loading files", None, cx),
        Some(Err(error)) => {
            return super::notice("Could not read the files", Some(error.clone()), cx)
        }
        Some(Ok(diffs)) => &diffs.files,
    };
    let added: u64 = files.iter().map(|changed| changed.file.additions).sum();
    let removed: u64 = files.iter().map(|changed| changed.file.deletions).sum();
    let folded = opened.list_folded;
    let group = div()
        .id("pull-files-group")
        .h(px(ROW_HEIGHT))
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_1)
        .rounded(RADIUS_2)
        .cursor_pointer()
        .hover(move |row| row.bg(t.surface_hover))
        .on_click(cx.listener(|area, _, _, cx| {
            if let Some(opened) = &mut area.pull {
                opened.list_folded = !opened.list_folded;
            }
            cx.notify();
        }))
        .child(crate::motion::turn(
            "pull-files-group:chevron",
            crate::motion::fold_angle(!folded),
            crate::motion::LIST_FOLD,
            ui::icon(Icon::Chevron, 13.).text_color(t.text_tertiary),
        ))
        .child(ui::icon(Icon::File, 13.).text_color(t.text_tertiary))
        .child(ui::text("Changed files", 12., FontWeight::MEDIUM).text_color(t.text_primary))
        .child(ui::meta(files.len().to_string(), cx).ml(SPACE_0))
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .gap(SPACE_1H)
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .font_features(theme::tabular())
                .child(div().text_color(t.live).child(format!("+{added}")))
                .child(div().text_color(t.danger).child(format!("−{removed}"))),
        );
    let last = files.len().saturating_sub(1);
    let rows = files.iter().enumerate().map(|(at, changed)| {
        let chosen = opened.focused.as_deref() == Some(changed.file.path.as_str());
        file_row(at, changed, at == 0, at == last, chosen, cx)
    });
    div()
        .p(SPACE_1H)
        .flex()
        .flex_col()
        .child(group)
        .when(!folded, |list| list.children(rows.collect::<Vec<_>>()))
        .into_any_element()
}

/// A changed file in the list: its icon, name and folder, with how it changed.
fn file_row(
    at: usize,
    changed: &Changed,
    first: bool,
    last: bool,
    chosen: bool,
    cx: &mut Context<HostArea>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let path = changed.file.path.clone();
    let (dir, name) = match path.rsplit_once('/') {
        Some((dir, name)) => (Some(dir.to_owned()), name.to_owned()),
        None => (None, path.clone()),
    };
    let (letter, ink) = match changed.file.status.as_str() {
        "added" => ("A", t.git_added),
        "removed" => ("D", t.git_deleted),
        "renamed" | "copied" => ("R", t.git_renamed),
        _ => ("M", t.git_modified),
    };
    div()
        .id(("pull-file", at))
        .relative()
        .w_full()
        .h(px(ROW_HEIGHT))
        .pl(px(18.))
        .pr(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_SM))
        .text_color(if chosen {
            t.text_primary
        } else {
            t.text_secondary
        })
        .cursor_pointer()
        .when(chosen, |row| row.bg(t.surface_chosen))
        .when(!chosen, |row| {
            row.hover(move |row| row.bg(t.surface_hover).text_color(t.text_primary))
        })
        .tooltip(ui::tooltip(path.clone()))
        .on_click(cx.listener(move |area, _, _, cx| {
            if let Some(opened) = &mut area.pull {
                opened.focus(&path);
            }
            cx.notify();
        }))
        .children(crate::git::pane::tree_lines(first, last, &t))
        .child(crate::file_icons::glyph(&name, 14., cx))
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(div().flex_none().child(name))
                .children(dir.map(|dir| {
                    div()
                        .ml(SPACE_1H)
                        .min_w_0()
                        .truncate()
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(dir)
                })),
        )
        .child(
            div()
                .flex_none()
                .w(px(14.))
                .text_center()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink)
                .child(letter),
        )
        .into_any_element()
}

fn commits_page(opened: &Opened, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(commits) = &opened.commits else {
        return super::notice("Loading commits", None, cx);
    };
    div()
        .flex()
        .flex_col()
        .children(commits.iter().rev().map(|commit| {
            let author = commit.author.clone().unwrap_or_else(|| "someone".into());
            let subject = commit.message.lines().next().unwrap_or_default().to_owned();
            let short: String = commit.sha.chars().take(7).collect();
            div()
                .px(SPACE_3)
                .py(SPACE_1H)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .child(ui::initials(&author, "", 18., cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().text_color(t.text_primary).child(subject))
                        .child(
                            div()
                                .text_size(crate::theme::ui_text(TEXT_3XS_H))
                                .text_color(t.text_tertiary)
                                .child(format!(
                                    "{author} · {}",
                                    commit.date.as_deref().map(ago).unwrap_or_default()
                                )),
                        ),
                )
                .child(ui::mono(short, 11., cx).text_color(t.text_tertiary))
        }))
        .into_any_element()
}

/// One line of the merge box: a mark, what it says, and why.
fn part(outcome: Outcome, title: impl Into<SharedString>, detail: Option<String>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let title: SharedString = title.into();
    div()
        .px(SPACE_3)
        .py(px(9.))
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(ui::run_mark(
                    format!("merge-part-{title}"),
                    outcome_word(outcome),
                    14.,
                    cx,
                ))
                .child(ui::text(title, 12., FontWeight::MEDIUM).text_color(t.text_primary)),
        )
        .children(detail.map(|detail| ui::meta(detail, cx).pl(px(22.))))
}

fn merge_page(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let opened = area.pull.as_ref().expect("a pull request is open");
    let view = match &opened.view {
        None => return super::notice("Loading pull request", None, cx),
        Some(Err(error)) => return super::notice("Could not read it", Some(error.clone()), cx),
        Some(Ok(view)) => view,
    };
    let pull = &view.pull;
    let base = pull
        .base
        .clone()
        .unwrap_or_else(|| "the base branch".into());
    let checks = checks(area, view, cx);
    let card = div()
        .m(SPACE_2H)
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .bg(t.surface_composer)
        .overflow_hidden();
    if pull.state == "merged" {
        let by = pull.merged_by.clone().unwrap_or_default();
        let when = pull.merged_at.as_deref().map(ago).unwrap_or_default();
        let sha = pull.merge_commit_sha.clone();
        return card
            .child(
                div()
                    .px(SPACE_3)
                    .py(px(9.))
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .child(pull_mark(view.state, 14., cx))
                            .child(
                                ui::text("Merged into", 12., FontWeight::MEDIUM)
                                    .text_color(t.text_primary),
                            )
                            .child(branch_chip(&base, t.text_primary, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .text_size(crate::theme::ui_text(TEXT_2XS))
                            .text_color(t.text_tertiary)
                            .when(!by.is_empty(), |row| {
                                let url = pull.avatars.get(&by).map(String::as_str);
                                row.child(area.face(Some(&by), url, 16., cx))
                                    .child(by.clone())
                            })
                            .child(when)
                            .children(sha.map(|sha| {
                                let short: String = sha.chars().take(7).collect();
                                div()
                                    .id("merge-sha")
                                    .font_family(theme::MONO)
                                    .cursor_pointer()
                                    .hover(move |style| style.text_color(t.text_primary))
                                    .tooltip(ui::tooltip("Copy the merge commit"))
                                    .on_click(cx.listener(move |area, _, _, cx| {
                                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                            sha.clone(),
                                        ));
                                        let short: String = sha.chars().take(7).collect();
                                        area.say(
                                            super::Said::Done(format!("Copied {short}").into()),
                                            cx,
                                        );
                                    }))
                                    .child(short)
                            })),
                    ),
            )
            .children(checks)
            .into_any_element();
    }
    if pull.state != "open" {
        return card
            .child(
                div()
                    .px(SPACE_3)
                    .py(px(9.))
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .border_b_1()
                    .border_color(t.border)
                    .child(pull_mark(view.state, 14., cx))
                    .child(
                        ui::text("Closed without merging", 12., FontWeight::MEDIUM)
                            .text_color(t.text_primary),
                    ),
            )
            .children(checks)
            .into_any_element();
    }
    let (outcome, said) = reviews(view);
    let verdict = part(outcome, said, None, cx);
    let readiness = readiness(view, area).map(|(outcome, title, detail)| {
        let files = view.conflicts.len();
        let detail = match files {
            0 => detail,
            1 => Some("1 file conflicts.".to_owned()),
            files => Some(format!("{files} files conflict.")),
        };
        part(outcome, title, detail, cx).children(view.conflicts.iter().map(|path| {
            div()
                .pl(px(22.))
                .h(px(22.))
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(crate::file_icons::glyph(
                    path.rsplit('/').next().unwrap_or(path),
                    13.,
                    cx,
                ))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
                        .text_color(t.text_secondary)
                        .child(path.clone()),
                )
        }))
    });
    card.child(verdict)
        .children(checks)
        .children(readiness)
        .into_any_element()
}

/// How the checks on the head commit stand, in words: how many passed and were skipped
/// once all pass, else what holds them up.
fn checks_said(tally: &CheckTally) -> String {
    let passed = tally.passed - tally.skipped;
    if tally.passed == tally.total {
        return match tally.skipped {
            0 => format!("{passed} checks passed"),
            skipped => format!("{passed} passed, {skipped} skipped"),
        };
    }
    tally_summary(tally)
}

/// The checks on the pull request's head commit: each run with its jobs hanging from it,
/// then each check an app outside the host's CI posted. A host that lists no jobs shows its
/// runs alone.
fn checks(area: &HostArea, view: &OpenPull, cx: &mut Context<HostArea>) -> Option<Div> {
    if view.jobs.is_empty() {
        return run_rows(view, cx);
    }
    let tally = view.checks.as_ref()?;
    let title = match tally.passed == tally.total {
        true => "All checks passed".to_owned(),
        false => format!("Checks: {}", tally_summary(tally)),
    };
    let turned = &area.pull.as_ref()?.turned_runs;
    let mut rows: Vec<AnyElement> = Vec::new();
    for listed in &view.runs {
        let run = &listed.run;
        let jobs: Vec<&Check> = (view.jobs.iter())
            .filter(|job| job.run_id.as_deref() == Some(run.id.as_str()))
            .collect();
        if jobs.is_empty() {
            continue;
        }
        let stuck = jobs.iter().any(|job| {
            matches!(
                job.outcome,
                Outcome::Failure | Outcome::Running | Outcome::Queued | Outcome::Blocked
            )
        });
        let open = stuck != turned.contains(&run.id);
        let skipped = jobs
            .iter()
            .filter(|job| job.outcome == Outcome::Skipped)
            .count();
        let count = match (jobs.len(), skipped) {
            (1, _) => "1 job".to_owned(),
            (jobs, 0) => format!("{jobs} jobs"),
            (jobs, skipped) => format!("{jobs} jobs, {skipped} skipped"),
        };
        let took = super::elapsed(
            run.started_at.as_deref().unwrap_or(&run.created_at),
            (!listed.unfinished).then_some(run.updated_at.as_str()),
        );
        let key = run.id.clone();
        rows.push(
            check_row(
                SharedString::from(format!("pull-run-{}", run.id)),
                run.outcome,
                Some(open),
                &run.name,
                Some(format!("#{} · {count}", run.run_number)),
                false,
                Some(took),
                cx,
            )
            .on_click(cx.listener(move |area, _, _, cx| {
                if let Some(opened) = &mut area.pull {
                    if !opened.turned_runs.remove(&key) {
                        opened.turned_runs.insert(key.clone());
                    }
                }
                cx.notify();
            }))
            .into_any_element(),
        );
        if open {
            let last = jobs.len() - 1;
            for (at, job) in jobs.into_iter().enumerate() {
                rows.push(job_row(view, job, at == 0, at == last, cx));
            }
        }
    }
    let grouped: HashSet<&str> = view
        .runs
        .iter()
        .map(|listed| listed.run.id.as_str())
        .collect();
    for (at, job) in view.jobs.iter().enumerate() {
        if job
            .run_id
            .as_deref()
            .is_some_and(|run| grouped.contains(run))
        {
            continue;
        }
        let took = job
            .started_at
            .as_deref()
            .map(|started| super::elapsed(started, job.finished_at.as_deref()));
        let url = job.url.clone();
        rows.push(
            check_row(
                SharedString::from(format!("pull-app-check-{at}")),
                job.outcome,
                None,
                &job.name,
                job.app.clone(),
                job.required,
                took,
                cx,
            )
            .when_some(url, |row, url| {
                row.on_click(move |_, _, cx| cx.open_url(&url))
            })
            .into_any_element(),
        );
    }
    Some(
        part(tally.outcome, title, None, cx)
            .child(
                div()
                    .pl(px(22.))
                    .mt(px(-2.))
                    .child(ui::meta(checks_said(tally), cx)),
            )
            .child(div().pt(SPACE_1H).flex().flex_col().children(rows)),
    )
}

/// A run, or an app's check, in the checks: its mark, a chevron while it holds jobs, its
/// name and what else it says, a Required badge, and how long it took.
#[allow(clippy::too_many_arguments)]
fn check_row(
    id: SharedString,
    outcome: Outcome,
    open: Option<bool>,
    name: &str,
    after: Option<String>,
    required: bool,
    took: Option<String>,
    cx: &mut Context<HostArea>,
) -> gpui_kit::Stateful<Div> {
    let t = Tokens::get(cx).clone();
    let chevron = div()
        .w(px(12.))
        .flex_none()
        .flex()
        .justify_center()
        .children(open.map(|open| {
            crate::motion::turn(
                format!("{id}:chevron"),
                crate::motion::fold_angle(open),
                crate::motion::LIST_FOLD,
                ui::icon(Icon::Chevron, 11.).text_color(t.text_tertiary),
            )
        }));
    div()
        .id(id.clone())
        .h(px(ROW_HEIGHT))
        .px(SPACE_1)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .cursor_pointer()
        .hover(move |row| row.bg(t.surface_hover))
        .child(chevron)
        .child(ui::run_mark(id.clone(), outcome_word(outcome), 14., cx))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .child(name.to_owned()),
        )
        .children(after.map(|after| {
            div()
                .min_w_0()
                .truncate()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .child(after)
        }))
        .child(div().flex_1())
        .when(required, |row| row.child(required_badge(cx)))
        .children(took.map(|took| took_cell(took, cx)))
}

/// One job of a run, hanging from it by the tree's line: its mark and name, a Required
/// badge, and how long it took or that it was skipped. It opens its run.
fn job_row(
    view: &OpenPull,
    job: &Check,
    first: bool,
    last: bool,
    cx: &mut Context<HostArea>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let skipped = job.outcome == Outcome::Skipped;
    let took = match skipped {
        true => Some("skipped".to_owned()),
        false => job
            .started_at
            .as_deref()
            .map(|started| super::elapsed(started, job.finished_at.as_deref())),
    };
    let (run, number) = (job.run_id.clone(), view.pull.number);
    div()
        .id(SharedString::from(format!(
            "pull-job-{}",
            job.job_id.as_deref().unwrap_or(&job.name)
        )))
        .relative()
        .h(px(ROW_HEIGHT))
        .pl(px(26.))
        .pr(SPACE_1)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .text_color(if skipped {
            t.text_tertiary
        } else {
            t.text_primary
        })
        .cursor_pointer()
        .hover(move |row| row.bg(t.surface_hover))
        .on_click(cx.listener(move |area, _, _, cx| {
            if let (Some(run), true) = (&run, area.elsewhere.is_none() && area.desk.is_none()) {
                area.open_run(run.clone(), Some(number), cx)
            }
        }))
        .children(
            crate::git::pane::tree_lines(first, last, &t)
                .into_iter()
                .map(|line| line.ml(SPACE_1)),
        )
        .child(ui::run_mark(
            format!("pull-job-mark-{}", job.name),
            outcome_word(job.outcome),
            13.,
            cx,
        ))
        .child(div().flex_1().min_w_0().truncate().child(job.name.clone()))
        .when(job.required, |row| row.child(required_badge(cx)))
        .children(took.map(|took| took_cell(took, cx)))
        .into_any_element()
}

fn required_badge(cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex_none()
        .px(SPACE_1H)
        .rounded_full()
        .border_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(TEXT_3XS_H))
        .text_color(t.text_tertiary)
        .child("Required")
}

fn took_cell(took: String, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex_none()
        .w(px(52.))
        .text_right()
        .text_size(crate::theme::ui_text(TEXT_XS))
        .font_features(theme::tabular())
        .text_color(t.text_tertiary)
        .child(took)
}

/// The runs on the head commit, one row each, from a host that does not list their jobs.
fn run_rows(view: &OpenPull, cx: &mut Context<HostArea>) -> Option<Div> {
    let t = Tokens::get(cx).clone();
    let tally = view.checks.as_ref().filter(|_| !view.runs.is_empty())?;
    let summary = tally_summary(tally);
    let title = if summary == "all passed" {
        "All checks passed".to_owned()
    } else {
        format!("Checks: {summary}")
    };
    Some(
        part(tally.outcome, title, None, cx).child(div().pt(SPACE_1).flex().flex_col().children(
            view.runs.iter().enumerate().map(|(index, listed)| {
                let run = &listed.run;
                let (id, number) = (run.id.clone(), view.pull.number);
                let took = super::elapsed(
                    run.started_at.as_deref().unwrap_or(&run.created_at),
                    (!listed.unfinished).then_some(run.updated_at.as_str()),
                );
                div()
                    .id(("pull-check", index))
                    .h(px(24.))
                    .pl(px(22.))
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
                    .rounded(RADIUS_2)
                    .cursor_pointer()
                    .hover(move |row| row.bg(t.surface_hover))
                    .on_click(cx.listener(move |area, _, _, cx| {
                        if area.elsewhere.is_none() && area.desk.is_none() {
                            area.open_run(id.clone(), Some(number), cx)
                        }
                    }))
                    .child(ui::run_mark(
                        format!("pull-check-{}", run.id),
                        outcome_word(run.outcome),
                        13.,
                        cx,
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(t.text_primary)
                            .child(format!("{} #{}", run.name, run.run_number)),
                    )
                    .child(
                        div()
                            .text_color(t.text_tertiary)
                            .child(super::event_label(&run.event)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_color(t.text_tertiary)
                            .child(super::outcome_label(run.outcome)),
                    )
                    .child(
                        div()
                            .w(px(48.))
                            .text_right()
                            .font_features(theme::tabular())
                            .text_color(t.text_tertiary)
                            .child(took),
                    )
            }),
        )),
    )
}

/// The description and everything said and done since, with the composer under them.
fn conversation(
    area: &mut HostArea,
    window: &mut Window,
    cx: &mut Context<HostArea>,
) -> AnyElement {
    let opened = area.pull.as_ref().expect("a pull request is open");
    let Some(Ok(found)) = &opened.view else {
        return div().into_any_element();
    };
    let pull = &found.pull;
    let base = pull.base.clone();
    let pushers: Pushers = opened
        .commits
        .as_deref()
        .map(thread::pushers)
        .unwrap_or_default();
    let opening = opened.opening.as_ref().map(|(_, post)| post);
    let blocks = match &opened.thread {
        Some(Ok(blocks)) => &blocks[..],
        _ => &[],
    };
    let avatars = &pull.avatars;
    let shown: &HostArea = area;
    let faces = |login: Option<&str>, url: Option<&str>, size: f32, cx: &App| match url {
        Some(_) => shown.face(login, url, size, cx),
        None => {
            let known = login
                .and_then(|login| avatars.get(login))
                .map(String::as_str);
            shown.face(login, known, size, cx)
        }
    };
    let mut column = div();
    column = column.child(thread::thread(
        opening,
        blocks,
        &pushers,
        base.as_deref(),
        &faces,
        &area.bench,
        window,
        cx,
    ));
    if let Some(Err(error)) = &opened.thread {
        column = column.child(super::notice(
            "Could not read the conversation",
            Some(error.clone()),
            cx,
        ));
    }
    let review = (pull.state == "open").then(|| {
        let mine = area
            .login()
            .is_some_and(|login| pull.author.as_deref() == Some(login));
        (mine, area.host.pulls.request_changes)
    });
    div()
        .id("pull-thread")
        .size_full()
        .overflow_y_scroll()
        .child(centred(column.child(composer(area, review, window, cx))))
        .into_any_element()
}

/// Every changed file's diff.
fn diffs(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    let view = cx.entity();
    let opened = area.pull.as_ref().expect("a pull request is open");
    let Some(Ok(found)) = &opened.view else {
        return div().into_any_element();
    };
    let pull = &found.pull;
    match &opened.files {
        None => super::notice("Loading files", None, cx),
        Some(Err(error)) => super::notice("Could not read the files", Some(error.clone()), cx),
        Some(Ok(diffs)) if diffs.files.is_empty() => super::notice("No files changed.", None, cx),
        Some(Ok(diffs)) => diffs.list(
            &view,
            |area| area.pull.as_ref()?.files.as_ref()?.as_ref().ok(),
            std::rc::Rc::new(|area| area.pull.as_mut()?.files.as_mut()?.as_mut().ok()),
            area.elsewhere.is_none().then_some(area.project.as_str()),
            Some(super::diffs::PullOf {
                number: pull.number,
                title: pull.title.clone(),
                url: pull.url.clone(),
                bench: area.bench.clone(),
            }),
            window,
        ),
    }
}

/// Where a comment or review is written, with what can be sent: `review` says whether
/// the pull request is the person's own, and whether the host takes asking for changes.
fn composer(
    area: &mut HostArea,
    review: Option<(bool, bool)>,
    window: &mut Window,
    cx: &mut Context<HostArea>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let host = area.host.label.clone();
    let Some(opened) = area.pull.as_mut() else {
        return div().into_any_element();
    };
    let placeholder = if review.is_some() {
        "Leave a comment or a review"
    } else {
        "Leave a comment"
    };
    let draft = opened
        .draft
        .get_or_insert_with(|| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 12)
                    .placeholder(placeholder)
            })
        })
        .clone();
    let written = !draft.read(cx).value().trim().is_empty();
    let sending = opened.sending;
    let busy = sending.is_some();
    let mut foot = div()
        .px(SPACE_2H)
        .py(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .border_t_1()
        .border_color(t.border);
    if let Some((true, _)) = review {
        foot = foot.child(ui::meta(
            format!("{host} does not let you approve your own pull request."),
            cx,
        ));
    }
    foot = foot.child(div().flex_1());
    if let Some((false, request_changes)) = review {
        if request_changes {
            foot = foot.child(
                Button::new("review-changes", "Request changes", Variant::Outline)
                    .disabled(busy || !written)
                    .on_click(cx.listener(|area, _, window, cx| {
                        area.send(Some("REQUEST_CHANGES"), window, cx)
                    })),
            );
        }
        let approving = if sending == Some("APPROVE") {
            "Approving…"
        } else {
            "Approve"
        };
        foot = foot.child(
            Button::new("review-approve", approving, Variant::Outline)
                .disabled(busy)
                .on_click(
                    cx.listener(|area, _, window, cx| area.send(Some("APPROVE"), window, cx)),
                ),
        );
    }
    let state = (opened.view.as_ref())
        .and_then(|view| view.as_ref().ok())
        .map(|view| view.pull.state.clone())
        .unwrap_or_default();
    let reopens = area.host.pulls.reopen;
    match state.as_str() {
        "open" => {
            let closing = if written && reopens {
                "Close with comment"
            } else {
                "Close pull request"
            };
            foot = foot.child(
                Button::new("pull-close", closing, Variant::Outline)
                    .icon(Icon::PullRequest)
                    .disabled(busy)
                    .on_click(cx.listener(|area, _, window, cx| area.close(window, cx))),
            );
        }
        "closed" if reopens => {
            foot = foot.child(
                Button::new("pull-reopen", "Reopen pull request", Variant::Outline)
                    .icon(Icon::PullRequest)
                    .disabled(busy)
                    .on_click(cx.listener(|area, _, _, cx| area.set_state(true, cx))),
            );
        }
        _ => {}
    }
    let commenting = if sending == Some("comment") {
        "Sending…"
    } else {
        "Comment"
    };
    foot = foot.child(
        Button::new("review-comment", commenting, Variant::Primary)
            .disabled(busy || !written)
            .on_click(cx.listener(|area, _, window, cx| area.send(None, window, cx))),
    );
    div()
        .px(SPACE_4)
        .pb(SPACE_4)
        .child(
            div()
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .bg(t.surface_composer)
                .child(
                    div()
                        .px(SPACE_3)
                        .py(SPACE_2)
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .child(
                            Textarea::new(&draft)
                                .appearance(false)
                                .text_size(crate::theme::ui_text(TEXT_SM)),
                        ),
                )
                .child(foot),
        )
        .into_any_element()
}
