//! The tab rail's pages for terminals with no project and for an SSH
//! host.

use crate::workbench::actions;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled,
};
use sikemux_view::layout::PaneKind;

use crate::agents::{
    faded, new_row, other_host, page_body, page_header, rail_empty, scope_switch, EDGE_FADE,
};
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_0, SPACE_1, SPACE_2, SPACE_2H};
use crate::ui;
use crate::ui::Row;
use crate::workbench::{Strip, Workbench};

/// The project's terminal tabs, or every project's, to bring forward or start.
pub(crate) fn terminals_page(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let strips: Vec<Strip> = if bench.rails.all_projects {
        bench
            .projects()
            .iter()
            .map(|project| Strip::Project(project.id.clone()))
            .collect()
    } else {
        bench.strip().into_iter().collect()
    };
    let rows = terminal_rows(bench, &strips, cx);
    let empty = rows.is_empty().then(|| {
        if strips.is_empty() {
            rail_empty(
                Icon::Terminal,
                "No project open",
                Some((actions::OpenProject::ID, "open one")),
                cx,
            )
        } else {
            let title = match (bench.rails.all_projects, bench.project(), other_host(bench)) {
                (false, Some(project), Some(host)) => {
                    format!("No terminals in {} on {host}", project.name)
                }
                _ => "No terminals open".to_owned(),
            };
            rail_empty(
                Icon::Terminal,
                title,
                Some((actions::NewTerminal::ID, "start one")),
                cx,
            )
        }
    });
    page_body()
        .child(page_header("Terminals", bench, cx))
        .child(
            div()
                .flex_none()
                .pt(SPACE_1)
                .px(SPACE_2H)
                .child(
                    new_row(
                        "new-terminal",
                        Icon::Plus,
                        "New terminal",
                        actions::NewTerminal::ID,
                        cx,
                    )
                    .on_click(cx.listener(|bench, _, _, cx| bench.new_terminal(cx))),
                )
                .child(div().mt(SPACE_2).child(scope_switch(bench, cx))),
        )
        .child(faded(
            div()
                .id("terminals-scroll")
                .size_full()
                .pt(px(EDGE_FADE))
                .px(SPACE_2H)
                .pb(px(EDGE_FADE))
                .overflow_y_scroll()
                .children(empty)
                .child(div().flex().flex_col().gap(SPACE_0).children(rows)),
            cx,
        ))
        .into_any_element()
}

/// A row for each terminal tab of `strips`, which brings it to the front.
pub(crate) fn terminal_rows(
    bench: &Workbench,
    strips: &[Strip],
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let front = bench.tab().map(|tab| tab.id.clone());
    let mut rows = Vec::new();
    for strip in strips {
        let project = match strip {
            Strip::Project(id) if bench.rails.all_projects => bench
                .projects()
                .iter()
                .find(|project| &project.id == id)
                .map(|project| project.name.clone()),
            _ => None,
        };
        for tab in bench.tabs(strip) {
            if !matches!(
                crate::tabs::first_view(&tab.pane),
                Some((PaneKind::Terminal, _))
            ) {
                continue;
            }
            let (shown, id) = (strip.clone(), tab.id.clone());
            let row = Row::new(
                SharedString::from(format!("terminal-{}", tab.id)),
                crate::tabs::label(bench, tab),
            )
            .mark(22., ui::icon(Icon::Terminal, 15.).text_color(t.live))
            .chosen(front.as_deref() == Some(tab.id.as_str()))
            .when_some(project.clone(), |row, project| {
                row.subtitle(ui::meta(project, cx))
            })
            .on_click(cx.listener(move |bench, _, _, _| bench.activate_tab(&shown, &id)));
            rows.push(
                div()
                    .id(SharedString::from(format!("terminal-menu-{}", tab.id)))
                    .child(row)
                    .context_menu(ui::menu(crate::tabs::tab_menu(bench, strip, tab, cx)))
                    .into_any_element(),
            );
        }
    }
    rows
}

/// What an SSH host without Sikemux offers: a login there, or installing Sikemux, over its
/// terminals (ADR 0019).
pub(crate) fn ssh_page(
    bench: &Workbench,
    alias: String,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let rows = terminal_rows(bench, &[Strip::Host(alias.clone())], cx);
    let (set_up_icon, set_up_label) = bench.set_up_label(&alias);
    let empty = rows.is_empty().then(|| {
        rail_empty(
            Icon::Terminal,
            "No terminals open",
            Some((actions::NewTerminal::ID, "start one")),
            cx,
        )
    });
    page_body()
        .child(page_header(alias.clone(), bench, cx))
        .child(
            div()
                .flex_none()
                .pt(SPACE_1)
                .px(SPACE_2H)
                .child(
                    new_row(
                        "new-terminal",
                        Icon::Terminal,
                        "New terminal",
                        actions::NewTerminal::ID,
                        cx,
                    )
                    .on_click(cx.listener(|bench, _, _, cx| bench.new_terminal(cx))),
                )
                .child(
                    new_row(
                        "install-sikemux",
                        set_up_icon,
                        set_up_label,
                        actions::NewChat::ID,
                        cx,
                    )
                    .on_click(cx.listener(move |bench, _, _, _| bench.open_install_pane(&alias))),
                ),
        )
        .child(faded(
            div()
                .id("ssh-terminals-scroll")
                .size_full()
                .pt(px(EDGE_FADE))
                .px(SPACE_2H)
                .pb(px(EDGE_FADE))
                .overflow_y_scroll()
                .children(empty)
                .child(div().flex().flex_col().gap(SPACE_0).children(rows)),
            cx,
        ))
        .into_any_element()
}
