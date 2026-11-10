//! How the pane draws.

use super::*;

/// The side column: the branch with its sync actions, then the changes and the commit box.
pub(crate) fn side(
    pane: &mut GitPane,
    window: &mut Window,
    cx: &mut Context<GitPane>,
) -> impl IntoElement {
    let order = pane.page_order();
    let (shown, start, motion) = pane.slide.shown(pane.page, &order);
    let cards = shown
        .into_iter()
        .map(|(page, place)| (side_page(pane, page, window, cx), place))
        .collect();
    let this = cx.entity().downgrade();
    let pages_row = track::row(
        "git-side-pages",
        &pane.slide.width(),
        cards,
        start,
        motion,
        track::Between::Seam(Tokens::get(cx).border),
        move |event, cx| {
            this.update(cx, |pane, cx| pane.swipe(event, cx))
                .unwrap_or(false)
        },
    )
    .flex_1()
    .min_h_0()
    .w_full();
    let commands = pane.commands_open.then(|| command_log(pane, cx));
    div()
        .w(relative(0.3))
        .min_w(px(320.))
        .max_w(px(400.))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            ui::panel(cx)
                .flex_1()
                .min_h_0()
                .child(head(pane, cx))
                .children(
                    pane.out_of_reach
                        .clone()
                        .map(|host| away_note(&host, cx).mx(SPACE_2)),
                )
                .child(pages_row)
                .child(pages(pane, cx)),
        )
        .children(commands)
}

/// One of the side column's pages, whole, with its filter.
pub(crate) fn side_page(
    pane: &mut GitPane,
    page: Page,
    window: &mut Window,
    cx: &mut Context<GitPane>,
) -> AnyElement {
    let count = (page == Page::History).then(|| pane.log().len());
    let filter = pane
        .filter_field(page, window, cx)
        .map(|field| filter_row(&field, count, cx));
    let body = div().size_full().flex().flex_col();
    match page {
        Page::Changes => body.child(changes(pane, filter, cx)),
        Page::History => body.children(filter).child(history_list(pane, cx)),
        Page::Branches => body.children(filter).child(branches_page(pane, cx)),
        Page::Pulls => body.children(filter).child(pulls_page(pane, cx)),
    }
    .into_any_element()
}

/// The git operations the pane ran, newest first.
pub(crate) fn command_log(pane: &GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let button = |id: &'static str, label: &'static str, tip: &'static str, disabled: bool| {
        BaseButton::new(id)
            .accessibility_label(tip)
            .tooltip(ui::tooltip(tip))
            .disabled(disabled)
            .px(SPACE_2)
            .py(SPACE_0)
            .border_1()
            .border_color(t.border)
            .text_size(crate::theme::ui_text(TEXT_3XS_H))
            .text_color(t.text_secondary)
            .when(disabled, |button| button.opacity(0.5))
            .hover(move |style| {
                style
                    .text_color(t.text_primary)
                    .border_color(t.border_strong)
            })
            .child(label)
    };
    let rows = pane.commands.iter().rev().map(|entry| {
        let (glyph, ink) = match entry.done {
            None => ("…", t.warn),
            Some(true) => ("✓", t.live),
            Some(false) => ("✗", t.danger),
        };
        div()
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .px(SPACE_3)
            .py(px(3.))
            .text_size(crate::theme::ui_text(TEXT_2XS_H))
            .child(
                div()
                    .w(px(60.))
                    .flex_none()
                    .text_size(crate::theme::ui_text(TEXT_3XS))
                    .text_color(t.text_tertiary)
                    .child(entry.time.clone()),
            )
            .child(div().w(px(12.)).flex_none().text_color(ink).child(glyph))
            .child(
                div()
                    .flex_none()
                    .text_color(t.text_primary)
                    .child(entry.label.clone()),
            )
            .children(entry.detail.clone().map(|detail| {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_tertiary)
                    .child(detail)
            }))
    });
    let empty = pane.commands.is_empty();
    div()
        .flex_none()
        .max_h(px(220.))
        .flex()
        .flex_col()
        .bg(t.surface)
        .border_t_1()
        .border_color(t.border)
        .font_family(theme::MONO)
        .text_color(t.text_secondary)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2H)
                .px(SPACE_3)
                .py(SPACE_1H)
                .border_b_1()
                .border_color(t.border)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .child(ui::kbd("@", cx))
                .child(
                    div()
                        .text_color(t.text_primary)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("command log"),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_tertiary)
                        .child(pane.commands.len().to_string()),
                )
                .child(div().flex_1())
                .child(
                    button("git-log-clear", "clear", "Clear log", empty).on_click(cx.listener(
                        |pane, _, _, cx| {
                            pane.commands.clear();
                            cx.notify();
                        },
                    )),
                )
                .child(
                    button("git-log-close", "close", "Hide log (@)", false).on_click(cx.listener(
                        |pane, _, _, cx| {
                            pane.commands_open = false;
                            cx.notify();
                        },
                    )),
                ),
        )
        .child(
            div()
                .id("git-log-rows")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .py(SPACE_1)
                .when(empty, |rows| {
                    rows.child(
                        div()
                            .px(SPACE_4)
                            .py(SPACE_3H)
                            .text_size(crate::theme::ui_text(TEXT_2XS_H))
                            .italic()
                            .text_color(t.text_tertiary)
                            .child("log empty — every git command will appear here"),
                    )
                })
                .children(rows),
        )
}

