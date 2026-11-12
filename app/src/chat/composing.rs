//! The chat's composer: what is being written and sent, its keys, the strip docked on it,
//! and the prompts and steering it sends.

use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use sikemux_view::prefs::PreferenceChange;
use std::cell::Cell;
use std::collections::HashMap;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AppContext, Context, Focusable, InteractiveElement, IntoElement, ParentElement, SharedString,
    Task, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::chat::calls::QueuedParams;
use sikemux_core::server::chat::queue::QueuedMessageParams as QueuedMessage;
use sikemux_view::prefs::MidTurnSend;
use sikemux_wire::protocol::PermissionMode;

use super::*;
use crate::chat::{Answering, Chat, Question};
use crate::core::ChatAsk;
use crate::ui::list_picker::{Choose, Close, Next, Previous};
use sikemux_core::server::workspace::{AgentPatch, UpdateAgentParams};

/// Puts `text` at the end of the draft after a blank line, keeping what it holds, chips
/// and all, with the caret after it.
fn append(draft: &Entity<TextareaState>, text: &str, window: &mut Window, cx: &mut App) {
    draft.update(cx, |draft, cx| {
        let held = draft.value();
        let kept = held.trim_end().len();
        let joined = if kept == 0 {
            text.to_owned()
        } else {
            format!("\n\n{text}")
        };
        draft.set_selected_range(kept..held.len(), cx);
        draft.replace(joined, window, cx);
    });
}

impl Workbench {
    /// The composer of the chat of agent `owner`.
    pub fn composing(&self, owner: &str) -> Option<&Composing> {
        self.chats.open.get(owner)?.composer.as_ref()
    }

    pub fn composing_mut(&mut self, owner: &str) -> Option<&mut Composing> {
        self.chats.open.get_mut(owner)?.composer.as_mut()
    }

    /// Empties `owner`'s composer once what it held has gone, and keeps it so.
    pub fn emptied(&mut self, owner: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composing) = self.composing_mut(owner) else {
            return;
        };
        composing.attachments.clear();
        composing.context.clear();
        composing.tracked_context.clear();
        composing.menu = None;
        let draft = composing.draft.clone();
        draft.update(cx, |draft, cx| draft.set_value("", window, cx));
        self.keep_draft(owner, cx);
    }

    /// Every composer made, with what names it.
    pub fn composers(&self) -> impl Iterator<Item = (&str, &Composing)> {
        (self.chats.open.iter())
            .filter_map(|(agent, chat)| Some((agent.as_str(), chat.composer.as_ref()?)))
    }
}

/// A card over the composer the agent waits on the person for.
#[derive(Clone)]
pub(super) enum Waiting {
    Questions(Vec<Question>),
    Answered(Vec<Question>),
    Permission(Box<PermissionAsk>),
    Approval(Box<sikemux_view::inbox::InboxItem>),
}

/// The draft, the field a secret answer is typed into, and what the draft last said while empty.
pub struct Composing {
    pub draft: Entity<TextareaState>,
    /// Where the composer is drawn, apart from the chat around it.
    pub view: Entity<ComposerView>,
    pub secret: Entity<TextareaState>,
    pub secret_focused: bool,
    pub hint: SharedString,
    /// What the agent predicts the person sends next, while the empty draft offers it.
    pub suggested: Option<SharedString>,
    /// The suggestion the person turned away with Escape, which is not offered again.
    pub dismissed: Option<SharedString>,
    /// The files going out with the next message, its pictures named in the draft by number.
    pub attachments: Vec<PathBuf>,
    /// The file attached last and when, whose tile flashes as it comes in.
    pub fresh: Option<(PathBuf, std::time::Instant)>,
    /// The agents, files or commands offered for the word at the caret.
    pub menu: Option<mention::Menu>,
    /// Whether the model picker is open, so `/model` can open it.
    pub model_open: bool,
    pub mic: Entity<crate::voice::MicButton>,
    /// What the message carries besides its files: earlier chats it names, and issues and
    /// pages put with it, such as a code host's issue to work on.
    pub context: Vec<ChatContext>,
    /// The code host's open issues and pull requests, read the first time `#` or `!` is typed.
    pub tracked: Option<mention::Tracked>,
    /// What each `#12` or `!34` in the draft tells the agent, read once it was picked.
    pub tracked_context: HashMap<String, ChatContext>,
    /// Why the core turned down the last message, until the next is sent.
    pub refusal: Option<SharedString>,
    /// The stash item the draft was just stashed as, while it can still be put back.
    pub stashed: Option<String>,
    pub stage: ComposeStage,
    /// The draft as the core last kept it.
    pub synced: super::draft::Kept,
    /// The core's revision of the draft this one has taken in, which a save names so the core
    /// puts back what it added since.
    pub read_rev: u64,
    /// A draft typed in another app, with this draft as it was when that came, and its revision.
    pub arrived: Option<(Sent, super::draft::Kept, u64)>,
    pub saving: super::draft::Saving,
    pub _keeping: Task<()>,
    pub _heard: [Subscription; 2],
}

/// Where the message being sent stands.
#[derive(Default)]
pub enum ComposeStage {
    #[default]
    Writing,
    /// The core is starting the chat in a worktree, at `step`; the message goes once the chat
    /// is ready there when `sends`.
    Worktree { step: String, sends: bool },
    /// The message waits for `agent` to install, or its install failed under it.
    Installing { agent: String, state: Installing },
    /// The message as it was written, chips, files and all, until the core takes it.
    Sent(Sent),
    /// A turned-down message, going back into the draft when it next draws.
    Returned(Sent),
}

impl ComposeStage {
    /// Whether the message waits on the core or an install.
    pub fn waits(&self) -> bool {
        !matches!(self, ComposeStage::Writing | ComposeStage::Returned(_))
    }
}

/// A message as the composer held it when it was sent.
pub struct Sent {
    pub content: gpui_kit::base::input::InputContent,
    pub attachments: Vec<PathBuf>,
    pub context: Vec<ChatContext>,
    pub tracked_context: HashMap<String, ChatContext>,
}

/// The composer's mic: it listens and then types what was said, and the first click turns
/// dictation on.
pub fn dictate(agent: &str, this: WeakEntity<Workbench>) -> crate::voice::Act {
    let agent = agent.to_owned();
    Rc::new(move |window, cx| {
        this.update(cx, |bench, cx| {
            let off = bench.voice.update(cx, |voice, cx| voice.toggle(&agent, cx));
            if off {
                crate::settings::set(bench, PreferenceChange::VoiceDictation(true));
                window.push_notification(
                    t!("Dictation is on. The speech model downloads once, about 600 MB, then click the microphone again."),
                    cx,
                );
            }
        })
        .ok();
    })
}

