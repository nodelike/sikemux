//! The composer at the top of Home that starts work: what should get done, and how. It is the
//! chat's own composer, with the way to start in its bar. While the person types, the core's
//! triage suggests one of three ways and says why; the person can always pick another. Sending
//! starts the agent picked, adds the card that tracks the work, held by that agent, and sends
//! the agent what was written with the files that go with it.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::{
    div, AnyElement, App, AppContext, ClipboardEntry, ClipboardItem, Context, Entity,
    InteractiveElement, IntoElement, ParentElement, PathPromptOptions, SharedString, Subscription,
    Task, WeakEntity, Window,
};
use serde_json::json;
use sikemux_core::server::call;
use sikemux_core::server::{BoardAddParams, TriageParams};
use sikemux_wire::protocol::{LauncherInfo, ProjectInfo};
use sikemux_wire::start::{Because, HardFact, StartMode};
use tokio::sync::oneshot;

use super::GoalsPane;
use crate::chat::composer::{Act, Composer, Notice, Strip};
use crate::chat::model_picker::{self, Moving, Picked};
use crate::chat::{NewerPrompt, OlderPrompt};
use crate::core::{ChatAsk, Target};
use crate::ui::Segmented;
use crate::voice::MicButton;
use crate::workbench::Workbench;

/// What dictation names the start box's draft by, as it names a chat's by its agent.
pub const START_DRAFT: &str = "goals-start";
/// How long typing pauses before the core is asked to suggest a way.
const SETTLE: Duration = Duration::from_millis(300);
/// The longest card title the box makes; the rest of what was typed goes into its details.
const TITLE_CHARS: usize = 80;

