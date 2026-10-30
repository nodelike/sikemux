//! The repositories inside a project folder that is not one itself, which the Git tab
//! lists until one is opened, with the one opened by default marked.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled,
};
use serde_json::json;

use crate::core::Ask;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_XS_H};
use crate::ui;
use crate::workbench::Workbench;

/// Opens `repo` by default in the project at `folder`, or with none goes back to asking.
fn set_default(bench: &Workbench, folder: &str, repo: Option<&str>) {
    let mut chosen = bench.preferences().default_git_repos.clone();
    match repo {
        Some(repo) => {
            chosen.insert(folder.to_owned(), repo.to_owned());
        }
        None => {
            chosen.remove(folder);
        }
    }
    let params = json!({ "key": "defaultGitRepos", "value": chosen });
    let _ = bench.asks.send(Ask::Call {
        method: "setPreference",
        params,
    });
}

pub fn picker(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let folder = bench.git_folder().unwrap_or_default().to_owned();
    let repos = bench.git_repos().cloned().unwrap_or_default();
    let name = folder.rsplit('/').next().unwrap_or_default().to_owned();
    if repos.is_empty() {
        return div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(SPACE_1H)
            .child(ui::text("Not a repository", TEXT_XS_H, FontWeight::MEDIUM).text_color(t.danger))
            .child(ui::meta(
                format!("No repository at {folder}, and none in the folders directly inside it."),
                cx,
            ))
            .into_any_element();
    }
    let chosen = bench.preferences().default_git_repos.get(&folder).cloned();
    let count = if repos.len() == 1 {
        "1 repository inside".to_owned()
    } else {
        format!("{} repositories inside", repos.len())
    };
    let rows = repos.iter().enumerate().map(|(index, repo)| {
        let path = repo["path"].as_str().unwrap_or_default().to_owned();
        let changes = repo["changes"].as_u64().unwrap_or_default();
        let (ahead, behind) = (
            repo["ahead"].as_i64().unwrap_or_default(),
            repo["behind"].as_i64().unwrap_or_default(),
        );
        let branch = repo["branch"].as_str().filter(|branch| !branch.is_empty());
        let default = chosen.as_deref() == Some(path.as_str());
        let (opened, starred, folder) = (path.clone(), path.clone(), folder.clone());
        div()
            .id(("git-repo", index))
            .h(px(32.))
            .px(SPACE_2H)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .rounded(theme::RADIUS_2)
            .cursor_pointer()
            .hover(move |row| row.bg(t.surface_hover))
            .on_click(cx.listener(move |bench, _, _, cx| bench.pick_git_repo(opened.clone(), cx)))
            .child(ui::icon(Icon::Git, 13.).text_color(if changes > 0 {
                t.git_modified
            } else {
                t.text_tertiary
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_XS_H))
                    .text_color(t.text_primary)
                    .child(repo["name"].as_str().unwrap_or_default().to_owned()),
            )
            .child(
                ui::mono(branch.unwrap_or("no commits").to_owned(), TEXT_2XS, cx)
                    .text_color(t.text_secondary),
            )
            .when(ahead > 0, |row| {
                row.child(ui::meta(format!("{ahead}↑"), cx))
            })
            .when(behind > 0, |row| {
                row.child(ui::meta(format!("{behind}↓"), cx))
            })
            .child(
                div()
                    .w(px(24.))
                    .text_right()
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(if changes > 0 {
                        t.git_modified
                    } else {
                        t.text_tertiary
                    })
                    .child(if changes > 0 {
                        changes.to_string()
                    } else {
                        "—".into()
                    }),
            )
            .child(
                ui::IconButton::new(
                    ("git-repo-default", index),
                    Icon::Star,
                    if default {
                        "Opened by default; click to stop"
                    } else {
                        "Open this one by default in this project"
                    },
                )
                .small()
                .tint(if default { t.accent } else { t.text_tertiary })
                .on_click(cx.listener(move |bench, _, _, cx| {
                    cx.stop_propagation();
                    set_default(bench, &folder, (!default).then_some(starred.as_str()));
                })),
            )
    });
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .px(SPACE_3)
                .py(SPACE_2H)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .border_b_1()
                .border_color(t.border)
                .child(ui::icon(Icon::Git, 13.).text_color(t.text_tertiary))
                .child(ui::text(name, TEXT_XS_H, FontWeight::MEDIUM).text_color(t.text_primary))
                .child(ui::meta(count, cx)),
        )
        .child(
            div()
                .id("git-repos")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(SPACE_1H)
                .children(rows),
        )
        .into_any_element()
}
