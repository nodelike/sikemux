use std::f32::consts::FRAC_PI_2;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui::Anchor;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::Side;
use gpui_kit::component::WindowExt;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, AnyElement, App, ClickEvent, ClipboardItem, Context, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Transformation, WeakEntity, Window,
    WindowControlArea,
};
use serde_json::Value;
use sikemux_wire::protocol::{Pane, PaneKind, Tab as LayoutTab};

use crate::agent_menu::AgentMenu;
use crate::icons::Icon;
use crate::motion;
use crate::ports::PortRow;
use crate::theme::Tokens;
use crate::toast;
use crate::ui::{self, IconButton, Tab};
use crate::workbench::{Strip, Workbench};

/// The bar across the window's top, under the window buttons.
const BAR_HEIGHT: f32 = 36.;
/// The tabs' row under the bar, with the gutter above it and a little less below.
const TAB_ROW_HEIGHT: f32 = 42.;

/// How tall the bar and, when a place has tabs, the row of them under it stand.
pub fn top_height(bench: &Workbench) -> f32 {
    BAR_HEIGHT
        + if strip_with_tabs(bench).is_some() {
            TAB_ROW_HEIGHT
        } else {
            0.
        }
}

/// The strip in front while it holds a tab: a place with none shows no row at all.
fn strip_with_tabs(bench: &Workbench) -> Option<Strip> {
    bench.strip().filter(|strip| !bench.tabs(strip).is_empty())
}

/// The bar, holding the window buttons, where the window is and the chips, then the
/// project's tabs on a row of their own from the window's left edge to its right.
pub fn top_bar(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let tabs = strip_with_tabs(bench).map(|strip| strip_tabs(bench, &strip, cx));
    let bar = div()
        .id("top-bar")
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .h(px(BAR_HEIGHT))
        .when(!crate::see_through::clear(cx), |bar| bar.bg(t.surface))
        .border_b_1()
        .border_color(t.border)
        .window_control_area(WindowControlArea::Drag)
        .on_hover(cx.listener(|bench, hovered: &bool, _, cx| {
            if bench.top_bar_hovered == *hovered {
                return;
            }
            bench.top_bar_hovered = *hovered;
            for bar in bench.plugin_bars.values() {
                bar.update(cx, |_, cx| cx.notify());
            }
        }))
        .child(div().w(px(80.)).flex_none())
        .child(div().flex_1())
        .child(crate::top_place::view(bench, cx))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .pr(px(16.))
                .children(bench.plugin_bars.values().cloned())
                .children(
                    git_chip(bench, cx)
                        .map(|chip| [Some(chip), ci_glyph(bench, cx), Some(separator(cx))])
                        .into_iter()
                        .flatten()
                        .flatten(),
                )
                .children(
                    run_chip(bench, cx)
                        .map(|chip| [chip, separator(cx)])
                        .into_iter()
                        .flatten(),
                )
                .children(
                    crate::ports::chip(bench, cx)
                        .map(|chip| [chip, separator(cx)])
                        .into_iter()
                        .flatten(),
                )
                .children(battery_chip(bench, cx))
                .child(clock(cx))
                .child(
                    IconButton::new(
                        "focus",
                        Icon::Focus,
                        match crate::keymap::hint("view.focusMode", cx) {
                            Some(keys) => format!("Focus mode — hide rails — {keys}"),
                            None => "Focus mode — hide rails".to_owned(),
                        },
                    )
                    .width(30.)
                    .pressed(!bench.side_rail_open && !bench.tab_rail_open)
                    .on_click(cx.listener(|bench, _, _, cx| bench.toggle_focus_mode(cx))),
                ),
        );
    let row = tabs.map(|(copy, tabs)| {
        let glide = bench.strip_glide.borrow();
        let skip = usize::from(copy.is_some());
        div()
            .on_children_prepainted(glide.keep_origin())
            .id("tab-row")
            .h(px(TAB_ROW_HEIGHT))
            .flex()
            .flex_none()
            .items_center()
            .pt(px(10.))
            .pb(px(6.))
            .px(px(10.))
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .h_full()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .relative()
                    .children(copy)
                    .children(tabs)
                    .on_children_prepainted(glide.keep_items(skip)),
            )
            .child(
                IconButton::new("new-tab", Icon::Plus, "New tab")
                    .glyph(13.)
                    .on_click(|_, window, cx| {
                        crate::keymap::dispatch("palette.newTab", window, cx)
                    }),
            )
    });
    div().flex().flex_col().w_full().child(bar).children(row)
}

fn separator(cx: &App) -> AnyElement {
    div()
        .w(px(1.))
        .h(px(16.))
        .bg(Tokens::get(cx).line)
        .into_any_element()
}