/// The icons at the side column's foot that turn its pages.
pub(crate) fn pages(pane: &GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let page = |id: &'static str, icon: Icon, label: &'static str, which: Page| {
        ui::IconButton::new(id, icon, label)
            .width(32.)
            .glyph(14.)
            .pressed(pane.page == which)
            .on_click(cx.listener(move |pane, _, _, cx| pane.turn(which, cx)))
    };
    div()
        .flex_none()
        .flex()
        .justify_center()
        .gap(SPACE_1)
        .px(SPACE_2)
        .py(SPACE_1)
        .border_t_1()
        .border_color(t.border)
        .child(page(
            "git-page-changes",
            Icon::File,
            "Changes — 1",
            Page::Changes,
        ))
        .child(page(
            "git-page-history",
            Icon::Commit,
            "History — 2",
            Page::History,
        ))
        .child(page(
            "git-page-branches",
            Icon::Git,
            "Branches — 3",
            Page::Branches,
        ))
        .child(page(
            "git-page-pulls",
            Icon::PullRequest,
            "Pull requests — 4",
            Page::Pulls,
        ))
}

pub(crate) fn history_list(pane: &mut GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    let inks = Inks::new(&t);
    let empty = pane.hashes.is_empty();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .when(empty, |page| {
            page.child(
                div()
                    .p(SPACE_5)
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child("No commits on this branch yet."),
            )
        })
        .child(
            list(pane.commit_rows.clone(), move |index, _, cx| {
                let cx = &*cx;
                let pane = view.read(cx);
                commit_row(&view, pane, index, &inks, cx)
            })
            .flex_1()
            .px(SPACE_1H)
            .py(SPACE_1),
        )
}

/// The way back to the repositories inside the project folder, while one of them is open.
pub(crate) fn other_repos(pane: &GitPane, cx: &mut Context<GitPane>) -> Option<impl IntoElement> {
    let bench = pane.bench.upgrade()?;
    let inside = bench
        .read(cx)
        .watched
        .project
        .as_deref()
        .is_some_and(|folder| folder != pane.repo && bench.read(cx).git_repos().is_some());
    inside.then(|| {
        ui::IconButton::new(
            "git-other-repos",
            Icon::Chevron,
            "Other repositories in this folder",
        )
        .turned(std::f32::consts::PI)
        .small()
        .on_click(move |_, _, cx| bench.update(cx, |bench, cx| bench.list_git_repos(cx)))
    })
}

