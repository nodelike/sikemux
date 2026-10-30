//! An agent's chat: the rows the core folds, kept in a transcript, and how
//! each row draws. Rows change only by the core's changes.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::f32::consts::FRAC_PI_2;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, Scrollbar};
use gpui_kit::component::input::{Input, InputState, TextareaState};
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, relative, AnyElement, App, AppContext, Context, Div, Entity, FontWeight,
    HighlightStyle, Hsla, InteractiveElement, IntoElement, KeyBinding, ParentElement, Radians,
    SharedString, StatefulInteractiveElement, StyleRefinement, Styled, StyledText, Subscription,
    Task, Transformation, WeakEntity, Window,
};
use gpui_transcript::{
    AwayChanged, Change, CodeColours, EmptyChanged, FoundChanged, Markdown, Pen, Row, RowsChanged,
    Style, Transcript,
};
use serde_json::{json, Value};
use sikemux_wire::catalog::Driver;
use sikemux_wire::chat::{
    ChatConnection, ChatHead, ChatMessage, ChatPage, ChatPart, ChatRole, ChatView, FiredRequest,
    LetterKind, MoveReason, NoticeSeverity, NoticeState, PartTree, PermissionAsk, SubagentCard,
    SubagentState, WorkFold,
};
use sikemux_wire::protocol::{
    AgentStatus, ChatAttachment, ChatContext, ChatMark, ConfigPicker, MidTurnSend, PermissionMode,
    WorkspaceAgent, DEFAULT_TEXT_SCALE, MAX_TEXT_SCALE, MIN_TEXT_SCALE,
};
use sikemux_wire::questions::{AgentQuestion, QuestionState};

use crate::agents::setup::Installing;
use crate::chat::composer::{Act, Composer, Notice, Strip};
use crate::core::{Ask, ChatAsk, ChatNews, Target};
use crate::icons::Icon;
use crate::keymap::{TextSizeDecrease, TextSizeIncrease, TextSizeReset};
use crate::motion;
use crate::picture::Picture;
use crate::syntax;
use crate::theme::{self, Tokens};
use crate::ui::shimmer::shimmer;
use crate::ui::{self, Button, IconButton, Variant};
use crate::workbench::{AgentRow, Workbench};
use crate::worktree_switch::Switch;
use cards::*;
use changes::Changed;
use composing::*;
use looks::*;
use questions::*;
use rows::*;
use tokio::sync::mpsc::UnboundedSender;

mod attach;
mod cards;
pub mod changes;
pub mod composer;
mod composing;
mod content;
pub(crate) mod draft;
mod edit;
mod files;
mod find;
mod gestures;
mod installing;
mod link;
pub(crate) mod live;
mod looks;
mod mention;
mod moving;
pub(crate) use attach::{named, named_agents, pill, site_icon, Mention};
pub use mention::Tray;
mod meta;
pub mod model_picker;
mod page;
mod questions;
mod rows;
mod schema;
mod status;
mod tools;

pub(crate) use installing::installs_moved;
mod worktree;

pub use attach::lasting;
pub use composing::{dictate, ComposeStage, ComposerView, Composing, Sent};
pub use draft::key as draft_key;
pub use questions::{question_keys, Answering, Question};

/// Turns asked for each time the reader nears the top of what is held.
const PAGE_TURNS: u32 = 20;
/// The turns a chat holds while it follows its latest rows; earlier ones are dropped, and
/// paged back when the reader scrolls up to them.
const HELD_TURNS: usize = 40;
/// How many turns past `HELD_TURNS` gather before any are dropped, so rows go in batches.
const SLACK_TURNS: usize = 20;
/// The rows a chat holds above and below the one at the top of its view while the reader is
/// scrolled up, and how many more gather on either side before any are dropped.
const HELD_AROUND: usize = 150;
const SLACK_ROWS: usize = 50;
/// The most rows a chat following its latest rows holds, for a turn of many tool calls.
const HELD_ROWS: usize = 400;
/// The rows a subagent's card holds, its latest, as the core sends them.
const SUBAGENT_ROWS: usize = 40;

/// A text longer than this many characters shows its start, with a way to show the rest.
const LONG_TEXT: usize = 20_000;

/// One message as the core folded it, and the tree of each text it holds.
pub struct ChatRow {
    message: ChatMessage,
    docs: Vec<Doc>,
}

/// A text part's markdown, and its text's length in UTF-16 units, which the core's patches count.
struct Doc {
    part: String,
    markdown: Markdown,
    units: usize,
}

impl Doc {
    fn new(part: &str, text: &str, md: Option<&PartTree>) -> Doc {
        let units = text.encode_utf16().count();
        let markdown = match md {
            Some(tree) => Markdown::new(&tree.blocks, units_of(tree.n)),
            None => Markdown::new(&paragraphs(text), units),
        };
        Doc {
            part: part.to_owned(),
            markdown,
            units,
        }
    }
}