/// Where the project's repository stands: its branch, and how far it is from its upstream.
fn git_chip(bench: &Workbench, cx: &App) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let project = bench.project()?;
    let git = project.git.as_ref()?;
    let host = project.code_host.as_ref();
    let mark = match host {
        Some(host) if host.host == "github" => {
            ui::icon(Icon::Github, 12.).text_color(t.text_primary)
        }
        _ => ui::icon(Icon::Git, 12.).text_color(if git.dirty { t.warn } else { t.live }),
    };
    let mut about: Vec<String> = host
        .map(|host| vec![host_label(&host.host).to_owned(), host.repo.clone()])
        .unwrap_or_default();
    about.push(git.branch.clone());
    about.push(if git.dirty {
        "changed".into()
    } else {
        "clean".into()
    });
    about.extend((git.ahead > 0).then(|| format!("ahead {}", git.ahead)));
    about.extend((git.behind > 0).then(|| format!("behind {}", git.behind)));
    let count = |arrow: &str, count: u32, color| {
        (count > 0)
            .then(|| ui::text(format!("{arrow}{count}"), 10., FontWeight::NORMAL).text_color(color))
    };
    Some(
        div()
            .id("git-chip")
            .h(px(24.))
            .max_w(px(200.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(crate::theme::RADIUS_1)
            .font_family(crate::theme::MONO)
            .text_size(crate::theme::ui_text(11.))
            .text_color(t.text_tertiary)
            .hover(move |style| style.text_color(t.text_primary))
            .tooltip(ui::tooltip(about.join(" · ")))
            .on_click(|_, window, cx| crate::keymap::dispatch("window.git", window, cx))
            .child(mark)
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(t.text_primary)
                    .child(git.branch.clone()),
            )
            .when(git.ahead > 0 || git.behind > 0, |chip| {
                chip.child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .children(count("↑", git.ahead, t.live))
                        .children(count("↓", git.behind, t.warn)),
                )
            })
            .into_any_element(),
    )
}

/// How the branch's latest CI run went, in the space of one mark.
fn ci_glyph(bench: &Workbench, cx: &App) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let run = bench.latest_run()?;
    let outcome = run["outcome"].as_str()?;
    let number = run["runNumber"].as_u64()?;
    let ago = chrono::DateTime::parse_from_rfc3339(run["createdAt"].as_str()?)
        .ok()?
        .timestamp_millis()
        .max(0) as u64;
    let ago = match crate::agents::age(ago) {
        now if now == "now" => now,
        age => format!("{age} ago"),
    };
    let about = format!(
        "{} · {} #{number} · {ago}",
        outcome_label(outcome),
        run["name"].as_str().unwrap_or_default()
    );
    Some(
        div()
            .id("ci-glyph")
            .on_click(|_, window, cx| {
                let area = crate::codehost::Area::Actions;
                window.dispatch_action(Box::new(crate::codehost::OpenArea { area }), cx)
            })
            .h(px(24.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(crate::theme::RADIUS_1)
            .text_size(crate::theme::ui_text(11.))
            .text_color(t.text_secondary)
            .hover(move |style| style.text_color(t.text_primary))
            .tooltip(ui::tooltip(about))
            .child(ui::status_of_run(outcome, 12., cx))
            .child(
                ui::mono(format!("#{number}"), 11., cx)
                    .font_features(crate::theme::tabular())
                    .text_color(t.text_secondary),
            )
            .into_any_element(),
    )
}

/// How full this device's battery is, drawn as the app's battery icon.
fn battery_chip(bench: &Workbench, cx: &App) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let battery = bench.battery?;
    let percent = battery.percent;
    let tint = match percent {
        _ if battery.charging => t.live,
        0..=10 => t.danger,
        11..=20 => t.warn,
        _ => t.accent,
    };
    let time = battery
        .minutes
        .map(|minutes| format!("{}:{:02}", minutes / 60, minutes % 60));
    let about = match (battery.charging, time) {
        (true, Some(time)) => format!("{percent}% · charging · {time} until full"),
        (true, None) => format!("{percent}% · charging"),
        (false, Some(time)) => format!("{percent}% · {time} remaining"),
        (false, None) => format!("{percent}%"),
    };
    let unit = 12. / 13.;
    let at = |units: f32| px(units * unit);
    let level = f32::from(percent) / 100.;
    let fill = if level > 0. {
        (20.5 * level).max(2.2)
    } else {
        0.
    };
    let frame = Hsla { a: 0.45, ..tint };
    let bolt = |glyph: Icon, color: Hsla| {
        ui::icon(glyph, 19. * unit)
            .w(at(12.))
            .absolute()
            .left(at(7.))
            .top(at(-3.))
            .text_color(color)
    };
    let icon = div()
        .relative()
        .flex_none()
        .w(at(28.))
        .h(at(13.))
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(at(24.5))
                .h(at(13.))
                .border(at(1.))
                .border_color(frame)
                .rounded(at(4.1)),
        )
        .child(
            div()
                .absolute()
                .left(at(25.4))
                .top(at(4.4))
                .w(at(1.6))
                .h(at(4.2))
                .rounded_r(at(1.6))
                .bg(frame),
        )
        .child(
            div()
                .absolute()
                .left(at(2.))
                .top(at(2.))
                .w(at(fill))
                .h(at(9.))
                .rounded(at(2.2))
                .bg(tint),
        )
        .when(battery.charging, |icon| {
            icon.child(bolt(Icon::BatteryBoltGap, t.surface_sunken))
                .child(bolt(Icon::BatteryBolt, tint))
        });
    Some(
        div()
            .id("battery")
            .flex()
            .items_center()
            .gap(px(5.))
            .tooltip(ui::tooltip(about))
            .child(icon)
            .child(
                ui::text(format!("{percent}%"), 11., FontWeight::SEMIBOLD)
                    .font_features(crate::theme::tabular())
                    .text_color(tint),
            )
            .into_any_element(),
    )
}

fn outcome_label(outcome: &str) -> &str {
    match outcome {
        "running" => "Running",
        "queued" => "Queued",
        "success" => "Passed",
        "failure" => "Failed",
        "cancelled" => "Cancelled",
        "skipped" => "Skipped",
        "blocked" => "Waiting for approval",
        _ => "Unknown",
    }
}

