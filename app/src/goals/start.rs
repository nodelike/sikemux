//! The composer at the bottom of Home that starts work: what should get done, and how. It is the
//! chat's own composer, made and kept as [`START`], with the way to start in its bar. While the
//! person types, the core's triage suggests one of three ways and says why; the person can always
//! pick another. Sending starts the agent picked and sends it what was written with the files and
//! mentions that go with it. Hand off and Plan first also add a card that tracks the work, held by
//! that agent; a Chat adds none.

use crate::board::ChangesBoards;
use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use sikemux_view::layout::PaneKind;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{div, AnyElement, App, Context, Focusable, IntoElement, SharedString, Task, Window};
use sikemux_core::server::board::BoardAddParams;
use sikemux_core::server::board::BoardHandOffParams;
use sikemux_core::server::call;
use sikemux_core::server::launching::StartParams;
use sikemux_core::server::triage::TriageParams;
use sikemux_core::server::worktree::agent::AgentWorktreeParams;
use sikemux_view::start::{Because, HardFact, StartMode};
use sikemux_view::workspace::{ConfigPicker, LauncherInfo, ProjectInfo};
use sikemux_wire::protocol::{ChatContext, PermissionMode, FOR_AGENT_URI};
use tokio::sync::oneshot;

use super::GoalsPane;
use crate::account_picker;
use crate::chat::composer::{Act, AsideRoom, Notice, ProjectMenu, Strip};
use crate::chat::model_picker::{self, Moving, Picked};
use crate::chat::{yolo_mode, START};
use crate::core::{ChatAsk, Target};
use crate::icons::Icon;
use crate::pickers::project_switcher;
use crate::ui::Segmented;
use crate::workbench::Workbench;
use crate::worktrees::switch::{self, Switch};
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

/// The switch's room: labelled only while the rest of the bar keeps its words beside it.
const MODE_ROOM: AsideRoom = AsideRoom {
    labelled_from: 760.,
    bare: 100.,
};

/// How the agent that takes the work starts, as the composer's bar picks it. There is no chat yet
/// for these to change, so they wait here until one starts.
#[derive(Clone, Default)]
struct Chosen {
    launcher: Option<String>,
    model: Option<String>,
    payer: Option<String>,
    effort: Option<String>,
    /// Whether it acts without asking; none for what its launcher does.
    yolo: Option<bool>,
}

