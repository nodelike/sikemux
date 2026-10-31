//! The box at the top of Home that starts work: what should get done, and how. While the person
//! types, the core's triage pre-picks one of three ways and says why; the person can always pick
//! another. Starting adds the card that tracks the work and starts the agent that takes it.

use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    div, AnyElement, AppContext, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
    Subscription, Task, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::{BoardAddParams, TriageParams};
use sikemux_wire::protocol::ProjectInfo;
use sikemux_wire::start::{Because, HardFact, StartMode};
use tokio::sync::oneshot;

use super::GoalsPane;
use crate::core::Target;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_3};
use crate::ui::{self, Button, Field, Segmented, Variant};

/// How long typing pauses before the core is asked to pre-pick.
const SETTLE: Duration = Duration::from_millis(300);
/// The longest card title the box makes; the rest of what was typed goes into its details.
const TITLE_CHARS: usize = 80;

const MODES: [(StartMode, &str); 3] = [
    (StartMode::DoIt, "Do it"),
    (StartMode::PlanFirst, "Plan first"),
    (StartMode::WithMe, "With me"),
];

/// What the box holds.
pub(super) struct Start {
    input: Option<(Entity<InputState>, Subscription)>,
    mode: StartMode,
    /// The person picked the mode, so what the core pre-picks no longer moves it.
    picked: bool,
    because: Option<Because>,
    _triage: Task<()>,
}

impl Default for Start {
    fn default() -> Self {
        Self {
            input: None,
            mode: StartMode::DoIt,
            picked: false,
            because: None,
            _triage: Task::ready(()),
        }
    }
}

impl GoalsPane {
    /// Makes the box's field, which needs the window.
    pub(super) fn start_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.start.input.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("What should get done?"));
        let typing = cx.subscribe_in(&input, window, |pane, _, event, window, cx| match event {
            InputEvent::Change => pane.typed(cx),
            InputEvent::PressEnter { .. } => pane.start_work(window, cx),
            _ => {}
        });
        self.start.input = Some((input, typing));
    }

    fn typed_text(&self, cx: &gpui_kit::App) -> String {
        self.start
            .input
            .as_ref()
            .map_or(String::new(), |(input, _)| {
                input.read(cx).value().trim().to_owned()
            })
    }

    /// The project work starts in: the one in front of the workbench.
    fn start_project(&self, cx: &gpui_kit::App) -> Option<ProjectInfo> {
        let bench = self.bench.upgrade()?;
        let project = bench.read(cx).project().cloned();
        project
    }

    /// Asks the core to pre-pick a way once typing settles.
    fn typed(&mut self, cx: &mut Context<Self>) {
        let text = self.typed_text(cx);
        if text.is_empty() {
            self.start = Start {
                input: self.start.input.take(),
                ..Start::default()
            };
            cx.notify();
            return;
        }
        let Some(project) = self.start_project(cx) else {
            return;
        };
        let params = TriageParams {
            project: project.path.to_string_lossy().into_owned(),
            text,
        };
        let asks = self.asks.clone();
        self.start._triage = cx.spawn(async move |pane, cx| {
            cx.background_executor().timer(SETTLE).await;
            let triaged = Target::here(&asks)
                .request_typed::<call::Triage>(params)
                .await;
            let _ = pane.update(cx, |pane, cx| {
                if let Ok(triaged) = triaged {
                    if !pane.start.picked {
                        pane.start.mode = triaged.mode;
                    }
                    pane.start.because = triaged.because;
                    cx.notify();
                }
            });
        });
    }

    fn pick(&mut self, mode: StartMode, cx: &mut Context<Self>) {
        self.start.mode = mode;
        self.start.picked = true;
        cx.notify();
    }

    /// Starts the agent that takes the work, adds the card that tracks it, held by that agent,
    /// then sends the agent a first message saying which card is theirs and what to do.
    fn start_work(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.typed_text(cx);
        if text.is_empty() || self.busy {
            return;
        }
        let launcher =
            (self.bench.upgrade()).and_then(|bench| Some(bench.read(cx).launcher()?.id.clone()));
        let (Some(launcher), Some(project)) = (launcher, self.start_project(cx)) else {
            self.said(
                "No agent is set up to start; set one up from the agents rail",
                cx,
            );
            return;
        };
        let (started, agent) = oneshot::channel();
        let folder = project.path.to_string_lossy().into_owned();
        Target::here(&self.asks).start_chat(launcher, project.id, None, Some(started));
        let (title, details) = titled(&text);
        let (mode, asks) = (self.start.mode, self.asks.clone());
        // An agent whose card could not be added would sit in an empty tab.
        let close = move |agent: &str| {
            Target::here(&asks).call("closeAgents", serde_json::json!({ "agentIds": [agent] }));
        };
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |pane, cx| {
            let Ok(agent) = agent.await else {
                let _ = pane.update(cx, |pane, cx| {
                    pane.busy = false;
                    pane.said("The agent did not start", cx);
                });
                return;
            };
            let Ok(asked) = pane.update(cx, |pane, _| {
                Target::here(&pane.asks).request_typed::<call::BoardAdd>(BoardAddParams {
                    project: folder,
                    title,
                    details,
                    acceptance: None,
                    promise: None,
                    asserts: Vec::new(),
                    parent: None,
                    depends_on: Vec::new(),
                    ticket: None,
                    owner: Some(agent.clone()),
                })
            }) else {
                close(&agent);
                return;
            };
            let added = asked.await;
            let _ = pane.update_in(cx, |pane, window, cx| {
                pane.busy = false;
                let board = match added {
                    Ok(board) => board,
                    Err(error) => {
                        close(&agent);
                        return pane.said(&error, cx);
                    }
                };
                let card = (board.cards.iter())
                    .filter(|card| card.owner.as_deref() == Some(agent.as_str()))
                    .map(|card| card.id)
                    .max();
                pane.take_one(board);
                let Some(card) = card else {
                    return;
                };
                if let Some((input, _)) = &pane.start.input {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
                pane.start = Start {
                    input: pane.start.input.take(),
                    ..Start::default()
                };
                let first = first_message(mode, card, &text);
                let _ = pane
                    .bench
                    .update(cx, |bench, cx| bench.deliver(&agent, first, window, cx));
                cx.notify();
            });
        })
        .detach();
    }
}