fn host_label(host: &str) -> &str {
    match host {
        "github" => "GitHub",
        "gitlab" => "GitLab",
        "bitbucket" => "Bitbucket",
        other => other,
    }
}

/// Runs the project's task, or stops the run going, and says how the last one went.
fn run_chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let tasks = bench.read("tasks")?["tasks"].as_array()?;
    let project = bench.watched_project()?;
    let ports = bench
        .read("ports")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let (running, last) = bench.person_runs();
    let task_of = |run: &Value| tasks.iter().find(|task| task["id"] == run["taskId"]);
    let picked = bench
        .picked_tasks
        .get(project)
        .and_then(|id| tasks.iter().find(|task| task["id"] == id.as_str()));
    let selected = running.and_then(task_of).or(picked).or(tasks.first())?;
    let id = selected["id"].as_str()?.to_owned();
    let label = selected["label"].as_str()?.to_owned();
    let last = last.filter(|run| running.is_none() && run["taskId"] == id.as_str());
    let finished = last.map(|run| crate::test_card::Finished {
        run,
        task: selected,
    });
    let outcome = finished.as_ref().and_then(crate::test_card::status);
    let failed = outcome
        .as_ref()
        .filter(|(_, wrong)| *wrong)
        .map(|(text, _)| text.clone());
    let passed = outcome
        .as_ref()
        .filter(|(_, wrong)| !*wrong)
        .map(|(text, _)| text.clone());
    let port = running.and_then(|run| {
        ports
            .iter()
            .find(|port| port["owner"]["executionId"] == run["executionId"])
    });
    let port_number = port.and_then(|port| port["port"].as_u64());
    let needs_java = selected["java"]["runsOn"]
        .is_null()
        .then(|| &selected["java"]["required"])
        .filter(|required| !required.is_null())
        .map(|required| format!("needs Java {}", plain(required)));
    let said = |text: String, color: Hsla| {
        div()
            .text_color(color)
            .child(format!("· {text}"))
            .into_any_element()
    };
    let status = match (port_number, running, &failed, &passed) {
        (Some(port), ..) => Some(said(format!(":{port}"), t.text_tertiary)),
        (None, Some(_), ..) => Some(bench.run_clock.clone().into_any_element()),
        (None, None, Some(failed), _) => Some(said(failed.clone(), t.danger)),
        (None, None, None, Some(passed)) => Some(said(passed.clone(), t.live)),
        _ => needs_java.map(|text| said(text, t.warn)),
    };
    let dot = match (running, &failed) {
        (Some(_), _) if port.is_some() => Some(div().size(px(7.)).rounded_full().bg(t.live)),
        (Some(_), _) => Some(
            div()
                .size(px(7.))
                .rounded_full()
                .border(px(1.5))
                .border_color(t.warn),
        ),
        (None, Some(_)) => Some(div().size(px(7.)).rounded_full().bg(t.danger)),
        (None, None) if passed.is_some() => Some(div().size(px(7.)).rounded_full().bg(t.live)),
        _ => None,
    };
    let execution = running
        .and_then(|run| run["executionId"].as_str())
        .map(str::to_owned);
    let output = running
        .or(last.filter(|_| failed.is_some()))
        .and_then(|run| run["ptyId"].as_u64())
        .map(|session| (session, label.clone()));
    let card = crate::run_check::card(bench, project, cx).or_else(|| {
        finished
            .as_ref()
            .and_then(|finished| crate::test_card::card(bench, finished, output.clone(), cx))
    });
    let why = finished
        .as_ref()
        .filter(|finished| crate::test_card::offers_why(bench, finished))
        .map(|_| {
            let tip = if passed.is_none() && last.is_some_and(|run| run["tests"].is_object()) {
                "Show the failed tests"
            } else {
                "Show why it stopped"
            };
            Button::new("run-why")
                .ghost()
                .compact()
                .h(px(24.))
                .px(px(8.))
                .text_size(crate::theme::ui_text(11.))
                .text_color(t.danger)
                .tooltip(tip)
                .child("Why?")
                .on_click(cx.listener(|bench, _, _, cx| {
                    crate::test_card::why(bench);
                    cx.notify();
                }))
        });
    let menu = Rc::new(RunMenu {
        bench: cx.entity().downgrade(),
        running: running.map(|_| RunningTask {
            id: id.clone(),
            label: label.clone(),
            started_at: running.and_then(|run| run["startedAt"].as_i64()),
            port: port_number,
        }),
        output: output.clone(),
        groups: task_groups(tasks, project),
        selected: id.clone(),
        others: ports
            .iter()
            .filter(|other| other["owner"]["kind"] != "terminal" && Some(*other) != port)
            .filter_map(|other| PortRow::new(other, bench))
            .collect(),
        finders: crate::send_to::choices(bench),
        project_name: bench
            .project()
            .map(|project| project.name.clone())
            .unwrap_or_default(),
    });
    let primary = if running.is_some() {
        format!("Stop {label}")
    } else {
        format!("Run {label}")
    };
    let (glyph, tint) = if running.is_some() {
        (Icon::Stop, t.danger)
    } else {
        (Icon::Run, t.live)
    };
    let chip = div()
        .h(px(24.))
        .flex()
        .rounded(crate::theme::RADIUS_2)
        .bg(Hsla {
            a: 0.94,
            ..t.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.06,
            ..t.text_primary
        })
        .child(
            div()
                .id("run-task")
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(crate::theme::ui_text(12.))
                .text_color(t.text_secondary)
                .hover(move |style| style.text_color(t.text_primary).bg(t.surface_hover))
                .tooltip(ui::tooltip(primary))
                .on_click(cx.listener(move |bench, _, _, cx| match &execution {
                    Some(execution) => bench.stop_run(execution),
                    None => bench.start_task(&id, cx),
                }))
                .child(ui::icon(glyph, 11.).text_color(tint)),
        )
        .child(
            Button::new("choose-task")
                .ghost()
                .compact()
                .rounded(px(0.))
                .h_full()
                .max_w(px(320.))
                .pl(px(8.))
                .pr(px(6.))
                .gap(px(5.))
                .border_l_1()
                .border_color(Hsla {
                    a: 0.08,
                    ..t.text_primary
                })
                .font_family(crate::theme::MONO)
                .text_size(crate::theme::ui_text(11.))
                .text_color(t.text_tertiary)
                .children(dot)
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(label),
                )
                .children(status)
                .child(
                    ui::icon(Icon::Chevron, 10.)
                        .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                        .text_color(t.text_tertiary),
                )
                .dropdown_menu_with_anchor(Anchor::TopRight, move |popup, window, cx| {
                    menu.build(popup, window, cx)
                }),
        );
    Some(
        div()
            .relative()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(chip)
            .children(why)
            .children(card)
            .into_any_element(),
    )
}

