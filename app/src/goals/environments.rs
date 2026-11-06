//! The page of where a project deploys and who a deploy waits on, which only the person sets.
//! Each environment gives the project's slices a step, in this order. What agents propose waits
//! at the top for the person's answer.

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, IntoElement, ParentElement, SharedString,
    Styled, Window,
};
use sikemux_core::server::board::BoardProposalParams;
use sikemux_core::server::board::BoardSettingsParams;
use sikemux_core::server::board::Environment;
use sikemux_core::server::board::Passes as EnvironmentPasses;
use sikemux_core::server::board::Recognise as EnvironmentRecognise;
use sikemux_core::server::call;

use super::goal::quiet_section;
use super::GoalsPane;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_3, TEXT_XS_H};
use crate::ui::{self, Button, Field, IconButton, Pick, Variant};

const PASSES: [(EnvironmentPasses, &str, &str); 4] = [
    (EnvironmentPasses::Nobody, "nobody", "Goes freely"),
    (EnvironmentPasses::Check, "check", "After the check"),
    (EnvironmentPasses::Person, "person", "A person"),
    (EnvironmentPasses::Production, "production", "Production"),
];

/// What the person recognises an environment by; each but Vercel's takes words.
#[derive(Clone, Copy, PartialEq)]
enum Fact {
    Rundeck,
    VercelPreview,
    VercelProduction,
    Aws,
    Kube,
    Terraform,
    Command,
}

const FACTS: [(Fact, &str, &str, &str); 7] = [
    (Fact::Rundeck, "rundeck", "Rundeck job", "dev/*"),
    (Fact::VercelPreview, "preview", "Vercel preview", ""),
    (
        Fact::VercelProduction,
        "vercelProd",
        "Vercel production",
        "",
    ),
    (Fact::Aws, "aws", "AWS profile", "acme-dev"),
    (Fact::Kube, "kube", "Kube context", "dev-cluster"),
    (Fact::Terraform, "terraform", "Terraform workspace", "dev"),
    (Fact::Command, "command", "Command", "make deploy-dev"),
];

/// A first environment, the next and the one after, as they usually go.
const FIRST: [(&str, EnvironmentPasses, &str); 3] = [
    ("Dev", EnvironmentPasses::Nobody, "dev/*"),
    ("Staging", EnvironmentPasses::Check, "staging/*"),
    ("Production", EnvironmentPasses::Production, "prod/*"),
];

/// One environment being written.
struct Row {
    name: Entity<InputState>,
    passes: EnvironmentPasses,
    fact: Fact,
    value: Entity<InputState>,
    health: Entity<InputState>,
    /// More it is recognised by than this page writes, as an agent may have proposed; kept.
    others: Vec<EnvironmentRecognise>,
    /// Let go from a held deploy, so it gives slices no step; kept.
    no_step: bool,
}

/// The project's environments while the person changes them.
#[derive(Default)]
pub(super) struct Editing {
    rows: Vec<Row>,
}

fn field(
    value: &str,
    placeholder: &str,
    window: &mut Window,
    cx: &mut Context<GoalsPane>,
) -> Entity<InputState> {
    let (value, placeholder) = (value.to_owned(), placeholder.to_owned());
    cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder(placeholder);
        input.set_value(value, window, cx);
        input
    })
}

/// The first fact this page can write of `facts`, with its words, and the rest to keep.
fn fact_of(facts: &[EnvironmentRecognise]) -> (Fact, String, Vec<EnvironmentRecognise>) {
    let Some((first, rest)) = facts.split_first() else {
        return (Fact::Rundeck, String::new(), Vec::new());
    };
    let (fact, words) = match first {
        EnvironmentRecognise::RundeckJob { path } => (Fact::Rundeck, path.clone()),
        EnvironmentRecognise::VercelTarget { production: false } => {
            (Fact::VercelPreview, String::new())
        }
        EnvironmentRecognise::VercelTarget { production: true } => {
            (Fact::VercelProduction, String::new())
        }
        EnvironmentRecognise::AwsProfile { name } => (Fact::Aws, name.clone()),
        EnvironmentRecognise::KubeContext { name } => (Fact::Kube, name.clone()),
        EnvironmentRecognise::TerraformWorkspace { name } => (Fact::Terraform, name.clone()),
        EnvironmentRecognise::Command { words } => (Fact::Command, words.clone()),
    };
    (fact, words, rest.to_vec())
}

