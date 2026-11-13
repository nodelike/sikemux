//! The Playbook's Setup page: what a new checkout of the project runs, its ports, its heavy
//! commands and what agents use one at a time, and whether that was proved on a fresh checkout.

use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, FontWeight, Hsla, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled,
};
use sikemux_calls::call;
use sikemux_calls::setup::Outcome;
use sikemux_calls::setup::SetupParams;
use sikemux_calls::setup::SetupRun;
use sikemux_calls::setup::SetupState;
use sikemux_calls::setup::SetupStep;
use sikemux_calls::setup::StepKind;
use sikemux_corekit::tasks::config::ProjectConfig;
use sikemux_corekit::tasks::config::WorktreeHook;

use super::PlaybookTab;
use crate::icons::Icon;
use crate::send_to::Recipient;
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, TEXT_2XS_H, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// The first port a checkout's ten start from, for the example the Ports card gives.
const EXAMPLE_PORT: u16 = 21040;

impl Workbench {
    pub(super) fn reload_setup(&self, folder: &str, cx: &mut Context<Self>) {
        let asked = self
            .target()
            .request_typed::<call::SetupState>(SetupParams {
                project: folder.to_owned(),
            });
        let folder = folder.to_owned();
        cx.spawn(async move |bench, cx| {
            let Ok(setup) = asked.await else {
                return;
            };
            let _ = bench.update(cx, |bench, cx| {
                if let Some(settings) = bench.settings_of(&folder) {
                    settings.setup = Some(setup);
                }
                bench.parts.redraw_stage(cx);
            });
        })
        .detach();
    }

    /// Checks the project's setup on fresh checkouts again, showing the page as checking.
    fn check_setup(&mut self, folder: &str, cx: &mut Context<Self>) {
        if let Some(setup) = self
            .settings_of(folder)
            .and_then(|settings| settings.setup.as_mut())
        {
            setup.checking = true;
        }
        self.parts.redraw_stage(cx);
        let asked = self
            .target()
            .request_typed::<call::SetupCheck>(SetupParams {
                project: folder.to_owned(),
            });
        let folder = folder.to_owned();
        cx.spawn(async move |bench, cx| {
            let ran = asked.await;
            let _ = bench.update(cx, |bench, cx| {
                if let Err(error) = &ran {
                    bench.notify(
                        crate::toast::error(t!(
                            "The setup was not checked: {error}",
                            error = error
                        )),
                        cx,
                    );
                }
                bench.reload_setup(&folder, cx);
            });
        })
        .detach();
    }

    /// Hands `prompt` to a new chat in the project, with the agent the person starts first.
    fn setup_agent(
        &mut self,
        prompt: String,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        let to = crate::send_to::choices(self)
            .into_iter()
            .map(|choice| choice.to)
            .find(|to| matches!(to, Recipient::NewChat { .. }));
        if let Some(to) = to {
            crate::send_to::send(self, to, async move { Some(prompt) }, window, cx);
        }
    }
}

pub(super) fn page(
    settings: &PlaybookTab,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(setup) = &settings.setup else {
        return vec![rows::card(
            false,
            rows::note(t!("Reading the project's setup…"), cx),
            cx,
        )];
    };
    let Some(last) = &setup.last else {
        return not_set_up(setup, folder, this, cx);
    };
    let config = setup.config.as_ref();
    let worktree = config.and_then(|config| config.worktree.as_ref());
    let created = worktree.map_or(&[][..], |worktree| &worktree.on_create[..]);
    let removed = worktree.map_or(&[][..], |worktree| &worktree.on_remove[..]);
    let ran: Vec<&SetupStep> = (last.steps.iter())
        .filter(|step| step.kind == StepKind::Create)
        .collect();
    let failed = last
        .steps
        .iter()
        .find(|step| step.outcome == Outcome::Failed);
    let mut cards = vec![steps_card(created, &ran, !setup.stale, cx)];
    let status = status_card(setup, last, failed, folder, this, cx);
    if failed.is_some() && !setup.stale {
        cards.push(status);
        cards.extend(removal_card(removed, cx));
        cards.extend(config.and_then(|config| ports_card(config, cx)));
        cards.extend(config.and_then(|config| heavy_card(config, cx)));
        cards.extend(config.and_then(|config| one_at_a_time_card(config, cx)));
    } else {
        cards.extend(removal_card(removed, cx));
        cards.extend(config.and_then(|config| ports_card(config, cx)));
        cards.extend(config.and_then(|config| heavy_card(config, cx)));
        cards.extend(config.and_then(|config| one_at_a_time_card(config, cx)));
        cards.push(status);
    }
    cards
}

