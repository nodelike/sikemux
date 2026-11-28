//! Every agent Sikemux can run, one row each: the release it runs, whether a newer one is out,
//! which hosts have it, who it is signed in as, and the one thing it needs next. A row opens
//! to its hosts, its accounts and the program it runs.

use crate::ui::Clicks;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Radians, SharedString, StatefulInteractiveElement, Styled, Transformation,
    Window,
};
use sikemux_agents::agent_accounts::status::AgentStatus;
use sikemux_agents::custom_agents::CustomAgentConfig;
use sikemux_calls::agent_updates::Updating;
use sikemux_calls::call;
use sikemux_calls::custom_agents::RemoveParams;
use sikemux_view::catalog::CatalogAgent;
use std::collections::BTreeMap;
use std::f32::consts::FRAC_PI_2;

use crate::agents::setup::{installing, run, signing_in, update_everywhere, Found, Sheet};
use crate::core::Target;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS, TEXT_2XS_H, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[crate::settings::index::Entry] =
    &[crate::settings::index::section(
        crate::settings::index::Page::Agents,
        "Agents",
        "install reinstall update upgrade version latest missing hosts claude codex opencode hermes pi omp grok antigravity agy gemini google cli executable path program profile custom acp add remove",
    )];

/// The row that is open, the program being written for the account it runs on, and the form
/// that adds an agent of the person's own.
pub struct Fields {
    open: Option<String>,
    editing: Option<String>,
    program: Entity<InputState>,
    /// The form for a custom agent is showing.
    adding: bool,
    /// An add or a remove is on its way to the core.
    busy: bool,
    /// Why the form was refused.
    failure: Option<String>,
    /// Why a custom agent could not be removed, with its id.
    removal_failure: Option<(String, String)>,
    label: Entity<InputState>,
    id: Entity<InputState>,
    command: Entity<InputState>,
    args: Entity<InputState>,
    env: Entity<InputState>,
}

impl Fields {
    pub fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let program =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("Leave empty to use PATH")));
        let label = cx.new(|cx| InputState::new(window, cx).placeholder(t!("DimCode")));
        let id = cx.new(|cx| InputState::new(window, cx).placeholder(t!("From the name if empty")));
        let command = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("dima or /Users/you/.local/bin/dima"))
        });
        let args = cx.new(|cx| InputState::new(window, cx).placeholder(t!("acp")));
        let env = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("KEY=VALUE, separated by commas or lines"))
        });
        Self {
            open: None,
            editing: None,
            program,
            adding: false,
            busy: false,
            failure: None,
            removal_failure: None,
            label,
            id,
            command,
            args,
            env,
        }
    }
}

/// One agent, as each host that has it found it.
struct Agent<'a> {
    kind: &'a str,
    label: &'a str,
    here: Option<&'a Found>,
    hosts: Vec<(String, &'a Found)>,
}

impl Agent<'_> {
    /// Every host where it is installed, this one first, with what each found.
    fn installed(&self) -> impl Iterator<Item = &Found> {
        (self.here.into_iter())
            .chain(self.hosts.iter().map(|(_, found)| *found))
            .filter(|found| !matches!(found.status, AgentStatus::Missing))
    }
}

fn agents(bench: &Workbench) -> Vec<Agent<'_>> {
    let setup = &bench.agent_setup;
    let mut agents: Vec<Agent> = (setup.found.iter().flatten())
        .map(|found| Agent {
            kind: &found.kind,
            label: &found.label,
            here: Some(found),
            hosts: Vec::new(),
        })
        .collect();
    for host in &setup.elsewhere {
        for found in &host.found {
            let name = bench.host_name(&host.host).to_string();
            match agents.iter_mut().find(|agent| agent.kind == found.kind) {
                Some(agent) => agent.hosts.push((name, found)),
                None => agents.push(Agent {
                    kind: &found.kind,
                    label: &found.label,
                    here: None,
                    hosts: vec![(name, found)],
                }),
            }
        }
    }
    agents
}