/// Each way to start, with what it is called and what it does, in a line.
const MODES: [(StartMode, &str, &str); 3] = [
    (
        StartMode::HandOff,
        "Hand off",
        "An agent takes it and works on its own; you hear back only if it needs you.",
    ),
    (
        StartMode::PlanFirst,
        "Plan first",
        "A lead talks it through with you, then works in slices you approve.",
    ),
    (
        StartMode::Chat,
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

/// What the box holds.
pub(super) struct Start {
    draft: Option<(Entity<TextareaState>, Subscription)>,
    mic: Option<Entity<MicButton>>,
    /// The files going with what is sent.
    files: Vec<PathBuf>,
    mode: StartMode,
    /// The person picked the mode, so what the core suggests no longer moves it.
    picked: bool,
    /// The way the core suggests for what is typed, and why when it is more than doing it.
    suggested: Option<(StartMode, Option<Because>)>,
    chosen: Chosen,
    /// What was started from the box, oldest first, which the arrows bring back.
    sent: Vec<String>,
    recalled: Option<(usize, String)>,
    _triage: Task<()>,
}

impl Default for Start {
    fn default() -> Self {
        Self {
            draft: None,
            mic: None,
            files: Vec::new(),
            mode: StartMode::default(),
            picked: false,
            suggested: None,
            chosen: Chosen::default(),
            sent: Vec::new(),
            recalled: None,
            _triage: Task::ready(()),
        }
    }
}

impl Start {
    /// Back to empty after a start, keeping the draft, the mic, the agent picked and what was sent.
    fn cleared(&mut self) {
        *self = Start {
            draft: self.draft.take(),
            mic: self.mic.take(),
            chosen: std::mem::take(&mut self.chosen),
            sent: std::mem::take(&mut self.sent),
            ..Start::default()
        };
    }
}

impl GoalsPane {
    /// Makes the box's draft and mic, which need the window.
    pub(super) fn start_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.start.draft.is_some() {
            return;
        }
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 12)
                .submit_on_enter(true)
        });
        draft.update(cx, |draft, cx| {
            draft.set_placeholder("What should get done?", window, cx)
        });
        let typing = cx.subscribe_in(&draft, window, |pane, _, event, window, cx| match event {
            InputEvent::Change => pane.typed(cx),
            InputEvent::PressEnter { shift: false, .. } => pane.start_work(window, cx),
            _ => {}
        });
        self.start.draft = Some((draft, typing));
        self.start.mic = self.bench.upgrade().map(|bench| {
            let voice = bench.read(cx).voice.clone();
            let toggle = crate::chat::dictate(START_DRAFT, bench.downgrade());
            cx.new(|cx| MicButton::new(voice, START_DRAFT.to_owned(), toggle, cx))
        });
    }

    /// The start box's draft, when `key` names it.
    pub fn start_draft(&self, key: &str) -> Option<Entity<TextareaState>> {
        let (draft, _) = self.start.draft.as_ref().filter(|_| key == START_DRAFT)?;
        Some(draft.clone())
    }

    fn typed_text(&self, cx: &App) -> String {
        self.start
            .draft
            .as_ref()
            .map_or(String::new(), |(draft, _)| {
                draft.read(cx).value().trim().to_owned()
            })
    }

    /// The project work starts in: the one in front of the workbench.
    fn start_project(&self, cx: &App) -> Option<ProjectInfo> {
        let bench = self.bench.upgrade()?;
        let project = bench.read(cx).project().cloned();
        project
    }

    /// The agent work starts with: the one picked, else the one the rail starts with.
    pub(super) fn start_launcher(&self, cx: &App) -> Option<LauncherInfo> {
        let bench = self.bench.upgrade()?;
        let bench = bench.read(cx);
        let picked = (self.start.chosen.launcher.as_deref())
            .and_then(|id| bench.launchers().iter().find(|known| known.id == id));
        picked.or_else(|| bench.launcher()).cloned()
    }

    /// Asks the core to suggest a way once typing settles.
    fn typed(&mut self, cx: &mut Context<Self>) {
        let text = self.typed_text(cx);
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

    fn pick(&mut self, mode: StartMode, cx: &mut Context<Self>) {
        self.start.mode = mode;
        self.start.picked = true;
        cx.notify();
    }

    /// Brings an older or newer start back into the draft, as a chat's arrows do.
    fn recall(&mut self, older: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((draft, _)) = self.start.draft.as_ref() else {
            return;
        };
        let draft = draft.clone();
        let typed = draft.read(cx).value().to_string();
        let start = &mut self.start;
        match crate::chat::step_prompts(&start.sent, &mut start.recalled, older, &typed) {
            Some(text) => draft.update(cx, |draft, cx| draft.set_value(text, window, cx)),
            None => cx.propagate(),
        }
    }

    fn add_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |pane, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = pane.update(cx, |pane, cx| pane.attach(paths, cx));
            }
        })
        .detach();
    }

    fn attach(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let paths = paths.iter().map(|path| crate::chat::lasting(path));
        self.start.files.extend(paths);
        cx.notify();
    }

    /// Takes pictures and files pasted into the draft as files going with it.
    fn pasted(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        let mut taken = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    let image = image.clone();
                    let kept = cx.background_spawn(async move { kept_picture(image) });
                    cx.spawn(async move |pane, cx| {
                        if let Some(path) = kept.await {
                            let _ = pane.update(cx, |pane, cx| pane.attach(vec![path], cx));
                        }
                    })
                    .detach();
                    taken = true;
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    self.attach(paths.paths().to_vec(), cx);
                    taken = true;
                }
                ClipboardEntry::String(_) => {}
            }
        }
        taken
    }

    /// Starts the agent picked to take the work, adds the card that tracks it, held by that
    /// agent, then sends the agent what was written, saying which card is theirs and what to do.
    fn start_work(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.typed_text(cx);
        if text.is_empty() || self.busy {
            return;
        }
        let (Some(launcher), Some(project)) = (self.start_launcher(cx), self.start_project(cx))
        else {
            self.said(
                "No agent is set up to start; set one up from the agents rail",
                cx,
            );
            return;
        };
        let (started, agent) = oneshot::channel();
        let folder = project.path.to_string_lossy().into_owned();
        let here = Target::here(&self.asks);
        here.start_chat(launcher.id, project.id.clone(), None, Some(started));
        let (title, details) = titled(&text);
        let (mode, files) = (self.start.mode, std::mem::take(&mut self.start.files));
        let Chosen { model, payer, .. } = self.start.chosen.clone();
        // An agent whose card could not be added would sit in an empty tab.
        let close = {
            let here = here.clone();
            move |agent: &str| here.call("closeAgents", json!({ "agentIds": [agent] }))
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
                let patch = json!({ "model": model, "modelProvider": payer });
                let changed = here
                    .request("updateAgent", json!({ "agentId": agent, "patch": patch }))
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
            let added = here
                .request_typed::<call::BoardAdd>(BoardAddParams {
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
                .await;
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
                if let Some((draft, _)) = &pane.start.draft {
                    draft.update(cx, |draft, cx| draft.set_value("", window, cx));
                }
                pane.start.sent.push(text.clone());
                pane.start.cleared();
                let _ = pane.bench.update(cx, |bench, _| {
                    bench.call(
                        "openTab",
                        json!({ "project": project.id, "kind": "agent", "target": agent }),
                    );
                    bench.ask_chat(ChatAsk::Prompt {
                        agent: agent.clone(),
                        text: first_message(mode, card, &text),
                        paths: (files.iter())
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect(),
                        context: Vec::new(),
                        attach: false,
                    });
                });
                cx.notify();
            });
        })
        .detach();
    }
}

