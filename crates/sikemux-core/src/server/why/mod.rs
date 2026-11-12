//! `why`: how something in a project came to be, read from what the core already keeps. It
//! answers a short chain of facts, newest first, each with a pointer the asking agent places after
//! the claim it supports, which the chat draws as a chip: the person's own words in this
//! project's chats, the commits that made a file or a line, the calls the person answered and the
//! rules they gave. It keeps nothing of its own, stays inside the asker's project, and passes on
//! only the person's words and the core's own facts, never another agent's tool output or
//! thinking.

use crate::server::parts::Parts;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use sikemux_fold::{ChatEvent, ChatEventKind, UpdateKind};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallClient, CallKind, CallState, PersonCall};
use sikemux_view::chat::{WhyChip, WhyKind, WhyOpen, WHY_URI};

use crate::cli::protocol::HarnessRequest;
use crate::sessions::asked::{may_hold_secret, person_prose};
use crate::text::clipped;
use crate::trailers::Agent;

use super::access::Peer;
use super::chat::history;
use super::connection::blocking;
use super::{board, memory, person_calls, who_wrote, Core};

/// The most facts one answer gives.
const MAX_FACTS: usize = 12;
/// The commits a file or some lines are walked back through.
const MAX_COMMITS: usize = 3;
/// What of the person's own words a fact quotes.
const MAX_SAID_CHARS: usize = 300;
/// How long one read may take, past which it answers with what it has.
const MAX_TIME: Duration = Duration::from_secs(2);
/// How much of each open chat is searched for the person's words.
const MAX_CHAT_BYTES: usize = 512 * 1024;
/// The person's messages a search for words gives at most.
const MAX_WORDS_FOUND: usize = 4;

/// Something a fact rests on, which a chip opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pointer {
    Commit(String),
    /// The person's words in an agent's chat, placed in its run when the core could.
    Said {
        agent: String,
        feed: Option<String>,
        index: Option<u64>,
    },
    Rule(u64),
    Call(String),
    /// What was decided, kept on the card `card`'s goal or, with none, on the project's board.
    Decision {
        card: Option<u64>,
        id: u64,
    },
}

impl Pointer {
    pub(crate) fn href(&self) -> String {
        match self {
            Pointer::Commit(hash) => format!("{WHY_URI}commit/{hash}"),
            Pointer::Said {
                agent,
                feed: Some(feed),
                index: Some(index),
            } => format!("{WHY_URI}said/{agent}/{feed}/{index}"),
            Pointer::Said { agent, .. } => format!("{WHY_URI}said/{agent}"),
            Pointer::Rule(id) => format!("{WHY_URI}rule/{id}"),
            Pointer::Call(id) => format!("{WHY_URI}call/{id}"),
            Pointer::Decision { card, id } => {
                format!("{WHY_URI}decision/{}/{id}", card.unwrap_or(0))
            }
        }
    }

    /// The pointer `href` names, when it is one.
    fn of(href: &str) -> Option<Self> {
        let rest = href.strip_prefix(WHY_URI)?;
        let parts: Vec<&str> = rest.split('/').collect();
        let plain = |part: &str| {
            !part.is_empty()
                && (part.chars()).all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        match parts.as_slice() {
            ["commit", hash] if hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit()) => {
                Some(Pointer::Commit((*hash).to_owned()))
            }
            ["said", agent] if plain(agent) => Some(Pointer::Said {
                agent: (*agent).to_owned(),
                feed: None,
                index: None,
            }),
            ["said", agent, feed, index] if plain(agent) && plain(feed) => Some(Pointer::Said {
                agent: (*agent).to_owned(),
                feed: Some((*feed).to_owned()),
                index: Some(index.parse().ok()?),
            }),
            ["rule", id] => Some(Pointer::Rule(id.parse().ok()?)),
            ["call", id] if plain(id) => Some(Pointer::Call((*id).to_owned())),
            ["decision", card, id] => Some(Pointer::Decision {
                card: Some(card.parse().ok()?).filter(|card| *card != 0),
                id: id.parse().ok()?,
            }),
            _ => None,
        }
    }
}