/// Backspace just after a chip selects it, as on the Mac, so only a second press removes it.
pub(super) fn select_chip(
    agent: &str,
    cx: &mut Context<Workbench>,
) -> impl Fn(&gpui_kit::component::input::Backspace, &mut Window, &mut App) {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |_, _, cx| {
        let draft = this
            .update(cx, |bench, _| Some(bench.composing(&agent)?.draft.clone()))
            .ok()
            .flatten();
        let Some(draft) = draft else {
            return;
        };
        let chip = {
            let state = draft.read(cx);
            let cursor = state.cursor();
            state
                .selected_range()
                .is_empty()
                .then(|| {
                    state
                        .tokens()
                        .iter()
                        .map(|token| token.range())
                        .find(|range| range.end == cursor)
                })
                .flatten()
        };
        if let Some(range) = chip {
            draft.update(cx, |state, cx| state.set_selected_range(range, cx));
            cx.stop_propagation();
        }
    }
}

pub(super) fn recall<A: gpui::Action>(
    agent: &str,
    older: bool,
    cx: &mut Context<Workbench>,
) -> impl Fn(&A, &mut Window, &mut App) + use<A> {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |_: &A, window: &mut Window, cx: &mut App| {
        let brought = this.update(cx, |bench, cx| {
            let chat = bench.chats.open.get_mut(&agent)?;
            let draft = chat.composer.as_ref()?.draft.clone();
            let typed = draft.read(cx).value().to_string();
            let prompt = chat.recall(older, &typed, cx)?;
            draft.update(cx, |draft, cx| draft.set_value(prompt, window, cx));
            bench.typed_draft(&agent, cx);
            Some(())
        });
        if !matches!(brought, Ok(Some(()))) {
            cx.propagate();
        }
    }
}

/// Puts the suggestion the empty draft offers into it, to change or send.
pub(super) fn take_suggestion(
    agent: &str,
    cx: &mut Context<Workbench>,
) -> impl Fn(&TakeSuggestion, &mut Window, &mut App) + use<> {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |_: &TakeSuggestion, window: &mut Window, cx: &mut App| {
        let taken = this.update(cx, |bench, cx| {
            let composing = bench.composing_mut(&agent)?;
            let draft = composing.draft.clone();
            if !draft.read(cx).value().is_empty() {
                return None;
            }
            let suggestion = composing.suggested.take()?;
            draft.update(cx, |draft, cx| draft.set_value(suggestion, window, cx));
            bench.typed_draft(&agent, cx);
            Some(())
        });
        if !matches!(taken, Ok(Some(()))) {
            cx.propagate();
        }
    }
}

/// Hands a key's action to the menu over the draft of `agent`'s chat.
pub(super) fn offered<A: gpui::Action>(
    agent: &str,
    act: fn(&mut Workbench, &str, &mut Window, &mut Context<Workbench>),
    cx: &mut Context<Workbench>,
) -> impl Fn(&A, &mut Window, &mut App) + use<A> {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |_: &A, window: &mut Window, cx: &mut App| {
        this.update(cx, |bench, cx| act(bench, &agent, window, cx))
            .ok();
    }
}

/// Stops the running turn from an empty draft, so a half-written message never stops the agent.
pub(super) fn stop_turn(
    agent: &str,
    cx: &mut Context<Workbench>,
) -> impl Fn(&StopTurn, &mut Window, &mut App) + use<> {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |_: &StopTurn, window: &mut Window, cx: &mut App| {
        let stopped = this.update(cx, |bench, cx| {
            let chat = bench.chats.open.get(&agent)?;
            let empty = chat.composer.as_ref()?.draft.read(cx).value().is_empty();
            if !empty {
                return None;
            }
            if chat.running() {
                bench.stop(&agent, window, cx);
                return Some(());
            }
            let composing = bench.composing_mut(&agent)?;
            composing.dismissed = Some(composing.suggested.take()?);
            cx.notify();
            Some(())
        });
        if !matches!(stopped, Ok(Some(()))) {
            cx.propagate();
        }
    }
}

/// The composer of one chat as a view of its own, mounted isolated, so a key typed in it draws
/// only it again rather than the chat around it.
pub struct ComposerView {
    pub bench: WeakEntity<Workbench>,
    /// The composer it draws, as [`Workbench::composing`] names it.
    pub owner: String,
    /// The chat's agent as the stage last drew it.
    pub agent: Option<AgentRow>,
}

impl gpui::Render for ComposerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(agent) = self.agent.clone() else {
            return div().into_any_element();
        };
        self.bench
            .update(cx, |bench, cx| {
                let chat = bench.chats.open.get(&agent.id)?;
                mounted_composer(bench, &agent, chat, cx)
            })
            .ok()
            .flatten()
            .unwrap_or_else(|| div().into_any_element())
    }
}

/// The composer with the keys it takes: the arrows through sent prompts, Escape to stop the
/// turn, and the menu's keys while it offers something.
pub(super) fn mounted_composer(
    bench: &Workbench,
    agent: &AgentRow,
    chat: &Chat,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let (composer, offering) = composer(bench, agent, chat, cx)?;
    Some(mounted(&agent.id, composer, offering, cx).into_any_element())
}

/// `owner`'s composer with the keys every composer takes: Backspace onto a chip, the arrows
/// through sent prompts, Escape to stop the turn, ⌘S to stash, and the menu's keys while it
/// offers something. Keys a composer has no use for go on to what holds it.
pub fn mounted(
    owner: &str,
    composer: Composer,
    offering: bool,
    cx: &mut Context<Workbench>,
) -> gpui_kit::Div {
    div()
        .key_context("ChatComposer")
        .capture_action(select_chip(owner, cx))
        .on_action(recall::<OlderPrompt>(owner, true, cx))
        .on_action(recall::<NewerPrompt>(owner, false, cx))
        .on_action(stop_turn(owner, cx))
        .on_action(take_suggestion(owner, cx))
        .on_action({
            let (owner, this) = (owner.to_owned(), cx.weak_entity());
            move |_: &StashPrompt, _: &mut Window, cx: &mut App| {
                this.update(cx, |bench, cx| bench.stash_prompt(&owner, cx))
                    .ok();
            }
        })
        .child(
            div()
                .when(offering, |menu| {
                    menu.key_context("ComposerMenu")
                        .on_action(offered::<Previous>(
                            owner,
                            |bench, owner, _, cx| bench.pick_choice(owner, -1, cx),
                            cx,
                        ))
                        .on_action(offered::<Next>(
                            owner,
                            |bench, owner, _, cx| bench.pick_choice(owner, 1, cx),
                            cx,
                        ))
                        .on_action(offered::<Choose>(
                            owner,
                            |bench, owner, window, cx| bench.take_choice(owner, None, window, cx),
                            cx,
                        ))
                        .on_action(offered::<Close>(
                            owner,
                            |bench, owner, _, cx| bench.dismiss_menu(owner, cx),
                            cx,
                        ))
                })
                .child(composer),
        )
}

