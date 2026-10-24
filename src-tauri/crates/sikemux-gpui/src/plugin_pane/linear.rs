//! Linear's pane: the person's open issues or a search, and beside them the issue picked, to
//! move, take, comment on or hand to an agent.

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClipboardItem, Context, Div, Entity, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    Subscription, Window,
};
use gpui_transcript::{Markdown, Pen};
use serde_json::{json, Value};
use sikemux_wire::linear::{IssueList, IssueRow, IssueView};

use super::{Failure, Host, Panel, PluginPane};
use crate::codehost::ago;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui::{self, Button, IconButton, Pick, Variant};

const PITCH: &str = "The issues you are working on, beside the code.";

pub const REGISTRATION: super::Registration = super::Registration {
    plugin: "sikemux.linear",
    surface: "issues",
    pane: super::boxed::<Linear>,
    top_bar: None,
    palettes: &[],
};

#[derive(Default)]
pub struct Linear {
    picked: Option<Entity<Picked>>,
    search: Option<Entity<InputState>>,
    /// The comment being written, and the issue it is for.
    draft: Option<(String, Entity<TextareaState>)>,
    words: Option<Words>,
    _typing: Vec<Subscription>,
}

/// What the person picked: the search, the issue open beside the list, and whether a change
/// to it is on its way.
#[derive(Default)]
struct Picked {
    query: String,
    issue: Option<String>,
    busy: bool,
}

/// The open issue's description and comments as markdown, kept while the issue reads the same.
struct Words {
    of: IssueView,
    description: Option<Rc<Markdown>>,
    comments: Vec<Rc<Markdown>>,
}

impl Words {
    fn of(issue: IssueView) -> Self {
        let render = |text: &str| Rc::new(crate::editor::preview::render(text));
        Self {
            description: issue.description.as_deref().map(render),
            comments: issue
                .comments
                .iter()
                .map(|comment| render(&comment.body))
                .collect(),
            of: issue,
        }
    }
}

impl Panel for Linear {
    fn render(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        if host.signed_in(cx) == Some(false) {
            return super::signed_out("Linear", PITCH, cx);
        }
        let picked = self.picked(window, cx);
        let (query, open) = {
            let picked = picked.read(cx);
            (picked.query.clone(), picked.issue.clone())
        };
        let list = host
            .view("listView", json!({ "query": query }), cx)
            .read::<IssueList>();
        if let Err(failure) = &list {
            if failure.signed_out() {
                return super::signed_out("Linear", PITCH, cx);
            }
        }
        let column = self.list(list, &picked, open.as_deref(), cx);
        let detail = match open {
            None => super::empty("Pick an issue to read it, comment on it or move it.", cx),
            Some(id) => self.issue(host, &id, &picked, window, cx),
        };
        div()
            .size_full()
            .flex()
            .child(column)
            .child(div().flex_1().min_w_0().h_full().child(detail))
            .into_any_element()
    }
}