pub fn render(
    bench: &Workbench,
    fields: &Fields,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let all = agents(bench);
    let behind = all
        .iter()
        .filter(|agent| agent.installed().any(|found| found.update_to().is_some()))
        .count();
    let update_all = (behind > 0).then(|| {
        let this = this.clone();
        ui::Button::new(
            "agent-update-all",
            t!("Update {count}", count = behind),
            ui::Variant::Outline,
        )
        .on_click(move |_, _, cx| this.update(cx, update_everywhere))
        .into_any_element()
    });
    let add_custom = {
        let this = this.clone();
        ui::Button::new(
            "agent-add-custom",
            t!("Add custom agent"),
            ui::Variant::Outline,
        )
        .on_click(move |_, window, cx| this.update(cx, |bench, cx| toggle_form(bench, window, cx)))
        .into_any_element()
    };
    let actions = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .children(update_all)
        .child(add_custom)
        .into_any_element();
    let body: AnyElement = match bench.agent_setup.found {
        None => rows::note(t!("Looking for agents…"), cx),
        Some(_) => div()
            .children(fields.adding.then(|| form(fields, this, cx)))
            .children(all.iter().map(|agent| {
                let open = fields.open.as_deref() == Some(agent.kind);
                div()
                    .child(row(bench, agent, open, this, cx))
                    .when(open, |shown| {
                        shown.child(detail(bench, agent, fields, this, cx))
                    })
            }))
            .into_any_element(),
    };
    rows::section_with(
        n!("Agents"),
        None,
        Some(actions),
        None,
        lit("Agents"),
        body,
        cx,
    )
}

/// The line under an agent's name, when there is more to say than its release.
fn trouble(agent: &Agent) -> Option<(SharedString, bool)> {
    for found in agent
        .here
        .into_iter()
        .chain(agent.hosts.iter().map(|(_, found)| *found))
    {
        match &found.updating {
            Some(Updating::Queued) => {
                return Some((t!("Waiting for another update").into(), false))
            }
            Some(Updating::Running { .. }) => return Some((t!("Updating…").into(), false)),
            Some(Updating::Failed { .. }) => return Some((t!("Its update failed").into(), true)),
            None => {}
        }
    }
    (agent
        .installed()
        .any(|found| matches!(found.status, AgentStatus::Broken { .. })))
    .then(|| {
        (
            t!("It doesn't start. Open it for what it said.").into(),
            true,
        )
    })
}