/// Why the core turned the last message down, or that it was just stashed, with Undo.
fn sent_notice(composing: &Composing, owner: &str, this: &WeakEntity<Workbench>) -> Option<Notice> {
    composing
        .refusal
        .clone()
        .map(|refusal| Notice::Refused(refusal, None))
        .or_else(|| {
            let id = composing.stashed.clone()?;
            let (this, owner) = (this.clone(), owner.to_owned());
            let undo: Act = Rc::new(move |_: &mut Window, cx: &mut App| {
                this.update(cx, |bench, cx| bench.unstash_prompt(&owner, &id, cx))
                    .ok();
            });
            Some(Notice::Done(
                t!("Stashed").into(),
                (t!("Undo").into(), undo),
            ))
        })
}

/// The chat's one-line notice, which its pane draws above the dock and the box: the agent
/// being installed, else what became of the last message.
pub(super) fn notice(
    bench: &Workbench,
    agent: &str,
    cx: &mut Context<Workbench>,
) -> Option<Notice> {
    let composing = bench.composing(agent)?;
    let this = cx.weak_entity();
    installing::notice(bench, composing, &this).or_else(|| sent_notice(composing, agent, &this))
}

/// What every composer offers, whatever it sends to: the files and chips going with the
/// message, the menu over the word at the caret, dictation, and why the core turned the last
/// message down or that it was just stashed; and whether the menu is open over it.
pub fn drafting(
    bench: &Workbench,
    owner: &str,
    provider: &str,
    label: String,
    (on_send, on_stop): (Act, Act),
    cx: &mut Context<Workbench>,
) -> Option<(Composer, bool)> {
    let composing = bench.composing(owner)?;
    let shown = composing
        .menu
        .as_ref()
        .filter(|menu| menu.answers(composing.draft.read(cx)));
    let dock = shown.and_then(|menu| {
        let repo = (bench.project())
            .and_then(|project| project.code_host.as_ref())
            .map(|host| host.repo.clone());
        mention::dock(bench, owner, menu, repo.as_deref(), cx)
    });
    let offering = dock.is_some();
    let this = cx.weak_entity();
    let on_add = {
        let (this, owner) = (this.clone(), owner.to_owned());
        Rc::new(move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.add_files(&owner, window, cx))
                .ok();
        })
    };
    let remove = {
        let (this, owner) = (this.clone(), owner.to_owned());
        move |index: usize, window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.detach(&owner, index, window, cx))
                .ok();
        }
    };
    let forget = {
        let (this, owner) = (this.clone(), owner.to_owned());
        move |index: usize, _: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.forget(&owner, index, cx))
                .ok();
        }
    };
    let open = {
        let this = this.clone();
        move |path: PathBuf, window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.show_picture(path, window, cx))
                .ok();
        }
    };
    let fresh = (composing.fresh.as_ref())
        .filter(|(_, at)| at.elapsed() < attach::FRESH)
        .map(|(path, _)| path.as_path());
    let tiles = attach::tiles(
        (&composing.attachments, fresh),
        &composing.context,
        remove,
        forget,
        open,
        Some(&bench.asks),
        cx,
    )
    .map(IntoElement::into_any_element);
    let notice = sent_notice(composing, owner, &this);
    let composer = Composer::new(&composing.draft, provider, label, on_send, on_stop)
        .notice(notice)
        .attachments(
            tiles,
            on_add,
            Rc::new(attach::on_paste(owner, cx)),
            attach::chip(cx.weak_entity()),
        )
        .when(cfg!(target_os = "macos"), |composer| {
            composer.snapshot(snapshot(owner, &this))
        })
        .dictation(composing.mic.clone(), bench.voice.read(cx).heard_for(owner))
        .suggestion(composing.suggested.clone())
        .menu(dock);
    Some((composer, offering))
}

/// Takes a SnapShot into `owner`'s draft.
fn snapshot(owner: &str, this: &WeakEntity<Workbench>) -> Act {
    let (this, owner) = (this.clone(), owner.to_owned());
    Rc::new(move |window: &mut Window, cx: &mut App| {
        crate::keymap::counted(crate::snapshot::COMMAND, cx);
        this.update(cx, |bench, cx| {
            bench.snapshot(Some(owner.clone()), false, window, cx)
        })
        .ok();
    })
}

/// The Stash on `owner`'s bar and its tray over the draft while it is open, and whether it is.
pub fn with_stash(
    bench: &Workbench,
    owner: &str,
    composer: Composer,
    cx: &mut Context<Workbench>,
) -> (Composer, bool) {
    let tray = bench.composing(owner).and_then(|composing| {
        let menu = (composing.menu.as_ref()).filter(|menu| menu.answers(composing.draft.read(cx)));
        menu.and_then(|menu| menu.tray())
    });
    let tray = tray.map(|tray| crate::stash::tray(bench, owner, &tray, cx));
    let open = tray.is_some();
    let toggle: Act = {
        let (this, owner) = (cx.weak_entity(), owner.to_owned());
        Rc::new(move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.toggle_stash_tray(&owner, window, cx))
                .ok();
        })
    };
    let fresh = crate::stash::Stash::of(bench).fresh_count();
    (composer.stash(fresh, open, toggle).tray(tray), open)
}

