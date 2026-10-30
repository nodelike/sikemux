//! A project's own settings in a tab of its own: what Sikemux keeps for it, its tasks and
//! check command, the repository its Git views use, and its sikemux.json.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
};
use serde_json::{json, Value};
use sikemux_core::server::{Source, TaskView};
use sikemux_wire::protocol::{PaneKind, ProjectInfo};

use crate::icons::Icon;
use crate::motion;
use crate::settings::{self, rows};
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_6, TEXT_2XS_H,
    TEXT_3XS, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::toast;
use crate::ui::{self, Button, Field, IconButton, Variant};
use crate::workbench::Workbench;

#[derive(Clone, Copy, Default, PartialEq)]
enum Page {
    #[default]
    General,
    Tasks,
    Git,
    Config,
}

const PAGES: [(Page, &str, Icon); 4] = [
    (Page::General, "General", Icon::Settings),
    (Page::Tasks, "Tasks", Icon::Run),
    (Page::Git, "Git", Icon::Git),
    (Page::Config, "sikemux.json", Icon::Code),
];

/// What a project's settings tab keeps between frames.
pub struct ProjectSettings {
    page: Page,
    /// What the core last said of the project's sikemux.json.
    config: Option<Value>,
    /// The repositories directly inside the project's folder.
    repos: Option<Vec<Value>>,
    check: Entity<InputState>,
    /// Whether the check field has been given the command sikemux.json names.
    check_filled: bool,
    filter: Entity<InputState>,
    space: rows::Choice,
    /// The spaces the space choice was made with, so a new one remakes it.
    spaces: Vec<String>,
    /// The folders of found tasks the person opened.
    opened: HashSet<String>,
    _typing: Vec<Subscription>,
}

impl Workbench {
    /// Opens the settings tab of the project `id`, or brings it forward.
    pub fn open_project_settings(&self, id: &str) {
        let Some(project) = self.projects().iter().find(|project| project.id == id) else {
            return;
        };
        self.call_in_front(
            "openTab",
            json!({
                "project": project.id,
                "kind": PaneKind::ProjectSettings,
                "target": project.path.to_string_lossy(),
            }),
        );
    }

    fn project_at(&self, folder: &str) -> Option<&ProjectInfo> {
        self.projects()
            .iter()
            .find(|project| project.path == Path::new(folder))
    }

    fn settings_of(&mut self, folder: &str) -> Option<&mut ProjectSettings> {
        self.project_settings.get_mut(folder)
    }

    fn reload_config(&self, folder: &str, cx: &mut Context<Self>) {
        let asked = self
            .target()
            .request("projectConfig", json!({ "project": folder }));
        let folder = folder.to_owned();
        cx.spawn(async move |bench, cx| {
            let Ok(config) = asked.await else {
                return;
            };
            let _ = bench.update(cx, |bench, cx| {
                if let Some(settings) = bench.settings_of(&folder) {
                    settings.config = Some(config);
                }
                bench.parts.redraw_stage(cx);
            });
        })
        .detach();
    }

