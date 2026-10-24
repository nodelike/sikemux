//! A new pull request, laid out like Changes: its branches, title and description with
//! the branch's files and commits on the left, and what it changes on the right.

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
};
use serde::Deserialize;
use serde_json::json;
use sikemux_core::git::diff::DiffRow;
use sikemux_core::git::log::GitCommit;
use sikemux_wire::codehost::ChangedFile;

use super::diffs::{Changed, Diffs};
use super::{branch_chip, HostArea, Read, Said};
use crate::git::Diff;
use crate::icons::Icon;
use crate::theme::{self, Tokens, RADIUS_2};
use crate::ui::{self, Button, Choice, IconButton, Toggle, Variant};

/// What a new pull request starts with, as the core works it out.
#[derive(Deserialize)]
struct Start {
    head: Option<String>,
    base: Option<String>,
    branches: Vec<String>,
    pushed: bool,
}

/// What the branch changes since it left its base, as `git.compare` answers.
#[derive(Deserialize)]
struct Compare {
    merge_base: String,
    files: Vec<CompareFile>,
    commits: Vec<GitCommit>,
}

#[derive(Deserialize)]
struct CompareFile {
    path: String,
    status: String,
}

/// Which branch a pick is for.
#[derive(Clone, Copy, PartialEq)]
pub enum End {
    Head,
    Base,
}

pub struct NewPull {
    start: Read<Start>,
    /// The branch the person picked, else the one the core started from.
    head: Option<String>,
    base: Option<String>,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    draft: bool,
    busy: bool,
    pushing: bool,
    /// A title from the branch's only commit, put in once the form next draws.
    suggested: Option<String>,
    /// What `base..head` changes, and the branches it was read for.
    compared: Option<((String, String), Result<Compare, SharedString>)>,
    diffs: Diffs,
    _typing: Subscription,
}

impl NewPull {
    fn head(&self) -> Option<&str> {
        self.head
            .as_deref()
            .or_else(|| self.start.as_ref()?.as_ref().ok()?.head.as_deref())
            .filter(|head| !head.is_empty())
    }

    fn branches(&self) -> &[String] {
        match &self.start {
            Some(Ok(start)) => &start.branches,
            _ => &[],
        }
    }

    /// The head is on the host, so a pull request can be opened from it.
    fn pushed(&self) -> bool {
        let Some(Ok(start)) = &self.start else {
            return true;
        };
        self.head() != start.head.as_deref() || start.pushed
    }

    fn compare(&self) -> Option<&Compare> {
        self.compared.as_ref()?.1.as_ref().ok()
    }
}