pub(super) fn composer(
    bench: &Workbench,
    agent: &AgentRow,
    chat: &Chat,
    cx: &mut Context<Workbench>,
) -> Option<(Composer, bool)> {
    chat.composer.as_ref()?;
    let head = chat.state.as_ref();
    let kind = bench.reads.catalog.kind(&agent.provider);
    let label = bench.agent_name(&agent.provider).to_owned();
    let reads_models = matches!(
        kind.and_then(|kind| kind.chat),
        Some(ChatDriver::Claude | ChatDriver::Codex)
    );
    let this = cx.weak_entity();
    let on_send: Act = {
        let (this, agent) = (this.clone(), agent.id.clone());
        Rc::new(move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.send(&agent, false, window, cx))
                .ok();
        })
    };
    let on_stop: Act = {
        let (this, agent) = (this.clone(), agent.id.clone());
        Rc::new(move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.stop(&agent, window, cx))
                .ok();
        })
    };
    let (composer, offering) = drafting(
        bench,
        &agent.id,
        &agent.provider,
        label,
        (on_send, on_stop),
        cx,
    )?;
    let (mut composer, tray_open) = with_stash(bench, &agent.id, composer, cx);
    let saved = bench.shown().and_then(|view| {
        view.workspace
            .agents
            .iter()
            .find(|saved| saved.id == agent.id)
    });
    let fresh = chat.is_empty(cx)
        && saved.is_some_and(|saved| saved.resume_id.is_none() && saved.worktree.is_none());
    let strip = saved.map(|saved| strip(bench, agent, saved, fresh, head, cx));
    composer = composer.notice(None);
    composer = composer
        .text_scale(chat.scale)
        .strip(strip)
        .ready(head.is_some_and(|head| head.connection == ChatConnection::Ready))
        .running(chat.running())
        .steers(bench.preferences().mid_turn_send == MidTurnSend::Steer)
        .usage(
            head.and_then(|head| head.usage.clone()),
            head.and_then(|head| head.compactions.clone()),
        )
        .expired(
            head.and_then(|head| head.expired_cache.clone())
                .map(|expired| {
                    let (this, agent) = (this.clone(), agent.id.clone());
                    let summarise: crate::chat::composer::Summarise =
                        Rc::new(move |on: bool, _: &mut Window, cx: &mut App| {
                            this.update(cx, |bench, _| bench.summarise_first(&agent, on))
                                .ok();
                        });
                    (expired, summarise)
                }),
        );
    let pick: crate::chat::model_picker::Set = {
        let (this, agent) = (this.clone(), agent.id.clone());
        Rc::new(
            move |config: &str, value: &str, _: &mut Window, cx: &mut App| {
                let ask = ChatAsk::Config {
                    agent: agent.clone(),
                    config: config.to_owned(),
                    value: value.to_owned(),
                };
                this.update(cx, |bench, _| bench.ask_chat(ask)).ok();
            },
        )
    };
    let model = head.and_then(|head| head.model.clone());
    if let Some(saved) = saved.filter(|_| fresh || model.is_some()) {
        let moving: crate::chat::model_picker::Move = {
            let (this, agent) = (this.clone(), agent.id.clone());
            Rc::new(move |to: crate::chat::model_picker::Moving, _, cx| {
                this.update(cx, |bench, cx| match fresh {
                    true => bench.move_chat(&agent, to),
                    false => bench.hand_off(&agent, to, false, cx),
                })
                .ok();
            })
        };
        let started = head.is_some_and(|head| head.connection == ChatConnection::Ready);
        let picked = crate::chat::model_picker::Picked {
            provider: agent.provider.clone(),
            profile: saved.profile_id.clone(),
            model: model.clone(),
            speed: head.and_then(|head| head.speed.clone()).filter(|_| started),
            model_provider: saved.model_provider.clone(),
            locked: !fresh,
            pending: match (started, reads_models) {
                (true, _) => None,
                (false, true) => Some(t!("Reading the models…")),
                (false, false) => Some(t!("Models show once the agent starts")),
            },
            set: pick.clone(),
            moving,
            target: bench.chat_target(&agent.id),
        };
        let open = chat
            .composer
            .as_ref()
            .is_some_and(|composing| composing.model_open);
        let opened: crate::chat::composer::Opened = {
            let (this, agent) = (this.clone(), agent.id.clone());
            Rc::new(move |open: bool, cx: &mut App| {
                this.update(cx, |bench, cx| {
                    if let Some(composing) = bench.composing_mut(&agent) {
                        composing.model_open = open;
                        cx.notify();
                    }
                })
                .ok();
            })
        };
        composer = composer
            .model_menu(picked, cx.entity())
            .model_open(open, opened);
    }
    let effort = head.and_then(|head| head.effort.clone()).or_else(|| {
        reads_models.then(|| ConfigPicker {
            id: "effort".into(),
            current: saved
                .and_then(|saved| saved.effort.clone())
                .unwrap_or_else(|| t!("Effort").into()),
            choices: Vec::new(),
        })
    });
    if model.is_some() || effort.is_some() {
        composer = composer.pickers(model, effort, pick);
    }
    if kind.is_some_and(|kind| kind.bypass) {
        let mode = head.map_or(PermissionMode::WorkspaceWrite, |head| head.permission_mode);
        let held = head.is_some_and(|head| head.changing_mode || !head.permissions.is_empty());
        let agent = agent.id.clone();
        let offered = [
            Some(PermissionMode::WorkspaceWrite),
            kind.is_some_and(|kind| kind.auto)
                .then_some(PermissionMode::Auto),
            Some(PermissionMode::Bypass),
        ];
        composer = composer.modes(super::composer::Modes {
            current: mode,
            offered: offered.into_iter().flatten().collect(),
            held,
            pick: Rc::new(move |chosen, _, cx| {
                this.update(cx, |bench, _| bench.switch_mode(&agent, chosen))
                    .ok();
            }),
        });
    }
    Some((composer, offering || tray_open))
}