fn not_set_up(
    setup: &SetupState,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let served = (setup.config.iter())
        .flat_map(|config| config.tasks.iter())
        .find_map(|task| Some((task.label.clone(), task.port?)));
    let why = match served {
        Some((label, port)) => t!(
            "Agents share this project's ports and builds until it is set up: two checkouts both start {label} on {port}, and their builds run at once.",
            label = label,
            port = port
        ),
        None => t!("Agents share this project's ports and builds until it is set up: two checkouts start the same servers on the same ports, and their builds run at once.").to_owned(),
    };
    let (starting, folder) = (this.clone(), folder.to_owned());
    let set_up = Button::new("setup-start", t!("Set up"), Variant::Primary)
        .icon(Icon::Sparkle)
        .on_click(move |_, window, cx| {
            let prompt =
                format!("Load the project-setup skill and set up the project in {folder}.");
            starting.update(cx, |bench, cx| bench.setup_agent(prompt, window, cx))
        });
    vec![
        rows::section_with(
            t!("Setup"),
            None,
            Some(set_up.into_any_element()),
            Some(why.into()),
            false,
            div(),
            cx,
        ),
        rows::section(
            t!("What setting up does"),
            None,
            Some(t!("An agent reads the project, writes what a new checkout runs, its ports and its heavy commands into sikemux.json, then proves it on a fresh checkout. Sikemux asks you once before it runs the new steps.").into()),
            false,
            div(),
            cx,
        ),
    ]
}

/// A row of a card: a number, a command, and what sits at its end.
fn step_row(
    at: usize,
    command: &str,
    ink: Hsla,
    end: Option<AnyElement>,
    cx: &App,
) -> gpui_kit::Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .px(SPACE_3H)
        .py(SPACE_2H)
        .border_b_1()
        .border_color(t.line_soft)
        .child(
            ui::text((at + 1).to_string(), TEXT_XS, FontWeight::NORMAL)
                .w(px(14.))
                .text_color(t.text_tertiary),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(theme::MONO)
                .text_size(theme::ui_text(TEXT_XS_H))
                .text_color(ink)
                .child(command.to_owned()),
        )
        .children(end)
}

fn steps_card(
    hooks: &[WorktreeHook],
    ran: &[&SetupStep],
    current: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let rows = hooks.iter().enumerate().map(|(at, hook)| {
        let outcome = (ran.get(at))
            .filter(|step| current && step.command == hook.command)
            .map(|step| step.outcome);
        let (ink, end) = match outcome {
            Some(Outcome::Passed) => (
                t.text_primary,
                Some(
                    ui::icon(Icon::Check, 14.)
                        .text_color(t.live)
                        .into_any_element(),
                ),
            ),
            Some(Outcome::Failed) => (
                t.danger,
                Some(
                    ui::icon(Icon::Close, 14.)
                        .text_color(t.danger)
                        .into_any_element(),
                ),
            ),
            Some(Outcome::NotRun) => (
                t.text_tertiary,
                Some(ui::meta(t!("not run"), cx).into_any_element()),
            ),
            None => (t.text_primary, None),
        };
        let row = step_row(at, &hook.command, ink, end, cx)
            .when(outcome == Some(Outcome::Failed), |row| {
                row.bg(t.danger.opacity(0.08))
            });
        let failure = (outcome == Some(Outcome::Failed))
            .then(|| ran.get(at).map(|step| failure_line(step, cx)))
            .flatten();
        div().child(row).children(failure).into_any_element()
    });
    let count = match hooks.len() {
        1 => t!("1 step").to_owned(),
        count => t!("{count} steps", count = count),
    };
    let body = if hooks.is_empty() {
        rows::note(t!("A new checkout runs nothing first."), cx)
    } else {
        div().children(rows).into_any_element()
    };
    rows::section(
        t!("New checkouts"),
        Some(count.into()),
        Some(t!("The steps run in a new checkout, in this order.").into()),
        false,
        body,
        cx,
    )
}

/// Under a failed step: its last line and the way to its log.
fn failure_line(step: &SetupStep, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let log = step.log.clone();
    div()
        .px(SPACE_3H)
        .pb(SPACE_2H)
        .pl(px(46.))
        .bg(t.danger.opacity(0.08))
        .border_b_1()
        .border_color(t.line_soft)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .px(SPACE_2H)
                .py(SPACE_1H)
                .rounded(theme::RADIUS_2)
                .border_1()
                .border_color(t.danger.opacity(0.3))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO)
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.danger)
                        .child(step.last_line.clone().unwrap_or_default()),
                )
                .children(log.map(|log| {
                    ui::sign_in::link("setup-open-log", t!("Open log"), cx)
                        .underline()
                        .on_click(move |_, _, cx| cx.open_with_system(&log))
                })),
        )
        .into_any_element()
}

