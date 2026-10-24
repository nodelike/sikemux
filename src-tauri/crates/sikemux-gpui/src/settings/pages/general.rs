//! The folders the person keeps projects in, and how deep each is searched; and sending the
//! project in front, with its chats and tabs, to another host.

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement,
    PathPromptOptions, SharedString, Styled, Subscription, Window,
};
use serde_json::json;
use sikemux_wire::protocol::{ProjectRoot, MAX_PROJECT_DEPTH};

use crate::core;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

/// How wide the project folders' "Index itself" and depth columns are.
const INDEX_COLUMN: f32 = 104.;
const DEPTH_COLUMN: f32 = 108.;

pub const SECTIONS: &[&str] = &["Project folders", "Session transfer"];

/// The folder being added: its path as typed, how deep to search it, and
/// whether it is a project itself.
pub struct Fields {
    path: Entity<InputState>,
    depth: u32,
    itself: bool,
    _entering: Subscription,
    transfer: Transfer,
}

/// Where the project in front is to go, and how, and what came of the last send.
#[derive(Default)]
struct Transfer {
    host: Option<String>,
    moving: bool,
    uncommitted: bool,
    sending: bool,
    said: Option<SharedString>,
}

/// Notes that a send went out, with none, or came back, with what it said.
pub fn sent(bench: &mut Workbench, said: Option<SharedString>) {
    if let Some(fields) = form_of(bench) {
        fields.transfer.sending = said.is_none();
        fields.transfer.said = said;
    }
}

fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let path =
        cx.new(|cx| InputState::new(window, cx).placeholder("~/proj    or    /Users/me/work"));
    let entering = cx.subscribe_in(&path, window, |bench, _, event: &InputEvent, window, cx| {
        if matches!(event, InputEvent::PressEnter { .. }) {
            add_typed(bench, window, cx);
        }
    });
    if let Some(settings) = bench.settings.as_mut() {
        settings.pages.general = Some(Fields {
            path,
            depth: 1,
            itself: false,
            _entering: entering,
            transfer: Transfer::default(),
        });
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

/// Changes the list of project folders in the core.
fn change(bench: &Workbench, edit: impl FnOnce(&mut Vec<ProjectRoot>)) {
    let mut roots = bench.preferences().project_roots;
    edit(&mut roots);
    crate::settings::set(
        bench,
        "projectRoots",
        serde_json::to_value(roots).unwrap_or_default(),
    );
}

fn form_of(bench: &mut Workbench) -> Option<&mut Fields> {
    bench.settings.as_mut()?.pages.general.as_mut()
}

/// Adds a folder with the depth and choice the add row holds, and clears it.
fn add(bench: &mut Workbench, path: String, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(fields) = form_of(bench) else {
        return;
    };
    let (depth, itself) = (fields.depth, fields.itself);
    fields.depth = 1;
    fields.itself = false;
    fields
        .path
        .update(cx, |input, cx| input.set_value("", window, cx));
    change(bench, |roots| {
        if !roots.iter().any(|root| root.path == path) {
            roots.push(ProjectRoot {
                path,
                depth,
                self_index: itself,
            });
        }
    });
    bench.parts.redraw(&bench.parts.stage, cx);
}

/// Adds the typed folder once the core says it is one.
fn add_typed(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(typed) = form_of(bench).map(|fields| fields.path.read(cx).value().trim().to_owned())
    else {
        return;
    };
    if typed.is_empty() {
        return;
    }
    let asks = bench.asks.clone();
    cx.spawn_in(window, async move |bench, cx| {
        let Ok(resolved) = core::request(&asks, "resolvePath", json!({ "path": typed })).await
        else {
            return;
        };
        if resolved["kind"].as_str() != Some("dir") {
            return;
        }
        let Some(path) = resolved["path"].as_str().map(str::to_owned) else {
            return;
        };
        let _ = bench.update_in(cx, |bench, window, cx| add(bench, path, window, cx));
    })
    .detach();
}

/// The folder as the person writes it, under ~ when it is in their home.
fn pretty(path: &str) -> String {
    let home = sikemux_process::paths::home()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// − and + around a depth, from 0 to the deepest the core searches.
fn stepper(
    id: &'static str,
    key: SharedString,
    depth: u32,
    set: impl Fn(u32, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let set = std::rc::Rc::new(set);
    let (less, more) = (set.clone(), set);
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(
            ui::IconButton::new(
                SharedString::from(format!("{id}-less-{key}")),
                Icon::Minus,
                "One level less",
            )
            .small()
            .on_click(move |_, _, cx| less(depth.saturating_sub(1), cx)),
        )
        .child(
            div()
                .w(px(18.))
                .text_center()
                .text_size(crate::theme::ui_text(12.))
                .text_color(t.text_primary)
                .child(depth.to_string()),
        )
        .child(
            ui::IconButton::new(
                SharedString::from(format!("{id}-more-{key}")),
                Icon::Plus,
                "One level more",
            )
            .small()
            .on_click(move |_, _, cx| more((depth + 1).min(MAX_PROJECT_DEPTH), cx)),
        )
        .into_any_element()
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.general else {
        cx.defer_in(window, make);
        return Vec::new();
    };
    let t = Tokens::get(cx).clone();
    let roots = bench.preferences().project_roots;
    let listed = roots.iter().map(|root| {
        let key: SharedString = root.path.clone().into();
        let (itself, deeper, remove) = (this.clone(), this.clone(), this.clone());
        let (path_a, path_b, path_c) = (root.path.clone(), root.path.clone(), root.path.clone());
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(8.))
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(12.))
                    .text_color(t.text_primary)
                    .child(pretty(&root.path)),
            )
            .child(
                div().w(px(INDEX_COLUMN)).flex().justify_center().child(
                    Checkbox::new(SharedString::from(format!("root-itself-{key}")))
                        .accessibility_label("Index itself")
                        .checked(root.self_index)
                        .on_click(move |on: &bool, _, cx| {
                            let (on, path) = (*on, path_a.clone());
                            itself.update(cx, |bench, _| {
                                change(bench, |roots| {
                                    roots
                                        .iter_mut()
                                        .filter(|root| root.path == path)
                                        .for_each(|root| root.self_index = on)
                                })
                            })
                        }),
                ),
            )
            .child(
                div()
                    .w(px(DEPTH_COLUMN))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(stepper(
                        "root-depth",
                        key.clone(),
                        root.depth,
                        move |depth, cx| {
                            let path = path_b.clone();
                            deeper.update(cx, |bench, _| {
                                change(bench, |roots| {
                                    roots
                                        .iter_mut()
                                        .filter(|root| root.path == path)
                                        .for_each(|root| root.depth = depth)
                                })
                            })
                        },
                        cx,
                    ))
                    .child(
                        ui::IconButton::new(
                            SharedString::from(format!("root-remove-{key}")),
                            Icon::Close,
                            "Remove",
                        )
                        .small()
                        .on_click(move |_, _, cx| {
                            let path = path_c.clone();
                            remove.update(cx, |bench, _| {
                                change(bench, |roots| roots.retain(|root| root.path != path))
                            })
                        }),
                    ),
            )
    });
    let adding = {
        let (depth_this, itself_this, browse, add_this) =
            (this.clone(), this.clone(), this.clone(), this.clone());
        let empty = fields.path.read(cx).value().trim().is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().child(ui::Field::new(&fields.path).mono()))
                    .child(stepper(
                        "add-depth",
                        "new".into(),
                        fields.depth,
                        move |depth, cx| {
                            depth_this.update(cx, |bench, cx| {
                                if let Some(fields) = form_of(bench) {
                                    fields.depth = depth;
                                }
                                bench.parts.redraw(&bench.parts.stage, cx);
                            })
                        },
                        cx,
                    ))
                    .child(
                        ui::IconButton::new("root-browse", Icon::Folder, "Browse…").on_click(
                            move |_, window, cx| {
                                browse.update(cx, |_, cx| {
                                    let picked = cx.prompt_for_paths(PathPromptOptions {
                                        files: false,
                                        directories: true,
                                        multiple: false,
                                        prompt: None,
                                    });
                                    cx.spawn_in(window, async move |bench, cx| {
                                        if let Ok(Ok(Some(paths))) = picked.await {
                                            if let Some(path) = paths.first() {
                                                let path = path.to_string_lossy().into_owned();
                                                let _ = bench.update_in(cx, |bench, window, cx| {
                                                    add(bench, path, window, cx)
                                                });
                                            }
                                        }
                                    })
                                    .detach();
                                })
                            },
                        ),
                    )
                    .child(
                        ui::Button::new("root-add", "Add", ui::Variant::Outline)
                            .icon(Icon::Plus)
                            .disabled(empty)
                            .on_click(move |_, window, cx| {
                                add_this.update(cx, |bench, cx| add_typed(bench, window, cx))
                            }),
                    ),
            )
            .child(
                Checkbox::new("root-add-itself")
                    .label("Index the folder itself as a project")
                    .checked(fields.itself)
                    .on_click(move |on: &bool, _, cx| {
                        let on = *on;
                        itself_this.update(cx, |bench, cx| {
                            if let Some(fields) = form_of(bench) {
                                fields.itself = on;
                            }
                            bench.parts.redraw(&bench.parts.stage, cx);
                        })
                    }),
            )
    };
    let count = roots.len();
    let heads = (count > 0).then(|| {
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .pt(px(10.))
            .pb(px(4.))
            .border_b_1()
            .border_color(t.line_soft)
            .child(div().flex_1().child(ui::caps("Folder", cx)))
            .child(
                div()
                    .w(px(INDEX_COLUMN))
                    .flex()
                    .justify_center()
                    .child(ui::caps("Index itself", cx)),
            )
            .child(div().w(px(DEPTH_COLUMN)).child(ui::caps("Depth", cx)))
    });
    let body = div()
        .children(heads)
        .children(listed)
        .child(adding)
        .child(rows::note(
            "Indexing a folder itself offers it in the picker even when it is not a repo — useful for a scratch directory.",
            cx,
        ));
    vec![
        rows::section(
            "Project folders",
            Some(format!("{count} {}", if count == 1 { "folder" } else { "folders" }).into()),
            Some("Each folder is scanned for git repos, as deep as its depth allows.".into()),
            lit("Project folders"),
            body,
            cx,
        ),
        rows::section(
            "Session transfer",
            None,
            Some("Send the project in front to another host, with its chats and tabs.".into()),
            lit("Session transfer"),
            transfer(bench, &fields.transfer, this, cx),
            cx,
        ),
    ]
}