pub(crate) fn head(pane: &mut GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let status = match &pane.overview {
        Some(Ok(overview)) => Some(&overview.status),
        _ => None,
    };
    let branch = match (&pane.overview, status) {
        (None, _) => "…".to_owned(),
        (_, Some(status)) if !status.branch.is_empty() => status.branch.clone(),
        _ => "detached".to_owned(),
    };
    let (ahead, behind) = status.map_or((0, 0), |status| (status.ahead, status.behind));
    let upstream = status.is_some_and(|status| status.upstream.is_some());
    let failed = matches!(pane.overview, Some(Err(_)));
    let busy = pane.busy.clone();
    let held = if busy.is_some() {
        Some("Wait for the running git command")
    } else if pane.branches.remotes_read && pane.branches.remotes.is_empty() {
        Some("This repository has no remote yet")
    } else {
        None
    };
    let count = |value: i32, colour: Hsla, arrow: &str| {
        (value > 0).then(|| {
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(colour)
                .child(format!("{arrow}{value}"))
        })
    };
    let sync = |id: &'static str,
                icon: Icon,
                label: &'static str,
                tip: String,
                number: i32,
                primary: bool,
                act: fn(&mut GitPane, &mut Context<GitPane>),
                cx: &mut Context<GitPane>| {
        let primary = primary && held.is_none();
        BaseButton::new(id)
            .accessibility_label(label)
            .tooltip(ui::tooltip(held.map_or(tip, String::from)))
            .h(px(30.))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_center()
            .gap(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .font_weight(FontWeight::MEDIUM)
            .map(|button| {
                if held.is_some() {
                    button.text_color(t.text_disabled)
                } else if primary {
                    button.bg(t.accent).text_color(t.on_accent)
                } else {
                    button
                        .text_color(t.text_secondary)
                        .hover(move |style| style.bg(t.surface_hover).text_color(t.text_primary))
                }
            })
            .when(held.is_none(), |button| {
                button.on_click(cx.listener(move |pane, _, _, cx| act(pane, cx)))
            })
            .child(ui::icon(icon, 12.).text_color(if primary {
                t.on_accent
            } else if held.is_some() {
                t.text_disabled
            } else {
                t.text_tertiary
            }))
            .child(label)
            .when(number > 0, |button| {
                button.child(
                    div()
                        .text_color(if primary { t.on_accent } else { t.accent })
                        .child(number.to_string()),
                )
            })
    };
    let plural = |count: i32| if count == 1 { "" } else { "s" };
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .flex_none()
        .pt(SPACE_2)
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .px(SPACE_2)
                .flex()
                .items_center()
                .gap(SPACE_1)
                .min_w_0()
                .children(other_repos(pane, cx))
                .child(branch_switch(pane, branch, cx))
                .children(count(ahead, t.live, "↑"))
                .children(count(behind, t.warn, "↓"))
                .child(div().flex_1())
                .when(failed, |title| {
                    title.child(
                        div()
                            .h(px(18.))
                            .px(SPACE_1H)
                            .flex()
                            .items_center()
                            .border_1()
                            .border_color(t.danger.opacity(0.32))
                            .rounded(px(4.))
                            .font_family(theme::MONO)
                            .text_size(crate::theme::ui_text(TEXT_2XS))
                            .text_color(t.danger)
                            .child("git error"),
                    )
                })
                .children(busy.map(|busy| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.warn)
                        .child(busy)
                }))
                .child(more_menu(pane, cx)),
        )
        .child(
            div()
                .flex()
                .border_t_1()
                .border_color(t.border)
                .child(sync(
                    "git-fetch",
                    Icon::Fetch,
                    "Fetch",
                    "Fetch all remotes (F)".into(),
                    0,
                    false,
                    GitPane::fetch,
                    cx,
                ))
                .child(div().w(px(1.)).bg(t.border))
                .child(sync(
                    "git-pull",
                    Icon::Pull,
                    "Pull",
                    if behind > 0 {
                        format!("Pull {behind} commit{} (p)", plural(behind))
                    } else {
                        "Pull (p)".into()
                    },
                    behind,
                    false,
                    GitPane::pull,
                    cx,
                ))
                .child(div().w(px(1.)).bg(t.border))
                .child(sync(
                    "git-push",
                    Icon::Push,
                    if upstream { "Push" } else { "Publish" },
                    if !upstream {
                        "Publish branch and set upstream (P)".into()
                    } else if ahead > 0 {
                        format!("Push {ahead} commit{} (P)", plural(ahead))
                    } else {
                        "Push (P)".into()
                    },
                    ahead,
                    ahead > 0 || !upstream,
                    GitPane::push,
                    cx,
                ))
                .child(div().w(px(1.)).bg(t.border))
                .child(sync(
                    "git-pr",
                    Icon::PullRequest,
                    "PR",
                    "Open pull request (⌃P)".into(),
                    0,
                    false,
                    GitPane::pull_request,
                    cx,
                )),
        )
}

/// A line of one of the head's menus, doing `act` to the pane.
pub(crate) fn menu_line(
    view: &Entity<GitPane>,
    row: ui::MenuRow,
    act: impl Fn(&mut GitPane, &mut Context<GitPane>) + 'static,
    cx: &App,
) -> PopupMenuItem {
    let view = view.clone();
    row.item(cx)
        .on_click(move |_, _, cx| view.update(cx, |pane, cx| act(pane, cx)))
}

/// The branch in the head, which opens a menu to make, find or switch to a branch.
pub(crate) fn branch_switch(
    pane: &GitPane,
    branch: String,
    cx: &mut Context<GitPane>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    let branches: Vec<(String, bool, Option<String>)> = match &pane.overview {
        Some(Ok(overview)) => overview
            .branches
            .iter()
            .map(|branch| (branch.name.clone(), branch.current, branch.upstream.clone()))
            .collect(),
        _ => Vec::new(),
    };
    ui::menu_button(
        "git-branch-switch",
        Anchor::TopLeft,
        BaseButton::new("git-switch")
            .accessibility_label("Switch branch")
            .tooltip(ui::tooltip("Switch branch"))
            .h(px(26.))
            .px(SPACE_1H)
            .min_w_0()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .rounded(RADIUS_2)
            .text_color(t.text_primary)
            .hover(move |style| style.bg(t.surface_hover))
            .child(ui::icon(Icon::Git, 13.).text_color(t.text_tertiary))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .font_weight(FontWeight::MEDIUM)
                    .child(branch),
            )
            .child(
                ui::icon(Icon::Chevron, 9.)
                    .text_color(t.text_tertiary)
                    .with_transformation(Transformation::rotate(radians(FRAC_PI_2))),
            ),
        move |mut popup, _, cx| {
            popup = popup
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("New branch…").icon(Icon::Plus).hint("N"),
                    |pane, cx| pane.new_branch(None, cx),
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Check out by name…")
                        .icon(Icon::Checkout)
                        .hint("c"),
                    |pane, cx| pane.checkout_by_name(cx),
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Show all branches")
                        .icon(Icon::Git)
                        .hint("3"),
                    |pane, cx| pane.turn(Page::Branches, cx),
                    cx,
                ))
                .separator();
            for (name, current, upstream) in &branches {
                let checked = name.clone();
                let hint = match current {
                    true => Some("current".to_owned()),
                    false => upstream.clone(),
                };
                popup = popup.item(
                    menu_line(
                        &view,
                        ui::MenuRow::new(name.clone())
                            .icon(Icon::Git)
                            .when_some(hint, ui::MenuRow::hint),
                        move |pane, cx| pane.checkout(checked.clone(), cx),
                        cx,
                    )
                    .disabled(*current),
                );
            }
            popup
        },
    )
}

