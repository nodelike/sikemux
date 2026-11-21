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
use sikemux_calls::agent_updates::Updating;
use std::f32::consts::FRAC_PI_2;

use crate::agents::setup::{installing, run, signing_in, update_everywhere, Found, Sheet};
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
        "install reinstall update upgrade version latest missing hosts claude codex opencode hermes pi omp grok antigravity agy gemini google cli executable path program profile",
    )];

/// The row that is open, and the program being written for the account it runs on.
pub struct Fields {
    open: Option<String>,
    editing: Option<String>,
    program: Entity<InputState>,
}

impl Fields {
    pub fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let program =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("Leave empty to use PATH")));
        Self {
            open: None,
            editing: None,
            program,
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
    let body: AnyElement = match bench.agent_setup.found {
        None => rows::note(t!("Looking for agents…"), cx),
        Some(_) => div()
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
        update_all,
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
    let label = div()
        .flex()
        .items_baseline()
        .gap(SPACE_2)
        .child(agent.label.to_owned())
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
            Some(ui::mark(agent.kind, 16.)),
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
    match agent.here.map(|found| (found, &found.status)) {
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
            match signing_in(found, folder.as_deref()) {
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
    div()
        .pl(px(48.))
        .pr(SPACE_4)
        .pb(SPACE_2)
        .border_b(theme::border())
        .border_color(t.line_soft)
        .child(part(t!("Hosts"), hosts.into_any_element()))
        .children(accounts.map(|accounts| part(t!("Accounts"), accounts)))
        .child(part(t!("Runs"), runs(bench, agent, fields, this, cx)))
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