/// A card title from what was typed: its first line, cut short, with the whole text as the
/// details when the title leaves some out.
fn titled(text: &str) -> (String, Option<String>) {
    let line = text.lines().next().unwrap_or_default();
    let title: String = line.chars().take(TITLE_CHARS).collect();
    let whole = title.len() == text.len();
    (title, (!whole).then(|| text.to_owned()))
}

/// What the agent that takes the work is told first.
fn first_message(mode: StartMode, card: u64, text: &str) -> String {
    match mode {
        StartMode::DoIt => format!(
            "{text}\n\nThis is card #{card} on the project's board, and you hold it: do it, \
             and mark it done with board_update once the project's checks pass."
        ),
        StartMode::PlanFirst => format!(
            "{text}\n\nYou lead goal card #{card} on the project's board, which you hold. \
             Before anything is built, talk it through with me: ask what you need to know, one \
             question at a time, and record what I decide with board_decide. Then propose the \
             slices as cards under #{card} with board_add, each with a promise, and wait for me \
             to approve them before any work starts."
        ),
        StartMode::WithMe => format!(
            "{text}\n\nSikemux tracks this as goal card #{card} on the project's board, which \
             you hold."
        ),
    }
}

/// Why the core pre-picked more than doing it at once, in a few words.
fn because_words(because: &Because) -> String {
    let fact = match because.fact {
        HardFact::Server => "server code",
        HardFact::Migration => "a migration",
        HardFact::Protocol => "the protocol",
        HardFact::SeenByUsers => "what people see",
        HardFact::Listed => "your project lists it",
    };
    format!("touches {} · {fact}", because.path)
}

pub(super) fn start_box(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some((input, _)) = &pane.start.input else {
        return div().into_any_element();
    };
    let project = pane
        .start_project(cx)
        .map_or_else(|| "no project open".to_owned(), |project| project.name);
    let chosen = MODES
        .iter()
        .position(|(mode, _)| *mode == pane.start.mode)
        .unwrap_or(0);
    let words = MODES
        .iter()
        .map(|(_, name)| (SharedString::from(*name), None))
        .collect();
    div()
        .mt(theme::SPACE_5)
        .p(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_3)
        .border_1()
        .border_color(t.border)
        .bg(t.surface_raised)
        .child(Field::new(input))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .child(ui::icon(Icon::Folder, 12.).text_color(t.text_tertiary))
                        .child(ui::meta(project, cx)),
                )
                .child(
                    Segmented::words("start-mode", words, chosen).on_select(cx.listener(
                        |pane, chosen: &usize, _, cx| {
                            if let Some((mode, _)) = MODES.get(*chosen) {
                                pane.pick(*mode, cx);
                            }
                        },
                    )),
                )
                .children(
                    pane.start
                        .because
                        .as_ref()
                        .map(|because| ui::meta(because_words(because), cx).truncate()),
                )
                .child(div().flex_1())
                .child(
                    Button::new("start-work", "Start", Variant::Primary)
                        .disabled(pane.busy)
                        .on_click(cx.listener(|pane, _, window, cx| pane.start_work(window, cx))),
                ),
        )
        .into_any_element()
}