fn row(
    bench: &Workbench,
    agent: &Agent,
    open: bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let shown = agent
        .here
        .filter(|found| found.version.is_some())
        .or_else(|| agent.installed().find(|found| found.version.is_some()));
    let newest = agent.installed().find_map(Found::update_to);
    let places: Vec<SharedString> = (agent.here.iter())
        .filter(|found| !matches!(found.status, AgentStatus::Missing))
        .map(|_| SharedString::from(t!("This host")))
        .chain(
            (agent.hosts.iter())
                .filter(|(_, found)| !matches!(found.status, AgentStatus::Missing))
                .map(|(host, _)| SharedString::from(host.clone())),
        )
        .collect();
    let where_: Option<SharedString> = match places.len() {
        0 => None,
        1 => places.into_iter().next(),
        count => Some(t!("{count} hosts", count = count).into()),
    };
    let signed_in: Vec<_> = (bench.accounts.of(agent.kind).into_iter())
        .filter(|profile| {
            crate::accounts::status(bench, profile)
                .and_then(|status| status.as_ref().ok())
                .is_some_and(|status| status.signed_in)
        })
        .take(3)
        .collect();
    let faces = div()
        .flex()
        .children(signed_in.iter().enumerate().map(|(at, profile)| {
            div()
                .when(at > 0, |face| face.ml(px(-5.)))
                .rounded_full()
                .border_1()
                .border_color(t.surface)
                .child(crate::accounts::avatar(bench.accounts.avatar(profile), 16.))
        }));
    let custom = bench
        .reads
        .catalog
        .kind(agent.kind)
        .is_some_and(|entry| entry.custom);
    let label = div()
        .flex()
        .items_baseline()
        .gap(SPACE_2)
        .child(agent.label.to_owned())
        .children(custom.then(|| {
            div()
                .px(SPACE_1H)
                .rounded(px(4.))
                .border_1()
                .border_color(t.line_soft)
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child(t!("Custom"))
        }))
        .children(
            shown
                .and_then(|found| found.version.clone())
                .map(|version| {
                    div()
                        .text_size(theme::ui_text(TEXT_2XS_H))
                        .text_color(t.text_tertiary)
                        .child(version)
                }),
        )
        .children(newest.map(|newest| {
            div()
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.accent)
                .child(format!("↑ {newest}"))
        }));
    let about = trouble(agent)
        .map(|(line, bad)| rows::about_line(line, if bad { t.danger } else { t.text_tertiary }));
    let control = div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .children(where_.map(|where_| {
            div()
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child(where_)
        }))
        .child(faces)
        .child(
            div()
                .min_w(px(78.))
                .flex()
                .justify_end()
                .child(state(bench, agent, this, cx)),
        )
        .child(
            ui::icon(Icon::Chevron, 10.)
                .text_color(t.text_tertiary)
                .when(open, |chevron| {
                    chevron.with_transformation(Transformation::rotate(Radians(FRAC_PI_2)))
                }),
        );
    let kind = agent.kind.to_owned();
    let toggle = this.clone();
    let missing = agent.installed().next().is_none();
    div()
        .id(SharedString::from(format!("agent-row-{kind}")))
        .aria_label(SharedString::from(agent.label.to_owned()))
        .test_support()
        .cursor_pointer()
        .hover(|row| row.bg(t.surface_hover))
        .when(missing, |row| row.opacity(0.7))
        .child(rows::row_parts(
            Some(ui::mark(agent.kind, 16., cx)),
            label,
            about,
            control,
            cx,
        ))
        .clicks(move |_, _, cx| {
            let kind = kind.clone();
            toggle.update(cx, |bench, cx| {
                super::change_fields(bench, cx, |fields, _| {
                    let fields = &mut fields.setup;
                    fields.editing = None;
                    fields.open = (fields.open.as_deref() != Some(kind.as_str())).then_some(kind);
                });
            })
        })
        .into_any_element()
}

/// The one word or button that says what the agent needs: nothing, a sign-in, an install, or a
/// reinstall because it doesn't start.
fn state(bench: &Workbench, agent: &Agent, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let sheet_button = |id: String, label: &'static str, sheet: Sheet| {
        let this = this.clone();
        ui::Button::new(SharedString::from(id), label, ui::Variant::Outline)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let sheet = sheet.clone();
                this.update(cx, |bench, cx| run(bench, sheet, cx))
            })
            .into_any_element()
    };
    let worded = |tone: ui::Tone, word: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(t.text_secondary)
            .child(ui::dot(tone, cx))
            .child(word)
            .into_any_element()
    };
    if agent.installed().any(|found| found.updating_now()) {
        return ui::sign_in::spinner("agent-updating", cx);
    }
    let custom = (bench.reads.catalog.kind(agent.kind)).is_some_and(|entry| entry.custom);
    match agent.here.map(|found| (found, &found.status)) {
        Some((_, AgentStatus::Missing)) if custom && agent.installed().next().is_none() => {
            worded(ui::Tone::Failed, t!("Command not found"))
        }
        Some((found, AgentStatus::Broken { .. })) => match installing(found) {
            Some(sheet) => sheet_button(
                format!("agent-install-{}", agent.kind),
                t!("Reinstall"),
                sheet,
            ),
            None => worded(ui::Tone::Failed, t!("Not working")),
        },
        Some((found, AgentStatus::SignedOut)) => {
            let folder = (bench.accounts.current(agent.kind))
                .and_then(|profile| profile.config_path.clone());
            match signing_in(&bench.reads.catalog, found, folder.as_deref()) {
                Some(sheet) => sheet_button(
                    format!("agent-sign-in-{}", agent.kind),
                    t!("Sign in"),
                    sheet,
                ),
                None => worded(ui::Tone::Pending, t!("Signed out")),
            }
        }
        Some((found, AgentStatus::Missing)) if agent.installed().next().is_none() => {
            match installing(found) {
                Some(sheet) => sheet_button(
                    format!("agent-install-{}", agent.kind),
                    t!("Install"),
                    sheet,
                ),
                None => worded(ui::Tone::Idle, t!("Not installed")),
            }
        }
        _ if agent
            .installed()
            .any(|found| matches!(found.status, AgentStatus::Broken { .. })) =>
        {
            worded(ui::Tone::Failed, t!("Not working"))
        }
        _ => worded(ui::Tone::Ok, t!("Ready")),
    }
}