/// Where the chat runs, for the strip docked on its composer: its project, which names a
/// worktree agent's project rather than its worktree's folder, the branch or worktree it works
/// on, and the account it runs on.
pub(super) fn strip(
    bench: &Workbench,
    agent: &AgentRow,
    saved: &WorkspaceAgent,
    fresh: bool,
    head: Option<&ChatHead>,
    cx: &mut Context<Workbench>,
) -> Strip {
    let project = bench
        .projects()
        .iter()
        .find(|project| project.id == saved.project);
    let folder = project.map_or(saved.cwd.as_path(), |project| project.path.as_path());
    let switch = bench.worktree_switch(&agent.id, cx);
    let elsewhere = switch
        .as_ref()
        .is_some_and(|(switch, _)| !matches!(switch, Switch::Choosing { .. }));
    let worktree = switch.map(|(switch, flip)| {
        crate::worktrees::switch::toggle(format!("worktree-{}", agent.id), &switch, flip, cx)
    });
    let on = saved.profile_id.clone().or_else(|| {
        bench
            .accounts
            .current(&agent.provider)
            .map(|profile| profile.id.clone())
    });
    let place = bench.chat_target(&agent.id).place().clone();
    let home = bench.home_at(&place);
    let host = match &place {
        crate::hosts::Place::Account(host) => Some(bench.host_name(host)),
        crate::hosts::Place::This => None,
    };
    // The accounts listed are this host's, and moving a chat to one is this host's to do.
    let here = host.is_none();
    let account = on.filter(|_| here).and_then(|on| {
        let started = !fresh;
        let pick: crate::account_picker::Pick = {
            let (this, agent) = (cx.entity(), agent.id.clone());
            Rc::new(move |chosen, cx| {
                this.update(cx, |bench, cx| match started {
                    true => crate::accounts::move_chat(bench, &agent, chosen, cx),
                    false => bench.start_as(
                        &agent,
                        AgentPatch {
                            profile_id: Some(Some(chosen.id.clone())),
                            ..AgentPatch::default()
                        },
                        &chosen.name,
                    ),
                })
            })
        };
        let chat = crate::account_picker::Chat {
            key: &agent.id,
            provider: &agent.provider,
            on: &on,
            started: started.then_some(agent.id.as_str()),
            pick,
        };
        let running = head.is_some_and(|head| head.running);
        // A chat whose agent has not started yet, such as one waiting for the person, starts
        // on the account picked.
        let unstarted = agent.status == AgentStatus::Starting
            && head.is_none_or(|head| head.connection == ChatConnection::Connecting);
        let ready = unstarted || head.is_some_and(|head| head.connection == ChatConnection::Ready);
        crate::account_picker::picker(bench, chat, running, !ready, cx)
    });
    let any_other = (bench.host_choices().iter())
        .any(|option| matches!(option.choice, crate::hosts::HostChoice::Account(_)));
    let hosts = (fresh && any_other && saved.worktree.is_none() && !saved.terminal).then(|| {
        let here = crate::copies::HostCopy {
            place: place.clone(),
            project: saved.project.clone(),
            host: host.clone().unwrap_or_else(|| t!("This host").into()),
            path: folder.to_path_buf(),
            git: project.and_then(|project| project.git.clone()),
            away: false,
        };
        let (agent, this) = (agent.id.clone(), cx.entity());
        Box::new(move |trigger: gpui_kit::base::Button| {
            crate::pickers::host_picker::around(trigger, &agent, here, &this)
        }) as super::composer::ChipMenu
    });
    Strip {
        host,
        hosts,
        project: folder
            .file_name()
            .map_or_else(|| folder.to_string_lossy(), |name| name.to_string_lossy())
            .into_owned()
            .into(),
        path: crate::ui::path::tilde(&folder.to_string_lossy(), home.as_deref()).into(),
        branch: project
            .filter(|_| !elsewhere)
            .and_then(|project| Some(project.git.as_ref()?.branch.clone().into())),
        moves: fresh.then(|| {
            let (agent, cwd, this) = (
                agent.id.clone(),
                saved.cwd.to_string_lossy().into_owned(),
                cx.entity(),
            );
            let place = bench.chat_target(&agent).place().clone();
            Box::new(move |trigger: gpui_kit::base::Button| {
                let place = place.clone();
                crate::pickers::project_switcher::around(trigger, &agent, &cwd, place, &this)
            }) as super::composer::ChipMenu
        }),
        worktree,
        pull: crate::worktrees::head::badges(bench, &agent.id, cx),
        account,
    }
}

/// One step through `prompts`, oldest first, as a shell's arrows: up from an empty draft, and
/// either way while the draft still shows a prompt they brought back, which `recalled` keeps with
/// what was typed. Stepping past the latest gives back what was typed. None leaves the arrow to
/// the draft's text.
pub fn step_prompts(
    prompts: &[String],
    recalled: &mut Option<(usize, String)>,
    older: bool,
    draft: &str,
) -> Option<String> {
    let showing = recalled
        .as_ref()
        .is_some_and(|(at, _)| prompts.get(*at).map(String::as_str) == Some(draft));
    if !showing {
        *recalled = None;
        if !older || !draft.trim().is_empty() {
            return None;
        }
    }
    let (at, typed) = recalled.take().unwrap_or((prompts.len(), draft.to_owned()));
    if older {
        let at = at.checked_sub(1)?;
        *recalled = Some((at, typed));
        return Some(prompts[at].clone());
    }
    match prompts.get(at + 1) {
        Some(prompt) => {
            *recalled = Some((at + 1, typed));
            Some(prompt.clone())
        }
        None => Some(typed),
    }
}

impl Chat {
    /// The choice the arrows are on: where they moved to, else the one picked, else the first.
    pub(super) fn cursor_at(&self, question: &Question) -> usize {
        match (
            &self.cursor,
            self.picks.get(&question.id).map(|pick| &pick.answer),
        ) {
            (Some((id, at)), _) if *id == question.id => *at,
            (_, Some(Answer::Choice(choice))) => *choice,
            _ => 0,
        }
    }

    /// What the person sent in this chat, oldest first, then what waits behind the turn, with
    /// a prompt sent again back to back kept once.
    pub(super) fn prompts(&self, cx: &App) -> Vec<String> {
        let rows = self.transcript.read(cx).rows();
        let sent = rows
            .iter()
            .map(|row| &row.message)
            .filter(|message| message.role == ChatRole::User && message.letters.is_empty());
        let texts = sent.map(|message| {
            message
                .parts
                .iter()
                .filter_map(text_of)
                .map(|(text, _)| text)
                .collect::<String>()
        });
        let queued = self
            .state
            .iter()
            .flat_map(|head| head.queued.iter().map(|queued| queued.text.clone()));
        let mut prompts: Vec<String> = Vec::new();
        for text in texts.chain(queued) {
            let text = text.trim();
            if !text.is_empty() && prompts.last().map(String::as_str) != Some(text) {
                prompts.push(text.to_owned());
            }
        }
        prompts
    }

    /// One step through the prompts sent in this chat.
    pub fn recall(&mut self, older: bool, draft: &str, cx: &App) -> Option<String> {
        step_prompts(&self.prompts(cx), &mut self.recalled, older, draft)
    }