/// How long the person's task has run, counted on the Run chip each second. It keeps its own
/// clock and draws itself again in place, leaving the bar around it as it was.
#[derive(Default)]
pub struct RunClock {
    /// When the task started, in milliseconds, while it counts.
    started_at: Option<Option<i64>>,
    _ticking: Option<Task<()>>,
}

impl RunClock {
    /// Counts from `started_at` while `counting`, or stops.
    pub fn set(&mut self, counting: bool, started_at: Option<i64>, cx: &mut Context<Self>) {
        let next = counting.then_some(started_at);
        if next == self.started_at {
            return;
        }
        self._ticking = counting.then(|| {
            cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.repaint()).is_err() {
                    break;
                }
            })
        });
        self.started_at = next;
        cx.notify();
    }
}

impl Render for RunClock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx);
        let since = seconds_since(self.started_at.flatten());
        div()
            .text_color(t.text_tertiary)
            .child(format!("· {since}s"))
    }
}

/// Whole seconds since `started_at`, in milliseconds; none counts as just started.
fn seconds_since(started_at: Option<i64>) -> i64 {
    started_at.map_or(0, |at| {
        (chrono::Utc::now().timestamp_millis() - at).max(0) / 1000
    })
}

/// What the Run chip's menu lists, as it was when the chip was drawn.
struct RunMenu {
    bench: WeakEntity<Workbench>,
    running: Option<RunningTask>,
    /// The terminal the task runs in, or ran in when it failed, and the task's name.
    output: Option<(u64, String)>,
    groups: Vec<TaskGroup>,
    selected: String,
    /// Other things listening in the project, that tasks or agents started.
    others: Vec<PortRow>,
    /// Who can be asked to find the project's tasks, and the project's name for the asking.
    finders: Vec<crate::send_to::Choice>,
    project_name: String,
}

struct RunningTask {
    id: String,
    label: String,
    started_at: Option<i64>,
    port: Option<u64>,
}

struct TaskGroup {
    title: String,
    meta: Option<String>,
    /// Each task's id, and its label without the folder the group's heading names.
    tasks: Vec<(String, String)>,
}

impl RunMenu {
    fn build(
        &self,
        popup: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let mut popup = popup
            .check_side(Side::Right)
            .scrollable(true)
            .min_w(px(220.))
            .max_w(px(360.))
            .max_h(px(480.));
        if let Some(running) = &self.running {
            let meta = match running.port {
                Some(port) => format!("up · :{port}"),
                None => format!("running · {}s", seconds_since(running.started_at)),
            };
            popup = popup.label(format!("{} · {meta}", running.label));
            if let Some(port) = running.port {
                let url = format!("http://localhost:{port}");
                popup = popup.item(
                    PopupMenuItem::new(format!("Open localhost:{port}"))
                        .icon(Icon::Globe)
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            let task = running.id.clone();
            popup = popup.item(PopupMenuItem::new("Restart").icon(Icon::Refresh).on_click(
                on_bench(&self.bench, move |bench, cx| bench.start_task(&task, cx)),
            ));
        }
        if let Some((session, label)) = &self.output {
            let (session, label) = (session.to_string(), label.clone());
            popup = popup.item(
                PopupMenuItem::new("Show output")
                    .icon(Icon::Terminal)
                    .on_click(on_bench(&self.bench, move |bench, _| {
                        bench.show("terminal", &session, Some(&label))
                    })),
            );
        }
        for group in &self.groups {
            popup = popup.separator().label(match &group.meta {
                Some(meta) => format!("{} · {meta}", group.title),
                None => group.title.clone(),
            });
            for (task, label) in &group.tasks {
                let picked = task.clone();
                popup = popup.item(
                    PopupMenuItem::new(label.clone())
                        .icon(Icon::Run)
                        .checked(*task == self.selected)
                        .on_click(on_bench(&self.bench, move |bench, cx| {
                            bench.pick_task(&picked, cx)
                        })),
                );
            }
        }
        if !self.others.is_empty() {
            popup = popup.separator().label("Running now");
        }
        for other in &self.others {
            popup = popup.item(other.running_item(&self.bench));
        }
        let bench = self.bench.clone();
        let popup = popup.separator().label("Your own tasks").item(
            PopupMenuItem::new("Add a task…")
                .icon(Icon::Plus)
                .on_click(move |_, window, cx| {
                    let bench = bench.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = bench.update(cx, |bench, cx| bench.add_task(window, cx));
                    });
                }),
        );
        if self.finders.is_empty() {
            return popup;
        }
        let (bench, finders, name) = (
            self.bench.clone(),
            self.finders.clone(),
            self.project_name.clone(),
        );
        popup.submenu(
            "Find tasks with an agent…",
            window,
            cx,
            move |menu, _, _| {
                finders.iter().fold(menu, |menu, choice| {
                    let (bench, to, name) = (bench.clone(), choice.to.clone(), name.clone());
                    menu.item(PopupMenuItem::new(choice.label.clone()).on_click(
                        move |_, window, cx| {
                            let _ = bench.update(cx, |bench, cx| {
                                bench.find_tasks_with(to.clone(), &name, window, cx)
                            });
                        },
                    ))
                })
            },
        )
    }
}