    fn save_check(&self, folder: &str, cx: &mut Context<Self>) {
        let Some(settings) = self.project_settings.get(folder) else {
            return;
        };
        let typed = settings.check.read(cx).value().trim().to_owned();
        let saved = settings.config.as_ref().map_or("", |config| {
            config["config"]["check"].as_str().unwrap_or_default()
        });
        if typed == saved {
            return;
        }
        let saving = self
            .target()
            .request("setCheck", json!({ "project": folder, "command": typed }));
        let folder = folder.to_owned();
        cx.spawn(async move |bench, cx| {
            let saved = saving.await;
            let _ = bench.update(cx, |bench, cx| match saved {
                Ok(_) => bench.reload_config(&folder, cx),
                Err(error) => bench.notify(
                    toast::error(format!("The check command was not saved: {error}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    fn trust_config(&self, folder: &str, cx: &mut Context<Self>) {
        let Some(fingerprint) = self
            .project_settings
            .get(folder)
            .and_then(|settings| settings.config.as_ref())
            .and_then(|config| config["fingerprint"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let trusting = self.target().request(
            "trustProject",
            json!({ "project": folder, "fingerprint": fingerprint }),
        );
        let folder = folder.to_owned();
        cx.spawn(async move |bench, cx| {
            if trusting.await.is_ok() {
                let _ = bench.update(cx, |bench, cx| bench.reload_config(&folder, cx));
            }
        })
        .detach();
    }

    fn remove_task(&mut self, folder: &str, task: &TaskView, cx: &mut Context<Self>) {
        let (id, label) = (&task.id, &task.label);
        let body = match task.source {
            Source::Personal => "It comes out of .sikemux/tasks.json.",
            _ => "It comes out of sikemux.json; commit that to remove it for the team.",
        };
        let (folder, id) = (folder.to_owned(), id.to_owned());
        self.confirm(
            format!("Remove {label}?"),
            body,
            "Remove",
            true,
            move |bench, yes, _, _| {
                if yes {
                    bench
                        .target()
                        .call("removeTask", json!({ "project": folder, "taskId": id }));
                }
            },
            cx,
        );
    }

    fn set_project_map<T: serde::Serialize>(
        &self,
        key: &str,
        mut map: BTreeMap<String, T>,
        folder: &str,
        value: Option<T>,
    ) {
        match value {
            Some(value) => map.insert(folder.to_owned(), value),
            None => map.remove(folder),
        };
        settings::set(self, key, serde_json::to_value(map).unwrap_or_default());
    }
}

fn make(bench: &mut Workbench, folder: String, window: &mut Window, cx: &mut Context<Workbench>) {
    let check = cx.new(|cx| InputState::new(window, cx).placeholder("pnpm check"));
    let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter tasks"));
    let saving = {
        let folder = folder.clone();
        cx.subscribe(&check, move |bench, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                bench.save_check(&folder, cx);
            }
        })
    };
    let filtering = cx.subscribe(&filter, |bench, _, event: &InputEvent, cx| {
        if let InputEvent::Change = event {
            bench.parts.redraw_stage(cx);
        }
    });
    let (space, spaces) = space_choice(bench, &folder, window, cx);
    bench.project_settings.insert(
        folder.clone(),
        ProjectSettings {
            page: Page::default(),
            config: None,
            repos: None,
            check,
            check_filled: false,
            filter,
            space,
            spaces,
            opened: HashSet::new(),
            _typing: vec![saving, filtering],
        },
    );
    bench.reload_config(&folder, cx);
    let finding = bench
        .target()
        .request("git.discoverRepos", json!({ "root": folder }));
    cx.spawn(async move |bench, cx| {
        let repos = finding
            .await
            .ok()
            .and_then(|found| found.as_array().cloned());
        let _ = bench.update(cx, |bench, cx| {
            if let Some(settings) = bench.settings_of(&folder) {
                settings.repos = Some(repos.unwrap_or_default());
            }
            bench.parts.redraw_stage(cx);
        });
    })
    .detach();
    bench.parts.redraw_stage(cx);
}

fn space_ids(bench: &Workbench) -> Vec<String> {
    bench.view.as_ref().map_or(Vec::new(), |view| {
        view.workspace
            .spaces
            .iter()
            .map(|space| space.id.clone())
            .collect()
    })
}

/// The dropdown of spaces the project can be listed under, and the spaces it offers.
fn space_choice(
    bench: &Workbench,
    folder: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (rows::Choice, Vec<String>) {
    let spaces = space_ids(bench);
    let mut labels = vec![SharedString::from("No Space")];
    if let Some(view) = &bench.view {
        labels.extend(
            view.workspace
                .spaces
                .iter()
                .map(|space| SharedString::from(space.name.clone())),
        );
    }
    let current = bench
        .project_at(folder)
        .and_then(|project| project.space.as_ref());
    let chosen = current
        .and_then(|current| spaces.iter().position(|space| space == current))
        .map_or(0, |at| at + 1);
    let (offered, folder) = (spaces.clone(), folder.to_owned());
    let choice = rows::Choice::new(
        labels,
        chosen,
        move |bench, at, _| {
            let Some(project) = bench.project_at(&folder).map(|project| project.id.clone()) else {
                return;
            };
            let space = at.checked_sub(1).and_then(|at| offered.get(at)).cloned();
            bench.call_in_front(
                "updateProject",
                json!({ "id": project, "patch": { "space": space } }),
            );
        },
        window,
        cx,
    );
    (choice, spaces)
}

fn remake_space(
    bench: &mut Workbench,
    folder: String,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let (space, spaces) = space_choice(bench, &folder, window, cx);
    if let Some(settings) = bench.settings_of(&folder) {
        settings.space = space;
        settings.spaces = spaces;
    }
    bench.parts.redraw_stage(cx);
}

/// The settings tab of the project in `folder`.
pub fn pane(
    bench: &Workbench,
    folder: &str,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let Some(settings) = bench.project_settings.get(folder) else {
        let folder = folder.to_owned();
        cx.defer_in(window, move |bench, window, cx| {
            make(bench, folder, window, cx)
        });
        return ui::pane(cx).into_any_element();
    };
    if let (false, Some(config)) = (settings.check_filled, &settings.config) {
        let check = config["config"]["check"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let folder = folder.to_owned();
        cx.defer_in(window, move |bench, window, cx| {
            if let Some(settings) = bench.settings_of(&folder) {
                settings.check_filled = true;
                settings
                    .check
                    .update(cx, |input, cx| input.set_value(check, window, cx));
            }
        });
    }
    if settings.spaces != space_ids(bench) {
        let folder = folder.to_owned();
        cx.defer_in(window, move |bench, window, cx| {
            remake_space(bench, folder, window, cx)
        });
    }
    let t = Tokens::get(cx).clone();
    let this = cx.entity();
    let name = bench
        .project_at(folder)
        .map_or_else(|| folder.to_owned(), |project| project.name.clone());
    let invalid = settings
        .config
        .as_ref()
        .is_some_and(|config| config["status"] == "invalid");
    let nav = PAGES.iter().map(|&(page, label, icon)| {
        let (this, folder) = (this.clone(), folder.to_owned());
        let on = settings.page == page;
        let ink = if on { t.text_primary } else { t.text_secondary };
        div()
            .id(SharedString::from(format!("project-page-{label}")))
            .flex()
            .items_center()
            .gap(SPACE_2)
            .h(px(28.))
            .px(SPACE_2)
            .rounded(theme::RADIUS_2)
            .text_size(crate::theme::ui_text(TEXT_SM))
            .text_color(ink)
            .when(on, |row| row.bg(t.surface_chosen))
            .when(!on, |row| row.hover(|row| row.bg(t.surface_hover)))
            .child(ui::icon(icon, 13.).text_color(ink))
            .child(div().flex_1().child(label))
            .when(page == Page::Config && invalid, |row| {
                row.child(div().text_color(t.danger).child("!"))
            })
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| {
                    if let Some(settings) = bench.settings_of(&folder) {
                        settings.page = page;
                    }
                    bench.parts.redraw_stage(cx);
                })
            })
    });
    let revealed = PathBuf::from(folder);
    let side = div()
        .w(px(200.))
        .flex_none()
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .p(SPACE_3)
        .border_r_1()
        .border_color(t.border)
        .child(
            div()
                .px(SPACE_2)
                .pb(SPACE_2H)
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(15.))
                        .text_color(t.text_primary)
                        .truncate()
                        .child(name),
                )
                .child(
                    ui::mono(
                        crate::ui::path::tilde(folder, bench.home_at(&bench.place()).as_deref()),
                        TEXT_3XS,
                        cx,
                    )
                    .truncate(),
                ),
        )
        .children(nav)
        .child(div().flex_1())
        .child(
            Button::new("project-reveal", ui::REVEAL, Variant::Outline)
                .icon(Icon::Folder)
                .on_click(move |_, _, cx| cx.reveal_path(&revealed)),
        );
    let body: Vec<AnyElement> = match settings.page {
        Page::General => vec![general(bench, settings, folder, &this, cx)],
        Page::Tasks => tasks(bench, settings, folder, &this, cx),
        Page::Git => vec![git(bench, settings, folder, &this, cx)],
        Page::Config => vec![config(settings, folder, &this, cx)],
    };
    ui::pane(cx)
        .flex()
        .child(side)
        .child(
            div()
                .id(SharedString::from(format!("project-settings-{folder}")))
                .flex_1()
                .min_w_0()
                .overflow_y_scroll()
                .child(
                    div()
                        .max_w(px(720.))
                        .flex()
                        .flex_col()
                        .gap(px(22.))
                        .px(px(28.))
                        .py(SPACE_6)
                        .children(body),
                ),
        )
        .into_any_element()
}

fn general(
    bench: &Workbench,
    settings: &ProjectSettings,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let preferences = bench.preferences();
    let worktree = bench
        .project_at(folder)
        .is_some_and(|project| project.worktree_default);
    let servers = crate::editor::language_servers_allowed(bench, folder);
    let (on_worktree, on_servers) = (this.clone(), this.clone());
    let (worktree_folder, servers_folder) = (folder.to_owned(), folder.to_owned());
    let defaults = preferences.agent_worktree_defaults.clone();
    let body = div()
        .child(rows::row(
            "New chats start in a worktree",
            Some("An agent gets its own checkout, so its edits stay off your branch until you merge them."),
            Switch::new("project-worktree").checked(worktree).on_change(
                move |on: &bool, _, cx| {
                    let on = *on;
                    let defaults = defaults.clone();
                    on_worktree.update(cx, |bench, _| {
                        bench.set_project_map(
                            "agentWorktreeDefaults",
                            defaults,
                            &worktree_folder,
                            on.then_some(true),
                        )
                    })
                },
            ),
            cx,
        ))
        .child(rows::row(
            "Language servers",
            Some("They can run code from the project, such as its build scripts. Only allow them for projects you trust."),
            Switch::new("project-language-servers").checked(servers).on_change(
                move |on: &bool, _, cx| {
                    let on = *on;
                    on_servers.update(cx, |bench, _| {
                        crate::editor::allow_language_servers(bench, servers_folder.clone(), on)
                    })
                },
            ),
            cx,
        ))
        .child(rows::row(
            "Space",
            Some("The rail shows the project under this space."),
            settings.space.view(),
            cx,
        ));
    rows::section(
        "General",
        None,
        Some("Kept in Sikemux, not in the repository.".into()),
        false,
        body,
        cx,
    )
}

/// Whether the person made the task, for the team or for themselves, rather than the build
/// files it was found in.
fn own(task: &TaskView) -> bool {
    !matches!(task.source, Source::Detected | Source::Rerun)
}

fn run_button(
    id: SharedString,
    task: &TaskView,
    folder: &str,
    this: &Entity<Workbench>,
) -> IconButton {
    let label = &task.label;
    let (this, folder) = (this.clone(), folder.to_owned());
    let task_id = task.id.clone();
    IconButton::new(id, Icon::Run, format!("Run {label}"))
        .small()
        .on_click(move |_, _, cx| {
            this.update(cx, |bench, cx| {
                bench.start_task_in(folder.clone(), &task_id, cx)
            })
        })
}

fn tasks(
    bench: &Workbench,
    settings: &ProjectSettings,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let listed = bench
        .reads
        .tasks
        .as_ref()
        .filter(|read| read.project.as_os_str() == folder)
        .map(|read| read.tasks.clone());
    let all = listed.clone().unwrap_or_default();
    let mine: Vec<&TaskView> = all.iter().filter(|task| own(task)).collect();
    let mine_rows = mine.iter().enumerate().map(|(at, task)| {
        let (remove, folder_owned, task_owned) = (this.clone(), folder.to_owned(), (*task).clone());
        let cwd = task.cwd.clone();
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .px(SPACE_3H)
            .py(SPACE_2)
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_SM))
                            .text_color(t.text_primary)
                            .child(task.label.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(theme::MONO)
                            .text_size(crate::theme::ui_text(TEXT_2XS_H))
                            .text_color(t.text_tertiary)
                            .child(task.command.clone()),
                    ),
            )
            .when(cwd != ".", |row| row.child(ui::meta(cwd, cx)))
            .child(ui::meta(
                if task.source == Source::Personal {
                    "Only me"
                } else {
                    "Team"
                },
                cx,
            ))
            .child(run_button(
                SharedString::from(format!("own-run-{at}")),
                task,
                folder,
                this,
            ))
            .child({
                let (editing, folder, task) = (this.clone(), folder.to_owned(), (*task).clone());
                IconButton::new(
                    SharedString::from(format!("own-edit-{at}")),
                    Icon::Pencil,
                    "Edit",
                )
                .small()
                .on_click(move |_, window, cx| {
                    editing.update(cx, |bench, cx| bench.edit_task(&folder, &task, window, cx))
                })
            })
            .child(
                IconButton::new(
                    SharedString::from(format!("own-remove-{at}")),
                    Icon::Trash,
                    "Remove",
                )
                .small()
                .on_click(move |_, _, cx| {
                    remove.update(cx, |bench, cx| {
                        bench.remove_task(&folder_owned, &task_owned, cx)
                    })
                }),
            )
            .into_any_element()
    });
    let mine_body: AnyElement = match (&listed, mine.is_empty()) {
        (None, _) => rows::note("Reading the project's tasks…", cx),
        (Some(_), true) => rows::note("No tasks of your own yet.", cx),
        (Some(_), false) => div().children(mine_rows).into_any_element(),
    };
    let adding = this.clone();
    let add = Button::new("project-add-task", "Add a task", Variant::Outline)
        .icon(Icon::Plus)
        .on_click(move |_, window, cx| adding.update(cx, |bench, cx| bench.add_task(window, cx)));
    let mut sections = vec![rows::section_with(
        "Your tasks",
        None,
        Some(add.into_any_element()),
        Some("Commands the run button and agents start. For the team goes in sikemux.json, which you commit; Only me stays in .sikemux/, which Git ignores.".into()),
        false,
        mine_body,
        cx,
    )];
    let found: Vec<&TaskView> = all.iter().filter(|task| !own(task)).collect();
    if !found.is_empty() {
        let words = settings.filter.read(cx).value().trim().to_lowercase();
        let shown: Vec<&TaskView> = found
            .iter()
            .copied()
            .filter(|task| {
                words.is_empty()
                    || format!("{} {} {}", task.label, task.command, task.cwd)
                        .to_lowercase()
                        .contains(&words)
            })
            .collect();
        let mut folders: Vec<(String, Vec<&TaskView>)> = Vec::new();
        for task in &shown {
            let cwd = task.cwd.clone();
            match folders.iter_mut().find(|(each, _)| *each == cwd) {
                Some((_, group)) => group.push(task),
                None => folders.push((cwd, vec![task])),
            }
        }
        let single = folders.len() == 1;
        let groups = folders.into_iter().map(|(cwd, group)| {
            let open =
                !words.is_empty() || single || settings.opened.contains(&cwd) != (cwd == ".");
            let (toggle, folder_owned, cwd_owned) = (this.clone(), folder.to_owned(), cwd.clone());
            let suffix = format!(" ({cwd})");
            let head = div()
                .id(SharedString::from(format!("found-folder-{cwd}")))
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .px(SPACE_3H)
                .py(px(7.))
                .text_size(crate::theme::ui_text(TEXT_XS_H))
                .text_color(t.text_secondary)
                .hover(|row| row.bg(t.surface_hover))
                .child(motion::turn(
                    format!("task-folder-chevron:{cwd}"),
                    motion::fold_angle(open),
                    motion::LIST_FOLD,
                    ui::icon(Icon::Chevron, 11.),
                ))
                .child(div().flex_1().child(if cwd == "." {
                    "Project root".to_owned()
                } else {
                    cwd.clone()
                }))
                .child(ui::meta(group.len().to_string(), cx))
                .on_click(move |_, _, cx| {
                    toggle.update(cx, |bench, cx| {
                        if let Some(settings) = bench.settings_of(&folder_owned) {
                            if !settings.opened.remove(&cwd_owned) {
                                settings.opened.insert(cwd_owned.clone());
                            }
                        }
                        bench.parts.redraw_stage(cx);
                    })
                });
            let tasks = group
                .into_iter()
                .enumerate()
                .filter(|_| open)
                .map(|(at, task)| {
                    let label = task.label.clone();
                    let label = label
                        .strip_suffix(&suffix)
                        .map_or(label.clone(), str::to_owned);
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_3)
                        .pl(px(31.))
                        .pr(SPACE_3H)
                        .py(px(5.))
                        .child(
                            div()
                                .w(px(160.))
                                .flex_none()
                                .truncate()
                                .text_size(crate::theme::ui_text(TEXT_XS_H))
                                .text_color(t.text_primary)
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(theme::MONO)
                                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                                .text_color(t.text_tertiary)
                                .child(task.command.clone()),
                        )
                        .child(run_button(
                            SharedString::from(format!("found-run-{cwd}-{at}")),
                            task,
                            folder,
                            this,
                        ))
                });
            div()
                .border_b_1()
                .border_color(t.line_soft)
                .child(head)
                .children(tasks)
                .into_any_element()
        });
        let body = div()
            .child(
                div()
                    .px(SPACE_3H)
                    .py(SPACE_2H)
                    .border_b_1()
                    .border_color(t.line_soft)
                    .child(Field::new(&settings.filter)),
            )
            .children(groups)
            .when(shown.is_empty(), |body| {
                body.child(rows::note("No task matches.", cx))
            });
        sections.push(rows::section(
            "Found in the build files",
            Some(format!("{} tasks", shown.len()).into()),
            Some("Read from package.json, Cargo, Maven, Gradle and the rest; change those files to change these.".into()),
            false,
            body,
            cx,
        ));
    }
    let invalid = settings
        .config
        .as_ref()
        .is_some_and(|config| config["status"] == "invalid");
    sections.push(rows::section(
        "Check",
        None,
        Some("Sikemux runs it on each commit, and agents run it on a card's change when its pull request has no CI.".into()),
        false,
        rows::row(
            "Check command",
            Some(if invalid {
                "sikemux.json has mistakes; fix them to change the check."
            } else {
                "Saved in sikemux.json for everyone. Leave it empty for none."
            }),
            div().w(px(240.)).child(Field::new(&settings.check).mono()),
            cx,
        ),
        cx,
    ));
    sections
}