fn units_of(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// Text the core sends without a tree, as the paragraphs it was written in.
fn paragraphs(text: &str) -> Value {
    let lines = |paragraph: &str| -> Vec<Value> {
        let mut nodes = Vec::new();
        for (ix, line) in paragraph.lines().enumerate() {
            if ix > 0 {
                nodes.push(json!({ "t": "br" }));
            }
            nodes.push(json!(line));
        }
        nodes
    };
    Value::Array(
        text.split("\n\n")
            .filter(|paragraph| !paragraph.trim().is_empty())
            .map(|paragraph| json!({ "t": "p", "c": lines(paragraph) }))
            .collect(),
    )
}

fn text_of(part: &ChatPart) -> Option<(&str, Option<&PartTree>)> {
    match part {
        ChatPart::Text { text, md, .. } | ChatPart::Thought { text, md, .. } => {
            Some((text, md.as_ref()))
        }
        _ => None,
    }
}

impl ChatRow {
    /// The chat event that opened the row, which pages are asked from.
    fn first_event(&self) -> Option<u64> {
        self.message.from_event
    }

    fn opens_turn(&self) -> bool {
        self.message.role == ChatRole::User
    }

    fn new(message: ChatMessage) -> ChatRow {
        let mut row = ChatRow {
            message,
            docs: Vec::new(),
        };
        row.read_docs();
        row
    }

    /// The markdown of each text part, and of each message another agent sent, which the row draws in place of its text.
    fn read_docs(&mut self) {
        let parts = self
            .message
            .parts
            .iter()
            .filter_map(|part| text_of(part).map(|(text, md)| Doc::new(part.id(), text, md)));
        let letters = self
            .message
            .letters
            .iter()
            .enumerate()
            .filter(|(_, letter)| letter.kind != LetterKind::Answer);
        let bodies = self
            .message
            .letters
            .iter()
            .enumerate()
            .filter_map(|(ix, letter)| {
                let request = letter.trigger.as_ref()?.request.as_ref()?;
                Some(Doc::new(
                    &request_doc(ix),
                    &request.body,
                    Some(request.body_tree.as_ref()?),
                ))
            });
        self.docs =
            parts
                .chain(letters.map(|(ix, letter)| {
                    Doc::new(&letter_doc(ix), &letter.text, letter.tree.as_ref())
                }))
                .chain(bodies)
                .collect();
    }

    fn doc(&self, part: &str) -> Option<&Doc> {
        self.docs.iter().find(|doc| doc.part == part)
    }

    fn put_part(&mut self, part: ChatPart) {
        if let Some((text, md)) = text_of(&part) {
            let doc = Doc::new(part.id(), text, md);
            match self.docs.iter_mut().find(|held| held.part == doc.part) {
                Some(held) => *held = doc,
                None => self.docs.push(doc),
            }
        }
        match self
            .message
            .parts
            .iter_mut()
            .find(|held| held.id() == part.id())
        {
            Some(held) => *held = part,
            None => self.message.parts.push(part),
        }
    }

    fn write(&mut self, part: &str, more: &str, md: Option<&Value>) {
        let Some(ChatPart::Text { text, .. } | ChatPart::Thought { text, .. }) =
            self.message.parts.iter_mut().find(|held| held.id() == part)
        else {
            return;
        };
        text.push_str(more);
        if let Some(doc) = self.docs.iter_mut().find(|doc| doc.part == part) {
            doc.units += more.encode_utf16().count();
            if let Some(md) = md {
                doc.markdown.write(md, doc.units);
            }
        }
    }

    fn set(&mut self, fields: changes::Fields<ChatMessage>, docs: bool) {
        for field in fields {
            field(&mut self.message);
        }
        // A part's own tree is the one it arrived with; its text's later patches live only
        // in its doc, so the docs are read again only when the parts themselves changed.
        if docs {
            self.read_docs();
        }
    }
}

impl Row for ChatRow {
    fn id(&self) -> &str {
        &self.message.id
    }

    fn docs(&self) -> Vec<&Markdown> {
        if !self.message.letters.is_empty() {
            return (0..self.message.letters.len())
                .filter_map(|ix| self.doc(&letter_doc(ix)))
                .map(|doc| &doc.markdown)
                .collect();
        }
        self.message
            .parts
            .iter()
            .filter_map(|part| self.doc(part.id()))
            .map(|doc| &doc.markdown)
            .collect()
    }

    /// Find reads the chat's prose, as on the Mac: not the agent's thinking, nor a message
    /// another agent sent.
    fn findable(&self, doc: usize) -> bool {
        self.message.letters.is_empty()
            && self
                .message
                .parts
                .iter()
                .filter(|part| self.doc(part.id()).is_some())
                .nth(doc)
                .is_some_and(|part| matches!(part, ChatPart::Text { .. }))
    }

    fn doc_name(&self, doc: usize) -> Option<&str> {
        self.doc_parts().nth(doc)
    }

    fn doc_named(&self, name: &str) -> Option<usize> {
        self.doc_parts().position(|part| part == name)
    }
}

impl ChatRow {
    /// The parts with a doc, in the order `docs` gives them; a message from another agent's
    /// letters are not parts, so it has none.
    fn doc_parts(&self) -> impl Iterator<Item = &str> {
        let parts = match self.message.letters.is_empty() {
            true => self.message.parts.as_slice(),
            false => &[],
        };
        parts
            .iter()
            .map(ChatPart::id)
            .filter(|part| self.doc(part).is_some())
    }
}

fn letter_doc(ix: usize) -> String {
    format!("letter-{ix}")
}

/// The doc of the body of the webhook request that fired letter `ix`'s trigger.
fn request_doc(ix: usize) -> String {
    format!("letter-{ix}-request")
}

actions!(
    questions,
    [
        PreviousChoice,
        NextChoice,
        PreviousQuestion,
        NextQuestion,
        Tick,
        Confirm
    ]
);
actions!(chat, [FindInChat]);
actions!(
    composer,
    [
        OlderPrompt,
        NewerPrompt,
        StopTurn,
        MenuUp,
        MenuDown,
        MenuPick,
        MenuClose,
        CancelEdit,
        StashPrompt
    ]
);

/// Picks the choice at this index of the question on show, from its number key.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(name = "questions.choose", no_json)]
pub struct Choose(pub usize);

/// Binds a transcript's keys, the composer's, and those the open question in front takes while
/// the draft is empty, whether the draft or something else in the chat holds the focus. The
/// question's come last, so they win over the composer's while it asks.
pub fn install(cx: &mut App) {
    gpui_transcript::init(cx);
    crate::ui::find_bar::install(cx);
    let find = if cfg!(target_os = "macos") {
        "cmd-f"
    } else {
        "ctrl-f"
    };
    cx.bind_keys([KeyBinding::new(find, FindInChat, Some("Chat"))]);
    let mut bindings = vec![
        KeyBinding::new("up", OlderPrompt, Some("ChatComposer > Input")),
        KeyBinding::new("down", NewerPrompt, Some("ChatComposer > Input")),
        KeyBinding::new("escape", StopTurn, Some("ChatComposer > Input")),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-s"
            } else {
                "ctrl-s"
            },
            StashPrompt,
            Some("ChatComposer"),
        ),
    ];
    for context in ["QuestionDock > Input", "QuestionDock"] {
        bindings.extend(
            ["1", "2", "3", "4", "5", "6", "7", "8", "9"]
                .into_iter()
                .enumerate()
                .map(|(choice, key)| KeyBinding::new(key, Choose(choice), Some(context))),
        );
        bindings.extend([
            KeyBinding::new("up", PreviousChoice, Some(context)),
            KeyBinding::new("down", NextChoice, Some(context)),
            KeyBinding::new("left", PreviousQuestion, Some(context)),
            KeyBinding::new("right", NextQuestion, Some(context)),
            KeyBinding::new("space", Tick, Some(context)),
            KeyBinding::new("enter", Confirm, Some(context)),
        ]);
    }
    bindings.extend([
        KeyBinding::new("up", MenuUp, Some("ComposerMenu > Input")),
        KeyBinding::new("down", MenuDown, Some("ComposerMenu > Input")),
        KeyBinding::new("tab", MenuPick, Some("ComposerMenu > Input")),
        KeyBinding::new("enter", MenuPick, Some("ComposerMenu > Input")),
        KeyBinding::new("escape", MenuClose, Some("ComposerMenu > Input")),
        KeyBinding::new("escape", CancelEdit, Some("MessageEditor > Input")),
    ]);
    cx.bind_keys(bindings);
}

/// What a key asks of the question on show.
#[derive(Clone, Copy, PartialEq)]
pub enum Press {
    Choose(usize),
    Up,
    Down,
    Left,
    Right,
    Tick,
    Pick,
}

/// What the rows of a chat are drawn with beside their own content: what the reader
/// opened, each subagent's rows, and whether a turn is running.
#[derive(Default)]
struct Reading {
    /// The finished turns and subagents the reader opened, by their first row or session.
    opened: HashSet<String>,
    tools: tools::Opened,
    /// Each subagent's rows, by its session.
    subagents: HashMap<String, Vec<ChatRow>>,
    /// A turn is running, so the last row's run of calls stays open.
    running: bool,
    /// The chat takes a sent message written again.
    editable: bool,
    editing: Option<edit::Editing>,
    /// The host no longer keeps the turns before the first row.
    earlier_lost: bool,
    /// Rows whose text streamed in while the chat was open, which never fold.
    streamed: HashSet<String>,
    /// Long texts the reader asked to see the rest of, by row and part.
    unfolded: HashSet<SharedString>,
    /// The find bar is looking for something, so every long text shows all of itself.
    finding: bool,
    /// Where the chat stands with a worktree, said above its first message.
    worktree: Option<crate::worktree_switch::Switch>,
}