/// Says that `host` is out of reach and what shows is what it last said.
pub(crate) fn away_note(host: &str, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .px(SPACE_3)
        .py(SPACE_2)
        .rounded(crate::theme::RADIUS_2)
        .border_1()
        .border_color(t.warn.opacity(0.4))
        .bg(t.warn.opacity(0.08))
        .text_size(crate::theme::ui_text(TEXT_XS))
        .text_color(t.text_primary)
        .child(format!(
            "{host} is out of reach. Showing what it last said; changes wait until it is back."
        ))
}

/// The head's menu of everything else: remotes, stashes, and the pane's own tools.
pub(crate) fn more_menu(pane: &GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let view = cx.entity();
    let changed = !pane.files().is_empty();
    let unstaged = pane.files().iter().any(GitFile::has_unstaged);
    let stashes = pane.stashes.len();
    let commands_open = pane.commands_open;
    ui::menu_button(
        "git-more",
        Anchor::TopRight,
        ui::IconButton::new("git-more-button", Icon::More, "Remotes, stashes and more").glyph(13.),
        move |popup, _, cx| {
            popup
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Fetch all remotes")
                        .icon(Icon::Fetch)
                        .hint("F"),
                    GitPane::fetch,
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Add remote…").icon(Icon::Plus),
                    GitPane::add_remote,
                    cx,
                ))
                .separator()
                .item(
                    menu_line(
                        &view,
                        ui::MenuRow::new("Stash changes…")
                            .icon(Icon::Stack)
                            .hint("s"),
                        GitPane::stash_menu,
                        cx,
                    )
                    .disabled(!changed),
                )
                .item(
                    menu_line(
                        &view,
                        ui::MenuRow::new("Discard unstaged changes…")
                            .icon(Icon::Discard)
                            .danger(true),
                        GitPane::discard_all,
                        cx,
                    )
                    .disabled(!unstaged),
                )
                .item(
                    menu_line(
                        &view,
                        ui::MenuRow::new(if stashes > 0 {
                            format!("Stashes ({stashes})…")
                        } else {
                            "No stashes".to_owned()
                        })
                        .icon(Icon::Stack),
                        GitPane::stashes_menu,
                        cx,
                    )
                    .disabled(stashes == 0),
                )
                .separator()
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Refresh").icon(Icon::Refresh).hint("r"),
                    GitPane::refresh,
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new(if commands_open {
                        "Hide command log"
                    } else {
                        "Show command log"
                    })
                    .icon(Icon::LogLines)
                    .hint("@"),
                    |pane, cx| {
                        pane.commands_open = !pane.commands_open;
                        cx.notify();
                    },
                    cx,
                ))
                .item(menu_line(
                    &view,
                    ui::MenuRow::new("Keyboard shortcuts")
                        .icon(Icon::Keyboard)
                        .hint("?"),
                    |pane, cx| {
                        pane.asking = Some(Asking::Keys);
                        cx.notify();
                    },
                    cx,
                ))
        },
    )
}

/// A page's filter: what to find, and how many there are when the page counts them.
pub(crate) fn filter_row(field: &Entity<InputState>, count: Option<usize>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex_none()
        .h(px(34.))
        .mb(SPACE_1)
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .border_b_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(ui::icon(Icon::Search, 12.).text_color(t.text_tertiary))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(field).appearance(false)),
        )
        .children(count.filter(|count| *count > 0).map(|count| {
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(count.to_string())
        }))
}

