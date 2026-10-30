//! The chat's composer: what is being written and sent, its keys, the strip docked on it,
//! and the prompts and steering it sends.

use super::*;
use sikemux_core::server::{call, QueuedMessage, QueuedParams};

/// A card over the composer the agent waits on the person for.
#[derive(Clone)]
pub(super) enum Waiting {
    Questions(Vec<Question>),
    Answered(Vec<Question>),
    Permission(Box<PermissionAsk>),
}

/// The draft, the masked field a secret answer is typed into, and what the draft last said while empty.
pub struct Composing {
    pub draft: Entity<TextareaState>,
    /// Where the composer is drawn, apart from the chat around it.
    pub view: Entity<ComposerView>,
    pub secret: Entity<InputState>,
    pub hint: SharedString,
    /// The files going out with the next message, its pictures named in the draft by number.
    pub attachments: Vec<PathBuf>,
    /// The agents, files or commands offered for the word at the caret.
    pub menu: Option<mention::Menu>,
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
    pub synced: Value,
    /// The core's revision of the draft this one has taken in, which a save names so the core
    /// puts back what it added since.
    pub read_rev: u64,
    /// A draft typed in another app, with this draft as it was when that came, and its revision.
    pub arrived: Option<(Sent, Value, u64)>,
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
                crate::settings::set(bench, "voiceDictation", true);
                window.push_notification(
                    "Dictation is on. The speech model downloads once, about 600 MB, then click the microphone again.",
                    cx,
                );
            }
        })
        .ok();
    })
}

/// Brings an older or newer sent prompt into the draft; the arrow goes on to the text when
/// there is none to bring.
/// Backspace just after a chip selects it, as on the Mac, so only a second press removes it.
pub(super) fn select_chip(
    agent: &str,
    cx: &mut Context<Workbench>,
) -> impl Fn(&gpui_kit::component::input::Backspace, &mut Window, &mut App) {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |_, _, cx| {
        let draft = this
            .update(cx, |bench, _| {
                Some(
                    bench
                        .chats
                        .open
                        .get(&agent)?
                        .composer
                        .as_ref()?
                        .draft
                        .clone(),
                )
            })
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
            (chat.running() && empty).then(|| bench.stop(&agent, window, cx))
        });
        if !matches!(stopped, Ok(Some(()))) {
            cx.propagate();
        }
    }
}

/// The composer, with what this chat lets the person change.
/// The composer of one chat as a view of its own, mounted isolated, so a key typed in it draws
/// only it again rather than the chat around it.
pub struct ComposerView {
    pub bench: WeakEntity<Workbench>,
    /// The chat's agent as the stage last drew it.
    pub agent: Option<AgentRow>,
    /// Whether the stage docked questions, subagents, tasks or queued messages on the composer.
    pub covered: bool,
}

