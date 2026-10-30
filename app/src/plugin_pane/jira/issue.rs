//! The issue opened beside the list: its fields, actions, activity and comments.

use super::*;
use sikemux_core::server::{BoardAddTicketParams, BoardTicketParams, TicketCard, Tracker};

use crate::theme::{SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_XS, TEXT_XS_H};

/// The people an issue can be given to, each that does not read left out and said.
fn people(found: Value) -> Vec<Person> {
    let Value::Array(found) = found else {
        eprintln!("sikemux: the people Jira can assign did not come as a list");
        return Vec::new();
    };
    found
        .into_iter()
        .filter_map(|person| match serde_json::from_value(person) {
            Ok(person) => Some(person),
            Err(error) => {
                eprintln!("sikemux: a person Jira can assign did not read: {error}");
                None
            }
        })
        .collect()
}

impl Jira {
    /// The commits in the project in front that name the issue, asked of the core again once
    /// the Git views see a newer commit.
    fn commits(
        &mut self,
        host: &Host,
        key: &str,
        cx: &mut Context<PluginPane>,
    ) -> Option<AnyElement> {
        let repo = host.git_repo(cx)?;
        let head = host
            .recent_commits(cx)
            .and_then(|log| log.first().map(|commit| commit.hash.clone()));
        let asked: Asked = (repo.clone(), head);
        let open = self.issue.as_mut()?;
        if open.commits.as_ref().is_none_or(|(at, _)| *at != asked) {
            open.commits = Some((asked.clone(), None));
            let call = host.commits_naming(&repo, key);
            cx.spawn(async move |pane, cx| {
                let found = call
                    .await
                    .ok()
                    .and_then(|value| serde_json::from_value::<Vec<GitCommit>>(value).ok());
                let _ = pane.update(cx, |pane, cx| {
                    let Some((jira, _)) = pane.parts::<Jira>() else {
                        return;
                    };
                    let kept = jira.issue.as_mut().and_then(|open| open.commits.as_mut());
                    if let Some((at, commits)) = kept {
                        if *at == asked {
                            *commits = found;
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
        }
        let named = open.commits.as_ref()?.1.as_ref()?;
        (!named.is_empty()).then(|| commit_list(named, cx))
    }

    pub(super) fn detail(
        &mut self,
        host: &mut Host,
        key: &str,
        site: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let read = host
            .view(
                "issueView",
                IssueRequest {
                    key: key.to_owned(),
                    site: site_of(site),
                },
                cx,
            )
            .read::<IssueView>();
        let view = match read {
            Ok(Some(view)) => view,
            Ok(None) => return crate::plugin_pane::empty(format!("Loading {key}"), cx),
            Err(failure) => return crate::plugin_pane::failure(&failure, cx),
        };
        let t = Tokens::get(cx).clone();
        let issue = &view.issue;
        let url = issue.url.clone();
        let (open_url, copy_url, key_url) = (url.clone(), url.clone(), url);
        let sent = view.clone();
        let head = div()
            .id("jira-detail-head")
            .px(SPACE_4)
            .pt(SPACE_3)
            .pb(SPACE_2H)
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(type_mark(issue.issue_type.as_deref(), cx))
                    .child(
                        div()
                            .id("jira-detail-key")
                            .cursor_pointer()
                            .tooltip(ui::tooltip("Open in browser"))
                            .child(ui::mono(issue.key.clone(), 12., cx).text_color(jira_blue()))
                            .on_click(move |_, _, cx| cx.open_url(&key_url)),
                    )
                    .child(
                        IconButton::new("jira-copy-link", Icon::Copy, "Copy link")
                            .small()
                            .on_click(listen::<Jira, _>(cx, move |_, host, _, _, cx| {
                                host.copy(copy_url.clone(), "the link", cx)
                            })),
                    )
                    .child(
                        IconButton::new("jira-open", Icon::Globe, "Open in browser")
                            .small()
                            .on_click(move |_, _, cx| cx.open_url(&open_url)),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("jira-send", "Send to agent board", Variant::Outline)
                            .icon(Icon::Agent)
                            .tooltip(
                                "Put this issue on the open project's board as a card for agents",
                            )
                            .on_click(listen::<Jira, _>(cx, move |_, host, _, _, cx| {
                                send_to_board(host, &sent, cx)
                            })),
                    )
                    .child(
                        IconButton::new("jira-close", Icon::Close, "Close the issue")
                            .small()
                            .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                                jira.open(None);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                ui::text(issue.summary.clone(), 15., FontWeight::SEMIBOLD)
                    .text_color(t.text_primary)
                    .whitespace_normal(),
            )
            .context_menu(ui::menu(issue_menu(issue, cx.weak_entity())));
        let mut style = host.reading(13., cx);
        let pane = cx.weak_entity();
        let (task_key, task_site) = (issue.key.clone(), site.to_owned());
        style.on_task = Some(Rc::new(move |index, text, done, _, cx| {
            let params = TaskRequest {
                key: task_key.clone(),
                index,
                text: text.to_owned(),
                done,
                site: site_of(&task_site),
            };
            let _ = pane.update(cx, |pane, cx| {
                if let Some((jira, host)) = pane.parts::<Jira>() {
                    jira.write(host, "Update the task", "setTask", params, cx);
                }
            });
        }));
        let pen = Pen::new(Rc::new(style));
        let actions = self.actions(&view, site, window, cx);
        let fields = self.fields(host, &view, cx);
        let description = self.description(&pen, &view, window, cx);
        let commits = self.commits(host, key, cx);
        let activity = self.activity(host, &pen, &view, site, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("jira-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(SPACE_4)
                    .py(SPACE_3)
                    .flex()
                    .flex_col()
                    .gap(SPACE_4)
                    .child(actions)
                    .child(fields)
                    .child(description)
                    .children(commits)
                    .child(activity),
            )
            .into_any_element()
    }

    fn actions(
        &mut self,
        view: &IssueView,
        site: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let issue = &view.issue;
        let busy = self.issue.as_ref().is_some_and(|issue| issue.busy);
        let picks = view
            .transitions
            .iter()
            .map(|transition| {
                let pick = ui::Pick::new(transition.id.clone(), transition.to.clone());
                if transition.name.eq_ignore_ascii_case(&transition.to) {
                    pick
                } else {
                    pick.hint(transition.name.clone())
                }
            })
            .collect();
        let at = |key: &str| (key.to_owned(), site.to_owned());
        let (move_key, move_site) = at(&issue.key);
        let ink = match issue.status_category.as_str() {
            "indeterminate" => jira_blue(),
            "done" => Tokens::get(cx).live,
            _ => Tokens::get(cx).text_tertiary,
        };
        let dot = div().size(px(8.)).flex_none().rounded_full().bg(ink);
        let status = ui::marked_select(
            "jira-transition",
            Some(dot.into_any_element()),
            issue.status.clone(),
            picks,
            None,
            busy || view.transitions.is_empty(),
            listen::<Jira, SharedString>(cx, move |jira, host, to, _, cx| {
                let params = TransitionRequest {
                    key: move_key.clone(),
                    to: Some(to.to_string()),
                    site: site_of(&move_site),
                };
                jira.write(host, "Move issue", "transition", params, cx);
            }),
            cx,
        );
        let assigned = issue.assignee.is_some();
        let assigning = self.issue.as_ref().is_some_and(|issue| issue.assigning);
        let parts = [
            (
                "jira-assign-me",
                Some(Icon::User),
                "Assign to me",
                busy,
                false,
            ),
            ("jira-unassign", None, "Unassign", busy || !assigned, false),
            ("jira-assign", None, "Assign…", busy, assigning),
        ];
        let t = Tokens::get(cx).clone();
        let group = div()
            .flex()
            .flex_none()
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.border)
            .overflow_hidden()
            .children(parts.into_iter().enumerate().map(
                |(at, (id, glyph, label, off, pressed))| {
                    let (me_key, me_site) = at_site(&issue.key, site);
                    div()
                        .id(id)
                        .h(px(26.))
                        .px(SPACE_2H)
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .when(at > 0, |part| part.border_l_1().border_color(t.border))
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(if off { t.text_tertiary } else { t.text_primary })
                        .when(pressed, |part| part.bg(t.surface_chosen))
                        .when(!off, |part| {
                            part.cursor_pointer()
                                .hover(move |part| part.bg(t.surface_hover))
                                .on_click(listen::<Jira, _>(
                                    cx,
                                    move |jira, host, _, _, cx| match at {
                                        0 => {
                                            let params = AssignRequest {
                                                key: me_key.clone(),
                                                account_id: None,
                                                me: true,
                                                site: site_of(&me_site),
                                            };
                                            jira.write(host, "Assign", "assign", params, cx);
                                        }
                                        1 => {
                                            let params = AssignRequest {
                                                key: me_key.clone(),
                                                account_id: None,
                                                me: false,
                                                site: site_of(&me_site),
                                            };
                                            jira.write(host, "Unassign", "assign", params, cx);
                                        }
                                        _ => {
                                            if let Some(issue) = &mut jira.issue {
                                                issue.assigning = !issue.assigning;
                                                issue.people.clear();
                                            }
                                            cx.notify();
                                        }
                                    },
                                ))
                        })
                        .children(
                            glyph.map(|glyph| ui::icon(glyph, 12.).text_color(t.text_tertiary)),
                        )
                        .child(label)
                },
            ));
        let row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_1H)
            .child(status)
            .child(group);
        let finder = self
            .issue
            .as_mut()
            .filter(|open| open.assigning)
            .map(|open| {
                let find = field(
                    &mut open.assign_find,
                    &mut self.typing,
                    "Find a person",
                    false,
                    (Some(Jira::find_people), None),
                    window,
                    cx,
                );
                let chips = open.people.iter().enumerate().map(|(index, person)| {
                    let (key, site) = at(&issue.key);
                    let account = person.account_id.clone();
                    Button::new(
                        ("jira-person", index),
                        person.name.clone(),
                        Variant::Outline,
                    )
                    .on_click(listen::<Jira, _>(
                        cx,
                        move |jira, host, _, _, cx| {
                            if let Some(issue) = &mut jira.issue {
                                issue.assigning = false;
                            }
                            let params = AssignRequest {
                                key: key.clone(),
                                account_id: Some(account.clone()),
                                me: false,
                                site: site_of(&site),
                            };
                            jira.write(host, "Assign", "assign", params, cx);
                        },
                    ))
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .child(ui::Field::new(&find).icon(Icon::Search))
                    .child(div().flex().flex_wrap().gap(SPACE_1).children(chips))
            });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .child(row)
            .children(finder)
            .into_any_element()
    }

    fn find_people(&mut self, host: &mut Host, query: &str, cx: &mut Context<PluginPane>) {
        let Some(key) = self.open_key().map(str::to_owned) else {
            return;
        };
        let params = AssignableRequest {
            key: key.clone(),
            query: query.to_owned(),
            site: site_of(&self.shown_site),
        };
        let call = host.call("assignable", params);
        let asked = query.to_owned();
        after::<Jira, _>(
            call,
            move |jira, _, found, _| {
                let still = jira
                    .issue
                    .as_mut()
                    .filter(|issue| issue.assign_find.is_some() && issue.key == key);
                if let (Some(issue), Ok(found)) = (still, found) {
                    issue.people = people(found);
                }
                let _ = asked;
            },
            cx,
        );
    }

    fn fields(&self, host: &Host, view: &IssueView, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let issue = &view.issue;
        let fact = |label: &'static str, value: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .min_h(px(34.))
                .px(SPACE_3)
                .py(SPACE_1)
                .child(
                    div()
                        .w(px(84.))
                        .flex_none()
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().flex().items_center().child(value))
        };
        let person = |who: Option<&Person>| {
            let name = who.map_or("Unassigned", |person| person.name.as_str());
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .text_size(crate::theme::ui_text(TEXT_XS_H))
                .text_color(t.text_primary)
                .child(host.face(
                    name,
                    who.and_then(|person| person.avatar_url.as_deref()),
                    20.,
                    cx,
                ))
                .child(name.to_owned())
                .into_any_element()
        };
        let plain = |text: String| {
            ui::text(text, 12.5, FontWeight::NORMAL)
                .text_color(t.text_primary)
                .whitespace_normal()
                .into_any_element()
        };
        let mut facts = div()
            .flex()
            .flex_col()
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.border)
            .bg(t.surface_composer)
            .overflow_hidden()
            .child(fact("Assignee", person(issue.assignee.as_ref())));
        if let Some(reporter) = &view.reporter {
            facts = facts.child(fact("Reporter", person(Some(reporter))));
        }
        if let Some(priority) = &issue.priority {
            facts = facts.child(fact(
                "Priority",
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(priority_mark(priority, cx))
                    .child(plain(priority.clone()))
                    .into_any_element(),
            ));
        }
        if let Some(due) = &issue.due {
            facts = facts.child(fact(
                "Due",
                due_chip(due, issue.status_category == "done", true, cx),
            ));
        }
        if let Some(parent) = &issue.parent {
            facts = facts.child(fact("Parent", epic_chip(parent, cx)));
        }
        if let Some(kind) = &issue.issue_type {
            facts = facts.child(fact(
                "Type",
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(type_mark(Some(kind), cx))
                    .child(plain(kind.clone()))
                    .into_any_element(),
            ));
        }
        if let Some(sprint) = &issue.sprint {
            facts = facts.child(fact("Sprint", plain(sprint.clone())));
        }
        if !view.labels.is_empty() {
            facts = facts.child(fact(
                "Labels",
                div()
                    .flex()
                    .flex_wrap()
                    .gap(SPACE_1)
                    .children(view.labels.iter().map(|label| tag(label, cx)))
                    .into_any_element(),
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .child(ui::caps("Details", cx))
            .child(facts)
            .into_any_element()
    }

    fn description(
        &mut self,
        pen: &Pen,
        view: &IssueView,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let body = if view.description.trim().is_empty() {
            ui::meta("No description.", cx).into_any_element()
        } else {
            let doc = self.markdown(&view.description);
            pen.markdown("jira-description", &doc, window, cx)
        };
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .text_color(t.text_primary)
            .child(ui::caps("Description", cx))
            .child(body)
            .into_any_element()
    }

    /// The issue's comments and edits, newest first, with a box for a new comment on top.
    fn activity(
        &mut self,
        host: &Host,
        pen: &Pen,
        view: &IssueView,
        site: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let chosen = SHOWN
            .iter()
            .position(|shown| *shown == self.shown)
            .unwrap_or_default();
        let switch = Segmented::words(
            "jira-activity",
            vec![
                ("All".into(), None),
                ("Comments".into(), Some(view.comment_count)),
                ("History".into(), None),
            ],
            chosen,
        )
        .on_select(listen::<Jira, usize>(cx, |jira, _, index, _, cx| {
            jira.shown = SHOWN[*index];
            cx.notify();
        }));
        let composer = (self.shown != Shown::History).then(|| {
            let draft = self
                .draft
                .get_or_insert_with(|| {
                    cx.new(|cx| {
                        TextareaState::new(window, cx)
                            .auto_grow(3, 12)
                            .placeholder("Add a comment")
                    })
                })
                .clone();
            let written = !draft.read(cx).value().trim().is_empty();
            let (key, site) = (view.issue.key.clone(), site.to_owned());
            div()
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .child(
                    div()
                        .px(SPACE_2H)
                        .py(SPACE_2)
                        .text_size(crate::theme::ui_text(TEXT_XS_H))
                        .child(
                            Textarea::new(&draft)
                                .appearance(false)
                                .text_size(crate::theme::ui_text(TEXT_XS_H)),
                        ),
                )
                .child(
                    div()
                        .px(SPACE_2)
                        .py(SPACE_1H)
                        .flex()
                        .justify_end()
                        .border_t_1()
                        .border_color(t.border)
                        .child(
                            Button::new(
                                "jira-comment",
                                if self.commenting {
                                    "Sending…"
                                } else {
                                    "Comment"
                                },
                                Variant::Primary,
                            )
                            .disabled(self.commenting || !written)
                            .on_click(listen::<Jira, _>(cx, move |jira, host, _, _, cx| {
                                jira.comment(host, &key, &site, cx)
                            })),
                        ),
                )
        });
        let entries: Vec<&Activity> = view
            .activity
            .iter()
            .filter(|entry| {
                matches!(
                    (self.shown, entry),
                    (Shown::All, _)
                        | (Shown::Comments, Activity::Comment { .. })
                        | (Shown::History, Activity::Change { .. })
                )
            })
            .collect();
        let shown_comments = view
            .activity
            .iter()
            .filter(|entry| matches!(entry, Activity::Comment { .. }))
            .count() as u64;
        let hidden = view.comment_count.saturating_sub(shown_comments);
        let mut part = div()
            .flex()
            .flex_col()
            .gap(SPACE_2H)
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(ui::caps("Activity", cx))
                    .child(div().flex_1())
                    .child(switch),
            )
            .children(composer);
        if entries.is_empty() {
            part = part.child(ui::meta(
                if self.shown == Shown::History {
                    "No changes yet."
                } else {
                    "No comments yet."
                },
                cx,
            ));
        }
        for (index, entry) in entries.into_iter().enumerate() {
            part =
                part.child(match entry {
                    Activity::Comment { comment } => {
                        let doc = self.markdown(&comment.body);
                        let body = pen.markdown(("jira-comment-body", index), &doc, window, cx);
                        said(
                            host,
                            &comment.author,
                            comment.author_avatar_url.as_deref(),
                            &comment.created,
                            body,
                            cx,
                        )
                    }
                    Activity::Change { change } => {
                        let lines = div().flex().flex_col().gap(SPACE_1).children(
                            change.fields.iter().map(|field| {
                                let mut line = div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(SPACE_1H)
                                    .text_size(crate::theme::ui_text(TEXT_XS))
                                    .text_color(t.text_secondary)
                                    .child("changed")
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.text_primary)
                                            .child(field.field.clone()),
                                    );
                                if field.from.is_some() || field.to.is_some() {
                                    line = line
                                        .child(
                                            div()
                                                .line_through()
                                                .text_color(t.text_tertiary)
                                                .child(field.from.clone().unwrap_or("None".into())),
                                        )
                                        .child("→")
                                        .child(
                                            div()
                                                .text_color(t.text_primary)
                                                .child(field.to.clone().unwrap_or("None".into())),
                                        );
                                }
                                line
                            }),
                        );
                        said(
                            host,
                            &change.author,
                            change.author_avatar_url.as_deref(),
                            &change.created,
                            lines.into_any_element(),
                            cx,
                        )
                    }
                });
        }
        if self.shown != Shown::History && hidden > 0 {
            let comments = if hidden == 1 { "comment" } else { "comments" };
            part = part.child(ui::meta(
                format!(
                    "{hidden} older {comments} not shown; open the issue in Jira to read them."
                ),
                cx,
            ));
        }
        part.into_any_element()
    }

    fn comment(&mut self, host: &Host, key: &str, site: &str, cx: &mut Context<PluginPane>) {
        let Some(draft) = self.draft.clone() else {
            return;
        };
        let body = draft.read(cx).value().trim().to_owned();
        if body.is_empty() || self.commenting {
            return;
        }
        self.commenting = true;
        let call = host.call(
            "comment",
            CommentRequest {
                key: key.to_owned(),
                body,
                site: site_of(site),
            },
        );
        let pane = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let done = call.await;
            cx.update(|cx| {
                let window = cx.active_window();
                let _ = pane.update(cx, |pane, cx| {
                    let Some((jira, host)) = pane.parts::<Jira>() else {
                        return;
                    };
                    jira.commenting = false;
                    match done {
                        Ok(_) => {
                            if let (Some(draft), Some(window)) = (&jira.draft, window) {
                                let draft = draft.clone();
                                let _ = window.update(cx, |_, window, cx| {
                                    draft.update(cx, |draft, cx| draft.set_value("", window, cx))
                                });
                            }
                        }
                        Err(failure) => host.said(&failure.titled("Comment"), cx),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }
}

/// An issue's key and its site, for a click to carry.
fn at_site(key: &str, site: &str) -> (String, String) {
    (key.to_owned(), site.to_owned())
}

/// Puts the issue on the front project's board as a card for agents, its open subtasks
/// under it.
fn send_to_board(host: &Host, view: &IssueView, cx: &mut App) {
    let ticket = |issue: &IssueSummary| BoardTicketParams {
        key: issue.key.clone(),
        url: Some(issue.url.clone()),
        source: issue.issue_type.clone(),
        provider: Some(Tracker::Jira),
    };
    let Some((folder, name)) = host.front_project(cx) else {
        host.notify(toast::error("Open a project to put this on its board"), cx);
        return;
    };
    let subtasks = view
        .subtasks
        .iter()
        .filter(|sub| sub.status_category != "done")
        .map(|sub| TicketCard {
            title: sub.summary.clone(),
            details: String::new(),
            ticket: Some(ticket(sub)),
        })
        .collect();
    let params = BoardAddTicketParams {
        project: folder,
        title: view.issue.summary.clone(),
        details: view.description.clone(),
        ticket: ticket(&view.issue),
        subtasks,
        blocked_by: view.blocked_by.clone(),
        blocks: view.blocks.clone(),
    };
    let put = format!("Put {} on {name}'s board", view.issue.key);
    host.add_board_ticket(params, put, cx);
}

/// Commits that name the issue, a line each.
fn commit_list(named: &[GitCommit], cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(ui::caps("Commits in this project", cx))
        .children(named.iter().map(|commit| {
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .min_w_0()
                .child(ui::mono(commit.hash.clone(), 11., cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_primary)
                        .child(commit.subject.clone()),
                )
                .child(ui::meta(commit.author.clone(), cx))
        }))
        .into_any_element()
}