/// sikemux.json's tasks, then one group for each folder the build files' tasks came from.
fn task_groups(tasks: &[Value], project: &str) -> Vec<TaskGroup> {
    let text = |task: &Value, key: &str| task[key].as_str().unwrap_or_default().to_owned();
    let mut groups = Vec::new();
    let configured: Vec<_> = tasks
        .iter()
        .filter(|task| matches!(task["source"].as_str(), Some("project" | "personal")))
        .map(|task| (text(task, "id"), text(task, "label")))
        .collect();
    if !configured.is_empty() {
        groups.push(TaskGroup {
            title: "sikemux.json".into(),
            meta: None,
            tasks: configured,
        });
    }
    let mut folders: Vec<(&str, Vec<&Value>)> = Vec::new();
    for task in tasks
        .iter()
        .filter(|task| matches!(task["source"].as_str(), Some("detected" | "rerun")))
    {
        let cwd = task["cwd"].as_str().unwrap_or(".");
        match folders.iter_mut().find(|(folder, _)| *folder == cwd) {
            Some((_, folder)) => folder.push(task),
            None => folders.push((cwd, vec![task])),
        }
    }
    for (cwd, folder_tasks) in folders {
        let folder = if cwd == "." {
            Path::new(project).file_name().map_or_else(
                || project.to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        } else {
            cwd.to_owned()
        };
        let first = folder_tasks[0];
        let tool = first["command"]
            .as_str()
            .and_then(|command| command.split(' ').next())
            .and_then(|program| match program {
                "./mvnw" => Some("Maven wrapper"),
                "mvn" => Some("Maven"),
                "./gradlew" => Some("Gradle wrapper"),
                "gradle" => Some("Gradle"),
                "go" => Some("Go"),
                _ => None,
            })
            .map(str::to_owned);
        let java = &first["java"];
        let java = java.is_object().then(|| match &java["runsOn"] {
            Value::Null => format!("needs Java {}", plain(&java["required"])),
            runs_on => format!("Java {}", plain(runs_on)),
        });
        let meta: Vec<String> = tool.into_iter().chain(java).collect();
        let suffix = format!(" ({folder})");
        groups.push(TaskGroup {
            tasks: folder_tasks
                .iter()
                .map(|task| {
                    let label = text(task, "label");
                    let short = match cwd {
                        "." => label.as_str(),
                        _ => label.strip_suffix(&suffix).unwrap_or(&label),
                    };
                    (text(task, "id"), short.to_owned())
                })
                .collect(),
            title: folder,
            meta: (!meta.is_empty()).then(|| meta.join(" · ")),
        });
    }
    groups
}

/// A number or a string as plain text.
fn plain(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// The strip's tabs: one just opened widens into its place, and one just closed narrows away,
/// unless the strip changed most of its tabs at once, as another project's does. The chosen
/// surface glides to the tab brought to the front; that copy comes first.
fn strip_tabs(
    bench: &Workbench,
    strip: &Strip,
    cx: &mut Context<Workbench>,
) -> (Option<AnyElement>, Vec<AnyElement>) {
    let front = bench.tab().map(|tab| tab.id.clone());
    let listed = bench.tabs(strip);
    let shown = listed
        .iter()
        .map(|tab| (tab.id.clone(), tab.clone()))
        .collect();
    let lasting = motion::TAB_ARRIVAL
        .grow
        .duration
        .max(motion::TAB_DEPARTURE.close.duration);
    let mut seen = bench.strip_seen.borrow_mut();
    let (arriving, left) = seen.update(format!("{strip:?}"), shown, 2, lasting);
    let mut order: Vec<String> = listed.iter().map(|tab| tab.id.clone()).collect();
    for gone in left {
        order.insert(gone.at.min(order.len()), gone.id.clone());
    }
    let mut glide = bench.strip_glide.borrow_mut();
    let glided = glide.choose(front.clone(), cx.reduce_motion());
    glide.set_order(order);
    let chosen = Tokens::get(cx).surface_chosen;
    let copy = glide.copy(move |copy| copy.rounded(crate::theme::RADIUS_2).bg(chosen));
    let mut drawn: Vec<AnyElement> = listed
        .iter()
        .map(|tab| {
            let active = front.as_deref() == Some(tab.id.as_str());
            let drawn = strip_tab(bench, strip, tab, active, glided && active, true, cx);
            match arriving.contains(&tab.id) {
                true => {
                    let key = SharedString::from(format!("tab-arrive:{}", tab.id));
                    motion::arriving(key, motion::TAB_ARRIVAL, drawn).into_any_element()
                }
                false => drawn,
            }
        })
        .collect();
    for gone in left {
        let tab = strip_tab(bench, strip, &gone.item, false, false, false, cx);
        let key = SharedString::from(format!("tab-depart:{}", gone.id));
        let tab = motion::departing(key, gone.since, motion::TAB_DEPARTURE, tab);
        drawn.insert(gone.at.min(drawn.len()), tab.into_any_element());
    }
    (copy, drawn)
}

/// A tab of the strip; one that is `live` takes clicks, one drawn only while it closes does not.
fn strip_tab(
    bench: &Workbench,
    strip: &Strip,
    tab: &LayoutTab,
    active: bool,
    glided: bool,
    live: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let tab_id = tab.id.clone();
    let mut base = Tab::new(
        SharedString::from(format!("tab-{}", tab.id)),
        label(bench, tab),
    )
    .active(active)
    .glided(glided)
    .italic(tab.preview);
    if live {
        base = base
            .on_click(cx.listener({
                let (strip, tab) = (strip.clone(), tab_id.clone());
                move |bench, event: &ClickEvent, _, _| match event.click_count() {
                    2 => bench.keep_tab(&strip, &tab),
                    _ => bench.activate_tab(&strip, &tab),
                }
            }))
            .on_close(cx.listener({
                let strip = strip.clone();
                move |bench, _, _, _| bench.close_tab(&strip, &tab_id)
            }));
    }
    let view = first_view(&tab.pane);
    let shown = match view {
        Some((PaneKind::Agent, target)) => match match strip {
            Strip::Project(project) => bench.agents_of(project),
            Strip::Host(_) | Strip::Terminals => Vec::new(),
        }
        .into_iter()
        .find(|agent| &agent.id == target)
        {
            Some(agent) => base
                .mark(ui::agent_mark(&agent, 19., cx))
                .note(
                    agent
                        .name
                        .as_ref()
                        .map(|name| SharedString::from(format!("@{name}"))),
                )
                .tail(ui::status_mark(&agent.id, agent.status, cx)),
            None => base,
        },
        Some((kind, target)) => {
            let unsaved = kind == PaneKind::Files
                && bench
                    .editors
                    .is_dirty(&crate::editor::FileKey::new(bench.place(), target));
            base.mark(ui::picture(
                view_icon(kind, target),
                13.,
                if kind == PaneKind::Terminal {
                    t.live
                } else {
                    t.text_tertiary
                },
            ))
            .tail(unsaved.then(|| div().size(px(6.)).rounded_full().bg(t.warn)))
        }
        None => base,
    };
    if !live {
        return div().flex().min_w_0().child(shown).into_any_element();
    }
    let menu = tab_menu(bench, strip, tab, cx);
    let drawn = div()
        .id(SharedString::from(format!("tab-menu-{}", tab.id)))
        .flex()
        .min_w_0()
        .child(shown)
        .context_menu(menu)
        .into_any_element();
    let index = bench
        .tabs(strip)
        .iter()
        .position(|other| other.id == tab.id)
        .unwrap_or(0);
    let dragged = crate::drag::DraggedTab {
        strip: strip.clone(),
        tab: tab.id.clone(),
        label: label(bench, tab),
    };
    let reorder = bench
        .reorder
        .as_ref()
        .filter(|reorder| reorder.strip == *strip && cx.has_active_drag());
    crate::drag::tab(drawn, dragged, index, reorder, &cx.entity()).into_any_element()
}

/// What right-clicking a tab offers, wherever it is listed: an agent's menu for an
/// agent's tab, else the tab's own.
pub fn tab_menu(
    bench: &Workbench,
    strip: &Strip,
    tab: &LayoutTab,
    cx: &Context<Workbench>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let tab_menu = TabMenu::new(bench, strip, tab, cx);
    let agent_menu = match first_view(&tab.pane) {
        Some((PaneKind::Agent, agent)) => AgentMenu::new(bench, agent, cx),
        _ => None,
    };
    let separable = bench.separable(tab);
    let (bench, strip, tab) = (cx.entity().downgrade(), strip.clone(), tab.id.clone());
    move |popup, _, _| {
        let popup = match &agent_menu {
            Some(menu) => menu.build(popup),
            None => tab_menu.build(popup),
        };
        let Some((view, kind)) = separable.clone() else {
            return popup;
        };
        let (strip, tab) = (strip.clone(), tab.clone());
        popup.separator().item(
            PopupMenuItem::new(if kind == PaneKind::Files {
                "Move Focused File Back to Editor"
            } else {
                "Move Focused Pane Back to Tab Bar"
            })
            .on_click(on_bench(&bench, move |bench, _| {
                bench.separate_pane(&strip, &tab, &view)
            })),
        )
    }
}

/// What right-clicking a strip tab offers, as today's tab menu does.
struct TabMenu {
    bench: WeakEntity<Workbench>,
    strip: Strip,
    tab: String,
    kind: Option<PaneKind>,
    /// The file a Files tab shows, and that path inside its project.
    file: Option<(String, String)>,
    named: bool,
    label: SharedString,
    /// The other tabs Close Others closes, which leaves agents' tabs open.
    others: Vec<String>,
    /// The strip's file tabs in order, each with whether it is saved, for a file tab's closes.
    files: Vec<(String, bool)>,
}

impl TabMenu {
    fn new(bench: &Workbench, strip: &Strip, tab: &LayoutTab, cx: &Context<Workbench>) -> Self {
        let view = first_view(&tab.pane);
        let project = match strip {
            Strip::Project(id) => bench
                .projects()
                .iter()
                .find(|project| &project.id == id)
                .map(|project| project.path.clone()),
            Strip::Host(_) | Strip::Terminals => None,
        };
        let file = view
            .filter(|(kind, _)| *kind == PaneKind::Files)
            .map(|(_, path)| {
                let relative = project
                    .as_deref()
                    .and_then(|project| Path::new(path).strip_prefix(project).ok())
                    .map_or_else(
                        || path.clone(),
                        |inside| inside.to_string_lossy().into_owned(),
                    );
                (path.clone(), relative)
            });
        Self {
            bench: cx.entity().downgrade(),
            strip: strip.clone(),
            tab: tab.id.clone(),
            kind: view.map(|(kind, _)| kind),
            file,
            named: tab.title.is_some(),
            label: label(bench, tab),
            others: bench
                .tabs(strip)
                .iter()
                .filter(|other| other.id != tab.id)
                .filter(|other| !matches!(first_view(&other.pane), Some((PaneKind::Agent, _))))
                .map(|other| other.id.clone())
                .collect(),
            files: bench
                .tabs(strip)
                .iter()
                .filter_map(|other| match first_view(&other.pane)? {
                    (PaneKind::Files, path) => Some((
                        other.id.clone(),
                        !bench
                            .editors
                            .is_dirty(&crate::editor::FileKey::new(bench.place(), path)),
                    )),
                    _ => None,
                })
                .collect(),
        }
    }

    fn build(&self, mut popup: PopupMenu) -> PopupMenu {
        let (strip, tab) = (self.strip.clone(), self.tab.clone());
        if self.kind == Some(PaneKind::Terminal) {
            let (strip, tab, label) = (strip.clone(), tab.clone(), self.label.to_string());
            popup = popup.item(PopupMenuItem::new("Rename…").icon(Icon::Pencil).on_click(
                on_bench(&self.bench, move |bench, cx| {
                    let (strip, tab) = (strip.clone(), tab.clone());
                    bench.prompt(
                        "Rename tab",
                        label.clone(),
                        move |bench, name, _, _| {
                            if !name.is_empty() {
                                bench.rename_tab(&strip, &tab, &name);
                            }
                        },
                        cx,
                    );
                }),
            ));
        }
        if self.kind == Some(PaneKind::Terminal) && self.named {
            let (strip, tab) = (strip.clone(), tab.clone());
            popup = popup.item(
                PopupMenuItem::new("Use Automatic Name")
                    .icon(Icon::Rotate)
                    .on_click(on_bench(&self.bench, move |bench, _| {
                        bench.rename_tab(&strip, &tab, "")
                    })),
            );
        }
        if self.kind == Some(PaneKind::Terminal) {
            popup = popup.separator();
        }
        if self.file.is_some() {
            return self.file_closes(popup);
        }
        if self.kind != Some(PaneKind::Agent) {
            let (strip, tab) = (strip.clone(), tab.clone());
            popup = popup.item(
                PopupMenuItem::new("Duplicate")
                    .icon(Icon::SquarePlus)
                    .on_click(on_bench(&self.bench, move |bench, _| {
                        bench.duplicate_tab(&strip, &tab)
                    })),
            );
        }
        let close = {
            let (strip, tab) = (strip.clone(), tab.clone());
            on_bench(&self.bench, move |bench, _| bench.close_tab(&strip, &tab))
        };
        let others = self.others.clone();
        popup = popup
            .item(
                PopupMenuItem::new("Close")
                    .icon(Icon::Close)
                    .action(Box::new(crate::keymap::CloseTab))
                    .on_click(close),
            )
            .item(
                PopupMenuItem::new("Close Others")
                    .icon(Icon::Close)
                    .disabled(others.is_empty())
                    .on_click(on_bench(&self.bench, move |bench, _| {
                        bench.close_tabs(&strip, &others)
                    })),
            );
        popup
    }

    /// A file tab's closes and paths, as the Mac's file tab menu has them.
    fn file_closes(&self, popup: PopupMenu) -> PopupMenu {
        let strip = self.strip.clone();
        let at = self.files.iter().position(|(id, _)| *id == self.tab);
        let ids = |files: &[(String, bool)]| files.iter().map(|(id, _)| id.clone()).collect();
        let left: Vec<String> = at.map_or(Vec::new(), |at| ids(&self.files[..at]));
        let right: Vec<String> = at.map_or(Vec::new(), |at| ids(&self.files[at + 1..]));
        let others: Vec<String> = ids(&self.files)
            .into_iter()
            .filter(|id: &String| *id != self.tab)
            .collect();
        let saved: Vec<String> = self
            .files
            .iter()
            .filter(|(_, saved)| *saved)
            .map(|(id, _)| id.clone())
            .collect();
        let all: Vec<String> = ids(&self.files);
        let close = |label: &'static str, tabs: Vec<String>, always: bool| {
            let strip = strip.clone();
            PopupMenuItem::new(label)
                .icon(Icon::Close)
                .disabled(!always && tabs.is_empty())
                .on_click(on_bench(&self.bench, move |bench, _| {
                    bench.close_tabs(&strip, &tabs)
                }))
        };
        let mut popup = popup
            .item(
                close("Close", vec![self.tab.clone()], true)
                    .action(Box::new(crate::keymap::CloseTab)),
            )
            .item(close("Close Others", others, false))
            .item(close("Close to the Left", left, false))
            .item(close("Close to the Right", right, false))
            .item(close("Close Saved", saved, false))
            .item(close("Close All", all, true));
        if let Some((path, relative)) = &self.file {
            let (copied, relative, revealed) = (path.clone(), relative.clone(), path.clone());
            popup = popup
                .separator()
                .item(PopupMenuItem::new("Copy Path").icon(Icon::Copy).on_click(
                    move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                        window.push_notification(toast::success("Copied path"), cx);
                    },
                ))
                .item(
                    PopupMenuItem::new("Copy Relative Path")
                        .icon(Icon::Copy)
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(relative.clone()));
                            window.push_notification(toast::success("Copied relative path"), cx);
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Reveal in Finder")
                        .icon(Icon::Folder)
                        .on_click(move |_, _, cx| cx.reveal_path(Path::new(&revealed))),
                );
        }
        popup
    }
}