pub(crate) fn changes(
    pane: &mut GitPane,
    filter: Option<Div>,
    cx: &mut Context<GitPane>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let empty = pane.entries.is_empty();
    let query = pane.filters[Page::Changes as usize].query.clone();
    let note = match &pane.overview {
        _ if !empty => None,
        None => Some((
            "Loading changed files".to_owned(),
            String::new(),
            t.text_tertiary,
        )),
        Some(Err(error)) => Some(("Git error".to_owned(), error.clone(), t.danger)),
        Some(Ok(_)) if !query.is_empty() => Some((
            "No matches".to_owned(),
            format!("Nothing matches \"{query}\"."),
            t.text_tertiary,
        )),
        Some(Ok(_)) => Some((
            "Nothing to commit".to_owned(),
            "The working tree is clean.".to_owned(),
            t.text_tertiary,
        )),
    };
    let view = cx.entity();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .pb(SPACE_1H)
        .child(composer(pane, cx))
        .children(filter)
        .children(note.map(|(title, message, colour)| {
            div()
                .p(SPACE_5)
                .flex()
                .flex_col()
                .items_center()
                .gap(SPACE_1)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .child(div().text_color(colour).child(title))
                .child(div().text_color(t.text_tertiary).child(message))
        }))
        .child(
            list(pane.rows.clone(), move |index, window, cx| {
                let cx = &*cx;
                let pane = view.read(cx);
                match pane.entries.get(index).cloned() {
                    Some(entry) => entry_row(&view, pane, entry, index, window, cx),
                    None => div().into_any_element(),
                }
            })
            .flex_1()
            .px(SPACE_1H),
        )
}

/// The 28px button beside the message's writer that picks the agent and model.
pub(crate) fn writer_picker(pane: &GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    let writers = pane.writers.clone();
    let current = pane.writer().map(|(kind, _, model)| (kind, model));
    ui::menu_button(
        "git-writer",
        Anchor::TopLeft,
        BaseButton::new("git-writer-pick")
            .accessibility_label("Pick the agent and model")
            .tooltip(ui::tooltip("Pick the agent and model"))
            .size(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(t.border)
            .hover(move |style| style.bg(t.surface_hover).border_color(t.border_strong))
            .child(
                ui::icon(Icon::Chevron, 9.)
                    .text_color(t.text_tertiary)
                    .with_transformation(Transformation::rotate(radians(FRAC_PI_2))),
            ),
        move |mut popup, _, cx| {
            for (index, (kind, label, models)) in writers.iter().enumerate() {
                if index > 0 {
                    popup = popup.separator();
                }
                for model in models {
                    let picked = Some((kind.clone(), model.clone()));
                    let checked = picked == current;
                    let (view, kind) = (view.clone(), kind.clone());
                    popup = popup.item(
                        ui::MenuRow::new(format!("{label} · {model}"))
                            .mark(move || ui::mark(&kind, 13.))
                            .item(cx)
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                let picked = picked.clone();
                                view.update(cx, |pane, cx| {
                                    pane.writer = picked;
                                    cx.notify();
                                });
                            }),
                    );
                }
            }
            popup
        },
    )
}