    /// What the empty draft says, as the chat stands: what typing answers, or where words go.
    /// `steer_first` is the person's choice that sending mid-turn steers rather than queues;
    /// `to` is who the words go to, said in place of asking about the project, and whether the
    /// chat's commands are offered beside it.
    pub fn hint<'a>(
        &self,
        agent: &str,
        questions: impl Iterator<Item = &'a Question>,
        steer_first: bool,
        to: Option<(&str, bool)>,
    ) -> String {
        let open: Vec<&Question> = questions.filter(|question| question.asks(agent)).collect();
        if let Some(current) = self
            .current(&open)
            .filter(|question| !question.secret && !self.is_folded(&open))
        {
            let ix = open
                .iter()
                .position(|question| question.id == current.id)
                .unwrap_or(0);
            return t!(
                "Or answer “{question}” in your own words",
                question = current.title(ix, open.len())
            );
        }
        let ask = |commands: bool| match (to, commands) {
            (Some((to, true)), true) => t!("{to}, or type / for commands", to = to),
            (Some((to, _)), _) => to.to_owned(),
            (None, true) => t!("Ask about this project, or type / for commands").to_owned(),
            (None, false) => t!("Ask about this project").to_owned(),
        };
        let Some(head) = &self.state else {
            return ask(false);
        };
        match head.connection {
            ChatConnection::Error | ChatConnection::Stopped => {
                t!("Reconnect to continue this conversation").into()
            }
            ChatConnection::Ready if head.running && steer_first && self.steers() => {
                t!("Send to steer this turn").into()
            }
            ChatConnection::Ready if head.running => {
                t!("Send to queue behind the running turn").into()
            }
            ChatConnection::Ready if head.commands.is_empty() => ask(false),
            _ => ask(true),
        }
    }

    /// What the agent predicts the person sends next, while nothing else waits on them: the
    /// session ready, no turn running, and no permission or question open.
    pub fn suggestion<'a>(
        &self,
        agent: &str,
        mut questions: impl Iterator<Item = &'a Question>,
    ) -> Option<&str> {
        let head = self.state.as_ref()?;
        let free = head.connection == ChatConnection::Ready
            && !head.running
            && head.permissions.is_empty()
            && !questions.any(|question| question.asks(agent));
        head.suggestion.as_deref().filter(|_| free)
    }

    /// The command `text` starts with when this session lists commands and not that one. Only a
    /// first word like `/name` counts, so a path such as `/usr/bin` goes to the agent as text.
    pub fn unknown_command<'a>(&self, text: &'a str) -> Option<&'a str> {
        let commands = &self.state.as_ref()?.commands;
        let word = text.strip_prefix('/')?;
        let name = &word[..word.find(char::is_whitespace).unwrap_or(word.len())];
        let named = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'));
        (named && !commands.is_empty() && !commands.iter().any(|command| command.name == name))
            .then_some(name)
    }

    /// What the composer's text asks of the core: the answer to the question it answers, or
    /// words for the agent, which steer a running turn unless queued behind it.
    pub fn sent(
        &mut self,
        agent: &str,
        text: String,
        (paths, context): (Vec<String>, Vec<ChatContext>),
        queue: bool,
        open: &[&Question],
    ) -> Option<Answering> {
        if let Some(composing) = self.composer.as_mut() {
            composing.refusal = None;
            composing.attachments.clear();
            composing.context.clear();
            composing.tracked_context.clear();
        }
        self.recalled = None;
        let answering = self
            .current(open)
            .filter(|question| !question.secret && !self.is_folded(open));
        if let Some(id) = answering.map(|question| question.id.clone()) {
            let shown = match text.is_empty() {
                true => paths.join(", "),
                false => text.clone(),
            };
            return self
                .pick(
                    &id,
                    Pick {
                        answer: Answer::Text {
                            text,
                            paths,
                            context,
                        },
                        shown,
                    },
                    open,
                )
                .map(Answering::Answers);
        }
        if self.steers() && !queue {
            let message = QueuedMessage {
                text,
                paths,
                context,
            };
            return Some(Answering::Ask(self.steer_queued(agent, Some(message))));
        }
        self.awaits_agent = !self.ready();
        Some(Answering::Ask(Ask::Chat(ChatAsk::Prompt {
            agent: agent.to_owned(),
            text,
            paths,
            context,
            attach: self.mark.is_none(),
        })))
    }

    /// A turn is running that takes words mid-turn.
    pub(super) fn steers(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|head| head.running && head.capabilities.steering)
    }

    /// With ⌘ and nothing written, the messages waiting behind the turn go into it now.
    pub fn steer_waiting(&self, agent: &str) -> Option<Ask> {
        let waiting = self
            .state
            .as_ref()
            .is_some_and(|head| !head.queued.is_empty());
        (self.steers() && waiting).then(|| self.steer_queued(agent, None))
    }

    /// Puts the messages waiting behind the running turn into it, and `message` after them.
    pub(super) fn steer_queued(&self, agent: &str, message: Option<QueuedMessage>) -> Ask {
        let ids = self.state.as_ref().map_or(Vec::new(), |head| {
            head.queued.iter().map(|queued| queued.id.clone()).collect()
        });
        crate::core::typed_call::<call::SteerQueued>(QueuedParams {
            agent_id: agent.to_owned(),
            ids,
            message,
        })
    }
}