impl Reading {
    fn folds(&self, row: &str, part: &SharedString) -> bool {
        !self.finding && !self.streamed.contains(row) && !self.unfolded.contains(part)
    }

    fn toggle(&mut self, id: &str) {
        if !self.opened.remove(id) {
            self.opened.insert(id.to_owned());
        }
    }
}

/// A chat the window shows: its rows in a transcript, how it stands, its
/// subagents' rows, and where in the core's changes it has got to.
/// The chats the window holds and what waits on them.
#[derive(Default)]
pub struct Chats {
    /// The chats of the agents with a tab open, by agent id.
    pub open: HashMap<String, Chat>,
    /// The chat in front and whether its session was ready when its draft last took the keys.
    pub(crate) focused: Option<(String, bool)>,
    /// Text handed to agents whose composer is not made yet, put in once it is.
    pub(crate) deliveries: HashMap<String, String>,
    /// What agents asked the person, and the assumptions they went on: this host's, then the
    /// chosen account host's.
    pub questions: Vec<Question>,
    /// The chosen account host's questions, apart, so this host's reading again keeps them.
    pub(crate) host_questions: Vec<Question>,
    /// Lets go of the chats out of sight once they have been for a while, counted from when
    /// the chat now in front came there.
    pub(crate) resting: Option<(Option<String>, gpui_kit::Task<()>)>,
}

pub struct Chat {
    pub transcript: Entity<Transcript<ChatRow>>,
    /// Swapped for new looks when the theme changes, which the transcript's rows draw with.
    looks: Rc<RefCell<Rc<Looks>>>,
    /// The chat's text size, as a share of its designed size.
    scale: f32,
    _theming: Subscription,
    /// The stage draws the welcome in an empty chat's place, and the activity line reads the
    /// last row, so both hear when the rows change.
    _rows: [Subscription; 2],
    /// The stage draws the find bar's count and the way back to the latest rows, so it hears
    /// when what was found changes and when the reader scrolls away from the end.
    _found: [Subscription; 2],
    /// The find bar over the transcript, while it is open.
    pub finding: Option<find::Finding>,
    /// The reader opened the agent's plan.
    plan_open: bool,
    /// The line under the last row saying what the agent is doing.
    activity_line: Entity<status::Activity>,
    /// The core of the host the agent runs on, which every ask of the chat goes to.
    pub target: Target,
    /// The head as last changed.
    state: Option<ChatHead>,
    /// The head changed since the window last drew what stands around the rows.
    head_moved: bool,
    reader: Entity<Reading>,
    /// Opened calls whose detail is older than the call, to ask the core for again.
    wanted: Vec<String>,
    pub mark: Option<ChatMark>,
    older_before: Option<u64>,
    asked_earlier: bool,
    /// The rows after `newer_after` were dropped while the reader was scrolled up, so only the
    /// head follows the live chat and `mark` stays where it was then; the chat attaches again
    /// from it once the reader pages back down to the live rows.
    detached: bool,
    newer_after: Option<u64>,
    asked_later: bool,
    /// What the person writes to the agent, made once the window can make it.
    pub composer: Option<Composing>,
    /// The answers picked for the agent's open questions, sent once each has one.
    picks: HashMap<String, Pick>,
    /// The open question on show, once the person chose one.
    shown: Option<String>,
    /// The first open question when the person folded them away; another coming first unfolds them.
    folded: Option<String>,
    /// The groups of live work over the composer the reader folded away, by kind.
    groups_folded: HashSet<String>,
    /// The cards the agent waits on as last drawn, to bring new ones in and fold answered ones away.
    waiting_seen: RefCell<motion::ListMemory<Waiting>>,
    /// The choice of the open question on show the arrow keys moved to.
    cursor: Option<(String, usize)>,
    /// The sent prompt the arrows brought back into the draft, and what was typed before.
    recalled: Option<(usize, String)>,
    /// The permission request a reply went to, until the core took it.
    replying: Option<String>,
    /// The background tasks asked to stop, until the core took each ask.
    stopping: HashSet<String>,
    /// The questions whose answers went to the core, until it took them.
    answering: HashSet<String>,
    /// A message went while the agent was not ready, so the chat says it is on its way; until
    /// then it warms up unseen.
    pub awaits_agent: bool,
    /// The menu of a file a message names, while it is open.
    file_menu: Option<files::FileMenu>,
    /// Out of sight long enough to stop following the core, which it attaches to again from
    /// its mark once it shows.
    pub resting: bool,
}

impl Chat {
    pub fn new(
        agent: String,
        provider: &str,
        bench: WeakEntity<Workbench>,
        target: Target,
        cx: &mut App,
    ) -> Chat {
        let reader = cx.new(|_| Reading::default());
        let stage = bench.clone();
        let chat = agent.clone();
        let looks = Rc::new(RefCell::new(Rc::new(Looks::new(
            Tokens::get(cx),
            provider,
            reader.clone(),
            (bench.clone(), target.asks().clone()),
            agent.clone(),
            Rc::default(),
        ))));
        let (kept_looks, text) = (looks.clone(), looks.borrow().styles.borrow().text.clone());
        let transcript = cx.new(|cx| {
            cx.observe(&reader, |_, _, cx| cx.notify()).detach();
            Transcript::new(
                Vec::new(),
                text,
                move |rows, ix, pen, window, cx| {
                    let looks = looks.borrow().clone();
                    looks.reader.clone().update(cx, |reading, cx| {
                        row(
                            rows,
                            ix,
                            pen,
                            &Drawn {
                                looks: &looks,
                                reading,
                            },
                            window,
                            cx,
                        )
                    })
                },
                cx,
            )
            .on_reach_top({
                let (bench, agent) = (bench.clone(), agent.clone());
                move |_, cx| {
                    bench.update(cx, |bench, _| bench.ask_earlier(&agent)).ok();
                }
            })
            .on_reach_bottom(move |_, cx| {
                bench.update(cx, |bench, _| bench.ask_later(&agent)).ok();
            })
            .rise_in(crate::motion::ease_out())
        });
        let redraw = |stage: &WeakEntity<Workbench>, cx: &mut App| {
            stage
                .update(cx, |bench, cx| bench.parts.redraw_stage(cx))
                .ok();
        };
        let found = [
            {
                let stage = stage.clone();
                cx.subscribe(&transcript, move |_, _: &FoundChanged, cx| {
                    redraw(&stage, cx)
                })
            },
            {
                let stage = stage.clone();
                cx.subscribe(&transcript, move |_, _: &AwayChanged, cx| {
                    redraw(&stage, cx)
                })
            },
        ];
        let rows_changed = {
            let stage = stage.clone();
            cx.subscribe(&transcript, move |_, _: &RowsChanged, cx| {
                stage
                    .update(cx, |bench, cx| {
                        if let Some(chat) = bench.chats.open.get_mut(&chat) {
                            chat.follow_activity(cx);
                            chat.trim(cx);
                        }
                    })
                    .ok();
            })
        };
        let emptied = cx.subscribe(&transcript, move |_, _: &EmptyChanged, cx| {
            stage
                .update(cx, |bench, cx| bench.parts.redraw_stage(cx))
                .ok();
        });
        let theming = cx.observe_global::<Tokens>({
            let (looks, transcript) = (kept_looks.clone(), transcript.downgrade());
            let (provider, asks) = (provider.to_owned(), target.asks().clone());
            move |cx| {
                let old = looks.borrow().clone();
                let new = Looks::new(
                    Tokens::get(cx),
                    &provider,
                    old.reader.clone(),
                    (old.bench.clone(), asks.clone()),
                    old.agent.clone(),
                    old.files.clone(),
                );
                new.pictures.replace(old.pictures.take());
                let style = new.rescale(old.scale.get());
                *looks.borrow_mut() = Rc::new(new);
                transcript
                    .update(cx, |transcript, cx| transcript.restyle(style, cx))
                    .ok();
            }
        });
        let activity_line = cx.new(|_| status::Activity::new(provider));
        transcript.update(cx, |transcript, cx| {
            transcript.set_footer(activity_line.clone().into(), cx)
        });
        Chat {
            transcript,
            looks: kept_looks,
            scale: 1.,
            _theming: theming,
            _found: found,
            finding: None,
            plan_open: false,
            activity_line,
            target,
            _rows: [emptied, rows_changed],
            state: None,
            head_moved: false,
            reader,
            wanted: Vec::new(),
            mark: None,
            older_before: None,
            asked_earlier: false,
            detached: false,
            newer_after: None,
            asked_later: false,
            composer: None,
            picks: HashMap::new(),
            shown: None,
            folded: None,
            groups_folded: HashSet::new(),
            waiting_seen: RefCell::default(),
            cursor: None,
            recalled: None,
            replying: None,
            stopping: HashSet::new(),
            answering: HashSet::new(),
            awaits_agent: false,
            file_menu: None,
            resting: false,
        }
    }