impl Chosen {
    fn yolo_on(&self, launcher: &LauncherInfo) -> bool {
        (self.yolo).unwrap_or(launcher.permission_mode == PermissionMode::Bypass)
    }
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
        if let Some(goals) = self.goals.pane.clone() {
            window.defer(cx, move |window, cx| {
                goals.update(cx, |goals, cx| goals.start_work(window, cx))
            });
        }
    }

    /// Asks the core how to start what Home's composer holds, once the window is done with the
    /// change.
    pub fn start_typed(&self, cx: &mut Context<Self>) {
        if let Some(goals) = self.goals.pane.clone() {
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
    /// The launcher of `provider` on `profile`, or its first when no account is named.
    fn launcher_on(&self, provider: &str, profile: Option<&str>, cx: &App) -> Option<String> {
        let bench = self.bench.upgrade()?;
        let launcher = (bench.read(cx).launchers().iter()).find(|known| {
            known.provider == provider
                && profile.is_none_or(|profile| profile_of(&known.id) == Some(profile))
        });
        launcher.map(|known| known.id.clone())
    }

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

    /// Flips whether the agent that takes the work acts without asking, when it can.
    pub(super) fn flip_yolo(&mut self, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let bench = bench.read(cx);
        let Some(launcher) = self.start_launcher(bench) else {
            return;
        };
        if skips_asking(bench, &launcher.provider) {
            self.start.chosen.yolo = Some(!self.start.chosen.yolo_on(&launcher));
            cx.notify();
        }
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
        let bench = self.bench.upgrade();
        let launcher = bench.and_then(|bench| {
            let bench = bench.read(cx);
            let launcher = self.start_launcher(bench)?;
            let skips = skips_asking(bench, &launcher.provider);
            Some((launcher, skips))
        });
        let (Some((launcher, skips)), Some(project)) = (launcher, self.start_project(cx)) else {
            self.said(
                "No agent is set up to start; set one up from the agents rail",
                cx,
            );
            return;
        };
        let asking = self.start.chosen.yolo.filter(|_| skips).map(yolo_mode);
        if self.start.mode == StartMode::HandOff {
            return self.hand_off(text, paths, project, launcher, asking, window, cx);
        }
        let (started, agent) = oneshot::channel();
        let folder = project.path.to_string_lossy().into_owned();
        let here = Target::here(&self.asks);
        let Chosen {
            model,
            payer,
            effort,
            ..
        } = self.start.chosen.clone();
        let first_model = model.clone().filter(|_| payer.is_none());
        let worktree = project.git.is_some() && project.worktree_default;
        here.start_chat_as(
            launcher.id,
            project.id.clone(),
            asking,
            (first_model, effort),
            Some(started),
        );
        let (title, details) = titled(&text);
        let mode = self.start.mode;
        let paying = payer.map(|payer| (model, payer));
        let starts_over = worktree || paying.is_some();
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
            let message = worktree.then(|| text.clone());
            let setup_error = match set_up(&here, &agent, paying, message).await {
                Ok(setup_error) => setup_error,
                Err(error) => {
                    close(&agent);
                    let _ = pane.update(cx, |pane, cx| {
                        pane.busy = false;
                        pane.said(&error, cx);
                    });
                    return;
                }
            };
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
                        attach: starts_over,
                    });
                });
                pane.expect_chat(agent, cx);
                if let Some(error) = setup_error {
                    pane.said(
                        &format!("Its worktree's setup did not all finish: {error}"),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// Hands the work to an agent that takes it in its own checkout, in the background, as a
    /// slice of its own; no chat opens.
    #[allow(clippy::too_many_arguments)]
    fn hand_off(
        &mut self,
        text: String,
        files: Vec<String>,
        project: ProjectInfo,
        launcher: LauncherInfo,
        asking: Option<PermissionMode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Chosen {
            model,
            payer,
            effort,
            ..
        } = self.start.chosen.clone();
        let asked =
            Target::here(&self.asks).request_typed::<call::BoardHandOff>(BoardHandOffParams {
                project: project.path.to_string_lossy().into_owned(),
                text: text.clone(),
                files,
                launcher: Some(launcher.id),
                model,
                provider: payer,
                permission_mode: asking,
                effort,
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

/// Readies a chat that has not started for its first message: onto the provider paying for it
/// and into a worktree of its own named after `worktree_named`. Answers why the worktree's setup
/// fell short, when it did.
async fn set_up(
    here: &Target,
    agent: &str,
    paying: Option<(Option<String>, String)>,
    worktree_named: Option<String>,
) -> Result<Option<String>, String> {
    let is_paid = paying.is_some();
    if let Some((model, payer)) = paying {
        let patch = AgentPatch {
            model: Some(model),
            model_provider: Some(Some(payer)),
            ..AgentPatch::default()
        };
        here.request_typed::<call::UpdateAgent>(UpdateAgentParams::new(agent, patch))
            .await?;
    }
    if let Some(message) = worktree_named {
        let made = here
            .request_typed::<call::CreateAgentWorktree>(AgentWorktreeParams {
                agent_id: agent.to_owned(),
                message,
                repo: None,
            })
            .await?;
        return Ok(made.setup_error);
    }
    if is_paid {
        // A provider is wired into the agent's process, so its waiting chat has to start again.
        here.request_typed::<call::StartChat>(StartParams {
            agent_id: agent.to_owned(),
            waits_for_person: true,
            again: true,
        })
        .await?;
    }
    Ok(None)
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

/// Whether agents of `provider` can start without asking before they act.
fn skips_asking(bench: &Workbench, provider: &str) -> bool {
    bench
        .reads
        .catalog
        .kind(provider)
        .is_some_and(|kind| kind.bypass)
}

/// The account a launcher's id names.
fn profile_of(launcher: &str) -> Option<&str> {
    launcher.split_once(':').map(|(_, profile)| profile)
}

/// The composer's menus over the agent that takes the work: its model, moving to another agent,
/// and the effort it runs at, which keep what is picked until it starts.
fn agent_menu(
    chosen: &Chosen,
    shown: &LauncherInfo,
    bench: &Workbench,
    goals: &gpui_kit::WeakEntity<GoalsPane>,
) -> (Picked, Option<ConfigPicker>, model_picker::Set) {
    let mut model = shown.model.clone();
    if let (Some(model), Some(picked)) = (model.as_mut(), &chosen.model) {
        model.current = picked.clone();
    }
    let mut effort = shown.effort.clone();
    if let Some(effort) = effort.as_mut() {
        if let Some(picked) = &chosen.effort {
            effort.current = picked.clone();
        }
        if effort.current.is_empty() {
            effort.current = "Effort".into();
        }
    }
    let set: model_picker::Set = {
        let goals = goals.clone();
        Rc::new(move |id, value, _, cx| {
            let _ = goals.update(cx, |pane, cx| {
                match id {
                    "model" => pane.start.chosen.model = Some(value.to_owned()),
                    _ => pane.start.chosen.effort = Some(value.to_owned()),
                }
                cx.notify();
            });
        })
    };
    let moving: model_picker::Move = {
        let goals = goals.clone();
        Rc::new(move |to: Moving, _, cx| {
            let _ = goals.update(cx, |pane, cx| {
                let launcher = pane.launcher_on(&to.provider, to.profile.as_deref(), cx);
                let chosen = &mut pane.start.chosen;
                chosen.launcher = launcher;
                chosen.model = to.model.clone();
                chosen.payer = to.model_provider.clone();
                chosen.effort = None;
                cx.notify();
            });
        })
    };
    let picked = Picked {
        provider: shown.provider.clone(),
        profile: profile_of(&shown.id).map(str::to_owned),
        model,
        model_provider: chosen.payer.clone(),
        locked: false,
        pending: Some("Models show once the agent starts"),
        set: set.clone(),
        moving,
        target: bench.target(),
    };
    (picked, effort, set)
}

/// The strip on Home's composer: this host, the project in front and a switcher to another, its
/// branch, whether the work takes a worktree, and the account at the right end.
fn strip(
    bench: &Workbench,
    project: &ProjectInfo,
    launcher: Option<&LauncherInfo>,
    mode: StartMode,
    goals: &gpui_kit::WeakEntity<GoalsPane>,
    cx: &mut Context<Workbench>,
) -> Strip {
    let folder = project.path.to_string_lossy().into_owned();
    let worktree = project.git.is_some().then(|| {
        let on = project.worktree_default;
        let shows = match mode {
            StartMode::HandOff => Switch::Always,
            _ => Switch::Choosing { on },
        };
        switch::toggle(
            "worktree-start",
            &shows,
            bench.flip_worktree_default(project, on),
            cx,
        )
    });
    let account = launcher.and_then(|launcher| {
        let on = (profile_of(&launcher.id).map(str::to_owned)).or_else(|| {
            let current = bench.accounts.current(&launcher.provider)?;
            Some(current.id.clone())
        })?;
        let pick: account_picker::Pick = {
            let provider = launcher.provider.clone();
            let goals = goals.clone();
            Rc::new(move |chosen, cx| {
                let _ = goals.update(cx, |pane, cx| {
                    pane.start.chosen.launcher = pane.launcher_on(&provider, Some(&chosen.id), cx);
                    cx.notify();
                });
            })
        };
        let chat = account_picker::Chat {
            key: START,
            provider: &launcher.provider,
            on: &on,
            started: false,
            pick,
        };
        account_picker::picker(bench, chat, false, false, cx)
    });
    let moves: ProjectMenu = {
        let (cwd, this) = (folder.clone(), cx.entity());
        let on_pick = project_switcher::activates(cx.weak_entity());
        Box::new(move |trigger| {
            project_switcher::picker(
                "start-project".into(),
                trigger,
                &cwd,
                gpui_kit::Anchor::BottomLeft,
                on_pick,
                &this,
            )
        })
    };
    Strip {
        host: None,
        project: project.name.clone().into(),
        path: crate::ui::path::tilde(&folder, crate::ui::path::this_home().as_deref()).into(),
        branch: (project.git.as_ref()).map(|git| git.branch.clone().into()),
        moves: Some(moves),
        worktree,
        pull: None,
        goal: None,
        account,
    }
}

/// Home's composer: the chat's, with where the work starts, the way it starts and the agent
/// that takes it.
pub fn start_composer(bench: &mut Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let Some(goals) = bench.goals.pane.clone() else {
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
    let Some((composer, offering)) = crate::chat::drafting(
        bench,
        START,
        provider,
        label,
        (on_send, Rc::new(|_, _| {})),
        cx,
    ) else {
        return div().into_any_element();
    };
    let (mut composer, tray_open) = crate::chat::with_stash(bench, START, composer, cx);
    let strip =
        (bench.project()).map(|project| strip(bench, project, launcher.as_ref(), mode, &weak, cx));
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
        .ready(true)
        .strip(strip)
        .aside(MODE_ROOM, move |roomy| modes.icons_only(!roomy));
    if notice.is_some() {
        composer = composer.notice(notice);
    }
    if let Some(launcher) = &launcher {
        let (picked, effort, set) = agent_menu(&chosen, launcher, bench, &weak);
        composer = composer.model_menu(picked, cx.entity());
        if effort.is_some() {
            composer = composer.pickers(None, effort, set);
        }
        if skips_asking(bench, &launcher.provider) {
            let flip = weak.clone();
            composer = composer.yolo(
                chosen.yolo_on(launcher),
                false,
                Rc::new(move |_, cx| {
                    let _ = flip.update(cx, |pane, cx| pane.flip_yolo(cx));
                }),
            );
        }
    }
    crate::chat::mounted(START, composer, offering || tray_open, cx).into_any_element()
}
