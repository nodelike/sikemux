//! The person's lists, spaces and starred filters down the side.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3H, TEXT_2XS};

impl Jira {
    pub(super) fn sidebar(
        &mut self,
        host: &mut Host,
        sites: &[String],
        site: &str,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let head = if sites.len() > 1 {
            let picks = sites
                .iter()
                .map(|host| ui::Pick::new(host.clone(), host.clone()))
                .collect();
            ui::select(
                "jira-site",
                site.to_owned(),
                picks,
                Some(site.to_owned().into()),
                false,
                listen::<Jira, SharedString>(cx, |jira, _, picked, _, cx| {
                    jira.site = picked.to_string();
                    jira.open(None);
                    if matches!(jira.listing.list, List::Filter { .. }) {
                        jira.show(List::Mine);
                    }
                    cx.notify();
                }),
                cx,
            )
        } else {
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(ui::icon(Icon::Jira, 16.).text_color(jira_blue()))
                .child(
                    ui::text(
                        site.trim_end_matches(".atlassian.net").to_owned(),
                        13.,
                        FontWeight::SEMIBOLD,
                    )
                    .text_color(t.text_primary)
                    .truncate(),
                )
                .into_any_element()
        };
        let search = field(
            &mut self.search,
            &mut self.typing,
            "Search",
            false,
            (
                None,
                Some(|jira, _, text, _| {
                    if !text.is_empty() {
                        jira.show(List::Search(text.to_owned()));
                    }
                }),
            ),
            window,
            cx,
        );
        let mut lists = div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(heading("Your work", cx));
        for (list, label, icon) in my_work() {
            lists = lists.child(self.list_row(list, label, icon, cx));
        }
        let spaces = host
            .view(
                "spacesView",
                SiteRequest {
                    site: site_of(site),
                },
                cx,
            )
            .read::<Spaces>()
            .ok()
            .flatten();
        let spaces_part = spaces
            .as_ref()
            .map(|spaces| self.spaces(spaces, window, cx));
        let filters = spaces.as_ref().map(|spaces| {
            let mut part = div().flex().flex_col().gap(px(1.));
            if !spaces.filters.is_empty() {
                part = part.child(heading("Starred filters", cx));
            }
            for filter in &spaces.filters {
                let list = List::Filter {
                    id: filter.id.clone(),
                    name: filter.name.clone(),
                    jql: filter.jql.clone(),
                };
                part = part.child(self.list_row(list, &filter.name, Icon::Star, cx));
            }
            part
        });
        let site_out = site.to_owned();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(SPACE_2H)
                    .pt(SPACE_2H)
                    .pb(SPACE_2)
                    .flex()
                    .flex_col()
                    .gap(SPACE_2)
                    .child(head)
                    .child(ui::Field::new(&search).icon(Icon::Search)),
            )
            .child(
                div()
                    .id("jira-lists")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(SPACE_1H)
                    .pb(SPACE_2)
                    .flex()
                    .flex_col()
                    .gap(SPACE_2H)
                    .child(lists)
                    .children(spaces_part)
                    .children(filters),
            )
            .child(
                div()
                    .px(SPACE_1H)
                    .py(SPACE_1H)
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        ui::Row::new("jira-sign-out", "Sign out")
                            .quiet()
                            .mark(22., ui::icon(Icon::LogOut, 14.).text_color(t.text_tertiary))
                            .on_click(listen::<Jira, _>(cx, move |jira, host, _, _, cx| {
                                let params = json!({ "site": site_out });
                                jira.write(host, "Sign out of Jira", "signOut", params, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn list_row(
        &self,
        list: List,
        label: &str,
        icon: Icon,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx);
        let id = SharedString::from(format!("jira-list-{}", list.id()));
        ui::Row::new(id, label.to_owned())
            .chosen(self.listing.list == list)
            .mark(22., ui::icon(icon, 14.).text_color(t.text_tertiary))
            .on_click(listen::<Jira, _>(cx, move |jira, _, _, _, cx| {
                jira.show(list.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn spaces(
        &mut self,
        listed: &Spaces,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        if listed.spaces.is_empty()
            && listed.other_boards.is_empty()
            && listed.boards_failed.is_none()
        {
            return div().into_any_element();
        }
        let active = match &self.listing.list {
            List::Project { key, .. } | List::Unassigned { key, .. } => Some(key.clone()),
            List::Board { id, .. } => listed
                .spaces
                .iter()
                .find(|space| space.boards.iter().any(|board| board.id == *id))
                .map(|space| space.project.key.clone()),
            _ => None,
        };
        let open_by_default = active.clone().or_else(|| {
            listed
                .spaces
                .iter()
                .find(|space| !space.boards.is_empty())
                .or(listed.spaces.first())
                .map(|space| space.project.key.clone())
        });
        let find = (listed.spaces.len() > SPACES_SHOWN).then(|| {
            field(
                &mut self.space_find,
                &mut self.typing,
                "Find a space or board",
                false,
                (None, None),
                window,
                cx,
            )
        });
        let wanted = words(&self.space_find, cx);
        let matching: Vec<_> = listed
            .spaces
            .iter()
            .filter(|space| {
                wanted.is_empty()
                    || space.project.name.to_lowercase().contains(&wanted)
                    || space.project.key.to_lowercase().contains(&wanted)
                    || space
                        .boards
                        .iter()
                        .any(|board| board.name.to_lowercase().contains(&wanted))
            })
            .collect();
        let shown: Vec<_> = matching
            .iter()
            .enumerate()
            .filter(|(index, space)| {
                !wanted.is_empty()
                    || self.every_space
                    || *index < SPACES_SHOWN
                    || Some(&space.project.key) == active.as_ref()
            })
            .map(|(_, space)| *space)
            .collect();
        let mut part = div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(heading("Spaces", cx))
            .children(find.map(|find| div().px(SPACE_0).pb(SPACE_1).child(ui::Field::new(&find))));
        for space in &shown {
            let key = space.project.key.clone();
            let open = !wanted.is_empty()
                || ((Some(&key) == open_by_default.as_ref()) != self.toggled.contains(&key));
            let flip = key.clone();
            part = part.child(
                ui::Row::new(
                    SharedString::from(format!("jira-space-{key}")),
                    space.project.name.clone(),
                )
                .mark(22., ui::initials(&space.project.name, &key, 16., cx))
                .status(Some(chevron(open, cx)))
                .on_click(listen::<Jira, _>(cx, move |jira, _, _, _, cx| {
                    if !jira.toggled.remove(&flip) {
                        jira.toggled.insert(flip.clone());
                    }
                    cx.notify();
                })),
            );
            if !open {
                continue;
            }
            let mut children = div().pl(SPACE_3H).flex().flex_col().gap(px(1.));
            for board in &space.boards {
                let list = List::Board {
                    id: board.id,
                    name: board.name.clone(),
                };
                children = children.child(self.list_row(list, &board.name, Icon::Board, cx));
            }
            let name = space.project.name.clone();
            let all = List::Project {
                key: key.clone(),
                name: name.clone(),
            };
            children = children
                .child(self.list_row(all, "All open", Icon::Stack, cx))
                .child(self.list_row(
                    List::Unassigned { key, name },
                    "Unassigned",
                    Icon::Inbox,
                    cx,
                ));
            part = part.child(children);
        }
        if wanted.is_empty() && !self.every_space && matching.len() > shown.len() {
            part = part.child(
                ui::Row::new(
                    "jira-every-space",
                    format!("Show all {} spaces", matching.len()),
                )
                .quiet()
                .on_click(listen::<Jira, _>(cx, |jira, _, _, _, cx| {
                    jira.every_space = true;
                    cx.notify();
                })),
            );
        }
        if !listed.other_boards.is_empty() {
            part = part.child(heading("Other boards", cx));
            for board in &listed.other_boards {
                let list = List::Board {
                    id: board.id,
                    name: board.name.clone(),
                };
                part = part.child(self.list_row(list, &board.name, Icon::Board, cx));
            }
        }
        if let Some(failed) = &listed.boards_failed {
            part = part.child(
                div()
                    .px(SPACE_2)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.danger)
                    .child(format!("{failed} Sign out and in again to retry.")),
            );
        }
        part.into_any_element()
    }
}