    /// What the core answered an attach with. A chat that can't be attached to while it holds
    /// rows from its middle, such as one asleep, asks for its latest rows instead.
    fn attached(
        &mut self,
        agent: &str,
        attachment: ChatAttachment,
        cx: &mut App,
    ) -> Option<ChatAsk> {
        if let ChatAttachment::Missing = attachment {
            let feed = self.mark.as_ref().filter(|_| self.detached)?.feed.clone();
            return Some(ChatAsk::Latest {
                agent: agent.to_owned(),
                feed,
            });
        }
        (self.detached, self.newer_after, self.asked_later) = (false, None, false);
        match attachment {
            ChatAttachment::Live { view, mark } => {
                match serde_json::from_value::<ChatView>(view) {
                    Ok(view) => self.show(view, cx),
                    Err(error) => eprintln!("sikemux: the core's chat view does not read: {error}"),
                }
                self.mark = Some(mark);
            }
            ChatAttachment::Resumed { changes, mark } => match changes::read(changes) {
                Ok(changes) => {
                    self.apply_all(changes, cx);
                    self.mark = Some(mark);
                }
                Err(error) => {
                    eprintln!("sikemux: the core's chat changes do not read: {error}");
                    self.mark = None;
                    return Some(ChatAsk::Attach {
                        agent: agent.to_owned(),
                        since: None,
                    });
                }
            },
            ChatAttachment::Missing => {}
        }
        None
    }