fn removal_card(hooks: &[WorktreeHook], cx: &mut Context<Workbench>) -> Option<AnyElement> {
    if hooks.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let rows = hooks
        .iter()
        .enumerate()
        .map(|(at, hook)| step_row(at, &hook.command, t.text_primary, None, cx));
    Some(rows::section(
        t!("When a checkout is removed"),
        None,
        Some(t!("The steps run before its folder goes.").into()),
        false,
        div().children(rows),
        cx,
    ))
}

fn ports_card(config: &ProjectConfig, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let names = config.port_names();
    if names.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let column = |words: &str, width: Option<f32>| {
        ui::caps(words, cx).when_some(width, |label, width| label.w(px(width)).flex_none())
    };
    let head = div()
        .flex()
        .items_center()
        .px(SPACE_3H)
        .py(SPACE_2)
        .border_b_1()
        .border_color(t.line_soft)
        .child(column(t!("Name"), Some(118.)))
        .child(column(t!("This folder"), Some(170.)))
        .child(column(t!("In a checkout"), None));
    let rows = names.iter().enumerate().map(|(at, (name, port))| {
        div()
            .flex()
            .items_center()
            .px(SPACE_3H)
            .py(SPACE_2H)
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                ui::text(name.to_string(), TEXT_XS_H, FontWeight::NORMAL)
                    .w(px(118.))
                    .flex_none()
                    .truncate()
                    .text_color(t.text_primary),
            )
            .child(
                ui::mono(port.to_string(), TEXT_XS_H, cx)
                    .w(px(170.))
                    .flex_none()
                    .text_color(t.text_primary),
            )
            .child(
                div()
                    .flex()
                    .gap(SPACE_1H)
                    .child(
                        ui::text(t!("one of its ten, such as"), TEXT_XS, FontWeight::NORMAL)
                            .text_color(t.text_tertiary),
                    )
                    .child(ui::mono(
                        (EXAMPLE_PORT + at as u16).to_string(),
                        TEXT_XS,
                        cx,
                    )),
            )
    });
    Some(rows::section(
        t!("Ports"),
        None,
        Some(t!("This folder keeps the usual ports; every checkout gets ten of its own, and its env values follow them.").into()),
        false,
        div().child(head).children(rows),
        cx,
    ))
}

fn heavy_card(config: &ProjectConfig, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let heavy = config.heavy.as_ref().filter(|heavy| !heavy.is_empty())?;
    let t = Tokens::get(cx).clone();
    let rows = heavy.iter().map(|command| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .px(SPACE_3H)
            .py(SPACE_2H)
            .border_b_1()
            .border_color(t.line_soft)
            .child(ui::mono(command.command.clone(), TEXT_XS_H, cx).text_color(t.text_primary))
            .when(command.before_commit, |row| {
                row.child(
                    div()
                        .px(SPACE_1H)
                        .rounded(theme::RADIUS_1)
                        .bg(t.accent.opacity(0.14))
                        .text_size(theme::ui_text(TEXT_2XS_H))
                        .text_color(t.accent)
                        .child(t!("before commit")),
                )
                .child(div().flex_1())
                .child(ui::meta(t!("goes ahead of the rest"), cx))
            })
            .when(command.light, |row| {
                row.child(div().flex_1())
                    .child(ui::meta(t!("never waits"), cx))
            })
    });
    Some(rows::section(
        t!("Heavy commands"),
        None,
        Some(t!("They take turns with every agent's on this host. A check before a commit goes first.").into()),
        false,
        div().children(rows),
        cx,
    ))
}