pub(crate) fn composer(pane: &mut GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let writer_label = match pane.writer() {
        Some((_, label, model)) => {
            let model = model
                .split_once('/')
                .map_or(model.as_str(), |(_, model)| model);
            format!("{label} · {model}")
        }
        None => "Write message".to_owned(),
    };
    let staged = pane.staged_count();
    let message = !pane.draft.read(cx).value().trim().is_empty();
    let can_commit = pane.busy.is_none() && staged > 0 && message;
    let label = if staged > 0 {
        format!("Commit {staged} file{}", if staged == 1 { "" } else { "s" })
    } else {
        "Commit".to_owned()
    };
    let tip = if staged > 0 {
        format!("{label} (⌘⏎)")
    } else {
        "Stage files to commit them".to_owned()
    };
    div()
        .flex_none()
        .mb(SPACE_1H)
        .border_b_1()
        .border_color(t.border)
        .capture_key_down(cx.listener(|pane, event: &KeyDownEvent, window, cx| {
            let keys = &event.keystroke;
            if keys.key == "enter" && (keys.modifiers.platform || keys.modifiers.control) {
                pane.commit(cx);
                cx.stop_propagation();
            } else if keys.key == "escape" {
                window.focus(&pane.focus, cx);
                cx.stop_propagation();
            }
        }))
        .child(
            div()
                .px(SPACE_2H)
                .py(px(9.))
                .text_size(crate::theme::ui_text(TEXT_SM))
                .line_height(crate::theme::ui_text(19.5))
                .child(
                    Textarea::new(&pane.draft)
                        .appearance(false)
                        .text_size(crate::theme::ui_text(TEXT_SM)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1)
                .px(SPACE_2H)
                .pb(SPACE_2)
                .child(
                    BaseButton::new("git-generate")
                        .accessibility_label("Write the message from the staged changes")
                        .tooltip(ui::tooltip("Write the message from the staged changes (g)"))
                        .h(px(28.))
                        .px(SPACE_2)
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .border_1()
                        .border_color(t.border)
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .hover(move |style| {
                            style
                                .bg(t.surface_hover)
                                .border_color(t.border_strong)
                                .text_color(t.text_primary)
                        })
                        .on_click(cx.listener(|pane, _, _, cx| pane.generate(cx)))
                        .child(ui::icon(Icon::Sparkle, 11.).text_color(t.cmd))
                        .child(div().min_w_0().truncate().child(if pane.generating {
                            "writing…".to_owned()
                        } else {
                            writer_label
                        })),
                )
                .child(writer_picker(pane, cx))
                .child(
                    BaseButton::new("git-commit")
                        .accessibility_label(SharedString::from(label.clone()))
                        .tooltip(ui::tooltip(tip))
                        .disabled(!can_commit)
                        .ml_auto()
                        .h(px(28.))
                        .px(SPACE_3)
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(t.accent)
                        .bg(t.accent)
                        .text_color(t.on_accent)
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .font_weight(FontWeight::MEDIUM)
                        .when(!can_commit, |button| button.opacity(0.45))
                        .on_click(cx.listener(|pane, _, _, cx| pane.commit(cx)))
                        .child(label),
                ),
        )
}

/// The most faces a changed file shows of the agents that wrote it.
pub(crate) const MAX_FACES: usize = 4;

/// The faces of the agents that wrote some of `path`'s changes: each agent's own face,
/// else its agent's mark.
pub(crate) fn faces(pane: &GitPane, path: &str, cx: &App) -> Option<AnyElement> {
    let (writers, names) = pane.writers(path);
    if writers.is_empty() {
        return None;
    }
    let rows: Vec<AgentRow> = pane.bench.upgrade().map_or_else(Vec::new, |bench| {
        let bench = bench.read(cx);
        bench
            .projects()
            .iter()
            .flat_map(|project| bench.agents_of(&project.id))
            .collect()
    });
    let marks: Vec<AnyElement> = writers
        .iter()
        .take(MAX_FACES)
        .map(|(id, cli)| match rows.iter().find(|row| &row.id == id) {
            Some(row) => row.mark(14., cx),
            None => ui::mark(cli, 14.),
        })
        .collect();
    Some(
        div()
            .id(SharedString::from(format!("faces:{path}")))
            .flex_none()
            .flex()
            .items_center()
            .gap(SPACE_0)
            .tooltip(ui::tooltip(format!("Written by {}", names.join(", "))))
            .children(marks)
            .into_any_element(),
    )
}

/// A glyph a row shows while the pointer is on it.
pub(crate) fn row_button(
    id: impl Into<SharedString>,
    icon: Icon,
    glyph: f32,
    label: impl Into<SharedString>,
    cx: &App,
) -> BaseButton {
    let t = Tokens::get(cx);
    let (rest, lit, wash) = (t.text_tertiary, t.text_primary, t.surface_active);
    let label = label.into();
    BaseButton::new(id.into())
        .group(ROW_ACTION)
        .accessibility_label(label.clone())
        .tooltip(ui::tooltip(label))
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .hover(move |style| style.bg(wash))
        .child(
            ui::icon(icon, glyph)
                .text_color(rest)
                .group_hover(ROW_ACTION, move |style| style.text_color(lit)),
        )
}

/// The hover group a row's glyph button puts its glyph in, so the glyph lights with it.
pub(crate) const ROW_ACTION: &str = "git-row-action";

/// The hover actions of a changed file, as its list row and its review card show them:
/// open it, discard its unstaged change, and move it to the other group.
pub fn file_actions(
    view: &Entity<GitPane>,
    file: &GitFile,
    side: Option<Side>,
    cx: &App,
) -> Vec<AnyElement> {
    let action = |id: String, icon: Icon, label: String, act: Act| {
        let view = view.clone();
        row_button(id, icon, 12., label, cx)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let act = act.clone();
                view.update(cx, |pane, cx| act(pane, cx));
            })
            .into_any_element()
    };
    let unstaged = side.map_or(file.has_unstaged(), |side| side == Side::Unstaged);
    let staged = side.map_or(file.is_staged(), |side| side == Side::Staged);
    let key = format!("{}:{:?}", file.path, side);
    let mut actions = Vec::new();
    if side.is_some() {
        let target = OpenAt {
            path: Path::new(&view.read(cx).repo).join(&file.path),
            line: 0,
            column: None,
        };
        actions.push(
            row_button(
                format!("open:{key}"),
                Icon::File,
                12.,
                format!("Open {}", file.path),
                cx,
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                window.dispatch_action(target.boxed_clone(), cx);
            })
            .into_any_element(),
        );
    }
    if unstaged {
        let discarded = file.clone();
        actions.push(action(
            format!("discard:{key}"),
            Icon::Discard,
            "Discard changes".into(),
            Rc::new(move |pane, cx| pane.discard(&discarded, cx)),
        ));
    }
    if staged {
        let moved = file.clone();
        actions.push(action(
            format!("unstage:{key}"),
            Icon::Minus,
            format!("Unstage {}", file.path),
            Rc::new(move |pane, cx| pane.unstage(&moved, cx)),
        ));
    }
    if unstaged {
        let moved = file.clone();
        actions.push(action(
            format!("stage:{key}"),
            Icon::Plus,
            format!("Stage {}", file.path),
            Rc::new(move |pane, cx| pane.stage(&moved, cx)),
        ));
    }
    actions
}