fn written_fact(fact: Fact, words: String) -> EnvironmentRecognise {
    match fact {
        Fact::Rundeck => EnvironmentRecognise::RundeckJob { path: words },
        Fact::VercelPreview => EnvironmentRecognise::VercelTarget { production: false },
        Fact::VercelProduction => EnvironmentRecognise::VercelTarget { production: true },
        Fact::Aws => EnvironmentRecognise::AwsProfile { name: words },
        Fact::Kube => EnvironmentRecognise::KubeContext { name: words },
        Fact::Terraform => EnvironmentRecognise::TerraformWorkspace { name: words },
        Fact::Command => EnvironmentRecognise::Command { words },
    }
}

fn fact_words(fact: &EnvironmentRecognise) -> String {
    match fact {
        EnvironmentRecognise::RundeckJob { path } => format!("Rundeck {path}"),
        EnvironmentRecognise::VercelTarget { production: false } => "Vercel preview".into(),
        EnvironmentRecognise::VercelTarget { production: true } => "Vercel production".into(),
        EnvironmentRecognise::AwsProfile { name } => format!("AWS {name}"),
        EnvironmentRecognise::KubeContext { name } => format!("context {name}"),
        EnvironmentRecognise::TerraformWorkspace { name } => format!("workspace {name}"),
        EnvironmentRecognise::Command { words } => format!("`{words}`"),
    }
}

fn passes_words(passes: EnvironmentPasses) -> &'static str {
    (PASSES.iter())
        .find(|(known, ..)| *known == passes)
        .map_or("A person", |(.., words)| words)
}

impl GoalsPane {
    /// How many environments the project in front has.
    pub(super) fn environments_set(&self) -> usize {
        (self.here.as_deref())
            .and_then(|project| self.board_of(project))
            .map_or(0, |board| board.settings.environments.len())
    }

    /// Opens the project's environments to change them, which needs the window.
    fn edit_environments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.here.clone() else {
            return;
        };
        let kept = (self.board_of(&project))
            .map(|board| board.settings.environments.clone())
            .unwrap_or_default();
        let rows = kept.iter().map(|environment| {
            let (fact, words, others) = fact_of(&environment.recognise);
            Row {
                name: field(&environment.name, "Name", window, cx),
                passes: environment.passes,
                fact,
                value: field(&words, placeholder_of(fact), window, cx),
                health: field(
                    environment.health.as_deref().unwrap_or_default(),
                    "Health address (optional)",
                    window,
                    cx,
                ),
                others,
                no_step: environment.no_step,
            }
        });
        let rows = rows.collect();
        self.environments = Some(Editing { rows });
        if kept.is_empty() {
            self.add_environment(window, cx);
        }
        cx.notify();
    }

    fn add_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = self
            .environments
            .as_ref()
            .map_or(0, |editing| editing.rows.len());
        let (name, passes, words) =
            FIRST
                .get(at)
                .copied()
                .unwrap_or(("", EnvironmentPasses::Person, ""));
        let row = Row {
            name: field(name, "Name", window, cx),
            passes,
            fact: Fact::Rundeck,
            value: field(words, "dev/*", window, cx),
            health: field("", "Health address (optional)", window, cx),
            others: Vec::new(),
            no_step: false,
        };
        self.environments
            .get_or_insert_with(Editing::default)
            .rows
            .push(row);
        cx.notify();
    }

    fn change_row(
        &mut self,
        at: usize,
        change: impl FnOnce(&mut Editing, usize),
        cx: &mut Context<Self>,
    ) {
        if let Some(editing) = &mut self.environments {
            if at < editing.rows.len() {
                change(editing, at);
                cx.notify();
            }
        }
    }

    /// Saves the environments as written; the core refuses what no setting may do.
    fn save_environments(&mut self, cx: &mut Context<Self>) {
        let (Some(project), Some(editing)) = (self.here.clone(), &self.environments) else {
            return;
        };
        let environments: Vec<Environment> = editing
            .rows
            .iter()
            .map(|row| {
                let words = row.value.read(cx).value().trim().to_owned();
                let mut recognise = vec![written_fact(row.fact, words)];
                recognise.extend(row.others.iter().cloned());
                let health = row.health.read(cx).value().trim().to_owned();
                Environment {
                    name: row.name.read(cx).value().trim().to_owned(),
                    passes: row.passes,
                    recognise,
                    health: (!health.is_empty()).then_some(health),
                    no_step: row.no_step,
                }
            })
            .collect();
        let params = BoardSettingsParams {
            project,
            environments: Some(environments),
            ..BoardSettingsParams::default()
        };
        self.ask_moved::<call::BoardSettings>(params, |pane| pane.environments = None, cx);
    }

    fn answer_proposal(&mut self, accept: bool, cx: &mut Context<Self>) {
        let Some(project) = self.here.clone() else {
            return;
        };
        let params = BoardProposalParams { project, accept };
        self.ask_moved::<call::BoardAnswerProposal>(params, |_| {}, cx);
    }
}

