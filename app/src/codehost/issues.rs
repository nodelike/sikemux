//! The code host's issues: the list, one issue opened beside it with its conversation,
//! a new issue, and starting an agent on one.

use std::collections::HashSet;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Window,
};
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::{Issue, IssueList, TimelineItem};

use super::pulls::Listed;
use super::thread::{self, Block, Post};
use super::{comments, label_chip, list_head, notice, Area, HostArea, Read, Said};
use crate::icons::Icon;
use crate::theme::{
    Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4,
    TEXT_3XS_H, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui::time::ago_written as ago;
use crate::ui::{self, Button, IconButton, Segmented, Variant};

pub const KEY: &str = "host-issues";

#[derive(Default)]
pub struct Issues {
    pub state: Listed,
    pub page: u32,
    pub list: Read<IssueList>,
    pub open: Option<OpenIssue>,
    pub composing: Option<NewIssue>,
    /// The issues an agent is being started on.
    working: HashSet<u64>,
}

impl Issues {
    /// The open issue's number, and the issue once it has been read.
    pub(super) fn shown(&self) -> Option<(u64, Option<&Issue>)> {
        let open = self.open.as_ref()?;
        let issue = open.issue.as_ref().and_then(|issue| issue.as_ref().ok());
        Some((open.number, issue))
    }
}

/// The side column's pages beside an open issue.
#[derive(Clone, Copy, Default, PartialEq)]
enum Side {
    #[default]
    Facts,
    List,
}

pub struct OpenIssue {
    number: u64,
    issue: Read<Issue>,
    opening: Option<Post>,
    thread: Read<Vec<Block>>,
    /// Everyone who took part, its author first.
    people: Vec<String>,
    /// The pull requests that mention it, each once.
    linked: Vec<TimelineItem>,
    side: Side,
    draft: Option<Entity<TextareaState>>,
    sending: bool,
    busy: bool,
}

pub struct NewIssue {
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    busy: bool,
    _typing: Subscription,
}

/// An issue's state, with why it was closed, as a glyph, its colour and its name.
pub(crate) fn look(state: &str, reason: Option<&str>, t: &Tokens) -> (Icon, Hsla, &'static str) {
    match (state, reason) {
        ("open", _) => (Icon::Issues, t.live, "Open"),
        (_, Some("not_planned")) => (
            Icon::MarkCancelled,
            t.text_tertiary,
            "Closed as not planned",
        ),
        _ => (Icon::Check, t.accent, "Closed"),
    }
}

pub(super) fn mark(issue: &Issue, size: f32, cx: &App) -> AnyElement {
    let (glyph, ink, label) = look(&issue.state, issue.state_reason.as_deref(), Tokens::get(cx));
    div()
        .id(SharedString::from(format!("issue-mark-{}", issue.number)))
        .flex_none()
        .tooltip(ui::tooltip(label))
        .child(ui::icon(glyph, size).text_color(ink))
        .into_any_element()
}

impl HostArea {
    pub(super) fn watch_issues(&self) {
        let params = json!({
            "project": self.project,
            "state": self.issues.state.word(),
            "page": self.issues.page.max(1),
        });
        self.ask_watch(KEY, "codeHostIssues", params);
    }

    pub(super) fn open_issue(&mut self, number: u64, cx: &mut Context<Self>) {
        let side = self
            .issues
            .open
            .as_ref()
            .map(|open| open.side)
            .unwrap_or_default();
        self.issues.open = Some(OpenIssue {
            number,
            issue: None,
            opening: None,
            thread: None,
            people: Vec::new(),
            linked: Vec::new(),
            side,
            draft: None,
            sending: false,
            busy: false,
        });
        self.read_issue(number, cx);
        cx.notify();
    }

    /// Reads the open issue again when the list the core keeps fresh shows it changed since.
    pub(super) fn follow_open_issue(&self, cx: &mut Context<Self>) {
        let Some(open) = &self.issues.open else {
            return;
        };
        let Some(Ok(read)) = &open.issue else {
            return;
        };
        let listed = match &self.issues.list {
            Some(Ok(list)) => list.issues.iter().find(|issue| issue.number == open.number),
            _ => None,
        };
        if listed.is_some_and(|listed| listed.updated_at != read.updated_at) {
            self.read_issue(open.number, cx);
        }
    }

    /// Reads the open issue and its conversation from the host.
    pub(super) fn read_issue(&self, number: u64, cx: &mut Context<Self>) {
        let issue = self.plugin("issue", json!({ "number": number }));
        let timeline = self.plugin("timeline", json!({ "number": number, "of": "issue" }));
        cx.spawn(async move |area, cx| {
            let (issue, timeline) = (issue.await, timeline.await);
            area.update(cx, |area, cx| {
                let Some(open) = area
                    .issues
                    .open
                    .as_mut()
                    .filter(|open| open.number == number)
                else {
                    return;
                };
                let issue: Read<Issue> = Some(issue.and_then(|found| {
                    serde_json::from_value(found).map_err(|error| error.to_string().into())
                }));
                let items: Result<Vec<TimelineItem>, SharedString> = timeline.and_then(|items| {
                    serde_json::from_value(items).map_err(|error| error.to_string().into())
                });
                if let Some(Ok(found)) = &issue {
                    open.opening = Some(
                        Post::new(
                            found.author.clone(),
                            None,
                            Some(found.created_at.clone()),
                            None,
                            &found.body,
                        )
                        .pictured(found.avatar_url.clone()),
                    );
                    let mut people: Vec<String> = Vec::new();
                    let actors = items.iter().flatten().filter_map(|item| item.actor.clone());
                    for login in found.author.clone().into_iter().chain(actors) {
                        if !people.contains(&login) {
                            people.push(login);
                        }
                    }
                    open.people = people;
                }
                if let Ok(items) = &items {
                    let mut seen = HashSet::new();
                    open.linked = items
                        .iter()
                        .filter(|item| item.kind == "cross-referenced")
                        .filter(|item| item.source.as_ref().is_some_and(|source| source.pull))
                        .filter(|item| {
                            seen.insert(item.source.as_ref().map(|source| source.number))
                        })
                        .cloned()
                        .collect();
                }
                let unread = open.issue.is_none() && matches!(issue, Some(Err(_)));
                open.issue = issue;
                open.thread = Some(items.map(thread::blocks));
                if unread {
                    area.fall_back(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_issue_state(&mut self, close: bool, cx: &mut Context<Self>) {
        let Some(open) = &mut self.issues.open else {
            return;
        };
        open.busy = true;
        let number = open.number;
        let state = if close { "closed" } else { "open" };
        let answer = self.plugin("setIssueState", json!({ "number": number, "state": state }));
        cx.spawn(async move |area, cx| {
            let answer = answer.await;
            area.update(cx, |area, cx| {
                if let Some(open) = &mut area.issues.open {
                    open.busy = false;
                }
                let said = match answer {
                    Ok(_) if close => Said::Done(format!("Closed #{number}").into()),
                    Ok(_) => Said::Done(format!("Reopened #{number}").into()),
                    Err(error) if close => {
                        Said::Failed(format!("Could not close it: {error}").into())
                    }
                    Err(error) => Said::Failed(format!("Could not reopen it: {error}").into()),
                };
                if matches!(said, Said::Done(_)) {
                    area.read_issue(number, cx);
                }
                area.say(said, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn comment_on_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &mut self.issues.open else {
            return;
        };
        let Some(draft) = open.draft.clone() else {
            return;
        };
        let body = draft.read(cx).value().trim().to_owned();
        let number = open.number;
        open.sending = true;
        let answer = self.plugin(
            "addComment",
            json!({ "number": number, "body": body, "of": "issue" }),
        );
        cx.spawn_in(window, async move |area, cx| {
            let answer = answer.await;
            area.update_in(cx, |area, window, cx| {
                if let Some(open) = &mut area.issues.open {
                    open.sending = false;
                }
                match answer {
                    Ok(_) => {
                        draft.update(cx, |draft, cx| draft.set_value("", window, cx));
                        area.read_issue(number, cx);
                        area.say(Said::Done("Comment added".into()), cx);
                    }
                    Err(error) => area.say(Said::Failed(error), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Starts a chat in the project with issue `number` in its composer, as the core does it.
    fn work_on(&mut self, number: u64, cx: &mut Context<Self>) {
        if !self.issues.working.insert(number) {
            return;
        }
        let params = json!({ "project": self.project, "number": number });
        let answer = self.core.request("codeHostWorkOn", params);
        cx.spawn(async move |area, cx| {
            let answer = answer.await;
            area.update(cx, |area, cx| {
                area.issues.working.remove(&number);
                let started = answer
                    .map_err(SharedString::from)
                    .and_then(|value| super::answer::<Value>(&value));
                if let Err(error) = started {
                    let said = format!("Could not start an agent on #{number}: {error}");
                    area.say(Said::Failed(said.into()), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn compose_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(6, 20)
                .placeholder("What happened, and what you expected")
        });
        let typing = cx.subscribe(&title, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        self.issues.composing = Some(NewIssue {
            title,
            body,
            busy: false,
            _typing: typing,
        });
        cx.notify();
    }

    fn create_issue(&mut self, cx: &mut Context<Self>) {
        let Some(composing) = &mut self.issues.composing else {
            return;
        };
        let title = composing.title.read(cx).value().trim().to_owned();
        let body = composing.body.read(cx).value().to_string();
        if title.is_empty() || composing.busy {
            return;
        }
        composing.busy = true;
        let answer = self.plugin("createIssue", json!({ "title": title, "body": body }));
        cx.spawn(async move |area, cx| {
            let answer = answer.await;
            area.update(cx, |area, cx| {
                match answer {
                    Ok(made) => {
                        let number = made["number"].as_u64().unwrap_or_default();
                        area.issues.composing = None;
                        area.say(Said::Done(format!("Opened #{number}").into()), cx);
                        if number > 0 {
                            area.open_issue(number, cx);
                        }
                    }
                    Err(error) => {
                        if let Some(composing) = &mut area.issues.composing {
                            composing.busy = false;
                        }
                        let said = format!("Could not open the issue: {error}");
                        area.say(Said::Failed(said.into()), cx);
                    }
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
    if area.issues.composing.is_some() {
        return new_issue(area, cx);
    }
    if area.issues.open.is_none() {
        return list(area, cx);
    }
    let t = Tokens::get(cx).clone();
    let left = side(area, cx);
    let right = detail(area, window, cx);
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

fn state_switch(area: &HostArea, cx: &mut Context<HostArea>) -> Segmented {
    let counts = area
        .issues
        .list
        .as_ref()
        .and_then(|list| list.as_ref().ok())
        .and_then(|list| list.counts.clone());
    let words = Listed::EVERY
        .iter()
        .map(|listed| {
            let count = counts.as_ref().and_then(|counts| match listed {
                Listed::Open => Some(counts.open),
                Listed::Closed => Some(counts.closed),
                Listed::All => None,
            });
            (SharedString::from(listed.label()), count)
        })
        .collect();
    let chosen = Listed::EVERY
        .iter()
        .position(|listed| *listed == area.issues.state)
        .unwrap_or_default();
    Segmented::words("issue-state", words, chosen).on_select(cx.listener(
        |area, index: &usize, _, cx| {
            area.issues.state = Listed::EVERY[*index];
            area.issues.page = 1;
            area.issues.list = None;
            area.watch();
            cx.notify();
        },
    ))
}

fn turn_page(area: &mut HostArea, by: i64, cx: &mut Context<HostArea>) {
    area.issues.page = (i64::from(area.issues.page.max(1)) + by).max(1) as u32;
    area.issues.list = None;
    area.watch();
    cx.notify();
}

fn list(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let total = area
        .issues
        .list
        .as_ref()
        .and_then(|list| list.as_ref().ok())
        .map(|list| list.total);
    let head = list_head(Area::Issues, &area.host, cx)
        .child(state_switch(area, cx))
        .child(div().flex_1())
        .children(total.map(|total| ui::meta(total.to_string(), cx)))
        .child(
            IconButton::new("issue-new", Icon::Plus, "New issue")
                .small()
                .on_click(cx.listener(|area, _, window, cx| area.compose_issue(window, cx))),
        )
        .child(super::shell::account(area, cx));
    let page = area.issues.page.max(1);
    let body = match &area.issues.list {
        None => notice("Loading issues", None, cx),
        Some(Err(error)) => notice("Could not read issues", Some(error.clone()), cx),
        Some(Ok(list)) if list.issues.is_empty() => {
            let which = match area.issues.state {
                Listed::All => String::new(),
                listed => format!("{} ", listed.word()),
            };
            notice(format!("No {which}issues."), None, cx)
        }
        Some(Ok(list)) => super::carded(div().flex().flex_col(), cx)
            .children(
                list.issues
                    .iter()
                    .enumerate()
                    .map(|(index, issue)| row(area, issue, index, cx)),
            )
            .into_any_element(),
    };
    let next = matches!(&area.issues.list, Some(Ok(list)) if list.next_page.is_some());
    let pager = (page > 1 || next).then(|| {
        div()
            .px(SPACE_3)
            .py(SPACE_2H)
            .flex()
            .items_center()
            .justify_center()
            .gap(SPACE_2H)
            .border_t_1()
            .border_color(t.border)
            .child(
                Button::new("issues-newer", "Newer", Variant::Outline)
                    .disabled(page <= 1)
                    .on_click(cx.listener(|area, _, _, cx| turn_page(area, -1, cx))),
            )
            .child(ui::meta(format!("Page {page}"), cx))
            .child(
                Button::new("issues-older", "Older", Variant::Outline)
                    .disabled(!next)
                    .on_click(cx.listener(|area, _, _, cx| turn_page(area, 1, cx))),
            )
    });
    let shown = div().flex().flex_col().child(body).children(pager);
    super::list_page("host-issues", head, shown, cx)
}

/// One issue: who opened it, its state and title, then its number, who and when, with
/// who it is assigned to, its comments and starting work on it at the end.
fn row(area: &HostArea, issue: &Issue, index: usize, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let author = issue.author.as_deref().unwrap_or("someone");
    let (opened, updated) = (ago(&issue.created_at), ago(&issue.updated_at));
    let mut who = format!("#{} · {author} opened {opened}", issue.number);
    if updated != opened {
        who.push_str(&format!(" · updated {updated}"));
    }
    let number = issue.number;
    let assignees = (!issue.assignees.is_empty()).then(|| {
        div()
            .id(("issue-assignees", index))
            .flex()
            .gap(SPACE_0)
            .tooltip(ui::tooltip(format!(
                "Assigned to {}",
                issue.assignees.join(", ")
            )))
            .children(
                issue
                    .assignees
                    .iter()
                    .map(|login| ui::initials(login, "", 18., cx)),
            )
    });
    let work = (issue.state == "open" && area.elsewhere.is_none()).then(|| {
        IconButton::new(("issue-work", index), Icon::Agent, "Work on this")
            .small()
            .disabled(area.issues.working.contains(&number))
            .on_click(cx.listener(move |area, _, _, cx| {
                cx.stop_propagation();
                area.work_on(number, cx)
            }))
    });
    div()
        .id(("issue", index))
        .w_full()
        .px(SPACE_3)
        .py(SPACE_2H)
        .flex()
        .items_center()
        .gap(SPACE_3)
        .when(index > 0, |row| row.border_t_1().border_color(t.border))
        .cursor_pointer()
        .hover(move |row| row.bg(t.surface_hover))
        .on_click(cx.listener(move |area, _, _, cx| area.open_issue(number, cx)))
        .child(area.face(Some(author), issue.avatar_url.as_deref(), 28., cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .min_w_0()
                        .child(mark(issue, 14., cx))
                        .child(
                            ui::text(issue.title.clone(), 13., FontWeight::SEMIBOLD)
                                .text_color(t.text_primary)
                                .min_w_0()
                                .truncate(),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_tertiary)
                        .child(div().flex_none().child(who))
                        .children(issue.labels.iter().map(|label| label_chip(label, cx))),
                ),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(SPACE_3)
                .children(assignees)
                .children(comments(issue.comments, cx))
                .children(work),
        )
        .into_any_element()
}

fn side(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let open = area.issues.open.as_ref().expect("an issue is open");
    let found = open.issue.as_ref().and_then(|issue| issue.as_ref().ok());
    let head = div()
        .px(SPACE_2H)
        .pt(SPACE_2H)
        .pb(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .when(!area.on_a_desk(), |head| {
            head.child(
                IconButton::new("issue-back", Icon::Chevron, "Back to the issues")
                    .turned(std::f32::consts::PI)
                    .small()
                    .on_click(cx.listener(|area, _, _, cx| {
                        area.issues.open = None;
                        area.elsewhere = None;
                        cx.notify();
                    })),
            )
        })
        .children(found.map(|issue| mark(issue, 14., cx)))
        .child(ui::mono(format!("#{}", open.number), 12., cx).text_color(t.text_primary))
        .child(div().flex_1())
        .child(
            IconButton::new("issue-new-side", Icon::Plus, "New issue")
                .small()
                .on_click(cx.listener(|area, _, window, cx| area.compose_issue(window, cx))),
        );
    let pages = [
        (Icon::Issue, "Details", Side::Facts),
        (Icon::Issues, "Issues", Side::List),
    ];
    let chosen = pages
        .iter()
        .position(|(_, _, page)| *page == open.side)
        .unwrap_or_default();
    let switch = div().px(SPACE_2H).pb(SPACE_2).child(
        Segmented::new(
            "issue-pages",
            pages
                .iter()
                .map(|(glyph, label, _)| (*glyph, SharedString::from(*label)))
                .collect(),
            chosen,
        )
        .on_select(cx.listener(move |area, index: &usize, _, cx| {
            if let Some(open) = &mut area.issues.open {
                open.side = pages[*index].2;
            }
            cx.notify();
        })),
    );
    let body = match open.side {
        Side::Facts => facts(open, cx),
        Side::List => side_list(area, open.number, cx),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(head)
        .child(switch)
        .child(
            div()
                .id("issue-side-page")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
        .into_any_element()
}

/// What the host keeps beside an issue, each fact under its name: who it is on, its
/// labels, who took part, its comments and the pull requests that mention it.
fn facts(open: &OpenIssue, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(Ok(issue)) = &open.issue else {
        return match &open.issue {
            Some(Err(error)) => notice("Could not read it", Some(error.clone()), cx),
            _ => notice("Loading issue", None, cx),
        };
    };
    let fact = |name: &'static str, value: AnyElement| {
        div()
            .px(SPACE_3)
            .py(SPACE_2)
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .border_b_1()
            .border_color(t.border)
            .child(ui::meta(name, cx))
            .child(value)
    };
    let none = |words: &'static str| ui::meta(words, cx).into_any_element();
    let people = |logins: &[String]| {
        div()
            .flex()
            .flex_wrap()
            .gap(SPACE_2)
            .children(logins.iter().map(|login| {
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_primary)
                    .child(ui::initials(login, "", 18., cx))
                    .child(login.clone())
            }))
            .into_any_element()
    };
    let assignees = if issue.assignees.is_empty() {
        none("No one")
    } else {
        people(&issue.assignees)
    };
    let labels = if issue.labels.is_empty() {
        none("None")
    } else {
        div()
            .flex()
            .flex_wrap()
            .gap(SPACE_1)
            .children(issue.labels.iter().map(|label| label_chip(label, cx)))
            .into_any_element()
    };
    let linked = if open.linked.is_empty() {
        none("None")
    } else {
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .children(open.linked.iter().filter_map(|item| {
                let source = item.source.as_ref()?;
                let (glyph, ink, _) = thread::mention_state(source, &t);
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .min_w_0()
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .child(ui::icon(glyph, 13.).text_color(ink))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text_primary)
                                .child(item.subject.clone().unwrap_or_default()),
                        )
                        .child(ui::meta(format!("#{}", source.number), cx)),
                )
            }))
            .into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .child(fact("Assignees", assignees))
        .child(fact("Labels", labels))
        .child(fact("Participants", people(&open.people)))
        .child(fact(
            "Comments",
            ui::text(issue.comments.to_string(), 12., FontWeight::NORMAL)
                .text_color(t.text_primary)
                .into_any_element(),
        ))
        .child(fact("Linked pull requests", linked))
        .into_any_element()
}

fn side_list(area: &HostArea, open: u64, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let issues = match &area.issues.list {
        None => return notice("Loading issues", None, cx),
        Some(Err(error)) => return notice("Could not read issues", Some(error.clone()), cx),
        Some(Ok(list)) => &list.issues,
    };
    div()
        .flex()
        .flex_col()
        .child(div().px(SPACE_2H).pb(SPACE_2).child(state_switch(area, cx)))
        .children(issues.iter().enumerate().map(|(index, issue)| {
            let number = issue.number;
            let sub = format!(
                "#{number} · {} · {}",
                issue.author.as_deref().unwrap_or("someone"),
                ago(&issue.updated_at)
            );
            div()
                .id(("issue-side", index))
                .px(SPACE_3)
                .py(SPACE_1H)
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .cursor_pointer()
                .when(number == open, |row| row.bg(t.surface_chosen))
                .hover(move |row| row.bg(t.surface_hover))
                .on_click(cx.listener(move |area, _, _, cx| area.open_issue(number, cx)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .child(mark(issue, 13., cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::theme::ui_text(TEXT_XS_H))
                                .text_color(t.text_primary)
                                .child(issue.title.clone()),
                        )
                        .children(comments(issue.comments, cx)),
                )
                .child(ui::meta(sub, cx).pl(px(19.)))
        }))
        .into_any_element()
}

fn detail(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let host = area.host.label.as_str();
    let open = area.issues.open.as_ref().expect("an issue is open");
    let issue = match &open.issue {
        None => return notice("Loading issue", None, cx),
        Some(Err(error)) => return notice("Could not read it", Some(error.clone()), cx),
        Some(Ok(issue)) => issue.clone(),
    };
    let author = issue.author.clone().unwrap_or_else(|| "someone".into());
    let url = issue.url.clone();
    let number = issue.number;
    let mut who = format!("#{number} · {author} opened {}", ago(&issue.created_at));
    if !issue.assignees.is_empty() {
        who.push_str(&format!(" · assigned to {}", issue.assignees.join(", ")));
    }
    let working = area.issues.working.contains(&number);
    let headline = div()
        .px(SPACE_4)
        .pt(SPACE_3H)
        .pb(SPACE_2H)
        .flex()
        .gap(SPACE_3)
        .border_b_1()
        .border_color(t.border)
        .child(area.face(Some(&author), issue.avatar_url.as_deref(), 36., cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(mark(&issue, 16., cx))
                        .child(
                            ui::text(issue.title.clone(), 15., FontWeight::SEMIBOLD)
                                .text_color(t.text_primary)
                                .min_w_0()
                                .truncate(),
                        )
                        .child(div().flex_1())
                        .when(issue.state == "open" && area.elsewhere.is_none(), |line| {
                            line.child(
                                Button::new(
                                    "issue-work-on",
                                    if working {
                                        "Starting…"
                                    } else {
                                        "Work on this"
                                    },
                                    Variant::Outline,
                                )
                                .icon(Icon::Agent)
                                .disabled(working)
                                .on_click(
                                    cx.listener(move |area, _, _, cx| area.work_on(number, cx)),
                                ),
                            )
                        })
                        .child(
                            IconButton::new(
                                "issue-on-host",
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
                        .gap(SPACE_1H)
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_tertiary)
                        .child(who)
                        .children(issue.labels.iter().map(|label| label_chip(label, cx))),
                ),
        );
    let blocks = match &open.thread {
        Some(Ok(blocks)) => &blocks[..],
        _ => &[],
    };
    let shown: &HostArea = area;
    let faces = |login: Option<&str>, url: Option<&str>, size: f32, cx: &App| {
        shown.face(login, url, size, cx)
    };
    let mut column = div()
        .id("issue-thread")
        .size_full()
        .overflow_y_scroll()
        .child(thread::thread(
            open.opening.as_ref(),
            blocks,
            &Default::default(),
            None,
            &faces,
            &area.bench,
            window,
            cx,
        ));
    if let Some(Err(error)) = &open.thread {
        column = column.child(notice(
            "Could not read the conversation",
            Some(error.clone()),
            cx,
        ));
    }
    let column = column.child(composer(area, issue.state == "open", window, cx));
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(headline)
        .child(div().flex_1().min_h_0().child(column))
        .into_any_element()
}

/// Where a comment is written, beside closing or reopening the issue.
fn composer(
    area: &mut HostArea,
    open_now: bool,
    window: &mut Window,
    cx: &mut Context<HostArea>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(open) = area.issues.open.as_mut() else {
        return div().into_any_element();
    };
    let draft = open
        .draft
        .get_or_insert_with(|| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 12)
                    .placeholder("Leave a comment")
            })
        })
        .clone();
    let written = !draft.read(cx).value().trim().is_empty();
    let (sending, busy) = (open.sending, open.busy);
    let state_label = match (open_now, busy) {
        (true, true) => "Closing…",
        (true, false) => "Close issue",
        (false, true) => "Reopening…",
        (false, false) => "Reopen issue",
    };
    let foot = div()
        .px(SPACE_2H)
        .py(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .border_t_1()
        .border_color(t.border)
        .child(div().flex_1())
        .child(
            Button::new("issue-state-change", state_label, Variant::Outline)
                .disabled(busy)
                .on_click(cx.listener(move |area, _, _, cx| area.set_issue_state(open_now, cx))),
        )
        .child(
            Button::new(
                "issue-comment",
                if sending { "Sending…" } else { "Comment" },
                Variant::Primary,
            )
            .disabled(sending || !written)
            .on_click(cx.listener(|area, _, window, cx| area.comment_on_issue(window, cx))),
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

fn new_issue(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(composing) = &area.issues.composing else {
        return div().into_any_element();
    };
    let ready = !composing.busy && !composing.title.read(cx).value().trim().is_empty();
    div()
        .id("issue-new-page")
        .size_full()
        .overflow_y_scroll()
        .child(
            div()
                .max_w(px(720.))
                .px(SPACE_4)
                .py(SPACE_3H)
                .flex()
                .flex_col()
                .gap(SPACE_2H)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(
                            IconButton::new("issue-new-back", Icon::Chevron, "Back to the issues")
                                .turned(std::f32::consts::PI)
                                .small()
                                .on_click(cx.listener(|area, _, _, cx| {
                                    area.issues.composing = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            ui::text("New issue", 15., FontWeight::SEMIBOLD)
                                .text_color(t.text_primary),
                        ),
                )
                .child(
                    div()
                        .rounded(RADIUS_2)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.surface_composer)
                        .child(
                            div()
                                .px(SPACE_2H)
                                .py(px(7.))
                                .border_b_1()
                                .border_color(t.border)
                                .text_size(crate::theme::ui_text(TEXT_SM))
                                .font_weight(FontWeight::MEDIUM)
                                .child(Input::new(&composing.title).appearance(false)),
                        )
                        .child(
                            div()
                                .px(SPACE_2H)
                                .py(px(7.))
                                .text_size(crate::theme::ui_text(TEXT_XS_H))
                                .child(
                                    Textarea::new(&composing.body)
                                        .appearance(false)
                                        .text_size(crate::theme::ui_text(TEXT_XS_H)),
                                ),
                        )
                        .child(
                            div()
                                .px(SPACE_2H)
                                .py(SPACE_2)
                                .flex()
                                .items_center()
                                .gap(SPACE_2)
                                .border_t_1()
                                .border_color(t.border)
                                .child(div().flex_1())
                                .child(
                                    Button::new("issue-new-cancel", "Cancel", Variant::Outline)
                                        .on_click(cx.listener(|area, _, _, cx| {
                                            area.issues.composing = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new(
                                        "issue-new-open",
                                        if composing.busy {
                                            "Opening…"
                                        } else {
                                            "Open issue"
                                        },
                                        Variant::Primary,
                                    )
                                    .disabled(!ready)
                                    .on_click(cx.listener(|area, _, _, cx| area.create_issue(cx))),
                                ),
                        ),
                ),
        )
        .into_any_element()
}