fn git(
    bench: &Workbench,
    settings: &ProjectSettings,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let itself = bench
        .project_at(folder)
        .is_some_and(|project| project.git.is_some());
    let preferences = bench.preferences();
    let chosen = preferences.default_git_repos.get(folder).cloned();
    let body: AnyElement = match &settings.repos {
        _ if itself => rows::note(
            "This project folder is a repository itself, so its Git views use it.",
            cx,
        ),
        None => rows::note("Looking for repositories…", cx),
        Some(repos) if repos.is_empty() => rows::note(
            "There is no repository in this folder or the folders directly inside it.",
            cx,
        ),
        Some(repos) => {
            let choice = |id: SharedString,
                          on: bool,
                          mark: Option<Icon>,
                          name: String,
                          detail: String,
                          changes: u64,
                          pick: Option<String>| {
                let (this, folder, repos) = (
                    this.clone(),
                    folder.to_owned(),
                    preferences.default_git_repos.clone(),
                );
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(SPACE_2H)
                    .px(SPACE_3H)
                    .py(px(9.))
                    .border_b_1()
                    .border_color(t.line_soft)
                    .when(on, |row| row.bg(t.surface_chosen))
                    .when(!on, |row| row.hover(|row| row.bg(t.surface_hover)))
                    .children(mark.map(|mark| ui::icon(mark, 12.).text_color(t.text_secondary)))
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_SM))
                            .text_color(t.text_primary)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(crate::theme::ui_text(TEXT_2XS_H))
                            .text_color(t.text_tertiary)
                            .child(detail),
                    )
                    .when(changes > 0, |row| {
                        row.child(ui::meta(format!("{changes} changed"), cx))
                    })
                    .on_click(move |_, _, cx| {
                        let (repos, pick) = (repos.clone(), pick.clone());
                        this.update(cx, |bench, _| {
                            bench.set_project_map("defaultGitRepos", repos, &folder, pick)
                        })
                    })
                    .into_any_element()
            };
            let only = repos.len() == 1;
            let first = choice(
                "repo-ask".into(),
                chosen.is_none(),
                None,
                if only {
                    "The only one inside"
                } else {
                    "Ask each time"
                }
                .to_owned(),
                match only {
                    true => repos[0]["name"].as_str().unwrap_or_default().to_owned(),
                    false => "the Git pane lists them".to_owned(),
                },
                0,
                None,
            );
            let listed = repos.iter().map(|repo| {
                let path = repo["path"].as_str().unwrap_or_default().to_owned();
                let branch = repo["branch"].as_str().unwrap_or_default();
                choice(
                    SharedString::from(format!("repo-{path}")),
                    chosen.as_deref() == Some(path.as_str()),
                    Some(Icon::Git),
                    repo["name"].as_str().unwrap_or_default().to_owned(),
                    if branch.is_empty() {
                        "no commits"
                    } else {
                        branch
                    }
                    .to_owned(),
                    repo["changes"].as_u64().unwrap_or(0),
                    Some(path),
                )
            });
            let opened = chosen.as_deref().and_then(|chosen| {
                Path::new(chosen)
                    .file_name()
                    .map(|name| format!("Git views open {}.", name.to_string_lossy()))
            });
            div()
                .child(first)
                .children(listed)
                .children(opened.map(|opened| rows::note(opened, cx)))
                .into_any_element()
        }
    };
    rows::section(
        "Default git repository",
        None,
        Some("When the project folder holds several repositories, its Git pane, the branch in the top bar, the diff pane and new agent worktrees use this one.".into()),
        false,
        body,
        cx,
    )
}