    /// Takes the core's changes numbered `seq`, and says whether some before them went missing.
    /// Applies the changes numbered `seq`, and says whether the chat must attach again from
    /// its mark: some came before them, or they did not read.
    fn changed(&mut self, seq: u64, changes: Result<Vec<Changed>, String>, cx: &mut App) -> bool {
        let changes = match changes {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("sikemux: the core's chat changes do not read: {error}");
                return self.mark.is_some() && !self.detached;
            }
        };
        if self.detached {
            let changes = changes
                .into_iter()
                .filter(|change| matches!(change, Changed::Head { .. } | Changed::View { .. }));
            self.apply_all(changes.collect(), cx);
            return false;
        }
        let Some(mark) = self.mark.as_mut() else {
            return false;
        };
        if seq <= mark.seq {
            return false;
        }
        let follows = changes.first().and_then(Changed::follows);
        if seq != mark.seq + 1 && follows != Some(mark.seq) {
            return true;
        }
        mark.seq = seq;
        self.apply_all(changes, cx);
        false
    }

    fn show(&mut self, view: ChatView, cx: &mut App) {
        let subagents = rows_by_session(view.subagents);
        self.reader.update(cx, |reading, cx| {
            reading.subagents = subagents;
            cx.notify();
        });
        self.state = Some(view.head);
        self.head_moved = true;
        self.follow_head(cx);
        self.older_before = view.older_before;
        self.asked_earlier = false;
        (self.detached, self.newer_after, self.asked_later) = (false, None, false);
        let rows = view.rows.into_iter().map(ChatRow::new).collect();
        self.transcript.update(cx, |transcript, cx| {
            transcript.push(Change::Reset(rows), cx)
        });
    }

    /// What the core sent of a tool call the reader opened.
    fn detail(&mut self, tool: String, detail: Value, cx: &mut App) {
        if let Ok(detail) = serde_json::from_value(detail) {
            self.reader.update(cx, |reading, cx| {
                reading.tools.held(tool, detail);
                cx.notify();
            });
        }
    }

    /// Takes what the core sent about this chat, other than what a find found, and asks it
    /// for what that leaves missing.
    pub fn hear(&mut self, agent: &str, news: ChatNews, cx: &mut App) {
        let ask = |chat: &Self, ask: ChatAsk| chat.target.ask(Ask::Chat(ask));
        match news {
            ChatNews::Attached(attachment) => {
                if let Some(next) = self.attached(agent, attachment, cx) {
                    ask(self, next);
                }
            }
            ChatNews::Taken(attachment) => {
                if let Some(composing) = self.composer.as_mut() {
                    if matches!(composing.stage, ComposeStage::Sent(_)) {
                        composing.stage = ComposeStage::Writing;
                    }
                }
                if let Some(next) = attachment.and_then(|it| self.attached(agent, it, cx)) {
                    ask(self, next);
                }
            }
            ChatNews::Changes { seq, changes } => {
                if self.changed(seq, changes, cx) {
                    let since = self.mark.clone();
                    ask(
                        self,
                        ChatAsk::Attach {
                            agent: agent.to_owned(),
                            since,
                        },
                    );
                }
                for tool in self.wanted() {
                    ask(
                        self,
                        ChatAsk::Detail {
                            agent: agent.to_owned(),
                            tool,
                        },
                    );
                }
            }
            ChatNews::Page(page) => self.page(page, cx),
            ChatNews::Later(page) => {
                if let Some(next) = self.later_page(agent, page, cx) {
                    ask(self, next);
                }
            }
            ChatNews::Around(page) => self.around_page(page, cx),
            ChatNews::Latest(page) => self.latest_page(page, cx),
            ChatNews::Found { .. } => {}
            ChatNews::Detail { tool, detail } => self.detail(tool, detail, cx),
            ChatNews::Replied { request, error } => self.replied(&request, error),
            ChatNews::TaskStopped { task, error } => self.task_stopped(&task, error),
            ChatNews::Refused { error, text } => {
                if let Some(composing) = self.composer.as_mut() {
                    composing.refusal = Some(error.into());
                    let held = match std::mem::take(&mut composing.stage) {
                        ComposeStage::Sent(sent) if sent.content.text().trim() == text => {
                            Some(sent)
                        }
                        _ => None,
                    };
                    composing.stage = ComposeStage::Returned(held.unwrap_or_else(|| Sent {
                        content: text.into(),
                        attachments: Vec::new(),
                        context: Vec::new(),
                        tracked_context: HashMap::new(),
                    }));
                }
                self.head_moved = true;
            }
        }
    }

    /// The tool calls whose detail to ask the core for again.
    fn wanted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.wanted)
    }

    /// Keeps what the rows draw of how the chat stands in step with its head.
    fn follow_head(&mut self, cx: &mut App) {
        if self.ready() {
            self.awaits_agent = false;
        }
        let running = self.running();
        let editable = self.state.as_ref().is_some_and(|head| {
            head.capabilities.editing && head.connection == ChatConnection::Ready && !head.running
        });
        let earlier_lost = self.state.as_ref().is_some_and(|head| head.earlier_lost);
        self.reader.update(cx, |reading, cx| {
            let now = (running, editable, earlier_lost);
            if (reading.running, reading.editable, reading.earlier_lost) != now {
                (reading.running, reading.editable, reading.earlier_lost) = now;
                cx.notify();
            }
        });
        self.follow_activity(cx);
    }

    /// Says on the line under the last row what the agent is doing, from the head and the
    /// rows as they stand; the rows land only when the transcript draws, so this runs again then.
    fn follow_activity(&self, cx: &mut App) {
        let doing = self.activity(cx);
        self.activity_line
            .update(cx, |activity, cx| activity.set(doing, cx));
    }

    /// While the find bar looks for something, long texts show all of themselves so every
    /// place it finds can be seen.
    pub fn unfold_for_find(&self, on: bool, cx: &mut App) {
        if self.reader.read(cx).finding == on {
            return;
        }
        self.reader.update(cx, |reading, cx| {
            reading.finding = on;
            cx.notify();
        });
        self.transcript.read(cx).list().remeasure();
    }

    /// The core took the reply to `request`, or says why not; a request that went
    /// meanwhile needs no telling.
    fn replied(&mut self, request: &str, error: Option<String>) {
        if self.replying.as_deref() == Some(request) {
            self.replying = None;
        }
        self.failed(error.filter(|error| !error.contains("no longer pending")));
    }

    /// Whether nothing the person did waits on the core's answer, so the chat can stop
    /// following it while out of sight.
    pub fn can_rest(&self) -> bool {
        self.answering.is_empty()
            && self.stopping.is_empty()
            && self.replying.is_none()
            && !self.awaits_agent
            && self
                .composer
                .as_ref()
                .is_none_or(|composing| !composing.stage.waits())
    }

    /// The core took the ask to stop `task`, or says why not.
    fn task_stopped(&mut self, task: &str, error: Option<String>) {
        self.stopping.remove(task);
        self.failed(error);
    }

    fn failed(&mut self, error: Option<String>) {
        if let (Some(error), Some(composing)) = (error, self.composer.as_mut()) {
            composing.refusal = Some(error.into());
        }
        self.head_moved = true;
    }

    fn forget_choices(&mut self) {
        if let Some(state) = self.state.as_mut() {
            state.model = None;
            state.effort = None;
        }
        self.head_moved = true;
    }

    /// Whether the head changed since this was last asked.
    pub fn take_head_moved(&mut self) -> bool {
        std::mem::take(&mut self.head_moved)
    }

    /// Draws the chat's text at `scale` times its designed size.
    pub fn rescale(&mut self, scale: f32, cx: &mut App) {
        if scale == self.scale {
            return;
        }
        self.scale = scale;
        let style = self.looks.borrow().rescale(scale);
        self.transcript
            .update(cx, |transcript, cx| transcript.restyle(style, cx));
    }

    /// What the messages waiting behind the running turn say.
    pub fn waiting(&self) -> Vec<String> {
        self.state.as_ref().map_or(Vec::new(), |head| {
            head.queued
                .iter()
                .map(|queued| queued.text.trim().to_owned())
                .filter(|text| !text.is_empty())
                .collect()
        })
    }

    /// The agent's session is up and takes messages.
    pub fn ready(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|head| head.connection == ChatConnection::Ready)
    }

    pub fn running(&self) -> bool {
        self.state.as_ref().is_some_and(|head| head.running)
    }

    pub fn is_empty(&self, cx: &App) -> bool {
        self.transcript.read(cx).rows().is_empty()
    }

    /// The earlier turns to ask for, when there are more and none are on their way.
    pub fn earlier(&mut self, agent: &str) -> Option<ChatAsk> {
        let (before, mark) = (self.older_before?, self.mark.as_ref()?);
        if self.asked_earlier {
            return None;
        }
        self.asked_earlier = true;
        Some(ChatAsk::History {
            agent: agent.to_owned(),
            feed: mark.feed.clone(),
            before,
            turns: PAGE_TURNS,
        })
    }

    fn page(&mut self, page: Value, cx: &mut App) {
        self.asked_earlier = false;
        let Ok(page) = serde_json::from_value::<ChatPage>(page) else {
            return;
        };
        self.older_before = page.older_before;
        self.reader.update(cx, |reading, cx| {
            for (session, rows) in rows_by_session(page.subagents) {
                reading.subagents.entry(session).or_insert(rows);
            }
            cx.notify();
        });
        let rows = page.rows.into_iter().map(ChatRow::new).collect();
        self.transcript.update(cx, |transcript, cx| {
            transcript.push(Change::Prepend(rows), cx)
        });
    }

    /// Brings the latest rows into view: those held, or, after rows below were dropped, the
    /// chat as it stands, attached afresh.
    pub fn show_latest(&mut self, agent: &str, cx: &mut App) -> Option<ChatAsk> {
        if self.detached {
            return Some(ChatAsk::Attach {
                agent: agent.to_owned(),
                since: None,
            });
        }
        self.transcript
            .update(cx, |transcript, cx| transcript.to_latest(cx));
        None
    }

    /// The rows after the last one held, when the reader is near it and they were dropped.
    pub fn later(&mut self, agent: &str) -> Option<ChatAsk> {
        if !self.detached || self.asked_later {
            return None;
        }
        self.asked_later = true;
        Some(match (self.newer_after, self.mark.as_ref()) {
            (Some(after), Some(mark)) => ChatAsk::Later {
                agent: agent.to_owned(),
                feed: mark.feed.clone(),
                after,
            },
            _ => ChatAsk::Attach {
                agent: agent.to_owned(),
                since: self.mark.clone(),
            },
        })
    }

    /// A later page, a `ChatPage` as JSON, whose `newer_after` is none once it reached the live
    /// rows; the chat then attaches again from its mark, which it answers.
    fn later_page(&mut self, agent: &str, page: Value, cx: &mut App) -> Option<ChatAsk> {
        self.asked_later = false;
        let page = serde_json::from_value::<ChatPage>(page).ok()?;
        let newer_after = page.newer_after;
        self.newer_after = newer_after;
        let rows = page.rows.into_iter().map(ChatRow::new).collect();
        self.transcript.update(cx, |transcript, cx| {
            transcript.push(Change::Append(rows), cx)
        });
        newer_after.is_none().then(|| ChatAsk::Attach {
            agent: agent.to_owned(),
            since: self.mark.clone(),
        })
    }

    /// The rows around a place found, which take the place of those held; the chat follows only
    /// its head until the reader pages back down to its live rows.
    fn around_page(&mut self, page: Value, cx: &mut App) {
        let Ok(page) = serde_json::from_value::<ChatPage>(page) else {
            return;
        };
        self.older_before = page.older_before;
        self.newer_after = page.newer_after;
        (self.detached, self.asked_earlier, self.asked_later) = (true, false, false);
        let rows = page.rows.into_iter().map(ChatRow::new).collect();
        self.transcript.update(cx, |transcript, cx| {
            transcript.push(Change::Place(rows), cx)
        });
    }

    /// The chat's latest rows, which take the place of those held.
    fn latest_page(&mut self, page: Value, cx: &mut App) {
        let Ok(page) = serde_json::from_value::<ChatPage>(page) else {
            return;
        };
        self.older_before = page.older_before;
        (self.detached, self.newer_after) = (false, None);
        (self.asked_earlier, self.asked_later) = (false, false);
        let rows = page.rows.into_iter().map(ChatRow::new).collect();
        self.transcript.update(cx, |transcript, cx| {
            transcript.push(Change::Reset(rows), cx);
            transcript.to_latest(cx);
        });
    }

    /// Drops the rows far from what the reader sees, so a chat holds about the same however long
    /// it grows: while it follows its latest rows, all but its last turns; while the reader is
    /// scrolled up, those well above and below the view, which pages bring back.
    pub fn trim(&mut self, cx: &mut App) {
        let transcript = self.transcript.read(cx);
        let rows = transcript.rows();
        let mut drops = Vec::new();
        let mut kept = 0..rows.len();
        if transcript.away() {
            let top = transcript.list().logical_scroll_top().item_ix;
            if top > HELD_AROUND + SLACK_ROWS {
                let from = &rows[top - HELD_AROUND];
                if let Some(event) = from.first_event() {
                    self.older_before = Some(event);
                    drops.push(Change::DropBefore(from.id().to_owned().into()));
                    kept.start = top - HELD_AROUND;
                }
            }
            if rows.len() > top + 2 * HELD_AROUND + SLACK_ROWS {
                let (last, cut) = (
                    &rows[top + 2 * HELD_AROUND - 1],
                    &rows[top + 2 * HELD_AROUND],
                );
                if let Some(event) = last.first_event() {
                    (self.newer_after, self.detached) = (Some(event), true);
                    drops.push(Change::DropFrom(cut.id().to_owned().into()));
                    kept.end = top + 2 * HELD_AROUND;
                }
            }
        } else {
            let turns: Vec<usize> = (0..rows.len())
                .filter(|&ix| rows[ix].opens_turn())
                .collect();
            let by_turns =
                (turns.len() > HELD_TURNS + SLACK_TURNS).then(|| turns[turns.len() - HELD_TURNS]);
            let by_rows = (rows.len() > HELD_ROWS + SLACK_ROWS).then(|| rows.len() - HELD_ROWS);
            if let Some(at) = by_turns.max(by_rows) {
                if let Some(event) = rows[at].first_event() {
                    self.older_before = Some(event);
                    drops.push(Change::DropBefore(rows[at].id().to_owned().into()));
                    kept.start = at;
                }
            }
        }
        if drops.is_empty() {
            return;
        }
        #[cfg(feature = "counts")]
        gpui_transcript::counts::add(&gpui_transcript::counts::TRIMS, 1);
        let held: HashSet<&str> = rows[kept].iter().map(|row| row.id()).collect();
        let streamed: Vec<String> = (self.reader.read(cx).streamed.iter())
            .filter(|row| held.contains(row.as_str()))
            .cloned()
            .collect();
        self.reader.update(cx, |reading, _| {
            reading.streamed = streamed.into_iter().collect();
        });
        self.transcript
            .update(cx, |transcript, cx| transcript.extend(drops, cx));
    }

    fn apply_all(&mut self, changes: Vec<Changed>, cx: &mut App) {
        let mut rows = Vec::new();
        for change in changes {
            match change {
                Changed::Head { set, .. } => {
                    let head = self.state.get_or_insert_with(ChatHead::default);
                    for field in set {
                        field(head);
                    }
                    self.head_moved = true;
                    self.follow_head(cx);
                }
                Changed::View { view } => {
                    self.transcript
                        .update(cx, |transcript, cx| transcript.extend(rows.drain(..), cx));
                    self.show(*view, cx);
                }
                Changed::Row {
                    row,
                    within: Some(session),
                } => {
                    self.reader.update(cx, |reading, cx| {
                        let held = reading.subagents.entry(session).or_default();
                        match held.iter_mut().find(|held| held.message.id == row.id) {
                            Some(held) => *held = ChatRow::new(row),
                            None => held.push(ChatRow::new(row)),
                        }
                        if held.len() > SUBAGENT_ROWS + SLACK_ROWS {
                            held.drain(..held.len() - SUBAGENT_ROWS);
                        }
                        cx.notify();
                    });
                }
                Changed::Row { row, within: None } => rows.push(Change::Put(ChatRow::new(row))),
                Changed::Part { row, part, within } => {
                    self.wanted.extend(self.reader.read(cx).tools.stale(&part));
                    self.update(&mut rows, row, within, move |held| held.put_part(part), cx)
                }
                Changed::Text {
                    row,
                    part,
                    text,
                    md,
                    within,
                } => {
                    if within.is_none() && !self.reader.read(cx).streamed.contains(&row) {
                        self.reader.update(cx, |reading, _| {
                            reading.streamed.insert(row.clone());
                        });
                    }
                    self.update(
                        &mut rows,
                        row,
                        within,
                        move |held| held.write(&part, &text, md.as_ref()),
                        cx,
                    )
                }
                Changed::Set {
                    row,
                    set,
                    docs,
                    within,
                } => self.update(&mut rows, row, within, move |held| held.set(set, docs), cx),
                Changed::Keep { rows: kept } => rows.push(Change::Keep(
                    kept.into_iter().map(SharedString::from).collect(),
                )),
                Changed::Drop { from } => rows.push(Change::DropFrom(from.into())),
            }
        }
        self.transcript
            .update(cx, |transcript, cx| transcript.extend(rows, cx));
    }

    /// Changes a row of the chat, or of one of its subagents.
    fn update(
        &mut self,
        rows: &mut Vec<Change<ChatRow>>,
        row: String,
        within: Option<String>,
        change: impl FnOnce(&mut ChatRow) + 'static,
        cx: &mut App,
    ) {
        match within {
            Some(session) => self.reader.update(cx, |reading, cx| {
                let held = reading
                    .subagents
                    .get_mut(&session)
                    .and_then(|held| held.iter_mut().find(|held| held.message.id == row));
                if let Some(held) = held {
                    change(held);
                    cx.notify();
                }
            }),
            None => rows.push(Change::Update(row.into(), Box::new(change))),
        }
    }
}