impl Linear {
    fn picked(&mut self, window: &mut Window, cx: &mut Context<PluginPane>) -> Entity<Picked> {
        let picked = self
            .picked
            .get_or_insert_with(|| cx.new(|_| Picked::default()))
            .clone();
        if self.search.is_none() {
            let search = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Search issues, or an id such as ENG-123")
            });
            let searching = picked.clone();
            self._typing.push(
                cx.subscribe(&search, move |_, search, event: &InputEvent, cx| {
                    let typed = search.read(cx).value().trim().to_owned();
                    let query = match event {
                        InputEvent::PressEnter { .. } => typed,
                        InputEvent::Change if typed.is_empty() => typed,
                        _ => return,
                    };
                    searching.update(cx, |picked, _| picked.query = query);
                    cx.notify();
                }),
            );
            self.search = Some(search);
        }
        picked
    }

    fn list(
        &self,
        list: Result<Option<Rc<IssueList>>, Failure>,
        picked: &Entity<Picked>,
        open: Option<&str>,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let body = match list {
            Ok(None) => super::loading(cx),
            Err(failure) => super::failure(&failure, cx),
            Ok(Some(list)) if list.issues.is_empty() => super::empty(list.empty.clone(), cx),
            Ok(Some(list)) => div()
                .id("linear-issues")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(6.))
                .pb(px(8.))
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .px(px(8.))
                        .pt(px(10.))
                        .pb(px(4.))
                        .child(ui::meta(list.heading.clone(), cx)),
                )
                .children(list.issues.iter().enumerate().map(|(index, issue)| {
                    row(index, issue, open == Some(&issue.identifier), picked, cx)
                }))
                .into_any_element(),
        };
        div()
            .w(px(320.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.border)
            .children(
                self.search
                    .as_ref()
                    .map(|search| ui::Field::new(search).icon(Icon::Search).flush()),
            )
            .child(body)
            .into_any_element()
    }

    fn issue(
        &mut self,
        host: &mut Host,
        id: &str,
        picked: &Entity<Picked>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let read = host
            .view("issueView", json!({ "id": id }), cx)
            .read::<IssueView>();
        let issue = match read {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
            Ok(Some(issue)) => issue,
        };
        if self.words.as_ref().is_none_or(|words| words.of != *issue) {
            self.words = Some(Words::of((*issue).clone()));
        }
        let draft = self.draft(id, window, cx);
        let words = self.words.as_ref().expect("read just now");
        let issue = &words.of;
        let t = Tokens::get(cx).clone();
        let busy = picked.read(cx).busy;
        let pen = Pen::new(Rc::new(crate::editor::preview::reading_style(13., &t)));
        let description = match &words.description {
            Some(description) => pen.markdown("linear-description", description, window, cx),
            None => note("No description.", cx),
        };
        let comments: Vec<AnyElement> = issue
            .comments
            .iter()
            .zip(&words.comments)
            .enumerate()
            .map(|(index, (comment, body))| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                ui::text(comment.author.clone(), 12., FontWeight::SEMIBOLD)
                                    .text_color(t.text_primary),
                            )
                            .child(ui::meta(ago(&comment.created_at), cx)),
                    )
                    .child(pen.markdown(("linear-comment", index), body, window, cx))
                    .into_any_element()
            })
            .collect();
        div()
            .id("linear-issue")
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(head(issue, host, picked, cx))
            .child(
                ui::text(issue.title.clone(), 16., FontWeight::SEMIBOLD).text_color(t.text_primary),
            )
            .child(facts(issue, picked, busy, cx))
            .child(description)
            .child(ui::text("Comments", 12., FontWeight::SEMIBOLD).text_color(t.text_secondary))
            .when(comments.is_empty(), |page| {
                page.child(note("No comments yet.", cx))
            })
            .children(comments)
            .child(composer(issue, &draft, picked, busy, cx))
            .into_any_element()
    }

    fn draft(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> Entity<TextareaState> {
        if let Some((of, draft)) = &self.draft {
            if of == id {
                return draft.clone();
            }
        }
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 10)
                .placeholder("Comment")
        });
        self._typing
            .push(cx.subscribe(&draft, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        self.draft = Some((id.to_owned(), draft.clone()));
        draft
    }
}

fn row(
    index: usize,
    issue: &IssueRow,
    open: bool,
    picked: &Entity<Picked>,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let hover = t.surface_hover;
    let (picked, id) = (picked.clone(), issue.identifier.clone());
    div()
        .id(("linear-row", index))
        .px(px(8.))
        .py(px(6.))
        .rounded(theme::RADIUS_2)
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .when(open, |row| row.bg(t.surface_chosen))
        .when(!open, |row| row.hover(move |row| row.bg(hover)))
        .on_click(cx.listener(move |_, _, _, cx| {
            picked.update(cx, |picked, _| picked.issue = Some(id.clone()));
            cx.notify();
        }))
        .child(dot(&issue.state.color, cx))
        .child(ui::mono(issue.identifier.clone(), 11., cx).flex_none())
        .child(
            ui::text(issue.title.clone(), 12.5, FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .flex_1()
                .min_w_0()
                .truncate(),
        )
        .child(ui::meta(issue.state.name.clone(), cx).flex_none())
        .into_any_element()
}

/// The issue's id, with opening it in Linear, copying its branch, handing it to an agent and
/// closing it.
fn head(
    issue: &IssueView,
    host: &Host,
    picked: &Entity<Picked>,
    cx: &mut Context<PluginPane>,
) -> Div {
    let (url, branch, closing) = (issue.url.clone(), issue.branch_name.clone(), picked.clone());
    let agents = Rc::new(host.agents(cx));
    let picks = agents
        .iter()
        .enumerate()
        .map(|(at, agent)| Pick::new(at.to_string(), agent.label.clone()))
        .collect();
    let (pane, text) = (cx.entity(), issue.for_agent.clone());
    let send = ui::select(
        "linear-send",
        "Send to agent",
        picks,
        None,
        agents.is_empty(),
        move |key, window, cx| {
            let Some(agent) = key.parse::<usize>().ok().and_then(|at| agents.get(at)) else {
                return;
            };
            let (to, text) = (agent.to.clone(), text.clone());
            pane.update(cx, |pane, cx| pane.host.send_to_agent(to, text, window, cx));
        },
        cx,
    );
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(ui::mono(issue.identifier.clone(), 12., cx))
        .child(div().flex_1())
        .child(
            IconButton::new("linear-open", Icon::Globe, "Open in Linear")
                .small()
                .on_click(move |_, _, cx| cx.open_url(&url)),
        )
        .child(
            IconButton::new("linear-branch", Icon::Copy, "Copy the branch name")
                .small()
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(branch.clone()))
                }),
        )
        .child(send)
        .child(
            IconButton::new("linear-close", Icon::Close, "Close the issue")
                .small()
                .on_click(cx.listener(move |_, _, _, cx| {
                    closing.update(cx, |picked, _| picked.issue = None);
                    cx.notify();
                })),
        )
}