/// A pasted picture, written to the temporary folder so it can go with the message as a file.
fn kept_picture(image: gpui_kit::Image) -> Option<PathBuf> {
    let image = crate::pasted::readable(image);
    let stamp = chrono::Local::now().format("pasted-%Y%m%d-%H%M%S%3f");
    let folder = std::env::temp_dir().join("sikemux-drops");
    std::fs::create_dir_all(&folder).ok()?;
    let path = folder.join(format!("{stamp}.{}", image.format.extension()));
    std::fs::write(&path, &image.bytes).ok()?;
    Some(path)
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
        StartMode::HandOff => format!(
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
        StartMode::Chat => format!(
            "{text}\n\nSikemux tracks this as goal card #{card} on the project's board, which \
             you hold."
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

/// The agent menu of the composer, picking which agent, account, provider and model start.
fn agent_menu(
    pane: &GoalsPane,
    shown: &LauncherInfo,
    bench: &Entity<Workbench>,
    cx: &mut Context<GoalsPane>,
) -> Picked {
    let launchers = bench.read(cx).launchers().to_vec();
    let mut model = shown.model.clone();
    if let (Some(model), Some(chosen)) = (model.as_mut(), &pane.start.chosen.model) {
        model.current = chosen.clone();
    }
    let this = cx.weak_entity();
    let choose = move |cx: &mut App, change: &dyn Fn(&mut Chosen)| {
        let _ = this.update(cx, |pane, cx| {
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
        model_provider: pane.start.chosen.payer.clone(),
        locked: false,
        pending: None,
        set,
        moving,
        target: bench.read(cx).target(),
    }
}

pub(super) fn start_box(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let (Some((draft, _)), Some(bench)) = (&pane.start.draft, pane.bench.upgrade()) else {
        return div().into_any_element();
    };
    let launcher = pane.start_launcher(cx);
    let provider = launcher.as_ref().map_or("", |launcher| &launcher.provider);
    let label = launcher.as_ref().map_or_else(
        || "No agent".to_owned(),
        |launcher| bench.read(cx).agent_name(&launcher.provider).to_owned(),
    );
    let this = cx.weak_entity();
    let act = |this: &WeakEntity<GoalsPane>,
               act: fn(&mut GoalsPane, &mut Window, &mut Context<GoalsPane>)|
     -> Act {
        let this = this.clone();
        Rc::new(move |window, cx| {
            let _ = this.update(cx, |pane, cx| act(pane, window, cx));
        })
    };
    let on_send = act(&this, GoalsPane::start_work);
    let on_add = act(&this, GoalsPane::add_files);
    let on_paste = {
        let this = this.clone();
        Rc::new(move |item: &ClipboardItem, _: &mut Window, cx: &mut App| {
            this.update(cx, |pane, cx| pane.pasted(item, cx))
                .unwrap_or(false)
        })
    };
    let remove = {
        let this = this.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            let _ = this.update(cx, |pane, cx| {
                if index < pane.start.files.len() {
                    pane.start.files.remove(index);
                }
                cx.notify();
            });
        }
    };
    let open = {
        let bench = bench.downgrade();
        move |path: PathBuf, window: &mut Window, cx: &mut App| {
            let _ = bench.update(cx, |bench, cx| bench.show_picture(path, window, cx));
        }
    };
    let tiles = crate::chat::tiles(
        &pane.start.files,
        &[],
        remove,
        |_, _, _| {},
        open,
        Some(&pane.asks),
        cx,
    )
    .map(IntoElement::into_any_element);
    let strip = pane.start_project(cx).map(|project| {
        let folder = project.path.to_string_lossy().into_owned();
        Strip {
            host: None,
            project: project.name.clone().into(),
            path: crate::ui::path::tilde(&folder, crate::ui::path::this_home().as_deref()).into(),
            branch: (project.git.as_ref()).map(|git| git.branch.clone().into()),
            moves: None,
            worktree: None,
            pull: None,
            account: None,
        }
    });
    let chosen = MODES
        .iter()
        .position(|(mode, ..)| *mode == pane.start.mode)
        .unwrap_or(0);
    let words = MODES
        .iter()
        .map(|(_, name, _)| (SharedString::from(*name), None))
        .collect();
    let tips = MODES.iter().map(|(.., tip)| SharedString::from(*tip));
    let mut modes = Segmented::words("start-mode", words, chosen)
        .tooltips(tips)
        .on_select(cx.listener(|pane, chosen: &usize, _, cx| {
            if let Some((mode, ..)) = MODES.get(*chosen) {
                pane.pick(*mode, cx);
            }
        }));
    if let Some((mode, because)) = &pane.start.suggested {
        let at = MODES.iter().position(|(each, ..)| each == mode);
        let why = because.as_ref().map(because_words);
        if let (Some(at), Some(why)) = (at, why) {
            modes = modes.suggested(at, why);
        }
    }
    let mut composer = Composer::new(draft, provider, label, on_send, Rc::new(|_, _| {}))
        .strip(strip)
        .attachments(
            tiles,
            on_add,
            on_paste,
            crate::chat::chip(bench.downgrade()),
        )
        .aside(modes)
        .notice(
            pane.busy
                .then(|| Notice::Waiting("Starting the agent…".into())),
        );
    if let Some(launcher) = &launcher {
        composer = composer.model_menu(agent_menu(pane, launcher, &bench, cx), bench.clone());
    }
    if let Some(mic) = &pane.start.mic {
        let heard = bench.read(cx).voice.read(cx).heard_for(START_DRAFT);
        composer = composer.dictation(mic.clone(), heard);
    }
    div()
        .key_context("ChatComposer")
        .on_action(cx.listener(|pane, _: &OlderPrompt, window, cx| pane.recall(true, window, cx)))
        .on_action(cx.listener(|pane, _: &NewerPrompt, window, cx| pane.recall(false, window, cx)))
        .child(composer)
        .into_any_element()
}