impl gpui::Render for ComposerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(agent) = self.agent.clone() else {
            return div().into_any_element();
        };
        let covered = self.covered;
        self.bench
            .update(cx, |bench, cx| {
                let chat = bench.chats.open.get(&agent.id)?;
                mounted_composer(bench, &agent, chat, covered, cx)
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
    covered: bool,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let id = &agent.id;
    composer(bench, agent, chat, cx)
        .map(|(composer, offering)| {
            let composer = composer.covered(covered);
            div()
                .key_context("ChatComposer")
                .capture_action(select_chip(id, cx))
                .on_action(recall::<OlderPrompt>(id, true, cx))
                .on_action(recall::<NewerPrompt>(id, false, cx))
                .on_action(stop_turn(id, cx))
                .on_action({
                    let (agent, this) = (id.clone(), cx.weak_entity());
                    move |_: &StashPrompt, _: &mut Window, cx: &mut App| {
                        this.update(cx, |bench, cx| bench.stash_prompt(&agent, cx))
                            .ok();
                    }
                })
                .child(
                    div()
                        .when(offering, |menu| {
                            menu.key_context("ComposerMenu")
                                .on_action(offered::<MenuUp>(
                                    id,
                                    |bench, agent, _, cx| bench.pick_choice(agent, -1, cx),
                                    cx,
                                ))
                                .on_action(offered::<MenuDown>(
                                    id,
                                    |bench, agent, _, cx| bench.pick_choice(agent, 1, cx),
                                    cx,
                                ))
                                .on_action(offered::<MenuPick>(
                                    id,
                                    |bench, agent, window, cx| {
                                        bench.take_choice(agent, None, window, cx)
                                    },
                                    cx,
                                ))
                                .on_action(offered::<MenuClose>(
                                    id,
                                    |bench, agent, _, cx| bench.dismiss_menu(agent, cx),
                                    cx,
                                ))
                        })
                        .child(composer),
                )
        })
        .map(IntoElement::into_any_element)
}

pub(super) fn composer(
    bench: &Workbench,
    agent: &AgentRow,
    chat: &Chat,
    cx: &mut Context<Workbench>,
) -> Option<(Composer, bool)> {
    let composing = chat.composer.as_ref()?;
    let shown = composing
        .menu
        .as_ref()
        .filter(|menu| menu.answers(composing.draft.read(cx)));
    let dock = shown.and_then(|menu| {
        let repo = (bench.project())
            .and_then(|project| project.code_host.as_ref())
            .map(|host| host.repo.clone());
        mention::dock(bench, &agent.id, menu, repo.as_deref(), cx)
    });
    let tray = shown
        .and_then(|menu| menu.tray())
        .map(|tray| crate::stash::tray(bench, &agent.id, &tray, cx));
    let tray_open = tray.is_some();
    let offering = dock.is_some() || tray_open;
    let head = chat.state.as_ref();
    let kind = bench.reads.catalog.kind(&agent.provider);
    let label = bench.agent_name(&agent.provider).to_owned();
    let reads_models = matches!(
        kind.and_then(|kind| kind.chat),
        Some(Driver::Claude | Driver::Codex)
    );
    let this = cx.weak_entity();
    let on_send = {
        let (this, agent) = (this.clone(), agent.id.clone());
        Rc::new(move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.send(&agent, false, window, cx))
                .ok();
        })
    };
    let on_stop = {
        let (this, agent) = (this.clone(), agent.id.clone());
        Rc::new(move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.stop(&agent, window, cx))
                .ok();
        })
    };
    let saved = bench.shown().and_then(|view| {
        view.workspace
            .agents
            .iter()
            .find(|saved| saved.id == agent.id)
    });
    let fresh = chat.is_empty(cx)
        && saved.is_some_and(|saved| saved.resume_id.is_none() && saved.worktree.is_none());
    let strip = saved.map(|saved| strip(bench, agent, saved, fresh, head, cx));
    let on_add = {
        let (this, agent) = (this.clone(), agent.id.clone());
        Rc::new(move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.add_files(&agent, window, cx))
                .ok();
        })
    };
    let remove = {
        let (this, agent) = (this.clone(), agent.id.clone());
        move |index: usize, window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.detach(&agent, index, window, cx))
                .ok();
        }
    };
    let forget = {
        let (this, agent) = (this.clone(), agent.id.clone());
        move |index: usize, _: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.forget(&agent, index, cx))
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
    let tiles = attach::tiles(
        &composing.attachments,
        &composing.context,
        remove,
        forget,
        open,
        Some(&bench.asks),
        cx,
    )
    .map(IntoElement::into_any_element);
    let mut composer = Composer::new(&composing.draft, &agent.provider, label, on_send, on_stop)
        .notice(
            installing::notice(bench, composing, &this)
                .or_else(|| {
                    composing
                        .refusal
                        .clone()
                        .map(|refusal| Notice::Refused(refusal, None))
                })
                .or_else(|| {
                    let id = composing.stashed.clone()?;
                    let (this, agent) = (this.clone(), agent.id.clone());
                    let undo: Act = Rc::new(move |_: &mut Window, cx: &mut App| {
                        this.update(cx, |bench, cx| bench.unstash_prompt(&agent, &id, cx))
                            .ok();
                    });
                    Some(Notice::Done("Stashed".into(), ("Undo".into(), undo)))
                }),
        )
        .text_scale(chat.scale)
        .stash(crate::stash::Stash::of(bench).fresh_count(), tray_open, {
            let (this, agent) = (this.clone(), agent.id.clone());
            Rc::new(move |window: &mut Window, cx: &mut App| {
                this.update(cx, |bench, cx| bench.toggle_stash_tray(&agent, window, cx))
                    .ok();
            })
        })
        .tray(tray)
        .attachments(
            tiles,
            on_add,
            Rc::new(attach::on_paste(&agent.id, cx)),
            attach::chip(cx.weak_entity()),
        )
        .strip(strip)
        .dictation(
            composing.mic.clone(),
            bench.voice.read(cx).heard_for(&agent.id),
        )
        .ready(head.is_some_and(|head| head.connection == ChatConnection::Ready))
        .running(chat.running())
        .steers(bench.preferences().mid_turn_send == MidTurnSend::Steer)
        .usage(head.and_then(|head| head.usage.clone()));
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
                this.update(cx, |bench, _| bench.move_chat(&agent, to)).ok();
            })
        };
        let started = head.is_some_and(|head| head.connection == ChatConnection::Ready);
        let picked = crate::chat::model_picker::Picked {
            provider: agent.provider.clone(),
            profile: saved.profile_id.clone(),
            model: model.clone(),
            model_provider: saved.model_provider.clone(),
            locked: !fresh,
            pending: match (started, reads_models) {
                (true, _) => None,
                (false, true) => Some("Reading the models…"),
                (false, false) => Some("Models show once the agent starts"),
            },
            set: pick.clone(),
            moving,
            target: bench.chat_target(&agent.id),
        };
        composer = composer.model_menu(picked, cx.entity());
    }
    let effort = head.and_then(|head| head.effort.clone()).or_else(|| {
        reads_models.then(|| ConfigPicker {
            id: "effort".into(),
            current: saved
                .and_then(|saved| saved.effort.clone())
                .unwrap_or_else(|| "Effort".into()),
            choices: Vec::new(),
        })
    });
    if model.is_some() || effort.is_some() {
        composer = composer.pickers(model, effort, pick);
    }
    if kind.is_some_and(|kind| kind.bypass) {
        let on = head.is_some_and(|head| head.permission_mode == PermissionMode::Bypass);
        let held = head.is_some_and(|head| head.changing_mode || !head.permissions.is_empty());
        let agent = agent.id.clone();
        composer = composer.yolo(
            on,
            held,
            Rc::new(move |_, cx| {
                this.update(cx, |bench, _| bench.toggle_yolo(&agent)).ok();
            }),
        );
    }
    Some((composer.menu(dock), offering))
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
        crate::worktree_switch::toggle(format!("worktree-{}", agent.id), &switch, flip, cx)
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
        let chat = crate::account_picker::Chat {
            agent: &agent.id,
            provider: &agent.provider,
            on: &on,
            started: !fresh,
        };
        let running = head.is_some_and(|head| head.running);
        // A chat whose agent has not started yet, such as one waiting for the person, starts
        // on the account picked.
        let unstarted = agent.status == AgentStatus::Starting
            && head.is_none_or(|head| head.connection == ChatConnection::Connecting);
        let ready = unstarted || head.is_some_and(|head| head.connection == ChatConnection::Ready);
        let this = cx.entity();
        crate::account_picker::picker(bench, chat, running, !ready, &this, cx)
    });
    Strip {
        host,
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
            (
                agent.id.clone(),
                saved.cwd.to_string_lossy().into_owned(),
                cx.entity(),
            )
        }),
        worktree,
        pull: crate::worktree_head::badges(bench, &agent.id, cx),
        account,
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

    /// One step through the sent prompts, as a shell's arrows: up from an empty draft, and either
    /// way while the draft still shows a prompt they brought back. Stepping past the latest gives
    /// back what was typed. None leaves the arrow to the draft's text.
    pub fn recall(&mut self, older: bool, draft: &str, cx: &App) -> Option<String> {
        let prompts = self.prompts(cx);
        let showing = self
            .recalled
            .as_ref()
            .is_some_and(|(at, _)| prompts.get(*at).map(String::as_str) == Some(draft));
        if !showing {
            self.recalled = None;
            if !older || !draft.trim().is_empty() {
                return None;
            }
        }
        let (at, typed) = self
            .recalled
            .take()
            .unwrap_or((prompts.len(), draft.to_owned()));
        if older {
            let at = at.checked_sub(1)?;
            self.recalled = Some((at, typed));
            return Some(prompts[at].clone());
        }
        match prompts.get(at + 1) {
            Some(prompt) => {
                self.recalled = Some((at + 1, typed));
                Some(prompt.clone())
            }
            None => Some(typed),
        }
    }

    /// What the empty draft says, as the chat stands: what typing answers, or where words go.
    /// `steer_first` is the person's choice that sending mid-turn steers rather than queues.
    pub fn hint(&self, agent: &str, questions: &[Question], steer_first: bool) -> String {
        let open: Vec<&Question> = questions
            .iter()
            .filter(|question| question.asks(agent))
            .collect();
        if let Some(current) = self
            .current(&open)
            .filter(|question| !question.secret && !self.is_folded(&open))
        {
            let ix = open
                .iter()
                .position(|question| question.id == current.id)
                .unwrap_or(0);
            return format!(
                "Or answer “{}” in your own words",
                current.title(ix, open.len())
            );
        }
        let Some(head) = &self.state else {
            return "Ask about this project".into();
        };
        match head.connection {
            ChatConnection::Error | ChatConnection::Stopped => {
                "Reconnect to continue this conversation".into()
            }
            ChatConnection::Ready if head.running && steer_first && self.steers() => {
                "Send to steer this turn".into()
            }
            ChatConnection::Ready if head.running => "Send to queue behind the running turn".into(),
            ChatConnection::Ready if head.commands.is_empty() => "Ask about this project".into(),
            ChatConnection::Ready => "Ask about this project, or type / for commands".into(),
            _ => "Ask about this project, or type / for commands".into(),
        }
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
    ) -> Option<Ask> {
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
            let answer = answer_text(&text, &paths, &context);
            return self.pick(
                &id,
                Pick {
                    answer: Answer::Text(answer.clone()),
                    shown: answer,
                },
                open,
            );
        }
        if self.steers() && !queue {
            let message = QueuedMessage {
                text,
                paths,
                context,
            };
            return Some(self.steer_queued(agent, Some(message)));
        }
        self.awaits_agent = !self.ready();
        Some(Ask::Chat(ChatAsk::Prompt {
            agent: agent.to_owned(),
            text,
            paths,
            context,
            attach: self.mark.is_none(),
        }))
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