/// Changes how the project in front is to go.
fn set_transfer(this: &Entity<Workbench>, change: impl FnOnce(&mut Transfer), cx: &mut App) {
    this.update(cx, |bench, cx| {
        if let Some(fields) = form_of(bench) {
            change(&mut fields.transfer);
        }
        bench.parts.redraw(&bench.parts.stage, cx);
    })
}

fn transfer(
    bench: &Workbench,
    transfer: &Transfer,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let Some(project) = bench.project().cloned() else {
        return rows::note("Open a project to send it to another host.", cx);
    };
    let hosts: Vec<_> = bench
        .other_hosts()
        .into_iter()
        .filter(|(.., reached, _)| *reached)
        .collect();
    if hosts.is_empty() {
        return rows::note(
            "Link another Sikemux host under Devices, then send projects there.",
            cx,
        );
    }
    let chosen = transfer
        .host
        .as_ref()
        .and_then(|key| hosts.iter().position(|(host, ..)| host == key))
        .unwrap_or(0);
    let (host, label, _, after) = hosts[chosen].clone();
    let keys: Vec<String> = hosts.iter().map(|(key, ..)| key.clone()).collect();
    let (pick_host, pick_way, pick_changes, send) =
        (this.clone(), this.clone(), this.clone(), this.clone());
    let moving = transfer.moving;
    let verb = if moving { "Move" } else { "Copy" };
    let name = project.name.clone();
    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(14.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .flex_wrap()
                .child(
                    ui::Segmented::words(
                        "transfer-host",
                        hosts.iter().map(|(_, label, ..)| (label.clone(), None)).collect(),
                        chosen,
                    )
                    .on_select(move |at: &usize, _, cx| {
                        let key = keys.get(*at).cloned();
                        set_transfer(&pick_host, |transfer| transfer.host = key, cx)
                    }),
                )
                .child(
                    ui::Segmented::words(
                        "transfer-way",
                        vec![("Copy".into(), None), ("Move".into(), None)],
                        usize::from(moving),
                    )
                    .on_select(move |at: &usize, _, cx| {
                        let moving = *at == 1;
                        set_transfer(&pick_way, |transfer| transfer.moving = moving, cx)
                    }),
                ),
        )
        .child(
            Checkbox::new("transfer-uncommitted")
                .label("Bring uncommitted changes")
                .checked(transfer.uncommitted)
                .on_click(move |on: &bool, _, cx| {
                    let on = *on;
                    set_transfer(&pick_changes, |transfer| transfer.uncommitted = on, cx)
                }),
        )
        .child(
            div().flex().child(
                ui::Button::new(
                    "transfer-send",
                    format!("{verb} {name} to {label}"),
                    ui::Variant::Primary,
                )
                .disabled(transfer.sending)
                .on_click({
                    let uncommitted = transfer.uncommitted;
                    move |_, _, cx| {
                        let sending = crate::hosts::Sending {
                            project: project.id.clone(),
                            name: project.name.clone(),
                            host: host.clone(),
                            label: label.clone(),
                            after: after.clone(),
                            moving,
                            uncommitted,
                        };
                        send.update(cx, |bench, cx| {
                            if !sending.moving {
                                return bench.send_session(sending, cx);
                            }
                            let title = format!("Move {} to {}?", sending.name, sending.label);
                            let held = std::cell::RefCell::new(Some(sending));
                            bench.confirm(
                                title,
                                "Its chats are closed here and their saved conversations removed, once the other host has every chat and tab. If anything does not arrive whole, nothing here changes.",
                                "Move",
                                true,
                                move |bench, yes, _, cx| {
                                    if let Some(sending) = held.borrow_mut().take().filter(|_| yes) {
                                        bench.send_session(sending, cx);
                                    }
                                },
                                cx,
                            );
                        })
                    }
                }),
            ),
        )
        .children(transfer.said.clone().map(|said| rows::note(said, cx)))
        .child(rows::note(
            "Chats come along when their agent keeps them in files, as Claude and Codex do; the others stay here. A chat that came along starts on the other host's sign-in when you next send to it.",
            cx,
        ));
    body.into_any_element()
}