/// Where the issue stands, who has it, its priority and labels.
fn facts(
    issue: &IssueView,
    picked: &Entity<Picked>,
    busy: bool,
    cx: &mut Context<PluginPane>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let picks = issue
        .states
        .iter()
        .map(|state| Pick::new(state.id.clone(), state.name.clone()))
        .collect();
    let (pane, moving, id, states, now) = (
        cx.entity(),
        picked.clone(),
        issue.identifier.clone(),
        issue.states.clone(),
        issue.state.id.clone(),
    );
    let state = ui::select(
        "linear-state",
        issue.state.name.clone(),
        picks,
        Some(issue.state.id.clone().into()),
        busy,
        move |key, _, cx| {
            let Some(state) = states.iter().find(|state| state.id == key.as_ref()) else {
                return;
            };
            if state.id == now {
                return;
            }
            let params = json!({ "id": id, "state": state.name });
            let picked = moving.clone();
            pane.update(cx, |pane, cx| write(pane, picked, "update", params, cx));
        },
        cx,
    );
    let take = issue.assignee.is_none().then(|| {
        let (picked, id) = (picked.clone(), issue.identifier.clone());
        Button::new("linear-take", "Assign to me", Variant::Outline)
            .disabled(busy)
            .on_click(cx.listener(move |pane, _, _, cx| {
                let params = json!({ "id": id, "assignee": "me" });
                write(pane, picked.clone(), "update", params, cx)
            }))
    });
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.))
        .child(state)
        .child(ui::meta(
            issue
                .assignee
                .clone()
                .unwrap_or_else(|| "Unassigned".into()),
            cx,
        ))
        .children(take)
        .children(
            issue
                .priority
                .clone()
                .map(|priority| ui::meta(priority, cx)),
        )
        .children(issue.labels.iter().map(|label| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(7.))
                .rounded_full()
                .border_1()
                .border_color(t.border)
                .text_size(theme::ui_text(11.))
                .text_color(t.text_secondary)
                .child(dot(&label.color, cx))
                .child(label.name.clone())
        }))
}

fn composer(
    issue: &IssueView,
    draft: &Entity<TextareaState>,
    picked: &Entity<Picked>,
    busy: bool,
    cx: &mut Context<PluginPane>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let written = !draft.read(cx).value().trim().is_empty();
    let (draft_now, picked, id) = (draft.clone(), picked.clone(), issue.identifier.clone());
    div()
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(t.border)
        .bg(t.surface)
        .child(
            div().px(px(12.)).py(px(8.)).child(
                Textarea::new(draft)
                    .appearance(false)
                    .text_size(theme::ui_text(13.)),
            ),
        )
        .child(
            div()
                .px(px(10.))
                .py(px(8.))
                .flex()
                .border_t_1()
                .border_color(t.border)
                .child(div().flex_1())
                .child(
                    Button::new(
                        "linear-comment-send",
                        if busy { "Saving…" } else { "Comment" },
                        Variant::Primary,
                    )
                    .disabled(busy || !written)
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        let body = draft_now.read(cx).value().trim().to_owned();
                        if body.is_empty() {
                            return;
                        }
                        draft_now.update(cx, |draft, cx| draft.set_value("", window, cx));
                        let params = json!({ "id": id, "body": body });
                        write(pane, picked.clone(), "comment", params, cx);
                    })),
                ),
        )
}

/// Calls the plugin's `method`, saying over the window when it fails; the core reads the
/// views again after it.
fn write(
    pane: &PluginPane,
    picked: Entity<Picked>,
    method: &'static str,
    params: Value,
    cx: &mut Context<PluginPane>,
) {
    picked.update(cx, |picked, _| picked.busy = true);
    let call = pane.host.call(method, params);
    cx.spawn(async move |pane, cx| {
        let answer = call.await;
        let _ = pane.update(cx, |pane, cx| {
            picked.update(cx, |picked, _| picked.busy = false);
            if let Err(failure) = answer {
                pane.host.said(&failure, cx);
            }
            cx.notify();
        });
    })
    .detach();
    cx.notify();
}

/// A state's or a label's colour, from the `#rrggbb` Linear gives.
fn dot(color: &str, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let ink = u32::from_str_radix(color.trim_start_matches('#'), 16)
        .map(|rgb| Hsla::from(gpui_kit::rgb(rgb)))
        .unwrap_or(t.text_tertiary);
    div().flex_none().size(px(8.)).rounded_full().bg(ink)
}

fn note(text: &'static str, cx: &App) -> AnyElement {
    ui::text(text, 12., FontWeight::NORMAL)
        .italic()
        .text_color(Tokens::get(cx).text_tertiary)
        .into_any_element()
}