fn rows_by_session(
    subagents: impl IntoIterator<Item = (String, Vec<ChatMessage>)>,
) -> HashMap<String, Vec<ChatRow>> {
    subagents
        .into_iter()
        .map(|(session, rows)| (session, rows.into_iter().map(ChatRow::new).collect()))
        .collect()
}

/// The chat in the stage: its rows, with a scrollbar over their right edge, or before its
/// first message `welcome`; under them what waits on the person, then the composer.
/// What `agent`'s empty chat says in place of the welcome while its session is not ready.
pub fn connection_note(
    agent: &AgentRow,
    chat: &Chat,
    resumes: bool,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    status::connection_note(agent, chat, resumes, cx)
}

/// How deep the transcript fades out as rows scroll up under the pane's header.
const TOP_FADE: f32 = 16.;

pub fn pane(
    bench: &Workbench,
    agent: &AgentRow,
    chat: &Chat,
    welcome: Option<AnyElement>,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    if let Some(saved) = bench.shown().and_then(|view| {
        view.workspace
            .agents
            .iter()
            .find(|saved| saved.id == agent.id)
    }) {
        let cwd = saved.cwd.to_string_lossy();
        let home = bench.home_at(chat.target.place());
        (chat.looks.borrow().files.borrow_mut()).place(&cwd, &chat.target, home, &chat.reader);
    }
    let file_menu = chat.file_menu.as_ref().map(|open| {
        gpui_kit::deferred(
            gpui_kit::anchored()
                .position(open.at)
                .snap_to_window_with_margin(px(8.))
                .child(open.menu.clone()),
        )
        .with_priority(1)
    });
    let worktree = bench
        .worktree_switch(&agent.id, cx)
        .map(|(switch, _)| switch)
        .filter(|switch| !matches!(switch, crate::worktree_switch::Switch::Choosing { .. }));
    let empty_note = worktree
        .as_ref()
        .filter(|_| chat.is_empty(cx))
        .and_then(|switch| crate::worktree_switch::note(switch, cx));
    chat.activity_line
        .update(cx, |line, cx| line.wear(agent, cx));
    chat.reader.update(cx, |reading, cx| {
        if reading.worktree != worktree {
            reading.worktree = worktree;
            cx.notify();
        }
    });
    let welcome = match (empty_note, welcome) {
        (None, welcome) => welcome,
        (Some(note), welcome) => Some(
            div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .child(note)
                .children(welcome)
                .into_any_element(),
        ),
    };
    let list = chat.transcript.read(cx).list().clone();
    // The scrollbar shows whenever its list moves, and a list following the latest rows moves
    // with every streamed line; only a reader who scrolled up has a thumb, as on the Mac.
    let scrollbar = chat.transcript.read(cx).away().then(|| {
        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .child(Scrollbar::vertical(&list))
    });
    let rows = div()
        .flex_1()
        .min_h_0()
        .relative()
        .child(
            div().size_full().fade_edges(px(TOP_FADE), px(0.)).child(
                chat.transcript
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            ),
        )
        .children(scrollbar)
        .children(chat.finding.as_ref().map(|finding| {
            let transcript = chat.transcript.read(cx);
            let (found, local) = (transcript.found(), transcript.found_at());
            let local = local.map(|(row, part, nth)| (row.to_owned(), part.to_owned(), nth));
            let local = local
                .as_ref()
                .map(|(row, part, nth)| (row.as_str(), part.as_str(), *nth));
            find::bar(&agent.id, finding, found, local, cx)
        }))
        .child({
            let away = chat.transcript.read(cx).away();
            let latest = cx.listener({
                let agent = agent.id.clone();
                move |bench, _, _, cx| bench.show_latest(&agent, cx)
            });
            let (line, ground) = (Tokens::get(cx).line, Tokens::get(cx).surface_composer);
            motion::animated(
                "chat-latest-shown",
                if away { 1. } else { 0. },
                motion::POP_UP,
                move |shown| {
                    if shown <= 0. {
                        return div().into_any_element();
                    }
                    div()
                        .absolute()
                        .bottom(px(8. - LATEST_RISE * (1. - shown)))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .opacity(shown)
                        .child(
                            div()
                                .rounded(theme::RADIUS_2)
                                .border_1()
                                .border_color(line)
                                .bg(ground)
                                .shadow_md()
                                .child(
                                    IconButton::new(
                                        "chat-latest",
                                        Icon::ArrowDown,
                                        "Jump to latest message",
                                    )
                                    .glyph(14.)
                                    .on_click(latest),
                                ),
                        )
                        .into_any_element()
                },
            )
        })
        .children(welcome.map(|welcome| crate::home::centred(welcome).absolute().inset_0()));
    let open: Vec<&Question> = bench
        .chats
        .questions
        .iter()
        .filter(|question| question.asks(&agent.id))
        .collect();
    let permissions = chat
        .state
        .as_ref()
        .map_or(&[][..], |head| head.permissions.as_slice());
    let mut waiting: Vec<(String, Waiting)> = permissions
        .iter()
        .map(|ask| {
            let key = format!("permission:{}", ask.request_id);
            (key, Waiting::Permission(Box::new(ask.clone())))
        })
        .collect();
    if chat.current(&open).is_some() {
        let asked = open.iter().map(|question| (*question).clone()).collect();
        waiting.push(("questions".into(), Waiting::Questions(asked)));
    }
    let answered: Vec<&Question> = bench
        .chats
        .questions
        .iter()
        .filter(|question| question.of(&agent.id, QuestionState::Answered))
        .collect();
    if !answered.is_empty() {
        let sent = answered
            .iter()
            .map(|question| (*question).clone())
            .collect();
        waiting.push(("answered".into(), Waiting::Answered(sent)));
    }
    let lasting = motion::CARD_ARRIVAL
        .lasts()
        .max(motion::CARD_DEPARTURE.close.duration);
    let mut seen = chat.waiting_seen.borrow_mut();
    let (arrived, left) = seen.update(agent.id.clone(), waiting, usize::MAX, lasting);
    let comes_in = |key: &str, card: AnyElement| match arrived.iter().any(|at| at == key) {
        true => {
            let id = SharedString::from(format!("arrive:{key}"));
            motion::arriving(id, motion::CARD_ARRIVAL, card).into_any_element()
        }
        false => card,
    };
    let mut permissions: Vec<AnyElement> = permissions
        .iter()
        .map(|ask| {
            let card = permission(&agent.id, ask, chat.replying.is_some(), cx).into_any_element();
            comes_in(&format!("permission:{}", ask.request_id), card)
        })
        .collect();
    let mut asking = questions(&agent.id, chat, &open, cx).map(|card| comes_in("questions", card));
    let mut sending =
        waiting_answers(&agent.id, &answered, cx).map(|card| comes_in("answered", card));
    for gone in left {
        let card = match &gone.item {
            Waiting::Permission(ask) => permission(&agent.id, ask, true, cx).into_any_element(),
            Waiting::Questions(asked) => {
                let asked: Vec<&Question> = asked.iter().collect();
                match questions(&agent.id, chat, &asked, cx) {
                    Some(card) => card,
                    None => continue,
                }
            }
            Waiting::Answered(sent) => {
                let sent: Vec<&Question> = sent.iter().collect();
                match waiting_answers(&agent.id, &sent, cx) {
                    Some(card) => card,
                    None => continue,
                }
            }
        };
        let id = SharedString::from(format!("depart:{}", gone.id));
        let card =
            motion::departing(id, gone.since, motion::CARD_DEPARTURE, card).into_any_element();
        match gone.item {
            Waiting::Permission(_) => permissions.insert(gone.at.min(permissions.len()), card),
            Waiting::Questions(_) => asking = asking.or(Some(card)),
            Waiting::Answered(_) => sending = sending.or(Some(card)),
        }
    }
    drop(seen);
    let mut groups: Vec<AnyElement> = asking.into_iter().collect();
    groups.extend(live::subagents(agent, chat, cx));
    groups.extend(live::tasks(agent, chat, cx));
    groups.extend(sending);
    groups.extend(live::queued(&agent.id, chat, cx));
    groups.extend(live::triggers(&agent.id, chat, cx));
    let stack = live::stack(groups, Tokens::get(cx));
    let covered = stack.is_some();
    let empty = chat
        .composer
        .as_ref()
        .is_some_and(|composing| composing.draft.read(cx).value().is_empty());
    let asking = empty
        && chat.current(&open).is_some_and(|question| !question.secret)
        && !chat.is_folded(&open);
    let id = &agent.id;
    let composing = chat.composer.is_some();
    let pane = div()
        .size_full()
        .relative()
        .flex()
        .flex_col()
        .when(composing, |pane| {
            crate::stash::takes_drops(attach::takes_drops(pane, id, cx), id, cx)
        })
        .when(asking, |pane| {
            question_keys(pane, id, true, &cx.weak_entity())
        })
        .child(rows)
        .children(file_menu)
        .children(status::plan(agent, chat, cx))
        .children(status::trouble(bench, agent, chat, cx))
        .children(permissions)
        .children(stack)
        .children(chat.composer.as_ref().map(|composing| {
            composing.view.update(cx, |view, cx| {
                view.agent = Some(agent.clone());
                if view.covered != covered {
                    view.covered = covered;
                    cx.notify();
                }
            });
            composing.view.clone().into_element().isolated()
        }))
        .when(composing && cx.has_active_drag(), |pane| {
            pane.child(gpui_kit::deferred(attach::drop_hint(cx)))
        });
    let this = cx.weak_entity();
    let agent = id.to_owned();
    div()
        .size_full()
        .key_context("Chat")
        .on_action({
            let this = this.clone();
            move |_: &FindInChat, window, cx| {
                this.update(cx, |bench, cx| bench.open_find(&agent, window, cx))
                    .ok();
            }
        })
        .on_pinch({
            let this = this.clone();
            move |event, _, cx| {
                this.update(cx, |bench, _| {
                    crate::pinch::resize(bench, crate::pinch::Surface::Chat, event)
                })
                .ok();
            }
        })
        .on_action(text_size::<TextSizeIncrease>(&this, Some(true)))
        .on_action(text_size::<TextSizeDecrease>(&this, Some(false)))
        .on_action(text_size::<TextSizeReset>(&this, None))
        .child(pane)
}