fn one_at_a_time_card(config: &ProjectConfig, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    if config.resources.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let rows = config.resources.iter().map(|resource| {
        let needed = |needs: &[String]| needs.contains(&resource.name);
        let held = match config.tasks.iter().any(|task| needed(&task.needs))
            || (config.heavy.iter().flatten()).any(|command| needed(&command.needs))
        {
            true => t!("held while it runs"),
            false => t!("held for an agent's turn"),
        };
        let held = match resource.count {
            0 | 1 => held.to_owned(),
            count => format!("{} · {held}", t!("{count} at once", count = count)),
        };
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .px(SPACE_3H)
            .py(SPACE_2H)
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                ui::icon(
                    match resource.port {
                        Some(_) => Icon::Globe,
                        None => Icon::Lock,
                    },
                    14.,
                )
                .text_color(t.text_tertiary),
            )
            .child(
                ui::text(resource.label.clone(), TEXT_XS_H, FontWeight::NORMAL)
                    .text_color(t.text_primary),
            )
            .child(div().flex_1())
            .child(ui::meta(held, cx))
    });
    Some(rows::section(
        t!("One at a time"),
        None,
        Some(
            t!("Only one agent holds it; the rest wait in line and are woken on their turn.")
                .into(),
        ),
        false,
        div().children(rows),
        cx,
    ))
}

/// When the setup was last checked and how it went, with Check again.
fn status_card(
    setup: &SetupState,
    last: &SetupRun,
    failed: Option<&SetupStep>,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let ago = ui::time::ago(last.at);
    let checked = last
        .steps
        .iter()
        .filter(|step| step.kind == StepKind::Create)
        .count();
    let (icon, ink, words, rest): (Icon, Hsla, String, Option<String>) = if setup.checking {
        (
            Icon::Check,
            t.text_tertiary,
            t!("Checking on two fresh checkouts…").to_owned(),
            None,
        )
    } else if setup.stale {
        (
            Icon::Check,
            t.text_tertiary,
            t!("sikemux.json changed since it was checked on a fresh checkout").to_owned(),
            Some(ago),
        )
    } else if let Some(failed) = failed {
        let at = numbered(last, failed);
        (
            Icon::Close,
            t.danger,
            match at {
                Some(at) => t!("Failed on a fresh checkout at step {at}", at = at),
                None => t!(
                    "The check on a fresh checkout failed: {step}",
                    step = failed.label.clone()
                ),
            },
            Some(ago),
        )
    } else {
        (
            Icon::Check,
            t.text_primary,
            t!("Checked on a fresh checkout {ago}", ago = ago),
            Some(match checked {
                1 => t!("the step passed").to_owned(),
                count => t!("all {count} steps passed", count = count),
            }),
        )
    };
    let again = {
        let (checking, folder) = (this.clone(), folder.to_owned());
        Button::new("setup-check-again", t!("Check again"), Variant::Outline)
            .disabled(setup.checking)
            .on_click(move |_, _, cx| {
                checking.update(cx, |bench, cx| bench.check_setup(&folder, cx))
            })
    };
    let fix = failed.filter(|_| !setup.stale && !setup.checking).map(|failed| {
        let prompt = format!(
            "Load the project-setup skill. setup_check failed on the project in {folder} at `{}`: {}. Its log is {}. Fix sikemux.json or what it runs, and prove it again.",
            failed.command,
            failed.last_line.clone().unwrap_or_default(),
            failed.log.clone().unwrap_or_else(PathBuf::new).display(),
        );
        let fixing = this.clone();
        Button::new("setup-fix", t!("Fix with an agent"), Variant::Primary)
            .icon(Icon::Sparkle)
            .on_click(move |_, window, cx| {
                let prompt = prompt.clone();
                fixing.update(cx, |bench, cx| bench.setup_agent(prompt, window, cx))
            })
    });
    let mark = match icon {
        Icon::Close => t.danger,
        _ if failed.is_none() && !setup.stale && !setup.checking => t.live,
        _ => t.text_tertiary,
    };
    rows::card(
        false,
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .px(SPACE_3H)
            .py(SPACE_3)
            .child(ui::icon(icon, 14.).flex_none().text_color(mark))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .gap(SPACE_1H)
                    .child(
                        ui::text(words, TEXT_XS_H, FontWeight::NORMAL)
                            .min_w_0()
                            .truncate()
                            .text_color(ink),
                    )
                    .children(rest.map(|rest| {
                        ui::text(format!("· {rest}"), TEXT_XS_H, FontWeight::NORMAL)
                            .flex_none()
                            .text_color(t.text_tertiary)
                    })),
            )
            .child(div().flex_none().child(again))
            .children(fix.map(|fix| div().flex_none().child(fix))),
        cx,
    )
}

/// The failed step's number among the new checkout's steps, when it is one of them.
fn numbered(last: &SetupRun, failed: &SetupStep) -> Option<usize> {
    (last.steps.iter())
        .filter(|step| step.kind == StepKind::Create)
        .position(|step| std::ptr::eq(step, failed))
        .map(|at| at + 1)
}