impl Workbench {
    /// Makes the composer of the chat in front, which needs the window.
    pub(crate) fn compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        if !(self.chats.open.get(&agent)).is_some_and(|chat| chat.composer.is_none()) {
            return;
        }
        let mut composing = self.new_composing(&agent, window, cx);
        let Some(chat) = self.chats.open.get_mut(&agent) else {
            return;
        };
        let drafts = match crate::chat::draft::is_elsewhere(chat) {
            true => self.feeds.front_mut().and_then(|feed| feed.drafts.as_ref()),
            false => self.feeds.this.drafts.as_ref(),
        };
        if let Some(drafts) = drafts {
            composing.start_from(drafts, &agent, window, cx);
        }
        chat.composer = Some(composing);
        self.watch_drafts();
        if let Some(text) = self.chats.deliveries.remove(&agent) {
            self.deliver(&agent, text, window, cx);
        }
        if let Some((paths, notes)) = self.chats.held_files.remove(&agent) {
            self.attach_there(&agent, paths, notes, window, cx);
        }
    }

    /// A composer for the chat of agent `owner`, sending what it holds on Return and keeping it
    /// as the person types.
    pub(crate) fn new_composing(
        &mut self,
        owner: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Composing {
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 12)
                .submit_on_enter(true)
        });
        let secret = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder(t!("Type or paste the answer"))
        });
        let sending = {
            let owner = owner.to_owned();
            let was_empty = Cell::new(true);
            cx.subscribe_in(
                &draft,
                window,
                move |bench, draft, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter {
                        shift: false,
                        secondary,
                    } => bench.send(&owner, *secondary, window, cx),
                    InputEvent::Change => {
                        let empty = draft.read(cx).value().is_empty();
                        if was_empty.replace(empty) != empty {
                            if !empty {
                                bench.began_writing(&owner);
                            }
                            cx.notify();
                        }
                        if let Some(composing) = bench.composing_mut(&owner) {
                            composing.drop_untagged(window, cx);
                        }
                        bench.offer(&owner, cx);
                        bench.typed_draft(&owner, cx);
                    }
                    _ => {}
                },
            )
        };
        let keeping = {
            let owner = owner.to_owned();
            cx.subscribe_in(
                &secret,
                window,
                move |bench, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => {
                        bench.keep_secret(&owner, window, cx);
                    }
                    InputEvent::Change => cx.notify(),
                    InputEvent::Focus | InputEvent::Blur => {
                        if let Some(composing) = bench.composing_mut(&owner) {
                            composing.secret_focused = matches!(event, InputEvent::Focus);
                        }
                        cx.notify();
                    }
                    _ => {}
                },
            )
        };
        let mic = {
            let (voice, toggle) = (
                self.voice.clone(),
                crate::chat::dictate(owner, cx.weak_entity()),
            );
            let owner = owner.to_owned();
            cx.new(|cx| crate::voice::MicButton::new(voice, owner, toggle, cx))
        };
        let bench = cx.weak_entity();
        let view = cx.new(|_| crate::chat::ComposerView {
            bench,
            owner: owner.to_owned(),
            agent: None,
        });
        Composing {
            draft,
            view,
            secret,
            secret_focused: false,
            mic,
            hint: SharedString::default(),
            suggested: None,
            dismissed: None,
            attachments: Vec::new(),
            fresh: None,
            menu: None,
            model_open: false,
            context: Vec::new(),
            tracked: None,
            tracked_context: HashMap::new(),
            refusal: None,
            stashed: None,
            stage: ComposeStage::Writing,
            synced: Default::default(),
            read_rev: 0,
            arrived: None,
            saving: Default::default(),
            _keeping: Task::ready(()),
            _heard: [sending, keeping],
        }
    }

    /// Hands `agent` text from elsewhere in the app, such as a terminal's selection, and brings
    /// its tab to the front: typed into its CLI when it runs in a terminal, else at the end of
    /// its chat's draft after a blank line, with the draft focused.
    pub fn deliver(
        &mut self,
        agent: &str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.this_view().and_then(|view| {
            let saved = view
                .workspace
                .agents
                .iter()
                .find(|saved| saved.id == agent)?;
            Some(saved.project.clone())
        });
        if let Some(project) = project {
            self.open_here(OpenTab::new(At::project(&project), PaneKind::Agent, agent));
        }
        if let Some((place, session)) = self.agent_terminal(agent) {
            let key = self.terminals.key(&place, session);
            crate::terminal::hand_to(self, key, text, cx);
            if let Some(focus) = self.terminals.focus(key) {
                window.focus(focus, cx);
            }
            return;
        }
        let Some(draft) = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| Some(chat.composer.as_ref()?.draft.clone()))
        else {
            self.chats
                .deliveries
                .entry(agent.to_owned())
                .and_modify(|held| *held = format!("{held}\n\n{text}"))
                .or_insert(text);
            return;
        };
        append(&draft, &text, window, cx);
        draft.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Stops the turn running in `agent`'s chat. The messages waiting behind it come back
    /// to the draft rather than going out after it.
    pub fn stop(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.open.get(agent) else {
            return;
        };
        let waiting = chat.waiting().join("\n\n");
        if let Some(composing) = chat.composer.as_ref().filter(|_| !waiting.is_empty()) {
            append(&composing.draft, &waiting, window, cx);
        }
        self.ask_chat(ChatAsk::Cancel {
            agent: agent.to_owned(),
        });
        cx.notify();
    }

    /// Keeps what the empty draft of the chat in front says in step with how the chat stands.
    pub(crate) fn hint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        self.send_after_worktree(&agent, window, cx);
        let Some(chat) = self.chats.open.get_mut(&agent) else {
            return;
        };
        let mut returned = false;
        if let Some(composing) = chat.composer.as_mut() {
            let empty = composing.draft.read(cx).value().trim().is_empty();
            let returned_held = match std::mem::take(&mut composing.stage) {
                ComposeStage::Returned(held) => Some(held),
                stage => {
                    composing.stage = stage;
                    None
                }
            };
            if let Some(held) = returned_held.filter(|_| empty) {
                composing
                    .draft
                    .update(cx, |draft, cx| draft.set_value(held.content, window, cx));
                composing.attachments = held.attachments;
                composing.context = held.context;
                composing.tracked_context = held.tracked_context;
                returned = true;
            }
            composing.take_arrived(window, cx);
        }
        if returned {
            self.keep_draft(&agent, cx);
        }
        self.focus_draft(&agent, window, cx);
        let steer_first = self.preferences().mid_turn_send == MidTurnSend::Steer;
        let chief_hears = (self.chats.open.get(&agent))
            .and_then(|chat| chat.state.as_ref())
            .is_some_and(|head| head.chief_hears.is_some());
        let to = self.agent().and_then(|row| match row.chief {
            true => Some((t!("Tell @Chief what you want").to_owned(), true)),
            false => chief_hears
                .then_some(row.name)
                .flatten()
                .map(|name| (t!("Send to @{name} · @Chief is told", name = name), false)),
        });
        let call_waits = (self.chats.open.get(&agent))
            .and_then(|chat| self.waiting_call(chat.target.place(), &agent, cx))
            .is_some();
        let Some(chat) = self.chats.open.get_mut(&agent) else {
            return;
        };
        let suggested = (!call_waits)
            .then(|| chat.suggestion(&agent, self.feeds.questions_outside_calls()))
            .flatten()
            .map(SharedString::new);
        let hint = SharedString::from(match call_waits {
            true => t!("Or answer in your own words").to_owned(),
            false => chat.hint(
                &agent,
                self.feeds.questions_outside_calls(),
                steer_first,
                to.as_ref().map(|(to, commands)| (to.as_str(), *commands)),
            ),
        });
        let Some(composing) = chat.composer.as_mut() else {
            return;
        };
        let suggested = suggested.filter(|suggested| {
            composing.attachments.is_empty()
                && composing.context.is_empty()
                && composing.dismissed.as_ref() != Some(suggested)
        });
        let hint = suggested.clone().unwrap_or(hint);
        composing.suggested = suggested;
        if composing.hint == hint {
            return;
        }
        composing.hint = hint.clone();
        composing
            .draft
            .update(cx, |draft, cx| draft.set_placeholder(hint, window, cx));
    }

    /// Puts the keys in the draft of the chat in front when it comes to the front and again
    /// once its session is ready, unless the person is typing somewhere other than a draft or
    /// the window itself.
    pub(crate) fn focus_draft(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.open.get(agent) else {
            return;
        };
        let shown = (agent.to_owned(), chat.ready());
        if self.chats.focused.as_ref() == Some(&shown) {
            return;
        }
        let Some(draft) = chat
            .composer
            .as_ref()
            .map(|composing| composing.draft.clone())
        else {
            return;
        };
        self.chats.focused = Some(shown);
        let held = window.focused(cx);
        let free = held.as_ref().is_none_or(|held| {
            *held == self.keys.focus
                || self.chats.open.values().any(|chat| {
                    chat.composer
                        .as_ref()
                        .is_some_and(|composing| composing.draft.read(cx).focus_handle(cx) == *held)
                })
        });
        if free {
            draft.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// Sends what the person wrote: the answer to a question it answers, else words for
    /// the agent, which join a running turn or wait behind it as the person prefers,
    /// and ⌘ does the other. ⌘ with nothing written sends the questions' answers, else
    /// steers in the messages waiting.
    pub fn send(
        &mut self,
        agent: &str,
        primary: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let waits = self
            .voice
            .update(cx, |voice, cx| voice.send_after_words(agent, primary, cx));
        if waits {
            return;
        }
        let preparing = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| chat.composer.as_ref())
            .is_some_and(|composing| matches!(composing.stage, ComposeStage::Worktree { .. }));
        if preparing || self.hold_for_install(agent, cx) {
            return;
        }
        if self.starts_in_worktree(agent, cx) {
            let text = self
                .chats
                .open
                .get(agent)
                .and_then(|chat| Some(chat.composer.as_ref()?.draft.read(cx).value().to_string()))
                .unwrap_or_default();
            return self.begin_worktree(agent, text, cx);
        }
        let typed = (self.chats.open.get(agent))
            .and_then(|chat| chat.composer.as_ref())
            .map(|composing| (composing.draft.clone(), composing.outgoing(cx)));
        if let Some((draft, outgoing)) = typed {
            let text = draft.read(cx).value().trim().to_owned();
            if !text.is_empty() && self.answer_in_chat(agent, &text, outgoing, cx) {
                let composing =
                    (self.chats.open.get_mut(agent)).and_then(|chat| chat.composer.as_mut());
                if let Some(composing) = composing {
                    composing.refusal = None;
                    composing.attachments.clear();
                    composing.context.clear();
                    composing.tracked_context.clear();
                }
                draft.update(cx, |draft, cx| draft.set_value("", window, cx));
                self.keep_draft(agent, cx);
                return cx.notify();
            }
        }
        let Some(composing) = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| chat.composer.as_ref())
        else {
            return;
        };
        let draft = composing.draft.clone();
        let outgoing = composing.outgoing(cx);
        let text = draft.read(cx).value().trim().to_owned();
        if text.is_empty() && outgoing.0.is_empty() {
            let place = (self.chats.open.get(agent)).map(|chat| chat.target.place().clone());
            let keyed = place.and_then(|place| self.keyed_call(&place, agent, cx));
            if let Some(id) = keyed.filter(|_| primary) {
                // A production call is never sent by a key; only its button sends it.
                if !self.ships_to_production(&id) {
                    self.go_with(&id, cx);
                }
                return;
            }
            if primary {
                self.answer(
                    agent,
                    |chat, open| {
                        chat.send_answers(open)
                            .map(Into::into)
                            .or_else(|| chat.steer_waiting(agent).map(Into::into))
                    },
                    cx,
                );
                cx.notify();
            }
            return;
        }
        let unknown = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| chat.unknown_command(&text))
            .map(|name| t!("/{name} is not available in this session", name = name));
        if let Some(refused) = unknown {
            if let Some(composing) = self
                .chats
                .open
                .get_mut(agent)
                .and_then(|chat| chat.composer.as_mut())
            {
                composing.refusal = Some(refused.into());
            }
            return cx.notify();
        }
        let sent = Sent {
            content: draft.read(cx).content(),
            attachments: composing.attachments.clone(),
            context: composing.context.clone(),
            tracked_context: composing.tracked_context.clone(),
        };
        let steers = self.preferences().mid_turn_send == MidTurnSend::Steer;
        self.answer(
            agent,
            |chat, open| {
                if let Some(composing) = chat.composer.as_mut() {
                    composing.stage = ComposeStage::Sent(sent);
                }
                chat.sent(agent, text, outgoing, primary == steers, open)
            },
            cx,
        );
        draft.update(cx, |draft, cx| draft.set_value("", window, cx));
        self.keep_draft(agent, cx);
        cx.notify();
    }

    pub fn keep_secret(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self
            .chats
            .open
            .get(agent)
            .and_then(|chat| Some(chat.composer.as_ref()?.secret.clone()))
        else {
            return;
        };
        let text = field.read(cx).value().to_string();
        let sending = self
            .chats
            .open
            .get(agent)
            .is_some_and(Chat::sending_answers);
        if text.is_empty() || sending {
            return;
        }
        self.answer(
            agent,
            |chat, open| chat.keep_secret(text, open).map(Into::into),
            cx,
        );
        field.update(cx, |field, cx| field.set_value("", window, cx));
        cx.notify();
    }

    /// Changes the chat of `agent` with its open questions to hand, and asks the core what that asks.
    pub fn answer(
        &mut self,
        agent: &str,
        change: impl FnOnce(&mut Chat, &[&Question]) -> Option<Answering>,
        cx: &mut Context<Self>,
    ) {
        let open: Vec<&Question> = self
            .feeds
            .questions_outside_calls()
            .filter(|question| question.asks(agent))
            .collect();
        if let Some(ask) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| change(chat, &open))
        {
            self.ask_answering(agent, ask, cx);
        }
    }

    /// Lets go of the questions `keep` turns down, of this host and of the host in front.
    pub fn keep_questions(&mut self, keep: impl Fn(&Question) -> bool) {
        self.feeds.this.questions.retain(&keep);
        if let Some(front) = self.feeds.front_mut() {
            front.questions.retain(keep);
        }
    }

    /// Whether the agent asks before it acts, kept for its next session and changed in the one running.
    pub fn switch_mode(&self, agent: &str, mode: PermissionMode) {
        self.chat_target(agent)
            .call_typed::<call::UpdateAgent>(UpdateAgentParams::new(
                agent,
                AgentPatch {
                    permission_mode: Some(mode),
                    ..AgentPatch::default()
                },
            ));
        self.ask_chat(ChatAsk::Mode {
            agent: agent.to_owned(),
            mode,
        });
    }
}
