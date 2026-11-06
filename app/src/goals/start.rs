//! The composer at the bottom of Home that starts work: what should get done, and how. It is the
//! chat's own composer, made and kept as [`START`], with the way to start in its bar. While the
//! person types, the core's triage suggests one of three ways and says why; the person can always
//! pick another. Sending starts the agent picked and sends it what was written with the files and
//! mentions that go with it. Hand off and Plan first also add a card that tracks the work, held by
//! that agent; a Chat adds none.

use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use sikemux_view::layout::PaneKind;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{div, AnyElement, App, Context, Focusable, IntoElement, SharedString, Task, Window};
use sikemux_core::server::board::BoardAddParams;
use sikemux_core::server::board::BoardHandOffParams;
use sikemux_core::server::call;
use sikemux_core::server::triage::TriageParams;
use sikemux_view::start::{Because, HardFact, StartMode};
use sikemux_view::workspace::{LauncherInfo, ProjectInfo};
use sikemux_wire::protocol::{ChatContext, FOR_AGENT_URI};
use tokio::sync::oneshot;

use super::GoalsPane;
use crate::chat::composer::{Act, Notice, Strip};
use crate::chat::model_picker::{self, Moving, Picked};
use crate::chat::START;
use crate::core::{ChatAsk, Target};
use crate::icons::Icon;
use crate::ui::Segmented;
use crate::workbench::Workbench;
use sikemux_core::server::workspace::{AgentPatch, CloseAgentsParams, UpdateAgentParams};

/// How long typing pauses before the core is asked to suggest a way.
const SETTLE: Duration = Duration::from_millis(300);
/// How long the line saying where a Hand off went stays under the composer.
const HANDED_SHOWN: Duration = Duration::from_secs(4);
/// The longest card title the box makes; the rest of what was typed goes into its details.
const TITLE_CHARS: usize = 80;

/// Each way to start, with its icon, what it is called and what it does, in a line.
const MODES: [(StartMode, Icon, &str, &str); 3] = [
    (
        StartMode::HandOff,
        Icon::HandOff,
        "Hand off",
        "An agent takes it and works on its own; you hear back only if it needs you.",
    ),
    (
        StartMode::PlanFirst,
        Icon::Steps,
        "Plan first",
        "A lead talks it through with you, then works in slices you approve.",
    ),
    (
        StartMode::Chat,
        Icon::Message,
        "Chat",
        "Work on it together, turn by turn.",
    ),
];

/// The agent the person picked to take the work, as the composer's agent menu picks it.
#[derive(Clone, Default)]
struct Chosen {
    launcher: Option<String>,
    model: Option<String>,
    payer: Option<String>,
}

/// How Home's composer starts work; what it holds is the composer's own.
pub(super) struct Start {
    mode: StartMode,
    /// The person picked the mode, so what the core suggests no longer moves it.
    picked: bool,
    /// The way the core suggests for what is typed, and why when it is more than doing it.
    suggested: Option<(StartMode, Option<Because>)>,
    chosen: Chosen,
    /// What was started from the box, oldest first, which the arrows bring back.
    sent: Vec<String>,
    recalled: Option<(usize, String)>,
    /// The card the last Hand off became, for a few seconds or until the person types again.
    handed: Option<u64>,
    _shown: Task<()>,
    _triage: Task<()>,
}

impl Default for Start {
    fn default() -> Self {
        Self {
            mode: StartMode::default(),
            picked: false,
            suggested: None,
            chosen: Chosen::default(),
            sent: Vec::new(),
            recalled: None,
            handed: None,
            _shown: Task::ready(()),
            _triage: Task::ready(()),
        }
    }
}

impl Start {
    /// Back to the way the core suggests after a start, keeping the agent picked and what was sent.
    fn cleared(&mut self) {
        *self = Start {
            chosen: std::mem::take(&mut self.chosen),
            sent: std::mem::take(&mut self.sent),
            ..Start::default()
        };
    }
}

impl Workbench {
    /// Starts the work Home's composer holds, once the window is done with what sent it.
    pub fn start_from_home(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(goals) = self.goals.clone() {
            window.defer(cx, move |window, cx| {
                goals.update(cx, |goals, cx| goals.start_work(window, cx))
            });
        }
    }

    /// Asks the core how to start what Home's composer holds, once the window is done with the
    /// change.
    pub fn start_typed(&self, cx: &mut Context<Self>) {
        if let Some(goals) = self.goals.clone() {
            cx.defer(move |cx| goals.update(cx, |goals, cx| goals.typed(cx)));
        }
    }
}