pub(crate) fn entry_row(
    view: &Entity<GitPane>,
    pane: &GitPane,
    entry: Entry,
    index: usize,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    match entry {
        Entry::Group { side, count } => group_row(view, pane, side, count, index, cx),
        Entry::File { side, file } => {
            let first = !matches!(
                pane.entries.get(index.wrapping_sub(1)),
                Some(Entry::File { .. })
            );
            let last = !matches!(pane.entries.get(index + 1), Some(Entry::File { .. }));
            let chosen = index == pane.selected && pane.focus.contains_focused(window, cx);
            let ranged = pane.range.is_some_and(|anchor| {
                (anchor.min(pane.selected)..=anchor.max(pane.selected)).contains(&index)
            });
            let (dir, base) = match file.path.trim_end_matches('/').rsplit_once('/') {
                Some((dir, base)) => (Some(dir.to_owned()), base.to_owned()),
                None => (None, file.path.clone()),
            };
            let change = match side {
                Side::Staged => file.index,
                Side::Unstaged => file.worktree,
            };
            let letter = decoration(change, &t).map(|(letter, colour, label)| {
                let colour = if side == Side::Unstaged && letter == "M" {
                    t.git_renamed
                } else {
                    colour
                };
                div()
                    .id(SharedString::from(format!("status:{}:{side:?}", file.path)))
                    .flex_none()
                    .w(px(14.))
                    .text_center()
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colour)
                    .tooltip(ui::tooltip(label))
                    .child(letter)
            });
            let glyph = match crate::file_icons::icon(&base, cx) {
                Some(icon) => img(icon).size(px(14.)).flex_none().into_any_element(),
                None => ui::icon(Icon::File, 14.)
                    .text_color(t.text_tertiary)
                    .into_any_element(),
            };
            let key = SharedString::from(format!("row:{}:{side:?}", file.path));
            let selecting = view.clone();
            div()
                .id(SharedString::from(format!(
                    "git-file:{}:{side:?}",
                    file.path
                )))
                .on_hover(hover(view, key.clone()))
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
                .when(chosen, |row| row.bg(t.surface_chosen))
                .when(ranged && !chosen, |row| row.bg(t.warn.opacity(0.1)))
                .when(!chosen, |row| {
                    row.hover(move |row| row.bg(t.surface_hover).text_color(t.text_primary))
                })
                .on_click(move |_, window, cx| {
                    selecting.update(cx, |pane, cx| {
                        window.focus(&pane.focus, cx);
                        pane.select(index, cx);
                    })
                })
                .children(tree_lines(first, last, &t))
                .children(ui::spot(|| format!("git-file-{}", file.path)))
                .child(glyph)
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(div().flex_none().child(base))
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
                .when(pane.hovered.as_ref() != Some(&key), |row| {
                    row.children(faces(pane, &file.path, cx))
                })
                .when(pane.hovered.as_ref() == Some(&key), |row| {
                    row.child(div().flex().gap(SPACE_0).children(file_actions(
                        view,
                        &file,
                        Some(side),
                        cx,
                    )))
                })
                .children(letter)
                .into_any_element()
        }
    }
}

/// The lines a group's row hangs from, as the project tree's children do: a spine from
/// under the group's chevron, a tick to the row, and an elbow at the last.
pub(crate) fn tree_lines(first: bool, last: bool, t: &Tokens) -> Vec<Div> {
    let top = px(if first { -13. } else { -1. });
    if last {
        return vec![div()
            .absolute()
            .left(px(12.))
            .top(top)
            .w(px(5.))
            .h(px(if first { 27. } else { 14.5 }))
            .border_l_1()
            .border_b_1()
            .border_color(t.tree_spine)
            .rounded_bl(px(4.))];
    }
    vec![
        div()
            .absolute()
            .left(px(12.))
            .top(top)
            .bottom(px(-1.))
            .w(px(1.))
            .bg(t.tree_spine),
        div()
            .absolute()
            .left(px(13.))
            .top(px(12.))
            .w(px(4.))
            .h(px(1.))
            .bg(t.tree_tick),
    ]
}