/// How much one press of ⌘= or ⌘− changes the chat's text size.
const TEXT_SCALE_STEP: f64 = 0.1;

/// Makes the chat's text bigger or smaller by `step`, or back to its designed size without
/// one, for every chat.
fn text_size<A: gpui_kit::Action>(
    this: &WeakEntity<Workbench>,
    grow: Option<bool>,
) -> impl Fn(&A, &mut Window, &mut App) + use<A> {
    let this = this.clone();
    move |_, _, cx| {
        this.update(cx, |bench, _| resize_text(bench, grow)).ok();
    }
}

/// Makes the chats' text bigger or smaller a step, or back to the default size without either.
pub(crate) fn resize_text(bench: &Workbench, grow: Option<bool>) {
    let scale = grow.map_or(DEFAULT_TEXT_SCALE, |grow| {
        let step = if grow {
            TEXT_SCALE_STEP
        } else {
            -TEXT_SCALE_STEP
        };
        ((bench.preferences().chat_text_scale + step) * 100.).round() / 100.
    });
    crate::settings::set(
        bench,
        "chatTextScale",
        scale.clamp(MIN_TEXT_SCALE, MAX_TEXT_SCALE),
    );
}

fn pressed<A: gpui::Action, F: Fn(&A) -> Press + 'static>(
    agent: &str,
    press: F,
    bench: &WeakEntity<Workbench>,
) -> impl Fn(&A, &mut Window, &mut App) + use<A, F> {
    let (agent, this) = (agent.to_owned(), bench.clone());
    move |action: &A, _: &mut Window, cx: &mut App| {
        let taken = this.update(cx, |bench, cx| {
            let open: Vec<&Question> = bench
                .chats
                .questions
                .iter()
                .filter(|question| question.asks(&agent))
                .collect();
            let chat = bench.chats.open.get_mut(&agent)?;
            let (taken, ask) = chat.press(press(action), &open);
            if let Some(ask) = ask {
                bench.ask_answering(&agent, ask, cx);
            }
            taken.then(|| cx.notify())
        });
        if !matches!(taken, Ok(Some(()))) {
            cx.propagate();
        }
    }
}