impl HostArea {
    /// Starts writing a new pull request from the project's branch.
    pub(super) fn compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 14)
                .placeholder("What changed, and why")
        });
        let typing = cx.subscribe(&title, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        self.composing = Some(NewPull {
            start: None,
            head: None,
            base: None,
            title,
            body,
            draft: false,
            busy: false,
            pushing: false,
            suggested: None,
            compared: None,
            diffs: Diffs::new(Vec::new()),
            _typing: typing,
        });
        self.read_start(cx);
        cx.notify();
    }

    fn read_start(&self, cx: &mut Context<Self>) {
        let answer = self
            .core
            .request("codeHostNewPull", json!({ "project": self.project }));
        cx.spawn(async move |area, cx| {
            let start = answer
                .await
                .map_err(SharedString::from)
                .and_then(|value| super::answer::<Start>(&value));
            area.update(cx, |area, cx| {
                let Some(composing) = &mut area.composing else {
                    return;
                };
                if composing.base.is_none() {
                    if let Ok(start) = &start {
                        composing.base = start.base.clone();
                    }
                }
                composing.start = Some(start);
                area.compare(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads what the head changes since it left the base, unless that is read already.
    fn compare(&mut self, cx: &mut Context<Self>) {
        let Some(composing) = &mut self.composing else {
            return;
        };
        let (Some(head), Some(base)) =
            (composing.head().map(str::to_owned), composing.base.clone())
        else {
            return;
        };
        if head == base
            || composing
                .compared
                .as_ref()
                .is_some_and(|(pair, _)| *pair == (base.clone(), head.clone()))
        {
            return;
        }
        composing.compared = None;
        composing.diffs = Diffs::new(Vec::new());
        let params = json!({ "repo": self.project, "base": base, "head": head });
        let answer = self.core.request("git.compare", params);
        cx.spawn(async move |area, cx| {
            let compared = answer.await.map_err(SharedString::from).and_then(|value| {
                serde_json::from_value::<Compare>(value).map_err(|error| error.to_string().into())
            });
            area.update(cx, |area, cx| {
                let Some(composing) = &mut area.composing else {
                    return;
                };
                if let Ok(compare) = &compared {
                    let subject = match compare.commits.as_slice() {
                        [only] => Some(only.subject.clone()),
                        _ => None,
                    };
                    composing.suggested = subject;
                    composing.diffs = Diffs::new(
                        compare
                            .files
                            .iter()
                            .map(|file| Changed::new(changed_file(file), None))
                            .collect(),
                    );
                }
                let read = compared
                    .as_ref()
                    .ok()
                    .map(|compare| compare.merge_base.clone());
                composing.compared = Some(((base, head.clone()), compared));
                if let Some(merge_base) = read {
                    area.read_compared(&merge_base, &head, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads each changed file's diff from where the branch left its base.
    fn read_compared(&self, merge_base: &str, head: &str, cx: &mut Context<Self>) {
        let Some(composing) = &self.composing else {
            return;
        };
        for changed in &composing.diffs.files {
            let path = changed.file.path.clone();
            let params = json!({ "repo": self.project, "path": path, "baseRev": merge_base, "headRev": head, "full": false });
            let answer = self.core.request("git.fileDiff", params);
            cx.spawn(async move |area, cx| {
                let rows: Vec<DiffRow> = answer
                    .await
                    .ok()
                    .and_then(|rows| serde_json::from_value(rows).ok())
                    .unwrap_or_default();
                area.update(cx, |area, cx| {
                    let Some(composing) = &mut area.composing else {
                        return;
                    };
                    if let Some(changed) = composing
                        .diffs
                        .files
                        .iter_mut()
                        .find(|changed| changed.file.path == path)
                    {
                        changed.diff = Some(Diff::new(rows));
                        composing.diffs.lay_out();
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    pub(super) fn pick(&mut self, end: End, branch: String, cx: &mut Context<Self>) {
        let Some(composing) = &mut self.composing else {
            return;
        };
        match end {
            End::Head => composing.head = Some(branch),
            End::Base => composing.base = Some(branch),
        }
        self.compare(cx);
        cx.notify();
    }

    fn push(&mut self, cx: &mut Context<Self>) {
        let Some(composing) = &mut self.composing else {
            return;
        };
        let Some(head) = composing.head().map(str::to_owned) else {
            return;
        };
        composing.pushing = true;
        let answer = self
            .core
            .request("git.push", json!({ "repo": self.project }));
        cx.spawn(async move |area, cx| {
            let pushed = answer.await;
            area.update(cx, |area, cx| {
                if let Some(composing) = &mut area.composing {
                    composing.pushing = false;
                }
                match pushed {
                    Ok(_) => {
                        area.say(Said::Done(format!("Pushed {head}").into()), cx);
                        area.read_start(cx);
                    }
                    Err(error) => area.say(
                        Said::Failed(format!("Could not push {head}: {error}").into()),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_new(&mut self, cx: &mut Context<Self>) {
        let Some(composing) = &mut self.composing else {
            return;
        };
        let (Some(head), Some(base)) =
            (composing.head().map(str::to_owned), composing.base.clone())
        else {
            return;
        };
        let title = composing.title.read(cx).value().trim().to_owned();
        if title.is_empty() || head == base || !composing.pushed() || composing.busy {
            return;
        }
        let body = composing.body.read(cx).value().to_string();
        composing.busy = true;
        let params = json!({ "title": title, "head": head, "base": base, "body": body, "draft": composing.draft });
        let made = self.plugin("createPull", params);
        cx.spawn(async move |area, cx| {
            let made = made.await;
            area.update(cx, |area, cx| {
                if let Some(composing) = &mut area.composing {
                    composing.busy = false;
                }
                match made.map(|pull| pull["number"].as_u64()) {
                    Ok(Some(number)) => {
                        area.composing = None;
                        area.say(Said::Done(format!("Opened #{number}").into()), cx);
                        area.open_pull(number, cx);
                    }
                    Ok(None) => area.say(
                        Said::Failed("The host did not say which pull request it opened".into()),
                        cx,
                    ),
                    Err(error) => area.say(
                        Said::Failed(format!("Could not open the pull request: {error}").into()),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

/// A compared file as a diff card's header names it.
fn changed_file(file: &CompareFile) -> ChangedFile {
    let status = match file.status.as_str() {
        "A" => "added",
        "D" => "removed",
        "R" => "renamed",
        _ => "modified",
    };
    ChangedFile {
        path: file.path.clone(),
        status: status.into(),
        additions: 0,
        deletions: 0,
        previous_path: None,
        patch: None,
        note: None,
    }
}

pub fn view(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    if let Some(composing) = &mut area.composing {
        if let Some(subject) = composing.suggested.take() {
            if composing.title.read(cx).value().is_empty() {
                composing
                    .title
                    .update(cx, |title, cx| title.set_value(subject, window, cx));
            }
        }
        composing
            .diffs
            .colour(cx, |area| Some(&mut area.composing.as_mut()?.diffs));
    }
    let left = side(area, cx);
    let right = main(area, window, cx);
    div()
        .size_full()
        .flex()
        .child(
            div()
                .id("compose-side")
                .w(px(320.))
                .flex_none()
                .h_full()
                .overflow_y_scroll()
                .border_r_1()
                .border_color(t.border)
                .child(left),
        )
        .child(div().flex_1().min_w_0().h_full().child(right))
        .into_any_element()
}

fn side(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let host = super::host_label(&area.host.host);
    let composing = area
        .composing
        .as_ref()
        .expect("a pull request is being written");
    let head = composing.head().map(str::to_owned);
    let base = composing.base.clone();
    let picker = |id: &'static str,
                  end: End,
                  shown: Option<String>,
                  empty: &'static str,
                  cx: &mut Context<HostArea>| {
        div()
            .id(id)
            .h(px(24.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.border)
            .text_size(crate::theme::ui_text(11.5))
            .text_color(if shown.is_some() {
                t.text_primary
            } else {
                t.text_tertiary
            })
            .cursor_pointer()
            .hover(move |chip| chip.bg(t.surface_hover))
            .on_click(cx.listener(move |area, _, _, cx| area.choose(end, cx)))
            .child(ui::icon(Icon::Git, 11.).text_color(t.text_tertiary))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::MONO)
                    .child(shown.unwrap_or_else(|| empty.into())),
            )
    };
    let title = composing.title.read(cx).value().trim().to_owned();
    let ready = !title.is_empty()
        && head.is_some()
        && base.is_some()
        && head != base
        && composing.pushed()
        && !composing.busy;
    let open_label = if composing.busy {
        "Opening…"
    } else if composing.draft {
        "Open as a draft"
    } else {
        "Open pull request"
    };
    let starting = composing
        .start
        .as_ref()
        .and_then(|start| start.as_ref().ok())
        .and_then(|start| start.head.clone());
    let push = (!composing.pushed()).then(|| {
        div()
            .mx(px(10.))
            .mb(px(8.))
            .px(px(10.))
            .py(px(7.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.warn.opacity(0.34))
            .bg(t.warn.opacity(0.08))
            .text_size(crate::theme::ui_text(11.5))
            .child(format!("Not on {host} yet"))
            .child(div().flex_1())
            .when(head == starting, |callout| {
                callout.child(
                    Button::new(
                        "compose-push",
                        if composing.pushing {
                            "Pushing…"
                        } else {
                            "Push"
                        },
                        Variant::Outline,
                    )
                    .icon(Icon::Push)
                    .disabled(composing.pushing)
                    .on_click(cx.listener(|area, _, _, cx| area.push(cx))),
                )
            })
    });
    let compare = composing.compare();
    let files = compare.map(|compare| {
        div()
            .flex()
            .flex_col()
            .pt(px(6.))
            .child(
                div()
                    .px(px(12.))
                    .py(px(4.))
                    .flex()
                    .gap(px(6.))
                    .text_size(crate::theme::ui_text(11.))
                    .text_color(t.text_secondary)
                    .child("Files")
                    .child(
                        div()
                            .text_color(t.text_tertiary)
                            .child(compare.files.len().to_string()),
                    ),
            )
            .children(compare.files.iter().enumerate().map(|(at, file)| {
                let path = file.path.clone();
                let (dir, name) = match path.rsplit_once('/') {
                    Some((dir, name)) => (Some(dir.to_owned()), name.to_owned()),
                    None => (None, path.clone()),
                };
                let ink = match file.status.as_str() {
                    "A" => t.git_added,
                    "D" => t.git_deleted,
                    "R" => t.git_renamed,
                    _ => t.git_modified,
                };
                div()
                    .id(("compose-file", at))
                    .h(px(26.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(crate::theme::ui_text(12.))
                    .cursor_pointer()
                    .hover(move |row| row.bg(t.surface_hover))
                    .on_click(cx.listener(move |area, _, _, cx| {
                        if let Some(composing) = &area.composing {
                            composing.diffs.reveal(&path);
                        }
                        cx.notify();
                    }))
                    .child(ui::icon(Icon::File, 13.).text_color(t.text_tertiary))
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
                            .font_family(theme::MONO)
                            .text_size(crate::theme::ui_text(11.))
                            .text_color(ink)
                            .child(file.status.clone()),
                    )
            }))
    });
    let commits = compare
        .filter(|compare| !compare.commits.is_empty())
        .map(|compare| {
            div()
                .flex()
                .flex_col()
                .pt(px(8.))
                .child(
                    div()
                        .px(px(12.))
                        .py(px(4.))
                        .flex()
                        .gap(px(6.))
                        .text_size(crate::theme::ui_text(11.))
                        .text_color(t.text_secondary)
                        .child("Commits")
                        .child(
                            div()
                                .text_color(t.text_tertiary)
                                .child(compare.commits.len().to_string()),
                        ),
                )
                .children(compare.commits.iter().map(|commit| {
                    div()
                        .px(px(12.))
                        .py(px(5.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(crate::theme::ui_text(12.))
                        .child(ui::initials(&commit.author, &commit.author_email, 16., cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text_primary)
                                .child(commit.subject.clone()),
                        )
                        .child(ui::mono(commit.hash.clone(), 11., cx).text_color(t.text_tertiary))
                }))
        });
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(10.))
                .py(px(10.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(picker(
                    "compose-head",
                    End::Head,
                    head.clone(),
                    "Choose a branch",
                    cx,
                ))
                .child(ui::meta("into", cx))
                .child(picker(
                    "compose-base",
                    End::Base,
                    base.clone(),
                    "Choose a base",
                    cx,
                ))
                .child(div().flex_1())
                .child(
                    IconButton::new("compose-cancel", Icon::Close, "Cancel")
                        .small()
                        .on_click(cx.listener(|area, _, _, cx| {
                            area.composing = None;
                            cx.notify();
                        })),
                ),
        )
        .children(push)
        .child(
            div()
                .mx(px(10.))
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .child(
                    div()
                        .px(px(10.))
                        .py(px(7.))
                        .border_b_1()
                        .border_color(t.border)
                        .text_size(crate::theme::ui_text(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(Input::new(&composing.title).appearance(false)),
                )
                .child(
                    div()
                        .px(px(10.))
                        .py(px(7.))
                        .text_size(crate::theme::ui_text(12.5))
                        .child(
                            Textarea::new(&composing.body)
                                .appearance(false)
                                .text_size(crate::theme::ui_text(12.5)),
                        ),
                )
                .child(
                    div()
                        .px(px(10.))
                        .py(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .border_t_1()
                        .border_color(t.border)
                        .child(
                            Toggle::new("compose-draft", "Draft", composing.draft).on_click(
                                cx.listener(|area, _, _, cx| {
                                    if let Some(composing) = &mut area.composing {
                                        composing.draft = !composing.draft;
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("compose-open", open_label, Variant::Primary)
                                .disabled(!ready)
                                .on_click(cx.listener(|area, _, _, cx| area.open_new(cx))),
                        ),
                ),
        )
        .children(files)
        .children(commits)
        .into_any_element()
}

fn main(area: &HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let composing = area
        .composing
        .as_ref()
        .expect("a pull request is being written");
    let head = composing.head();
    let base = composing.base.as_deref();
    let (Some(head), Some(base)) = (head, base) else {
        return super::notice("Choose the branch to open a pull request from.", None, cx);
    };
    let compare = match &composing.compared {
        None => return super::notice("Comparing branches", None, cx),
        Some((_, Err(_))) => {
            return super::notice(
                format!("{head} is not in this checkout. Fetch it to see what it changes."),
                None,
                cx,
            )
        }
        Some((_, Ok(compare))) => compare,
    };
    if compare.files.is_empty() {
        return super::notice(
            format!("{head} has nothing that {base} does not."),
            None,
            cx,
        );
    }
    let title = composing.title.read(cx).value().trim().to_owned();
    let commits = compare.commits.len();
    let files = compare.files.len();
    let plural =
        |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    let newest = compare
        .commits
        .first()
        .map(|commit| commit.date.clone())
        .unwrap_or_default();
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(16.))
                .pt(px(14.))
                .pb(px(10.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    ui::text(
                        if title.is_empty() {
                            "New pull request".to_owned()
                        } else {
                            title
                        },
                        15.,
                        FontWeight::SEMIBOLD,
                    )
                    .text_color(t.text_primary),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(crate::theme::ui_text(10.5))
                        .text_color(t.text_tertiary)
                        .child(format!(
                            "{} · {}",
                            plural(commits, "commit"),
                            plural(files, "file")
                        ))
                        .child(branch_chip(head, t.text_secondary, cx))
                        .child("→")
                        .child(branch_chip(base, t.text_secondary, cx))
                        .when(!newest.is_empty(), |line| line.child(newest)),
                ),
        )
        .child(div().flex_1().min_h_0().child(composing.diffs.list(
            &cx.entity(),
            |area| Some(&area.composing.as_ref()?.diffs),
            Some(&area.project),
            None,
            window,
        )))
        .into_any_element()
}

/// The branches the person can pick for `end`, as a menu.
pub fn choices(area: &HostArea, end: End, cx: &mut Context<HostArea>) -> Vec<Choice> {
    let Some(composing) = &area.composing else {
        return Vec::new();
    };
    let other = match end {
        End::Head => composing.base.clone(),
        End::Base => composing.head().map(str::to_owned),
    };
    let view = cx.entity().downgrade();
    composing
        .branches()
        .iter()
        .filter(|branch| Some(*branch) != other.as_ref())
        .map(|branch| {
            let (view, picked) = (view.clone(), branch.clone());
            Choice::new(None, branch.clone(), move |_, cx| {
                let picked = picked.clone();
                view.update(cx, |area, cx| {
                    area.picking = None;
                    area.pick(end, picked, cx);
                })
                .ok();
            })
        })
        .collect()
}