/// An opened row: each host with its release, the agent's accounts, and the program it runs.
fn detail(
    bench: &Workbench,
    agent: &Agent,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let part = |name: &'static str, body: AnyElement| {
        div()
            .flex()
            .items_start()
            .gap(SPACE_3)
            .py(SPACE_2)
            .child(
                div()
                    .w(px(72.))
                    .flex_none()
                    .pt(px(2.))
                    .text_size(theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child(name),
            )
            .child(div().flex_1().min_w_0().child(body))
    };
    let meta = |text: SharedString| {
        div()
            .text_size(theme::ui_text(TEXT_2XS_H))
            .text_color(t.text_tertiary)
            .child(text)
    };
    let place = |name: SharedString, found: &Found| {
        let tone = match found.status {
            AgentStatus::Ready { .. } | AgentStatus::Unknown => None,
            AgentStatus::SignedOut => Some((ui::Tone::Pending, t!("signed out"))),
            AgentStatus::Missing => Some((ui::Tone::Idle, t!("not installed"))),
            AgentStatus::Broken { .. } => Some((ui::Tone::Failed, t!("doesn't start"))),
        };
        div()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(t.text_primary)
            .children(tone.map(|(tone, _)| ui::dot(tone, cx)))
            .child(name)
            .children(found.version.clone().map(|version| meta(version.into())))
            .children(tone.map(|(_, word)| meta(word.into())))
    };
    let places = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(SPACE_4)
        .gap_y(SPACE_1)
        .children(agent.here.map(|found| place(t!("This host").into(), found)))
        .children((agent.hosts.iter()).map(|(host, found)| place(host.clone().into(), found)));
    let reasons: Vec<String> = (agent.installed())
        .filter_map(|found| match (&found.status, &found.updating) {
            (AgentStatus::Broken { reason }, _) => Some(reason.clone()),
            (_, Some(Updating::Failed { reason })) => Some(reason.clone()),
            _ => None,
        })
        .collect();
    let hosts = div()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(places)
        .children(reasons.into_iter().map(|reason| {
            div()
                .font_family(theme::mono_font())
                .text_size(theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(reason)
        }));
    let signs_in =
        (bench.reads.catalog.agents()).any(|kind| kind.kind == agent.kind && kind.accounts);
    let accounts = signs_in.then(|| super::accounts::list(bench, agent.kind, this, cx));
    let custom = (bench.reads.catalog.kind(agent.kind)).filter(|entry| entry.custom);
    div()
        .pl(px(48.))
        .pr(SPACE_4)
        .pb(SPACE_2)
        .border_b(theme::border())
        .border_color(t.line_soft)
        .child(part(t!("Hosts"), hosts.into_any_element()))
        .children(accounts.map(|accounts| part(t!("Accounts"), accounts)))
        .child(part(t!("Runs"), runs(bench, agent, fields, this, cx)))
        .children(
            custom.map(|own| part(t!("Custom"), custom_command(agent, own, fields, this, cx))),
        )
        .into_any_element()
}

/// The program the agent's account in use starts, with the way to point it at another.
fn runs(
    bench: &Workbench,
    agent: &Agent,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(profile) = bench.accounts.current(agent.kind).cloned() else {
        return div()
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(t.text_secondary)
            .child(t!("{program} from PATH", program = agent.kind))
            .into_any_element();
    };
    if fields.editing.as_deref() == Some(profile.id.as_str()) {
        let (save, cancel) = (this.clone(), this.clone());
        let input = fields.program.clone();
        return div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(ui::Field::new(&fields.program)),
            )
            .child(
                ui::Button::new("agent-program-save", t!("Save"), ui::Variant::Primary).on_click(
                    move |_, _, cx| {
                        let path = input.read(cx).value().trim().to_owned();
                        let mut profile = profile.clone();
                        profile.executable_path = (!path.is_empty()).then_some(path);
                        save.update(cx, |bench, cx| {
                            crate::accounts::save(bench, &profile, cx);
                            super::change_fields(bench, cx, |fields, _| {
                                fields.setup.editing = None
                            });
                        })
                    },
                ),
            )
            .child(
                ui::Button::new("agent-program-cancel", t!("Cancel"), ui::Variant::Ghost).on_click(
                    move |_, _, cx| {
                        cancel.update(cx, |bench, cx| {
                            super::change_fields(bench, cx, |fields, _| {
                                fields.setup.editing = None
                            });
                        })
                    },
                ),
            )
            .into_any_element();
    }
    let shown: SharedString = match &profile.executable_path {
        Some(path) => path.clone().into(),
        None => t!("{program} from PATH", program = agent.kind).into(),
    };
    let change = {
        let this = this.clone();
        ui::Button::new(
            SharedString::from(format!("agent-program-{}", agent.kind)),
            t!("Change"),
            ui::Variant::Ghost,
        )
        .on_click(move |_, window, cx| {
            let profile = profile.clone();
            this.update(cx, |bench, cx| {
                super::change_fields(bench, cx, |fields, cx| {
                    let value = profile.executable_path.clone().unwrap_or_default();
                    fields
                        .setup
                        .program
                        .update(cx, |input, cx| input.set_value(value, window, cx));
                    fields.setup.editing = Some(profile.id.clone());
                });
            })
        })
    };
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(shown),
        )
        .child(change)
        .into_any_element()
}