impl GoalsPane {
    /// What Home's composer holds: its text, and the files and mentions going with it.
    fn written(&self, cx: &App) -> Option<(String, Vec<String>, Vec<ChatContext>)> {
        let bench = self.bench.upgrade()?;
        let composing = bench.read(cx).composing(START)?;
        let text = composing.draft.read(cx).value().trim().to_owned();
        let (paths, context) = composing.outgoing(cx);
        Some((text, paths, context))
    }

    /// Empties Home's composer once what it held has started.
    fn emptied(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let _ = (self.bench).update(cx, |bench, cx| bench.emptied(START, window, cx));
        self.start.sent.push(text);
        self.start.cleared();
    }

    /// The agent work starts with: the one picked, else the one the rail starts with.
    pub(super) fn start_launcher(&self, bench: &Workbench) -> Option<LauncherInfo> {
        let picked = (self.start.chosen.launcher.as_deref())
            .and_then(|id| bench.launchers().iter().find(|known| known.id == id));
        picked.or_else(|| bench.launcher()).cloned()
    }

    /// Asks the core to suggest a way once typing settles.
    fn typed(&mut self, cx: &mut Context<Self>) {
        let text = self.written(cx).map(|(text, ..)| text).unwrap_or_default();
        if !text.is_empty() {
            self.start.handed = None;
        }
        if text.is_empty() {
            self.start.suggested = None;
            if !self.start.picked {
                self.start.mode = StartMode::default();
            }
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
                    pane.start.suggested = Some((triaged.mode, triaged.because));
                    cx.notify();
                }
            });
        });
    }

    /// The project work starts in: the one in front of the workbench.
    fn start_project(&self, cx: &App) -> Option<ProjectInfo> {
        let bench = self.bench.upgrade()?;
        let project = bench.read(cx).project().cloned();
        project
    }

    fn pick(&mut self, mode: StartMode, cx: &mut Context<Self>) {
        self.start.mode = mode;
        self.start.picked = true;
        cx.notify();
    }

    /// Brings an older or newer start back into the draft, as a chat's arrows do.
    pub(super) fn recall(&mut self, older: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let Some(draft) = (bench.read(cx).composing(START)).map(|start| start.draft.clone()) else {
            return;
        };
        let typed = draft.read(cx).value().to_string();
        let start = &mut self.start;
        match crate::chat::step_prompts(&start.sent, &mut start.recalled, older, &typed) {
            Some(text) => draft.update(cx, |draft, cx| draft.set_value(text, window, cx)),
            None => cx.propagate(),
        }
    }

    /// Starts the agent picked and sends it what was written. Planning first also adds the card
    /// that tracks the work, held by that agent, and tells it which card is theirs and what to do.
    fn start_work(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, paths, context)) = self.written(cx) else {
            return;
        };
        if text.is_empty() || self.busy {
            return;
        }
        let launcher = (self.bench.upgrade()).and_then(|bench| self.start_launcher(bench.read(cx)));
        let (Some(launcher), Some(project)) = (launcher, self.start_project(cx)) else {
            self.said(
                "No agent is set up to start; set one up from the agents rail",
                cx,
            );
            return;
        };
        if self.start.mode == StartMode::HandOff {
            return self.hand_off(text, paths, project, launcher, window, cx);
        }
        let (started, agent) = oneshot::channel();
        let folder = project.path.to_string_lossy().into_owned();
        let here = Target::here(&self.asks);
        here.start_chat(launcher.id, project.id.clone(), None, Some(started));
        let (title, details) = titled(&text);
        let mode = self.start.mode;
        let Chosen { model, payer, .. } = self.start.chosen.clone();
        // An agent whose card could not be added would sit in an empty tab.
        let close = {
            let here = here.clone();
            move |agent: &str| {
                here.call_typed::<call::CloseAgents>(CloseAgentsParams {
                    agent_ids: vec![agent.to_owned()],
                })
            }
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
            if model.is_some() || payer.is_some() {
                let patch = AgentPatch {
                    model: Some(model),
                    model_provider: Some(payer),
                    ..AgentPatch::default()
                };
                let changed = here
                    .request_typed::<call::UpdateAgent>(UpdateAgentParams::new(
                        agent.clone(),
                        patch,
                    ))
                    .await;
                if let Err(error) = changed {
                    close(&agent);
                    let _ = pane.update(cx, |pane, cx| {
                        pane.busy = false;
                        pane.said(&error, cx);
                    });
                    return;
                }
            }
            let added = match mode {
                StartMode::PlanFirst => Some(
                    here.request_typed::<call::BoardAdd>(BoardAddParams {
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
                    .await,
                ),
                _ => None,
            };
            let _ = pane.update_in(cx, |pane, window, cx| {
                pane.busy = false;
                let mut context = context;
                if let Some(added) = added {
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
                    context.push(lead_brief(card));
                }
                pane.emptied(text.clone(), window, cx);
                let _ = pane.bench.update(cx, |bench, _| {
                    bench.open_here(OpenTab::new(
                        At::project(&project.id),
                        PaneKind::Agent,
                        agent.clone(),
                    ));
                    bench.ask_chat(ChatAsk::Prompt {
                        agent: agent.clone(),
                        text,
                        paths,
                        context,
                        attach: false,
                    });
                });
                pane.expect_chat(agent, cx);
            });
        })
        .detach();
    }

    /// Hands the work to an agent that takes it in its own checkout, in the background, as a
    /// slice of its own; no chat opens.
    fn hand_off(
        &mut self,
        text: String,
        files: Vec<String>,
        project: ProjectInfo,
        launcher: LauncherInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Chosen { model, payer, .. } = self.start.chosen.clone();
        let asked =
            Target::here(&self.asks).request_typed::<call::BoardHandOff>(BoardHandOffParams {
                project: project.path.to_string_lossy().into_owned(),
                text: text.clone(),
                files,
                launcher: Some(launcher.id),
                model,
                provider: payer,
            });
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |pane, cx| {
            let handed = asked.await;
            let _ = pane.update_in(cx, |pane, window, cx| {
                pane.busy = false;
                match handed {
                    Ok(handed) => {
                        pane.take_one(handed.board);
                        pane.emptied(text, window, cx);
                        pane.start.handed = Some(handed.card);
                        pane.start._shown = cx.spawn(async move |pane, cx| {
                            cx.background_executor().timer(HANDED_SHOWN).await;
                            let _ = pane.update(cx, |pane, cx| {
                                pane.start.handed = None;
                                cx.notify();
                            });
                        });
                        cx.emit(super::Moved);
                    }
                    Err(error) => pane.said(&error, cx),
                }
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

/// What a lead that plans first is told with the person's first message, which the person wrote
/// and the chat shows alone.
fn lead_brief(card: u64) -> ChatContext {
    ChatContext {
        uri: format!("{FOR_AGENT_URI}card-{card}"),
        title: format!("Goal card #{card}"),
        text: format!(
            "You lead goal card #{card} on the project's board, which you hold. Before anything \
             is built, talk it through with me: ask what you need to know, one question at a \
             time, and record what I decide with board_decide. Then propose the slices as cards \
             under #{card} with board_add, each with a promise, and wait for me to approve them \
             before any work starts."
        ),
    }
}

/// Why the core suggested planning first, in a few words.
fn because_words(because: &Because) -> String {
    let fact = match because.fact {
        HardFact::Server => "server code",
        HardFact::Migration => "a migration",
        HardFact::Protocol => "the protocol",
        HardFact::SeenByUsers => "what people see",
        HardFact::Listed => "your project lists it",
    };
    format!("Touches {} · {fact}", because.path)
}

/// The line under the composer: the agent starting, or the card the last Hand off became, which
/// opens it.
fn notice(pane: &GoalsPane, goals: &gpui_kit::WeakEntity<GoalsPane>) -> Option<Notice> {
    if pane.busy {
        return Some(Notice::Waiting("Starting the agent…".into()));
    }
    let card = pane.start.handed?;
    let goals = goals.clone();
    let open: Act = Rc::new(move |_, cx| {
        let _ = goals.update(cx, |pane, cx| pane.go(super::At::Slice(card), cx));
    });
    Some(Notice::Done(
        format!("Handed off as #{card}").into(),
        ("Open".into(), open),
    ))
}

/// The agent menu of the composer, picking which agent, account, provider and model start.
fn agent_menu(
    chosen: &Chosen,
    shown: &LauncherInfo,
    bench: &Workbench,
    goals: &gpui_kit::WeakEntity<GoalsPane>,
) -> Picked {
    let launchers = bench.launchers().to_vec();
    let mut model = shown.model.clone();
    if let (Some(model), Some(picked)) = (model.as_mut(), &chosen.model) {
        model.current = picked.clone();
    }
    let goals = goals.clone();
    let choose = move |cx: &mut App, change: &dyn Fn(&mut Chosen)| {
        let _ = goals.update(cx, |pane, cx| {
            change(&mut pane.start.chosen);
            cx.notify();
        });
    };
    let choose = Rc::new(choose);
    let set: model_picker::Set = {
        let choose = choose.clone();
        Rc::new(move |id, value, _, cx| {
            if id == "model" {
                choose(cx, &|chosen| chosen.model = Some(value.to_owned()));
            }
        })
    };
    let moving: model_picker::Move = Rc::new(move |to: Moving, _, cx| {
        let profile_of = |id: &str| id.split_once(':').map(|(_, profile)| profile.to_owned());
        let launcher = launchers.iter().find(|known| {
            known.provider == to.provider
                && (to.profile.as_deref())
                    .is_none_or(|profile| profile_of(&known.id).as_deref() == Some(profile))
        });
        let launcher = launcher.map(|known| known.id.clone());
        choose(cx, &|chosen| {
            *chosen = Chosen {
                launcher: launcher.clone(),
                model: to.model.clone(),
                payer: to.model_provider.clone(),
            }
        });
    });
    Picked {
        provider: shown.provider.clone(),
        profile: shown
            .id
            .split_once(':')
            .map(|(_, profile)| profile.to_owned()),
        model,
        model_provider: chosen.payer.clone(),
        locked: false,
        pending: None,
        set,
        moving,
        target: bench.target(),
    }
}

/// Home's composer: the chat's, with where the work starts, the way it starts and the agent
/// that takes it.
pub fn start_composer(bench: &mut Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let Some(goals) = bench.goals.clone() else {
        return div().into_any_element();
    };
    let weak = goals.downgrade();
    let (launcher, mode, suggested, chosen, notice) = {
        let pane = goals.read(cx);
        (
            pane.start_launcher(bench),
            pane.start.mode,
            pane.start.suggested.clone(),
            pane.start.chosen.clone(),
            notice(pane, &weak),
        )
    };
    let provider = launcher.as_ref().map_or("", |launcher| &launcher.provider);
    let label = launcher.as_ref().map_or_else(
        || "No agent".to_owned(),
        |launcher| bench.agent_name(&launcher.provider).to_owned(),
    );
    let on_send: Act = {
        let this = cx.weak_entity();
        Rc::new(move |window, cx| {
            this.update(cx, |bench, cx| bench.send(START, false, window, cx))
                .ok();
        })
    };
    let Some((mut composer, offering)) = crate::chat::drafting(
        bench,
        START,
        provider,
        label,
        (on_send, Rc::new(|_, _| {})),
        cx,
    ) else {
        return div().into_any_element();
    };
    let strip = bench.project().map(|project| {
        let folder = project.path.to_string_lossy().into_owned();
        Strip {
            host: None,
            project: project.name.clone().into(),
            path: crate::ui::path::tilde(&folder, crate::ui::path::this_home().as_deref()).into(),
            branch: (project.git.as_ref()).map(|git| git.branch.clone().into()),
            moves: None,
            worktree: None,
            pull: None,
            goal: None,
            account: None,
        }
    });
    let at = MODES
        .iter()
        .position(|(each, ..)| *each == mode)
        .unwrap_or(0);
    let choices = MODES
        .iter()
        .map(|(_, glyph, name, _)| (*glyph, SharedString::from(*name)))
        .collect();
    let tips = MODES.iter().map(|(.., tip)| SharedString::from(*tip));
    let picking = weak.clone();
    let draft = (bench.composing(START)).map(|start| start.draft.read(cx).focus_handle(cx));
    let mut modes = Segmented::new("start-mode", choices, at)
        .tooltips(tips)
        .on_select(move |chosen: &usize, window, cx| {
            if let Some((mode, ..)) = MODES.get(*chosen) {
                let _ = picking.update(cx, |pane, cx| pane.pick(*mode, cx));
            }
            if let Some(draft) = &draft {
                draft.focus(window, cx);
            }
        });
    if let Some((mode, because)) = &suggested {
        let at = MODES.iter().position(|(each, ..)| each == mode);
        let why = because.as_ref().map(because_words);
        if let (Some(at), Some(why)) = (at, why) {
            modes = modes.suggested(at, why);
        }
    }
    composer = composer
        .short()
        .strip(strip)
        .aside(move |roomy| modes.icons_only(!roomy));
    if notice.is_some() {
        composer = composer.notice(notice);
    }
    if let Some(launcher) = &launcher {
        let picked = agent_menu(&chosen, launcher, bench, &weak);
        composer = composer.model_menu(picked, cx.entity());
    }
    crate::chat::mounted(START, composer, offering, cx).into_any_element()
}