/// Centred on the transcript's column.
fn column() -> gpui_kit::Div {
    div().w_full().px(px(22.)).flex().justify_center()
}

impl Workbench {
    /// How many tool calls of the chat in front have their detail held, and how many are open.
    pub fn tool_details(&self, cx: &App) -> Option<(usize, usize)> {
        let chat = self.chats.open.get(&self.agent()?.id)?;
        Some(chat.reader.read(cx).tools.counts())
    }

    /// Opens the first tool call of the chat in front, as a click on its row does.
    pub fn open_first_tool(&mut self, cx: &mut App) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        let Some(chat) = self.chats.open.get(&agent) else {
            return;
        };
        let first = chat.transcript.read(cx).rows().iter().find_map(|row| {
            row.message.parts.iter().find_map(|part| match part {
                ChatPart::Tool { tool, ended_at, .. } => {
                    Some((tool.tool_call_id.clone(), *ended_at))
                }
                _ => None,
            })
        });
        let Some((tool, ended)) = first else {
            return;
        };
        let ask = chat.reader.update(cx, |reading, cx| {
            cx.notify();
            reading.tools.toggle(&tool, ended)
        });
        if ask {
            self.ask_chat(ChatAsk::Detail { agent, tool });
        }
    }

    /// Opens `menu` for a file the chat of `agent` names, at `at`, until it is dismissed.
    pub fn show_file_menu(
        &mut self,
        agent: &str,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        menu: Entity<gpui_kit::component::menu::PopupMenu>,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.open.get_mut(agent) else {
            return;
        };
        let agent = agent.to_owned();
        let dismissed = cx.subscribe(&menu, move |bench, _, _: &gpui_kit::DismissEvent, cx| {
            if let Some(chat) = bench.chats.open.get_mut(&agent) {
                chat.file_menu = None;
            }
            bench.parts.redraw_stage(cx);
        });
        chat.file_menu = Some(files::FileMenu {
            at,
            menu,
            _dismissed: dismissed,
        });
        self.parts.redraw_stage(cx);
    }
}