/// What a custom agent starts, and the way to take it away again.
fn custom_command(
    agent: &Agent,
    own: &CatalogAgent,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let shown: SharedString = std::iter::once(own.command.as_str())
        .chain(own.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .into();
    let missing = agent.installed().next().is_none();
    let failure = (fields.removal_failure.as_ref())
        .filter(|(kind, _)| kind == agent.kind)
        .map(|(_, message)| message.clone());
    let kind = agent.kind.to_owned();
    let this = this.clone();
    let remove = ui::Button::new(
        SharedString::from(format!("agent-remove-{kind}")),
        t!("Remove"),
        ui::Variant::Outline,
    )
    .disabled(fields.busy)
    .on_click(move |_, _, cx| {
        let kind = kind.clone();
        this.update(cx, |bench, cx| remove_custom(bench, kind, cx))
    });
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::mono_font())
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(shown),
                )
                .child(remove),
        )
        .children(missing.then(|| {
            div()
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.danger)
                .child(t!(
                    "Its command was not found on this host. Check the path or remove it."
                ))
        }))
        .children(failure.map(|message| {
            div()
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.danger)
                .child(message)
        }))
        .into_any_element()
}

/// Shows or hides the form for a custom agent, starting it empty.
fn toggle_form(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    super::change_fields(bench, cx, |fields, cx| {
        let fields = &mut fields.setup;
        fields.adding = !fields.adding;
        fields.failure = None;
        if !fields.adding {
            return;
        }
        for (input, value) in [
            (&fields.label, ""),
            (&fields.id, ""),
            (&fields.command, ""),
            (&fields.args, "acp"),
            (&fields.env, ""),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        fields.label.update(cx, |input, cx| input.focus(window, cx));
    });
}

/// The labelled boxes that describe a custom agent, with what went wrong and the buttons.
fn form(fields: &Fields, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let boxed = |name: &'static str, state: &Entity<InputState>, mono: bool| {
        let field = ui::Field::new(state);
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(
                div()
                    .text_size(theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child(name),
            )
            .child(if mono { field.mono() } else { field })
    };
    let (save, cancel) = (this.clone(), this.clone());
    div()
        .px(SPACE_4)
        .py(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .border_b(theme::border())
        .border_color(t.line_soft)
        .child(
            div()
                .flex()
                .gap(SPACE_3)
                .child(
                    div()
                        .flex_1()
                        .child(boxed(t!("Name"), &fields.label, false)),
                )
                .child(div().flex_1().child(boxed(t!("ID"), &fields.id, true))),
        )
        .child(boxed(t!("Command"), &fields.command, true))
        .child(boxed(t!("Arguments"), &fields.args, true))
        .child(boxed(t!("Environment"), &fields.env, true))
        .children(fields.failure.clone().map(|message| {
            div()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.danger)
                .child(message)
        }))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(SPACE_2)
                .child(
                    ui::Button::new("agent-custom-cancel", t!("Cancel"), ui::Variant::Ghost)
                        .on_click(move |_, window, cx| {
                            cancel.update(cx, |bench, cx| {
                                super::change_fields(bench, cx, |fields, _| {
                                    fields.setup.adding = false;
                                    fields.setup.failure = None;
                                });
                                window.refresh();
                            })
                        }),
                )
                .child(
                    ui::Button::new(
                        "agent-custom-add",
                        if fields.busy {
                            t!("Adding…")
                        } else {
                            t!("Add")
                        },
                        ui::Variant::Primary,
                    )
                    .disabled(fields.busy)
                    .on_click(move |_, _, cx| save.update(cx, add_custom)),
                ),
        )
        .into_any_element()
}