/// One thing the core knows, with what it rests on.
#[derive(Clone, Debug)]
struct Fact {
    said: String,
    /// When it happened, in Unix milliseconds, where the core knows.
    at: Option<u64>,
    /// The agent it comes from; none for the person and for the core.
    who: Option<String>,
    pointer: Pointer,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Shown {
    said: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    who: Option<String>,
    /// What the agent places right after the claim this fact supports.
    r#ref: String,
}

/// What the read walks back from.
struct Asked {
    /// The asking agent's project, as the core knows it, never as the caller says.
    project: String,
    /// The checkout the asking agent works in.
    checkout: PathBuf,
    about: Option<String>,
    words: Option<String>,
}

/// Gathers facts until there are enough or time runs out.
struct Facts {
    found: Vec<Fact>,
    deadline: Instant,
    /// The project's calls, read from its file once when first wanted.
    calls: Option<Vec<PersonCall>>,
}

impl Facts {
    fn add(&mut self, fact: Fact) {
        let known = (self.found.iter()).any(|found| found.pointer == fact.pointer);
        if !known && self.found.len() < MAX_FACTS {
            self.found.push(fact);
        }
    }

    fn extend(&mut self, fact: Option<Fact>) {
        if let Some(fact) = fact {
            self.add(fact);
        }
    }

    fn full(&self) -> bool {
        self.found.len() >= MAX_FACTS || Instant::now() >= self.deadline
    }

    fn calls(&mut self, core: &Core, project: &str) -> &[PersonCall] {
        self.calls
            .get_or_insert_with(|| person_calls::history(core, project).unwrap_or_default())
    }
}

/// How often one agent may ask, within [`ASKS_WINDOW`].
const MAX_ASKS: usize = 6;
const ASKS_WINDOW: Duration = Duration::from_secs(60);

/// When each agent last asked, newest last.
fn asks() -> &'static Mutex<HashMap<String, Vec<Instant>>> {
    static ASKS: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    ASKS.get_or_init(Default::default)
}

/// Whether `agent` may ask again now, counting this ask when it may.
fn may_ask(agent: &str) -> bool {
    let now = Instant::now();
    let mut asks = asks().locked();
    let times = asks.entry(agent.to_owned()).or_default();
    times.retain(|at| now.duration_since(*at) < ASKS_WINDOW);
    let room = times.len() < MAX_ASKS;
    if room {
        times.push(now);
    }
    room
}