/// A menu item's click, doing `work` on the store.
fn on_bench(
    bench: &WeakEntity<Workbench>,
    work: impl Fn(&mut Workbench, &mut Context<Workbench>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let bench = bench.clone();
    move |_, _, cx| {
        let _ = bench.update(cx, |bench, cx| work(bench, cx));
    }
}

/// A tab's name: the person's, else what its first view shows.
pub fn label(bench: &Workbench, tab: &LayoutTab) -> SharedString {
    match &tab.title {
        Some(title) => title.clone().into(),
        None => view_label(bench, &tab.pane),
    }
}

/// What a pane is called by the first view in it: its agent's task, its shell's title or its kind.
pub fn view_label(bench: &Workbench, pane: &Pane) -> SharedString {
    match first_view(pane) {
        Some((PaneKind::Agent, target)) => bench
            .view
            .as_ref()
            .and_then(|view| {
                view.workspace
                    .agents
                    .iter()
                    .find(|agent| &agent.id == target)
            })
            .map_or_else(|| "Agent".into(), |agent| bench.title_of(agent).into()),
        Some((PaneKind::Terminal, target)) => bench
            .session(target)
            .and_then(|session| {
                session
                    .window_title
                    .clone()
                    .or_else(|| session.title.clone())
            })
            .map_or_else(|| "Terminal".into(), SharedString::from),
        Some((PaneKind::Plugin, target)) => {
            let id = crate::plugin_pane::parts(target).0;
            bench
                .read("plugins")
                .and_then(Value::as_array)
                .and_then(|plugins| plugins.iter().find(|plugin| plugin["id"] == id))
                .and_then(|plugin| plugin["name"].as_str())
                .map_or_else(
                    || "Plugin".into(),
                    |name| SharedString::from(name.to_owned()),
                )
        }
        Some((kind, _)) => kind_label(kind).into(),
        None => "".into(),
    }
}

pub fn first_view(pane: &Pane) -> Option<(PaneKind, &String)> {
    match pane {
        Pane::View { kind, target, .. } => Some((*kind, target)),
        Pane::Split { panes, .. } => panes.iter().find_map(first_view),
    }
}

/// A view's icon: its plugin's mark for a plugin's pane, else its kind's.
pub fn view_icon(kind: PaneKind, target: &str) -> Icon {
    match kind {
        PaneKind::Plugin => {
            crate::rail::plugin_mark(crate::plugin_pane::parts(target).0).unwrap_or(Icon::Window)
        }
        kind => kind_icon(kind),
    }
}

pub fn kind_icon(kind: PaneKind) -> Icon {
    match kind {
        PaneKind::Agent => Icon::Agent,
        PaneKind::Terminal => Icon::Terminal,
        PaneKind::Files => Icon::Editor,
        PaneKind::Git => Icon::Git,
        PaneKind::Search => Icon::Search,
        PaneKind::Board => Icon::Board,
        PaneKind::Browser => Icon::Globe,
        PaneKind::Plugin => Icon::Window,
        PaneKind::Desk => Icon::PanelRight,
        PaneKind::ProjectSettings => Icon::Settings,
        PaneKind::InstallSikemux => Icon::Download,
        PaneKind::Unknown => Icon::Window,
    }
}

fn kind_label(kind: PaneKind) -> &'static str {
    match kind {
        PaneKind::Agent => "Agent",
        PaneKind::Terminal => "Terminal",
        PaneKind::Files => "Files",
        PaneKind::Git => "Git",
        PaneKind::Search => "Search",
        PaneKind::Board => "Agent board",
        PaneKind::Browser => "Browser",
        PaneKind::Plugin => "Plugin",
        PaneKind::Desk => "Desk",
        PaneKind::ProjectSettings => "Project settings",
        PaneKind::InstallSikemux => "Install Sikemux",
        PaneKind::Unknown => "Needs a newer Sikemux",
    }
}

fn clock(cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx);
    let now = chrono::Local::now();
    div()
        .flex()
        .items_baseline()
        .font_features(crate::theme::tabular())
        .child(
            ui::text(now.format("%-I:%M").to_string(), 12., FontWeight::SEMIBOLD)
                .text_color(t.text_secondary),
        )
        .child(
            ui::text(now.format("%p").to_string(), 10., FontWeight::SEMIBOLD)
                .letter_spacing(0.06)
                .ml(px(4.))
                .text_color(t.text_tertiary),
        )
}
