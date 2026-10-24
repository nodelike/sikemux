//! An agent's chat: the rows the core folds, kept in a transcript, and how
//! each row draws. Rows change only by the core's changes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::f32::consts::FRAC_PI_2;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, Scrollbar};
use gpui_kit::component::input::{Input, InputState, TextareaState};
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, relative, AnyElement, App, AppContext, Context, Div, Entity, Focusable,
    FontWeight, HighlightStyle, Hsla, InteractiveElement, IntoElement, KeyBinding, ParentElement,
    Radians, SharedString, StatefulInteractiveElement, StyleRefinement, Styled, StyledText,
    Subscription, Task, Transformation, WeakEntity, Window,
};
use gpui_transcript::{
    AwayChanged, Change, CodeColours, CodeRuns, EmptyChanged, FoundChanged, Markdown, Pen, Row,
    RowsChanged, Style, Transcript,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_wire::chat::{
    ChatChange, ChatConnection, ChatHead, ChatMessage, ChatPage, ChatPart, ChatRole, ChatView,
    LetterKind, MoveReason, NoticeSeverity, NoticeState, PartTree, PermissionAsk, SubagentCard,
    SubagentState, WorkFold,
};
use sikemux_wire::protocol::{
    AgentStatus, ChatAttachment, ChatContext, ChatMark, ConfigPicker, MidTurnSend, WorkspaceAgent,
    DEFAULT_TEXT_SCALE, MAX_TEXT_SCALE, MIN_TEXT_SCALE,
};

use crate::composer::{Composer, Strip};
use crate::core::{Ask, ChatAsk, Target};
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
use gpui_kit::component::Sizable;
use tokio::sync::mpsc::UnboundedSender;

mod attach;
mod content;
pub(crate) mod draft;
mod edit;
mod files;
mod find;
mod link;
mod live;
mod mention;
mod meta;
mod page;
mod status;
mod tools;
mod worktree;

pub use draft::key as draft_key;

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
            .filter(|(_, letter)| letter.kind == LetterKind::Message);
        self.docs =
            parts
                .chain(letters.map(|(ix, letter)| {
                    Doc::new(&letter_doc(ix), &letter.text, letter.tree.as_ref())
                }))
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

    fn set(&mut self, fields: &Value) {
        let Ok(mut whole) = serde_json::to_value(&self.message) else {
            return;
        };
        merge(&mut whole, fields);
        if let Ok(message) = serde_json::from_value(whole) {
            self.message = message;
            // A part's own tree is the one it arrived with; its text's later patches live only
            // in its doc, so the docs are read again only when the parts themselves changed.
            if fields.get("parts").is_some() || fields.get("letters").is_some() {
                self.read_docs();
            }
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

/// `fields` laid over `on`; a field sent as null is gone.
fn merge(on: &mut Value, fields: &Value) {
    let (Some(on), Some(fields)) = (on.as_object_mut(), fields.as_object()) else {
        return;
    };
    for (key, value) in fields {
        if value.is_null() {
            on.remove(key);
        } else {
            on.insert(key.clone(), value.clone());
        }
    }
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
        CancelEdit
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
    ];
    for context in ["QuestionDock > Input", "QuestionDock"] {
        bindings
            .extend((1..=9).map(|n| KeyBinding::new(&n.to_string(), Choose(n - 1), Some(context))));
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
pub struct Chat {
    pub transcript: Entity<Transcript<ChatRow>>,
    looks: Rc<Looks>,
    /// The chat's text size, as a share of its designed size.
    scale: f32,
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
    head: Map<String, Value>,
    /// The head as last changed, read once for each change.
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
    /// The assumption the person chose to change, which the composer then answers.
    writing: Option<String>,
    /// The groups of live work over the composer the reader folded away, by kind.
    groups_folded: HashSet<String>,
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
    /// The menu of a file a message names, while it is open.
    file_menu: Option<files::FileMenu>,
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
    /// What the core is doing to start the chat in a worktree, while it does.
    pub preparing: Option<String>,
    /// The message waits for the chat to be ready in its new worktree.
    pub send_when_ready: bool,
    /// The last message as it was written, chips, files and all, until the core takes it.
    pub sent: Option<Sent>,
    /// A turned-down message, going back into the draft when it next draws.
    pub returned: Option<Sent>,
    /// The draft as the core last kept it.
    pub synced: Value,
    /// A draft typed in another app, with this draft as it was when that came.
    pub arrived: Option<(Sent, Value)>,
    pub _keeping: Task<()>,
    pub _heard: [Subscription; 2],
}

/// A message as the composer held it when it was sent.
pub struct Sent {
    pub content: gpui_kit::base::input::InputContent,
    pub attachments: Vec<PathBuf>,
    pub context: Vec<ChatContext>,
    pub tracked_context: HashMap<String, ChatContext>,
}

/// A question an agent put to the person, as the core's `questions` call lists it.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    id: String,
    agent_id: String,
    name: String,
    text: String,
    header: Option<String>,
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    multi: bool,
    assumption: Option<String>,
    #[serde(default)]
    secret: bool,
    /// `lead` while the asker's lead has it, `open`, `assumed` or `answered`.
    state: String,
    via_note: Option<String>,
    secret_note: Option<String>,
    /// The person's answer, once answered; none for a secret.
    answer: Option<String>,
    /// The account host whose agent asks, by its key; none for this host's own.
    #[serde(skip)]
    pub(crate) host: Option<String>,
}

impl Question {
    /// Who asked and what, while `agent_id` waits on the person's answer.
    pub(crate) fn open_from(&self, agent_id: &str) -> Option<(&str, &str)> {
        (self.agent_id == agent_id && self.state == "open").then_some((&self.name, &self.text))
    }
}

#[derive(Clone, Deserialize)]
struct Choice {
    label: String,
    detail: Option<String>,
}

impl Question {
    pub fn of(&self, agent: &str, state: &str) -> bool {
        self.agent_id == agent && self.state == state
    }

    fn title(&self, ix: usize, count: usize) -> String {
        match &self.header {
            Some(header) => header.clone(),
            None if count > 1 => format!("Question {}", ix + 1),
            None => "Question".into(),
        }
    }
}

enum Answer {
    Choice(usize),
    Choices(Vec<usize>),
    Text(String),
}

/// An answer picked for an open question, and what the dock shows was picked; empty for a secret.
struct Pick {
    answer: Answer,
    shown: String,
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
        let looks = Rc::new(Looks::new(
            Tokens::get(cx),
            provider,
            reader.clone(),
            (bench.clone(), target.asks().clone()),
            agent.clone(),
        ));
        let (kept_looks, text) = (looks.clone(), looks.styles.borrow().text.clone());
        let transcript = cx.new(|cx| {
            cx.observe(&reader, |_, _, cx| cx.notify()).detach();
            Transcript::new(
                Vec::new(),
                text,
                move |rows, ix, pen, window, cx| {
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
                .update(cx, |bench, cx| bench.parts.redraw(&bench.parts.stage, cx))
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
                        if let Some(chat) = bench.chats.get_mut(&chat) {
                            chat.follow_activity(cx);
                            chat.trim(cx);
                        }
                    })
                    .ok();
            })
        };
        let emptied = cx.subscribe(&transcript, move |_, _: &EmptyChanged, cx| {
            stage
                .update(cx, |bench, cx| bench.parts.redraw(&bench.parts.stage, cx))
                .ok();
        });
        Chat {
            transcript,
            looks: kept_looks,
            scale: 1.,
            _found: found,
            finding: None,
            plan_open: false,
            activity_line: cx.new(|_| status::Activity::new(provider)),
            target,
            _rows: [emptied, rows_changed],
            head: Map::new(),
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
            writing: None,
            groups_folded: HashSet::new(),
            cursor: None,
            recalled: None,
            replying: None,
            stopping: HashSet::new(),
            answering: HashSet::new(),
            file_menu: None,
        }
    }

    /// What the core answered an attach with. A chat that can't be attached to while it holds
    /// rows from its middle, such as one asleep, asks for its latest rows instead.
    pub fn attached(
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
            ChatAttachment::Resumed { changes, mark } => {
                self.apply(changes, cx);
                self.mark = Some(mark);
            }
            ChatAttachment::Missing => {}
        }
        None
    }

    /// Takes the core's changes numbered `seq`, and says whether some before them went missing.
    pub fn changed(&mut self, seq: u64, changes: Value, cx: &mut App) -> bool {
        if self.detached {
            let changes = read_changes(changes).into_iter().filter(|change| {
                matches!(change, ChatChange::Head { .. } | ChatChange::View { .. })
            });
            self.apply_all(changes.collect(), cx);
            return false;
        }
        let Some(mark) = self.mark.as_mut() else {
            return false;
        };
        if seq <= mark.seq {
            return false;
        }
        if seq != mark.seq + 1 {
            return true;
        }
        mark.seq = seq;
        self.apply(changes, cx);
        false
    }

    fn show(&mut self, view: ChatView, cx: &mut App) {
        self.head = serde_json::to_value(&view.head)
            .ok()
            .and_then(|head| head.as_object().cloned())
            .unwrap_or_default();
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
    pub fn detail(&mut self, tool: String, detail: Value, cx: &mut App) {
        if let Ok(detail) = serde_json::from_value(detail) {
            self.reader.update(cx, |reading, cx| {
                reading.tools.held(tool, detail);
                cx.notify();
            });
        }
    }

    /// The tool calls whose detail to ask the core for again.
    pub fn wanted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.wanted)
    }

    /// Keeps what the rows draw of how the chat stands in step with its head.
    fn follow_head(&self, cx: &mut App) {
        let running = self.running();
        let editable = self.state.as_ref().is_some_and(|head| {
            head.capabilities["editing"] == true
                && head.connection == ChatConnection::Ready
                && !head.running
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
    pub fn replied(&mut self, request: &str, error: Option<String>) {
        if self.replying.as_deref() == Some(request) {
            self.replying = None;
        }
        self.failed(error.filter(|error| !error.contains("no longer pending")));
    }

    /// The core took the answers to `ids`, or says why not, leaving them picked for another go.
    fn took_answers(&mut self, ids: &[String], error: Option<String>) {
        for id in ids {
            self.answering.remove(id);
        }
        if error.is_none() {
            self.picks.retain(|id, _| !ids.contains(id));
            if self.shown.as_ref().is_some_and(|shown| ids.contains(shown)) {
                self.shown = None;
            }
            self.cursor = None;
        }
        self.failed(error);
    }

    /// Answers on their way to the core keep the questions from being answered again.
    pub fn sending_answers(&self) -> bool {
        !self.answering.is_empty()
    }

    /// The core took the ask to stop `task`, or says why not.
    pub fn task_stopped(&mut self, task: &str, error: Option<String>) {
        self.stopping.remove(task);
        self.failed(error);
    }

    fn failed(&mut self, error: Option<String>) {
        if let (Some(error), Some(composing)) = (error, self.composer.as_mut()) {
            composing.refusal = Some(error.into());
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
        let style = self.looks.rescale(scale);
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

    pub fn page(&mut self, page: Value, cx: &mut App) {
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
    pub fn later_page(&mut self, agent: &str, page: Value, cx: &mut App) -> Option<ChatAsk> {
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
    pub fn around_page(&mut self, page: Value, cx: &mut App) {
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
    pub fn latest_page(&mut self, page: Value, cx: &mut App) {
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

    fn apply(&mut self, changes: Value, cx: &mut App) {
        self.apply_all(read_changes(changes), cx);
    }

    fn apply_all(&mut self, changes: Vec<ChatChange>, cx: &mut App) {
        let mut rows = Vec::new();
        for change in changes {
            match change {
                ChatChange::Head { set } => {
                    let mut head = Value::Object(std::mem::take(&mut self.head));
                    merge(&mut head, &set);
                    self.state = serde_json::from_value(head.clone()).ok();
                    self.head_moved = true;
                    self.follow_head(cx);
                    self.head = head.as_object().cloned().unwrap_or_default();
                }
                ChatChange::View { view } => {
                    self.transcript
                        .update(cx, |transcript, cx| transcript.extend(rows.drain(..), cx));
                    self.show(*view, cx);
                }
                ChatChange::Row {
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
                ChatChange::Row { row, within: None } => rows.push(Change::Put(ChatRow::new(row))),
                ChatChange::Part { row, part, within } => {
                    self.wanted.extend(self.reader.read(cx).tools.stale(&part));
                    self.update(&mut rows, row, within, move |held| held.put_part(part), cx)
                }
                ChatChange::Text {
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
                ChatChange::Set { row, set, within } => {
                    self.update(&mut rows, row, within, move |held| held.set(&set), cx)
                }
                ChatChange::Keep { rows: kept } => rows.push(Change::Keep(
                    kept.into_iter().map(SharedString::from).collect(),
                )),
                ChatChange::Drop { from } => rows.push(Change::DropFrom(from.into())),
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

/// The questions the agent put to the person, answered from its chat.
impl Chat {
    /// The open question on show: the one the person chose, else the first still unanswered.
    fn current<'a>(&self, open: &[&'a Question]) -> Option<&'a Question> {
        let chosen = self
            .shown
            .as_ref()
            .and_then(|id| open.iter().find(|question| &question.id == id));
        chosen
            .or_else(|| {
                open.iter()
                    .find(|question| !self.picks.contains_key(&question.id))
            })
            .or(open.first())
            .copied()
    }

    fn is_folded(&self, open: &[&Question]) -> bool {
        open.first()
            .is_some_and(|first| self.folded.as_ref() == Some(&first.id))
    }

    pub fn fold(&mut self, open: &[&Question], fold: bool) {
        self.folded = open.first().filter(|_| fold).map(|first| first.id.clone());
    }

    pub fn toggle_group(&mut self, key: &str) {
        if !self.groups_folded.remove(key) {
            self.groups_folded.insert(key.to_owned());
        }
    }

    pub fn show_question(&mut self, id: &str) {
        self.shown = Some(id.to_owned());
    }

    pub fn write(&mut self, id: &str) {
        self.writing = Some(id.to_owned());
    }

    /// Holds the answer, then moves on to the next question still without one, or sends a lone question.
    fn pick(&mut self, id: &str, pick: Pick, open: &[&Question]) -> Option<Ask> {
        self.picks.insert(id.to_owned(), pick);
        self.move_on(id, open)
    }

    /// Moves on from a question with its answer: to the next still without one, or sends a lone question.
    fn move_on(&mut self, id: &str, open: &[&Question]) -> Option<Ask> {
        self.cursor = None;
        if open.len() == 1 {
            return self.send_answers(open);
        }
        let after = open.iter().skip_while(|question| question.id != id).skip(1);
        if let Some(next) = after
            .chain(open)
            .find(|question| !self.picks.contains_key(&question.id))
        {
            self.shown = Some(next.id.clone());
        }
        None
    }

    /// The choice the arrows are on: where they moved to, else the one picked, else the first.
    fn cursor_at(&self, question: &Question) -> usize {
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
    fn prompts(&self, cx: &App) -> Vec<String> {
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
    pub fn hint(&self, agent: &str, questions: &[Question], resumes: bool) -> String {
        let open: Vec<&Question> = questions
            .iter()
            .filter(|question| question.of(agent, "open"))
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
        if let Some(asker) = self
            .writing
            .as_ref()
            .and_then(|id| questions.iter().find(|question| &question.id == id))
        {
            return format!("Answer {}", asker.name);
        }
        let Some(head) = &self.state else {
            return "Ask about this project".into();
        };
        match head.connection {
            ChatConnection::Error | ChatConnection::Stopped => {
                "Reconnect to continue this conversation".into()
            }
            ChatConnection::Ready if head.running => "Send to queue behind the running turn".into(),
            ChatConnection::Ready if head.commands.is_empty() => "Ask about this project".into(),
            ChatConnection::Ready => "Ask about this project, or type / for commands".into(),
            _ if resumes => "Resuming — this message sends as soon as the session is back".into(),
            _ => "Ask about this project, or type / for commands".into(),
        }
    }

    /// A key the question on show takes while nothing is written: a number picks, the
    /// arrows move, Space ticks, Enter picks, moves on or sends. Says whether it took it.
    pub fn press(&mut self, press: Press, open: &[&Question]) -> (bool, Option<Ask>) {
        let Some(current) = self
            .current(open)
            .filter(|_| !self.is_folded(open) && !self.sending_answers())
        else {
            return (false, None);
        };
        let (id, count, multi, cursor) = (
            current.id.clone(),
            current.choices.len(),
            current.multi,
            self.cursor_at(current),
        );
        let ready = open
            .iter()
            .all(|question| self.picks.contains_key(&question.id));
        match press {
            Press::Up | Press::Down if count > 0 => {
                let step = if press == Press::Up { count - 1 } else { 1 };
                self.cursor = Some((id, (cursor + step) % count));
                (true, None)
            }
            Press::Left | Press::Right if open.len() > 1 => {
                let at = open
                    .iter()
                    .position(|question| question.id == id)
                    .unwrap_or(0);
                let step = if press == Press::Left {
                    open.len() - 1
                } else {
                    1
                };
                self.shown = Some(open[(at + step) % open.len()].id.clone());
                self.cursor = None;
                (true, None)
            }
            Press::Tick if multi && count > 0 => (true, self.choose(&id, cursor, open)),
            Press::Pick if multi => match self.picks.contains_key(&id) {
                false if count > 0 => (true, self.choose(&id, cursor, open)),
                false => (false, None),
                true if ready => (true, self.send_answers(open)),
                true => (true, self.move_on(&id, open)),
            },
            Press::Pick => {
                let already = matches!(self.picks.get(&id).map(|pick| &pick.answer), Some(Answer::Choice(choice)) if *choice == cursor);
                if ready && (already || count == 0) {
                    (true, self.send_answers(open))
                } else if count > 0 {
                    (true, self.choose(&id, cursor, open))
                } else {
                    (false, None)
                }
            }
            Press::Choose(ix) if ix < count => {
                self.cursor = Some((id.clone(), ix));
                (true, self.choose(&id, ix, open))
            }
            _ => (false, None),
        }
    }

    /// Picks a choice, or on a question that takes several, ticks or unticks it.
    pub fn choose(&mut self, id: &str, choice: usize, open: &[&Question]) -> Option<Ask> {
        let question = open.iter().find(|question| question.id == id)?;
        let label = |ix: &usize| {
            question
                .choices
                .get(*ix)
                .map_or("", |choice| choice.label.as_str())
        };
        if !question.multi {
            return self.pick(
                id,
                Pick {
                    answer: Answer::Choice(choice),
                    shown: label(&choice).to_owned(),
                },
                open,
            );
        }
        let mut ticked = match self.picks.remove(id) {
            Some(Pick {
                answer: Answer::Choices(ticked),
                ..
            }) => ticked,
            _ => Vec::new(),
        };
        match ticked.iter().position(|held| *held == choice) {
            Some(at) => {
                ticked.remove(at);
            }
            None => {
                ticked.push(choice);
                ticked.sort_unstable();
            }
        }
        if !ticked.is_empty() {
            let shown = ticked.iter().map(label).collect::<Vec<_>>().join(", ");
            self.picks.insert(
                id.to_owned(),
                Pick {
                    answer: Answer::Choices(ticked),
                    shown,
                },
            );
        }
        None
    }

    pub fn keep_secret(&mut self, text: String, open: &[&Question]) -> Option<Ask> {
        let question = self
            .current(open)
            .filter(|question| question.secret && !self.is_folded(open))?;
        let id = question.id.clone();
        self.pick(
            &id,
            Pick {
                answer: Answer::Text(text),
                shown: String::new(),
            },
            open,
        )
    }

    /// Sends every open question's answer together, once each has one.
    pub fn send_answers(&mut self, open: &[&Question]) -> Option<Ask> {
        if open.is_empty()
            || !open
                .iter()
                .all(|question| self.picks.contains_key(&question.id))
        {
            return None;
        }
        let answers: Vec<Value> = open
            .iter()
            .filter_map(|question| {
                let id = &question.id;
                Some(match &self.picks.get(id)?.answer {
                    Answer::Choice(choice) => json!({ "id": id, "choice": choice }),
                    Answer::Choices(choices) => json!({ "id": id, "choices": choices }),
                    Answer::Text(answer) => json!({ "id": id, "answer": answer }),
                })
            })
            .collect();
        Some(Ask::Call {
            method: "answerQuestions",
            params: json!({ "answers": answers }),
        })
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
        if let Some(id) = self.writing.take() {
            return Some(Ask::Call {
                method: "answerQuestion",
                params: json!({ "id": id, "answer": answer_text(&text, &paths, &context) }),
            });
        }
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
            let message = json!({ "text": text, "paths": paths, "context": context });
            return Some(self.steer_queued(agent, Some(message)));
        }
        Some(Ask::Chat(ChatAsk::Prompt {
            agent: agent.to_owned(),
            text,
            paths,
            context,
            attach: self.mark.is_none(),
        }))
    }

    /// A turn is running that takes words mid-turn.
    fn steers(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|head| head.running && head.capabilities["steering"] == true)
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
    fn steer_queued(&self, agent: &str, message: Option<Value>) -> Ask {
        let ids: Vec<&str> = self.state.as_ref().map_or(Vec::new(), |head| {
            head.queued
                .iter()
                .map(|queued| queued.id.as_str())
                .collect()
        });
        Ask::Call {
            method: "steerQueued",
            params: json!({ "agentId": agent, "ids": ids, "message": message }),
        }
    }
}

fn read_changes(changes: Value) -> Vec<ChatChange> {
    serde_json::from_value(changes).unwrap_or_else(|error| {
        eprintln!("sikemux: the core's chat changes do not read: {error}");
        Vec::new()
    })
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
        (chat.looks.files.borrow_mut()).place(&cwd, &chat.target, &chat.reader);
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
                .pt(px(44.))
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
            chat.transcript
                .clone()
                .cached(StyleRefinement::default().size_full()),
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
        .children(chat.transcript.read(cx).away().then(|| {
            let latest = cx.listener({
                let agent = agent.id.clone();
                move |bench, _, _, cx| bench.show_latest(&agent, cx)
            });
            div()
                .absolute()
                .bottom(px(8.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .rounded(theme::RADIUS_2)
                        .border_1()
                        .border_color(Tokens::get(cx).line)
                        .bg(Tokens::get(cx).surface_composer)
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
        }))
        .children(welcome.map(|welcome| crate::home::centred(welcome).absolute().inset_0()));
    let open: Vec<&Question> = bench
        .questions
        .iter()
        .filter(|question| question.of(&agent.id, "open"))
        .collect();
    let permissions = chat
        .state
        .as_ref()
        .map_or(&[][..], |head| head.permissions.as_slice());
    let permissions: Vec<_> = permissions
        .iter()
        .map(|ask| permission(&agent.id, ask, chat.replying.is_some(), cx))
        .collect();
    let assumed: Vec<_> = bench
        .questions
        .iter()
        .filter(|question| question.of(&agent.id, "assumed"))
        .map(|question| assumption(question, chat.answering.contains(&question.id), cx))
        .collect();
    let answered: Vec<_> = bench
        .questions
        .iter()
        .filter(|question| question.of(&agent.id, "answered"))
        .map(|question| {
            let card = answered_question(
                None,
                &question.text,
                question.answer.as_deref().unwrap_or_default(),
                question.secret,
                Tokens::get(cx),
            );
            column().child(div().w_full().max_w(px(756.)).pt(px(12.)).child(card))
        })
        .collect();
    let mut groups: Vec<AnyElement> = questions(&agent.id, chat, &open, cx).into_iter().collect();
    groups.extend(live::subagents(agent, chat, cx));
    groups.extend(live::tasks(agent, chat, cx));
    groups.extend(live::queued(&agent.id, chat, cx));
    let stack = live::stack(groups, Tokens::get(cx));
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
        .when(composing, |pane| attach::takes_drops(pane, id, cx))
        .when(asking, |pane| {
            question_keys(pane, id, true, &cx.weak_entity())
        })
        .child(rows)
        .children(file_menu)
        .child(chat.activity_line.clone())
        .children(status::plan(agent, chat, cx))
        .children(status::trouble(bench, agent, chat, cx))
        .children(permissions)
        .children(answered)
        .children(assumed)
        .children(stack)
        .children(bench.read_only(id).map(|why| read_only_line(why, cx)))
        .children(chat.composer.as_ref().map(|composing| {
            composing
                .view
                .update(cx, |view, _| view.agent = Some(agent.clone()));
            composing.view.clone().into_element().isolated()
        }))
        .when(composing, |pane| pane.child(attach::drop_hint(cx)));
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

/// Hands a key's action to the question on show; one it does not take goes on to the draft.
/// The keys the open question on show takes: its choices by number and the arrows, and with
/// `all`, space to tick and return to pick, which only the chat itself takes.
pub fn question_keys<E: InteractiveElement + FluentBuilder>(
    keyed: E,
    agent: &str,
    all: bool,
    bench: &WeakEntity<Workbench>,
) -> E {
    keyed
        .key_context("QuestionDock")
        .on_action(pressed(
            agent,
            |choose: &Choose| Press::Choose(choose.0),
            bench,
        ))
        .on_action(pressed(agent, |_: &PreviousChoice| Press::Up, bench))
        .on_action(pressed(agent, |_: &NextChoice| Press::Down, bench))
        .on_action(pressed(agent, |_: &PreviousQuestion| Press::Left, bench))
        .on_action(pressed(agent, |_: &NextQuestion| Press::Right, bench))
        .when(all, |keyed| {
            keyed
                .on_action(pressed(agent, |_: &Tick| Press::Tick, bench))
                .on_action(pressed(agent, |_: &Confirm| Press::Pick, bench))
        })
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
                .questions
                .iter()
                .filter(|question| question.of(&agent, "open"))
                .collect();
            let chat = bench.chats.get_mut(&agent)?;
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

/// Brings an older or newer sent prompt into the draft; the arrow goes on to the text when
/// there is none to bring.
/// Backspace just after a chip selects it, as on the Mac, so only a second press removes it.
fn select_chip(
    agent: &str,
    cx: &mut Context<Workbench>,
) -> impl Fn(&gpui_kit::component::input::Backspace, &mut Window, &mut App) {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |_, _, cx| {
        let draft = this
            .update(cx, |bench, _| {
                Some(bench.chats.get(&agent)?.composer.as_ref()?.draft.clone())
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

fn recall<A: gpui::Action>(
    agent: &str,
    older: bool,
    cx: &mut Context<Workbench>,
) -> impl Fn(&A, &mut Window, &mut App) + use<A> {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |_: &A, window: &mut Window, cx: &mut App| {
        let brought = this.update(cx, |bench, cx| {
            let chat = bench.chats.get_mut(&agent)?;
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
fn offered<A: gpui::Action>(
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
fn stop_turn(
    agent: &str,
    cx: &mut Context<Workbench>,
) -> impl Fn(&StopTurn, &mut Window, &mut App) + use<> {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |_: &StopTurn, window: &mut Window, cx: &mut App| {
        let stopped = this.update(cx, |bench, cx| {
            let chat = bench.chats.get(&agent)?;
            let empty = chat.composer.as_ref()?.draft.read(cx).value().is_empty();
            (chat.running() && empty).then(|| bench.stop(&agent, window, cx))
        });
        if !matches!(stopped, Ok(Some(()))) {
            cx.propagate();
        }
    }
}

/// Centred on the transcript's column.
fn column() -> gpui_kit::Div {
    div().w_full().px(px(22.)).flex().justify_center()
}

/// What stands where the composer would, in a chat that can only be read here.
fn read_only_line(why: &str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    column()
        .pt(px(8.))
        .pb(px(14.))
        .child(
            div()
                .w_full()
                .max_w(px(756.))
                .px(px(14.))
                .py(px(10.))
                .rounded(theme::RADIUS_2)
                .border_1()
                .border_color(t.line)
                .bg(t.surface_composer)
                .text_color(t.text_secondary)
                .child(why.to_owned()),
        )
        .into_any_element()
}

fn permission(
    agent: &str,
    ask: &PermissionAsk,
    busy: bool,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let rejects = |kind: &str| kind.starts_with("reject");
    let mut answers: Vec<(String, Option<String>, bool)> = ask
        .options
        .iter()
        .map(|option| {
            (
                option.name.clone(),
                Some(option.option_id.clone()),
                !rejects(&option.kind),
            )
        })
        .collect();
    if !ask.options.iter().any(|option| rejects(&option.kind)) {
        answers.push(("Cancel".into(), None, false));
    }
    let buttons: Vec<_> = answers
        .into_iter()
        .map(|(name, option, allow)| {
            let (agent, request) = (agent.to_owned(), ask.request_id.clone());
            let id = SharedString::from(format!(
                "permission-{request}-{}",
                option.as_deref().unwrap_or("")
            ));
            BaseButton::new(id)
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .border_1()
                .border_color(if allow {
                    Hsla { a: 0.45, ..t.warn }
                } else {
                    t.line
                })
                .rounded(theme::RADIUS_2)
                .bg(t.line_soft)
                .when(!busy, |button| {
                    button.hover(move |style| style.bg(t.surface_chosen))
                })
                .text_size(crate::theme::ui_text(10.5))
                .text_color(if allow {
                    t.text_primary
                } else {
                    t.text_tertiary
                })
                .disabled(busy)
                .on_click(cx.listener(move |bench, _, _, cx| {
                    if let Some(chat) = bench.chats.get_mut(&agent) {
                        chat.replying = Some(request.clone());
                    }
                    bench.parts.redraw(&bench.parts.stage, cx);
                    bench.ask_chat(ChatAsk::Permission {
                        agent: agent.clone(),
                        request: request.clone(),
                        option: option.clone(),
                    })
                }))
                .child(name)
        })
        .collect();
    let copy = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            ui::text(ask.tool_call.title.clone(), 12., FontWeight::SEMIBOLD)
                .text_color(t.text_primary),
        )
        .child(
            ui::text(
                "Agent needs permission before this tool can continue.",
                11.,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary),
        );
    column().child(
        div()
            .w_full()
            .max_w(px(756.))
            .mt(px(12.))
            .mb(px(12.))
            .p(px(12.))
            .border_1()
            .border_color(Hsla { a: 0.44, ..t.warn })
            .rounded(theme::RADIUS_3)
            .bg(Hsla { a: 0.07, ..t.warn })
            .child(
                div()
                    .flex()
                    .gap(px(9.))
                    .child(
                        ui::icon(Icon::ShieldBolt, 16.)
                            .flex_none()
                            .text_color(t.warn),
                    )
                    .child(copy),
            )
            .child(
                div()
                    .mt(px(11.))
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .children(buttons),
            ),
    )
}

/// A plain text button, for answering in a line of text.
fn word(id: impl Into<SharedString>, label: &'static str, t: &Tokens) -> BaseButton {
    let hover = t.text_primary;
    BaseButton::new(gpui_kit::ElementId::Name(id.into()))
        .px(px(4.))
        .rounded(theme::RADIUS_1)
        .text_color(t.text_secondary)
        .hover(move |style| style.text_color(hover))
        .child(label)
}

/// A question the agent went on without, on the assumption it stated.
fn assumption(question: &Question, busy: bool, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let (fine, change) = (question.id.clone(), question.id.clone());
    let (agent, assumed) = (
        question.agent_id.clone(),
        question.assumption.clone().unwrap_or_default(),
    );
    let said = format!("{} assumed {assumed}", question.name);
    let bold = HighlightStyle {
        color: Some(t.text_primary),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    column().child(
        div()
            .w_full()
            .max_w(px(756.))
            .min_h(px(26.))
            .pt(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(crate::theme::ui_text(12.))
            .text_color(t.text_tertiary)
            .child(
                ui::icon(Icon::Check, 14.)
                    .flex_none()
                    .text_color(t.text_disabled),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(StyledText::new(said).with_highlights([(0..question.name.len(), bold)])),
            )
            .child("·")
            .child(
                word(format!("fine-{fine}"), "Fine", &t)
                    .disabled(busy)
                    .on_click(cx.listener({
                        let agent = agent.clone();
                        move |bench, _, _, cx| {
                            let params = json!({ "id": fine, "fine": true });
                            let ask = Ask::Call {
                                method: "answerQuestion",
                                params,
                            };
                            bench.ask_answering(&agent, ask, cx);
                        }
                    })),
            )
            .child(
                word(format!("change-{change}"), "Change", &t)
                    .disabled(busy)
                    .on_click(cx.listener(move |bench, _, window, cx| {
                        if let Some(chat) = bench.chats.get_mut(&agent) {
                            chat.write(&change);
                            if let Some(composing) = &chat.composer {
                                window.focus(&composing.draft.focus_handle(cx), cx);
                            }
                        }
                    })),
            ),
    )
}

/// The questions the agent waits on, at the top of the live work over the composer: a tab
/// for each, what the one on show asks, and its choices.
fn questions(
    agent: &str,
    chat: &Chat,
    open: &[&Question],
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let current = chat.current(open)?;
    let t = Tokens::get(cx).clone();
    let count = open.len();
    let body = if chat.is_folded(open) {
        let agent = agent.to_owned();
        let peek = open
            .iter()
            .enumerate()
            .map(|(ix, question)| question.title(ix, count))
            .collect::<Vec<_>>()
            .join(" · ");
        BaseButton::new("questions-unfold")
            .w_full()
            .px(px(11.))
            .py(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(crate::theme::ui_text(10.))
            .text_color(t.text_disabled)
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.answer(
                    &agent,
                    |chat, open| {
                        chat.fold(open, false);
                        None
                    },
                    cx,
                );
                cx.notify();
            }))
            .child(if count == 1 { "QUESTION" } else { "QUESTIONS" })
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_color(t.text_tertiary)
                    .child(count.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::ui_text(11.))
                    .child(peek),
            )
            .child(ui::icon(Icon::Chevron, 10.).text_color(t.text_disabled))
            .into_any_element()
    } else {
        asking(agent, chat, open, current, &t, cx).into_any_element()
    };
    Some(body)
}

fn asking(
    agent: &str,
    chat: &Chat,
    open: &[&Question],
    current: &Question,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let count = open.len();
    let edge = Hsla {
        a: 0.06,
        ..t.text_primary
    };
    let tabs: Vec<_> = open
        .iter()
        .enumerate()
        .map(|(ix, question)| {
            let on = question.id == current.id;
            let (agent, id) = (agent.to_owned(), question.id.clone());
            let mark = if chat.picks.contains_key(&question.id) {
                ui::icon(Icon::Check, 10.)
                    .text_color(t.text_disabled)
                    .into_any_element()
            } else {
                div()
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(10.))
                    .text_color(t.text_disabled)
                    .child((ix + 1).to_string())
                    .into_any_element()
            };
            let hover = t.surface_hover;
            BaseButton::new(SharedString::from(format!("question-tab-{id}")))
                .h(px(24.))
                .px(px(9.))
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(theme::RADIUS_2)
                .text_size(crate::theme::ui_text(11.))
                .map(|tab| {
                    if on {
                        tab.bg(t.surface_chosen).text_color(t.text_primary)
                    } else {
                        tab.border_1()
                            .border_color(edge)
                            .text_color(t.text_secondary)
                    }
                })
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |bench, _, _, cx| {
                    bench.answer(
                        &agent,
                        |chat, _| {
                            chat.show_question(&id);
                            None
                        },
                        cx,
                    );
                    cx.notify();
                }))
                .child(mark)
                .child(div().min_w_0().truncate().child(question.title(ix, count)))
        })
        .collect();
    let ready = open
        .iter()
        .all(|question| chat.picks.contains_key(&question.id));
    let sending = chat.sending_answers();
    let send = (count > 1 || current.multi).then(|| {
        let agent = agent.to_owned();
        BaseButton::new("questions-send")
            .disabled(!ready || sending)
            .h(px(22.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(theme::RADIUS_2)
            .text_size(crate::theme::ui_text(11.))
            .map(|send| {
                if ready {
                    send.bg(t.surface_chosen).text_color(t.text_primary)
                } else {
                    send.border_1()
                        .border_color(edge)
                        .text_color(t.text_disabled)
                }
            })
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.answer(&agent, |chat, open| chat.send_answers(open), cx);
                cx.notify();
            }))
            .child("Send")
            .child(
                div()
                    .pl(px(6.))
                    .border_l_1()
                    .border_color(t.line)
                    .text_size(crate::theme::ui_text(9.5))
                    .text_color(t.text_disabled)
                    .child("⌘↵"),
            )
    });
    let fold = {
        let agent = agent.to_owned();
        let hover = t.surface_hover;
        BaseButton::new("questions-fold")
            .accessibility_label("Fold the questions away")
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS_2)
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.answer(
                    &agent,
                    |chat, open| {
                        chat.fold(open, true);
                        None
                    },
                    cx,
                );
                cx.notify();
            }))
            .child(
                ui::icon(Icon::Chevron, 10.)
                    .text_color(t.text_disabled)
                    .with_transformation(Transformation::rotate(Radians(FRAC_PI_2))),
            )
    };
    let head = div()
        .flex()
        .items_center()
        .gap(px(4.))
        .px(px(6.))
        .pt(px(6.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .gap(px(4.))
                .overflow_hidden()
                .children(tabs),
        )
        .children(send)
        .child(fold);
    let mut asked = current.text.clone();
    let plain = asked.len();
    if current.multi {
        asked.push_str(" Pick any.");
    }
    if let Some(via) = &current.via_note {
        asked.push_str(&format!(" · {via}"));
    }
    let faint = HighlightStyle {
        color: Some(t.text_disabled),
        ..Default::default()
    };
    let text = div()
        .px(px(12.))
        .pt(px(10.))
        .pb(px(4.))
        .text_size(crate::theme::ui_text(13.5))
        .line_height(relative(1.5))
        .text_color(t.text_primary)
        .child(crate::ui::selectable::selectable(
            SharedString::from(format!("question-text-{}", current.id)),
            asked.clone(),
            [(plain..asked.len(), faint)],
        ));
    let picked = chat.picks.get(&current.id);
    let cursor = chat
        .cursor
        .as_ref()
        .filter(|(id, _)| *id == current.id)
        .map(|(_, at)| *at);
    let chosen = |ix: usize| match picked.map(|pick| &pick.answer) {
        Some(Answer::Choice(choice)) => *choice == ix,
        Some(Answer::Choices(choices)) => choices.contains(&ix),
        _ => false,
    };
    let mut choices: Vec<AnyElement> = current
        .choices
        .iter()
        .enumerate()
        .map(|(ix, choice)| {
            let (agent, id) = (agent.to_owned(), current.id.clone());
            let key = if current.multi {
                tick(chosen(ix), t)
            } else {
                div()
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child((ix + 1).to_string())
                    .into_any_element()
            };
            let (label, recommended) = recommended(&choice.label);
            let label = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(8.))
                .child(
                    ui::text(label.to_owned(), 13., FontWeight::MEDIUM)
                        .line_height(crate::theme::ui_text(19.)),
                )
                .when(recommended, |label| {
                    label.child(
                        ui::text("recommended", 11., FontWeight::NORMAL)
                            .text_color(t.text_disabled),
                    )
                });
            let detail = choice.detail.clone().map(|detail| {
                div()
                    .text_size(crate::theme::ui_text(12.))
                    .line_height(relative(1.45))
                    .text_color(t.text_tertiary)
                    .child(detail)
            });
            answer_row(
                SharedString::from(format!("question-{id}-{ix}")),
                chosen(ix),
                cursor == Some(ix),
                key,
                div().child(label).children(detail),
                t,
            )
            .disabled(chat.sending_answers())
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.answer(&agent, |chat, open| chat.choose(&id, ix, open), cx);
                cx.notify();
            }))
            .into_any_element()
        })
        .collect();
    if let Some(Pick {
        answer: Answer::Text(_),
        shown,
    }) = picked.filter(|_| !current.secret)
    {
        let key = ui::icon(Icon::Pencil, 11.).mt(px(4.)).into_any_element();
        let typed = ui::text(shown.clone(), 13., FontWeight::MEDIUM)
            .line_height(crate::theme::ui_text(19.));
        choices.push(
            answer_row(
                SharedString::from(format!("question-{}-typed", current.id)),
                true,
                false,
                key,
                typed,
                t,
            )
            .into_any_element(),
        );
    }
    let secret = match (current.secret, picked) {
        (false, _) => None,
        (true, Some(_)) => Some(kept(t).into_any_element()),
        (true, None) => secret(agent, current, chat, t, cx).map(IntoElement::into_any_element),
    };
    div()
        .flex()
        .flex_col()
        .child(head)
        .child(text)
        .when(!choices.is_empty(), |body| {
            body.child(
                div()
                    .px(px(12.))
                    .pt(px(6.))
                    .pb(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(choices),
            )
        })
        .children(secret)
}

/// A choice's label without the "(recommended)" the agent put after it, and whether it did.
fn recommended(label: &str) -> (&str, bool) {
    const MARK: &str = "(recommended)";
    let trimmed = label.trim_end();
    let cut = trimmed
        .len()
        .checked_sub(MARK.len())
        .filter(|&at| trimmed.is_char_boundary(at));
    match cut {
        Some(at) if trimmed[at..].eq_ignore_ascii_case(MARK) => (trimmed[..at].trim_end(), true),
        _ => (label, false),
    }
}

fn answer_row(
    id: SharedString,
    chosen: bool,
    cursor: bool,
    key: AnyElement,
    body: impl IntoElement,
    t: &Tokens,
) -> BaseButton {
    let hover = t.surface_chosen;
    BaseButton::new(id)
        .w_full()
        .pt(px(7.))
        .pb(px(8.))
        .pl(px(4.))
        .pr(px(8.))
        .flex()
        .items_start()
        .rounded(theme::RADIUS_2)
        .bg(match (chosen, cursor) {
            (true, _) => Hsla {
                a: 0.14,
                ..t.text_primary
            },
            (false, true) => t.surface_chosen,
            (false, false) => t.surface_hover,
        })
        .text_color(if chosen {
            t.text_primary
        } else {
            t.text_secondary
        })
        .hover(move |style| style.bg(hover))
        .child(
            div()
                .w(px(22.))
                .flex_none()
                .flex()
                .justify_center()
                .line_height(crate::theme::ui_text(19.))
                .text_color(t.text_disabled)
                .child(key),
        )
        .child(div().flex_1().min_w_0().flex().flex_col().child(body))
}

/// The box of a choice on a question that takes several.
fn tick(on: bool, t: &Tokens) -> AnyElement {
    let tick = div()
        .mt(px(3.5))
        .size(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.));
    match on {
        true => tick
            .bg(t.text_primary)
            .child(ui::icon(Icon::Check, 9.).text_color(t.surface_sunken)),
        false => tick.border_1().border_color(Hsla {
            a: 0.3,
            ..t.text_primary
        }),
    }
    .into_any_element()
}

fn kept(t: &Tokens) -> impl IntoElement {
    div()
        .px(px(12.))
        .pt(px(4.))
        .pb(px(10.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(crate::theme::ui_text(11.))
        .text_color(t.text_tertiary)
        .child(ui::icon(Icon::Shield, 12.).text_color(t.text_tertiary))
        .child("Answer kept, sent with the rest")
}

/// The masked field a secret answer is typed into, which keeps nothing once it is picked.
fn secret(
    agent: &str,
    question: &Question,
    chat: &Chat,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Option<impl IntoElement> {
    let field = chat.composer.as_ref()?.secret.clone();
    let agent = agent.to_owned();
    let done = Button::new("secret-done", "Done", Variant::Outline)
        .disabled(chat.sending_answers())
        .on_click(cx.listener(move |bench, _, window, cx| bench.keep_secret(&agent, window, cx)));
    Some(
        div()
            .px(px(12.))
            .pt(px(4.))
            .pb(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(30.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .border_1()
                            .border_color(t.line)
                            .rounded(theme::RADIUS_2)
                            .bg(t.surface_sunken)
                            .text_size(crate::theme::ui_text(12.))
                            .child(Input::new(&field).appearance(false).bordered(false)),
                    )
                    .child(done),
            )
            .children(question.secret_note.clone().map(|note| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(crate::theme::ui_text(11.))
                    .text_color(t.text_tertiary)
                    .child(ui::icon(Icon::Shield, 12.).text_color(t.text_tertiary))
                    .child(note)
            })),
    )
}

/// The composer, with what this chat lets the person change.
/// The composer of one chat as a view of its own, mounted isolated, so a key typed in it draws
/// only it again rather than the chat around it.
pub struct ComposerView {
    pub bench: WeakEntity<Workbench>,
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
                let chat = bench.chats.get(&agent.id)?;
                mounted_composer(bench, &agent, chat, cx)
            })
            .ok()
            .flatten()
            .unwrap_or_else(|| div().into_any_element())
    }
}

/// The composer with the keys it takes: the arrows through sent prompts, Escape to stop the
/// turn, and the menu's keys while it offers something.
fn mounted_composer(
    bench: &Workbench,
    agent: &AgentRow,
    chat: &Chat,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let id = &agent.id;
    composer(bench, agent, chat, cx)
        .map(|(composer, offering)| {
            div()
                .key_context("ChatComposer")
                .capture_action(select_chip(id, cx))
                .on_action(recall::<OlderPrompt>(id, true, cx))
                .on_action(recall::<NewerPrompt>(id, false, cx))
                .on_action(stop_turn(id, cx))
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

fn composer(
    bench: &Workbench,
    agent: &AgentRow,
    chat: &Chat,
    cx: &mut Context<Workbench>,
) -> Option<(Composer, bool)> {
    let composing = chat.composer.as_ref()?;
    let dock = composing
        .menu
        .as_ref()
        .filter(|menu| menu.answers(composing.draft.read(cx)))
        .and_then(|menu| {
            let repo = (bench.project())
                .and_then(|project| project.code_host.as_ref())
                .map(|host| host.repo.clone());
            mention::dock(bench, &agent.id, menu, repo.as_deref(), cx)
        });
    let offering = dock.is_some();
    let head = chat.state.as_ref();
    let kinds = bench.read("catalog").and_then(Value::as_array);
    let kind = kinds.and_then(|kinds| {
        kinds
            .iter()
            .find(|kind| kind["type"] == agent.provider.as_str())
    });
    let label = kind
        .and_then(|kind| kind["label"].as_str())
        .unwrap_or(&agent.provider)
        .to_owned();
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
        cx,
    )
    .map(IntoElement::into_any_element);
    let mut composer = Composer::new(&composing.draft, &agent.provider, label, on_send, on_stop)
        .answering(answering(bench, &agent.id, chat, cx))
        .refusal(composing.refusal.clone())
        .text_scale(chat.scale)
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
    let pick: crate::model_picker::Set = {
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
        let moving: crate::model_picker::Move = {
            let (this, agent) = (this.clone(), agent.id.clone());
            Rc::new(move |to: crate::model_picker::Moving, _, cx| {
                this.update(cx, |bench, _| bench.move_chat(&agent, to)).ok();
            })
        };
        let started = head.is_some_and(|head| head.connection == ChatConnection::Ready);
        let read = matches!(
            kind.and_then(|kind| kind["chat"].as_str()),
            Some("claude" | "codex")
        );
        let picked = crate::model_picker::Picked {
            provider: agent.provider.clone(),
            profile: saved.profile_id.clone(),
            model: model.clone(),
            model_provider: saved.model_provider.clone(),
            locked: !fresh,
            pending: match (started, read) {
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
        let driver = kind.and_then(|kind| kind["chat"].as_str());
        matches!(driver, Some("claude" | "codex")).then(|| ConfigPicker {
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
    if kind.is_some_and(|kind| kind["bypass"] == true) {
        let on = head.is_some_and(|head| head.permission_mode == "bypass");
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
fn strip(
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
    let host = match bench.chat_target(&agent.id).place() {
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
        path: tilde(folder).into(),
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

impl Workbench {
    /// How many tool calls of the chat in front have their detail held, and how many are open.
    pub fn tool_details(&self, cx: &App) -> Option<(usize, usize)> {
        let chat = self.chats.get(&self.agent()?.id)?;
        Some(chat.reader.read(cx).tools.counts())
    }

    /// Moves a chat that has not started to another agent, account or provider.
    pub fn move_chat(&self, agent: &str, to: crate::model_picker::Moving) {
        let account = to
            .profile
            .as_deref()
            .and_then(|id| self.accounts.profile(id));
        let title = account.map_or(to.provider.as_str(), |account| &account.name);
        let patch = json!({
            "provider": to.provider,
            "profileId": to.profile,
            "model": to.model,
            "modelProvider": to.model_provider,
            "effort": null,
        });
        self.start_as(agent, patch, title);
    }

    /// The model menu of the chat in front, as its composer offers it: the model chosen and
    /// the ones it lists.
    pub fn front_models(&self) -> Option<(String, Vec<String>)> {
        let chat = self.chats.get(&self.agent()?.id)?;
        let model = chat.state.as_ref()?.model.as_ref()?;
        let offered = model.choices.iter().map(|choice| choice.value.clone());
        Some((model.current.clone(), offered.collect()))
    }

    /// Chooses `value` in the model menu of the chat in front, as a click on its row does.
    pub fn choose_front_model(&self, value: &str) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        let Some(config) = self
            .chats
            .get(&agent)
            .and_then(|chat| Some(chat.state.as_ref()?.model.as_ref()?.id.clone()))
        else {
            return;
        };
        self.ask_chat(ChatAsk::Config {
            agent,
            config,
            value: value.to_owned(),
        });
    }

    /// Moves the chat in front to `provider` on its chosen account, as the model menu's chip for
    /// the agent does.
    pub fn move_front_to(&self, provider: &str) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        let to = crate::model_picker::Moving {
            provider: provider.to_owned(),
            profile: self
                .accounts
                .current(provider)
                .map(|profile| profile.id.clone()),
            model: None,
            model_provider: None,
        };
        self.move_chat(&agent, to);
    }

    /// Opens the first tool call of the chat in front, as a click on its row does.
    pub fn open_first_tool(&mut self, cx: &mut App) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        let Some(chat) = self.chats.get(&agent) else {
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
        let Some(chat) = self.chats.get_mut(agent) else {
            return;
        };
        let agent = agent.to_owned();
        let dismissed = cx.subscribe(&menu, move |bench, _, _: &gpui_kit::DismissEvent, cx| {
            if let Some(chat) = bench.chats.get_mut(&agent) {
                chat.file_menu = None;
            }
            bench.parts.redraw(&bench.parts.stage, cx);
        });
        chat.file_menu = Some(files::FileMenu {
            at,
            menu,
            _dismissed: dismissed,
        });
        self.parts.redraw(&self.parts.stage, cx);
    }

    /// The chat in front while its question asks and nothing else holds the keys, as when the
    /// person clicked away from every field, so the question takes its keys from anywhere.
    pub fn asking_freely(&self, window: &Window) -> Option<String> {
        if !self.focus.is_focused(window) {
            return None;
        }
        self.front_asks()
    }

    /// The chat in front, while it shows a question its keys answer.
    pub fn front_asks(&self) -> Option<String> {
        let agent = self.agent()?;
        let chat = self.chats.get(&agent.id)?;
        let open: Vec<&Question> = self
            .questions
            .iter()
            .filter(|question| question.of(&agent.id, "open"))
            .collect();
        let asking = chat.current(&open).is_some() && !chat.is_folded(&open);
        (asking && !chat.sending_answers()).then_some(agent.id)
    }

    /// Asks what a change to `agent`'s questions asks. Answers wait on the core, the questions
    /// they answer held as sending, then leave the open ones without waiting for its next list.
    pub fn ask_answering(&mut self, agent: &str, ask: Ask, cx: &mut Context<Self>) {
        let (method, params) = match ask {
            Ask::Call {
                method: method @ ("answerQuestion" | "answerQuestions"),
                params,
            } => (method, params),
            ask => return self.chat_target(agent).ask(ask),
        };
        let ids: Vec<String> = params["id"]
            .as_str()
            .into_iter()
            .chain(
                params["answers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|answer| answer["id"].as_str()),
            )
            .map(str::to_owned)
            .collect();
        if let Some(chat) = self.chats.get_mut(agent) {
            chat.answering.extend(ids.iter().cloned());
            if let Some(composing) = chat.composer.as_mut() {
                composing.refusal = None;
            }
        }
        self.parts.redraw(&self.parts.stage, cx);
        let answered = self.chat_target(agent).request(method, params);
        let agent = agent.to_owned();
        cx.spawn(async move |bench, cx| {
            let error = answered.await.err();
            bench
                .update(cx, |bench, cx| {
                    if error.is_none() {
                        bench.keep_questions(|question| {
                            question.state == "answered" || !ids.contains(&question.id)
                        });
                    }
                    if let Some(chat) = bench.chats.get_mut(&agent) {
                        chat.took_answers(&ids, error);
                    }
                    bench.parts.redraw(&bench.parts.stage, cx);
                })
                .ok();
        })
        .detach();
    }
}

impl Workbench {
    /// Starts `agent`, which has not started yet, again as `patch` changes it, titled `title`
    /// unless the person named it.
    pub fn start_as(&self, agent: &str, mut patch: Value, title: &str) {
        let renamed = self.shown().is_some_and(|view| {
            view.workspace
                .agents
                .iter()
                .any(|saved| saved.id == agent && saved.renamed)
        });
        patch["resumeId"] = Value::Null;
        if !renamed {
            patch["title"] = title.into();
        }
        self.ask_chat(ChatAsk::Move {
            agent: agent.to_owned(),
            patch,
        });
    }

    /// Lets `agent` run without asking, or has it ask again, when its kind of agent can.
    /// Lets the agent of the tab in front, or of its desk, act without asking, or has it ask
    /// again, wherever the keys are.
    pub fn toggle_yolo_in_front(&self) {
        let Some(tab) = self.tab() else {
            return;
        };
        let agent = crate::workbench::shows(tab, sikemux_wire::protocol::PaneKind::Agent)
            .or_else(|| crate::workbench::shows(tab, sikemux_wire::protocol::PaneKind::Desk));
        if let Some(agent) = agent {
            self.toggle_yolo(agent);
        }
    }

    pub fn toggle_yolo(&self, agent: &str) {
        let Some(saved) = self
            .shown()
            .and_then(|view| view.workspace.agents.iter().find(|saved| saved.id == agent))
        else {
            return;
        };
        let can = self
            .read("catalog")
            .and_then(Value::as_array)
            .and_then(|kinds| {
                kinds
                    .iter()
                    .find(|kind| kind["type"] == saved.provider.as_str())
            })
            .is_some_and(|kind| kind["bypass"] == true);
        if can {
            let on = saved.permission_mode == "bypass";
            self.switch_mode(agent, if on { "workspace-write" } else { "bypass" });
        }
    }
}

/// The chip over a draft that answers a question in the person's own words: who asked and
/// what, and a way back to writing a message.
fn answering(
    bench: &Workbench,
    agent: &str,
    chat: &Chat,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let id = chat.writing.as_ref()?;
    let question = bench.questions.iter().find(|question| &question.id == id)?;
    let provider = bench.shown().and_then(|view| {
        view.workspace
            .agents
            .iter()
            .find(|saved| saved.id == question.agent_id)
            .map(|saved| saved.provider.clone())
    });
    let t = Tokens::get(cx).clone();
    let (this, agent) = (cx.weak_entity(), agent.to_owned());
    let label = format!("Answering {} · {}", question.name, question.text);
    Some(
        gpui_kit::component::attachment::Attachment::new()
            .id("composer-answering")
            .with_size(gpui_kit::component::Size::Small)
            .tooltip(question.text.clone())
            .media(
                gpui_kit::component::attachment::AttachmentMedia::new().child(match provider {
                    Some(provider) => ui::mark(&provider, 14.),
                    None => ui::icon(Icon::Agent, 13.)
                        .text_color(t.text_tertiary)
                        .into_any_element(),
                }),
            )
            .content(
                gpui_kit::component::attachment::AttachmentContent::new()
                    .title(gpui_kit::component::attachment::AttachmentTitle::new(label)),
            )
            .on_remove(move |_, _, cx| {
                this.update(cx, |bench, cx| {
                    if let Some(chat) = bench.chats.get_mut(&agent) {
                        chat.writing = None;
                    }
                    bench.parts.redraw(&bench.parts.stage, cx);
                })
                .ok();
            })
            .into_any_element(),
    )
}

pub(crate) fn tilde(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// How the chat's markdown looks: an answer, a thought, and what the person wrote.
struct Looks {
    /// The prose of replies, of thoughts and of the person's messages, at the size they
    /// were designed, and at the chat's text size.
    designed: Styles,
    styles: RefCell<Styles>,
    line: Hsla,
    bubble: Hsla,
    quiet: Hsla,
    faint: Hsla,
    ink: Hsla,
    hover: Hsla,
    rule: Hsla,
    accent: Hsla,
    provider: String,
    reader: Entity<Reading>,
    tints: Tints,
    bench: WeakEntity<Workbench>,
    agent: String,
    /// The pictures in the chat's replies, read from their bytes once, by part.
    pictures: RefCell<HashMap<SharedString, Option<Picture>>>,
    /// What the chat's host holds of the names its messages write.
    files: Rc<RefCell<files::Files>>,
}

#[derive(Clone)]
struct Styles {
    text: Rc<Style>,
    thought: Rc<Style>,
    user: Rc<Style>,
}

impl Looks {
    /// The agent's mark as the rail draws it, its face when it wears one, else its CLI's.
    fn mark_of(&self, agent: &str, cli: &str, size: f32, cx: &App) -> AnyElement {
        let row = (self.bench.upgrade()).and_then(|bench| bench.read(cx).agent_by_id(agent));
        match row {
            Some(row) => ui::agent_mark(&row, size, cx),
            None => ui::mark(cli, size),
        }
    }

    /// Sets the prose to `scale` times its designed size, and answers the replies' style.
    fn rescale(&self, scale: f32) -> Rc<Style> {
        let scaled = |style: &Rc<Style>| Rc::new(style.scaled(scale));
        let styles = Styles {
            text: scaled(&self.designed.text),
            thought: scaled(&self.designed.thought),
            user: scaled(&self.designed.user),
        };
        let text = styles.text.clone();
        *self.styles.borrow_mut() = styles;
        text
    }
}

/// The colours a run of tool calls is drawn in, mixed once.
struct Tints {
    looked: Hsla,
    changed: Hsla,
    deleted: Hsla,
    ran: Hsla,
    called: Hsla,
    failed: Hsla,
    added: Hsla,
    deleted_line: Hsla,
    added_ground: Hsla,
    deleted_ground: Hsla,
    added_mark: Hsla,
    deleted_mark: Hsla,
    spine: Hsla,
    tick: Hsla,
    line_soft: Hsla,
    well: Hsla,
}

impl Tints {
    fn new(t: &Tokens) -> Tints {
        let dim = t.text_tertiary;
        Tints {
            looked: theme::mix(t.git_renamed, dim, 0.76),
            changed: theme::mix(t.git_modified, dim, 0.76),
            deleted: theme::mix(t.git_deleted, dim, 0.76),
            ran: theme::mix(t.cmd, dim, 0.7),
            called: theme::mix(t.live, dim, 0.66),
            failed: t.danger,
            added: t.git_added,
            deleted_line: t.git_deleted,
            added_ground: Hsla {
                a: 0.09,
                ..t.git_added
            },
            deleted_ground: Hsla {
                a: 0.09,
                ..t.git_deleted
            },
            added_mark: Hsla {
                a: 0.26,
                ..t.git_added
            },
            deleted_mark: Hsla {
                a: 0.26,
                ..t.git_deleted
            },
            spine: t.tree_spine,
            tick: t.tree_tick,
            line_soft: t.line_soft,
            well: Hsla {
                a: 0.62,
                ..t.surface_sunken
            },
        }
    }
}

/// Colours a code block once syntax.rs has worked it out: the first draw asks for it,
/// and the chat redraws when it arrives. Plain text takes the editor's text colour.
fn code_colours(reader: Entity<Reading>, plain: Hsla) -> CodeColours {
    // The texts asked to be coloured in the theme of this version; a new theme drops the colours
    // worked out in the old one, so they are asked for again.
    let asked = RefCell::new((0, HashSet::<(SharedString, SharedString)>::new()));
    Rc::new(move |lang, code, cx| {
        let language = SharedString::from(lang?.to_lowercase());
        if !syntax::knows(&language) {
            return None;
        }
        if let Some(lines) = syntax::coloured_lines(&language, code, cx) {
            return Some(spread(&lines, code, plain));
        }
        #[cfg(feature = "counts")]
        gpui_transcript::counts::add(&gpui_transcript::counts::UNCOLOURED, 1);
        let mut asked = asked.borrow_mut();
        let version = syntax::version(cx);
        if asked.0 != version {
            *asked = (version, HashSet::new());
        }
        if asked.1.insert((language.clone(), code.clone())) {
            let coloured = syntax::colour_lines(&language, code.clone(), cx);
            let reader = reader.downgrade();
            cx.spawn(async move |cx| {
                if coloured.await.is_some() {
                    #[cfg(feature = "counts")]
                    gpui_transcript::counts::add(&gpui_transcript::counts::COLOURED, 1);
                    let _ = reader.update(cx, |_, cx| cx.notify());
                }
            })
            .detach();
        }
        None
    })
}

/// Each line's runs as runs over the whole text, with plain text filled in.
fn spread(lines: &[syntax::LineRuns], code: &str, plain: Hsla) -> CodeRuns {
    let plain_style = HighlightStyle {
        color: Some(plain),
        ..HighlightStyle::default()
    };
    let mut runs = Vec::new();
    let mut start = 0;
    for (line, runs_of_line) in code.split('\n').zip(lines) {
        let mut cursor = start;
        for (range, style) in runs_of_line {
            let (from, to) = (start + range.start, start + range.end);
            if cursor < from {
                runs.push((cursor..from, plain_style));
            }
            let color = style.color.or(Some(plain));
            runs.push((from..to, HighlightStyle { color, ..*style }));
            cursor = to;
        }
        if cursor < start + line.len() {
            runs.push((cursor..start + line.len(), plain_style));
        }
        start += line.len() + 1;
    }
    runs.into()
}

impl Looks {
    /// What reaching for a file the chat names does, once its host says the file is there.
    fn file_pressed(&self) -> files::FilePressed {
        let (bench, agent, files) = (self.bench.clone(), self.agent.clone(), self.files.clone());
        Rc::new(move |name, press, window, cx| {
            if let Some(kind) = files::Files::kind_of(&files, name, cx) {
                files::pressed(&agent, &files, &bench, (name, kind, press), window, cx)
            }
        })
    }

    fn new(
        t: &Tokens,
        provider: &str,
        reader: Entity<Reading>,
        (bench, asks): (WeakEntity<Workbench>, UnboundedSender<Ask>),
        agent: String,
    ) -> Looks {
        let files = Rc::new(RefCell::new(files::Files::default()));
        let mut mono = gpui_kit::font(theme::MONO);
        mono.fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(vec![
            theme::MONO_ICONS.into()
        ]));
        let text = Style {
            font: theme::UI.into(),
            mono,
            ink: t.text_primary,
            ink_dim: t.text_tertiary,
            ink_faint: t.text_disabled,
            accent: t.accent,
            line: Hsla { a: 0.8, ..t.line },
            line_soft: t.line_soft,
            selection: Hsla { a: 0.3, ..t.accent },
            mark_ground: Hsla { a: 0.1, ..t.accent },
            table_ground: Hsla {
                a: 0.72,
                ..t.line_soft
            },
            code_ground: Hsla {
                a: 0.72,
                ..t.surface_sunken
            },
            code: Some(code_colours(reader.clone(), theme::color(&t.editor.fg))),
            on_link: Some(Rc::new({
                let (bench, agent) = (bench.clone(), agent.clone());
                move |href: &str, window: &mut Window, cx: &mut App| {
                    link::open(href, &agent, &bench, window, cx)
                }
            })),
            file_of: Some(Rc::new({
                let files = files.clone();
                move |name: &str, cx: &mut App| files::Files::kind_of(&files, name, cx)
            })),
            file_icon: Some(Rc::new(
                |name: &str, kind, size, _: &mut Window, cx: &mut App| {
                    let (icon, glyph) = match kind {
                        gpui_transcript::FileKind::Folder => (None, crate::icons::Icon::Folder),
                        gpui_transcript::FileKind::File => {
                            (crate::file_icons::icon(name, cx), crate::icons::Icon::File)
                        }
                    };
                    Some(match icon {
                        Some(icon) => gpui_kit::img(icon).size(size).into_any_element(),
                        None => crate::ui::icon(glyph, f32::from(size))
                            .text_color(Tokens::get(cx).text_tertiary)
                            .into_any_element(),
                    })
                },
            )),
            on_file: Some(Rc::new({
                let (bench, agent, files) = (bench.clone(), agent.clone(), files.clone());
                move |name: &str, kind, press, window: &mut Window, cx: &mut App| {
                    files::pressed(&agent, &files, &bench, (name, kind, press), window, cx)
                }
            })),
            image: Some(Rc::new({
                let (bench, quiet) = (bench.clone(), t.text_tertiary);
                move |src: &str, _: &str, _: &mut Window, _: &mut App| {
                    content::image(src, &bench, &asks, quiet)
                }
            })),
            ..Style::default()
        };
        let thought = Style {
            text_size: px(12.5),
            ink: t.text_disabled,
            italic: true,
            ..text.clone()
        };
        let user = Style {
            text_size: px(13.5),
            ..text.clone()
        };
        let designed = Styles {
            text: Rc::new(text),
            thought: Rc::new(thought),
            user: Rc::new(user),
        };
        Looks {
            styles: RefCell::new(designed.clone()),
            designed,
            line: Hsla { a: 0.8, ..t.line },
            bubble: Hsla {
                a: 0.72,
                ..t.line_soft
            },
            quiet: t.text_tertiary,
            faint: t.text_disabled,
            ink: t.text_primary,
            hover: t.surface_hover,
            rule: Hsla {
                a: 0.14,
                ..t.text_primary
            },
            accent: t.accent,
            provider: provider.to_owned(),
            reader,
            tints: Tints::new(t),
            bench,
            agent,
            pictures: RefCell::default(),
            files,
        }
    }
}

/// The chat's looks with what the reader has open, for drawing its rows once.
struct Drawn<'a> {
    looks: &'a Looks,
    reading: &'a Reading,
}

impl std::ops::Deref for Drawn<'_> {
    type Target = Looks;

    fn deref(&self) -> &Looks {
        self.looks
    }
}

/// A message's row: the person's in a bubble at the right, the agent's across
/// the column. A finished turn folds its work behind a line saying how long it
/// worked, which the reader opens; its reply stays out.
fn row(
    rows: &[ChatRow],
    ix: usize,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(row) = rows.get(ix) else {
        return div().into_any_element();
    };
    let message = &row.message;
    let user = message.role == ChatRole::User;
    let fold = message.fold.as_ref();
    let folded = fold.is_some_and(|fold| !looks.reading.opened.contains(&fold.id));
    let opens = fold.filter(|fold| {
        ix == 0
            || rows[ix - 1]
                .message
                .fold
                .as_ref()
                .is_none_or(|before| before.id != fold.id)
    });
    let from = match fold {
        Some(fold) if folded && fold.last != message.id => usize::MAX,
        Some(fold) if folded => usize::try_from(fold.from).unwrap_or(0),
        _ => 0,
    };
    let shown = message.parts.iter().enumerate().filter(|(index, part)| {
        let reply = matches!(part, ChatPart::Text { .. } | ChatPart::Content { .. });
        from == 0 || outlives(part) || *index >= from && reply
    });
    let groups = runs(shown.map(|(_, part)| part));
    let (count, live) = (groups.len(), ix + 1 == rows.len() && looks.reading.running);
    let parts: Vec<AnyElement> = groups
        .into_iter()
        .enumerate()
        .map(|(at, group)| match group {
            Shown::Part(ChatPart::Notice { notice, .. })
                if let Some(card) = notice
                    .subagent
                    .as_deref()
                    .and_then(|session| card_of(rows, session)) =>
            {
                self::subagent(card, pen, looks, window, cx)
            }
            Shown::Part(part) => self::part(row, part, user, pen, looks, window, cx),
            Shown::Run(calls) => tools::run(&calls, live && at + 1 == count, looks, window, cx),
        })
        .collect();
    let summary = opens.map(|fold| work(fold, !folded, looks, cx));
    let carried = !message.attachments.is_empty() || !message.context.is_empty();
    let lost = (ix == 0 && looks.reading.earlier_lost).then(|| {
        div()
            .w_full()
            .pt(px(44.))
            .pb(px(8.))
            .flex()
            .justify_center()
            .child(
                ui::text(
                    "Earlier messages are no longer kept.",
                    12.,
                    FontWeight::NORMAL,
                )
                .text_color(Tokens::get(cx).text_tertiary),
            )
    });
    let worktree = (ix == 0)
        .then_some(looks.reading.worktree.as_ref())
        .flatten()
        .and_then(|switch| crate::worktree_switch::note(switch, cx))
        .map(|note| {
            div()
                .w_full()
                .flex()
                .justify_center()
                .when(!looks.reading.earlier_lost, |row| row.pt(px(44.)))
                .child(note)
        });
    let noted = worktree.is_some();
    if parts.is_empty() && message.letters.is_empty() && summary.is_none() && !carried {
        return div().children(lost).children(worktree).into_any_element();
    }
    let lettered = !message.letters.is_empty();
    let content = if lettered {
        letters(row, pen, looks, window, cx)
    } else if user {
        let editing = looks
            .reading
            .editing
            .as_ref()
            .filter(|editing| editing.is(&message.id));
        match editing {
            Some(editing) => div().w_full().flex().justify_end().child(edit::editor(
                &looks.agent,
                editing,
                looks,
            )),
            None => {
                let group = SharedString::from(format!("sent:{}", message.id));
                let bench = looks.bench.clone();
                let open = move |path: PathBuf, window: &mut Window, cx: &mut App| {
                    bench
                        .update(cx, |bench, cx| bench.show_picture(path, window, cx))
                        .ok();
                };
                div()
                    .group(group.clone())
                    .max_w(relative(0.8))
                    .flex()
                    .flex_col()
                    .items_end()
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(8.))
                            .border_1()
                            .border_color(looks.line)
                            .rounded(theme::RADIUS_3)
                            .bg(looks.bubble)
                            .children(attach::sent(message, open, looks.file_pressed(), cx))
                            .children(parts),
                    )
                    .children(meta::line(rows, ix, &group, looks, cx))
            }
        }
    } else {
        let group = SharedString::from(format!("answer:{}", message.id));
        div()
            .group(group.clone())
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .min_w_0()
                    .children(summary)
                    .children(parts),
            )
            .children(meta::line(rows, ix, &group, looks, cx))
    };
    let user = user && !lettered;
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .children(lost)
        .children(worktree)
        .child(
            div()
                .w_full()
                .max_w(px(800.))
                .px(px(22.))
                .map(|row| {
                    if user {
                        row.pt(px(17.)).pb(px(10.)).flex().justify_end()
                    } else {
                        row.py(px(5.))
                    }
                })
                .when(ix == 0 && !looks.reading.earlier_lost && !noted, |row| {
                    row.pt(px(56.))
                })
                .when(ix + 1 == rows.len(), |row| row.pb(px(32.)))
                .child(content),
        )
        .into_any_element()
}

/// An answer to a question as one text: what was typed, the files attached to it, then any context.
fn answer_text(text: &str, paths: &[String], context: &[ChatContext]) -> String {
    let attached = (!paths.is_empty()).then(|| {
        let listed: Vec<String> = paths.iter().map(|path| format!("- {path}")).collect();
        format!("Attached:\n{}", listed.join("\n"))
    });
    let context = context
        .iter()
        .map(|item| format!("{}:\n{}", item.title, item.text));
    std::iter::once(text.to_owned())
        .chain(attached)
        .chain(context)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A question as it reads once answered: who asked, when not the chat's own agent, its words,
/// and the answer, or only that it was answered when the answer is a secret.
fn answered_question(
    asker: Option<(&str, &str)>,
    question: &str,
    answer: &str,
    secret: bool,
    t: &Tokens,
) -> gpui_kit::Div {
    let bold = HighlightStyle {
        color: Some(t.text_primary),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    let asker = asker.map(|(cli, name)| {
        div()
            .h(px(22.))
            .flex()
            .items_center()
            .gap(px(7.))
            .text_size(crate::theme::ui_text(11.))
            .text_color(t.text_tertiary)
            .child(ui::mark(cli, 16.))
            .child(
                StyledText::new(format!("{name} asked you"))
                    .with_highlights([(0..name.len(), bold)]),
            )
    });
    let answered = if secret {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(ui::icon(Icon::Shield, 12.).text_color(t.text_disabled))
            .child("Answered, kept private")
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_wrap()
            .gap_x(px(4.))
            .child("You answered:")
            .child(div().text_color(t.text_primary).child(answer.to_owned()))
            .into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .children(asker)
        .child(
            div()
                .text_size(crate::theme::ui_text(14.))
                .text_color(t.text_primary)
                .child(question.to_owned()),
        )
        .child(
            div()
                .mt(px(6.))
                .text_size(crate::theme::ui_text(12.))
                .text_color(t.text_tertiary)
                .child(answered),
        )
}

/// Messages other agents sent and the person's answers to questions, which the core hands
/// the agent as one message: each drawn as what it is rather than as the text the agent reads.
fn letters(
    row: &ChatRow,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::Div {
    let drawn = row
        .message
        .letters
        .iter()
        .enumerate()
        .map(|(ix, letter)| match letter.kind {
            LetterKind::Answer => answered_question(
                (letter.agent_id != looks.agent)
                    .then_some((letter.cli.as_str(), letter.name.as_str())),
                &letter.question,
                &letter.text,
                letter.secret,
                Tokens::get(cx),
            )
            .into_any_element(),
            LetterKind::Message => {
                let (bench, agent) = (looks.bench.clone(), letter.agent_id.clone());
                let name = div()
                    .id(SharedString::from(format!(
                        "{}:{}:from",
                        row.message.id,
                        letter_doc(ix)
                    )))
                    .text_color(looks.accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .hover(|name| name.underline())
                    .child(letter.name.clone())
                    .on_click(move |_, _, cx| {
                        bench.update(cx, |bench, _| bench.open_agent(&agent)).ok();
                    });
                let from = div()
                    .mb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(crate::theme::ui_text(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(looks.quiet)
                            .child(looks.mark_of(&letter.agent_id, &letter.cli, 17., cx))
                            .child(div().flex().gap(px(4.)).child("Message from").child(name)),
                    )
                    .child(div().h(px(1.)).bg(looks.rule));
                let id = SharedString::from(format!("{}:{}", row.message.id, letter_doc(ix)));
                let text = row.doc(&letter_doc(ix)).map(|doc| {
                    let look = looks.styles.borrow().text.clone();
                    pen.markdown_with(&look, id, &doc.markdown, window, cx)
                });
                div()
                    .flex()
                    .flex_col()
                    .child(from)
                    .children(text)
                    .into_any_element()
            }
        });
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.))
        .min_w_0()
        .children(drawn)
}

enum Shown<'a> {
    Part(&'a ChatPart),
    Run(Vec<&'a ChatPart>),
}

/// A message's parts with each run of tool calls together; a page the agent showed stands on its own.
fn runs<'a>(parts: impl Iterator<Item = &'a ChatPart>) -> Vec<Shown<'a>> {
    let mut shown = Vec::new();
    for part in parts {
        match (part, shown.last_mut()) {
            (ChatPart::Tool { page: None, .. }, Some(Shown::Run(calls))) => calls.push(part),
            (ChatPart::Tool { page: None, .. }, _) => shown.push(Shown::Run(vec![part])),
            _ => shown.push(Shown::Part(part)),
        }
    }
    shown
}

/// A page the agent showed and a subagent still at work stay in view when the turn around them folds.
fn outlives(part: &ChatPart) -> bool {
    match part {
        ChatPart::Tool { page, .. } => page.is_some(),
        ChatPart::Subagent { subagent, .. } => subagent.state == SubagentState::Running,
        _ => false,
    }
}

/// The line a finished turn folds behind: who worked, for how long, with how many tool calls.
fn work(fold: &WorkFold, open: bool, looks: &Drawn, cx: &App) -> AnyElement {
    let verb = if fold.thought { "Thought" } else { "Worked" };
    let worked = fold.took.map_or_else(
        || verb.to_owned(),
        |took| format!("{verb} for {}", duration(took)),
    );
    let calls = (fold.calls > 0).then(|| {
        format!(
            "· {} tool {}",
            fold.calls,
            if fold.calls == 1 { "call" } else { "calls" }
        )
    });
    let (reader, id) = (looks.reader.clone(), fold.id.clone());
    let (hover, ink) = (looks.hover, looks.ink);
    let toggle = div()
        .id(SharedString::from(format!("work:{}", fold.id)))
        .h(px(22.))
        .ml(px(-6.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(looks.quiet)
        .cursor_pointer()
        .hover(move |toggle| toggle.bg(hover).text_color(ink))
        .child(looks.mark_of(&looks.agent, &looks.provider, 17., cx))
        .child(worked)
        .children(calls)
        .child(motion::turn(
            format!("work-chevron:{}", fold.id),
            motion::fold_angle(open),
            motion::DETAIL_FOLD,
            ui::icon(Icon::Chevron, 10.).text_color(looks.quiet),
        ))
        .on_click(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&id);
                cx.notify();
            })
        });
    div()
        .mt(px(3.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().flex().child(toggle))
        .child(div().h(px(1.)).bg(looks.rule))
        .into_any_element()
}

/// The card of the subagent running in `session`, as the chat last showed it; a subagent's
/// notice shows its card rather than a notice of its own.
fn card_of<'a>(rows: &'a [ChatRow], session: &str) -> Option<&'a SubagentCard> {
    rows.iter().rev().find_map(|row| {
        row.message.parts.iter().find_map(|part| match part {
            ChatPart::Subagent { subagent, .. } if subagent.session_id == session => Some(subagent),
            _ => None,
        })
    })
}

/// The box something said beside the reply sits in: a background task that ended, a notice
/// from the agent, or a move to another account.
fn notice_box(looks: &Drawn) -> Div {
    div()
        .my(px(4.))
        .px(px(10.))
        .py(px(4.))
        .flex()
        .items_center()
        .gap(px(7.))
        .min_w_0()
        .border_1()
        .border_color(looks.tints.line_soft)
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(10.5))
        .text_color(looks.quiet)
}

/// What the notice is about, giving way before its state does.
fn notice_name(name: String, mono: bool, looks: &Drawn) -> Div {
    div()
        .min_w_0()
        .truncate()
        .when(mono, |name| name.font_family(theme::MONO))
        .text_color(looks.ink)
        .child(name)
}

fn notice_summary(summary: String) -> Div {
    div().min_w_0().truncate().child(summary)
}

/// A subagent the agent started: its name, its task and how it stands, opening onto
/// what it did. A closed card draws none of its rows, though they keep arriving.
fn subagent(
    card: &SubagentCard,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = format!("subagent:{}", card.session_id);
    let open = looks.reading.opened.contains(&key);
    let (tint, mark, word) = match card.state {
        SubagentState::Running => (
            theme::mix(t.accent, t.text_disabled, 0.62),
            Icon::Hourglass,
            "working",
        ),
        SubagentState::Completed => (
            theme::mix(t.live, t.text_disabled, 0.58),
            Icon::Check,
            "done",
        ),
        SubagentState::Failed => (t.danger, Icon::Warning, "failed"),
        SubagentState::Disconnected => (t.danger, Icon::Plug, "lost"),
        SubagentState::Cancelled => (t.text_disabled, Icon::Close, "stopped"),
    };
    let tokens = card
        .tokens
        .map(|tokens| format!("{} tokens", crate::home::compact(tokens as f64)));
    let calls = (card.calls > 0).then(|| {
        format!(
            "{} {}",
            card.calls,
            if card.calls == 1 { "call" } else { "calls" }
        )
    });
    let cost = [tokens, calls]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    let task = card
        .task
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let (reader, toggled, hover) = (
        looks.reader.clone(),
        key.clone(),
        theme::mix(t.text_primary, t.surface_raised, 0.05),
    );
    let summary = div()
        .id(SharedString::from(key.clone()))
        .min_h(px(24.))
        .py(px(4.))
        .pl(px(11.))
        .pr(px(12.))
        .flex()
        .items_center()
        .gap(px(7.))
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_2)
        .when(open, |summary| summary.rounded_b_none())
        .bg(t.surface_raised)
        .text_size(crate::theme::ui_text(11.))
        .cursor_pointer()
        .hover(move |summary| summary.bg(hover))
        .on_click(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&toggled);
                cx.notify();
            })
        })
        .child(
            div()
                .flex_none()
                .opacity(if open { 1. } else { 0.5 })
                .child(motion::turn(
                    format!("{key}:chevron"),
                    motion::fold_angle(open),
                    motion::DETAIL_FOLD,
                    ui::icon(Icon::Chevron, 9.).text_color(looks.quiet),
                )),
        )
        .child(div().flex_none().child(ui::mark(&looks.provider, 18.)))
        .child(
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(10.5))
                .text_color(looks.ink)
                .child(card.name.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::theme::ui_text(10.5))
                .text_color(looks.faint)
                .map(|line| match card.state {
                    SubagentState::Running => line.child(shimmer(format!("{key}:task"), task)),
                    _ => line.child(task),
                }),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(7.))
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(9.5))
                .text_color(Hsla {
                    a: 0.8,
                    ..looks.faint
                })
                .when(!cost.is_empty(), |end| end.child(cost))
                .child(
                    div()
                        .id(SharedString::from(format!("{key}:state")))
                        .tooltip(ui::tooltip(word))
                        .child(ui::icon(mark, 12.).text_color(tint)),
                ),
        );
    let body = open.then(|| {
        let rows = looks
            .reading
            .subagents
            .get(&card.session_id)
            .map_or(&[][..], Vec::as_slice);
        let live = card.state == SubagentState::Running;
        let mut drawn = Vec::new();
        for (ix, row) in rows.iter().enumerate() {
            let groups = runs(row.message.parts.iter());
            let count = groups.len();
            for (at, group) in groups.into_iter().enumerate() {
                drawn.push(match group {
                    Shown::Part(part) => self::part(row, part, false, pen, looks, window, cx),
                    Shown::Run(calls) => tools::run(
                        &calls,
                        live && ix + 1 == rows.len() && at + 1 == count,
                        looks,
                        window,
                        cx,
                    ),
                });
            }
        }
        if drawn.is_empty() {
            drawn.push(
                div()
                    .text_size(crate::theme::ui_text(10.5))
                    .text_color(looks.faint)
                    .child("No output yet.")
                    .into_any_element(),
            );
        }
        div()
            .id(SharedString::from(format!("{key}:body")))
            .mx(px(12.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .pt(px(4.))
            .pb(px(8.))
            .pl(px(13.))
            .pr(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .border_1()
            .border_t_0()
            .border_color(t.border)
            .rounded_b(theme::RADIUS_2)
            .bg(Hsla {
                a: 0.45,
                ..t.surface_raised
            })
            .children(drawn)
    });
    div()
        .my(px(6.))
        .flex()
        .flex_col()
        .child(summary)
        .children(body)
        .into_any_element()
}

/// How long something took, as the app says it: milliseconds under a second, then seconds, minutes and hours.
fn duration(ms: u64) -> String {
    let seconds = (ms + 500) / 1000;
    match ms {
        0..1000 => format!("{ms}ms"),
        _ if seconds < 60 => format!("{seconds}s"),
        _ if seconds < 3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

fn part(
    row: &ChatRow,
    part: &ChatPart,
    user: bool,
    pen: &Pen,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = SharedString::from(format!("{}:{}", row.message.id, part.id()));
    let line = |icon: Icon, text: String, tint: Hsla| {
        div()
            .flex()
            .items_center()
            .gap(px(7.))
            .min_w_0()
            .text_size(crate::theme::ui_text(12.5))
            .text_color(tint)
            .child(ui::icon(icon, 12.).text_color(tint))
            .child(div().min_w_0().truncate().child(text))
            .into_any_element()
    };
    match part {
        ChatPart::Text { .. } | ChatPart::Thought { .. } => {
            let Some(doc) = row.doc(part.id()) else {
                return div().into_any_element();
            };
            let styles = looks.styles.borrow();
            let look = match (part, user) {
                (ChatPart::Thought { .. }, _) => styles.thought.clone(),
                (_, true) => styles.user.clone(),
                _ => styles.text.clone(),
            };
            drop(styles);
            let cut = (doc.units > LONG_TEXT && looks.reading.folds(&row.message.id, &id))
                .then(|| doc.markdown.cut(LONG_TEXT))
                .flatten();
            let Some((blocks, hidden)) = cut else {
                return pen.markdown_with(&look, id, &doc.markdown, window, cx);
            };
            let reader = looks.reader.clone();
            let label = format!("Show the rest · {}k more characters", (hidden + 500) / 1000);
            let more = Button::new(
                SharedString::from(format!("rest:{id}")),
                label,
                Variant::Outline,
            )
            .on_click({
                let id = id.clone();
                move |_, _, cx| {
                    reader.update(cx, |reading, cx| {
                        reading.unfolded.insert(id.clone());
                        cx.notify();
                    })
                }
            });
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .min_w_0()
                .child(pen.blocks_with(&look, id, &doc.markdown, blocks, window, cx))
                .child(div().flex().child(more))
                .into_any_element()
        }
        ChatPart::Tool {
            page: Some(page), ..
        } => self::page::page(page, looks, window, cx),
        ChatPart::Tool { tool, failure, .. } => {
            let tint = if failure.is_some() {
                Tokens::get(cx).danger
            } else {
                looks.quiet
            };
            line(Icon::Run, tool.title.clone(), tint)
        }
        ChatPart::Subagent { subagent, .. } => self::subagent(subagent, pen, looks, window, cx),
        ChatPart::Notice { notice, .. } => {
            let t = Tokens::get(cx);
            let icon = match notice.task_type.as_deref() {
                Some("workflow") => Icon::Workflow,
                _ => Icon::Timer,
            };
            let (state, tint) = match notice.state {
                NoticeState::Completed => ("completed", looks.quiet),
                NoticeState::Failed => ("failed", t.danger),
                NoticeState::Stopped => ("stopped", looks.quiet),
            };
            notice_box(looks)
                .child(ui::icon(icon, 12.).text_color(looks.quiet))
                .child(notice_name(notice.name.clone(), true, looks))
                .child(div().flex_none().text_color(tint).child(state))
                .children(notice.summary.clone().map(notice_summary))
                .into_any_element()
        }
        ChatPart::AgentNotice { notice, .. } => {
            let t = Tokens::get(cx);
            let (icon, tint, state) = match notice.severity {
                NoticeSeverity::Info => (Icon::Inbox, looks.quiet, looks.quiet),
                NoticeSeverity::Warning => (Icon::Warning, t.warn, looks.quiet),
                NoticeSeverity::Error => (Icon::Warning, t.danger, t.danger),
            };
            notice_box(looks)
                .child(ui::icon(icon, 12.).text_color(tint))
                .child(
                    div()
                        .flex_none()
                        .text_color(state)
                        .child(notice.title.clone()),
                )
                .children(notice.description.clone().map(notice_summary))
                .into_any_element()
        }
        ChatPart::Account { moved, .. } => {
            let why = moved
                .from
                .as_ref()
                .filter(|_| moved.reason == MoveReason::Limit)
                .map(|from| format!("{from} reached its usage limit"));
            notice_box(looks)
                .child(ui::icon(Icon::User, 12.).text_color(looks.quiet))
                .child(div().flex_none().child("Moved to"))
                .child(notice_name(moved.label.clone(), false, looks))
                .children(why.map(notice_summary))
                .into_any_element()
        }
        ChatPart::Content { content, .. } => content::content(id, content, looks, cx),
    }
}