fn config(
    settings: &ProjectSettings,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(config) = &settings.config else {
        return rows::section(
            "sikemux.json",
            None,
            None,
            false,
            rows::note("Reading sikemux.json…", cx),
            cx,
        );
    };
    let status = config["status"].as_str().unwrap_or("absent");
    let trusted = config["trusted"].as_bool().unwrap_or(false);
    let errors = config["errors"].as_array().cloned().unwrap_or_default();
    let (label, about) = match (status, trusted) {
        ("absent", _) => (
            "Not made yet",
            "Adding a task for the team makes it.".to_owned(),
        ),
        ("invalid", _) => (
            "Has mistakes",
            format!(
                "{} {}; Sikemux ignores the file until they are fixed.",
                errors.len(),
                if errors.len() == 1 {
                    "problem"
                } else {
                    "problems"
                }
            ),
        ),
        (_, true) => (
            "Trusted",
            "Sikemux runs its commands without asking until the file changes.".to_owned(),
        ),
        (_, false) => (
            "Not trusted yet",
            "Sikemux asks before it runs any of its commands.".to_owned(),
        ),
    };
    let path = PathBuf::from(config["path"].as_str().unwrap_or_default());
    let (trusting, opening, folder) = (this.clone(), this.clone(), folder.to_owned());
    let controls = div()
        .flex()
        .gap(SPACE_1H)
        .when(status == "valid" && !trusted, |controls| {
            controls.child(
                Button::new("config-trust", "Trust", Variant::Outline).on_click(move |_, _, cx| {
                    trusting.update(cx, |bench, cx| bench.trust_config(&folder, cx))
                }),
            )
        })
        .when(status != "absent", |controls| {
            controls.child(
                Button::new("config-open", "Open", Variant::Outline).on_click(move |_, _, cx| {
                    opening.update(cx, |bench, cx| bench.open_file(&path, cx))
                }),
            )
        });
    let mistakes = errors.iter().map(|error| {
        div()
            .flex()
            .gap(SPACE_2)
            .px(SPACE_3H)
            .py(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_color(t.text_secondary)
                    .child(error["path"].as_str().unwrap_or_default().to_owned()),
            )
            .child(
                div()
                    .text_color(t.text_tertiary)
                    .child(error["message"].as_str().unwrap_or_default().to_owned()),
            )
    });
    rows::section(
        "sikemux.json",
        None,
        Some("The project's actions, tasks and preview, checked into the repository for everyone who opens it.".into()),
        false,
        div()
            .child(rows::row_with(None, label, Some(about.into()), controls, cx))
            .children(mistakes),
        cx,
    )
}
