//! One pull request opened from the list: its files, commits, checks and merge in the
//! side column, and its conversation or every changed file's diff beside them.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Div, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use serde::Deserialize;
use serde_json::json;
use sikemux_core::git::diff::DiffRow;
use sikemux_wire::codehost::{
    ChangedFile, MergeReadiness, OpenPull, Outcome, PullCommit, ReviewVerdict,
};

use super::diffs::{Changed, Diffs};
use super::thread::{self, Block, Post, Pushers};
use super::{ago, branch_chip, outcome_word, pull_mark, tally_summary, HostArea, Read};
use crate::git::Diff;
use crate::icons::Icon;
use crate::theme::{self, Tokens, RADIUS_2};
use crate::ui::{self, Button, IconButton, Segmented, Variant};
use gpui_kit::component::input::{Textarea, TextareaState};

pub const VIEW: &str = "host-pull";
pub const FILES: &str = "host-pull-files";

/// The side column's pages.
#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Files,
    Commits,
    Merge,
}

/// What the main column shows.
#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Conversation,
    Files,
}

fn method_label(method: &str) -> &str {
    match method {
        "squash" => "Squash",
        "merge" => "Merge commit",
        "rebase" => "Rebase",
        other => other,
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
    pub page: Page,
    pub tab: Tab,
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
            page: Page::Files,
            tab: Tab::Conversation,
            method: 0,
            busy: false,
            draft: None,
            sending: None,
        }
    }

    pub fn take_view(&mut self, view: Read<OpenPull>) {
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
        self.tab = Tab::Files;
        if let Some(Ok(diffs)) = &self.files {
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

    fn read_pull_extras(&self, number: u64, cx: &mut Context<Self>) {
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
    let t = Tokens::get(cx).clone();
    let left = side(area, cx);
    if opened.tab == Tab::Files {
        if let Some(Some(Ok(diffs))) = area.pull.as_mut().map(|opened| opened.files.as_mut()) {
            diffs.colour(cx, |area| area.pull.as_mut()?.files.as_mut()?.as_mut().ok());
        }
    }
    let right = main(area, window, cx);
    div()
        .size_full()
        .flex()
        .child(
            div()
                .w(px(320.))
                .flex_none()
                .h_full()
                .border_r_1()
                .border_color(t.border)
                .child(left),
        )
        .child(div().flex_1().min_w_0().h_full().child(right))
        .into_any_element()
}

fn side(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let opened = area.pull.as_ref().expect("a pull request is open");
    let host = super::host_label(&area.host.host);
    let view = opened.view.as_ref().and_then(|view| view.as_ref().ok());
    let state = view.map(|view| view.state);
    let merge_label = match view.map(|view| view.pull.state.as_str()) {
        Some("merged") => "Merged",
        Some("closed") => "Closed",
        _ => "Merge",
    };
    let url = view.map(|view| view.pull.url.clone());
    let checkout = view
        .and_then(|view| view.checkout.clone())
        .filter(|_| area.elsewhere.is_none());
    let current = area.pulls.branch.as_ref().map(|(branch, _)| branch.clone());
    let here = checkout.is_some() && checkout == current;
    let open = view.is_some_and(|view| view.pull.state == "open");
    let checkout_label = if here {
        "Checked out"
    } else if opened.busy {
        "Checking out…"
    } else {
        "Check out"
    };
    let head = div()
        .px(px(10.))
        .pt(px(10.))
        .pb(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            IconButton::new("pull-back", Icon::Chevron, "Back to the pull requests")
                .turned(std::f32::consts::PI)
                .small()
                .on_click(cx.listener(|area, _, _, cx| {
                    area.pull = None;
                    area.elsewhere = None;
                    area.watch();
                    cx.notify();
                })),
        )
        .children(state.map(|state| pull_mark(state, 14., cx)))
        .child(ui::mono(format!("#{}", opened.number), 12., cx).text_color(t.text_primary));
    let actions = div()
        .px(px(10.))
        .pb(px(10.))
        .flex()
        .gap(px(6.))
        .flex_wrap()
        .child(
            Button::new("pull-checkout", checkout_label, Variant::Outline)
                .icon(if here { Icon::Check } else { Icon::Checkout })
                .disabled(here || !open || checkout.is_none() || opened.busy)
                .on_click(cx.listener(|area, _, _, cx| area.checkout(cx))),
        )
        .child(
            Button::new("pull-on-host", host.to_owned(), Variant::Outline)
                .icon(Icon::Globe)
                .disabled(url.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(url) = &url {
                        cx.open_url(url);
                    }
                }),
        )
        .child(
            Button::new("pull-merge-page", merge_label, Variant::Outline)
                .icon(Icon::Merge)
                .on_click(cx.listener(|area, _, _, cx| {
                    if let Some(opened) = &mut area.pull {
                        opened.page = Page::Merge;
                    }
                    cx.notify();
                })),
        );
    let pages = [
        (Icon::File, "Files", Page::Files),
        (Icon::Commit, "Commits", Page::Commits),
        (Icon::Check, "Checks", Page::Merge),
    ];
    let chosen = pages
        .iter()
        .position(|(_, _, page)| *page == opened.page)
        .unwrap_or_default();
    let switch = div().px(px(10.)).pb(px(8.)).child(
        Segmented::new(
            "pull-pages",
            pages
                .iter()
                .map(|(glyph, label, _)| (*glyph, SharedString::from(*label)))
                .collect(),
            chosen,
        )
        .on_select(cx.listener(move |area, index: &usize, _, cx| {
            if let Some(opened) = &mut area.pull {
                opened.page = pages[*index].2;
            }
            cx.notify();
        })),
    );
    let body = match opened.page {
        Page::Files => files_page(opened, cx),
        Page::Commits => commits_page(opened, cx),
        Page::Merge => merge_page(area, cx),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(head)
        .child(actions)
        .child(switch)
        .child(
            div()
                .id("pull-side-page")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
        .into_any_element()
}

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
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(12.))
                .py(px(4.))
                .flex()
                .gap(px(8.))
                .text_size(crate::theme::ui_text(11.))
                .font_features(theme::tabular())
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .child(format!("{} files", files.len())),
                )
                .child(div().flex_1())
                .child(div().text_color(t.live).child(format!("+{added}")))
                .child(div().text_color(t.danger).child(format!("−{removed}"))),
        )
        .children(files.iter().enumerate().map(|(at, changed)| {
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
            let glyph = match crate::file_icons::icon(&name, cx) {
                Some(icon) => gpui_kit::img(icon)
                    .size(px(14.))
                    .flex_none()
                    .into_any_element(),
                None => ui::icon(Icon::File, 14.)
                    .text_color(t.text_tertiary)
                    .into_any_element(),
            };
            div()
                .id(("pull-file", at))
                .h(px(26.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(crate::theme::ui_text(12.))
                .cursor_pointer()
                .hover(move |row| row.bg(t.surface_hover))
                .tooltip(ui::tooltip(path.clone()))
                .on_click(cx.listener(move |area, _, _, cx| {
                    if let Some(opened) = &mut area.pull {
                        opened.focus(&path);
                    }
                    cx.notify();
                }))
                .child(glyph)
                .child(div().flex_none().text_color(t.text_primary).child(name))
                .children(dir.map(|dir| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::theme::ui_text(10.5))
                        .text_color(t.text_tertiary)
                        .child(dir)
                }))
                .child(
                    div()
                        .ml_auto()
                        .flex_none()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(11.))
                        .text_color(ink)
                        .child(letter),
                )
        }))
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
                .px(px(12.))
                .py(px(6.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(crate::theme::ui_text(12.))
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
                                .text_size(crate::theme::ui_text(10.5))
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
    div()
        .px(px(12.))
        .py(px(9.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(ui::status_of_run(outcome_word(outcome), 14., cx))
                .child(ui::text(title.into(), 12., FontWeight::MEDIUM).text_color(t.text_primary)),
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
    let host = super::host_label(&area.host.host);
    let base = pull
        .base
        .clone()
        .unwrap_or_else(|| "the base branch".into());
    let checks = checks(view, cx);
    let card = div()
        .m(px(10.))
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .overflow_hidden();
    if pull.state == "merged" {
        let by = pull.merged_by.clone().unwrap_or_default();
        let when = pull.merged_at.as_deref().map(ago).unwrap_or_default();
        let sha = pull.merge_commit_sha.clone();
        return card
            .child(
                div()
                    .px(px(12.))
                    .py(px(9.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
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
                            .gap(px(6.))
                            .text_size(crate::theme::ui_text(11.))
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
                    .px(px(12.))
                    .py(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(t.border)
                    .child(pull_mark(view.state, 14., cx))
                    .child(
                        ui::text("Closed without merging", 12., FontWeight::MEDIUM)
                            .text_color(t.text_primary),
                    )
                    .child(div().flex_1())
                    .when(area.host.pulls.reopen, |row| {
                        row.child(
                            Button::new("pull-reopen", "Reopen", Variant::Outline)
                                .disabled(opened.busy)
                                .on_click(cx.listener(|area, _, _, cx| area.set_state(true, cx))),
                        )
                    }),
            )
            .children(checks)
            .into_any_element();
    }
    let verdict = match view.verdict {
        Some(ReviewVerdict::Approved) => part(Outcome::Success, "Approved", None, cx),
        Some(ReviewVerdict::ChangesRequested) => {
            part(Outcome::Failure, "Changes requested", None, cx)
        }
        None if view.reviewed => part(Outcome::Queued, "No approving review yet", None, cx),
        None => part(Outcome::Queued, "No reviews yet", None, cx),
    };
    let readiness = if pull.draft {
        Some(part(
            Outcome::Queued,
            "This is a draft",
            Some(format!(
                "Mark it ready for review on {host} before merging."
            )),
            cx,
        ))
    } else if area.host.pulls.mergeability {
        let (outcome, title, detail) = match view.merge {
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
        };
        Some(part(outcome, title, detail, cx))
    } else {
        None
    };
    let dirty = pull.merge_state.as_deref() == Some("dirty");
    let number = pull.number;
    let title = pull.title.clone();
    let base_name = base.clone();
    let actions = div()
        .px(px(12.))
        .py(px(10.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .when(!pull.draft, |actions| {
            actions
                .child(
                    Segmented::words(
                        "merge-method",
                        area.host
                            .pulls
                            .merge_methods
                            .iter()
                            .map(|method| {
                                (SharedString::from(method_label(method).to_owned()), None)
                            })
                            .collect(),
                        opened.method,
                    )
                    .on_select(cx.listener(|area, index: &usize, _, cx| {
                        if let Some(opened) = &mut area.pull {
                            opened.method = *index;
                        }
                        cx.notify();
                    })),
                )
                .child(
                    Button::new("pull-merge", "Merge pull request", Variant::Primary)
                        .icon(Icon::Merge)
                        .disabled(opened.busy || dirty)
                        .on_click(cx.listener(move |area, _, _, cx| {
                            area.ask(
                                super::Asking::new(
                                    format!("Merge #{number} into {base_name}?"),
                                    title.clone(),
                                    "Merge",
                                    false,
                                    |area, cx| area.merge(cx),
                                ),
                                cx,
                            );
                        })),
                )
        })
        .child({
            let title = pull.title.clone();
            Button::new("pull-close", "Close", Variant::Outline)
                .disabled(opened.busy)
                .on_click(cx.listener(move |area, _, _, cx| {
                    area.ask(
                        super::Asking::new(
                            format!("Close #{number} without merging?"),
                            title.clone(),
                            "Close pull request",
                            true,
                            |area, cx| area.set_state(false, cx),
                        ),
                        cx,
                    );
                }))
        });
    card.child(verdict)
        .children(checks)
        .children(readiness)
        .child(actions)
        .into_any_element()
}

/// The runs on the pull request's head commit, one row each under how they stand.
fn checks(view: &OpenPull, cx: &mut Context<HostArea>) -> Option<Div> {
    let t = Tokens::get(cx).clone();
    let tally = view.checks.as_ref().filter(|_| !view.runs.is_empty())?;
    let summary = tally_summary(tally);
    let title = if summary == "all passed" {
        "All checks passed".to_owned()
    } else {
        format!("Checks: {summary}")
    };
    Some(
        part(tally.outcome, title, None, cx).child(div().pt(px(4.)).flex().flex_col().children(
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
                    .gap(px(8.))
                    .text_size(crate::theme::ui_text(11.5))
                    .rounded(RADIUS_2)
                    .cursor_pointer()
                    .hover(move |row| row.bg(t.surface_hover))
                    .on_click(cx.listener(move |area, _, _, cx| {
                        if area.elsewhere.is_none() {
                            area.open_run(id.clone(), Some(number), cx)
                        }
                    }))
                    .child(ui::status_of_run(outcome_word(run.outcome), 13., cx))
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

fn main(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let host = super::host_label(&area.host.host);
    let view = cx.entity();
    let opened = area.pull.as_ref().expect("a pull request is open");
    let found = match &opened.view {
        None => return super::notice("Loading pull request", None, cx),
        Some(Err(error)) => return super::notice("Could not read it", Some(error.clone()), cx),
        Some(Ok(found)) => found,
    };
    let pull = &found.pull;
    let author = pull.author.clone().unwrap_or_else(|| "someone".into());
    let url = pull.url.clone();
    let said = opened
        .thread
        .as_ref()
        .and_then(|thread| thread.as_ref().ok())
        .map(|blocks| thread::said(blocks) + 1);
    let tabs = [
        (
            Tab::Conversation,
            "Conversation",
            said.map(|said| said as u64),
        ),
        (Tab::Files, "Files changed", pull.changed_files),
    ];
    let headline = div()
        .px(px(16.))
        .pt(px(14.))
        .pb(px(10.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .gap(px(12.))
                .child(area.face(Some(&author), pull.avatar_url.as_deref(), 36., cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(pull_mark(found.state, 16., cx))
                                .child(
                                    ui::text(pull.title.clone(), 15., FontWeight::SEMIBOLD)
                                        .text_color(t.text_primary)
                                        .min_w_0()
                                        .truncate(),
                                )
                                .child(
                                    IconButton::new(
                                        "pull-open-host",
                                        Icon::Globe,
                                        format!("Open on {host}"),
                                    )
                                    .small()
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(crate::theme::ui_text(10.5))
                                .text_color(t.text_tertiary)
                                .child(format!(
                                    "{author} · #{}, updated {}",
                                    pull.number,
                                    ago(&pull.updated_at)
                                ))
                                .children(
                                    pull.head
                                        .as_ref()
                                        .map(|head| branch_chip(head, t.text_secondary, cx)),
                                )
                                .children(pull.base.as_ref().map(|base| {
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child("→")
                                        .child(branch_chip(base, t.text_secondary, cx))
                                }))
                                .children(
                                    pull.labels.iter().map(|label| super::label_chip(label, cx)),
                                ),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(6.))
                .children(tabs.into_iter().map(|(tab, label, count)| {
                    let on = opened.tab == tab;
                    div()
                        .id(SharedString::from(format!("pull-tab-{label}")))
                        .h(px(24.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded_full()
                        .border_1()
                        .border_color(if on { t.border_strong } else { t.border })
                        .when(on, |chip| chip.bg(t.surface_active))
                        .text_size(crate::theme::ui_text(11.5))
                        .text_color(if on { t.text_primary } else { t.text_secondary })
                        .cursor_pointer()
                        .on_click(cx.listener(move |area, _, _, cx| {
                            if let Some(opened) = &mut area.pull {
                                opened.tab = tab;
                            }
                            cx.notify();
                        }))
                        .child(label)
                        .children(count.map(|count| {
                            div()
                                .font_features(theme::tabular())
                                .text_color(t.text_tertiary)
                                .child(count.to_string())
                        }))
                })),
        );
    let body = match opened.tab {
        Tab::Conversation => {
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
            let mut column = div().id("pull-thread").size_full().overflow_y_scroll();
            column = column.child(thread::thread(
                opening,
                blocks,
                &pushers,
                base.as_deref(),
                &faces,
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
            column
                .child(composer(area, review, window, cx))
                .into_any_element()
        }
        Tab::Files => match &opened.files {
            None => super::notice("Loading files", None, cx),
            Some(Err(error)) => super::notice("Could not read the files", Some(error.clone()), cx),
            Some(Ok(diffs)) if diffs.files.is_empty() => {
                super::notice("No files changed.", None, cx)
            }
            Some(Ok(diffs)) => diffs.list(
                &view,
                |area| area.pull.as_ref()?.files.as_ref()?.as_ref().ok(),
                area.elsewhere.is_none().then_some(area.project.as_str()),
                Some(super::diffs::PullOf {
                    number: pull.number,
                    title: pull.title.clone(),
                    url: pull.url.clone(),
                    bench: area.bench.clone(),
                }),
                window,
            ),
        },
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(headline)
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
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
    let host = super::host_label(&area.host.host).to_owned();
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
        .px(px(10.))
        .py(px(8.))
        .flex()
        .items_center()
        .gap(px(6.))
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
        .px(px(16.))
        .pb(px(16.))
        .pl(px(56.))
        .child(
            div()
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .child(
                    div()
                        .px(px(12.))
                        .py(px(8.))
                        .text_size(crate::theme::ui_text(13.))
                        .child(
                            Textarea::new(&draft)
                                .appearance(false)
                                .text_size(crate::theme::ui_text(13.)),
                        ),
                )
                .child(foot),
        )
        .into_any_element()
}