pub(crate) async fn tool(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = (request.agent_id.clone())
        .ok_or("why answers only an agent Sikemux started, in its own project")?;
    let named = core
        .names()
        .get(&agent)
        .ok_or("why answers only an agent Sikemux knows")?;
    let text = |key: &str| {
        (request.params.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let (about, words) = (text("about"), text("words"));
    if about.is_none() && words.is_none() {
        return Err(
            "give about (a file, file:line, a commit, a call or a rule) or words the person used"
                .into(),
        );
    }
    if !may_ask(&agent) {
        return Err(format!(
            "why was asked {MAX_ASKS} times in the last minute; answer from what it gave"
        ));
    }
    asking()
        .locked()
        .entry(agent.clone())
        .or_insert_with(epoch_ms);
    let checkout = (core.chats().get(&agent))
        .map(|chat| chat.launch.cwd.clone())
        .unwrap_or_else(|| PathBuf::from(&named.project));
    let asked = Asked {
        project: named.project,
        checkout,
        about,
        words,
    };
    let (reading, asker) = (core.clone(), agent.clone());
    let (found, searched) = blocking(move || {
        count(&reading, &asker);
        Ok(read(&reading, &asked))
    })
    .await
    .map_err(|error| error.to_string())?;
    let shown: Vec<Shown> = (found.into_iter())
        .map(|fact| Shown {
            r#ref: format!("[·]({})", fact.pointer.href()),
            said: fact.said,
            at: fact.at,
            who: fact.who,
        })
        .collect();
    let mut note = match shown.is_empty() {
        true => "Sikemux keeps nothing about that. Say so plainly; don't guess.".to_owned(),
        false => "Write the answer from these facts only. Put a fact's ref right after the claim it supports; a sentence with no ref is not shown.".to_owned(),
    };
    if searched {
        note.push_str(" Words were looked for only in the project's open chats.");
    }
    Ok(json!({ "facts": shown, "note": note }))
}

/// The facts behind what `asked` names, and whether words were searched for. Reads git, the
/// chats' history files and the calls file, so it blocks; it stops at [`MAX_TIME`].
fn read(core: &Core, asked: &Asked) -> (Vec<Fact>, bool) {
    let mut facts = Facts {
        found: Vec::new(),
        deadline: Instant::now() + MAX_TIME,
        calls: None,
    };
    if let Some(about) = &asked.about {
        about_target(core, asked, about, &mut facts);
    }
    if let Some(words) = &asked.words {
        said_words(core, asked, words, &mut facts);
    }
    (facts.found, asked.words.is_some())
}

/// Whether `agent` works in `project`, so what it was told and asked may be read for an asker
/// there.
fn of_project(core: &Core, agent: &str, project: &str) -> bool {
    (core.names().get(agent)).is_some_and(|named| named.project == project)
}

fn about_target(core: &Core, asked: &Asked, about: &str, facts: &mut Facts) {
    let rule = about
        .strip_prefix("rule")
        .map(|rest| rest.trim_start_matches([' ', ':', '#']))
        .and_then(|id| id.parse::<u64>().ok());
    if let Some(id) = rule {
        rule_facts(core, asked, id, facts);
        return;
    }
    if about.contains(char::is_whitespace) {
        return;
    }
    let call = (facts.calls(core, &asked.project).iter())
        .find(|call| call.id == about)
        .cloned();
    if let Some(call) = call {
        call_facts(core, asked, &call, facts);
        return;
    }
    let path = about.rsplit_once(':').map_or(about, |(path, _)| path);
    if Path::new(path).is_absolute()
        && core.workspaces().project_of(Path::new(path)) != asked.project
    {
        return;
    }
    let Ok(commits) = who_wrote::behind(core, &asked.checkout, about, MAX_COMMITS) else {
        return;
    };
    for commit in commits {
        if facts.full() {
            break;
        }
        let agents: Vec<&Agent> = (commit.agents.iter())
            .filter(|agent| of_project(core, &agent.chat, &asked.project))
            .collect();
        facts.add(Fact {
            said: format!("Commit \"{}\"", commit.subject),
            at: Some(commit.at),
            who: agents.first().map(|agent| agent.name.clone()),
            pointer: Pointer::Commit(commit.hash.clone()),
        });
        for agent in agents {
            agent_facts(core, asked, &agent.chat, commit.at, facts);
        }
    }
}

/// What the agent `agent` of the project went by when it made something at `at`: what the
/// person last asked it, what was decided for it, the calls the person answered it, and the rules
/// the person gave it.
fn agent_facts(core: &Core, asked: &Asked, agent: &str, at: u64, facts: &mut Facts) {
    if facts.full() {
        return;
    }
    if let Some((said, pointer, when)) = asked_before(core, agent, at) {
        facts.add(Fact {
            said: format!("You said: \"{said}\""),
            at: when,
            who: None,
            pointer,
        });
    }
    let decided = (board::decisions_of(core, &asked.project).into_iter())
        .filter(|(_, decision)| {
            decision.holder.as_deref() == Some(agent) && decision.learned.at <= at
        })
        .rev()
        .take(2);
    for (card, decision) in decided.collect::<Vec<_>>() {
        facts.extend(decision_fact(card, &decision));
    }
    let answered: Vec<Fact> = (facts.calls(core, &asked.project).iter())
        .filter(|call| {
            let waited = std::iter::once(&call.asker).chain(&call.joined);
            let theirs = waited.into_iter().any(|asker| asker.agent_id == agent);
            let when = call.answer.as_ref().map(|answer| answer.at);
            theirs && when.is_some_and(|when| when <= at)
        })
        .filter_map(call_fact)
        .take(2)
        .collect();
    for fact in answered {
        facts.add(fact);
    }
    let (rules, _) = memory::for_playbook(core, &asked.project, &Peer::Local);
    let given = (rules.into_iter()).filter(|rule| {
        let said_to = rule.learned.with.as_ref().map(|with| with.agent.as_str());
        said_to == Some(agent) && rule.learned.at <= at
    });
    for rule in given.take(2) {
        facts.add(Fact {
            said: format!("Your rule: \"{}\"", rule.text),
            at: Some(rule.learned.at),
            who: None,
            pointer: Pointer::Rule(rule.id),
        });
    }
}

/// What an agent or a lead decided, said as theirs, when it holds nothing that may be a secret.
fn decision_fact(card: Option<u64>, decision: &sikemux_view::slice::Decision) -> Option<Fact> {
    let says = decision.says();
    if may_hold_secret(&says) {
        return None;
    }
    let who = decision.learned.name.clone();
    let by = who
        .as_deref()
        .map_or("An agent".to_owned(), |name| format!("@{name}"));
    Some(Fact {
        said: clipped(&format!("{by} decided: {says}"), MAX_SAID_CHARS),
        at: Some(decision.learned.at),
        who,
        pointer: Pointer::Decision {
            card,
            id: decision.id,
        },
    })
}

/// The person's answer to `call`, in words, when they gave one. A secret call, or an access
/// call, is never a fact.
fn call_fact(call: &PersonCall) -> Option<Fact> {
    if call.secret || call.kind == CallKind::Access {
        return None;
    }
    let answer = call.answer.as_ref()?;
    let picked = (answer.option.iter().chain(&answer.options))
        .filter_map(|id| call.options.iter().find(|option| option.id == *id))
        .map(|option| option.label.clone());
    let typed = answer.note.iter().chain(&answer.words).cloned();
    let said: Vec<String> = picked.chain(typed).collect();
    let how = match (&answer.by, said.is_empty()) {
        (_, true) => "you answered".to_owned(),
        (CallClient::Chat, false) => format!("you answered in the chat: {}", said.join("; ")),
        (_, false) => format!("you answered: {}", said.join("; ")),
    };
    let how = match () {
        _ if crate::server::person_calls::handed_back(call) => {
            "you handed it back to be decided without you".to_owned()
        }
        _ if crate::server::person_calls::let_stand(call) => {
            "you let the asker's assumption stand".to_owned()
        }
        _ => how,
    };
    let said = format!("Asked \"{}\", {how}", call.question);
    (!may_hold_secret(&said)).then(|| Fact {
        said: clipped(&said, MAX_SAID_CHARS),
        at: Some(answer.at),
        who: None,
        pointer: Pointer::Call(call.id.clone()),
    })
}

fn call_facts(core: &Core, asked: &Asked, call: &PersonCall, facts: &mut Facts) {
    if call.secret || call.kind == CallKind::Access {
        return;
    }
    match call_fact(call) {
        Some(fact) => facts.add(fact),
        None if call.state == CallState::Open => facts.add(Fact {
            said: format!("Asked \"{}\", still waiting on you", call.question),
            at: Some(call.raised_at),
            who: Some(call.asker.name.clone()),
            pointer: Pointer::Call(call.id.clone()),
        }),
        None => {}
    }
    if of_project(core, &call.asker.agent_id, &asked.project) {
        agent_facts(core, asked, &call.asker.agent_id, call.raised_at, facts);
    }
}

fn rule_facts(core: &Core, asked: &Asked, id: u64, facts: &mut Facts) {
    let (rules, _) = memory::for_playbook(core, &asked.project, &Peer::Local);
    let Some(rule) = rules.into_iter().find(|rule| rule.id == id) else {
        let apart = (memory::set_aside(core, &asked.project).into_iter())
            .find(|apart| apart.rule.id == id && !apart.rule.sensitive);
        if let Some(apart) = apart {
            set_aside_facts(core, &apart, facts);
        }
        return;
    };
    facts.add(Fact {
        said: format!("Your rule: \"{}\"", rule.text),
        at: Some(rule.learned.at),
        who: None,
        pointer: Pointer::Rule(rule.id),
    });
    let Some(with) = &rule.learned.with else {
        return;
    };
    if !of_project(core, &with.agent, &asked.project) {
        return;
    }
    let feed = rule.learned.feed.clone();
    let index = (feed.as_deref())
        .zip(rule.learned.turn)
        .and_then(|(feed, turn)| turn_prompt(core, feed, turn));
    let pointer = Pointer::Said {
        agent: with.agent.clone(),
        feed: index.and(feed),
        index,
    };
    facts.add(Fact {
        said: format!(
            "You said it to @{}",
            with.name.as_deref().unwrap_or("an agent")
        ),
        at: Some(rule.learned.at),
        who: None,
        pointer,
    });
}

/// A rule of the person's that left the list: what it said, and who set it aside and why.
fn set_aside_facts(core: &Core, apart: &memory::SetAside, facts: &mut Facts) {
    let pointer = match &apart.commit {
        Some(commit) => Pointer::Commit(commit.clone()),
        None => Pointer::Rule(apart.rule.id),
    };
    facts.add(Fact {
        said: format!(
            "Your rule #{} \"{}\" was set aside by @{}, {}",
            apart.rule.id,
            apart.rule.text,
            core.names().name_or_id(&apart.by),
            memory::set_aside_why(apart)
        ),
        at: Some(apart.at),
        who: Some(apart.by.clone()),
        pointer,
    });
}

/// The person's prompt that started the turn beginning at event `turn` of the chat run `feed`,
/// when the person's words started it.
fn turn_prompt(core: &Core, feed: &str, turn: u64) -> Option<u64> {
    let dir = core.chats().history_dir()?;
    let events = history::tail(dir, feed, MAX_CHAT_BYTES as u64);
    let at = events.iter().position(|(index, _)| *index == turn)?;
    (events.get(..=at)?.iter().rev())
        .take_while(|(index, event)| *index == turn || event.kind != ChatEventKind::TurnCompleted)
        .find(|(_, event)| {
            event.kind == ChatEventKind::Prompt
                && event.payload.get("person") == Some(&Value::Bool(true))
        })
        .map(|(index, _)| *index)
}

/// The person's own words in the project's open chats that hold every one of `words`, newest
/// first, and the decisions and rules that hold them.
fn said_words(core: &Core, asked: &Asked, words: &str, facts: &mut Facts) {
    let wanted: Vec<String> = (words.split_whitespace())
        .map(str::to_lowercase)
        .filter(|word| word.chars().count() > 2)
        .collect();
    if wanted.is_empty() {
        return;
    }
    let holds = |text: &str| {
        let text = text.to_lowercase();
        wanted.iter().all(|word| text.contains(word.as_str()))
    };
    let mut found = 0;
    for chat in core.chats().all() {
        if found >= MAX_WORDS_FOUND || facts.full() {
            break;
        }
        if !of_project(core, chat.agent_id(), &asked.project) {
            continue;
        }
        let said =
            (persons_said(core, chat.agent_id()).into_iter()).find(|(words, _, _)| holds(words));
        if let Some((words, pointer, at)) = said {
            facts.add(Fact {
                said: format!("You said: \"{words}\""),
                at,
                who: None,
                pointer,
            });
            found += 1;
        }
    }
    let decided = (board::decisions_of(core, &asked.project).into_iter())
        .filter(|(_, decision)| holds(&decision.says()))
        .rev()
        .take(2);
    for (card, decision) in decided.collect::<Vec<_>>() {
        facts.extend(decision_fact(card, &decision));
    }
    let (rules, _) = memory::for_playbook(core, &asked.project, &Peer::Local);
    for rule in rules.into_iter().filter(|rule| holds(&rule.text)).take(2) {
        facts.add(Fact {
            said: format!("Your rule: \"{}\"", rule.text),
            at: Some(rule.learned.at),
            who: None,
            pointer: Pointer::Rule(rule.id),
        });
    }
    let apart = (memory::set_aside(core, &asked.project).into_iter())
        .filter(|apart| !apart.rule.sensitive && holds(&apart.rule.text))
        .take(2);
    for apart in apart.collect::<Vec<_>>() {
        set_aside_facts(core, &apart, facts);
    }
}

/// What the person typed into `agent`'s chat, newest first, as the core recorded each prompt
/// they sent: never another agent's message, a step Sikemux took for them, a file they attached
/// or anything that may be a secret. Read from
/// the chat's history file, never under the chat's lock.
fn persons_said(core: &Core, agent: &str) -> Vec<(String, Pointer, Option<u64>)> {
    let Some(chat) = core.chats().get(agent) else {
        return Vec::new();
    };
    let feed = chat.feed.id().to_owned();
    (persons_said_in(core, &feed).into_iter())
        .filter_map(|said| {
            let pointer = Pointer::Said {
                agent: agent.to_owned(),
                feed: Some(feed.clone()),
                index: Some(said.index),
            };
            Some((said.words?, pointer, said.at))
        })
        .collect()
}

/// A prompt the person sent, as [`persons_said_in`] reads it back.
pub(crate) struct Said {
    pub index: u64,
    pub at: Option<u64>,
    /// What they typed, as one line; none when it may be a secret, answered a call whose words
    /// are never kept, or holds no words of theirs.
    pub words: Option<String>,
    /// It answered a call whose words are never kept.
    pub hidden: bool,
    /// It was a slash command.
    pub command: bool,
}

/// What the person typed into the chat run `feed`, newest first, as the core recorded each
/// prompt they sent. Read from the chat's history file, never under the chat's lock, whether
/// or not the chat runs now.
pub(crate) fn persons_said_in(core: &Core, feed: &str) -> Vec<Said> {
    let Some(dir) = core.chats().history_dir() else {
        return Vec::new();
    };
    (history::persons_prompts(dir, feed, MAX_CHAT_BYTES as u64).into_iter())
        .map(|said| {
            let hidden = said.answered == Some(super::chat::Answered::HiddenCall);
            Said {
                index: said.index,
                at: said.at,
                words: (!hidden).then(|| persons(&said.text)).flatten(),
                hidden,
                command: super::chat::prompt::is_command(&said.text),
            }
        })
        .collect()
}

/// What the person typed in `text`, as one line, and never anything that may be a secret.
fn persons(text: &str) -> Option<String> {
    let words = person_prose(text)?;
    (!may_hold_secret(&words)).then(|| clipped(&crate::text::one_line(&words), MAX_SAID_CHARS))
}

/// The last thing the person typed to `agent` before `at`, where in its chat, and when.
fn asked_before(core: &Core, agent: &str, at: u64) -> Option<(String, Pointer, Option<u64>)> {
    (persons_said(core, agent).into_iter()).find(|(_, _, when)| when.is_some_and(|when| when <= at))
}

/// How much of a chat's history is read back for the turn that asked `why`.
const MAX_TURN_BYTES: u64 = 512 * 1024;
/// How many of an answer's links are checked; a sentence resting on one not checked stays.
const MAX_LINKS: usize = 24;

/// The turns that called `why`, by agent: when one first did.
fn asking() -> &'static Mutex<HashMap<String, u64>> {
    static ASKING: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    ASKING.get_or_init(Default::default)
}

/// `agent` started a turn, so what an earlier turn of it asked, one that never ended, is not
/// sourced.
pub(crate) fn turn_started(agent: &str) {
    asking().locked().remove(agent);
}

/// Counts an ask of `agent` by whether the person's words started its turn or it asked by
/// itself. Reads the chat's history file, so it blocks.
fn count(core: &Core, agent: &str) {
    let by = match started_by_person(core, agent) {
        true => "why.person",
        false => "why.agent",
    };
    core.activity_log().core_used(by);
}

fn started_by_person(core: &Core, agent: &str) -> bool {
    let (Some(chat), Some(dir)) = (core.chats().get(agent), core.chats().history_dir()) else {
        return false;
    };
    history::turn_prompt(dir, chat.feed.id(), MAX_TURN_BYTES).is_some()
}

/// `agent`'s turn ended. If it called `why`, its answer is sourced off the chat's path.
pub(crate) fn turn_ended(core: &Core, agent: &str) {
    let Some(asked_at) = asking().locked().remove(agent) else {
        return;
    };
    let Some(core) = core.arc() else {
        return;
    };
    let agent = agent.to_owned();
    tokio::spawn(async move {
        let sourcing = core.clone();
        let _ = blocking(move || {
            source(&sourcing, &agent, asked_at);
            Ok(())
        })
        .await;
    });
}

/// What the claims of `agent`'s answer since it called `why` at `asked_at` rest on. The answer is
/// read from the chat's history file, never under its lock; each `why` link it holds is checked
/// against the project, and the chat is told which hold, for its fold to leave out the sentences
/// that rest on none. Reads git and files, so it blocks; it checks at most [`MAX_LINKS`] links
/// and stops at [`MAX_TIME`].
fn source(core: &Core, agent: &str, asked_at: u64) {
    let (Some(chat), Some(dir)) = (core.chats().get(agent), core.chats().history_dir()) else {
        return;
    };
    let events = history::tail(dir, chat.feed.id(), MAX_TURN_BYTES);
    let stamp = |event: &ChatEvent| {
        let at = |key| event.payload.get(key).and_then(Value::as_u64);
        at("until").or(at("at"))
    };
    let Some(start) =
        (events.iter()).position(|(_, event)| stamp(event).is_some_and(|at| at >= asked_at))
    else {
        return;
    };
    let from = events[start].0;
    let Some(until) = (events[start..].iter())
        .find(|(_, event)| event.kind == ChatEventKind::TurnCompleted)
        .map(|(index, _)| *index)
    else {
        return;
    };
    let mut answer = String::new();
    for (index, event) in &events[start..] {
        if *index > until {
            break;
        }
        let chunk = (event.kind == ChatEventKind::SessionUpdate
            && UpdateKind::within(&event.payload) == Some(UpdateKind::AgentMessageChunk))
        .then(|| event.payload.pointer("/update/content/text"))
        .flatten()
        .and_then(Value::as_str);
        match chunk {
            Some(text) => answer.push_str(text),
            None => answer.push('\n'),
        }
    }
    let mut hrefs: Vec<String> = Vec::new();
    for href in links(&answer) {
        if hrefs.len() >= MAX_LINKS {
            break;
        }
        if !hrefs.contains(&href) {
            hrefs.push(href);
        }
    }
    if hrefs.is_empty() {
        return;
    }
    let checkout = chat.launch.cwd.clone();
    let mut checking = Checking {
        core,
        project: core.workspaces().project_of(&checkout),
        checkout,
        deadline: Instant::now() + MAX_TIME,
        calls: None,
        prompts: HashMap::new(),
    };
    let mut checked = Vec::new();
    let mut chips = Vec::new();
    for href in hrefs {
        if Instant::now() >= checking.deadline {
            break;
        }
        match checking.check(&href) {
            Checked::Found(chip) => {
                chips.push(chip);
                checked.push(href);
            }
            Checked::Absent => checked.push(href),
            Checked::Unknown => {}
        }
    }
    let payload = json!({ "from": from, "until": until, "chips": chips, "checked": checked });
    chat.feed.emit(ChatEventKind::Sourced, payload);
}

/// What an answer's links are checked against, each read once when first wanted.
struct Checking<'a> {
    core: &'a Core,
    project: String,
    checkout: PathBuf,
    deadline: Instant,
    /// The project's calls, read once when first wanted; `None` within when its file could not
    /// be read.
    calls: Option<Option<Vec<PersonCall>>>,
    /// Each history file read, by its name: the first index its read stretch holds, and the
    /// indexes of the person's prompts in it.
    prompts: HashMap<String, Option<(u64, Vec<u64>)>>,
}