pub(crate) fn group_row(
    view: &Entity<GitPane>,
    pane: &GitPane,
    side: Side,
    count: usize,
    index: usize,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let folded = pane.folded.contains(&side);
    let staged = side == Side::Staged;
    let key = SharedString::from(format!("group:{side:?}"));
    let button =
        |id: &str, icon: Icon, label: &str, act: fn(&mut GitPane, &mut Context<GitPane>)| {
            let view = view.clone();
            row_button(format!("{id}-{side:?}"), icon, 13., label.to_owned(), cx).on_click(
                move |_, _, cx| {
                    cx.stop_propagation();
                    view.update(cx, act);
                },
            )
        };
    let folding = view.clone();
    div()
        .when(index > 0, |row| row.pt(SPACE_1H))
        .child(
            div()
                .id(SharedString::from(format!("git-group:{side:?}")))
                .on_hover(hover(view, key.clone()))
                .h(px(ROW_HEIGHT))
                .pl(SPACE_0)
                .pr(SPACE_1)
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .rounded(RADIUS_2)
                .hover(move |row| row.bg(t.surface_hover))
                .child(
                    div()
                        .id(SharedString::from(format!("git-group-toggle:{side:?}")))
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .px(SPACE_1)
                        .items_center()
                        .gap(SPACE_1)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            folding.update(cx, |pane, cx| {
                                if !pane.folded.remove(&side) {
                                    pane.folded.insert(side);
                                }
                                pane.list(cx);
                            })
                        })
                        .child(motion::turn(
                            format!("{key}:chevron"),
                            motion::fold_angle(!folded),
                            motion::LIST_FOLD,
                            ui::icon(Icon::Chevron, 13.).text_color(t.text_tertiary),
                        ))
                        .child(
                            ui::icon(if staged { Icon::FilePlus } else { Icon::File }, 13.)
                                .text_color(t.text_tertiary),
                        )
                        .child(
                            ui::text(
                                if staged { "Staged Changes" } else { "Changes" },
                                TEXT_XS,
                                FontWeight::MEDIUM,
                            )
                            .text_color(t.text_primary),
                        ),
                )
                .when(pane.hovered.as_ref() == Some(&key), |row| {
                    row.child(
                        div()
                            .flex()
                            .gap(SPACE_0)
                            .when(index == 0, |actions| {
                                actions.child(button(
                                    "git-fold-review",
                                    if pane.review_folded {
                                        Icon::ExpandAll
                                    } else {
                                        Icon::CollapseAll
                                    },
                                    if pane.review_folded {
                                        "Expand all diffs"
                                    } else {
                                        "Collapse all diffs"
                                    },
                                    |pane, cx| {
                                        pane.review_folded = !pane.review_folded;
                                        pane.review.fold_all(pane.review_folded);
                                        cx.notify();
                                    },
                                ))
                            })
                            .when(index == 0, |actions| {
                                actions.child(button(
                                    "git-stash",
                                    Icon::Stash,
                                    "Stash",
                                    GitPane::stash_menu,
                                ))
                            })
                            .when(!staged, |actions| {
                                actions.child(button(
                                    "git-discard-all",
                                    Icon::Discard,
                                    "Discard all",
                                    GitPane::discard_all,
                                ))
                            })
                            .map(|actions| {
                                if staged {
                                    actions.child(button(
                                        "git-unstage-all",
                                        Icon::Minus,
                                        "Unstage all",
                                        GitPane::unstage_all,
                                    ))
                                } else {
                                    actions.child(button(
                                        "git-stage-all",
                                        Icon::Plus,
                                        "Stage all",
                                        GitPane::stage_all,
                                    ))
                                }
                            }),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .min_w(px(18.))
                        .h(px(18.))
                        .px(px(5.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(t.surface_chosen)
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.text_secondary)
                        .child(count.to_string()),
                ),
        )
        .into_any_element()
}

/// Keeps the pane's `hovered` on `key` while the pointer is inside it.
pub fn hover(
    view: &Entity<GitPane>,
    key: SharedString,
) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
    let view = view.clone();
    move |inside, _, cx| {
        view.update(cx, |pane, cx| {
            if *inside {
                pane.hovered = Some(key.clone());
            } else if pane.hovered.as_ref() == Some(&key) {
                pane.hovered = None;
            }
            cx.notify();
        })
    }
}

impl Render for GitPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(written) = self.written.take() {
            self.draft
                .update(cx, |draft, cx| draft.set_value(written, window, cx));
        }
        let reviewed = review(self, window, cx);
        let pushed =
            matches!(&self.overview, Some(Ok(overview)) if overview.status.upstream.is_some());
        let card = if self.page == Page::Branches {
            self.chosen_branch()
                .and_then(|entry| branch_card(self, &entry, cx))
        } else {
            self.review.commit().and_then(|hash| {
                let commit = self.log().iter().find(|commit| commit.full_hash == hash)?;
                Some(commit_card(self, commit, pushed, cx))
            })
        };
        let asking = self.asked(window, cx);
        div()
            .id("git-pane")
            .key_context("GitPane")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .gap(SPACE_2)
            .font_family(theme::UI)
            .on_key_down(cx.listener(Self::key))
            .child(side(self, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .children(card.map(|card| card.mb(SPACE_2)))
                    .child(div().flex_1().min_h_0().child(reviewed)),
            )
            .children(asking)
    }
}
