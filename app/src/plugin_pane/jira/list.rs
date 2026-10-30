//! The issues of the list picked, one row each.

use super::*;

use crate::theme::{SPACE_1, SPACE_1H, SPACE_2, SPACE_3};

impl Jira {
    pub(super) fn issues(
        &mut self,
        host: &mut Host,
        site: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let Some(asked) = self.listing.list.asked() else {
            return div().into_any_element();
        };
        let read = host
            .view(
                "listView",
                ListRequest {
                    list: asked,
                    site: site_of(site),
                },
                cx,
            )
            .read::<IssueList>();
        let page = match read {
            Ok(None) => return crate::plugin_pane::loading(cx),
            Err(failure) => return crate::plugin_pane::failure(&failure, cx),
            Ok(Some(page)) if page.issues.is_empty() => {
                return crate::plugin_pane::empty(self.listing.list.empty(), cx)
            }
            Ok(Some(page)) => page,
        };
        self.listing
            .statuses
            .retain(|status| page.statuses.iter().any(|each| each.status == *status));
        let t = Tokens::get(cx).clone();
        let chips = (page.statuses.len() > 1).then(|| {
            div()
                .px(SPACE_3)
                .py(SPACE_2)
                .flex()
                .flex_wrap()
                .gap(SPACE_1)
                .border_b_1()
                .border_color(t.border)
                .child(
                    Toggle::new(
                        "jira-status-all",
                        format!("All {}", page.issues.len()),
                        self.listing.statuses.is_empty(),
                    )
                    .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                        jira.listing.statuses.clear();
                        cx.notify();
                    })),
                )
                .children(page.statuses.iter().enumerate().map(|(index, count)| {
                    let status = count.status.clone();
                    Toggle::new(
                        ("jira-status", index),
                        format!("{} {}", count.status, count.count),
                        self.listing.statuses.contains(&count.status),
                    )
                    .on_click(listen::<Jira, _>(
                        cx,
                        move |jira, _, _, _, cx| {
                            if !jira.listing.statuses.remove(&status) {
                                jira.listing.statuses.insert(status.clone());
                            }
                            cx.notify();
                        },
                    ))
                }))
        });
        let rows: Vec<AnyElement> = page
            .issues
            .iter()
            .filter(|issue| {
                self.listing.statuses.is_empty() || self.listing.statuses.contains(&issue.status)
            })
            .enumerate()
            .map(|(index, issue)| {
                let chosen = self.open_key() == Some(issue.key.as_str());
                issue_row(host, issue, index, chosen, cx)
            })
            .collect();
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(chips)
            .child(
                div()
                    .id("jira-issues")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}

fn issue_row(
    host: &Host,
    issue: &IssueSummary,
    index: usize,
    chosen: bool,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = issue.key.clone();
    let who = issue
        .assignee
        .as_ref()
        .map_or("Unassigned", |person| person.name.as_str());
    let avatar = issue
        .assignee
        .as_ref()
        .and_then(|person| person.avatar_url.as_deref());
    div()
        .id(("jira-issue", index))
        .px(SPACE_3)
        .py(SPACE_2)
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .when(index > 0, |row| row.border_t_1().border_color(t.border))
        .when(chosen, |row| row.bg(t.surface_chosen))
        .cursor_pointer()
        .hover(move |row| row.bg(t.surface_hover))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(type_mark(issue.issue_type.as_deref(), cx))
                .child(ui::mono(issue.key.clone(), 11., cx).text_color(t.text_secondary))
                .child(status_chip(&issue.status, &issue.status_category, cx)),
        )
        .child(
            ui::text(issue.summary.clone(), 12.5, FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .whitespace_normal()
                .text_ellipsis()
                .line_clamp(2),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .children(
                    issue
                        .priority
                        .as_deref()
                        .map(|priority| priority_mark(priority, cx)),
                )
                .children(
                    issue
                        .sprint
                        .clone()
                        .map(|sprint| ui::meta(sprint, cx).min_w_0().truncate()),
                )
                .child(div().flex_1())
                .child(ui::meta(who.to_owned(), cx).min_w_0().truncate())
                .child(host.face(who, avatar, 18., cx)),
        )
        .on_click(listen::<Jira, _>(cx, move |jira, _, _, _, cx| {
            jira.open(Some(key.clone()));
            cx.notify();
        }))
        .context_menu(ui::menu(issue_menu(issue, cx.weak_entity())))
        .into_any_element()
}

/// Ways to take an issue out of Sikemux: to the browser, or to the clipboard in the shapes
/// people paste.
pub(super) fn issue_menu(
    issue: &IssueSummary,
    pane: WeakEntity<PluginPane>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let (key, summary, url) = (issue.key.clone(), issue.summary.clone(), issue.url.clone());
    move |popup, _, _| {
        let copies = [
            ("Copy link", url.clone(), "the link"),
            ("Copy key", key.clone(), "the key"),
            (
                "Copy key and title",
                format!("{key} {summary}"),
                "the key and title",
            ),
            (
                "Copy as Markdown link",
                format!("[{key}]({url}) {summary}"),
                "the Markdown link",
            ),
        ];
        let url = url.clone();
        let mut popup = popup
            .item(
                PopupMenuItem::new("Open in browser")
                    .icon(Icon::Globe)
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .separator();
        for (label, text, what) in copies {
            let pane = pane.clone();
            popup = popup.item(PopupMenuItem::new(label).icon(Icon::Copy).on_click(
                move |_, _, cx| {
                    let _ = pane.update(cx, |pane, cx| pane.host.copy(text.clone(), what, cx));
                },
            ));
        }
        popup
    }
}