/// What checking one of an answer's links found.
enum Checked {
    Found(WhyChip),
    /// The core looked, and it is certainly not in the project.
    Absent,
    /// The core could not tell, so the link holds its sentence as other markup does.
    Unknown,
}

impl Checking<'_> {
    /// What `href` names, when it is in the project: a commit its checkout has, a prompt the
    /// person sent to one of its agents, one of the person's rules there, a decision kept there,
    /// or one of its calls.
    fn check(&mut self, href: &str) -> Checked {
        let chip = |kind, at, landed, open| {
            Checked::Found(WhyChip {
                href: href.to_owned(),
                kind,
                at,
                landed,
                open,
            })
        };
        let (core, project) = (self.core, self.project.as_str());
        let Some(pointer) = Pointer::of(href) else {
            return Checked::Absent;
        };
        match pointer {
            Pointer::Commit(hash) => {
                let spec = format!("{hash}^{{commit}}");
                let missing = git_line(
                    &self.checkout,
                    &["cat-file", "-e", "--end-of-options", &spec],
                )
                .is_none();
                if missing {
                    return match one_repository(&self.checkout, Path::new(project)) {
                        true => Checked::Absent,
                        false => Checked::Unknown,
                    };
                }
                let made = git_line(
                    &self.checkout,
                    &["log", "-1", "--format=%H %ct", "--end-of-options", &spec],
                );
                let Some((sha, at)) = made.as_deref().and_then(|made| made.split_once(' ')) else {
                    return Checked::Unknown;
                };
                let at = at.parse::<u64>().ok().map(|at| at * 1000);
                let landed = git_line(
                    Path::new(project),
                    &["merge-base", "--is-ancestor", sha, "HEAD"],
                )
                .is_some();
                let open = WhyOpen::Commit {
                    sha: sha.to_owned(),
                };
                chip(WhyKind::Change, at, landed, open)
            }
            Pointer::Said {
                agent,
                feed: Some(feed),
                index: Some(index),
            } => {
                let Some(named) = core.names().get(&agent) else {
                    return Checked::Unknown;
                };
                if named.project != project {
                    return Checked::Absent;
                }
                match self.persons_prompt(&agent, &feed, index) {
                    Some(true) => {}
                    Some(false) => return Checked::Absent,
                    None => return Checked::Unknown,
                }
                let open = WhyOpen::Chat {
                    agent,
                    feed: Some(feed),
                    index: Some(index),
                };
                chip(WhyKind::Said, None, false, open)
            }
            Pointer::Said { .. } => Checked::Unknown,
            Pointer::Rule(id) => {
                let (rules, _) = memory::for_playbook(core, project, &Peer::Local);
                let Some(rule) = rules.into_iter().find(|rule| rule.id == id) else {
                    return Checked::Absent;
                };
                let open = WhyOpen::Rule { id };
                chip(WhyKind::Rule, Some(rule.learned.at), false, open)
            }
            Pointer::Decision { card, id } => {
                let found = (board::decisions_of(core, project).into_iter())
                    .find(|(kept, decision)| *kept == card && decision.id == id);
                let Some((_, decision)) = found else {
                    return Checked::Absent;
                };
                let Some(agent) = (decision.learned.agent.clone()).or(decision.holder.clone())
                else {
                    return Checked::Unknown;
                };
                let open = WhyOpen::Chat {
                    agent,
                    feed: decision.learned.feed.clone(),
                    index: decision.learned.turn,
                };
                let at = Some(decision.learned.at);
                chip(WhyKind::Decision, at, false, open)
            }
            Pointer::Call(id) => {
                let calls =
                    (self.calls).get_or_insert_with(|| person_calls::history(core, project));
                let Some(calls) = calls else {
                    return Checked::Unknown;
                };
                let found = (calls.iter())
                    .find(|call| call.id == id && !call.secret && call.kind != CallKind::Access);
                let Some(call) = found else {
                    return Checked::Absent;
                };
                let at = (call.answer.as_ref()).map_or(call.raised_at, |answer| answer.at);
                let open = WhyOpen::Call { id };
                chip(WhyKind::Call, Some(at), false, open)
            }
        }
    }

    /// Whether the event `index` of `agent`'s chat run `feed` is a prompt the person sent, as
    /// the core recorded it; `None` when the core cannot tell, as when the chat is closed, has
    /// begun another run, or the event is older than the stretch of its history read.
    fn persons_prompt(&mut self, agent: &str, feed: &str, index: u64) -> Option<bool> {
        let chat = self.core.chats().get(agent)?;
        let dir = self.core.chats().history_dir()?;
        if chat.feed.id() != feed {
            return None;
        }
        let read = self.prompts.entry(feed.to_owned()).or_insert_with(|| {
            let events = history::tail(dir, feed, MAX_CHAT_BYTES as u64);
            let first = events.first()?.0;
            let prompts = (events.into_iter())
                .filter(|(_, event)| {
                    event.kind == ChatEventKind::Prompt
                        && event.payload.get("person") == Some(&Value::Bool(true))
                })
                .map(|(index, _)| index)
                .collect();
            Some((first, prompts))
        });
        let (first, prompts) = read.as_ref()?;
        (index >= *first).then(|| prompts.contains(&index))
    }
}

/// Whether `checkout` and `project` are the same git repository, worktrees included, so a commit
/// the one lacks the other lacks too. A project that is a folder of repositories is not.
fn one_repository(checkout: &Path, project: &Path) -> bool {
    let common = |dir: &Path| {
        git_line(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
    };
    common(checkout).is_some_and(|checkout| common(project) == Some(checkout))
}

/// What git prints in `dir` when it succeeds, on one line.
fn git_line(dir: &Path, args: &[&str]) -> Option<String> {
    super::edits::git(dir, args)
        .ok()
        .map(|said| said.trim().to_owned())
}

/// Every `why` link `text` holds, as written.
fn links(text: &str) -> Vec<String> {
    let opening = format!("]({WHY_URI}");
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(&opening) {
        let href = &rest[at + 2..];
        let end = href
            .find(|c: char| c == ')' || c.is_whitespace())
            .unwrap_or(href.len());
        found.push(href[..end].to_owned());
        rest = &href[end..];
    }
    found
}