/// Sends the form to the core, which keeps the agent and tells every client.
fn add_custom(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(fields) = super::fields(bench, cx).map(|fields| &fields.setup) else {
        return;
    };
    if fields.busy {
        return;
    }
    let typed = |input: &Entity<InputState>| input.read(cx).value().to_string();
    let built = config_of(
        &typed(&fields.label),
        &typed(&fields.id),
        &typed(&fields.command),
        &typed(&fields.args),
        &typed(&fields.env),
    );
    let config = match built {
        Ok(config) => config,
        Err(message) => {
            super::change_fields(bench, cx, |fields, _| fields.setup.failure = Some(message));
            return;
        }
    };
    super::change_fields(bench, cx, |fields, _| {
        fields.setup.busy = true;
        fields.setup.failure = None;
    });
    let asked = Target::here(&bench.asks).request_typed::<call::CustomAgentAdd>(config);
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| {
            super::change_fields(bench, cx, |fields, _| {
                let fields = &mut fields.setup;
                fields.busy = false;
                match &answer {
                    Ok(added) => {
                        fields.adding = false;
                        fields.failure = None;
                        fields.open = Some(added.r#type.clone());
                    }
                    Err(error) => fields.failure = Some(error.to_string()),
                }
            });
            if answer.is_ok() {
                crate::agents::setup::read(bench, cx);
            }
        });
    })
    .detach();
}

/// Takes a custom agent away, closes its row and reads the agents again.
fn remove_custom(bench: &mut Workbench, kind: String, cx: &mut Context<Workbench>) {
    super::change_fields(bench, cx, |fields, _| {
        fields.setup.busy = true;
        fields.setup.removal_failure = None;
    });
    let asked = Target::here(&bench.asks)
        .request_typed::<call::CustomAgentRemove>(RemoveParams { kind: kind.clone() });
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| {
            super::change_fields(bench, cx, |fields, _| {
                let fields = &mut fields.setup;
                fields.busy = false;
                match &answer {
                    Ok(_) => {
                        fields.open = None;
                        fields.editing = None;
                    }
                    Err(error) => fields.removal_failure = Some((kind.clone(), error.to_string())),
                }
            });
            crate::agents::setup::read(bench, cx);
        });
    })
    .detach();
}