fn placeholder_of(fact: Fact) -> &'static str {
    (FACTS.iter())
        .find(|(known, ..)| *known == fact)
        .map_or("", |(.., placeholder)| placeholder)
}

/// The page: what an agent proposed, the project's environments, and the lines that change them.
pub(super) fn page(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let board = pane
        .here
        .as_deref()
        .and_then(|project| pane.board_of(project));
    let kept = board
        .map(|board| board.settings.environments.clone())
        .unwrap_or_default();
    let proposal = board.and_then(|board| board.environments_proposal.clone());
    let t = Tokens::get(cx).clone();
    let busy = pane.busy;
    let listed = kept.len();
    let view = div()
        .flex()
        .flex_col()
        .child(quiet_section("Environments", listed, cx))
        .when_some(proposal, |view, proposal| {
            let steps: Vec<String> = (proposal.environments.iter())
                .map(|environment| {
                    let facts: Vec<String> = environment.recognise.iter().map(fact_words).collect();
                    format!(
                        "{}: {}, by {}",
                        environment.name,
                        passes_words(environment.passes).to_lowercase(),
                        facts.join(", ")
                    )
                })
                .collect();
            view.child(
                div()
                    .py(SPACE_2)
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .child(
                        div()
                            .text_size(theme::ui_text(TEXT_XS_H))
                            .text_color(t.text_primary)
                            .child(format!("@{} proposes these. {}", proposal.by, proposal.why)),
                    )
                    .children(steps.into_iter().map(|step| ui::meta(step, cx)))
                    .child(
                        div()
                            .pt(SPACE_1H)
                            .flex()
                            .gap(SPACE_2)
                            .child(
                                Button::new("environments-accept", "Accept", Variant::Primary)
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|pane, _, _, cx| {
                                            pane.answer_proposal(true, cx)
                                        }),
                                    ),
                            )
                            .child(
                                Button::new("environments-decline", "Turn down", Variant::Outline)
                                    .disabled(busy)
                                    .on_click(cx.listener(|pane, _, _, cx| {
                                        pane.answer_proposal(false, cx)
                                    })),
                            ),
                    ),
            )
        });
    let view = match &pane.environments {
        Some(editing) => view.child(editor(editing, busy, cx)),
        None => view.child(listing(&kept, busy, cx)),
    };
    view.into_any_element()
}

fn listing(kept: &[Environment], busy: bool, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let hint = if kept.is_empty() {
        "None set: every deploy waits for you, and slices go Built, Looks right, Landed."
    } else {
        "Slices take a step for each, in this order. What Sikemux doesn't recognise waits as production."
    };
    div()
        .flex()
        .flex_col()
        .children(kept.iter().map(|environment| {
            let facts: Vec<String> = environment.recognise.iter().map(fact_words).collect();
            div()
                .py(SPACE_1H)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .w(px(96.))
                        .truncate()
                        .text_size(theme::ui_text(TEXT_XS_H))
                        .text_color(t.text_primary)
                        .child(environment.name.clone()),
                )
                .child(ui::meta(
                    match environment.no_step {
                        true => format!("{}, no step", passes_words(environment.passes)),
                        false => passes_words(environment.passes).to_owned(),
                    },
                    cx,
                ))
                .child(div().flex_1())
                .child(ui::meta(facts.join(", "), cx).truncate())
        }))
        .child(
            div()
                .py(SPACE_1H)
                .child(ui::meta(hint, cx).whitespace_normal()),
        )
        .child(
            div().pt(SPACE_1H).flex().child(
                Button::new(
                    "environments-edit",
                    if kept.is_empty() {
                        "Set environments"
                    } else {
                        "Change"
                    },
                    Variant::Outline,
                )
                .disabled(busy)
                .on_click(cx.listener(|pane, _, window, cx| pane.edit_environments(window, cx))),
            ),
        )
        .into_any_element()
}

