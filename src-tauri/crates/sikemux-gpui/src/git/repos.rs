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
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

/// Opens `repo` by default in the project at `folder`, or with none goes back to asking.
fn set_default(bench: &Workbench, folder: &str, repo: Option<&str>) {
    let mut chosen = bench.preferences().default_git_repos;
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
            .gap(px(6.))
            .child(ui::text("Not a repository", 12.5, FontWeight::MEDIUM).text_color(t.danger))
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
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
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
                    .text_size(crate::theme::ui_text(12.5))
                    .text_color(t.text_primary)
                    .child(repo["name"].as_str().unwrap_or_default().to_owned()),
            )
            .child(
                ui::mono(branch.unwrap_or("no commits").to_owned(), 11., cx)
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
                    .text_size(crate::theme::ui_text(11.))
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
                .px(px(12.))
                .py(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .border_b_1()
                .border_color(t.border)
                .child(ui::icon(Icon::Git, 13.).text_color(t.text_tertiary))
                .child(ui::text(name, 12.5, FontWeight::MEDIUM).text_color(t.text_primary))
                .child(ui::meta(count, cx)),
        )
        .child(
            div()
                .id("git-repos")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(6.))
                .children(rows),
        )
        .into_any_element()
}