/// What the form says, as the agent the core adds: an empty id is made from the name.
fn config_of(
    label: &str,
    id: &str,
    command: &str,
    args: &str,
    env: &str,
) -> Result<CustomAgentConfig, String> {
    let (label, id, command) = (label.trim(), id.trim(), command.trim());
    let id = match id.is_empty() {
        true => slug(label),
        false => id.to_owned(),
    };
    if id.is_empty() {
        return Err(t!("Give it a name or an ID").to_owned());
    }
    if command.is_empty() {
        return Err(t!("Enter the command that starts it").to_owned());
    }
    let env = parse_env(env)?;
    Ok(CustomAgentConfig {
        r#type: id,
        label: (!label.is_empty()).then(|| label.to_owned()),
        command: command.to_owned(),
        args: Some(split_args(args)),
        env: (!env.is_empty()).then_some(env),
        ..CustomAgentConfig::default()
    })
}

/// `text` as an ID: lowercase letters and digits, with `-` between the words.
fn slug(text: &str) -> String {
    let mut slug = String::new();
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(slug.trim_end_matches('-').len());
    slug.chars().take(64).collect()
}

/// The words of `text`, split at spaces; a quoted run is one word.
fn split_args(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    for character in text.chars() {
        match (quote, character) {
            (Some(open), character) if character == open => quote = None,
            (Some(_), character) => word.push(character),
            (None, '"' | '\'') => {
                quote = Some(character);
                started = true;
            }
            (None, character) if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, character) => {
                word.push(character);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// `KEY=VALUE` pairs separated by commas or lines.
fn parse_env(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    for pair in text
        .split([',', '\n'])
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
    {
        match pair.split_once('=') {
            Some((name, value)) if !name.trim().is_empty() => {
                env.insert(name.trim().to_owned(), value.trim().to_owned());
            }
            _ => {
                return Err(t!("{pair} is not KEY=VALUE", pair = pair));
            }
        }
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_made_from_the_name() {
        assert_eq!(slug("DimCode"), "dimcode");
        assert_eq!(slug("  My  Agent: v2! "), "my-agent-v2");
        assert_eq!(slug("日本語"), "");
    }

    #[test]
    fn arguments_split_at_spaces_and_keep_quotes() {
        assert_eq!(split_args("acp"), ["acp"]);
        assert_eq!(split_args("  acp   --fast "), ["acp", "--fast"]);
        assert_eq!(
            split_args(r#"serve --name "two words" '' x"#),
            ["serve", "--name", "two words", "", "x"]
        );
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn variables_are_pairs_split_at_commas_or_lines() {
        let env = parse_env("A=1, B = two\nC=x=y\n\n").unwrap();
        assert_eq!(env["A"], "1");
        assert_eq!(env["B"], "two");
        assert_eq!(env["C"], "x=y");
        assert!(parse_env("A").is_err());
        assert!(parse_env("=x").is_err());
        assert!(parse_env("").unwrap().is_empty());
    }

    #[test]
    fn the_form_becomes_a_custom_agent() {
        let config = config_of("DimCode", " ", " dima ", "acp", "K=V").unwrap();
        assert_eq!(config.r#type, "dimcode");
        assert_eq!(config.label.as_deref(), Some("DimCode"));
        assert_eq!(config.command, "dima");
        assert_eq!(config.args, Some(vec!["acp".to_owned()]));
        assert_eq!(config.env.unwrap()["K"], "V");
        let by_id = config_of("", "dima", "dima", "", "").unwrap();
        assert_eq!((by_id.r#type.as_str(), by_id.label), ("dima", None));
        assert_eq!(by_id.args, Some(Vec::new()));
        assert!(config_of("", "", "dima", "", "").is_err());
        assert!(config_of("x", "", "  ", "", "").is_err());
    }
}