fn editor(editing: &Editing, busy: bool, cx: &mut Context<GoalsPane>) -> AnyElement {
    let entity = cx.entity();
    div()
        .pt(SPACE_2)
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .children(editing.rows.iter().enumerate().map(|(at, row)| {
            let picks: Vec<Pick> = (PASSES.iter())
                .map(|(_, key, words)| Pick::new(*key, *words))
                .collect();
            let on_passes = entity.clone();
            let passes = ui::select(
                SharedString::from(format!("environment-passes-{at}")),
                passes_words(row.passes),
                picks,
                (PASSES.iter())
                    .find(|(known, ..)| *known == row.passes)
                    .map(|(_, key, _)| SharedString::from(*key)),
                busy,
                move |picked, _, cx| {
                    let picked = (PASSES.iter())
                        .find(|(_, key, _)| *key == picked.as_ref())
                        .map(|(passes, ..)| *passes);
                    on_passes.update(cx, |pane, cx| {
                        pane.change_row(
                            at,
                            |editing, at| {
                                if let Some(picked) = picked {
                                    editing.rows[at].passes = picked;
                                }
                            },
                            cx,
                        )
                    })
                },
                cx,
            );
            let fact_picks: Vec<Pick> = (FACTS.iter())
                .map(|(_, key, words, _)| Pick::new(*key, *words))
                .collect();
            let on_fact = entity.clone();
            let chosen = FACTS.iter().find(|(known, ..)| *known == row.fact);
            let facts = ui::select(
                SharedString::from(format!("environment-fact-{at}")),
                chosen.map_or("", |(_, _, words, _)| words),
                fact_picks,
                chosen.map(|(_, key, ..)| SharedString::from(*key)),
                busy,
                move |picked, _, cx| {
                    let picked = (FACTS.iter())
                        .find(|(_, key, ..)| *key == picked.as_ref())
                        .map(|(fact, ..)| *fact);
                    on_fact.update(cx, |pane, cx| {
                        pane.change_row(
                            at,
                            |editing, at| {
                                if let Some(picked) = picked {
                                    editing.rows[at].fact = picked;
                                }
                            },
                            cx,
                        )
                    })
                },
                cx,
            );
            let words = !matches!(row.fact, Fact::VercelPreview | Fact::VercelProduction);
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(div().w(px(112.)).child(Field::new(&row.name)))
                        .child(passes)
                        .child(div().flex_1())
                        .child(
                            IconButton::new(
                                SharedString::from(format!("environment-up-{at}")),
                                Icon::ArrowUp,
                                "Earlier",
                            )
                            .small()
                            .disabled(at == 0)
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                pane.change_row(at, |editing, at| editing.rows.swap(at, at - 1), cx)
                            })),
                        )
                        .child(
                            IconButton::new(
                                SharedString::from(format!("environment-remove-{at}")),
                                Icon::Close,
                                "Remove",
                            )
                            .small()
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                pane.change_row(
                                    at,
                                    |editing, at| {
                                        editing.rows.remove(at);
                                    },
                                    cx,
                                )
                            })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(facts)
                        .when(words, |line| {
                            line.child(div().flex_1().min_w_0().child(Field::new(&row.value).mono()))
                        })
                        .child(div().flex_1().min_w_0().child(Field::new(&row.health).mono())),
                )
        }))
        .child(ui::meta(
            "Production always waits for you. What reads as production can't go without asking, and what Sikemux doesn't recognise waits as production.",
            cx,
        ).whitespace_normal())
        .child(
            div()
                .flex()
                .gap(SPACE_2)
                .child(
                    Button::new("environments-add", "Add environment", Variant::Outline)
                        .disabled(busy)
                        .on_click(cx.listener(|pane, _, window, cx| pane.add_environment(window, cx))),
                )
                .child(div().flex_1())
                .child(
                    Button::new("environments-cancel", "Cancel", Variant::Outline).on_click(
                        cx.listener(|pane, _, _, cx| {
                            pane.environments = None;
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    Button::new("environments-save", "Save", Variant::Primary)
                        .disabled(busy)
                        .on_click(cx.listener(|pane, _, _, cx| pane.save_environments(cx))),
                ),
        )
        .into_any_element()
}
